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
    if (-not $Condition) { throw "batch sandbox assertion failed: $Message" }
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

function New-TestImage {
    param([string]$Path, [string]$Color)
    $parent = Split-Path -Parent $Path
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    & ffmpeg -y -v error -f lavfi -i "color=c=${Color}:s=96x64:rate=1" -frames:v 1 $Path 2>$null
    Assert-True ($LASTEXITCODE -eq 0) "could not create fixture $Path"
}

foreach ($tool in @('ffmpeg', 'ffprobe')) {
    Assert-True ($null -ne (Get-Command $tool -ErrorAction SilentlyContinue)) "$tool is required"
}
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'batch-suite-' + [Guid]::NewGuid().ToString('N')
)
$inputRoot = Join-Path $casePath '输入 images'
$outputRoot = Join-Path $casePath '输出 webp'
New-Item -ItemType Directory -Path $inputRoot -Force | Out-Null

New-TestImage -Path (Join-Path $inputRoot 'root.png') -Color 'red'
New-TestImage -Path (Join-Path $inputRoot 'level-1\duplicate.png') -Color 'green'
New-TestImage -Path (Join-Path $inputRoot 'level-1\duplicate.jpg') -Color 'blue'
New-TestImage -Path (Join-Path $inputRoot 'level-1\二级\three\雪.png') -Color 'yellow'
New-TestImage -Path (Join-Path $inputRoot 'level-1\二级\three\final.jpg') -Color 'purple'
[System.IO.File]::WriteAllText((Join-Path $inputRoot 'skip.txt'), 'not an image')
$cycle = Join-Path $inputRoot 'level-1\二级\cycle'
New-Item -ItemType Junction -Path $cycle -Target $inputRoot | Out-Null

$database = Join-Path $casePath 'batch.sqlite3'
$paused = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', $database,
    'batch-images', $inputRoot, '--output-dir', $outputRoot,
    '--to', 'webp', '--width', '48', '--quality', '82', '--pause-after', '2'
)
Assert-True ($paused.Data.discovered -eq 7) 'discovered count did not include five files, text, and junction'
Assert-True ($paused.Data.planned -eq 5) 'five image jobs were not planned'
Assert-True ($paused.Data.skipped -eq 2) 'text and directory junction were not skipped'
Assert-True ($paused.Data.completed -eq 2) 'pause-after did not finish exactly two jobs'
Assert-True ($paused.Data.queued -eq 3 -and $paused.Data.paused) 'pause did not leave three durable queued jobs'
Assert-True (@(Get-ChildItem -LiteralPath $outputRoot -Recurse -Filter '*.webp' -File).Count -eq 2) 'pause scheduled too many outputs'

$resumed = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', $database, 'jobs', 'run', '--limit', '100', '--parallel', '4'
)
Assert-True ($resumed.Data.selected -eq 3) 'resume did not select the three queued jobs'
Assert-True ($resumed.Data.completed -eq 3) 'resume did not complete every queued job'
Assert-True (-not $resumed.Data.stopped) 'queue runner stopped unexpectedly'
Assert-True ($resumed.Data.parallelism -eq 4) 'queue runner did not apply requested parallelism'
Assert-True (
    $resumed.Data.peak_active -ge 2 -and $resumed.Data.peak_active -le $resumed.Data.selected
) "bounded parallel scheduler did not execute multiple jobs within the selected window"
$outputs = @(Get-ChildItem -LiteralPath $outputRoot -Recurse -Filter '*.webp' -File)
Assert-True ($outputs.Count -eq 5) 'batch output count did not reconcile'
Assert-True (
    $null -ne (Get-Item -LiteralPath (Join-Path $outputRoot 'level-1\二级\three\雪.webp'))
) 'relative Unicode directory structure was not preserved'
Assert-True (
    @(Get-ChildItem -LiteralPath (Join-Path $outputRoot 'level-1') -Filter 'duplicate*.webp' -File).Count -eq 2
) 'duplicate stems did not receive deterministic distinct names'
foreach ($output in $outputs) {
    $facts = & ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,width,height -of json $output.FullName 2>$null
    Assert-True ($LASTEXITCODE -eq 0) "output did not open: $($output.FullName)"
    $stream = (($facts -join "`n") | ConvertFrom-Json).streams[0]
    Assert-True ($stream.codec_name -eq 'webp') 'batch output codec is not WebP'
    Assert-True ($stream.width -eq 48 -and $stream.height -eq 32) 'batch output dimensions are wrong'
}
$states = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', $database, 'jobs', 'list', '--limit', '100'
)
Assert-True (@($states.Data | Where-Object state -eq 'completed').Count -eq 5) 'durable job states did not reconcile'

$conflict = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
    '--json', '--state-db', (Join-Path $casePath 'conflict.sqlite3'),
    'batch-images', $inputRoot, '--output-dir', $outputRoot, '--to', 'webp', '--queue-only'
)
Assert-True ($conflict.Data.code -eq 'OUTPUT_CONFLICT') 'existing batch output was not blocked'

$changedInputRoot = Join-Path $casePath 'changed-input'
$changedOutputRoot = Join-Path $casePath 'changed-output'
$changedInput = Join-Path $changedInputRoot 'change.png'
New-TestImage -Path $changedInput -Color 'black'
$changedDatabase = Join-Path $casePath 'changed.sqlite3'
$queued = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', $changedDatabase,
    'batch-images', $changedInputRoot, '--output-dir', $changedOutputRoot,
    '--to', 'webp', '--queue-only'
)
Assert-True ($queued.Data.queued -eq 1) 'queue-only did not persist one job'
New-TestImage -Path $changedInput -Color 'white'
$changedRun = Invoke-FormatWrightJson -Arguments @(
    '--json', '--state-db', $changedDatabase, 'jobs', 'run', '--limit', '10'
)
Assert-True ($changedRun.Data.blocked -eq 1) 'changed input was not blocked during reinspection'
Assert-True (@(Get-ChildItem -LiteralPath $changedOutputRoot -Filter '*.webp' -File).Count -eq 0) 'changed input produced output'

Assert-True (
    @(Get-ChildItem -LiteralPath $casePath -Recurse -Filter '.formatwright-partial-*' -File).Count -eq 0
) 'batch suite left staged output files'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    discovered = $paused.Data.discovered
    planned = $paused.Data.planned
    skipped = $paused.Data.skipped
    pause = [ordered]@{ completed = $paused.Data.completed; queued = $paused.Data.queued }
    resume = [ordered]@{
        selected = $resumed.Data.selected
        completed = $resumed.Data.completed
        parallelism = $resumed.Data.parallelism
        peak_active = $resumed.Data.peak_active
    }
    final_outputs = $outputs.Count
    recursive_structure_preserved = $true
    directory_symlink_cycle_skipped = $true
    duplicate_stems_resolved = $true
    output_conflict_blocked = $true
    changed_input_blocked = $true
    staged_outputs_remaining = 0
}
$summaryPath = Join-Path $casePath 'batch-sandbox-result.json'
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8
$summary | ConvertTo-Json -Depth 8
