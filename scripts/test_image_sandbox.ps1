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
    if (-not $Condition) { throw "image sandbox assertion failed: $Message" }
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

function Get-ImageFacts {
    param([string]$Path)
    $lines = & ffprobe -v error -select_streams v:0 `
        -show_entries stream=codec_name,width,height,pix_fmt -of json $Path 2>$null
    Assert-True ($LASTEXITCODE -eq 0) "ffprobe could not open $Path"
    (($lines -join "`n") | ConvertFrom-Json).streams[0]
}

foreach ($tool in @('ffmpeg', 'ffprobe')) {
    Assert-True ($null -ne (Get-Command $tool -ErrorAction SilentlyContinue)) "$tool is required"
}
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'image-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath | Out-Null

$pngInput = Join-Path $casePath 'opaque source.png'
Invoke-Ffmpeg @(
    '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=1',
    '-frames:v', '1', $pngInput
)
$pngHash = (Get-FileHash -LiteralPath $pngInput -Algorithm SHA256).Hash
$pngProbe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $pngInput)
Assert-True ($pngProbe.Data.format.id -eq 'png') 'PNG format was not normalized'
Assert-True ($pngProbe.Data.format.kind -eq 'image') 'PNG was not classified as an image'

$webpOutput = Join-Path $casePath 'scaled.webp'
$webpPlan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'plan', $pngInput, '--to', 'webp', '--width', '160', '--quality', '88',
    '--output', $webpOutput
)
Assert-True ($webpPlan.Data.target_format -eq 'webp') 'WebP target was not planned'
Assert-True ($webpPlan.Data.steps[0].arguments.quality -eq '88') 'WebP quality was not explicit'
$webp = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'webp.sqlite3'),
    'convert', $pngInput, '--to', 'webp', '--width', '160', '--quality', '88',
    '--output', $webpOutput
)
Assert-True ($webp.Data.status -eq 'pass') 'PNG to WebP did not validate'
$webpFacts = Get-ImageFacts -Path $webpOutput
Assert-True ($webpFacts.codec_name -eq 'webp') 'independent probe did not detect WebP'
Assert-True ($webpFacts.width -eq 160 -and $webpFacts.height -eq 90) 'WebP resize changed aspect ratio'

$jpegInput = Join-Path $casePath 'photo source.jpg'
Invoke-Ffmpeg @(
    '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=192x128:rate=1',
    '-frames:v', '1', '-q:v', '3', $jpegInput
)
$avifOutput = Join-Path $casePath 'photo output.avif'
$avif = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'avif.sqlite3'),
    'convert', $jpegInput, '--to', 'avif', '--quality', '70', '--output', $avifOutput
)
Assert-True ($avif.Data.status -eq 'pass') 'JPEG to AVIF did not validate'
$avifFacts = Get-ImageFacts -Path $avifOutput
Assert-True ($avifFacts.codec_name -eq 'av1') 'independent probe did not detect AV1 in AVIF'

$pngOutput = Join-Path $casePath 'lossless copy.png'
$pngPlan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'plan', $jpegInput, '--to', 'png', '--output', $pngOutput
)
Assert-True ($pngPlan.Data.steps[0].loss_class -eq 'lossless') 'decoded image to PNG was not lossless'
$png = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'png.sqlite3'),
    'convert', $jpegInput, '--to', 'png', '--output', $pngOutput
)
Assert-True ($png.Data.status -eq 'pass') 'JPEG to PNG did not validate'

$alphaInput = Join-Path $casePath 'transparent.png'
Invoke-Ffmpeg @(
    '-v', 'error', '-f', 'lavfi', '-i', 'color=c=red@0.25:s=128x96:rate=1,format=rgba',
    '-frames:v', '1', $alphaInput
)
$alphaProbe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $alphaInput)
Assert-True ($alphaProbe.Data.streams[0].properties.pix_fmt -match 'a') 'alpha fixture has no alpha pixel format'
$alphaJpeg = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', 'plan', $alphaInput, '--to', 'jpg'
)
Assert-True ($alphaJpeg.Data.code -eq 'POLICY_BLOCKED') 'JPEG silently dropped alpha'
$alphaWebpOutput = Join-Path $casePath 'transparent.webp'
$alphaWebp = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'alpha-webp.sqlite3'),
    'convert', $alphaInput, '--to', 'webp', '--output', $alphaWebpOutput
)
Assert-True ($alphaWebp.Data.status -eq 'pass') 'WebP did not preserve required alpha'

$badQuality = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @(
    '--json', 'plan', $pngInput, '--to', 'webp', '--quality', '0'
)
Assert-True ($badQuality.Data.code -eq 'INPUT_INVALID') 'zero image quality was accepted'
$badWidth = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @(
    '--json', 'plan', $pngInput, '--to', 'webp', '--width', '0'
)
Assert-True ($badWidth.Data.code -eq 'INPUT_INVALID') 'zero image width was accepted'

$disguised = Join-Path $casePath 'actually-png.bin'
Copy-Item -LiteralPath $pngInput -Destination $disguised
$disguisedProbe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $disguised)
Assert-True ($disguisedProbe.Data.format.id -eq 'png') 'header-first probe missed disguised PNG'
Assert-True ($disguisedProbe.Data.format.extension_matches -eq $false) 'PNG extension mismatch was missed'

$conflict = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'conflict.sqlite3'),
    'convert', $pngInput, '--to', 'webp', '--output', $webpOutput
)
Assert-True ($conflict.Data.code -eq 'OUTPUT_CONFLICT') 'existing image output was overwritten'
Assert-True (
    $pngHash -eq (Get-FileHash -LiteralPath $pngInput -Algorithm SHA256).Hash
) 'conversion modified the PNG input'
Assert-True (
    @(Get-ChildItem -LiteralPath $casePath -Filter '.formatwright-partial-*' -File).Count -eq 0
) 'image suite left staged output files'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    webp = [ordered]@{ status = $webp.Data.status; codec = $webpFacts.codec_name; dimensions = '160x90' }
    avif = [ordered]@{ status = $avif.Data.status; codec = $avifFacts.codec_name }
    png = [ordered]@{ status = $png.Data.status; loss_class = $pngPlan.Data.steps[0].loss_class }
    alpha_to_jpeg_blocked = $true
    alpha_to_webp = $alphaWebp.Data.status
    invalid_constraints_blocked = $true
    wrong_extension_detected = $true
    output_conflict_blocked = $true
    source_unchanged = $true
    staged_outputs_remaining = 0
}
$summaryPath = Join-Path $casePath 'image-sandbox-result.json'
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8
$summary | ConvertTo-Json -Depth 8
