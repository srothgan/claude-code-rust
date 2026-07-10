param(
    [string]$Release,
    [string]$InstallDir,
    [switch]$NoModifyPath,
    [switch]$Verify,
    [switch]$Run,
    [switch]$RemoveNpm,
    [switch]$KeepNpm,
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
  -NoModifyPath          Do not update the user PATH.
  -Verify                Run strict runtime diagnostics after install.
  -Run                   Start claude-rs after a successful install.
  -RemoveNpm             Remove an existing global npm install when found.
  -KeepNpm               Keep an existing global npm install without prompting.
  -Uninstall             Remove the script install layout and user PATH entry.
  -Help                  Show this help.

Environment:
  CLAUDE_RS_RELEASE
  CLAUDE_RS_INSTALL_DIR
  CLAUDE_RS_NON_INTERACTIVE
  CLAUDE_RS_NO_MODIFY_PATH
  CLAUDE_RS_VERIFY
  CLAUDE_RS_RUN
  CLAUDE_RS_REMOVE_NPM
  CLAUDE_RS_KEEP_NPM
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
if (Test-TruthyEnv "CLAUDE_RS_VERIFY") {
    $Verify = $true
}
if (Test-TruthyEnv "CLAUDE_RS_RUN") {
    $Run = $true
}
if (Test-TruthyEnv "CLAUDE_RS_REMOVE_NPM") {
    $RemoveNpm = $true
}
if (Test-TruthyEnv "CLAUDE_RS_KEEP_NPM") {
    $KeepNpm = $true
}
if (Test-TruthyEnv "CLAUDE_RS_UNINSTALL") {
    $Uninstall = $true
}
if ($RemoveNpm -and $KeepNpm) {
    throw "-RemoveNpm and -KeepNpm cannot be used together"
}

$UseColor = -not $env:NO_COLOR
$OkMark = [char]0x2713
$FailMark = [char]0x2717

function Write-InstallerLine {
    param(
        [string]$Mark,
        [string]$Message,
        [ConsoleColor]$Color
    )
    if ($UseColor) {
        Write-Host "$Mark $Message" -ForegroundColor $Color
    } else {
        Write-Host "$Mark $Message"
    }
}

function Write-Ok {
    param([string]$Message)
    Write-InstallerLine -Mark $OkMark -Message $Message -Color Green
}

function Write-WarnLine {
    param([string]$Message)
    if ($UseColor) {
        $previousColor = [Console]::ForegroundColor
        [Console]::ForegroundColor = [ConsoleColor]::Yellow
        [Console]::Error.WriteLine("! $Message")
        [Console]::ForegroundColor = $previousColor
    } else {
        [Console]::Error.WriteLine("! $Message")
    }
}

function Write-WarnDetail {
    param([string]$Message)
    [Console]::Error.WriteLine($Message)
}

function Write-FailLine {
    param([string]$Message)
    if ($UseColor) {
        $previousColor = [Console]::ForegroundColor
        [Console]::ForegroundColor = [ConsoleColor]::Red
        [Console]::Error.WriteLine("$FailMark $Message")
        [Console]::ForegroundColor = $previousColor
    } else {
        [Console]::Error.WriteLine("$FailMark $Message")
    }
}

function Test-CanPrompt {
    return (-not $NonInteractive) -and (-not [Console]::IsInputRedirected)
}

function Confirm-DefaultNo {
    param([string]$Prompt)
    if (-not (Test-CanPrompt)) {
        return $false
    }
    Write-Host -NoNewline "$Prompt [y/N] "
    $answer = [Console]::ReadLine()
    return $answer -match "^(y|Y|yes|YES)$"
}

function Fail {
    param([string]$Message)
    Write-FailLine $Message
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

function Get-NpmCommand {
    $npm = Get-Command "npm.cmd" -ErrorAction SilentlyContinue
    if (-not $npm) {
        $npm = Get-Command "npm" -ErrorAction SilentlyContinue
    }
    if (-not $npm) {
        return $null
    }
    if ($npm.Source) {
        return $npm.Source
    }
    return $npm.Name
}

function Get-NpmInstall {
    $npm = Get-NpmCommand
    if (-not $npm) {
        return $null
    }

    try {
        $npmRoot = (& $npm "root" "-g" 2>$null | Select-Object -First 1).Trim()
    } catch {
        return $null
    }
    if ([string]::IsNullOrWhiteSpace($npmRoot)) {
        return $null
    }

    $packageJsonPath = Join-Path (Join-Path $npmRoot $RootPackage) "package.json"
    if (-not (Test-Path -LiteralPath $packageJsonPath -PathType Leaf)) {
        return $null
    }

    try {
        $packageJson = Get-Content -LiteralPath $packageJsonPath -Raw | ConvertFrom-Json
        $version = if ($packageJson.PSObject.Properties.Name -contains "version") {
            [string]$packageJson.version
        } else {
            "unknown"
        }
        return [pscustomobject]@{
            Npm = $npm
            Root = $npmRoot
            PackageJson = $packageJsonPath
            Version = $version
        }
    } catch {
        return [pscustomobject]@{
            Npm = $npm
            Root = $npmRoot
            PackageJson = $packageJsonPath
            Version = "unknown"
        }
    }
}

function Remove-NpmInstall {
    param($NpmInstall)
    & $NpmInstall.Npm "uninstall" "-g" $RootPackage | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Fail "could not remove npm install. Run manually: npm uninstall -g $RootPackage"
    }
    Write-Ok "Removed npm install"
}

function Resolve-NpmInstallChoice {
    $npmInstall = Get-NpmInstall
    if (-not $npmInstall) {
        return
    }

    Write-WarnLine "Existing npm install found: $RootPackage $($npmInstall.Version)"
    if ($RemoveNpm) {
        Remove-NpmInstall -NpmInstall $npmInstall
        return
    }

    if (-not $KeepNpm -and (Confirm-DefaultNo "Remove the npm install so only this installer owns ``claude-rs`` on PATH?")) {
        Remove-NpmInstall -NpmInstall $npmInstall
        return
    }

    Write-WarnLine "Existing npm install kept. Remove later with: npm uninstall -g $RootPackage"
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

function Invoke-InstalledClaudeRs {
    param([string[]]$CommandArgs)
    $installedExe = Join-Path $InstallDir "claude-rs.exe"
    & $installedExe @CommandArgs
}

function Assert-InstalledCommand {
    $versionOutput = (Invoke-InstalledClaudeRs @("--version") | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($versionOutput)) {
        Fail "installed claude-rs did not run successfully"
    }
    Invoke-InstalledClaudeRs @("--help") | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Fail "installed claude-rs help check failed"
    }
    Write-Ok "Verified $versionOutput"
}

function Invoke-InstallDoctor {
    $doctorOutput = (Invoke-InstalledClaudeRs @("doctor", "--strict") 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        if (-not [string]::IsNullOrWhiteSpace($doctorOutput)) {
            Write-Host $doctorOutput
        }
        Fail "runtime diagnostics failed"
    }
    Write-Ok "Runtime diagnostics passed"
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
        Write-WarnLine "Another claude-rs is also on PATH: $($command.Source)"
        Write-WarnDetail "  If a new shell runs that copy, remove it with: npm uninstall -g $RootPackage"
    }
}

function Uninstall-ScriptInstall {
    $installParent = Split-Path -Parent $InstallDir
    $lock = Acquire-InstallLock -InstallParent $installParent
    try {
        if (Remove-UserPath -Directory $InstallDir) {
            Write-Ok "Removed $InstallDir from the user PATH"
        }

        if (Test-Path -LiteralPath $InstallDir) {
            if (Test-ScriptInstallDirectory -Path $InstallDir) {
                Remove-Item -LiteralPath $InstallDir -Recurse -Force
                Write-Ok "Removed script install directory $InstallDir"
            } else {
                Write-WarnLine "Not removing $InstallDir because it does not look like a claude-rs script install"
            }
        }
    } finally {
        $lock.Dispose()
    }

    Write-Ok "Script install uninstall complete"
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
    $targetLabel = if ($target -eq "win32-x64-msvc") { "Windows x64" } else { "Windows arm64" }
    Write-Host "Installing claude-rs"
    Write-Host
    Write-Ok "$targetLabel detected"
    Write-Ok "Install location: $InstallDir"

    $tag = Resolve-ReleaseTag -RequestedRelease $Release -TempDir $tempDir
    $archiveName = Get-ArchiveName -Target $target -Tag $tag
    $baseUrl = "https://github.com/$RepoSlug/releases/download/$tag"
    $checksumsPath = Join-Path $tempDir "SHA256SUMS"
    $archivePath = Join-Path $tempDir $archiveName

    Write-Ok "Release $tag selected"
    Resolve-NpmInstallChoice

    Invoke-WebRequest -Uri "$baseUrl/SHA256SUMS" -OutFile $checksumsPath
    try {
        Invoke-WebRequest -Uri "$baseUrl/$archiveName" -OutFile $archivePath
    } catch {
        Fail-Unavailable
    }
    Write-Ok "Downloaded release archive"

    $expectedSha = Get-ExpectedSha256 -ChecksumsPath $checksumsPath -ArchiveName $archiveName
    $actualSha = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualSha -ne $expectedSha) {
        Fail "checksum mismatch for $archiveName"
    }
    Write-Ok "Verified release archive integrity"

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
    Write-Ok "Installed files"

    $pathChanged = Add-UserPath -Directory $InstallDir
    Prepend-ProcessPath -Directory $InstallDir
    if ($pathChanged) {
        Write-Ok "Updated PATH for new shells"
    } elseif ($NoModifyPath) {
        Write-WarnLine "PATH update skipped"
    } else {
        Write-Ok "PATH already points to this script install"
    }

    Assert-InstalledCommand
    if ($Verify) {
        Invoke-InstallDoctor
    }

    $resolvedCommand = Get-Command "claude-rs" -ErrorAction SilentlyContinue
    $resolved = if ($resolvedCommand) { $resolvedCommand.Source } else { $null }
    $expectedCommand = Join-Path $InstallDir "claude-rs.exe"
    if ($resolved -and ($resolved.TrimEnd("\") -ine $expectedCommand.TrimEnd("\"))) {
        Write-WarnLine "claude-rs resolves to $resolved instead of $expectedCommand"
    }
    Warn-OtherClaudeRsCommands -ExpectedCommand $expectedCommand

    Write-Host
    Write-Host "claude-rs is installed."
    if ($pathChanged) {
        Write-Host "PATH is updated for new shells."
    }
    if ($Run -or (Confirm-DefaultNo "Start claude-rs now?")) {
        Invoke-InstalledClaudeRs @()
    } elseif ($NoModifyPath) {
        Write-Host "Run directly: $(Join-Path $InstallDir "claude-rs.exe")"
    } else {
        Write-Host "Run in a new shell: claude-rs"
    }
} finally {
    if ($lockStream) {
        $lockStream.Dispose()
    }
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
