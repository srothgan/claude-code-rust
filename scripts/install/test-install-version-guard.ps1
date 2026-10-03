Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$installerPath = Join-Path $PSScriptRoot "install.ps1"
$tokens = $null
$parseErrors = $null
$installerAst = [System.Management.Automation.Language.Parser]::ParseFile(
    $installerPath,
    [ref]$tokens,
    [ref]$parseErrors
)
if ($parseErrors) {
    throw "install.ps1 contains parser errors: $($parseErrors -join '; ')"
}

$helperNames = @(
    "Test-CanPrompt",
    "Confirm-DefaultNo",
    "Get-ReleaseVersion",
    "Get-MatchingReleaseAdvisories",
    "Get-ScriptInstallInfo",
    "Test-ScriptInstallDirectory",
    "Confirm-SameVersionReinstall"
)
$helperDefinitions = @($installerAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $helperNames -contains $node.Name
}, $true))
if ($helperDefinitions.Count -ne $helperNames.Count) {
    $loadedNames = @($helperDefinitions | ForEach-Object { $_.Name })
    $missingNames = @($helperNames | Where-Object { $loadedNames -notcontains $_ })
    throw "Could not load installer version-guard helpers: $($missingNames -join ', ')"
}
Invoke-Expression (($helperDefinitions | ForEach-Object { $_.Extent.Text }) -join [Environment]::NewLine)

# The decision helper only needs the prompt result. Progress lifecycle behavior
# is covered independently by test-install-progress.ps1.
function Stop-InstallerProgress {}

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) {
        throw $Message
    }
}

function Assert-Equal {
    param($Expected, $Actual, [string]$Message)
    if ($Expected -cne $Actual) {
        throw "$Message (expected '$Expected', got '$Actual')"
    }
}

function New-OwnedInstall {
    param([string]$Path, [string]$Version)
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
    $packageJson = [pscustomobject]@{
        name = "claude-code-rust"
        version = $Version
    } | ConvertTo-Json
    [IO.File]::WriteAllText((Join-Path $Path "package.json"), "$packageJson$([Environment]::NewLine)")
    [IO.File]::WriteAllText((Join-Path $Path "claude-rs.exe"), "existing binary")
    [IO.File]::WriteAllText((Join-Path $Path "claude-rs-bridge-bun.exe"), "existing runtime")
}

function Invoke-InstallerScenario {
    param(
        [string]$RequestedRelease,
        [ValidateSet("Default", "Update", "Yes", "Latest")]
        [string]$Mode = "Default",
        [string]$Advisories
    )

    $sandbox = Join-Path ([IO.Path]::GetTempPath()) "claude-rs-version-guard-test-$PID-$([Guid]::NewGuid().ToString('N'))"
    $installDir = Join-Path $sandbox "install"
    $wrapperPath = Join-Path $sandbox "invoke-installer.ps1"
    $downloadLog = Join-Path $sandbox "downloads.log"
    $advisoriesFile = Join-Path $sandbox "advisories-fixture.json"
    $selectedVersion = "9.8.7-preview.1+build.5"
    New-OwnedInstall -Path $installDir -Version $selectedVersion
    if ($Advisories) {
        [IO.File]::WriteAllText($advisoriesFile, $Advisories)
    }

    $wrapper = @'
param(
    [string]$InstallerPath,
    [string]$InstallDir,
    [string]$RequestedRelease,
    [string]$Mode,
    [string]$DownloadLog,
    [string]$LatestTag,
    [string]$AdvisoriesFile
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
Get-ChildItem Env:CLAUDE_RS_* -ErrorAction SilentlyContinue | Remove-Item -ErrorAction SilentlyContinue
$env:CLAUDE_RS_NON_INTERACTIVE = "1"

function Invoke-WebRequest {
    param([string]$Uri, [string]$OutFile, [int]$TimeoutSec)
    Add-Content -LiteralPath $DownloadLog -Value $Uri
    if ($Uri.EndsWith("/release-advisories.json", [StringComparison]::Ordinal)) {
        Copy-Item -LiteralPath $AdvisoriesFile -Destination $OutFile
        return
    }
    if ($Mode -eq "Latest" -and $Uri.EndsWith("/releases/latest", [StringComparison]::Ordinal)) {
        [IO.File]::WriteAllText($OutFile, "{`"tag_name`":`"$LatestTag`"}")
        return
    }
    throw "mock release payload download blocked: $Uri"
}

$installerArgs = @{
    Release = $RequestedRelease
    InstallDir = $InstallDir
    NoModifyPath = $true
    KeepNpm = $true
}
if ($Mode -eq "Update") {
    $installerArgs["Update"] = $true
}
if ($Mode -eq "Yes") {
    $installerArgs["Yes"] = $true
}
& $InstallerPath @installerArgs
exit $LASTEXITCODE
'@
    [IO.File]::WriteAllText($wrapperPath, $wrapper)

    $engine = (Get-Process -Id $PID).Path
    try {
        $previousErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $outputLines = @(& $engine -NoLogo -NoProfile -File $wrapperPath `
                -InstallerPath $installerPath `
                -InstallDir $installDir `
                -RequestedRelease $RequestedRelease `
                -Mode $Mode `
                -DownloadLog $downloadLog `
                -LatestTag "v$selectedVersion" `
                -AdvisoriesFile $advisoriesFile 2>&1)
            $status = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $previousErrorActionPreference
        }
        $downloads = @()
        if (Test-Path -LiteralPath $downloadLog -PathType Leaf) {
            $downloads = @(Get-Content -LiteralPath $downloadLog)
        }
        $installedPackage = Get-Content -LiteralPath (Join-Path $installDir "package.json") -Raw | ConvertFrom-Json
        return [pscustomobject]@{
            Status = $status
            Output = (($outputLines | ForEach-Object { [string]$_ }) -join [Environment]::NewLine)
            Downloads = $downloads
            InstalledVersion = [string]$installedPackage.version
        }
    } finally {
        Remove-Item -LiteralPath $sandbox -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$RootPackage = "claude-code-rust"
$NonInteractive = $true
$InstallDir = "C:\test\claude-rs"

Assert-Equal "1.2.3-preview.1+build.5" (Get-ReleaseVersion -Tag "v1.2.3-preview.1+build.5") "Release normalization did not remove exactly one v prefix"
Assert-Equal "Version1" (Get-ReleaseVersion -Tag "Version1") "Release normalization removed non-prefix version characters"

$metadataSandbox = Join-Path ([IO.Path]::GetTempPath()) "claude-rs-version-metadata-test-$PID"
try {
    Remove-Item -LiteralPath $metadataSandbox -Recurse -Force -ErrorAction SilentlyContinue
    New-OwnedInstall -Path $metadataSandbox -Version "1.2.3-preview.1+build.5"
    $installInfo = Get-ScriptInstallInfo -Path $metadataSandbox
    Assert-True ($null -ne $installInfo) "Installer-owned metadata was not recognized"
    Assert-Equal "1.2.3-preview.1+build.5" $installInfo.Version "Installer-owned version was not read exactly"
    Assert-True (Test-ScriptInstallDirectory -Path $metadataSandbox) "Installer-owned directory check rejected valid metadata"
} finally {
    Remove-Item -LiteralPath $metadataSandbox -Recurse -Force -ErrorAction SilentlyContinue
}

$advisoriesSandbox = Join-Path ([IO.Path]::GetTempPath()) "claude-rs-advisories-test-$PID.json"
try {
    [IO.File]::WriteAllText($advisoriesSandbox, @'
{
  "advisories": [
    {
      "start":"1.2.0",
      "end":"1.10.3",
      "summary":"First issue."
    },
    {"summary": "Second issue.", "end": "1.10.3", "start": "1.10.3"}
  ]
}
'@)
    function Get-Advisories {
        param([string]$Version)
        return (@(Get-MatchingReleaseAdvisories -Path $advisoriesSandbox -Version $Version) -join "|")
    }
    Assert-Equal "" (Get-Advisories "1.1.9") "Version below the range matched an advisory"
    Assert-Equal "First issue." (Get-Advisories "1.2.0") "Range start is not inclusive"
    Assert-Equal "First issue." (Get-Advisories "1.9.0") "Version components were not compared numerically"
    Assert-Equal "First issue.|Second issue." (Get-Advisories "1.10.3") "Range end is not inclusive or not every matching advisory was returned"
    Assert-Equal "" (Get-Advisories "1.10.4") "Version above the range matched an advisory"
    Assert-Equal "First issue." (Get-Advisories "1.2.0-preview.1+build.5") "Prerelease and build suffixes were not ignored"
    Assert-Equal "" (Get-Advisories "nightly") "Unparseable version matched an advisory"
} finally {
    Remove-Item -LiteralPath $advisoriesSandbox -Force -ErrorAction SilentlyContinue
}

$Update = $true
$Yes = $true
Assert-True (-not (Confirm-SameVersionReinstall -Version "1.2.3")) "Update mode allowed a same-version reinstall"
$Update = $false
$Yes = $true
Assert-True (Confirm-SameVersionReinstall -Version "1.2.3") "-Yes did not approve a same-version reinstall"
$Yes = $false
Assert-True (-not (Confirm-SameVersionReinstall -Version "1.2.3")) "Non-interactive mode approved a same-version reinstall"

$selectedVersion = "9.8.7-preview.1+build.5"
$selectedTag = "v$selectedVersion"

$defaultResult = Invoke-InstallerScenario -RequestedRelease $selectedTag
Assert-Equal 0 $defaultResult.Status "Pinned same-version install did not exit successfully: $($defaultResult.Output)"
Assert-True $defaultResult.Output.Contains("already installed; no changes made") "Pinned same-version install did not report its no-op"
Assert-Equal 0 $defaultResult.Downloads.Count "Pinned same-version install attempted a release download"
Assert-Equal $selectedVersion $defaultResult.InstalledVersion "Pinned same-version no-op changed installed metadata"

$updateResult = Invoke-InstallerScenario -RequestedRelease $selectedTag -Mode "Update"
Assert-Equal 0 $updateResult.Status "Same-version update did not exit successfully"
Assert-True $updateResult.Output.Contains("already installed; no changes made") "Same-version update did not report its no-op"
Assert-True (-not $updateResult.Output.Contains("Reinstalling claude-rs")) "Same-version update attempted a reinstall"
Assert-Equal 0 $updateResult.Downloads.Count "Same-version update attempted a release download"

$yesResult = Invoke-InstallerScenario -RequestedRelease $selectedTag -Mode "Yes"
Assert-True ($yesResult.Status -ne 0) "Mock release payload failure did not stop the -Yes reinstall"
Assert-True $yesResult.Output.Contains("Reinstalling claude-rs $selectedVersion") "-Yes did not report a same-version reinstall"
Assert-Equal 2 $yesResult.Downloads.Count "-Yes reinstall made an unexpected number of download attempts"
Assert-True $yesResult.Downloads[0].EndsWith("/main/scripts/install/release-advisories.json", [StringComparison]::Ordinal) "-Yes reinstall did not check release advisories before downloading"
Assert-True $yesResult.Downloads[1].EndsWith("/$selectedTag/SHA256SUMS", [StringComparison]::Ordinal) "-Yes reinstall did not reach the checksum download"
Assert-True (-not $yesResult.Output.Contains("Known issue")) "Unavailable release advisories produced a warning"

$latestResult = Invoke-InstallerScenario -RequestedRelease "latest" -Mode "Latest"
Assert-Equal 0 $latestResult.Status "Latest same-version install did not exit successfully"
Assert-True $latestResult.Output.Contains("Release $selectedTag selected") "Latest same-version install did not resolve the expected release"
Assert-True $latestResult.Output.Contains("already installed; no changes made") "Latest same-version install did not report its no-op"
Assert-Equal 1 $latestResult.Downloads.Count "Latest same-version install requested release payloads"
Assert-True $latestResult.Downloads[0].EndsWith("/releases/latest", [StringComparison]::Ordinal) "Latest same-version install did not limit network access to metadata"

$differentlyCasedTag = "v9.8.7-Preview.1+build.5"
$differentResult = Invoke-InstallerScenario -RequestedRelease $differentlyCasedTag
Assert-True ($differentResult.Status -ne 0) "Mock release payload failure did not stop the distinct release install"
Assert-True (-not $differentResult.Output.Contains("already installed; no changes made")) "Release comparison ignored prerelease identifier casing"
Assert-Equal 2 $differentResult.Downloads.Count "Distinct release install made an unexpected number of download attempts"
Assert-True $differentResult.Downloads[1].EndsWith("/$differentlyCasedTag/SHA256SUMS", [StringComparison]::Ordinal) "Distinct release install did not reach the checksum download"

$matchingAdvisories = '{"advisories":[{"start":"9.8.0","end":"9.8.7","summary":"Known test issue. Fixed in 9.8.8."}]}'
$advisedResult = Invoke-InstallerScenario -RequestedRelease $selectedTag -Mode "Yes" -Advisories $matchingAdvisories
Assert-True $advisedResult.Output.Contains("Known issue in claude-rs ${selectedVersion}: Known test issue. Fixed in 9.8.8.") "Selected release inside an advisory range did not warn: $($advisedResult.Output)"
Assert-Equal 2 $advisedResult.Downloads.Count "Release advisory warning stopped the install before the checksum download"

$advisedNoOpResult = Invoke-InstallerScenario -RequestedRelease $selectedTag -Advisories $matchingAdvisories
Assert-Equal 0 $advisedNoOpResult.Downloads.Count "Same-version no-op fetched release advisories"

$unrelatedAdvisories = '{"advisories":[{"start":"9.8.8","end":"9.9.0","summary":"Known test issue."}]}'
$unadvisedResult = Invoke-InstallerScenario -RequestedRelease $selectedTag -Mode "Yes" -Advisories $unrelatedAdvisories
Assert-True (-not $unadvisedResult.Output.Contains("Known issue")) "Selected release outside every advisory range warned"

$malformedResult = Invoke-InstallerScenario -RequestedRelease $selectedTag -Mode "Yes" -Advisories "not json"
Assert-True (-not $malformedResult.Output.Contains("Known issue")) "Malformed release advisories produced a warning"
Assert-Equal 2 $malformedResult.Downloads.Count "Malformed release advisories stopped the install before the checksum download"

Write-Output "PowerShell installer version guard tests passed"
