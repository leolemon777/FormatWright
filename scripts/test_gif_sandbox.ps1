#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "GIF sandbox assertion failed: $Message" }
}

function Invoke-FormatWrightJson {
    param([string[]]$Arguments, [int[]]$ExpectedExitCodes = @(0))
    $lines = & $script:BinaryPath @Arguments 2>$null
    $exitCode = $LASTEXITCODE
    Assert-True ($ExpectedExitCodes -contains $exitCode) (
        "unexpected exit code $exitCode for: formatwright " + ($Arguments -join ' ')
    )
    $text = $lines -join "`n"
    Assert-True (-not [string]::IsNullOrWhiteSpace($text)) 'JSON stdout was empty'
    [pscustomobject]@{ ExitCode = $exitCode; Data = $text | ConvertFrom-Json }
}

foreach ($tool in @('ffmpeg', 'ffprobe')) {
    Assert-True ($null -ne (Get-Command $tool -ErrorAction SilentlyContinue)) "$tool is required"
}
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'gif-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath | Out-Null

$input = Join-Path $casePath '动画 source with 空格.mkv'
& ffmpeg -v error `
    -f lavfi -i 'testsrc2=size=320x180:rate=30' `
    -f lavfi -i 'sine=frequency=660:sample_rate=48000' `
    -t 4 -c:v libx264 -preset ultrafast -c:a aac $input
Assert-True ($LASTEXITCODE -eq 0) 'failed to generate GIF source fixture'
$inputHash = (Get-FileHash -LiteralPath $input -Algorithm SHA256).Hash

$output = Join-Path $casePath 'result 动画.gif'
$database = Join-Path $casePath 'gif.sqlite3'
$plan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'plan', $input, '--to', 'gif', '--output', $output,
    '--start-ms', '500', '--duration-ms', '1500', '--width', '240',
    '--fps', '12', '--loop-count', '2'
)
Assert-True ($plan.Data.steps[0].operation -eq 'transcode') 'GIF must be a transcode'
Assert-True ($plan.Data.steps[0].loss_class -eq 'lossy') 'GIF must be classified lossy'
Assert-True ($plan.Data.steps[0].arguments.start_millis -eq '500') 'start was not planned'
Assert-True ($plan.Data.steps[0].arguments.duration_millis -eq '1500') 'duration was not planned'
Assert-True ($plan.Data.steps[0].arguments.width -eq '240') 'width was not planned'
Assert-True ($plan.Data.steps[0].arguments.frames_per_second -eq '12') 'frame rate was not planned'
Assert-True ($plan.Data.steps[0].arguments.loop_count -eq '2') 'loop count was not planned'
Assert-True ($plan.Data.steps[0].arguments.palette_max_colors -eq '256') 'palette size was not explicit'

$conversion = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', $database, 'convert', $input, '--to', 'gif',
    '--output', $output, '--start-ms', '500', '--duration-ms', '1500',
    '--width', '240', '--fps', '12', '--loop-count', '2'
)
Assert-True ($conversion.Data.status -eq 'pass') 'GIF validation did not pass'
Assert-True (Test-Path -LiteralPath $output -PathType Leaf) 'GIF output is missing'

$probeLines = & ffprobe -v error -count_frames -select_streams v:0 `
    -show_entries 'stream=codec_name,width,height,nb_read_frames:format=format_name,duration' `
    -of json $output 2>$null
Assert-True ($LASTEXITCODE -eq 0) 'independent ffprobe could not open GIF'
$probe = ($probeLines -join "`n") | ConvertFrom-Json
Assert-True ($probe.format.format_name -eq 'gif') 'independent probe did not detect GIF'
Assert-True ($probe.streams[0].codec_name -eq 'gif') 'GIF video codec mismatch'
Assert-True ($probe.streams[0].width -eq 240) 'GIF width mismatch'
Assert-True ($probe.streams[0].height -eq 136) 'GIF aspect-ratio height mismatch'
$frames = [int]$probe.streams[0].nb_read_frames
Assert-True ($frames -ge 17 -and $frames -le 19) 'GIF frame count is outside the 12fps tolerance'
$duration = [double]$probe.format.duration
Assert-True ([Math]::Abs($duration - 1.5) -le 0.25) 'GIF duration is outside tolerance'

$zeroDuration = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @(
    '--json', 'plan', $input, '--to', 'gif', '--duration-ms', '0'
)
Assert-True ($zeroDuration.Data.code -eq 'INPUT_INVALID') 'zero duration was not rejected'
$badFps = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @(
    '--json', 'plan', $input, '--to', 'gif', '--fps', '61'
)
Assert-True ($badFps.Data.code -eq 'INPUT_INVALID') 'unbounded GIF frame rate was not rejected'
$outsideRange = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @(
    '--json', 'plan', $input, '--to', 'gif', '--start-ms', '5000'
)
Assert-True ($outsideRange.Data.code -eq 'INPUT_INVALID') 'out-of-range GIF start was not rejected'

$disguised = Join-Path $casePath 'actually-gif.bin'
Copy-Item -LiteralPath $output -Destination $disguised
$wrongExtension = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $disguised)
Assert-True ($wrongExtension.Data.format.id -eq 'gif') 'header-first probe missed disguised GIF'
Assert-True ($wrongExtension.Data.format.extension_matches -eq $false) 'GIF extension mismatch was missed'
Assert-True (
    $inputHash -eq (Get-FileHash -LiteralPath $input -Algorithm SHA256).Hash
) 'GIF conversion modified the source'
Assert-True (
    @(Get-ChildItem -LiteralPath $casePath -Filter '.formatwright-partial-*' -File).Count -eq 0
) 'GIF suite left a staged output'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    status = $conversion.Data.status
    format = $probe.format.format_name
    codec = $probe.streams[0].codec_name
    width = $probe.streams[0].width
    height = $probe.streams[0].height
    frames = $frames
    duration_seconds = $duration
    invalid_duration_blocked = $true
    invalid_fps_blocked = $true
    invalid_range_blocked = $true
    wrong_extension_detected = $true
    source_unchanged = $true
    staged_outputs_remaining = 0
}
$summaryPath = Join-Path $casePath 'gif-sandbox-result.json'
$summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $summaryPath -Encoding utf8
$summary | ConvertTo-Json -Depth 6
