#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts'),
    [string]$Python = '',
    [string]$Pandoc = '',
    [string]$Soffice = '',
    [string]$PdfInfo = '',
    [string]$PdfToPpm = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "document sandbox assertion failed: $Message" }
}

function Write-Utf8 {
    param([string]$Path, [string]$Content)
    [IO.File]::WriteAllText($Path, $Content, [Text.UTF8Encoding]::new($false))
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

$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
$pythonPath = Resolve-ToolPath -Explicit $Python -EnvironmentName 'FORMATWRIGHT_TEST_PYTHON' -CommandName 'python'
$pandocPath = Resolve-ToolPath -Explicit $Pandoc -EnvironmentName 'FORMATWRIGHT_ENGINE_PANDOC' -CommandName 'pandoc'
$sofficePath = Resolve-ToolPath -Explicit $Soffice -EnvironmentName 'FORMATWRIGHT_ENGINE_SOFFICE' -CommandName 'soffice'
$pdfInfoPath = Resolve-ToolPath -Explicit $PdfInfo -EnvironmentName 'FORMATWRIGHT_ENGINE_PDFINFO' -CommandName 'pdfinfo'
$pdfToPpmPath = Resolve-ToolPath -Explicit $PdfToPpm -EnvironmentName 'FORMATWRIGHT_ENGINE_PDFTOPPM' -CommandName 'pdftoppm'
$env:FORMATWRIGHT_ENGINE_PANDOC = $pandocPath
$env:FORMATWRIGHT_ENGINE_SOFFICE = $sofficePath
$env:FORMATWRIGHT_ENGINE_PDFINFO = $pdfInfoPath
$env:FORMATWRIGHT_ENGINE_PDFTOPPM = $pdfToPpmPath
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'document-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath | Out-Null

$markdown = Join-Path $casePath '说明 document.md'
Write-Utf8 -Path $markdown -Content @'
# FormatWright Document

本地转换可以验证结果。

- Alpha item
- Beta item
'@
$sourceHash = (Get-FileHash -LiteralPath $markdown -Algorithm SHA256).Hash
$docx = Join-Path $casePath '说明 output.docx'
$plan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'plan', $markdown, '--to', 'docx', '--output', $docx
)
Assert-True ($plan.Data.steps[0].engine.engine_id -eq 'pandoc') 'Pandoc was not selected'
Assert-True ($plan.Data.steps[0].arguments.sandbox -eq 'true') 'Pandoc sandbox was not explicit'
Assert-True ($plan.Data.network_policy -eq 'deny') 'network policy was not deny'
$converted = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'markdown.sqlite3'),
    'convert', $markdown, '--to', 'docx', '--output', $docx
)
Assert-True ($converted.Data.status -eq 'pass') 'Markdown to DOCX did not validate'
$docxProbe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $docx)
Assert-True ($docxProbe.Data.format.id -eq 'docx') 'DOCX package was not detected'
$docxProperties = $docxProbe.Data.streams[0].properties
$requiredParts = if ($null -ne $docxProperties.PSObject.Properties['required_parts_present']) {
    $docxProperties.required_parts_present
} else {
    $docxProperties.required_part_present
}
Assert-True ($requiredParts) 'required DOCX parts missing'

Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::OpenRead($docx)
try {
    $names = @($archive.Entries | ForEach-Object FullName)
    foreach ($required in @('[Content_Types].xml', '_rels/.rels', 'word/document.xml')) {
        Assert-True ($names -contains $required) "independent ZIP check missed $required"
    }
} finally {
    $archive.Dispose()
}

$html = Join-Path $casePath 'simple.html'
Write-Utf8 -Path $html -Content '<!doctype html><html><body><h1>HTML Heading</h1><p>Simple local text 42.</p></body></html>'
$htmlDocx = Join-Path $casePath 'simple.docx'
$htmlResult = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'html.sqlite3'),
    'convert', $html, '--to', 'docx', '--output', $htmlDocx
)
Assert-True ($htmlResult.Data.status -eq 'pass') 'HTML to DOCX did not validate'

$markdownPdf = Join-Path $casePath '说明 output.pdf'
$pdfPlan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'plan', $markdown, '--to', 'pdf', '--output', $markdownPdf
)
Assert-True (@($pdfPlan.Data.steps).Count -eq 4) 'markup PDF Plan did not pin four engine steps'
Assert-True ($pdfPlan.Data.steps[0].engine.engine_id -eq 'pandoc') 'markup PDF Pandoc step missing'
Assert-True ($pdfPlan.Data.steps[1].engine.engine_id -eq 'soffice') 'markup PDF LibreOffice step missing'
Assert-True ($pdfPlan.Data.steps[2].engine.engine_id -eq 'pdfinfo') 'markup PDF pdfinfo step missing'
Assert-True ($pdfPlan.Data.steps[3].engine.engine_id -eq 'pdftoppm') 'markup PDF render validation step missing'
$markdownPdfResult = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'markdown-pdf.sqlite3'),
    'convert', $markdown, '--to', 'pdf', '--output', $markdownPdf
)
Assert-True ($markdownPdfResult.Data.status -eq 'warning') 'uncalibrated markup PDF fidelity was not Warning'
Assert-True (@($markdownPdfResult.Data.checks | Where-Object { $_.required -and $_.status -ne 'pass' }).Count -eq 0) 'required markup PDF validation failed'
Assert-True (@($markdownPdfResult.Data.checks | Where-Object code -eq 'DOCX_SEMANTIC_TOKEN_DIGEST').Count -eq 1) 'intermediate semantic digest evidence missing'

$htmlPdf = Join-Path $casePath 'simple.pdf'
$htmlPdfResult = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'html-pdf.sqlite3'),
    'convert', $html, '--to', 'pdf', '--output', $htmlPdf
)
Assert-True ($htmlPdfResult.Data.status -eq 'warning') 'HTML to PDF did not validate with expected Warning'

$renderRoot = Join-Path $casePath 'independent-pdf-renders'
New-Item -ItemType Directory -Path $renderRoot | Out-Null
foreach ($pdfCase in @(
    [pscustomobject]@{ Name = 'markdown'; Pdf = $markdownPdf },
    [pscustomobject]@{ Name = 'html'; Pdf = $htmlPdf }
)) {
    $directory = Join-Path $renderRoot $pdfCase.Name
    New-Item -ItemType Directory -Path $directory | Out-Null
    & $pdfToPpmPath -r 96 -png $pdfCase.Pdf (Join-Path $directory 'page') 2>$null
    Assert-True ($LASTEXITCODE -eq 0) "independent PDF render failed for $($pdfCase.Name)"
    Assert-True (@(Get-ChildItem -LiteralPath $directory -Filter '*.png' -File).Count -ge 1) "no PDF pages rendered for $($pdfCase.Name)"
}
$pixelVerifier = @'
from pathlib import Path
from PIL import Image
import sys
for path in Path(sys.argv[1]).glob("*/*.png"):
    with Image.open(path) as image:
        image.load()
        assert image.width > 0 and image.height > 0
'@
$pixelVerifier | & $pythonPath - $renderRoot
Assert-True ($LASTEXITCODE -eq 0) 'independent markup PDF pixel validation failed'

$remote = Join-Path $casePath 'remote.md'
Write-Utf8 -Path $remote -Content '# Remote`n`n![pixel](https://example.invalid/pixel.png)'
$remotePlan = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', 'plan', $remote, '--to', 'docx'
)
Assert-True ($remotePlan.Data.code -eq 'POLICY_BLOCKED') 'external resource was not blocked before execution'
$conflict = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'conflict.sqlite3'),
    'convert', $markdown, '--to', 'docx', '--output', $docx
)
Assert-True ($conflict.Data.code -eq 'OUTPUT_CONFLICT') 'existing DOCX was overwritten'
$pdfConflict = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'pdf-conflict.sqlite3'),
    'convert', $markdown, '--to', 'pdf', '--output', $markdownPdf
)
Assert-True ($pdfConflict.Data.code -eq 'OUTPUT_CONFLICT') 'existing markup PDF was overwritten'

$resumeDatabase = Join-Path $casePath 'pdf-resume.sqlite3'
$resumeOutput = Join-Path $casePath 'resumed markup.pdf'
$cancelled = Invoke-FormatWrightJson -ExpectedExitCodes @(130) -Arguments @(
    '--json', '--state-db', $resumeDatabase, 'convert', $markdown,
    '--to', 'pdf', '--output', $resumeOutput, '--timeout-seconds', '0'
)
Assert-True ($cancelled.Data.code -eq 'CANCELLED') 'markup PDF cancellation failed'
Assert-True (-not (Test-Path -LiteralPath $resumeOutput)) 'cancelled markup PDF was committed'
$jobs = Invoke-FormatWrightJson -Arguments @('--json', '--state-db', $resumeDatabase, 'jobs', 'list', '--limit', '10')
$cancelledJob = @($jobs.Data | Where-Object state -eq 'cancelled')[0]
Assert-True ($null -ne $cancelledJob) 'cancelled markup PDF job was not durable'
$null = Invoke-FormatWrightJson -Arguments @('--json', '--state-db', $resumeDatabase, 'jobs', 'retry', $cancelledJob.id)
$resumed = Invoke-FormatWrightJson -Arguments @('--json', '--state-db', $resumeDatabase, 'jobs', 'run', '--limit', '1')
Assert-True ($resumed.Data.warning -eq 1) 'queued markup PDF Plan did not resume to validated Warning'
Assert-True ((Test-Path -LiteralPath $resumeOutput -PathType Leaf)) 'resumed markup PDF missing'
Assert-True ($sourceHash -eq (Get-FileHash -LiteralPath $markdown -Algorithm SHA256).Hash) 'source changed'
Assert-True (@(Get-ChildItem $casePath -Filter '.formatwright-partial-*' -File).Count -eq 0) 'staged DOCX remains'
Assert-True (@(Get-ChildItem -LiteralPath $casePath -Force -Directory | Where-Object { $_.Name -like '.fw-*' }).Count -eq 0) 'staged markup PDF workspace remains'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    markdown_to_docx = $converted.Data.status
    html_to_docx = $htmlResult.Data.status
    markdown_to_pdf = 'warning-required-checks-pass'
    html_to_pdf = 'warning-required-checks-pass'
    required_opc_parts = $true
    semantic_token_digest = 'pass'
    independent_pdf_render = 'pass'
    external_resource_blocked = $true
    output_conflict_blocked = $true
    cancellation_and_queue_retry = 'pass'
    source_unchanged = $true
    staged_outputs_remaining = 0
}
$summary | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $casePath 'document-sandbox-result.json') -Encoding utf8
$summary | ConvertTo-Json -Depth 8
