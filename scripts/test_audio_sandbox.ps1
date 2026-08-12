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
    if (-not $Condition) {
        throw "audio sandbox assertion failed: $Message"
    }
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

function Invoke-Ffmpeg {
    param([string[]]$Arguments)
    & ffmpeg @Arguments 2>$null
    Assert-True ($LASTEXITCODE -eq 0) ('fixture generation failed: ' + ($Arguments -join ' '))
}

function Get-AudioCodec {
    param([string]$Path)
    $lines = & ffprobe -v error -select_streams a:0 -show_entries stream=codec_name -of json $Path 2>$null
    Assert-True ($LASTEXITCODE -eq 0) "ffprobe could not open $Path"
    $probe = ($lines -join "`n") | ConvertFrom-Json
    [string]$probe.streams[0].codec_name
}

foreach ($tool in @('ffmpeg', 'ffprobe')) {
    Assert-True ($null -ne (Get-Command $tool -ErrorAction SilentlyContinue)) "$tool is required"
}
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'audio-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath | Out-Null

$multiTrack = Join-Path $casePath '多音轨 source.mkv'
Invoke-Ffmpeg @(
    '-v', 'error',
    '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24',
    '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000',
    '-f', 'lavfi', '-i', 'sine=frequency=880:sample_rate=48000',
    '-t', '2', '-map', '0:v', '-map', '1:a', '-map', '2:a',
    '-metadata:s:a:0', 'language=eng', '-metadata:s:a:1', 'language=zho',
    '-c:v', 'libx264', '-preset', 'ultrafast', '-c:a', 'aac',
    $multiTrack
)
$inputHash = (Get-FileHash -LiteralPath $multiTrack -Algorithm SHA256).Hash

$blocked = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', 'plan', $multiTrack, '--to', 'mp3', '--audio-stream', '2'
)
Assert-True ($blocked.Data.code -eq 'POLICY_BLOCKED') 'multiple tracks were silently reduced'

$mp3Output = Join-Path $casePath 'selected 中文 track.mp3'
$mp3Db = Join-Path $casePath 'mp3.sqlite3'
$mp3Plan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'plan', $multiTrack, '--to', 'mp3', '--audio-stream', '2',
    '--allow-stream-drop', '--output', $mp3Output
)
Assert-True ($mp3Plan.Data.steps[0].operation -eq 'transcode') 'AAC to MP3 must transcode'
Assert-True ($mp3Plan.Data.steps[0].arguments.audio_stream_index -eq '2') 'wrong audio stream planned'
Assert-True ($mp3Plan.Data.steps[0].arguments.audio_mode -eq 'libmp3lame') 'wrong MP3 encoder planned'
$mp3 = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', $mp3Db, 'convert', $multiTrack, '--to', 'mp3',
    '--audio-stream', '2', '--allow-stream-drop', '--output', $mp3Output
)
Assert-True ($mp3.Data.status -eq 'pass') 'selected MP3 conversion did not validate'
Assert-True ((Get-AudioCodec -Path $mp3Output) -eq 'mp3') 'independent probe did not detect MP3 audio'

$m4aOutput = Join-Path $casePath 'remuxed.m4a'
$m4aPlan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'plan', $multiTrack, '--to', 'm4a', '--audio-stream', '1',
    '--allow-stream-drop', '--output', $m4aOutput
)
Assert-True ($m4aPlan.Data.steps[0].operation -eq 'remux') 'AAC to M4A should remux'
Assert-True ($m4aPlan.Data.steps[0].arguments.audio_mode -eq 'copy') 'AAC remux should copy audio'
$m4a = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'm4a.sqlite3'), 'convert', $multiTrack,
    '--to', 'm4a', '--audio-stream', '1', '--allow-stream-drop', '--output', $m4aOutput
)
Assert-True ($m4a.Data.status -eq 'pass') 'M4A remux did not validate'
Assert-True ((Get-AudioCodec -Path $m4aOutput) -eq 'aac') 'independent probe did not detect AAC'

$flacInput = Join-Path $casePath 'lossless input.flac'
Invoke-Ffmpeg @(
    '-v', 'error', '-f', 'lavfi', '-i', 'sine=frequency=523:sample_rate=48000',
    '-t', '2', '-c:a', 'flac', '-metadata', 'title=FormatWright fixture', $flacInput
)
$wavOutput = Join-Path $casePath 'lossless-output.wav'
$wavPlan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'plan', $flacInput, '--to', 'wav', '--output', $wavOutput
)
Assert-True ($wavPlan.Data.steps[0].loss_class -eq 'lossless') 'FLAC to WAV must be lossless'
$wav = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'wav.sqlite3'), 'convert', $flacInput,
    '--to', 'wav', '--output', $wavOutput
)
Assert-True ($wav.Data.status -eq 'pass') 'FLAC to WAV did not validate'
Assert-True ((Get-AudioCodec -Path $wavOutput) -eq 'pcm_s16le') 'WAV codec was not PCM s16le'

$disguised = Join-Path $casePath 'actually-flac.bin'
Copy-Item -LiteralPath $flacInput -Destination $disguised
$wrongExtension = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $disguised)
Assert-True ($wrongExtension.Data.format.id -eq 'flac') 'header-first probe missed disguised FLAC'
Assert-True ($wrongExtension.Data.format.extension_matches -eq $false) 'FLAC extension mismatch was missed'

$videoOnly = Join-Path $casePath 'video-only.mkv'
Invoke-Ffmpeg @(
    '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=160x90:rate=10',
    '-t', '1', '-an', '-c:v', 'libx264', '-preset', 'ultrafast', $videoOnly
)
$noAudio = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @(
    '--json', 'plan', $videoOnly, '--to', 'mp3'
)
Assert-True ($noAudio.Data.code -eq 'INPUT_INVALID') 'video without audio was not rejected in planning'

Assert-True (
    $inputHash -eq (Get-FileHash -LiteralPath $multiTrack -Algorithm SHA256).Hash
) 'conversion modified the multi-track input'
Assert-True (
    @(Get-ChildItem -LiteralPath $casePath -Filter '.formatwright-partial-*' -File).Count -eq 0
) 'audio suite left staged output files'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    multiple_audio_policy = 'blocked without --allow-stream-drop'
    mp3 = [ordered]@{ status = $mp3.Data.status; codec = Get-AudioCodec -Path $mp3Output }
    m4a = [ordered]@{ status = $m4a.Data.status; codec = Get-AudioCodec -Path $m4aOutput; operation = $m4aPlan.Data.steps[0].operation }
    wav = [ordered]@{ status = $wav.Data.status; codec = Get-AudioCodec -Path $wavOutput; loss_class = $wavPlan.Data.steps[0].loss_class }
    wrong_extension_detected = $true
    no_audio_blocked = $true
    source_unchanged = $true
    staged_outputs_remaining = 0
}
$summaryPath = Join-Path $casePath 'audio-sandbox-result.json'
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8
$summary | ConvertTo-Json -Depth 8
