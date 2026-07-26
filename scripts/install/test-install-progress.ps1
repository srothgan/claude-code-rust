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

$progressCommands = @($installerAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
        $node.GetCommandName() -eq "Write-Progress"
}, $true))
if ($progressCommands.Count -ne 0) {
    throw "install.ps1 must use the inline spinner instead of Write-Progress"
}

$helperNames = @(
    "New-InstallerProgressWorker",
    "Close-InstallerProgressWorker",
    "Stop-InstallerProgress",
    "Start-InstallerProgress",
    "Complete-InstallerProgress",
    "Write-InstallerLine",
    "Write-Ok",
    "Write-WarnLine",
    "Write-WarnDetail",
    "Write-FailLine",
    "Write-InstallerDiagnostic",
    "Format-DownloadBytes",
    "Write-DownloadDiagnostic",
    "Format-DownloadEta",
    "Format-DownloadProgress",
    "Write-DownloadProgress",
    "Clear-DownloadProgress",
    "ConvertTo-CurlConfigValue",
    "Get-DownloadContentLength",
    "Get-CurlApplication",
    "Invoke-ArchiveDownload",
    "Test-CanPrompt",
    "Confirm-DefaultNo",
    "Warn-MissingClaudeCli"
)
$helperDefinitions = @($installerAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $helperNames -contains $node.Name
}, $true))
if ($helperDefinitions.Count -ne $helperNames.Count) {
    $loadedNames = @($helperDefinitions | ForEach-Object { $_.Name })
    $missingNames = @($helperNames | Where-Object { $loadedNames -notcontains $_ })
    throw "Could not load installer helper definitions: $($missingNames -join ', ')"
}
Invoke-Expression (($helperDefinitions | ForEach-Object { $_.Extent.Text }) -join [Environment]::NewLine)

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) {
        throw $Message
    }
}

function Assert-Equal {
    param($Expected, $Actual, [string]$Message)
    if ($Expected -ne $Actual) {
        throw "$Message (expected '$Expected', got '$Actual')"
    }
}

$UseColor = $false
$OkMark = [char]0x2713
$FailMark = [char]0x2717
$NonInteractive = $true
$script:InstallerProgressSupported = $true
$script:InstallerProgressActive = $false
$script:InstallerProgressWorker = $null
$script:DownloadProgressWidth = 0
$Verify = $false
$DownloadRetryCount = 3
$DownloadConnectTimeoutSeconds = 30
$DownloadLowSpeedBytesPerSecond = 1024
$DownloadLowSpeedTimeSeconds = 30

function Get-Command {
    param(
        [string]$Name,
        [object]$CommandType,
        [object]$ErrorAction
    )

    if ($Name -eq "curl") {
        return @(
            [pscustomobject]@{ Source = "/usr/bin/curl" },
            [pscustomobject]@{ Source = "/bin/curl" }
        )
    }
}
try {
    $resolvedCurl = Get-CurlApplication
    Assert-Equal "/usr/bin/curl" $resolvedCurl.Source "Curl resolution returned multiple application paths"
} finally {
    Remove-Item Function:\Get-Command
}

Assert-Equal "[>.........]   0% Downloading release archive" `
    (Format-DownloadProgress -DownloadedBytes 0 -TotalBytes 1000 -ElapsedSeconds 0 -IncludeDiagnostics $false) `
    "Download progress did not render the empty fixed-width bar"
Assert-Equal "[=====>....]  50% Downloading release archive" `
    (Format-DownloadProgress -DownloadedBytes 500 -TotalBytes 1000 -ElapsedSeconds 2 -IncludeDiagnostics $false) `
    "Download progress did not render the half-filled fixed-width bar"
Assert-Equal "[==========] 100% Downloading release archive" `
    (Format-DownloadProgress -DownloadedBytes 1000 -TotalBytes 1000 -ElapsedSeconds 4 -IncludeDiagnostics $false) `
    "Download progress did not render the completed fixed-width bar"
Assert-Equal "[...>......]  --% Downloading release archive" `
    (Format-DownloadProgress -DownloadedBytes 0 -TotalBytes 0 -ElapsedSeconds 0 -IncludeDiagnostics $false -UnknownPosition 3) `
    "Download progress did not render the fixed-width unknown-length bar"
Assert-Equal "[=====>....]  50% Downloading release archive | 512.0 KiB / 1.0 MiB | 256.0 KiB/s | ETA 00:02" `
    (Format-DownloadProgress -DownloadedBytes 524288 -TotalBytes 1048576 -ElapsedSeconds 2 -IncludeDiagnostics $true) `
    "Download diagnostics did not include fixed-width transfer details"

$downloadOutputWriter = New-Object System.IO.StringWriter
$originalConsoleOut = [Console]::Out
[Console]::SetOut($downloadOutputWriter)
try {
    Write-DownloadProgress -Text "[=====>....]  50% Downloading release archive"
    Write-DownloadProgress -Text "[==========] 100% Downloading release archive" -Complete
} finally {
    [Console]::SetOut($originalConsoleOut)
    $downloadOutput = $downloadOutputWriter.ToString()
    $downloadOutputWriter.Dispose()
}
Assert-True $downloadOutput.Contains("`r[=====>....]  50% Downloading release archive") "Download progress did not update in place"
Assert-True $downloadOutput.Contains("`r[==========] 100% Downloading release archive") "Download progress did not render completion"
Assert-Equal 0 $script:DownloadProgressWidth "Download progress retained width after completion"

$downloadSandbox = Join-Path ([IO.Path]::GetTempPath()) "claude-rs-download-test-$PID"
$downloadSource = Join-Path $downloadSandbox "source.bin"
$downloadDestination = Join-Path $downloadSandbox "destination.bin"
New-Item -ItemType Directory -Path $downloadSandbox -Force | Out-Null
try {
    $sourceBytes = New-Object byte[] 65536
    (New-Object Random 42).NextBytes($sourceBytes)
    [IO.File]::WriteAllBytes($downloadSource, $sourceBytes)
    $downloadUri = (New-Object Uri($downloadSource)).AbsoluteUri
    $script:InstallerProgressSupported = $false
    Invoke-ArchiveDownload -Uri $downloadUri -Destination $downloadDestination
    Assert-Equal `
        (Get-FileHash -LiteralPath $downloadSource -Algorithm SHA256).Hash `
        (Get-FileHash -LiteralPath $downloadDestination -Algorithm SHA256).Hash `
        "Streaming downloader changed the downloaded bytes"
    foreach ($suffix in @(".headers", ".stderr", ".curlrc")) {
        Assert-True (-not (Test-Path -LiteralPath "$downloadDestination$suffix")) "Streaming downloader left $suffix state behind"
    }
} finally {
    Remove-Item -LiteralPath $downloadSandbox -Recurse -Force -ErrorAction SilentlyContinue
}
$script:InstallerProgressSupported = $true

# Exercise the real background runspace with Console.Out captured. This verifies
# that the renderer emits an inline animated line and that stopping it erases
# the line without relying on the host-native progress area.
$spinnerOutputWriter = New-Object System.IO.StringWriter
$spinnerOutput = ""
[Console]::SetOut($spinnerOutputWriter)
try {
    Start-InstallerProgress "Downloading release archive"
    Start-Sleep -Milliseconds 1000
    Assert-True $script:InstallerProgressActive "Real inline spinner did not become active"
    Assert-True ($null -ne $script:InstallerProgressWorker) "Real inline spinner did not retain its worker"
    Stop-InstallerProgress
} finally {
    Stop-InstallerProgress
    [Console]::SetOut($originalConsoleOut)
    $spinnerOutput = $spinnerOutputWriter.ToString()
    $spinnerOutputWriter.Dispose()
}
Assert-True $spinnerOutput.Contains("`r| Downloading release archive") "Inline spinner did not render its vertical frame"
Assert-True $spinnerOutput.Contains("`r/ Downloading release archive") "Inline spinner did not render its slash frame"
Assert-True $spinnerOutput.Contains("`r- Downloading release archive") "Inline spinner did not render its dash frame"
Assert-True $spinnerOutput.Contains("`r\ Downloading release archive") "Inline spinner did not render its backslash frame"
Assert-True (-not $spinnerOutput.Contains("Installing claude-rs")) "Inline spinner rendered a host-style progress banner"
Assert-True (-not $script:InstallerProgressActive) "Real inline spinner remained active after stop"
Assert-True ($null -eq $script:InstallerProgressWorker) "Real inline spinner retained its worker after stop"
Assert-Equal "SilentlyContinue" $ProgressPreference "Real inline spinner changed the outer progress preference"

$fastOutputWriter = New-Object System.IO.StringWriter
$fastOutput = ""
[Console]::SetOut($fastOutputWriter)
try {
    $script:InstallerProgressSupported = $true
    Start-InstallerProgress "Resolving release"
    Start-Sleep -Milliseconds 50
    Stop-InstallerProgress
} finally {
    Stop-InstallerProgress
    [Console]::SetOut($originalConsoleOut)
    $fastOutput = $fastOutputWriter.ToString()
    $fastOutputWriter.Dispose()
}
Assert-Equal "" $fastOutput "A fast operation rendered or cleared a spinner before the delay elapsed"
Assert-True (-not $script:InstallerProgressActive) "Delayed spinner remained active after an early stop"
Assert-True ($null -eq $script:InstallerProgressWorker) "Delayed spinner retained its worker after an early stop"

$script:RecordedEvents = New-Object System.Collections.ArrayList
function New-InstallerProgressWorker {
    param([string]$Message)
    [void]$script:RecordedEvents.Add([pscustomobject]@{
        Kind = "SpinnerStart"
        Message = $Message
    })
    return [pscustomobject]@{
        Message = $Message
        Width = $Message.Length + 2
    }
}

function Close-InstallerProgressWorker {
    param($Worker)
    [void]$script:RecordedEvents.Add([pscustomobject]@{
        Kind = "SpinnerStop"
        Message = $Worker.Message
    })
}

function Write-Host {
    param(
        [Parameter(Position = 0)]
        [object]$Object,
        [ConsoleColor]$ForegroundColor,
        [switch]$NoNewline
    )
    [void]$script:RecordedEvents.Add([pscustomobject]@{
        Kind = "Host"
        Text = [string]$Object
        ForegroundColor = $ForegroundColor
        NoNewline = [bool]$NoNewline
    })
}

function Reset-RecordedEvents {
    $script:RecordedEvents.Clear()
}

function Assert-OutputBoundaryClearsProgress {
    param([string]$Name, [scriptblock]$Action)
    $script:InstallerProgressSupported = $true
    Start-InstallerProgress "active $Name"
    Reset-RecordedEvents
    & $Action
    Assert-True ($script:RecordedEvents.Count -ge 1) "$Name did not clear the active spinner"
    Assert-Equal "SpinnerStop" $script:RecordedEvents[0].Kind "$Name did not stop progress before output"
    Assert-True (-not $script:InstallerProgressActive) "$Name left progress active"
    Assert-True ($null -eq $script:InstallerProgressWorker) "$Name retained the progress worker"
    Assert-Equal "SilentlyContinue" $ProgressPreference "$Name changed the outer progress preference"
}

Reset-RecordedEvents
$script:InstallerProgressSupported = $false
$script:InstallerProgressActive = $false
$script:InstallerProgressWorker = $null
Start-InstallerProgress "disabled"
Assert-Equal 0 $script:RecordedEvents.Count "Disabled progress started a spinner"
Assert-True (-not $script:InstallerProgressActive) "Disabled progress became active"
Assert-True ($null -eq $script:InstallerProgressWorker) "Disabled progress created a worker"

$script:InstallerProgressSupported = $true
Start-InstallerProgress "Downloading release archive"
Assert-Equal 1 $script:RecordedEvents.Count "Progress start emitted an unexpected event sequence"
Assert-Equal "SpinnerStart" $script:RecordedEvents[0].Kind "Progress start emitted the wrong event"
Assert-Equal "Downloading release archive" $script:RecordedEvents[0].Message "Progress start used the wrong message"
Assert-True $script:InstallerProgressActive "Progress start did not become active"
Assert-True ($null -ne $script:InstallerProgressWorker) "Progress start did not retain its worker"

Complete-InstallerProgress "Downloaded release archive"
Assert-Equal 3 $script:RecordedEvents.Count "Progress completion emitted an unexpected event sequence"
Assert-Equal "SpinnerStop" $script:RecordedEvents[1].Kind "Progress completion did not stop the spinner"
Assert-Equal "Host" $script:RecordedEvents[2].Kind "Progress completion did not print the success line last"
Assert-Equal "$OkMark Downloaded release archive" $script:RecordedEvents[2].Text "Progress completion changed the success line"
Assert-True (-not $script:InstallerProgressActive) "Progress completion left the spinner active"
Assert-True ($null -eq $script:InstallerProgressWorker) "Progress completion retained the worker"

Reset-RecordedEvents
$script:InstallerProgressSupported = $true
Start-InstallerProgress "repeated stop"
Reset-RecordedEvents
Stop-InstallerProgress
Stop-InstallerProgress
Assert-Equal 1 $script:RecordedEvents.Count "Repeated progress stop emitted extra events"
Assert-Equal "SpinnerStop" $script:RecordedEvents[0].Kind "Progress stop emitted the wrong event"

Assert-OutputBoundaryClearsProgress "normal status" {
    Write-InstallerLine -Mark $OkMark -Message "normal status" -Color Green
}
Assert-OutputBoundaryClearsProgress "warning" { Write-WarnLine "warning" }
Assert-OutputBoundaryClearsProgress "warning detail" { Write-WarnDetail "warning detail" }
Assert-OutputBoundaryClearsProgress "failure" { Write-FailLine "failure" }
Assert-OutputBoundaryClearsProgress "prompt" { [void](Confirm-DefaultNo "prompt") }
Assert-OutputBoundaryClearsProgress "diagnostic" { Write-InstallerDiagnostic "diagnostic" }

$originalPath = $env:PATH
$originalConsoleError = [Console]::Error
$warningOutputWriter = New-Object System.IO.StringWriter
try {
    $env:PATH = ""
    [Console]::SetError($warningOutputWriter)
    Reset-RecordedEvents
    Warn-MissingClaudeCli
    $warningText = $warningOutputWriter.ToString()
    Assert-True $warningText.Contains("Claude Code CLI ('claude') not found on PATH") "Missing Claude CLI warning was not emitted"
    Assert-True $warningText.Contains("Install it from https://claude.com/claude-code") "Missing Claude CLI install guidance was not emitted"

    $warningOutputWriter.GetStringBuilder().Clear() | Out-Null
    Set-Item -Path Function:\claude -Value {}
    Reset-RecordedEvents
    Warn-MissingClaudeCli
    Assert-Equal 0 $script:RecordedEvents.Count "Available Claude CLI emitted a missing-CLI warning"
    Assert-Equal "" $warningOutputWriter.ToString() "Available Claude CLI wrote a missing-CLI warning to stderr"
} finally {
    [Console]::SetError($originalConsoleError)
    $warningOutputWriter.Dispose()
    Remove-Item -Path Function:\claude -ErrorAction SilentlyContinue
    $env:PATH = $originalPath
}

Assert-Equal "SilentlyContinue" $ProgressPreference "Installer helpers changed the script-wide progress preference"
Write-Output "PowerShell installer inline progress helper tests passed"
