#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts'),
    [ValidateRange(2, 16)][int]$RunnerCount = 4,
    [ValidateRange(2, 256)][int]$JobCount = 24
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "multi-process queue assertion failed: $Message" }
}

function Invoke-Json {
    param([string[]]$Arguments)
    $lines = & $script:BinaryPath @Arguments 2>$null
    Assert-True ($LASTEXITCODE -eq 0) ('command failed: ' + ($Arguments -join ' '))
    ($lines -join "`n") | ConvertFrom-Json
}

$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'multi-process-queue-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath -Force | Out-Null
$idempotencyDatabase = Join-Path $casePath 'idempotency.sqlite3'
$idempotencyOutput = Join-Path $casePath 'idempotent-output.yaml'
[System.IO.File]::WriteAllText(
    (Join-Path $casePath 'idempotency-input.json'),
    '[{"id":1,"name":"idempotency"}]'
)
$null = Invoke-Json -Arguments @(
    '--json', '--state-db', $idempotencyDatabase, 'jobs', 'list', '--limit', '1'
)
$idempotencyProcesses = [System.Collections.Generic.List[System.Diagnostics.Process]]::new()
for ($index = 0; $index -lt $RunnerCount; $index++) {
    $idempotencyProcesses.Add((Start-Process -FilePath $script:BinaryPath -ArgumentList @(
        '--json', '--state-db', $idempotencyDatabase, 'convert',
        (Join-Path $casePath 'idempotency-input.json'), '--to', 'yaml',
        '--output', $idempotencyOutput, '--queue-only',
        '--idempotency-key', 'multi-process-idempotent-replay'
    ) -RedirectStandardOutput (Join-Path $casePath "idempotency-$index.json") `
        -RedirectStandardError (Join-Path $casePath "idempotency-$index.stderr.log") `
        -WindowStyle Hidden -PassThru))
}
for ($index = 0; $index -lt $idempotencyProcesses.Count; $index++) {
    $idempotencyProcesses[$index].WaitForExit()
    Assert-True ($idempotencyProcesses[$index].ExitCode -eq 0) (
        "idempotency runner $index exited $($idempotencyProcesses[$index].ExitCode): " +
        (Get-Content -LiteralPath (Join-Path $casePath "idempotency-$index.stderr.log") -Raw)
    )
}
$idempotencyIds = @(0..($RunnerCount - 1) | ForEach-Object {
    [string](Get-Content -LiteralPath (Join-Path $casePath "idempotency-$_.json") -Raw |
        ConvertFrom-Json).id
} | Sort-Object -Unique)
$idempotencyJobs = @(Invoke-Json -Arguments @(
    '--json', '--state-db', $idempotencyDatabase, 'jobs', 'list', '--limit', '100'
))
Assert-True ($idempotencyIds.Count -eq 1) 'concurrent idempotent replay returned different Job IDs'
Assert-True ($idempotencyJobs.Count -eq 1) 'concurrent idempotent replay created duplicate Jobs'

$database = Join-Path $casePath 'jobs.sqlite3'
$input = Join-Path $casePath 'input.json'
$gate = Join-Path $casePath 'start.signal'
[System.IO.File]::WriteAllText($input, '[{"id":1,"name":"multi-process"}]')

$jobIds = [System.Collections.Generic.List[string]]::new()
for ($index = 0; $index -lt $JobCount; $index++) {
    $queued = Invoke-Json -Arguments @(
        '--json', '--state-db', $database, 'convert', $input,
        '--to', 'yaml', '--output', (Join-Path $casePath "output-$index.yaml"),
        '--queue-only', '--idempotency-key', "multi-process-$index"
    )
    $jobIds.Add([string]$queued.id)
}

$processes = [System.Collections.Generic.List[System.Diagnostics.Process]]::new()
for ($index = 0; $index -lt $RunnerCount; $index++) {
    $processes.Add((Start-Process -FilePath $script:BinaryPath -ArgumentList @(
        '--json', '--state-db', $database, 'jobs', 'run',
        '--limit', [string]$JobCount, '--parallel', '2', '--start-gate', $gate
    ) -RedirectStandardOutput (Join-Path $casePath "runner-$index.json") `
        -RedirectStandardError (Join-Path $casePath "runner-$index.stderr.log") `
        -WindowStyle Hidden -PassThru))
}
Start-Sleep -Milliseconds 500
New-Item -ItemType File -Path $gate | Out-Null
for ($index = 0; $index -lt $processes.Count; $index++) {
    $processes[$index].WaitForExit()
    Assert-True ($processes[$index].ExitCode -eq 0) (
        "runner $index exited $($processes[$index].ExitCode): " +
        (Get-Content -LiteralPath (Join-Path $casePath "runner-$index.stderr.log") -Raw)
    )
}

$runs = @(0..($RunnerCount - 1) | ForEach-Object {
    Get-Content -LiteralPath (Join-Path $casePath "runner-$_.json") -Raw | ConvertFrom-Json
})
$jobs = @(Invoke-Json -Arguments @('--json', '--state-db', $database, 'jobs', 'list', '--limit', '1000'))
$engineStarts = 0
foreach ($jobId in $jobIds) {
    $details = Invoke-Json -Arguments @('--json', '--state-db', $database, 'jobs', 'show', $jobId)
    $engineStarts += @($details.events | Where-Object code -eq 'ENGINE_STARTED').Count
}
$selected = [int](($runs | Measure-Object selected -Sum).Sum)
$completed = [int](($runs | Measure-Object completed -Sum).Sum)
$contended = [int](($runs | Measure-Object contended -Sum).Sum)
$failed = [int](($runs | Measure-Object failed -Sum).Sum)
$outputs = @(Get-ChildItem -LiteralPath $casePath -Filter 'output-*.yaml' -File).Count
$reports = @(Get-ChildItem -LiteralPath (Join-Path $casePath 'reports') -Filter '*.json' -File).Count

Assert-True ($selected -eq ($JobCount * $RunnerCount)) 'every gated runner must see the same queued window'
Assert-True ($completed -eq $JobCount) 'each Job must complete exactly once'
Assert-True ($contended -eq ($selected - $JobCount)) 'every losing claim must be reported as contended'
Assert-True ($failed -eq 0) 'contention must not create failed Jobs'
Assert-True (@($jobs | Where-Object state -eq 'completed').Count -eq $JobCount) 'final state count mismatch'
Assert-True ($engineStarts -eq $JobCount) 'an engine started more than once for a Job'
Assert-True ($outputs -eq $JobCount -and $reports -eq $JobCount) 'output/report reconciliation failed'
Assert-True (@(Get-ChildItem -LiteralPath $casePath -Recurse -Filter '.formatwright-partial-*' -File).Count -eq 0) 'partial outputs remain'

$result = [ordered]@{
    schema_version = 1
    case_path = $casePath
    runner_count = $RunnerCount
    jobs = $JobCount
    selected = $selected
    completed = $completed
    contended = $contended
    failed = $failed
    engine_started_events = $engineStarts
    outputs = $outputs
    reports = $reports
    staged_outputs_remaining = 0
    idempotency_processes = $RunnerCount
    idempotency_unique_jobs = $idempotencyJobs.Count
}
$result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (
    Join-Path $casePath 'multi-process-queue-result.json'
) -Encoding utf8
$result | ConvertTo-Json -Depth 6
