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
    if (-not $Condition) { throw "metadata sandbox assertion failed: $Message" }
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
    [pscustomobject]@{ ExitCode = $exitCode; Text = $text; Data = $text | ConvertFrom-Json }
}

function Invoke-Ffmpeg {
    param([string[]]$Arguments)
    & ffmpeg @Arguments 2>$null
    Assert-True ($LASTEXITCODE -eq 0) ('fixture generation failed: ' + ($Arguments -join ' '))
}

function Get-Ffprobe {
    param([string]$Path)
    $lines = & ffprobe -v error -show_format -show_streams -show_chapters -of json $Path 2>$null
    Assert-True ($LASTEXITCODE -eq 0) "ffprobe could not open $Path"
    ($lines -join "`n") | ConvertFrom-Json
}

foreach ($tool in @('ffmpeg', 'ffprobe')) {
    Assert-True ($null -ne (Get-Command $tool -ErrorAction SilentlyContinue)) "$tool is required"
}
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'metadata-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath | Out-Null

$input = Join-Path $casePath 'tagged source.mkv'
Invoke-Ffmpeg @(
    '-v', 'error',
    '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24',
    '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000',
    '-t', '2', '-c:v', 'libx264', '-preset', 'ultrafast', '-c:a', 'aac',
    '-metadata', 'title=Private title',
    '-metadata', 'artist=Private artist',
    '-metadata', 'comment=Private comment',
    '-metadata', 'custom_tag=RetainMe',
    $input
)
$sourceHash = (Get-FileHash -LiteralPath $input -Algorithm SHA256).Hash
$inputProbe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $input)
$privateKeys = @(
    $inputProbe.Data.metadata.PSObject.Properties |
        Where-Object { $_.Value.classification -in @('private', 'secret') } |
        ForEach-Object Name
)
$unknownKeys = @(
    $inputProbe.Data.metadata.PSObject.Properties |
        Where-Object { $_.Value.classification -eq 'unknown' } |
        ForEach-Object Name
)
Assert-True ($privateKeys.Count -ge 3) 'private title/artist/comment were not classified'
Assert-True ($unknownKeys -contains 'CUSTOM_TAG') 'unknown custom metadata was not retained by policy'

$output = Join-Path $casePath 'tagged source.cleaned.mkv'
$plan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'clean', $input, '--output', $output, '--dry-run'
)
Assert-True ($plan.Data.steps[0].operation -eq 'metadata-clean') 'wrong clean operation'
Assert-True ($plan.Data.steps[0].arguments.payload_mode -eq 'copy') 'metadata clean would re-encode payload'
foreach ($key in $privateKeys) {
    Assert-True ($plan.Data.constraints.removed_metadata_keys -contains $key) "Plan omitted private key $key"
}
Assert-True ($plan.Data.constraints.retained_metadata_keys -contains 'CUSTOM_TAG') 'Plan omitted retained unknown key'
Assert-True (-not $plan.Text.Contains('Private title')) 'Plan leaked a removed metadata value'
Assert-True (-not $plan.Text.Contains('Private artist')) 'Plan leaked a removed metadata value'

$clean = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'clean.sqlite3'),
    'clean', $input, '--output', $output
)
Assert-True ($clean.Data.status -eq 'pass') 'metadata-clean output did not validate'
$independent = Get-Ffprobe -Path $output
$outputTags = $independent.format.tags
foreach ($key in @('title', 'artist', 'comment')) {
    Assert-True ($null -eq $outputTags.PSObject.Properties[$key]) "private metadata remains: $key"
    Assert-True ($null -eq $outputTags.PSObject.Properties[$key.ToUpperInvariant()]) "private metadata remains: $key"
}
Assert-True ($outputTags.CUSTOM_TAG -eq 'RetainMe') 'unknown metadata was not retained'
Assert-True ($independent.streams[0].codec_name -eq 'h264') 'video payload codec changed'
Assert-True ($independent.streams[1].codec_name -eq 'aac') 'audio payload codec changed'
Assert-True ($independent.streams[0].width -eq 320 -and $independent.streams[0].height -eq 180) 'dimensions changed'

$inPlace = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', 'clean', $input, '--output', $input, '--dry-run'
)
Assert-True ($inPlace.Data.code -eq 'POLICY_BLOCKED') 'in-place clean was accepted'
$conflict = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'conflict.sqlite3'),
    'clean', $input, '--output', $output
)
Assert-True ($conflict.Data.code -eq 'OUTPUT_CONFLICT') 'existing clean output was overwritten'
Assert-True (
    $sourceHash -eq (Get-FileHash -LiteralPath $input -Algorithm SHA256).Hash
) 'metadata cleaning modified the source'
Assert-True (
    @(Get-ChildItem -LiteralPath $casePath -Filter '.formatwright-partial-*' -File).Count -eq 0
) 'metadata suite left staged output files'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    status = $clean.Data.status
    removed_private_keys = $privateKeys
    retained_unknown_keys = $unknownKeys
    payload_codecs = @($independent.streams | ForEach-Object codec_name)
    dimensions = '320x180'
    plan_values_redacted = $true
    in_place_blocked = $true
    output_conflict_blocked = $true
    source_unchanged = $true
    staged_outputs_remaining = 0
}
$summaryPath = Join-Path $casePath 'metadata-sandbox-result.json'
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8
$summary | ConvertTo-Json -Depth 8
