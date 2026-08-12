#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts'),
    [string]$Python = '',
    [string]$Soffice = '',
    [string]$PdfInfo = '',
    [string]$PdfToPpm = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "Office sandbox assertion failed: $Message" }
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
$sofficePath = Resolve-ToolPath -Explicit $Soffice -EnvironmentName 'FORMATWRIGHT_ENGINE_SOFFICE' -CommandName 'soffice'
$pdfInfoPath = Resolve-ToolPath -Explicit $PdfInfo -EnvironmentName 'FORMATWRIGHT_ENGINE_PDFINFO' -CommandName 'pdfinfo'
$pdfToPpmPath = Resolve-ToolPath -Explicit $PdfToPpm -EnvironmentName 'FORMATWRIGHT_ENGINE_PDFTOPPM' -CommandName 'pdftoppm'
$env:FORMATWRIGHT_ENGINE_SOFFICE = $sofficePath
$env:FORMATWRIGHT_ENGINE_PDFINFO = $pdfInfoPath
$env:FORMATWRIGHT_ENGINE_PDFTOPPM = $pdfToPpmPath

New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'office-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath | Out-Null

$fixtureGenerator = @'
from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED
import sys
from docx import Document
from pptx import Presentation
from pptx.dml.color import RGBColor
from pptx.enum.shapes import MSO_SHAPE
from pptx.util import Inches
from openpyxl import Workbook
from openpyxl.styles import PatternFill, Font

root = Path(sys.argv[1])

docx = root / "Writer 文档.docx"
document = Document()
document.add_heading("FormatWright Writer", 0)
document.add_paragraph("First page - local text, table, and deterministic pagination.")
table = document.add_table(rows=2, cols=2)
table.cell(0, 0).text = "Key"; table.cell(0, 1).text = "Value"
table.cell(1, 0).text = "alpha"; table.cell(1, 1).text = "42"
document.add_page_break()
document.add_heading("Second Page", 1)
document.add_paragraph("Second page validation marker.")
document.save(docx)

pptx = root / "Slides 演示.pptx"
presentation = Presentation()
for index, color in enumerate(((220, 40, 40), (40, 80, 220)), 1):
    slide = presentation.slides.add_slide(presentation.slide_layouts[5])
    slide.shapes.title.text = f"Slide {index}"
    shape = slide.shapes.add_shape(MSO_SHAPE.RECTANGLE, Inches(1), Inches(2), Inches(5), Inches(2))
    shape.fill.solid(); shape.fill.fore_color.rgb = RGBColor(*color)
    shape.text = f"FormatWright presentation page {index}"
presentation.save(pptx)

xlsx = root / "Sheet 数据.xlsx"
workbook = Workbook()
sheet = workbook.active
sheet.title = "Data"
sheet.append(["Name", "Value"])
for row in range(1, 25):
    sheet.append([f"row-{row}", row * 3])
sheet["A1"].font = Font(bold=True); sheet["B1"].font = Font(bold=True)
sheet["A1"].fill = PatternFill("solid", fgColor="4F81BD")
sheet["B1"].fill = PatternFill("solid", fgColor="4F81BD")
sheet.print_area = "A1:B25"
sheet.sheet_properties.pageSetUpPr.fitToPage = True
sheet.page_setup.fitToWidth = 1; sheet.page_setup.fitToHeight = 1
workbook.save(xlsx)

def copy_with_extra(source, target, name, data):
    with ZipFile(source, "r") as incoming, ZipFile(target, "w", ZIP_DEFLATED) as outgoing:
        for item in incoming.infolist():
            outgoing.writestr(item, incoming.read(item.filename))
        outgoing.writestr(name, data)

external_xml = b'''<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rExternal" Type="urn:test" Target="https://example.invalid/resource" TargetMode="External"/>
</Relationships>'''
copy_with_extra(docx, root / "external.docx", "custom/_rels/external.rels", external_xml)
copy_with_extra(docx, root / "macro.docx", "word/vbaProject.bin", b"not-a-real-macro")
(root / "disguised.bin").write_bytes(docx.read_bytes())
(root / "truncated.docx").write_bytes(docx.read_bytes()[:256])
'@
$fixtureGenerator | & $pythonPath - $casePath
Assert-True ($LASTEXITCODE -eq 0) 'Python could not generate Office fixtures'

$fixtures = @(
    [pscustomobject]@{ Name = 'docx'; Input = (Join-Path $casePath 'Writer 文档.docx'); Pages = 2 },
    [pscustomobject]@{ Name = 'pptx'; Input = (Join-Path $casePath 'Slides 演示.pptx'); Pages = 2 },
    [pscustomobject]@{ Name = 'xlsx'; Input = (Join-Path $casePath 'Sheet 数据.xlsx'); Pages = 1 }
)
$sourceHashes = @{}
$outputs = @()
foreach ($fixture in $fixtures) {
    $sourceHashes[$fixture.Name] = (Get-FileHash -LiteralPath $fixture.Input -Algorithm SHA256).Hash
    $probe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $fixture.Input)
    Assert-True ($probe.Data.format.id -eq $fixture.Name) "$($fixture.Name) content detection failed"
    Assert-True ($probe.Data.evidence.engine_id -eq 'formatwright.office-inspector') 'native Office evidence missing'
    Assert-True (-not $probe.Data.streams[0].properties.has_macros) 'macro-free fixture marked macro-bearing'
    Assert-True (-not $probe.Data.streams[0].properties.has_external_relationships) 'local fixture marked external'

    $output = Join-Path $casePath "$($fixture.Name) output.pdf"
    $plan = Invoke-FormatWrightJson -Arguments @('--json', 'plan', $fixture.Input, '--to', 'pdf', '--output', $output)
    Assert-True ($plan.Data.steps[0].engine.engine_id -eq 'soffice') 'LibreOffice step missing'
    Assert-True ($plan.Data.steps[1].engine.engine_id -eq 'pdfinfo') 'structural-validation step missing'
    Assert-True ($plan.Data.steps[2].engine.engine_id -eq 'pdftoppm') 'render-validation step missing'
    Assert-True ($plan.Data.constraints.isolated_user_profile) 'isolated profile not explicit'
    Assert-True ($plan.Data.constraints.macros -eq 'disabled') 'macro policy not explicit'
    Assert-True ($plan.Data.network_policy -eq 'deny') 'network policy was not deny'
    $result = Invoke-FormatWrightJson -Arguments @(
        '--json', '--state-db', (Join-Path $casePath "$($fixture.Name).sqlite3"),
        'convert', $fixture.Input, '--to', 'pdf', '--output', $output
    )
    Assert-True ($result.Data.status -eq 'warning') 'uncertified visual fidelity was not surfaced as Warning'
    Assert-True (@($result.Data.checks | Where-Object { $_.required -and $_.status -ne 'pass' }).Count -eq 0) 'required Office PDF check failed'
    $pageCheck = @($result.Data.checks | Where-Object code -eq 'OFFICE_PDF_PAGE_COUNT')[0]
    Assert-True ($pageCheck.observed -eq $fixture.Pages) "$($fixture.Name) output page count mismatch"
    Assert-True ((Test-Path -LiteralPath $output -PathType Leaf)) "$($fixture.Name) PDF missing"
    $outputs += [pscustomobject]@{ Name = $fixture.Name; Pdf = $output; Pages = $fixture.Pages }
}

$disguised = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', (Join-Path $casePath 'disguised.bin'))
Assert-True ($disguised.Data.format.id -eq 'docx') 'wrong-extension OOXML detection failed'
Assert-True ($disguised.Data.format.extension_matches -eq $false) 'OOXML extension mismatch not reported'
$external = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @('--json', 'inspect', (Join-Path $casePath 'external.docx'))
Assert-True ($external.Data.code -eq 'POLICY_BLOCKED') 'external Office relationship was not blocked'
$macro = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @('--json', 'inspect', (Join-Path $casePath 'macro.docx'))
Assert-True ($macro.Data.code -eq 'POLICY_BLOCKED') 'macro-bearing Office package was not blocked'
$truncated = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @('--json', 'inspect', (Join-Path $casePath 'truncated.docx'))
Assert-True ($truncated.Data.code -eq 'INPUT_INVALID') 'truncated Office package was not rejected'

$renderRoot = Join-Path $casePath 'independent-renders'
New-Item -ItemType Directory -Path $renderRoot | Out-Null
foreach ($output in $outputs) {
    $directory = Join-Path $renderRoot $output.Name
    New-Item -ItemType Directory -Path $directory | Out-Null
    & $pdfToPpmPath -r 96 -png $output.Pdf (Join-Path $directory 'page') 2>$null
    Assert-True ($LASTEXITCODE -eq 0) "independent pdftoppm failed for $($output.Name)"
    Assert-True (@(Get-ChildItem -LiteralPath $directory -Filter '*.png' -File).Count -eq $output.Pages) "independent render count mismatch for $($output.Name)"
}
$pixelVerifier = @'
from pathlib import Path
from PIL import Image
import sys
root = Path(sys.argv[1])
expected = {"docx": 2, "pptx": 2, "xlsx": 1}
for family, count in expected.items():
    pages = sorted((root / family).glob("*.png"))
    assert len(pages) == count
    for page in pages:
        with Image.open(page) as image:
            image.load()
            assert image.width > 0 and image.height > 0
'@
$pixelVerifier | & $pythonPath - $renderRoot
Assert-True ($LASTEXITCODE -eq 0) 'independent Pillow render validation failed'

$existingOutput = $outputs[0].Pdf
$conflict = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'conflict.sqlite3'),
    'convert', $fixtures[0].Input, '--to', 'pdf', '--output', $existingOutput
)
Assert-True ($conflict.Data.code -eq 'OUTPUT_CONFLICT') 'existing Office PDF was overwritten'

$resumeDatabase = Join-Path $casePath 'resume.sqlite3'
$resumeOutput = Join-Path $casePath 'resumed Office.pdf'
$cancelled = Invoke-FormatWrightJson -ExpectedExitCodes @(130) -Arguments @(
    '--json', '--state-db', $resumeDatabase, 'convert', $fixtures[1].Input,
    '--to', 'pdf', '--output', $resumeOutput, '--timeout-seconds', '0'
)
Assert-True ($cancelled.Data.code -eq 'CANCELLED') 'Office process-tree cancellation failed'
Assert-True (-not (Test-Path -LiteralPath $resumeOutput)) 'cancelled Office PDF was committed'
$jobs = Invoke-FormatWrightJson -Arguments @('--json', '--state-db', $resumeDatabase, 'jobs', 'list', '--limit', '10')
$cancelledJob = @($jobs.Data | Where-Object state -eq 'cancelled')[0]
Assert-True ($null -ne $cancelledJob) 'cancelled Office job was not durable'
$null = Invoke-FormatWrightJson -Arguments @('--json', '--state-db', $resumeDatabase, 'jobs', 'retry', $cancelledJob.id)
$resumed = Invoke-FormatWrightJson -Arguments @('--json', '--state-db', $resumeDatabase, 'jobs', 'run', '--limit', '1')
Assert-True ($resumed.Data.warning -eq 1) 'queued Office Plan did not resume to validated Warning'
Assert-True ((Test-Path -LiteralPath $resumeOutput -PathType Leaf)) 'resumed Office PDF missing'

foreach ($fixture in $fixtures) {
    Assert-True ($sourceHashes[$fixture.Name] -eq (Get-FileHash -LiteralPath $fixture.Input -Algorithm SHA256).Hash) "$($fixture.Name) source changed"
}
Assert-True (@(Get-ChildItem -LiteralPath $casePath -Force -Directory | Where-Object { $_.Name -like '.formatwright-partial-*' -or $_.Name -like '.fw-*' }).Count -eq 0) 'staged Office workspace remains'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    docx_to_pdf = 'warning-required-checks-pass'
    pptx_to_pdf = 'warning-required-checks-pass'
    xlsx_to_pdf = 'warning-required-checks-pass'
    page_counts = @{ docx = 2; pptx = 2; xlsx = 1 }
    independent_all_page_render = 'pass'
    external_relationship_blocked = $true
    macro_package_blocked = $true
    truncated_package_rejected = $true
    output_conflict_blocked = $true
    cancellation_and_queue_retry = 'pass'
    source_unchanged = $true
    staged_workspaces_remaining = 0
}
$summary | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $casePath 'office-sandbox-result.json') -Encoding utf8
$summary | ConvertTo-Json -Depth 8
