param(
    [string]$Release,
    [string]$InstallDir,
    [switch]$Yes,
    [switch]$NoModifyPath,
    [switch]$Uninstall,
    [switch]$Help
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$RepoOwner = "srothgan"
$RepoName = "claude-code-rust"
$RepoSlug = "$RepoOwner/$RepoName"
$RootPackage = "claude-code-rust"

function Show-Usage {
    @"
Usage: install.ps1 [options]

Options:
  -Release <version>      Release tag or version. Defaults to latest.
  -InstallDir <dir>      App install directory.
  -Yes                   Accept prompts.
  -NoModifyPath          Do not update the user PATH.
  -Uninstall             Remove the script install layout and user PATH entry.
  -Help                  Show this help.

Environment:
  CLAUDE_RS_RELEASE
  CLAUDE_RS_INSTALL_DIR
  CLAUDE_RS_NON_INTERACTIVE
  CLAUDE_RS_NO_MODIFY_PATH
  CLAUDE_RS_UNINSTALL
"@
}

if ($Help) {
    Show-Usage
    exit 0
}

function Test-TruthyEnv {
    param([string]$Name)
    $value = [Environment]::GetEnvironmentVariable($Name)
    return $value -match "^(1|true|TRUE|yes|YES)$"
}

if (-not $PSBoundParameters.ContainsKey("Release") -or [string]::IsNullOrWhiteSpace($Release)) {
    $Release = if ($env:CLAUDE_RS_RELEASE) { $env:CLAUDE_RS_RELEASE } else { "latest" }
}

if (-not $PSBoundParameters.ContainsKey("InstallDir") -or [string]::IsNullOrWhiteSpace($InstallDir)) {
    if ($env:CLAUDE_RS_INSTALL_DIR) {
        $InstallDir = $env:CLAUDE_RS_INSTALL_DIR
    } else {
        $localAppData = if ($env:LOCALAPPDATA) {
            $env:LOCALAPPDATA
        } else {
            [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
        }
        $InstallDir = Join-Path $localAppData "Programs\claude-rs"
    }
}

$NonInteractive = Test-TruthyEnv "CLAUDE_RS_NON_INTERACTIVE"
if ($env:CI) {
    $NonInteractive = $true
}
if (Test-TruthyEnv "CLAUDE_RS_NO_MODIFY_PATH") {
    $NoModifyPath = $true
}
if (Test-TruthyEnv "CLAUDE_RS_UNINSTALL") {
    $Uninstall = $true
}

function Fail {
    param([string]$Message)
    throw $Message
}

function Fail-Unavailable {
    [Console]::Error.WriteLine("install script is currently not available for this release")
    exit 1
}

function Resolve-ReleaseTag {
    param([string]$RequestedRelease, [string]$TempDir)
    if ([string]::IsNullOrWhiteSpace($RequestedRelease) -or $RequestedRelease -eq "latest") {
        $latestJson = Join-Path $TempDir "latest.json"
        Invoke-WebRequest -Uri "https://api.github.com/repos/$RepoSlug/releases/latest" -OutFile $latestJson
        $releaseInfo = Get-Content -LiteralPath $latestJson -Raw | ConvertFrom-Json
        if (-not $releaseInfo.tag_name) {
            Fail "could not parse latest GitHub Release tag"
        }
        return [string]$releaseInfo.tag_name
    }
    if ($RequestedRelease.StartsWith("v", [StringComparison]::Ordinal)) {
        return $RequestedRelease
    }
    return "v$RequestedRelease"
}

function Get-Target {
    $architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($architecture) {
        "X64" { return "win32-x64-msvc" }
        "Arm64" { return "win32-arm64-msvc" }
        default { Fail "unsupported Windows architecture: $architecture" }
    }
}

function Get-ArchiveName {
    param([string]$Target, [string]$Tag)
    $version = $Tag.TrimStart("v")
    return "$RootPackage-$version-$Target.zip"
}

function Get-ExpectedSha256 {
    param([string]$ChecksumsPath, [string]$ArchiveName)
    $expectedPath = "dist-install/$ArchiveName"
    foreach ($line in Get-Content -LiteralPath $ChecksumsPath) {
        if ($line -match "^([A-Fa-f0-9]{64})\s+\*?$([regex]::Escape($expectedPath))$") {
            return $Matches[1].ToLowerInvariant()
        }
    }
    Fail "SHA256SUMS does not contain $expectedPath"
}

function Assert-NoReparsePoints {
    param([string]$Path)
    $reparsePoints = Get-ChildItem -LiteralPath $Path -Recurse -Force |
        Where-Object { ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 }
    if ($reparsePoints) {
        $joined = ($reparsePoints | ForEach-Object { $_.FullName }) -join [Environment]::NewLine
        Fail "archive contains reparse points or symlinks:$([Environment]::NewLine)$joined"
    }
}

function Assert-RequiredFiles {
    param([string]$AppRoot)
    $requiredFiles = @(
        "claude-rs.exe",
        "claude-rs-bridge-bun.exe",
        "package.json",
        "THIRD-PARTY-NOTICES.md",
        "agent-sdk\package.json",
        "agent-sdk\dist\bridge.js",
        "agent-sdk\dist\types.js",
        "node_modules\@anthropic-ai\claude-agent-sdk\package.json"
    )
    foreach ($relativePath in $requiredFiles) {
        $fullPath = Join-Path $AppRoot $relativePath
        if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
            Fail "archive is missing required file: $relativePath"
        }
    }
}

function Acquire-InstallLock {
    param([string]$InstallParent)
    New-Item -ItemType Directory -Path $InstallParent -Force | Out-Null
    $lockPath = Join-Path $InstallParent ".claude-rs-install.lock"
    return [System.IO.File]::Open($lockPath, [System.IO.FileMode]::OpenOrCreate, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
}

function Replace-AppDirectory {
    param([string]$SourceApp, [string]$FinalApp)
    $backup = $null
    if (Test-Path -LiteralPath $FinalApp) {
        $backup = "$FinalApp.backup.$PID"
        Move-Item -LiteralPath $FinalApp -Destination $backup
    }

    try {
        Move-Item -LiteralPath $SourceApp -Destination $FinalApp
        if ($backup -and (Test-Path -LiteralPath $backup)) {
            Remove-Item -LiteralPath $backup -Recurse -Force
        }
    } catch {
        if ($backup -and (Test-Path -LiteralPath $backup) -and -not (Test-Path -LiteralPath $FinalApp)) {
            Move-Item -LiteralPath $backup -Destination $FinalApp
        }
        throw
    }
}

function Test-ScriptInstallDirectory {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        return $false
    }

    $packageJsonPath = Join-Path $Path "package.json"
    if (-not (Test-Path -LiteralPath $packageJsonPath -PathType Leaf)) {
        return $false
    }
    if (-not (Test-Path -LiteralPath (Join-Path $Path "claude-rs.exe") -PathType Leaf)) {
        return $false
    }
    if (-not (Test-Path -LiteralPath (Join-Path $Path "claude-rs-bridge-bun.exe") -PathType Leaf)) {
        return $false
    }

    try {
        $packageJson = Get-Content -LiteralPath $packageJsonPath -Raw | ConvertFrom-Json
        return ($packageJson.PSObject.Properties.Name -contains "name") -and $packageJson.name -eq $RootPackage
    } catch {
        return $false
    }
}

function Split-PathEntries {
    param([string]$PathValue)
    if ([string]::IsNullOrWhiteSpace($PathValue)) {
        return @()
    }
    return $PathValue.Split([IO.Path]::PathSeparator, [StringSplitOptions]::RemoveEmptyEntries)
}

function Add-UserPath {
    param([string]$Directory)
    if ($NoModifyPath) {
        return $false
    }
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @(Split-PathEntries $userPath)
    $normalizedDirectory = $Directory.TrimEnd("\")
    $remainingEntries = @($entries | Where-Object { $_.TrimEnd("\") -ine $normalizedDirectory })
    if ($entries.Count -gt 0 -and $entries[0].TrimEnd("\") -ieq $normalizedDirectory) {
        return $false
    }
    $newEntries = @($Directory) + $remainingEntries
    [Environment]::SetEnvironmentVariable("Path", ($newEntries -join [IO.Path]::PathSeparator), "User")
    return $true
}

function Remove-UserPath {
    param([string]$Directory)
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @(Split-PathEntries $userPath)
    $normalizedDirectory = $Directory.TrimEnd("\")
    $newEntries = @($entries | Where-Object { $_.TrimEnd("\") -ine $normalizedDirectory })
    if ($newEntries.Count -ne $entries.Count) {
        [Environment]::SetEnvironmentVariable("Path", ($newEntries -join [IO.Path]::PathSeparator), "User")
        return $true
    }
    return $false
}

function Prepend-ProcessPath {
    param([string]$Directory)
    $env:Path = "$Directory$([IO.Path]::PathSeparator)$env:Path"
}

function Invoke-ClaudeRs {
    param([string[]]$CommandArgs)
    & "claude-rs" @CommandArgs
}

function Warn-OtherClaudeRsCommands {
    param([string]$ExpectedCommand)
    $commands = @(Get-Command "claude-rs" -All -ErrorAction SilentlyContinue)
    foreach ($command in $commands) {
        if (-not $command.Source) {
            continue
        }
        if ($command.Source.TrimEnd("\") -ieq $ExpectedCommand.TrimEnd("\")) {
            continue
        }
        Write-Warning "another claude-rs is on PATH at $($command.Source); remove the old install if it takes precedence in new shells"
    }
}

function Uninstall-ScriptInstall {
    $installParent = Split-Path -Parent $InstallDir
    $lock = Acquire-InstallLock -InstallParent $installParent
    try {
        if (Remove-UserPath -Directory $InstallDir) {
            Write-Host "Removed $InstallDir from the user PATH. Reopen your shell for new sessions to see the change."
        }

        if (Test-Path -LiteralPath $InstallDir) {
            if (Test-ScriptInstallDirectory -Path $InstallDir) {
                Remove-Item -LiteralPath $InstallDir -Recurse -Force
                Write-Host "Removed script install directory $InstallDir"
            } else {
                Write-Warning "not removing $InstallDir because it does not look like a claude-rs script install"
            }
        }
    } finally {
        $lock.Dispose()
    }

    Write-Host "Script install uninstall complete"
}

if ($Uninstall) {
    Uninstall-ScriptInstall
    exit 0
}

$tempDir = Join-Path ([IO.Path]::GetTempPath()) "claude-rs-install-$PID"
$lockStream = $null
$pathChanged = $false

try {
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

    $target = Get-Target
    $tag = Resolve-ReleaseTag -RequestedRelease $Release -TempDir $tempDir
    $archiveName = Get-ArchiveName -Target $target -Tag $tag
    $baseUrl = "https://github.com/$RepoSlug/releases/download/$tag"
    $checksumsPath = Join-Path $tempDir "SHA256SUMS"
    $archivePath = Join-Path $tempDir $archiveName

    Write-Host "Installing claude-rs $tag for $target"
    Invoke-WebRequest -Uri "$baseUrl/SHA256SUMS" -OutFile $checksumsPath
    try {
        Invoke-WebRequest -Uri "$baseUrl/$archiveName" -OutFile $archivePath
    } catch {
        Fail-Unavailable
    }

    $expectedSha = Get-ExpectedSha256 -ChecksumsPath $checksumsPath -ArchiveName $archiveName
    $actualSha = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualSha -ne $expectedSha) {
        Fail "checksum mismatch for $archiveName"
    }

    $extractDir = Join-Path $tempDir "extract"
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDir -Force
    $topLevelDirs = @(Get-ChildItem -LiteralPath $extractDir -Directory)
    $topLevelFiles = @(Get-ChildItem -LiteralPath $extractDir -File)
    if ($topLevelDirs.Count -ne 1 -or $topLevelFiles.Count -ne 0) {
        Fail "archive extraction did not produce exactly one app directory"
    }

    $extractedApp = $topLevelDirs[0].FullName
    Assert-NoReparsePoints -Path $extractedApp
    Assert-RequiredFiles -AppRoot $extractedApp

    $installParent = Split-Path -Parent $InstallDir
    $lockStream = Acquire-InstallLock -InstallParent $installParent
    Replace-AppDirectory -SourceApp $extractedApp -FinalApp $InstallDir

    $pathChanged = Add-UserPath -Directory $InstallDir
    Prepend-ProcessPath -Directory $InstallDir

    Invoke-ClaudeRs @("--version")
    Invoke-ClaudeRs @("--help") | Out-Null
    Invoke-ClaudeRs @("doctor", "--strict")

    $resolvedCommand = Get-Command "claude-rs" -ErrorAction SilentlyContinue
    $resolved = if ($resolvedCommand) { $resolvedCommand.Source } else { $null }
    $expectedCommand = Join-Path $InstallDir "claude-rs.exe"
    if ($resolved -and ($resolved.TrimEnd("\") -ine $expectedCommand.TrimEnd("\"))) {
        Write-Warning "claude-rs resolves to $resolved instead of $expectedCommand"
    }
    Warn-OtherClaudeRsCommands -ExpectedCommand $expectedCommand
    if ($pathChanged) {
        Write-Host "Updated the user PATH. Reopen your shell to use this script install in new sessions."
    }
    Write-Host "Installed claude-rs to $InstallDir"
} finally {
    if ($lockStream) {
        $lockStream.Dispose()
    }
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
