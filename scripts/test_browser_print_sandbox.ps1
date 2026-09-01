#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts'),
    [string]$PdfInfo = '',
    [string]$PdfToPpm = '',
    [string]$PdfToText = '',
    [string]$PdfFonts = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "Browser-print sandbox assertion failed: $Message" }
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
$pdfInfoPath = Resolve-ToolPath -Explicit $PdfInfo -EnvironmentName 'FORMATWRIGHT_ENGINE_PDFINFO' -CommandName 'pdfinfo'
$pdfToPpmPath = Resolve-ToolPath -Explicit $PdfToPpm -EnvironmentName 'FORMATWRIGHT_ENGINE_PDFTOPPM' -CommandName 'pdftoppm'
$pdfToTextPath = Resolve-ToolPath -Explicit $PdfToText -EnvironmentName 'FORMATWRIGHT_ENGINE_PDFTOTEXT' -CommandName 'pdftotext'
$pdfFontsPath = Resolve-ToolPath -Explicit $PdfFonts -EnvironmentName 'FORMATWRIGHT_ENGINE_PDFFONTS' -CommandName 'pdffonts'
$env:FORMATWRIGHT_ENGINE_PDFINFO = $pdfInfoPath
$env:FORMATWRIGHT_ENGINE_PDFTOPPM = $pdfToPpmPath
$env:FORMATWRIGHT_ENGINE_PDFTOTEXT = $pdfToTextPath
$env:FORMATWRIGHT_ENGINE_PDFFONTS = $pdfFontsPath
# msedge is intentionally NOT pinned: ADR-0012 discovery (canonical install
# locations, or FORMATWRIGHT_ENGINE_MSEDGE when the caller sets it) must work.

New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'browser-print-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath | Out-Null

# Fixture: a carton-drawing-shaped HTML page with the exact stressors the
# inspector must tolerate - a void <meta> element, CJK text, barcode digits,
# a watermark, and an inline SVG vector panel.
$cartonHtml = @'
<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<title>Carton 440010147700</title>
<style>
  @page { size: 420mm 293mm; margin: 8mm; }
  body { font-family: "Microsoft YaHei", Arial, sans-serif; }
  .watermark { position: absolute; top: 40%; left: 30%; font-size: 48pt;
               color: rgba(0,0,0,0.18); transform: rotate(-18deg); }
  .field { margin: 6pt 0; font-size: 14pt; }
  .barcode { font-family: "Courier New", monospace; font-size: 22pt;
             letter-spacing: 3pt; }
</style>
</head>
<body>
  <div class="watermark">FORMATWRIGHT SANDBOX</div>
  <h1>电子元件外箱标签 ELECTRIC CARTON</h1>
  <p class="field">公司: 示例电子有限公司 SPECIMEN ELECTRONICS CO., LTD.</p>
  <p class="field">品名: 电容器 CAPACITOR 440V 10uF</p>
  <p class="field">数量 QTY: 24 PCS &nbsp; 毛重 G.W.: 6.8 kg</p>
  <p class="barcode">|4|4|0|0|1|0|1|4|7|7|0|0|</p>
  <svg width="300" height="140" xmlns="http://www.w3.org/2000/svg">
    <rect x="4" y="4" width="292" height="132" fill="none" stroke="black" stroke-width="2"/>
    <line x1="4" y1="70" x2="296" y2="70" stroke="black" stroke-width="1"/>
    <path d="M 30 110 L 80 30 L 130 110 Z" fill="none" stroke="black" stroke-width="2"/>
    <text x="150" y="50" font-size="16">VECTOR PANEL 440</text>
  </svg>
</body>
</html>
'@
$cartonHtmlPath = Join-Path $casePath 'carton.html'
Set-Content -LiteralPath $cartonHtmlPath -Value $cartonHtml -Encoding utf8

# Fixture: a pure-vector SVG (no raster <image>) that must route to the
# browser lane, plus a raster-referencing SVG that must be rejected.
$vectorSvg = @'
<svg xmlns="http://www.w3.org/2000/svg" width="420" height="293">
  <rect x="8" y="8" width="404" height="277" fill="none" stroke="black" stroke-width="3"/>
  <circle cx="120" cy="146" r="60" fill="none" stroke="black" stroke-width="2"/>
  <text x="24" y="140" font-size="26">SVG LANE 440</text>
  <text x="24" y="180" font-size="18">矢量图形示例 · 中文文本</text>
</svg>
'@
$vectorSvgPath = Join-Path $casePath 'label.svg'
Set-Content -LiteralPath $vectorSvgPath -Value $vectorSvg -Encoding utf8

$rasterSvg = @'
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <image href="photo.png" width="100" height="100"/>
</svg>
'@
$rasterSvgPath = Join-Path $casePath 'raster.svg'
Set-Content -LiteralPath $rasterSvgPath -Value $rasterSvg -Encoding utf8

# --- Inspection ------------------------------------------------------------
$probe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $cartonHtmlPath)
Assert-True ($probe.Data.format.id -eq 'html') 'HTML was not detected'
Assert-True ($probe.Data.streams[0].properties.text_characters -gt 0) 'HTML text was not extracted'

$svgProbe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $vectorSvgPath)
Assert-True ($svgProbe.Data.format.id -eq 'svg') 'SVG was not detected'
Assert-True ($svgProbe.Data.format.mime_type -eq 'image/svg+xml') 'SVG MIME type missing'

$rasterProbe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $rasterSvgPath)
Assert-True ($rasterProbe.Data.format.id -eq 'svg') 'raster SVG must still inspect as SVG'
Assert-True (
    $rasterProbe.Data.streams[0].properties.has_external_resource -eq $true
) 'the raster <image> reference was not flagged under deny-all'
$rasterPlan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'plan', $rasterSvgPath, '--to', 'pdf'
) -ExpectedExitCodes @(1, 3, 8)
Assert-True ($rasterPlan.ExitCode -ne 0) 'planning a raster <image> SVG must be policy-blocked'

# --- HTML -> PDF through the browser lane -----------------------------------
$cartonPlan = Invoke-FormatWrightJson -Arguments @(
    '--json', 'plan', $cartonHtmlPath, '--to', 'pdf'
)
Assert-True ($cartonPlan.Data.steps[0].engine.engine_id -eq 'msedge') 'msedge was not selected'
Assert-True ($cartonPlan.Data.network_policy -eq 'deny') 'network policy was not deny'
Assert-True (@($cartonPlan.Data.validators).Count -ge 5) 'EDGE_PDF validators were not declared'

$cartonPdf = Join-Path $casePath 'carton.converted.pdf'
$cartonResult = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'carton.sqlite3'),
    'convert', $cartonHtmlPath, '--to', 'pdf', '--output', $cartonPdf
)
Assert-True ($cartonResult.Data.status -in @('pass', 'warning')) 'HTML to PDF did not validate'
$requiredFails = @(
    $cartonResult.Data.checks | Where-Object { $_.required -and $_.status -ne 'pass' }
)
Assert-True ($requiredFails.Count -eq 0) ("a required EDGE_PDF check failed: " + ($requiredFails | ForEach-Object code) -join ',')
$checkCodes = @($cartonResult.Data.checks | ForEach-Object code)
foreach ($expected in @(
        'EDGE_PDF_OPENS', 'EDGE_PDF_PAGE_COUNT', 'EDGE_PDF_ALL_PAGES_RENDER',
        'EDGE_PDF_TEXT_EXTRACTABLE', 'EDGE_PDF_FONTS_EMBEDDED')) {
    Assert-True ($checkCodes -contains $expected) "missing check $expected"
}
Assert-True (Test-Path -LiteralPath $cartonPdf) 'PDF output file was not committed'

# Independent text-layer verification with the pinned pdftotext.
$extractedText = & $pdfToTextPath $cartonPdf - 2>$null
Assert-True ($LASTEXITCODE -eq 0) 'pdftotext could not read the printed PDF'
$extractedText = ($extractedText -join ' ')
$compactText = $extractedText -replace '[\s|]', ''
Assert-True ($compactText -match '440010147700') 'barcode digits missing from the text layer'
Assert-True ($extractedText -match '电子元件外箱标签') 'CJK heading missing from the text layer'
Assert-True ($extractedText -match 'VECTOR PANEL 440') 'inline SVG panel text missing from the text layer'

# --- SVG -> PDF (browser-only lane) -----------------------------------------
$labelPdf = Join-Path $casePath 'label.converted.pdf'
$labelResult = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'label.sqlite3'),
    'convert', $vectorSvgPath, '--to', 'pdf', '--output', $labelPdf
)
Assert-True ($labelResult.Data.status -in @('pass', 'warning')) 'SVG to PDF did not validate'
$labelRequiredFails = @(
    $labelResult.Data.checks | Where-Object { $_.required -and $_.status -ne 'pass' }
)
Assert-True ($labelRequiredFails.Count -eq 0) 'a required EDGE_PDF check failed for SVG'
Assert-True (Test-Path -LiteralPath $labelPdf) 'SVG PDF output file was not committed'
$labelText = ((& $pdfToTextPath $labelPdf - 2>$null) -join ' ')
$compactLabelText = $labelText -replace '\s', ''
Assert-True ($compactLabelText -match 'SVGLANE440') 'SVG vector text missing from the text layer'
Assert-True ($labelText -match '矢量图形示例') 'SVG CJK text missing from the text layer'

Write-Output ("BROWSER PRINT SANDBOX PASS " + (Split-Path -Leaf $casePath))
Write-Output ("  html: {0} bytes; svg: {1} bytes" -f (Get-Item -LiteralPath $cartonPdf).Length, (Get-Item -LiteralPath $labelPdf).Length)
