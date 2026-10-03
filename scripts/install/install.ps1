param(
    [string]$Release,
    [string]$InstallDir,
    [switch]$Yes,
    [switch]$NoModifyPath,
    [switch]$Verify,
    [switch]$Run,
    [switch]$RemoveNpm,
    [switch]$KeepNpm,
    [switch]$Uninstall,
    [switch]$Update,
    [int]$UpdateParentProcessId,
    [switch]$Help
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$RepoOwner = "srothgan"
$RepoName = "claude-code-rust"
$RepoSlug = "$RepoOwner/$RepoName"
$RootPackage = "claude-code-rust"
$DownloadRetryCount = 3
$DownloadConnectTimeoutSeconds = 30
$DownloadLowSpeedBytesPerSecond = 1024
$DownloadLowSpeedTimeSeconds = 30
$ReleaseAdvisoriesUrl = "https://raw.githubusercontent.com/$RepoSlug/main/scripts/install/release-advisories.json"
$ReleaseAdvisoriesTimeoutSeconds = 10

function Show-Usage {
    @"
Usage: install.ps1 [options]

Options:
  -Release <version>      Release tag or version. Defaults to latest.
  -InstallDir <dir>      App install directory.
  -Yes                   Reinstall the selected version when already installed;
                         skip optional installer prompts.
  -NoModifyPath          Do not update the user PATH.
  -Verify                Show download diagnostics and run strict runtime
                         diagnostics after install.
  -Run                   Start claude-rs after a successful install.
  -RemoveNpm             Remove an existing global npm install when found.
  -KeepNpm               Keep an existing global npm install without prompting.
  -Uninstall             Remove the script install layout and user PATH entry.
  -Update                Update an existing script install in place.
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
  CLAUDE_RS_UPDATE
  CLAUDE_RS_UPDATE_PARENT_PID
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
if (Test-TruthyEnv "CLAUDE_RS_UPDATE") {
    $Update = $true
}
if (-not $PSBoundParameters.ContainsKey("UpdateParentProcessId") -and $env:CLAUDE_RS_UPDATE_PARENT_PID) {
    $UpdateParentProcessId = [int]$env:CLAUDE_RS_UPDATE_PARENT_PID
}
if ($Update -and $Uninstall) {
    throw "-Update and -Uninstall cannot be used together"
}
if ($Update) {
    $Yes = $true
    $NonInteractive = $true
    $NoModifyPath = $true
    $KeepNpm = $true
}
if ($RemoveNpm -and $KeepNpm) {
    throw "-RemoveNpm and -KeepNpm cannot be used together"
}

# Cyan marks information and interaction, magenta running work, green success.
$UseColor = -not $env:NO_COLOR

# The installer draws one framed transcript: an intro line, one entry per step,
# and an outro line, joined by a gutter bar. This file stays ASCII because
# Windows PowerShell reads a script without a BOM in the ANSI code page, so
# every symbol is built from its code point.
$Glyph = @{
    BarStart = [string][char]0x250C
    Bar = [string][char]0x2502
    BarEnd = [string][char]0x2514
    BarBranch = [string][char]0x251C
    Line = [string][char]0x2500
    CornerTop = [string][char]0x256E
    CornerBottom = [string][char]0x256F
    Step = [string][char]0x25C7
    Active = [string][char]0x25C6
    Info = [string][char]0x25CF
    Warn = [string][char]0x25B2
    Error = [string][char]0x25A0
    Fill = [string][char]0x2501
    Empty = [string][char]0x2500
}
$SpinnerFrames = @([string][char]0x25D2, [string][char]0x25D0, [string][char]0x25D3, [string][char]0x25D1)

# Terminals with font fallback draw every symbol. The classic console host only
# draws what its own font ships, and Consolas has no outlined diamond or
# half-circle glyphs, so it gets the nearest symbols Consolas does have.
$fontFallbackTerminal = $env:WT_SESSION -or
    $env:TERM_PROGRAM -eq "vscode" -or
    $env:TERMINAL_EMULATOR -eq "JetBrains-JediTerm" -or
    $env:ConEmuTask -eq "{cmd::Cmder}" -or
    $env:TERM -match "^(xterm-256color|alacritty|rxvt-unicode)"
if (-not $fontFallbackTerminal) {
    $Glyph.Step = [string][char]0x25CA
    $Glyph.Active = [string][char]0x2666
    $SpinnerFrames = @("|", "/", "-", "\")
}

$script:InstallerProgressSupported = $false
$script:InstallerProgressActive = $false
$script:InstallerProgressWorker = $null
$script:DownloadProgressWidth = 0
$script:InstallerFrameOpen = $false
$script:PreviousOutputEncoding = $null

# Redirected host output and the [Console] writers encode through the console
# output encoding, which defaults to a legacy OEM code page that cannot carry
# the frame symbols. The console is shared with the calling shell, so the
# previous encoding is restored when the installer finishes.
try {
    $script:PreviousOutputEncoding = [Console]::OutputEncoding
    [Console]::OutputEncoding = New-Object Text.UTF8Encoding($false)
} catch {
    $script:PreviousOutputEncoding = $null
}

if (-not $env:CI -and $env:TERM -ne "dumb") {
    try {
        if (-not [Console]::IsOutputRedirected) {
            # Accessing cursor state throws in hosts without a real console.
            $null = [Console]::CursorLeft
            $script:InstallerProgressSupported = $true
        }
    } catch {
        $script:InstallerProgressSupported = $false
    }
}

function New-InstallerProgressWorker {
    param([string]$Message)

    $stopEvent = $null
    $renderedEvent = $null
    $runspace = $null
    $powerShell = $null
    try {
        $stopEvent = New-Object System.Threading.ManualResetEvent($false)
        $renderedEvent = New-Object System.Threading.ManualResetEvent($false)
        $runspace = [System.Management.Automation.Runspaces.RunspaceFactory]::CreateRunspace()
        $runspace.Open()

        $renderer = {
            param(
                [string]$Message,
                [System.Threading.ManualResetEvent]$StopEvent,
                [System.Threading.ManualResetEvent]$RenderedEvent,
                [bool]$UseColor,
                [string[]]$Frames
            )

            $frameIndex = 0
            if ($StopEvent.WaitOne(200)) {
                return
            }
            while (-not $StopEvent.WaitOne(0)) {
                [Console]::Write("`r")
                if ($UseColor) {
                    $previousColor = [Console]::ForegroundColor
                    try {
                        [Console]::ForegroundColor = [ConsoleColor]::Magenta
                        [Console]::Write($Frames[$frameIndex])
                    } finally {
                        [Console]::ForegroundColor = $previousColor
                    }
                } else {
                    [Console]::Write($Frames[$frameIndex])
                }
                [Console]::Write("  $Message")
                $null = $RenderedEvent.Set()

                $frameIndex = ($frameIndex + 1) % $Frames.Count
                if ($StopEvent.WaitOne(150)) {
                    break
                }
            }
        }

        $powerShell = [System.Management.Automation.PowerShell]::Create()
        $powerShell.Runspace = $runspace
        $null = $powerShell.AddScript($renderer.ToString())
        $null = $powerShell.AddArgument($Message)
        $null = $powerShell.AddArgument($stopEvent)
        $null = $powerShell.AddArgument($renderedEvent)
        $null = $powerShell.AddArgument($UseColor)
        $null = $powerShell.AddArgument([string[]]$SpinnerFrames)
        $asyncResult = $powerShell.BeginInvoke()

        return [pscustomobject]@{
            PowerShell = $powerShell
            AsyncResult = $asyncResult
            Runspace = $runspace
            StopEvent = $stopEvent
            RenderedEvent = $renderedEvent
            Width = $Message.Length + 3
        }
    } catch {
        if ($powerShell) {
            $powerShell.Dispose()
        }
        if ($runspace) {
            $runspace.Dispose()
        }
        if ($stopEvent) {
            $stopEvent.Dispose()
        }
        if ($renderedEvent) {
            $renderedEvent.Dispose()
        }
        throw
    }
}

function Close-InstallerProgressWorker {
    param($Worker)

    $rendered = $false
    try {
        $null = $Worker.StopEvent.Set()
        $null = $Worker.PowerShell.EndInvoke($Worker.AsyncResult)
    } finally {
        $rendered = $Worker.RenderedEvent.WaitOne(0)
        $Worker.PowerShell.Dispose()
        $Worker.Runspace.Dispose()
        $Worker.StopEvent.Dispose()
        $Worker.RenderedEvent.Dispose()
        if ($rendered) {
            [Console]::Write("`r" + (" " * $Worker.Width) + "`r")
        }
    }
}

function Stop-InstallerProgress {
    $worker = $script:InstallerProgressWorker
    $script:InstallerProgressWorker = $null
    $script:InstallerProgressActive = $false
    if (-not $worker) {
        return
    }

    try {
        Close-InstallerProgressWorker $worker
    } catch {
        $script:InstallerProgressSupported = $false
    }
}

function Start-InstallerProgress {
    param([string]$Message)
    Stop-InstallerProgress
    if (-not $script:InstallerProgressSupported) {
        return
    }

    try {
        $script:InstallerProgressWorker = New-InstallerProgressWorker $Message
        $script:InstallerProgressActive = $true
    } catch {
        $script:InstallerProgressWorker = $null
        $script:InstallerProgressActive = $false
        $script:InstallerProgressSupported = $false
    }
}

function Complete-InstallerProgress {
    param([string]$Message)
    Stop-InstallerProgress
    Write-Ok $Message
}

# Writes one run of text to the host, or to stderr for warnings and failures.
function Write-InstallerText {
    param(
        [string]$Text,
        $Color = $null,
        [switch]$NoNewline,
        [switch]$ToError
    )
    $colored = $UseColor -and ($null -ne $Color)
    if (-not $ToError) {
        if ($colored) {
            Write-Host $Text -ForegroundColor $Color -NoNewline:$NoNewline
        } else {
            Write-Host $Text -NoNewline:$NoNewline
        }
        return
    }

    $previousColor = $null
    if ($colored) {
        $previousColor = [Console]::ForegroundColor
        [Console]::ForegroundColor = $Color
    }
    try {
        if ($NoNewline) {
            [Console]::Error.Write($Text)
        } else {
            [Console]::Error.WriteLine($Text)
        }
    } finally {
        if ($colored) {
            [Console]::ForegroundColor = $previousColor
        }
    }
}

function Write-InstallerGutter {
    Write-InstallerText -Text $Glyph.Bar -Color DarkGray
}

function Write-InstallerDetail {
    param(
        [string]$Message,
        [switch]$ToError
    )
    Write-InstallerText -Text "$($Glyph.Bar)  $Message" -Color DarkGray -ToError:$ToError
}

function Write-InstallerLine {
    param(
        [string]$Mark,
        [string]$Message,
        [ConsoleColor]$Color,
        [string[]]$Detail = @(),
        [switch]$ToError
    )
    Stop-InstallerProgress
    Write-InstallerText -Text $Mark -Color $Color -NoNewline -ToError:$ToError
    Write-InstallerText -Text "  $Message" -ToError:$ToError
    foreach ($line in $Detail) {
        Write-InstallerDetail -Message $line -ToError:$ToError
    }
}

function Write-Info {
    param(
        [string]$Message,
        [string[]]$Detail = @()
    )
    Write-InstallerLine -Mark $Glyph.Info -Message $Message -Color Cyan -Detail $Detail
}

function Write-Ok {
    param(
        [string]$Message,
        [string[]]$Detail = @()
    )
    Write-InstallerLine -Mark $Glyph.Step -Message $Message -Color Green -Detail $Detail
}

function Write-WarnLine {
    param(
        [string]$Message,
        [string[]]$Detail = @()
    )
    Write-InstallerLine -Mark $Glyph.Warn -Message $Message -Color Yellow -Detail $Detail -ToError
}

function Write-WarnDetail {
    param([string]$Message)
    Stop-InstallerProgress
    Write-InstallerDetail -Message $Message -ToError
}

function Write-FailLine {
    param([string]$Message)
    $lines = @($Message -split "\r?\n")
    $detail = @($lines | Select-Object -Skip 1)
    Write-InstallerLine -Mark $Glyph.Error -Message $lines[0] -Color Red -Detail $detail -ToError
}

function Write-Intro {
    param([string]$Name)
    $script:InstallerFrameOpen = $true
    Write-InstallerText -Text "$($Glyph.BarStart)  " -Color DarkGray -NoNewline
    if ($UseColor) {
        Write-Host " claude-rs " -ForegroundColor Black -BackgroundColor Cyan -NoNewline
        Write-InstallerText -Text " $Name" -Color DarkGray
    } else {
        Write-InstallerText -Text "claude-rs $Name"
    }
    Write-InstallerGutter
}

function Write-Outro {
    param([string]$Message)
    Stop-InstallerProgress
    Write-InstallerGutter
    Write-InstallerText -Text $Glyph.BarEnd -Color DarkGray -NoNewline
    Write-InstallerText -Text "  $Message"
    $script:InstallerFrameOpen = $false
}

# Ends a frame that no outro closed: a failure, or an interrupted run.
function Close-InstallerFrame {
    param([string]$Message)
    if (-not $script:InstallerFrameOpen) {
        return
    }
    $script:InstallerFrameOpen = $false
    Write-InstallerText -Text "$($Glyph.BarEnd)  $Message" -Color Red -ToError
}

# A titled box of follow-up instructions, attached to the gutter.
function Write-Note {
    param(
        [string]$Title,
        [string[]]$Lines
    )
    Stop-InstallerProgress
    $width = $Title.Length
    foreach ($line in $Lines) {
        $width = [Math]::Max($width, $line.Length)
    }
    Write-InstallerGutter
    Write-InstallerText -Text $Glyph.Info -Color Cyan -NoNewline
    Write-InstallerText -Text "  $Title " -NoNewline
    Write-InstallerText -Text (($Glyph.Line * ($width + 1 - $Title.Length)) + $Glyph.CornerTop) -Color DarkGray
    foreach ($line in @("") + $Lines + @("")) {
        Write-InstallerText -Text ($Glyph.Bar + "  " + $line.PadRight($width) + "  " + $Glyph.Bar) -Color DarkGray
    }
    Write-InstallerText -Text ($Glyph.BarBranch + ($Glyph.Line * ($width + 4)) + $Glyph.CornerBottom) -Color DarkGray
}

function Format-DownloadBytes {
    param([double]$Bytes)

    $units = @("B", "KiB", "MiB", "GiB")
    $value = $Bytes
    $unitIndex = 0
    while ($value -ge 1024 -and $unitIndex -lt ($units.Count - 1)) {
        $value /= 1024
        $unitIndex++
    }

    if ($unitIndex -eq 0) {
        return [string]::Format(
            [Globalization.CultureInfo]::InvariantCulture,
            "{0:0} {1}",
            $value,
            $units[$unitIndex]
        )
    }
    return [string]::Format(
        [Globalization.CultureInfo]::InvariantCulture,
        "{0:0.0} {1}",
        $value,
        $units[$unitIndex]
    )
}

# Returns the -Verify detail line for curl's tab-separated transfer stats.
function Format-DownloadDiagnostic {
    param([string]$StatsLine)

    $prefix = "__CLAUDE_RS_DOWNLOAD_STATS__"
    if ([string]::IsNullOrWhiteSpace($StatsLine) -or
        -not $StatsLine.StartsWith($prefix, [StringComparison]::Ordinal)) {
        return $null
    }

    $parts = $StatsLine.Substring($prefix.Length).Split([char]9)
    if ($parts.Count -ne 4) {
        return $null
    }

    $culture = [Globalization.CultureInfo]::InvariantCulture
    $size = [double]::Parse($parts[1], $culture)
    $speed = [double]::Parse($parts[2], $culture)
    $elapsed = [double]::Parse($parts[3], $culture)
    $elapsedText = $elapsed.ToString("0.000", $culture)
    return "Download: $(Format-DownloadBytes $size) in ${elapsedText}s ($(Format-DownloadBytes $speed)/s, HTTP $($parts[0]))"
}

function Format-DownloadEta {
    param([double]$Seconds)

    if ([double]::IsNaN($Seconds) -or [double]::IsInfinity($Seconds) -or $Seconds -lt 0) {
        return "--:--"
    }

    $rounded = [Math]::Ceiling($Seconds)
    $span = [TimeSpan]::FromSeconds($rounded)
    if ($span.TotalHours -ge 1) {
        return "{0:00}:{1:00}:{2:00}" -f [Math]::Floor($span.TotalHours), $span.Minutes, $span.Seconds
    }
    return "{0:00}:{1:00}" -f $span.Minutes, $span.Seconds
}

$DownloadBarWidth = 20

# Returns the segments of the live download line. With an unknown total the
# filled cells slide across the bar instead.
function Format-DownloadProgress {
    param(
        [long]$DownloadedBytes,
        [long]$TotalBytes,
        [double]$ElapsedSeconds,
        [int]$Tick = 0
    )

    if ($TotalBytes -gt 0) {
        $percent = [int][Math]::Min(100, [Math]::Max(0, [Math]::Floor(($DownloadedBytes * 100.0) / $TotalBytes)))
        $lead = 0
        $fill = [int][Math]::Floor(($percent * $DownloadBarWidth) / 100)
        $percentText = "{0,3}%" -f $percent
        $sizes = "$(Format-DownloadBytes $DownloadedBytes) / $(Format-DownloadBytes $TotalBytes)"
    } else {
        $fill = 4
        $lead = $Tick % ($DownloadBarWidth - $fill + 1)
        $percentText = ""
        $sizes = Format-DownloadBytes $DownloadedBytes
    }

    $speed = $DownloadedBytes / [Math]::Max($ElapsedSeconds, 0.001)
    $eta = if ($TotalBytes -gt 0 -and $speed -gt 0) {
        Format-DownloadEta (($TotalBytes - $DownloadedBytes) / $speed)
    } else {
        "--:--"
    }

    return [pscustomobject]@{
        Lead = $lead
        Fill = $fill
        Percent = $percentText
        Sizes = $sizes
        Rate = "$(Format-DownloadBytes $speed)/s  ETA $eta"
    }
}

# Trailing segments are dropped rather than wrapped when the console is narrow,
# because a wrapped line cannot be redrawn in place.
function Write-DownloadProgress {
    param(
        $Progress,
        [int]$Tick
    )

    $columns = [Console]::WindowWidth
    $runs = New-Object System.Collections.Generic.List[object]
    $runs.Add(@($SpinnerFrames[$Tick % $SpinnerFrames.Count], [ConsoleColor]::Magenta))
    $runs.Add(@("  Downloading", $null))
    $width = 14
    if (($width + 2 + $DownloadBarWidth) -lt $columns) {
        $trail = $DownloadBarWidth - $Progress.Lead - $Progress.Fill
        $runs.Add(@(("  " + ($Glyph.Empty * $Progress.Lead)), [ConsoleColor]::DarkGray))
        $runs.Add(@(($Glyph.Fill * $Progress.Fill), [ConsoleColor]::Cyan))
        $runs.Add(@(($Glyph.Empty * $trail), [ConsoleColor]::DarkGray))
        $width += 2 + $DownloadBarWidth
    }
    if ($Progress.Percent) {
        $runs.Add(@(" $($Progress.Percent)", $null))
        $width += 1 + $Progress.Percent.Length
    }
    $rate = if ($Verify) { $Progress.Rate } else { "" }
    foreach ($segment in @($Progress.Sizes, $rate)) {
        if (-not $segment) {
            continue
        }
        if (($width + 2 + $segment.Length) -ge $columns) {
            break
        }
        $runs.Add(@("  $segment", [ConsoleColor]::DarkGray))
        $width += 2 + $segment.Length
    }

    Write-InstallerText -Text "`r" -NoNewline
    foreach ($run in $runs) {
        Write-InstallerText -Text $run[0] -Color $run[1] -NoNewline
    }
    if ($script:DownloadProgressWidth -gt $width) {
        Write-InstallerText -Text (" " * ($script:DownloadProgressWidth - $width)) -NoNewline
    }
    $script:DownloadProgressWidth = $width
}

function Clear-DownloadProgress {
    if ($script:DownloadProgressWidth -gt 0) {
        [Console]::Write("`r" + (" " * $script:DownloadProgressWidth) + "`r")
        $script:DownloadProgressWidth = 0
    }
}

function ConvertTo-CurlConfigValue {
    param([string]$Value)

    return '"' + $Value.Replace("\", "\\").Replace('"', '\"').Replace("`r", "\r").Replace("`n", "\n") + '"'
}

function Get-DownloadContentLength {
    param([string]$HeadersPath)

    if (-not (Test-Path -LiteralPath $HeadersPath -PathType Leaf)) {
        return 0
    }

    try {
        $contentLength = 0
        foreach ($line in [IO.File]::ReadAllLines($HeadersPath)) {
            if ($line -match "^[Cc]ontent-[Ll]ength:\s*(\d+)\s*$") {
                $contentLength = [long]$Matches[1]
            }
        }
        return $contentLength
    } catch {
        return 0
    }
}

function Get-CurlApplication {
    $curl = Get-Command "curl.exe" -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $curl) {
        $curl = Get-Command "curl" -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
    }
    return $curl
}

function Invoke-ArchiveDownload {
    param(
        [string]$Uri,
        [string]$Destination
    )

    $curl = Get-CurlApplication
    if (-not $curl) {
        Write-WarnLine "curl executable not found; using the slower PowerShell downloader"
        Start-InstallerProgress "Downloading release archive"
        $stopwatch = [Diagnostics.Stopwatch]::StartNew()
        Invoke-WebRequest -Uri $Uri -OutFile $Destination
        $stopwatch.Stop()
        $downloadedBytes = (Get-Item -LiteralPath $Destination).Length
        $detail = @()
        if ($Verify) {
            $elapsedSeconds = [Math]::Max($stopwatch.Elapsed.TotalSeconds, 0.001)
            $elapsedText = $elapsedSeconds.ToString("0.000", [Globalization.CultureInfo]::InvariantCulture)
            $detail = @("Download: $(Format-DownloadBytes $downloadedBytes) in ${elapsedText}s ($(Format-DownloadBytes ($downloadedBytes / $elapsedSeconds))/s, HTTP unavailable)")
        }
        Write-Ok "Downloaded release archive ($(Format-DownloadBytes $downloadedBytes))" -Detail $detail
        return
    }

    $headersPath = "$Destination.headers"
    $stderrPath = "$Destination.stderr"
    $configPath = "$Destination.curlrc"
    $totalBytes = 0
    & $curl.Source `
        "--fail" `
        "--location" `
        "--head" `
        "--silent" `
        "--retry" "$DownloadRetryCount" `
        "--connect-timeout" "$DownloadConnectTimeoutSeconds" `
        "--dump-header" $headersPath `
        "--output" "-" `
        $Uri 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) {
        $totalBytes = Get-DownloadContentLength -HeadersPath $headersPath
    }

    $statsFormat = "__CLAUDE_RS_DOWNLOAD_STATS__%{http_code}\t%{size_download}\t%{speed_download}\t%{time_total}"
    $configLines = @(
        "fail",
        "location",
        "retry = $DownloadRetryCount",
        "connect-timeout = $DownloadConnectTimeoutSeconds",
        "speed-limit = $DownloadLowSpeedBytesPerSecond",
        "speed-time = $DownloadLowSpeedTimeSeconds",
        "silent",
        "show-error",
        "dump-header = $(ConvertTo-CurlConfigValue $headersPath)",
        "stderr = $(ConvertTo-CurlConfigValue $stderrPath)",
        "output = $(ConvertTo-CurlConfigValue $Destination)",
        "write-out = $(ConvertTo-CurlConfigValue $statsFormat)",
        "url = $(ConvertTo-CurlConfigValue $Uri)"
    )
    [IO.File]::WriteAllLines($configPath, $configLines, (New-Object Text.UTF8Encoding($false)))

    # The spinner covers the size lookup above; the live bar replaces it here.
    Stop-InstallerProgress
    $process = $null
    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    $tick = 0
    try {
        $startInfo = New-Object Diagnostics.ProcessStartInfo
        $startInfo.FileName = $curl.Source
        $startInfo.Arguments = "--config `"$configPath`""
        $startInfo.UseShellExecute = $false
        $startInfo.CreateNoWindow = $true
        $startInfo.RedirectStandardOutput = $true
        $process = [Diagnostics.Process]::Start($startInfo)
        if (-not $process) {
            throw "could not start curl executable"
        }

        while (-not $process.WaitForExit(200)) {
            if ($script:InstallerProgressSupported) {
                $downloadedBytes = if (Test-Path -LiteralPath $Destination -PathType Leaf) {
                    (Get-Item -LiteralPath $Destination).Length
                } else {
                    0
                }
                $progress = Format-DownloadProgress -DownloadedBytes $downloadedBytes -TotalBytes $totalBytes -ElapsedSeconds $stopwatch.Elapsed.TotalSeconds -Tick $tick
                Write-DownloadProgress -Progress $progress -Tick $tick
                $tick++
            }
        }

        $curlOutput = $process.StandardOutput.ReadToEnd()
        $stopwatch.Stop()
        Clear-DownloadProgress
        if ($process.ExitCode -ne 0) {
            if (Test-Path -LiteralPath $stderrPath -PathType Leaf) {
                foreach ($line in Get-Content -LiteralPath $stderrPath) {
                    if (-not [string]::IsNullOrWhiteSpace($line)) {
                        Write-WarnDetail $line
                    }
                }
            }
            throw "curl executable failed with exit code $($process.ExitCode)"
        }

        # The live download line collapses into the one completed entry.
        $detail = @()
        if ($Verify) {
            $statsLine = @($curlOutput -split "\r?\n" | Where-Object {
                $_.StartsWith("__CLAUDE_RS_DOWNLOAD_STATS__", [StringComparison]::Ordinal)
            } | Select-Object -Last 1)
            if ($statsLine.Count -gt 0) {
                $diagnostic = Format-DownloadDiagnostic -StatsLine "$($statsLine[0])"
                if ($diagnostic) {
                    $detail = @($diagnostic)
                }
            }
        }
        $downloadedBytes = (Get-Item -LiteralPath $Destination).Length
        Write-Ok "Downloaded release archive ($(Format-DownloadBytes $downloadedBytes))" -Detail $detail
    } finally {
        if ($process) {
            if (-not $process.HasExited) {
                $process.Kill()
                $process.WaitForExit()
            }
            $process.Dispose()
        }
        Clear-DownloadProgress
        Remove-Item -LiteralPath $headersPath, $stderrPath, $configPath -Force -ErrorAction SilentlyContinue
    }
}

function Test-CanPrompt {
    return (-not $NonInteractive) -and (-not [Console]::IsInputRedirected)
}

# Immediate y/n confirmation that collapses into its answer. Polling for a key
# instead of blocking on one keeps Ctrl-C able to stop the installer.
function Read-ConfirmAnswer {
    param([string]$Prompt)

    $selected = $false
    $answered = $false
    $cursorVisible = [Console]::CursorVisible
    Write-InstallerText -Text $Glyph.Active -Color Cyan -NoNewline
    Write-InstallerText -Text "  $Prompt"
    Write-InstallerText -Text $Glyph.Bar -Color Cyan -NoNewline
    Write-InstallerText -Text "  y" -Color Cyan -NoNewline
    Write-InstallerText -Text " Yes / " -Color DarkGray -NoNewline
    Write-InstallerText -Text "N" -Color Cyan -NoNewline
    Write-InstallerText -Text " No" -Color DarkGray
    Write-InstallerText -Text $Glyph.BarEnd -Color Cyan
    try {
        [Console]::CursorVisible = $false
        while (-not $answered) {
            while (-not [Console]::KeyAvailable) {
                Start-Sleep -Milliseconds 25
            }
            $key = [Console]::ReadKey($true).KeyChar
            if ($key -ceq 'y' -or $key -ceq 'Y') {
                $selected = $true
                $answered = $true
            } elseif ($key -ceq 'n' -or $key -ceq 'N') {
                $selected = $false
                $answered = $true
            }
            # Enter, navigation, and trailing letters are ignored.
        }
    } finally {
        [Console]::CursorVisible = $cursorVisible
        if (-not $answered) {
            # Let the line that ends the frame replace the prompt's corner.
            [Console]::SetCursorPosition(0, [Math]::Max(0, [Console]::CursorTop - 1))
        }
    }

    # Replace the active question with its answer.
    $columns = [Console]::BufferWidth
    $rows = [int][Math]::Ceiling(($Prompt.Length + 3) / $columns) + [int][Math]::Ceiling(15.0 / $columns) + 1
    $row = [Console]::CursorTop
    $firstRow = [Math]::Max(0, $row - $rows)
    for ($clearRow = $firstRow; $clearRow -lt $row; $clearRow++) {
        [Console]::SetCursorPosition(0, $clearRow)
        [Console]::Write(" " * ($columns - 1))
    }
    [Console]::SetCursorPosition(0, $firstRow)
    Write-Info $Prompt -Detail $(if ($selected) { "Yes" } else { "No" })
    return $selected
}

function Confirm-DefaultNo {
    param([string]$Prompt)
    Stop-InstallerProgress
    if (-not (Test-CanPrompt)) {
        return $false
    }
    if ($script:InstallerProgressSupported) {
        return Read-ConfirmAnswer -Prompt $Prompt
    }
    Write-InstallerText -Text $Glyph.Active -Color Cyan -NoNewline
    Write-InstallerText -Text "  $Prompt [y/N] " -NoNewline
    $answer = [Console]::ReadLine()
    return $answer -match "^(y|Y|yes|YES)$"
}

# Failures are thrown to the top-level handler, which reports them in the frame.
function Fail {
    param([string]$Message)
    throw $Message
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
    $version = Get-ReleaseVersion -Tag $Tag
    return "$RootPackage-$version-$Target.zip"
}

function Get-ReleaseVersion {
    param([string]$Tag)
    if ($Tag.StartsWith("v", [StringComparison]::Ordinal)) {
        return $Tag.Substring(1)
    }
    return $Tag
}

# Returns the summary of every advisory whose inclusive start..end range contains
# the version. Only the MAJOR.MINOR.PATCH core is compared.
function Get-MatchingReleaseAdvisories {
    param([string]$Path, [string]$Version)
    if ($Version -notmatch '^(\d+\.\d+\.\d+)(?:[-+].*)?$') {
        return
    }
    $versionCore = [version]$Matches[1]
    $document = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    foreach ($advisory in @($document.advisories)) {
        $start = [string]$advisory.start
        $end = [string]$advisory.end
        $summary = [string]$advisory.summary
        if ($start -notmatch '^\d+\.\d+\.\d+$' -or $end -notmatch '^\d+\.\d+\.\d+$' -or
            [string]::IsNullOrWhiteSpace($summary)) {
            continue
        }
        if ([version]$start -le $versionCore -and $versionCore -le [version]$end) {
            $summary
        }
    }
}

# Release advisories are optional guidance. Any failure to fetch or read them is
# ignored so that it can never delay or fail the install.
function Write-ReleaseAdvisories {
    param([string]$Version, [string]$TempDir)
    try {
        $advisoriesPath = Join-Path $TempDir "release-advisories.json"
        Invoke-WebRequest -Uri $ReleaseAdvisoriesUrl -OutFile $advisoriesPath -TimeoutSec $ReleaseAdvisoriesTimeoutSeconds
        $summaries = @(Get-MatchingReleaseAdvisories -Path $advisoriesPath -Version $Version)
    } catch {
        return
    }
    foreach ($summary in $summaries) {
        Write-WarnLine "Known issue in claude-rs ${Version}: $summary"
    }
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
    try {
        # DeleteOnClose removes the lock file when it is released, even when
        # the installer exits abnormally.
        return New-Object System.IO.FileStream($lockPath, [System.IO.FileMode]::OpenOrCreate, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None, 4096, [System.IO.FileOptions]::DeleteOnClose)
    } catch {
        Fail "another claude-rs installer appears to be running: $lockPath"
    }
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
            try {
                Remove-Item -LiteralPath $backup -Recurse -Force
            } catch {
                if (-not $Update) {
                    throw
                }
                if ($UpdateParentProcessId -gt 0) {
                    try {
                        Start-BackupCleanup -BackupPath $backup -ParentProcessId $UpdateParentProcessId
                        Write-Ok "Scheduled old install cleanup after claude-rs exits"
                    } catch {
                        Write-WarnLine "Updated successfully, but could not schedule cleanup of $backup"
                    }
                } else {
                    Write-WarnLine "Updated successfully. Remove the old install backup after claude-rs exits: $backup"
                }
            }
        }
    } catch {
        if ($backup -and (Test-Path -LiteralPath $backup) -and -not (Test-Path -LiteralPath $FinalApp)) {
            Move-Item -LiteralPath $backup -Destination $FinalApp
        }
        throw
    }
}

function Start-BackupCleanup {
    param([string]$BackupPath, [int]$ParentProcessId)
    $workerScript = @'
$ErrorActionPreference = "SilentlyContinue"
$parentId = [int]$env:CLAUDE_RS_CLEANUP_PARENT_PID
Wait-Process -Id $parentId -ErrorAction SilentlyContinue
$backup = $env:CLAUDE_RS_CLEANUP_BACKUP
for ($attempt = 0; $attempt -lt 20; $attempt++) {
    Remove-Item -LiteralPath $backup -Recurse -Force -ErrorAction SilentlyContinue
    if (-not (Test-Path -LiteralPath $backup)) {
        break
    }
    Start-Sleep -Milliseconds 250
}
'@
    $encodedWorker = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($workerScript))
    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = (Get-Process -Id $PID).Path
    $startInfo.Arguments = "-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand $encodedWorker"
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.WindowStyle = [System.Diagnostics.ProcessWindowStyle]::Hidden
    $startInfo.EnvironmentVariables["CLAUDE_RS_CLEANUP_PARENT_PID"] = [string]$ParentProcessId
    $startInfo.EnvironmentVariables["CLAUDE_RS_CLEANUP_BACKUP"] = $BackupPath
    $worker = [System.Diagnostics.Process]::Start($startInfo)
    if (-not $worker) {
        throw "could not start update cleanup worker"
    }
    $worker.Dispose()
}

function Get-ScriptInstallInfo {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        return $null
    }

    $packageJsonPath = Join-Path $Path "package.json"
    if (-not (Test-Path -LiteralPath $packageJsonPath -PathType Leaf)) {
        return $null
    }
    if (-not (Test-Path -LiteralPath (Join-Path $Path "claude-rs.exe") -PathType Leaf)) {
        return $null
    }
    if (-not (Test-Path -LiteralPath (Join-Path $Path "claude-rs-bridge-bun.exe") -PathType Leaf)) {
        return $null
    }

    try {
        $packageJson = Get-Content -LiteralPath $packageJsonPath -Raw | ConvertFrom-Json
        if (-not ($packageJson.PSObject.Properties.Name -contains "name") -or $packageJson.name -cne $RootPackage) {
            return $null
        }
        $version = if ($packageJson.PSObject.Properties.Name -contains "version") {
            [string]$packageJson.version
        } else {
            $null
        }
        return [pscustomobject]@{
            Path = $Path
            PackageJson = $packageJsonPath
            Version = $version
        }
    } catch {
        return $null
    }
}

function Test-ScriptInstallDirectory {
    param([string]$Path)
    return $null -ne (Get-ScriptInstallInfo -Path $Path)
}

function Confirm-SameVersionReinstall {
    param([string]$Version)
    if ($Update) {
        return $false
    }
    if ($Yes) {
        return $true
    }
    return Confirm-DefaultNo "claude-rs $Version is already installed at $InstallDir. Reinstall the same version?"
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
    # The script install is already complete at this point; a failed npm
    # removal must not fail the install.
    try {
        & $NpmInstall.Npm "uninstall" "-g" $RootPackage | Out-Null
    } catch {
        Write-WarnLine "Could not remove npm install" -Detail $_.Exception.GetBaseException().Message
        return
    }
    if ($LASTEXITCODE -ne 0) {
        Write-WarnLine "Could not remove npm install. Remove manually: npm uninstall -g $RootPackage"
        return
    }
    Write-Ok "Removed npm install"
}

function Resolve-NpmInstallChoice {
    $npmInstall = Get-NpmInstall
    if (-not $npmInstall) {
        return
    }

    Write-WarnLine "Existing npm install found: $RootPackage $($npmInstall.Version)" -Detail "Location: $($npmInstall.Root)"
    if ($RemoveNpm) {
        Remove-NpmInstall -NpmInstall $npmInstall
        return
    }

    if (-not $KeepNpm -and -not $Yes -and (Confirm-DefaultNo "Uninstall this npm installation of claude-rs?")) {
        Remove-NpmInstall -NpmInstall $npmInstall
        return
    }

    Write-WarnLine "Existing npm install kept. Remove later with: npm uninstall -g $RootPackage"
}

# One identity for paths from arguments, the registry, and command discovery.
# Resolve existing Windows files/directories through the handle so junctions,
# symlinks, and short names do not make this installation look like another copy.
function Get-NormalizedPath {
    param([string]$Path)
    $expanded = [Environment]::ExpandEnvironmentVariables($Path).Trim('"')
    $absolute = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($expanded)
    $normalized = [IO.Path]::GetFullPath($absolute)
    if ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT) {
        if (-not ("ClaudeRsInstall.PathIdentity" -as [type])) {
            Add-Type -TypeDefinition @'
namespace ClaudeRsInstall {
    public static class PathIdentity {
        [System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode, SetLastError = true)]
        private static extern Microsoft.Win32.SafeHandles.SafeFileHandle CreateFile(string path, uint access, uint share, System.IntPtr security, uint creation, uint flags, System.IntPtr template);
        [System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode, SetLastError = true)]
        private static extern uint GetFinalPathNameByHandle(Microsoft.Win32.SafeHandles.SafeFileHandle handle, System.Text.StringBuilder path, uint size, uint flags);
        public static string Resolve(string path) {
            using (var handle = CreateFile(path, 0, 7, System.IntPtr.Zero, 3, 0x02000000, System.IntPtr.Zero)) {
                if (handle.IsInvalid) return path;
                var buffer = new System.Text.StringBuilder(32768);
                var length = GetFinalPathNameByHandle(handle, buffer, (uint)buffer.Capacity, 0);
                if (length == 0 || length >= buffer.Capacity) return path;
                var resolved = buffer.ToString();
                if (resolved.StartsWith(@"\\?\UNC\", System.StringComparison.OrdinalIgnoreCase)) return @"\\" + resolved.Substring(8);
                if (resolved.StartsWith(@"\\?\", System.StringComparison.Ordinal)) return resolved.Substring(4);
                return resolved;
            }
        }
    }
}
'@
        }
        $normalized = [ClaudeRsInstall.PathIdentity]::Resolve($normalized)
    }
    if ($normalized.Length -gt ([IO.Path]::GetPathRoot($normalized)).Length) {
        $normalized = $normalized.TrimEnd([char[]]@('\', '/'))
    }
    return $normalized
}

function Split-PathEntries {
    param([string]$PathValue)
    if ([string]::IsNullOrWhiteSpace($PathValue)) {
        return @()
    }
    return $PathValue.Split([IO.Path]::PathSeparator, [StringSplitOptions]::RemoveEmptyEntries)
}

# The user PATH is read and written through the registry with unexpanded
# values. [Environment]::GetEnvironmentVariable expands %VAR% entries and
# SetEnvironmentVariable writes plain REG_SZ, which would permanently freeze
# REG_EXPAND_SZ entries to their current expansion. Writes preserve an existing
# string value's registry kind.
function Get-UserPathRaw {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment")
    if (-not $key) {
        return ""
    }
    try {
        return [string]$key.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    } finally {
        $key.Close()
    }
}

function Set-UserPathRaw {
    param([string]$Value)
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
    try {
        if ([string]::IsNullOrEmpty($Value)) {
            $key.DeleteValue("Path", $false)
        } else {
            $valueKind = [Microsoft.Win32.RegistryValueKind]::ExpandString
            if ($key.GetValueNames() -contains "Path") {
                $existingKind = $key.GetValueKind("Path")
                if ($existingKind -eq [Microsoft.Win32.RegistryValueKind]::String -or
                    $existingKind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) {
                    $valueKind = $existingKind
                }
            }
            $key.SetValue("Path", $Value, $valueKind)
        }
    } finally {
        $key.Close()
    }
    Send-EnvironmentChange
}

function Send-EnvironmentChange {
    try {
        if (-not ("ClaudeRsInstall.NativeMethods" -as [type])) {
            Add-Type -Namespace ClaudeRsInstall -Name NativeMethods -MemberDefinition '[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);'
        }
        $result = [UIntPtr]::Zero
        # HWND_BROADCAST WM_SETTINGCHANGE "Environment" so Explorer and new
        # shells re-read the user PATH without a logoff. Writing the registry
        # directly skips the broadcast SetEnvironmentVariable would have sent.
        [void]([ClaudeRsInstall.NativeMethods]::SendMessageTimeout([IntPtr]0xFFFF, 0x001A, [UIntPtr]::Zero, "Environment", 2, 5000, [ref]$result))
    } catch {
        # Best effort; a missed broadcast only delays PATH pickup until logon.
    }
}

function Expand-PathEntry {
    param([string]$Entry)
    return Get-NormalizedPath -Path $Entry
}

function Add-UserPath {
    param([string]$Directory)
    if ($NoModifyPath) {
        return $false
    }
    $entries = @(Split-PathEntries (Get-UserPathRaw))
    $normalizedDirectory = Get-NormalizedPath -Path $Directory
    $remainingEntries = @($entries | Where-Object { (Expand-PathEntry $_) -ine $normalizedDirectory })
    if ($entries.Count -gt 0 -and (Expand-PathEntry $entries[0]) -ieq $normalizedDirectory) {
        return $false
    }
    Set-UserPathRaw -Value ((@($Directory) + $remainingEntries) -join [IO.Path]::PathSeparator)
    return $true
}

function Remove-UserPath {
    param([string]$Directory)
    $entries = @(Split-PathEntries (Get-UserPathRaw))
    $normalizedDirectory = Get-NormalizedPath -Path $Directory
    $newEntries = @($entries | Where-Object { (Expand-PathEntry $_) -ine $normalizedDirectory })
    if ($newEntries.Count -ne $entries.Count) {
        Set-UserPathRaw -Value ($newEntries -join [IO.Path]::PathSeparator)
        return $true
    }
    return $false
}

function Prepend-ProcessPath {
    param([string]$Directory)
    $normalizedDirectory = Get-NormalizedPath -Path $Directory
    $remainingEntries = @(Split-PathEntries $env:Path | Where-Object { (Expand-PathEntry $_) -ine $normalizedDirectory })
    $env:Path = (@($Directory) + $remainingEntries) -join [IO.Path]::PathSeparator
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
    Complete-InstallerProgress "Verified $versionOutput"
}

function Invoke-InstallDoctor {
    # Windows PowerShell 5.1 turns native stderr into terminating errors when
    # merged with 2>&1 under "Stop"; relax the preference while capturing.
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $doctorLines = @(Invoke-InstalledClaudeRs @("doctor", "--strict") 2>&1 | ForEach-Object { "$_" })
    } finally {
        $ErrorActionPreference = $previousPreference
    }
    $doctorOutput = ($doctorLines -join [Environment]::NewLine).Trim()
    if ($LASTEXITCODE -ne 0) {
        Stop-InstallerProgress
        if (-not [string]::IsNullOrWhiteSpace($doctorOutput)) {
            foreach ($line in $doctorOutput -split "\r?\n") {
                Write-InstallerDetail -Message $line
            }
        }
        Fail "runtime diagnostics failed"
    }
    Complete-InstallerProgress "Runtime diagnostics passed"
}

function Warn-MissingClaudeCli {
    # -ErrorAction SilentlyContinue keeps the check silent; resolves claude.cmd/.ps1/.exe via PATHEXT.
    if (Get-Command "claude" -ErrorAction SilentlyContinue) {
        return
    }
    Write-WarnLine "Claude Code CLI ('claude') not found on PATH" -Detail "Install it from https://claude.com/claude-code"
}

function Test-InstallDirectoryOverlap {
    param([string]$First, [string]$Second)
    $firstPath = (Get-NormalizedPath -Path $First).TrimEnd([char[]]@('\', '/')) + [IO.Path]::DirectorySeparatorChar
    $secondPath = (Get-NormalizedPath -Path $Second).TrimEnd([char[]]@('\', '/')) + [IO.Path]::DirectorySeparatorChar
    return $firstPath.StartsWith($secondPath, [StringComparison]::OrdinalIgnoreCase) -or
        $secondPath.StartsWith($firstPath, [StringComparison]::OrdinalIgnoreCase)
}

function Warn-OtherClaudeRsCommands {
    param([string]$ExpectedCommand)
    $expectedPath = Get-NormalizedPath -Path $ExpectedCommand
    $seen = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    $commands = @(Get-Command "claude-rs" -All -ErrorAction SilentlyContinue)
    foreach ($command in $commands) {
        if ($command.CommandType -ne [System.Management.Automation.CommandTypes]::Application -and
            $command.CommandType -ne [System.Management.Automation.CommandTypes]::ExternalScript) {
            Write-WarnLine "A $($command.CommandType) named claude-rs takes precedence over the installed command" -Detail "Check its definition with: Get-Command claude-rs -All"
            continue
        }
        # Command discovery is a snapshot; an earlier cleanup may have removed
        # this executable through another PATH spelling.
        if (-not (Test-Path -LiteralPath $command.Path -PathType Leaf)) {
            continue
        }
        $candidatePath = Get-NormalizedPath -Path $command.Path
        if ($candidatePath -ieq $expectedPath -or -not $seen.Add($candidatePath)) {
            continue
        }
        $candidateDirectory = Split-Path -Parent $candidatePath
        if (Test-ScriptInstallDirectory -Path $candidateDirectory) {
            Write-WarnLine "Another script installation is also on PATH: $candidatePath" -Detail "Current installation: $expectedPath"
            if (Test-InstallDirectoryOverlap -First $candidateDirectory -Second (Split-Path -Parent $expectedPath)) {
                Write-WarnLine "Cannot remove that copy automatically because its directory overlaps this installation"
            } elseif (-not $Yes -and -not $Update -and
                (Confirm-DefaultNo "Uninstall this other script installation at ${candidateDirectory}?")) {
                try {
                    Uninstall-ScriptInstall -Directory $candidateDirectory
                } catch {
                    Write-WarnLine "Could not uninstall $candidateDirectory" -Detail $_.Exception.GetBaseException().Message
                }
            }
        } else {
            Write-WarnLine "Another claude-rs is also on PATH: $candidatePath" -Detail @(
                "Current installation: $expectedPath",
                "Check PATH order or remove that copy using the tool that installed it."
            )
        }
    }
}

function Uninstall-ScriptInstall {
    param([string]$Directory = $InstallDir)
    $Directory = Get-NormalizedPath -Path $Directory
    $installParent = Split-Path -Parent $Directory
    $lock = Acquire-InstallLock -InstallParent $installParent
    try {
        if ((Test-Path -LiteralPath $Directory) -and -not (Test-ScriptInstallDirectory -Path $Directory)) {
            Write-WarnLine "Not removing $Directory because it does not look like a claude-rs script install"
            return
        }
        if (Remove-UserPath -Directory $Directory) {
            Write-Ok "Removed $Directory from the user PATH"
        }

        if (Test-Path -LiteralPath $Directory) {
            Remove-Item -LiteralPath $Directory -Recurse -Force
            Write-Ok "Removed script install directory $Directory"
        }
    } finally {
        $lock.Dispose()
    }
}

$flowLabel = if ($Uninstall) { "Uninstall" } elseif ($Update) { "Update" } else { "Installation" }
$tempDir = Join-Path ([IO.Path]::GetTempPath()) "claude-rs-install-$PID"
$lockStream = $null
$pathChanged = $false
$sameVersionReinstallApproved = $false
$failed = $false

try {
    $InstallDir = Get-NormalizedPath -Path $InstallDir
    if ($Uninstall) {
        Write-Intro "uninstaller"
        Uninstall-ScriptInstall
        Write-Outro "Uninstall complete"
        return
    }

    if ($Update) {
        Write-Intro "updater"
        if (-not (Test-ScriptInstallDirectory -Path $InstallDir)) {
            Fail "-Update requires an existing claude-rs script install: $InstallDir"
        }
    } else {
        Write-Intro "installer"
    }

    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

    $target = Get-Target
    $targetLabel = if ($target -eq "win32-x64-msvc") { "Windows x64" } else { "Windows arm64" }
    Write-Info "$targetLabel detected"
    Write-Info "Install location: $InstallDir"
    Warn-MissingClaudeCli

    Start-InstallerProgress "Resolving release"
    $tag = Resolve-ReleaseTag -RequestedRelease $Release -TempDir $tempDir
    $selectedVersion = Get-ReleaseVersion -Tag $tag
    $archiveName = Get-ArchiveName -Target $target -Tag $tag
    $baseUrl = "https://github.com/$RepoSlug/releases/download/$tag"
    $checksumsPath = Join-Path $tempDir "SHA256SUMS"
    $archivePath = Join-Path $tempDir $archiveName

    Complete-InstallerProgress "Release $tag selected"

    $existingInstall = Get-ScriptInstallInfo -Path $InstallDir
    if ($existingInstall) {
        if ([string]::IsNullOrWhiteSpace($existingInstall.Version)) {
            Write-WarnLine "Could not determine the version of the existing script install; continuing with installation"
        } elseif ([string]::Equals($existingInstall.Version, $selectedVersion, [StringComparison]::Ordinal)) {
            if (Confirm-SameVersionReinstall -Version $selectedVersion) {
                $sameVersionReinstallApproved = $true
                Write-Info "Reinstalling claude-rs $selectedVersion"
            } else {
                Write-Outro "claude-rs $selectedVersion is already installed; no changes made"
                return
            }
        }
    }

    Write-ReleaseAdvisories -Version $selectedVersion -TempDir $tempDir

    Start-InstallerProgress "Downloading release archive"
    try {
        Invoke-WebRequest -Uri "$baseUrl/SHA256SUMS" -OutFile $checksumsPath
    } catch {
        Write-WarnDetail "Download failed: $($_.Exception.GetBaseException().Message)"
        Fail "could not download SHA256SUMS for $tag"
    }
    try {
        Invoke-ArchiveDownload -Uri "$baseUrl/$archiveName" -Destination $archivePath
    } catch {
        Write-WarnDetail "Download failed: $($_.Exception.GetBaseException().Message)"
        Fail "install script is currently not available for this release"
    }

    Start-InstallerProgress "Verifying release archive"
    $expectedSha = Get-ExpectedSha256 -ChecksumsPath $checksumsPath -ArchiveName $archiveName
    $actualSha = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualSha -ne $expectedSha) {
        Fail "checksum mismatch for $archiveName"
    }
    Complete-InstallerProgress "Verified release archive integrity"

    Start-InstallerProgress "Installing files"
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
    if (-not $sameVersionReinstallApproved) {
        $lockedInstall = Get-ScriptInstallInfo -Path $InstallDir
        if ($lockedInstall -and
            -not [string]::IsNullOrWhiteSpace($lockedInstall.Version) -and
            [string]::Equals($lockedInstall.Version, $selectedVersion, [StringComparison]::Ordinal)) {
            Write-Outro "claude-rs $selectedVersion was installed by another installer; no changes made"
            return
        }
    }
    Replace-AppDirectory -SourceApp $extractedApp -FinalApp $InstallDir
    Complete-InstallerProgress "Installed files"

    Prepend-ProcessPath -Directory $InstallDir
    if ($Update) {
        Write-Ok "Preserved existing PATH configuration"
    } else {
        $pathChanged = Add-UserPath -Directory $InstallDir
        if ($pathChanged) {
            Write-Ok "Updated PATH for new shells"
        } elseif ($NoModifyPath) {
            Write-WarnLine "PATH update skipped"
        } else {
            Write-Ok "PATH already points to this script install"
        }
    }

    Start-InstallerProgress "Verifying installed command"
    Assert-InstalledCommand
    if ($Verify) {
        Start-InstallerProgress "Running runtime diagnostics"
        Invoke-InstallDoctor
    }

    # Only offer to remove an existing npm install after the script install
    # has fully succeeded, so a failed install never leaves the user without
    # claude-rs.
    # Release the completed installation's lock before optional cleanup of
    # another copy, which can share the same parent directory.
    $lockStream.Dispose()
    $lockStream = $null
    if (-not $Update) {
        Resolve-NpmInstallChoice
    }

    $expectedCommand = Join-Path $InstallDir "claude-rs.exe"
    Warn-OtherClaudeRsCommands -ExpectedCommand $expectedCommand

    if ($Update) {
        Write-Outro "claude-rs is updated. Start claude-rs again to use v$selectedVersion."
    } elseif ($Run -or ((-not $Yes) -and (Confirm-DefaultNo "Start claude-rs now?"))) {
        Write-Outro "claude-rs $selectedVersion is installed"
        Invoke-InstalledClaudeRs @()
    } else {
        if ($NoModifyPath) {
            Write-Note "Next steps" @("Run claude-rs directly:", "  $expectedCommand")
        } else {
            Write-Note "Next steps" @("Start a new shell, then run:", "  claude-rs")
        }
        Write-Outro "claude-rs $selectedVersion is installed"
    }
} catch {
    $failed = $true
    Write-FailLine $_.Exception.GetBaseException().Message
} finally {
    Stop-InstallerProgress
    # A frame still open here was ended by a failure, or by Ctrl-C when nothing
    # was caught.
    Close-InstallerFrame $(if ($failed) { "$flowLabel failed" } else { "$flowLabel cancelled" })
    if ($lockStream) {
        $lockStream.Dispose()
    }
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    if ($script:PreviousOutputEncoding) {
        try {
            [Console]::OutputEncoding = $script:PreviousOutputEncoding
        } catch {
            # Best effort; the calling shell keeps UTF-8 console output.
        }
    }
}

# Exiting only on failure keeps a successful `irm | iex` run from closing the
# calling shell.
if ($failed) {
    exit 1
}
