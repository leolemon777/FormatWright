#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts'),
    [string]$Python = '',
    [string]$HeifConvert = '',
    [string]$Ffprobe = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "HEIC sandbox assertion failed: $Message" }
}

function Resolve-ToolPath {
    param([string]$Explicit, [string]$EnvironmentName, [string]$CommandName)
    if (-not [string]::IsNullOrWhiteSpace($Explicit)) {
        return (Resolve-Path -LiteralPath $Explicit).Path
    }
    $environmentValue = [Environment]::GetEnvironmentVariable($EnvironmentName)
    if (-not [string]::IsNullOrWhiteSpace($environmentValue)) {
        return (Resolve-Path -LiteralPath $environmentValue).Path
    }
    $command = Get-Command $CommandName -ErrorAction SilentlyContinue
    Assert-True ($null -ne $command) "$CommandName is required"
    return $command.Source
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

$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
$pythonPath = Resolve-ToolPath -Explicit $Python -EnvironmentName 'FORMATWRIGHT_TEST_PYTHON' -CommandName 'python'
$heifConvertPath = Resolve-ToolPath -Explicit $HeifConvert -EnvironmentName 'FORMATWRIGHT_ENGINE_HEIF_CONVERT' -CommandName 'heif-convert'
$ffprobePath = Resolve-ToolPath -Explicit $Ffprobe -EnvironmentName 'FORMATWRIGHT_ENGINE_FFPROBE' -CommandName 'ffprobe'
$env:FORMATWRIGHT_ENGINE_HEIF_CONVERT = $heifConvertPath
$env:FORMATWRIGHT_ENGINE_FFPROBE = $ffprobePath

New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'heic-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath | Out-Null

# 64x64 deterministic HEVC HEIC color fixture from the libheif fuzz corpus.
# Upstream: strukturag/libheif, fuzzing/data/corpus/colors-no-alpha.heic.
# libheif is LGPL-3.0-or-later; this byte fixture is generated only in ignored test artifacts.
$fixtureBase64 = 'AAAAGGZ0eXBoZWljAAAAAG1pZjFoZWljAAABLm1ldGEAAAAAAAAAIWhkbHIAAAAAAAAAAHBpY3QAAAAAAAAAAAAAAAAAAAAADnBpdG0AAAAAAAEAAAAiaWxvYwAAAABEQAABAAEAAAAAAU4AAQAAAAAAAAClAAAAI2lpbmYAAAAAAAEAAAAVaW5mZQIAAAAAAQAAaHZjMQAAAACuaXBycAAAAJFpcGNvAAAAdWh2Y0MBA3AAAAAAAAAAAAAe8AD8/fj4AAAPAyAAAQAYQAEMAf//A3AAAAMAkAAAAwAAAwAeugJAIQABAChCAQEDcAAAAwCQAAADAAADAB6gIIEFlupJKa5sCAAAAwAIAAADAAhAIgABAAdEAcFysCJAAAAAFGlzcGUAAAAAAAAAQAAAAEAAAAAVaXBtYQAAAAAAAAABAAECgQIAAACtbWRhdAAAAKEmAa8TgIGSEXXAGM2sfMMD8HKXsBNBYjkEW6//QKl1HfLCc/SN/bWOG2ARaa8rk4JsxRuKJFz/vIlnrSBv0Pk7pYMv503LniUfVt0RGOMyTBZVcbnDhlXs0nsTVObq7679Fh7MfXPARYndCrwpKWSNTQcCjNVYWPVOenDxU81lLBnE070xnN107IoLiTNywdiNWzedf/q6zzV3iwZflrO94A=='
$heic = Join-Path $casePath '颜色 fixture.heic'
[IO.File]::WriteAllBytes($heic, [Convert]::FromBase64String($fixtureBase64))
Assert-True ((Get-FileHash -LiteralPath $heic -Algorithm SHA256).Hash -eq '76F82FFC717A647B1C9C2551E5EA0545832A2D3216C7540F7E5B092282A04B63') 'fixture hash mismatch'
$sourceHash = (Get-FileHash -LiteralPath $heic -Algorithm SHA256).Hash
Copy-Item -LiteralPath $heic -Destination (Join-Path $casePath 'disguised.bin')
[IO.File]::WriteAllBytes((Join-Path $casePath 'truncated.heic'), [IO.File]::ReadAllBytes($heic)[0..63])

$probe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $heic)
Assert-True ($probe.Data.format.id -eq 'heic') 'HEIC content detection failed'
Assert-True ($probe.Data.streams[0].width -eq 64 -and $probe.Data.streams[0].height -eq 64) 'HEIC dimensions mismatch'
$disguised = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', (Join-Path $casePath 'disguised.bin'))
Assert-True ($disguised.Data.format.id -eq 'heic') 'wrong-extension HEIC detection failed'
Assert-True ($disguised.Data.format.extension_matches -eq $false) 'HEIC extension mismatch missing'

$jpeg = Join-Path $casePath '颜色 output.jpg'
$jpegPlan = Invoke-FormatWrightJson -Arguments @('--json', 'plan', $heic, '--to', 'jpg', '--quality', '82', '--output', $jpeg)
Assert-True ($jpegPlan.Data.steps[0].engine.engine_id -eq 'heif-convert') 'libheif fallback was not selected'
Assert-True ($jpegPlan.Data.steps[0].arguments.quality -eq '82') 'JPEG quality missing from Plan'
Assert-True ($jpegPlan.Data.steps[0].arguments.metadata -eq 'drop') 'metadata policy missing from Plan'
$jpegResult = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'jpeg.sqlite3'),
    'convert', $heic, '--to', 'jpg', '--quality', '82', '--output', $jpeg
)
Assert-True ($jpegResult.Data.status -eq 'pass') 'HEIC to JPEG did not validate'

$png = Join-Path $casePath '颜色 output.png'
$pngResult = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'png.sqlite3'),
    'convert', $heic, '--to', 'png', '--output', $png
)
Assert-True ($pngResult.Data.status -eq 'pass') 'HEIC to PNG did not validate'

$pixelVerifier = @'
from PIL import Image
import sys
for path, expected in ((sys.argv[1], "JPEG"), (sys.argv[2], "PNG")):
    with Image.open(path) as image:
        image.load()
        assert image.format == expected
        assert image.size == (64, 64)
        extrema = image.convert("RGB").getextrema()
        assert any(low != high for low, high in extrema)
'@
$pixelVerifier | & $pythonPath - $jpeg $png
Assert-True ($LASTEXITCODE -eq 0) 'independent Pillow HEIC output validation failed'
foreach ($output in @($jpeg, $png)) {
    $probeJson = & $ffprobePath -v error -show_entries stream=codec_name,width,height -of json $output | ConvertFrom-Json
    Assert-True (@($probeJson.streams).Count -eq 1) 'independent ffprobe stream count mismatch'
    Assert-True ($probeJson.streams[0].width -eq 64 -and $probeJson.streams[0].height -eq 64) 'independent ffprobe dimensions mismatch'
}

$qualityError = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @('--json', 'plan', $heic, '--to', 'jpg', '--quality', '0')
Assert-True ($qualityError.Data.code -eq 'INPUT_INVALID') 'invalid HEIC JPEG quality was accepted'
$pngQuality = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @('--json', 'plan', $heic, '--to', 'png', '--quality', '80')
Assert-True ($pngQuality.Data.code -eq 'INPUT_INVALID') 'PNG quality was accepted'
$resize = Invoke-FormatWrightJson -ExpectedExitCodes @(3) -Arguments @('--json', 'plan', $heic, '--to', 'jpg', '--width', '32')
Assert-True ($resize.Data.code -eq 'UNSUPPORTED') 'unsupported HEIC resize was not explicit'
$truncated = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @('--json', 'inspect', (Join-Path $casePath 'truncated.heic'))
Assert-True ($truncated.Data.code -eq 'INPUT_INVALID') 'truncated HEIC was not rejected'
$conflict = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'conflict.sqlite3'),
    'convert', $heic, '--to', 'jpg', '--output', $jpeg
)
Assert-True ($conflict.Data.code -eq 'OUTPUT_CONFLICT') 'existing HEIC output was overwritten'

$resumeDatabase = Join-Path $casePath 'resume.sqlite3'
$resumeOutput = Join-Path $casePath 'resumed HEIC.png'
$cancelled = Invoke-FormatWrightJson -ExpectedExitCodes @(130) -Arguments @(
    '--json', '--state-db', $resumeDatabase, 'convert', $heic,
    '--to', 'png', '--output', $resumeOutput, '--timeout-seconds', '0'
)
Assert-True ($cancelled.Data.code -eq 'CANCELLED') 'HEIC cancellation failed'
Assert-True (-not (Test-Path -LiteralPath $resumeOutput)) 'cancelled HEIC output was committed'
$jobs = Invoke-FormatWrightJson -Arguments @('--json', '--state-db', $resumeDatabase, 'jobs', 'list', '--limit', '10')
$cancelledJob = @($jobs.Data | Where-Object state -eq 'cancelled')[0]
Assert-True ($null -ne $cancelledJob) 'cancelled HEIC job was not durable'
$null = Invoke-FormatWrightJson -Arguments @('--json', '--state-db', $resumeDatabase, 'jobs', 'retry', $cancelledJob.id)
$resumed = Invoke-FormatWrightJson -Arguments @('--json', '--state-db', $resumeDatabase, 'jobs', 'run', '--limit', '1')
Assert-True ($resumed.Data.completed -eq 1) 'queued HEIC Plan did not resume to Pass'
Assert-True ((Test-Path -LiteralPath $resumeOutput -PathType Leaf)) 'resumed HEIC output missing'

Assert-True ($sourceHash -eq (Get-FileHash -LiteralPath $heic -Algorithm SHA256).Hash) 'HEIC source changed'
Assert-True (@(Get-ChildItem -LiteralPath $casePath -Force | Where-Object { $_.Name -like '.formatwright-partial-*' }).Count -eq 0) 'staged HEIC workspace remains'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    fixture_sha256 = $sourceHash.ToLowerInvariant()
    jpeg_quality_82 = 'pass'
    png_lossless = 'pass'
    independent_ffprobe_and_pillow = 'pass'
    wrong_extension_detected = $true
    truncated_input_rejected = $true
    invalid_constraints_rejected = $true
    output_conflict_blocked = $true
    cancellation_and_queue_retry = 'pass'
    source_unchanged = $true
    staged_workspaces_remaining = 0
}
$summary | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $casePath 'heic-sandbox-result.json') -Encoding utf8
$summary | ConvertTo-Json -Depth 8
