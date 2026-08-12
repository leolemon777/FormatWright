#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts'),
    [string]$Python = '',
    [string]$PdfInfo = '',
    [string]$PdfToPpm = '',
    [string]$Ffprobe = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "PDF sandbox assertion failed: $Message" }
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
$pdfInfoPath = Resolve-ToolPath -Explicit $PdfInfo -EnvironmentName 'FORMATWRIGHT_ENGINE_PDFINFO' -CommandName 'pdfinfo'
$pdfToPpmPath = Resolve-ToolPath -Explicit $PdfToPpm -EnvironmentName 'FORMATWRIGHT_ENGINE_PDFTOPPM' -CommandName 'pdftoppm'
$ffprobePath = Resolve-ToolPath -Explicit $Ffprobe -EnvironmentName 'FORMATWRIGHT_ENGINE_FFPROBE' -CommandName 'ffprobe'
$env:FORMATWRIGHT_ENGINE_PDFINFO = $pdfInfoPath
$env:FORMATWRIGHT_ENGINE_PDFTOPPM = $pdfToPpmPath
$env:FORMATWRIGHT_ENGINE_FFPROBE = $ffprobePath

New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'pdf-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath | Out-Null

$fixtureGenerator = @'
from pathlib import Path
import sys
from reportlab.lib.pagesizes import A4, letter, landscape
from reportlab.pdfgen.canvas import Canvas
from reportlab.lib.colors import red, green, blue
from pypdf import PdfReader, PdfWriter

root = Path(sys.argv[1])
source = root / "three-pages.pdf"
c = Canvas(str(source), pagesize=letter)
c.setFillColor(red); c.rect(72, 600, 180, 90, fill=1, stroke=0)
c.setFillColorRGB(0, 0, 0); c.drawString(72, 720, "FormatWright PDF page 1")
c.showPage()
c.setPageSize(A4)
c.setFillColor(green); c.circle(180, 620, 70, fill=1, stroke=0)
c.setFillColorRGB(0, 0, 0); c.drawString(72, 780, "FormatWright PDF page 2")
c.showPage()
c.setPageSize(landscape(letter))
c.setFillColor(blue); c.rect(72, 380, 240, 100, fill=1, stroke=0)
c.setFillColorRGB(0, 0, 0); c.drawString(72, 540, "FormatWright PDF page 3")
c.save()

reader = PdfReader(str(source))
writer = PdfWriter()
writer.append_pages_from_reader(reader)
writer.encrypt("formatwright-secret")
with (root / "encrypted.pdf").open("wb") as stream:
    writer.write(stream)

(root / "disguised.bin").write_bytes(source.read_bytes())
(root / "truncated.pdf").write_bytes(source.read_bytes()[:128])
'@
$fixtureGenerator | & $pythonPath - $casePath
Assert-True ($LASTEXITCODE -eq 0) 'Python could not generate PDF fixtures'

$source = Join-Path $casePath 'three-pages.pdf'
$sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
$probe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $source)
Assert-True ($probe.Data.format.id -eq 'pdf') 'PDF was not detected'
Assert-True (@($probe.Data.streams).Count -eq 3) 'pdfinfo page count was not preserved'
Assert-True (@($probe.Data.streams | Where-Object kind -eq 'page').Count -eq 3) 'page probes missing'
Assert-True ($probe.Data.evidence.engine_id -eq 'pdfinfo') 'pdfinfo evidence missing'

$disguised = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', (Join-Path $casePath 'disguised.bin'))
Assert-True ($disguised.Data.format.id -eq 'pdf') 'header-first PDF detection failed'
Assert-True ($disguised.Data.format.extension_matches -eq $false) 'extension mismatch was not reported'

$pngDirectory = Join-Path $casePath 'PNG 页面'
$pngPlan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'plan', $source, '--to', 'png', '--dpi', '144', '--color-mode', 'rgb',
    '--output', $pngDirectory
)
Assert-True ($pngPlan.Data.steps[0].engine.engine_id -eq 'pdftoppm') 'pdftoppm was not selected'
Assert-True ($pngPlan.Data.steps[0].operation -eq 'render') 'PDF operation was not render'
Assert-True ($pngPlan.Data.constraints.output_kind -eq 'page-directory') 'output was not a page directory'
Assert-True ($pngPlan.Data.constraints.page_count -eq 3) 'Plan page count mismatch'
Assert-True ($pngPlan.Data.constraints.dpi -eq 144) 'Plan DPI mismatch'
Assert-True ($pngPlan.Data.network_policy -eq 'deny') 'network policy was not deny'
$pngResult = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'png.sqlite3'),
    'convert', $source, '--to', 'png', '--dpi', '144', '--color-mode', 'rgb',
    '--output', $pngDirectory
)
Assert-True ($pngResult.Data.status -eq 'pass') 'PDF to PNG did not validate'
Assert-True (@($pngResult.Data.checks | Where-Object status -ne 'pass').Count -eq 0) 'PNG required check failed'
Assert-True (@(Get-ChildItem -LiteralPath $pngDirectory -File).Count -eq 3) 'PNG page count mismatch'
$pngNames = (Get-ChildItem -LiteralPath $pngDirectory -File | Select-Object -ExpandProperty Name) -join ','
Assert-True ($pngNames -eq 'page-000001.png,page-000002.png,page-000003.png') 'PNG names are not deterministic'

$jpegDirectory = Join-Path $casePath 'jpeg-gray-pages'
$jpegResult = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'jpeg.sqlite3'),
    'convert', $source, '--to', 'jpg', '--dpi', '96', '--color-mode', 'gray',
    '--quality', '77', '--output', $jpegDirectory
)
Assert-True ($jpegResult.Data.status -eq 'pass') 'PDF to grayscale JPEG did not validate'
Assert-True (@($jpegResult.Data.checks | Where-Object status -ne 'pass').Count -eq 0) 'JPEG required check failed'
Assert-True (@(Get-ChildItem -LiteralPath $jpegDirectory -File).Count -eq 3) 'JPEG page count mismatch'

$independentCheck = @'
from pathlib import Path
from PIL import Image
import sys

png_root, jpg_root = map(Path, sys.argv[1:3])
expected_png = [(1224, 1584), (1191, 1684), (1584, 1224)]
expected_jpg = [(816, 1056), (794, 1123), (1056, 816)]
for index, expected in enumerate(expected_png, 1):
    with Image.open(png_root / f"page-{index:06}.png") as image:
        image.load()
        assert image.size == expected, (index, image.size, expected)
        if "A" in image.getbands():
            assert image.getchannel("A").getextrema() == (255, 255)
for index, expected in enumerate(expected_jpg, 1):
    with Image.open(jpg_root / f"page-{index:06}.jpg") as image:
        image.load()
        assert image.size == expected, (index, image.size, expected)
        rgb = image.convert("RGB")
        width, height = rgb.size
        step = max(1, (width * height) // 20000)
        for offset, (red, green, blue) in enumerate(rgb.get_flattened_data()):
            if offset % step == 0:
                assert max(red, green, blue) - min(red, green, blue) <= 3
'@
$independentCheck | & $pythonPath - $pngDirectory $jpegDirectory
Assert-True ($LASTEXITCODE -eq 0) 'independent Pillow dimension/color/alpha check failed'

$encrypted = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', 'inspect', (Join-Path $casePath 'encrypted.pdf')
)
Assert-True ($encrypted.Data.code -eq 'POLICY_BLOCKED') 'encrypted PDF was not blocked'
$truncated = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @(
    '--json', 'inspect', (Join-Path $casePath 'truncated.pdf')
)
Assert-True ($truncated.Data.code -eq 'INPUT_INVALID') 'truncated PDF was not rejected'
$badDpi = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @(
    '--json', 'plan', $source, '--to', 'png', '--dpi', '601'
)
Assert-True ($badDpi.Data.code -eq 'INPUT_INVALID') 'invalid DPI was not rejected'
$pngQuality = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @(
    '--json', 'plan', $source, '--to', 'png', '--quality', '80'
)
Assert-True ($pngQuality.Data.code -eq 'INPUT_INVALID') 'PNG quality was not rejected'
$conflict = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'conflict.sqlite3'),
    'convert', $source, '--to', 'png', '--output', $pngDirectory
)
Assert-True ($conflict.Data.code -eq 'OUTPUT_CONFLICT') 'existing page directory was overwritten'

$resumeDatabase = Join-Path $casePath 'resume.sqlite3'
$resumeDirectory = Join-Path $casePath 'resumed-pages'
$cancelled = Invoke-FormatWrightJson -ExpectedExitCodes @(130) -Arguments @(
    '--json', '--state-db', $resumeDatabase,
    'convert', $source, '--to', 'png', '--dpi', '600', '--output', $resumeDirectory,
    '--timeout-seconds', '0'
)
Assert-True ($cancelled.Data.code -eq 'CANCELLED') 'PDF render did not cancel through the process-tree path'
Assert-True (-not (Test-Path -LiteralPath $resumeDirectory)) 'cancelled PDF render committed an output'
$cancelledJobs = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', $resumeDatabase, 'jobs', 'list', '--limit', '10'
)
$cancelledJob = @($cancelledJobs.Data | Where-Object state -eq 'cancelled')[0]
Assert-True ($null -ne $cancelledJob) 'cancelled PDF job was not durable'
$null = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', $resumeDatabase, 'jobs', 'retry', $cancelledJob.id
)
$resumed = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', $resumeDatabase, 'jobs', 'run', '--limit', '1'
)
Assert-True ($resumed.Data.completed -eq 1) 'queued PDF Plan did not resume to completion'
Assert-True ((Test-Path -LiteralPath $resumeDirectory -PathType Container)) 'resumed page directory missing'
Assert-True (@(Get-ChildItem -LiteralPath $resumeDirectory -File).Count -eq 3) 'resumed page set is incomplete'
Assert-True ($sourceHash -eq (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash) 'source changed'
Assert-True (@(Get-ChildItem -LiteralPath $casePath -Filter '.formatwright-partial-*' -Directory).Count -eq 0) 'staged page directory remains'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    page_count = 3
    png_rgb_144_dpi = $pngResult.Data.status
    jpeg_gray_96_dpi = $jpegResult.Data.status
    deterministic_page_names = $true
    independent_pixel_check = 'pass'
    encrypted_pdf_blocked = $true
    truncated_pdf_rejected = $true
    invalid_dpi_rejected = $true
    output_conflict_blocked = $true
    cancellation_and_queue_retry = 'pass'
    source_unchanged = $true
    staged_directories_remaining = 0
}
$summary | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $casePath 'pdf-sandbox-result.json') -Encoding utf8
$summary | ConvertTo-Json -Depth 8
