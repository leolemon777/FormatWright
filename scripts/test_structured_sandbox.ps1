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
        throw "structured sandbox assertion failed: $Message"
    }
}

function Write-Utf8Fixture {
    param([string]$Path, [string]$Content)
    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
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
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'structured-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath | Out-Null

$jsonInput = Join-Path $casePath 'typed records.json'
Write-Utf8Fixture -Path $jsonInput -Content @'
[
  {"id": 9007199254740993, "active": true, "note": null, "name": "雪"},
  {"id": -2, "active": false, "name": "second"}
]
'@
$jsonHash = (Get-FileHash -LiteralPath $jsonInput -Algorithm SHA256).Hash
$jsonProbe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $jsonInput)
Assert-True ($jsonProbe.Data.format.id -eq 'json') 'JSON input was not detected'
$yamlOutput = Join-Path $casePath 'typed records.yaml'
$yaml = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'json-yaml.sqlite3'),
    'convert', $jsonInput, '--to', 'yaml', '--output', $yamlOutput
)
Assert-True ($yaml.Data.status -eq 'pass') 'JSON to YAML did not validate'
$yamlProbe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $yamlOutput)
Assert-True (
    $jsonProbe.Data.streams[0].properties.semantic_digest -eq
        $yamlProbe.Data.streams[0].properties.semantic_digest
) 'JSON to YAML changed typed record values'

$csvInput = Join-Path $casePath 'quoted source.csv'
Write-Utf8Fixture -Path $csvInput -Content @'
name,empty,note
alpha,,"comma, quote ""and newline
inside"""
雪,,plain
'@
$csvOutput = Join-Path $casePath 'quoted source.json'
$csv = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'csv-json.sqlite3'),
    'convert', $csvInput, '--to', 'json', '--output', $csvOutput
)
Assert-True ($csv.Data.status -eq 'pass') 'CSV to JSON did not validate'
$parsedCsvJson = Get-Content -LiteralPath $csvOutput -Raw | ConvertFrom-Json
Assert-True ($parsedCsvJson.Count -eq 2) 'CSV row count changed'
Assert-True ($parsedCsvJson[0].note -match "newline`ninside") 'quoted CSV newline was not preserved'

$lossyInput = Join-Path $casePath 'lossy values.json'
Write-Utf8Fixture -Path $lossyInput -Content @'
[
  {"id": 7, "enabled": true, "optional": null},
  {"id": 8, "enabled": false}
]
'@
$lossyBlockedOutput = Join-Path $casePath 'must-not-exist.csv'
$lossyBlocked = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', 'plan', $lossyInput, '--to', 'csv', '--output', $lossyBlockedOutput
)
Assert-True ($lossyBlocked.Data.code -eq 'POLICY_BLOCKED') 'lossy mapping was not blocked by default'
Assert-True (-not (Test-Path -LiteralPath $lossyBlockedOutput)) 'blocked Plan created an output'
$lossyOutput = Join-Path $casePath 'authorized lossy.csv'
$lossy = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'lossy.sqlite3'),
    'convert', $lossyInput, '--to', 'csv', '--allow-lossy-data', '--output', $lossyOutput
)
Assert-True ($lossy.Data.status -eq 'warning') 'authorized lossy mapping was not reported as Warning'
Assert-True (
    @($lossy.Data.checks | Where-Object code -eq 'STRUCTURED_SEMANTIC_DIGEST')[0].status -eq 'warning'
) 'lossy semantic-digest check was not a Warning'

$nestedInput = Join-Path $casePath 'nested.json'
Write-Utf8Fixture -Path $nestedInput -Content '[{"id":1,"child":{"value":2}}]'
$nested = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', 'plan', $nestedInput, '--to', 'csv', '--allow-lossy-data'
)
Assert-True ($nested.Data.code -eq 'POLICY_BLOCKED') 'nested data was flattened implicitly'

$duplicateInput = Join-Path $casePath 'duplicate.json'
Write-Utf8Fixture -Path $duplicateInput -Content '[{"id":1,"id":2}]'
$duplicate = Invoke-FormatWrightJson -ExpectedExitCodes @(2) -Arguments @(
    '--json', 'inspect', $duplicateInput
)
Assert-True ($duplicate.Data.code -eq 'INPUT_INVALID') 'duplicate JSON key was accepted'

$xmlInput = Join-Path $casePath 'records.xml'
Write-Utf8Fixture -Path $xmlInput -Content @'
<?xml version="1.0" encoding="UTF-8"?>
<records>
  <record><id>1</id><name>alpha &amp; beta</name></record>
  <record><id>2</id><name>雪</name></record>
</records>
'@
$xmlOutput = Join-Path $casePath 'records.json'
$xml = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'xml-json.sqlite3'),
    'convert', $xmlInput, '--to', 'json', '--output', $xmlOutput
)
Assert-True ($xml.Data.status -eq 'pass') 'XML to JSON did not validate'
$xmlJson = Get-Content -LiteralPath $xmlOutput -Raw | ConvertFrom-Json
Assert-True ($xmlJson[0].name -eq 'alpha & beta') 'XML entity text changed'

$dtdInput = Join-Path $casePath 'dtd.xml'
Write-Utf8Fixture -Path $dtdInput -Content '<!DOCTYPE records [<!ENTITY x "unsafe">]><records><record><id>&x;</id></record></records>'
$dtd = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @('--json', 'inspect', $dtdInput)
Assert-True ($dtd.Data.code -eq 'POLICY_BLOCKED') 'XML DTD was not blocked'

$disguised = Join-Path $casePath 'actually-json.bin'
Copy-Item -LiteralPath $jsonInput -Destination $disguised
$disguisedProbe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $disguised)
Assert-True ($disguisedProbe.Data.format.id -eq 'json') 'header-first detection missed disguised JSON'
Assert-True ($disguisedProbe.Data.format.extension_matches -eq $false) 'extension mismatch was missed'

$bomInput = Join-Path $casePath 'bom.json'
$bomEncoding = [System.Text.UTF8Encoding]::new($true)
$bomPayload = $bomEncoding.GetPreamble() + $bomEncoding.GetBytes('[{"id":1,"name":"BOM"}]')
[System.IO.File]::WriteAllBytes($bomInput, $bomPayload)
$bomProbe = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $bomInput)
Assert-True ($bomProbe.Data.format.id -eq 'json') 'UTF-8 BOM JSON was not parsed'

$attributeInput = Join-Path $casePath 'attributes.xml'
Write-Utf8Fixture -Path $attributeInput -Content '<records version="1"><record><id>1</id></record></records>'
$attributes = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', 'inspect', $attributeInput
)
Assert-True ($attributes.Data.code -eq 'POLICY_BLOCKED') 'unmapped XML attributes were silently dropped'

$conflict = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'conflict.sqlite3'),
    'convert', $jsonInput, '--to', 'yaml', '--output', $yamlOutput
)
Assert-True ($conflict.Data.code -eq 'OUTPUT_CONFLICT') 'existing output was overwritten'
Assert-True (
    $jsonHash -eq (Get-FileHash -LiteralPath $jsonInput -Algorithm SHA256).Hash
) 'conversion modified the JSON input'
Assert-True (
    @(Get-ChildItem -LiteralPath $casePath -Filter '.formatwright-partial-*' -File).Count -eq 0
) 'structured suite left staged output files'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    json_to_yaml = [ordered]@{ status = $yaml.Data.status; semantic_digest_preserved = $true }
    csv_to_json = [ordered]@{ status = $csv.Data.status; rows = $parsedCsvJson.Count }
    xml_to_json = [ordered]@{ status = $xml.Data.status; entity_text_preserved = $true }
    lossy_mapping = [ordered]@{ default = 'blocked'; authorized_status = $lossy.Data.status }
    nested_flattening_blocked = $true
    duplicate_json_key_blocked = $true
    xml_dtd_blocked = $true
    xml_attributes_blocked = $true
    utf8_bom_parsed = $true
    wrong_extension_detected = $true
    output_conflict_blocked = $true
    source_unchanged = $true
    staged_outputs_remaining = 0
}
$summaryPath = Join-Path $casePath 'structured-sandbox-result.json'
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8
$summary | ConvertTo-Json -Depth 8
