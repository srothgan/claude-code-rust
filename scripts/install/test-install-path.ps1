Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$installerPath = Join-Path $PSScriptRoot "install.ps1"
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($installerPath, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors) { throw "Installer parser errors: $parseErrors" }
$helperNames = @(
    "Get-NormalizedPath", "Split-PathEntries", "Expand-PathEntry",
    "Add-UserPath", "Remove-UserPath", "Prepend-ProcessPath",
    "Get-ScriptInstallInfo", "Test-ScriptInstallDirectory", "Warn-OtherClaudeRsCommands",
    "Test-InstallDirectoryOverlap", "Uninstall-ScriptInstall", "Acquire-InstallLock", "Fail",
    "Resolve-NpmInstallChoice"
)
$definitions = @($ast.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $helperNames -contains $node.Name
}, $true))
if ($definitions.Count -ne $helperNames.Count) { throw "Missing PATH helpers" }
Invoke-Expression (($definitions | ForEach-Object { $_.Extent.Text }) -join [Environment]::NewLine)

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Get-UserPathRaw { return $script:UserPath }
function Set-UserPathRaw { param([string]$Value) $script:UserPath = $Value }
function Write-WarnLine {
    param([string]$Message, [string[]]$Detail = @())
    $script:Warnings.Add(($Message + "`n" + ($Detail -join "`n")))
}
function Write-Ok { param([string]$Message) $script:Completed.Add($Message) }
function Confirm-DefaultNo {
    param([string]$Prompt)
    if ($NonInteractive) { return $false }
    $script:Questions.Add($Prompt)
    if ($script:OnConfirm) { & $script:OnConfirm }
    return $script:Answer
}
function Get-NpmInstall { return $script:NpmInstall }
function Remove-NpmInstall { param($NpmInstall) $script:NpmRemovals++ }
function Get-Command {
    [CmdletBinding()]
    param([string]$Name, [switch]$All)
    if ($Name -ne "claude-rs") { throw "Unexpected command discovery: $Name" }
    return $script:Commands
}
function New-CommandRecord {
    param([string]$Path)
    return [pscustomobject]@{
        Name = "claude-rs.exe"
        Path = $Path
        CommandType = [System.Management.Automation.CommandTypes]::Application
    }
}

$sandbox = Join-Path ([IO.Path]::GetTempPath()) "claude-rs-path-test-$PID"
$previousPath = $env:Path
$previousTestPath = $env:CLAUDE_RS_PATH_TEST
$NoModifyPath = $false
$RootPackage = "claude-code-rust"
$Yes = $false
$Update = $false
$NonInteractive = $false
$KeepNpm = $false
$RemoveNpm = $false
$script:Answer = $false
$script:OnConfirm = $null
$script:UserPath = ""
$script:NpmInstall = $null
$script:NpmRemovals = 0
$script:Warnings = New-Object System.Collections.Generic.List[string]
$script:Completed = New-Object System.Collections.Generic.List[string]
$script:Questions = New-Object System.Collections.Generic.List[string]
try {
    New-Item -ItemType Directory -Path $sandbox | Out-Null
    $currentDirectory = Join-Path $sandbox "current"
    $otherDirectory = Join-Path $sandbox "older copy's app"
    $unknownDirectory = Join-Path $sandbox "unknown"
    foreach ($directory in @($currentDirectory, $otherDirectory, $unknownDirectory)) {
        New-Item -ItemType Directory -Path $directory | Out-Null
        [IO.File]::WriteAllText((Join-Path $directory "claude-rs.exe"), "fixture")
    }
    [IO.File]::WriteAllText((Join-Path $otherDirectory "package.json"), '{"name":"claude-code-rust","version":"1.2.3"}')
    [IO.File]::WriteAllText((Join-Path $otherDirectory "claude-rs-bridge-bun.exe"), "fixture")
    $expected = Join-Path $currentDirectory "claude-rs.exe"
    $InstallDir = $currentDirectory

    # The same executable reported repeatedly, with different spelling, is one installation.
    $script:Commands = @(
        (New-CommandRecord $expected),
        (New-CommandRecord ($expected.Replace('\', '/'))),
        (New-CommandRecord ($expected.ToUpperInvariant())),
        (New-CommandRecord (Join-Path (Join-Path $currentDirectory ".") "claude-rs.exe"))
    )
    Warn-OtherClaudeRsCommands -ExpectedCommand $expected.Replace('\', '/')
    Assert-True ($script:Warnings.Count -eq 0) "This installation warned about itself"

    # Replacing a previous version in the same directory does not create another installation.
    [IO.File]::WriteAllText($expected, "replacement")
    Warn-OtherClaudeRsCommands -ExpectedCommand $expected
    Assert-True ($script:Warnings.Count -eq 0) "A reinstall in the same directory raised a warning"

    if ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT) {
        $junction = Join-Path $sandbox "alias"
        New-Item -ItemType Junction -Path $junction -Target $currentDirectory | Out-Null
        $script:Commands = @((New-CommandRecord (Join-Path $junction "claude-rs.exe")))
        Warn-OtherClaudeRsCommands -ExpectedCommand $expected
        Assert-True ($script:Warnings.Count -eq 0) "A junction to this installation raised a warning"
    }

    $otherCommand = Join-Path $otherDirectory "claude-rs.exe"
    $unknownCommand = Join-Path $unknownDirectory "claude-rs.exe"
    $script:Commands = @(
        (New-CommandRecord $expected),
        (New-CommandRecord $otherCommand),
        (New-CommandRecord $otherCommand.ToUpperInvariant()),
        (New-CommandRecord $unknownCommand)
    )
    Warn-OtherClaudeRsCommands -ExpectedCommand $expected
    Assert-True ($script:Warnings.Count -eq 2) "Different installations were missed or duplicated"
    Assert-True (-not $script:Warnings[0].Contains("use this installer with")) "Interactive cleanup still included a manual uninstall command"
    Assert-True $script:Warnings[1].Contains("tool that installed it") "Unknown installation received inappropriate cleanup advice"
    Assert-True (-not ($script:Warnings -join "`n").Contains("npm uninstall")) "PATH discovery guessed npm ownership"
    Assert-True ($script:Questions.Count -eq 1 -and $script:Questions[0].Contains($otherDirectory)) "Cleanup did not offer exactly the recognized installation with its full path"
    Assert-True (Test-Path -LiteralPath $otherCommand) "Declining cleanup removed the other installation"

    foreach ($mode in @("Yes", "Update", "NonInteractive")) {
        $Yes = $mode -eq "Yes"
        $Update = $mode -eq "Update"
        $NonInteractive = $mode -eq "NonInteractive"
        $script:Answer = $true
        $script:Questions.Clear()
        Warn-OtherClaudeRsCommands -ExpectedCommand $expected
        Assert-True (Test-Path -LiteralPath $otherCommand) "$mode removed another installation without an explicit answer"
        Assert-True ($script:Questions.Count -eq 0) "$mode displayed an optional cleanup prompt"
    }
    $Yes = $false
    $Update = $false
    $NonInteractive = $false
    $script:Questions.Clear()
    $separator = [IO.Path]::PathSeparator
    $script:UserPath = $currentDirectory + $separator + $otherDirectory + $separator + $unknownDirectory
    $cleanupLock = Acquire-InstallLock -InstallParent $sandbox
    try {
        Warn-OtherClaudeRsCommands -ExpectedCommand $expected
        Assert-True (Test-Path -LiteralPath $otherCommand) "Cleanup ignored another installer's active lock"
        Assert-True (($script:Warnings -join "`n").Contains("Could not uninstall")) "Optional cleanup failure was not explained"
    } finally { $cleanupLock.Dispose() }
    $script:OnConfirm = {
        [IO.File]::WriteAllText((Join-Path $otherDirectory "package.json"), '{"name":"another-package"}')
    }
    Warn-OtherClaudeRsCommands -ExpectedCommand $expected
    Assert-True (Test-Path -LiteralPath $otherCommand) "Cleanup failed to recheck ownership after confirmation"
    Assert-True ($script:UserPath.Contains($otherDirectory)) "Rejected cleanup removed the other PATH entry"
    $script:OnConfirm = $null
    [IO.File]::WriteAllText((Join-Path $otherDirectory "package.json"), '{"name":"claude-code-rust","version":"1.2.3"}')
    $script:Questions.Clear()
    Warn-OtherClaudeRsCommands -ExpectedCommand $expected
    Assert-True (-not (Test-Path -LiteralPath $otherDirectory)) "Accepting cleanup did not uninstall the recognized copy"
    Assert-True (Test-Path -LiteralPath $expected) "Cleanup removed the newly installed copy"
    Assert-True (Test-Path -LiteralPath $unknownCommand) "Cleanup removed an unknown copy"
    Assert-True ($script:UserPath -ceq ($currentDirectory + $separator + $unknownDirectory)) "Cleanup removed the wrong user PATH entry"
    Assert-True ($script:Questions.Count -eq 1) "Duplicate PATH aliases produced multiple removal prompts"
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $sandbox ".claude-rs-install.lock"))) "Cleanup left an installation lock"

    # Nested installations cannot be removed through another install's cleanup prompt.
    $nestedDirectory = Join-Path $currentDirectory "nested"
    New-Item -ItemType Directory -Path $nestedDirectory | Out-Null
    foreach ($file in @("claude-rs.exe", "claude-rs-bridge-bun.exe")) {
        [IO.File]::WriteAllText((Join-Path $nestedDirectory $file), "fixture")
    }
    [IO.File]::WriteAllText((Join-Path $nestedDirectory "package.json"), '{"name":"claude-code-rust"}')
    $script:Commands = @((New-CommandRecord (Join-Path $nestedDirectory "claude-rs.exe")))
    $script:Questions.Clear()
    Warn-OtherClaudeRsCommands -ExpectedCommand $expected
    Assert-True ($script:Questions.Count -eq 0 -and (Test-Path -LiteralPath $nestedDirectory)) "Cleanup offered to delete an overlapping installation"

    # Explicit uninstall also validates ownership before changing PATH.
    $script:UserPath = $unknownDirectory
    Uninstall-ScriptInstall -Directory $unknownDirectory
    Assert-True ((Test-Path -LiteralPath $unknownCommand) -and $script:UserPath -ceq $unknownDirectory) "Uninstall changed an unrecognized installation"

    # npm owns its own removal flow; optional prompts never turn -Yes into cleanup.
    $script:NpmInstall = [pscustomobject]@{ Version = "1.2.3"; Root = (Join-Path $sandbox "npm") }
    $script:Answer = $false
    Resolve-NpmInstallChoice
    Assert-True ($script:NpmRemovals -eq 0) "Declining npm cleanup removed the package"
    $script:Answer = $true
    foreach ($mode in @("Yes", "KeepNpm", "NonInteractive")) {
        $Yes = $mode -eq "Yes"
        $KeepNpm = $mode -eq "KeepNpm"
        $NonInteractive = $mode -eq "NonInteractive"
        Resolve-NpmInstallChoice
        Assert-True ($script:NpmRemovals -eq 0) "$mode unexpectedly removed the npm package"
    }
    $Yes = $false
    $KeepNpm = $false
    $NonInteractive = $false
    Resolve-NpmInstallChoice
    Assert-True ($script:NpmRemovals -eq 1) "Accepting npm cleanup did not use the npm removal flow"
    $RemoveNpm = $true
    $Yes = $true
    Resolve-NpmInstallChoice
    Assert-True ($script:NpmRemovals -eq 2) "Explicit -RemoveNpm did not use the npm removal flow"
    $RemoveNpm = $false
    $Yes = $false
    $realNpmRemoval = $ast.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq "Remove-NpmInstall"
    }, $true)
    Invoke-Expression $realNpmRemoval.Extent.Text
    Remove-NpmInstall -NpmInstall ([pscustomobject]@{ Npm = { throw "fixture npm uninstall failure" } })
    Assert-True (($script:Warnings -join "`n").Contains("fixture npm uninstall failure")) "npm removal exception failed the completed installation or lacked an explanation"

    $script:Warnings.Clear()
    $script:Commands = @([pscustomobject]@{
        Name = "claude-rs"
        CommandType = [System.Management.Automation.CommandTypes]::Alias
    })
    Warn-OtherClaudeRsCommands -ExpectedCommand $expected
    Assert-True ($script:Warnings.Count -eq 1 -and $script:Warnings[0].Contains("Alias")) "A shadowing shell command was silently ignored"

    # Updating PATH uses the same identity, preserving raw expandable entries when already first.
    $env:CLAUDE_RS_PATH_TEST = $currentDirectory
    $separator = [IO.Path]::PathSeparator
    $script:UserPath = '%CLAUDE_RS_PATH_TEST%/' + $separator + $unknownDirectory
    $unchangedPath = $script:UserPath
    Assert-True (-not (Add-UserPath $currentDirectory)) "An equivalent first PATH entry was replaced"
    Assert-True ($script:UserPath -ceq $unchangedPath) "Raw expandable PATH text changed unnecessarily"
    $env:Path = $currentDirectory + $separator + $currentDirectory.Replace('\', '/') + $separator + $unknownDirectory
    Prepend-ProcessPath $currentDirectory
    Assert-True (@(Split-PathEntries $env:Path).Count -eq 2) "Process PATH kept duplicate aliases of this installation"
    Assert-True (Remove-UserPath $currentDirectory) "Equivalent PATH entry was not removed on uninstall"
    Assert-True ($script:UserPath -ceq $unknownDirectory) "Uninstall removed an unrelated PATH entry"
    Write-Output "PowerShell installation identity, PATH updates, and confirmed script/npm cleanup passed"
} finally {
    $env:Path = $previousPath
    $env:CLAUDE_RS_PATH_TEST = $previousTestPath
    # Remove the test junction itself before deleting the fixture directory.
    if ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT) {
        $junction = Join-Path $sandbox "alias"
        if (Test-Path -LiteralPath $junction) { [IO.Directory]::Delete($junction) }
    }
    $resolvedSandbox = [IO.Path]::GetFullPath($sandbox)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([char[]]@('\', '/')) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolvedSandbox.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) { throw "Fixture directory is outside the temporary directory" }
    Remove-Item -LiteralPath $resolvedSandbox -Recurse -Force
}
