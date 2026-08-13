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
    if (-not $Condition) { throw "queue crash-recovery assertion failed: $Message" }
}

function Invoke-Json {
    param([string[]]$Arguments)
    $lines = & $script:BinaryPath @Arguments 2>$null
    Assert-True ($LASTEXITCODE -eq 0) ('command failed: ' + ($Arguments -join ' '))
    ($lines -join "`n") | ConvertFrom-Json
}

function Get-StagedPath {
    param([string]$Output, [string]$JobId)
    Join-Path (Split-Path -Parent $Output) (
        '.formatwright-partial-' + $JobId + '-' + (Split-Path -Leaf $Output)
    )
}

foreach ($tool in @('ffmpeg', 'ffprobe')) {
    Assert-True ($null -ne (Get-Command $tool -ErrorAction SilentlyContinue)) "$tool is required"
}
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'queue-crash-recovery-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath -Force | Out-Null
$database = Join-Path $casePath 'jobs.sqlite3'
$input = Join-Path $casePath 'long-source.mkv'
$output = Join-Path $casePath 'recovered-output.mp4'

& ffmpeg -hide_banner -loglevel error -y `
    -f lavfi -i 'testsrc2=size=1280x720:rate=30' `
    -f lavfi -i 'sine=frequency=880:sample_rate=48000' `
    -t 120 -c:v mpeg4 -q:v 3 -c:a libmp3lame -b:a 128k $input
Assert-True ($LASTEXITCODE -eq 0) 'unable to generate crash fixture'
$inputHash = (Get-FileHash -LiteralPath $input -Algorithm SHA256).Hash
$queued = Invoke-Json -Arguments @(
    '--json', '--state-db', $database, 'convert', $input,
    '--to', 'mp4', '--output', $output, '--queue-only',
    '--idempotency-key', 'queue-crash-recovery'
)
$jobId = [string]$queued.id
$partial = Get-StagedPath -Output $output -JobId $jobId
$stdout = Join-Path $casePath 'killed-runner.stdout.json'
$stderr = Join-Path $casePath 'killed-runner.stderr.log'
$runner = Start-Process -FilePath $script:BinaryPath -ArgumentList @(
    '--json', '--state-db', $database, 'jobs', 'run', '--limit', '1', '--parallel', '1'
) -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WindowStyle Hidden -PassThru

$deadline = [DateTime]::UtcNow.AddSeconds(45)
$runningObserved = $false
do {
    Start-Sleep -Milliseconds 100
    $details = Invoke-Json -Arguments @('--json', '--state-db', $database, 'jobs', 'show', $jobId)
    $runningObserved = $details.job.state -eq 'running'
    $runner.Refresh()
} until (
    ($runningObserved -and (Test-Path -LiteralPath $partial) -and
        (Get-Item -LiteralPath $partial).Length -gt 0) -or
    $runner.HasExited -or [DateTime]::UtcNow -ge $deadline
)
Assert-True (-not $runner.HasExited) 'runner exited before crash injection'
Assert-True $runningObserved 'queued Job never reached Running'
Assert-True (Test-Path -LiteralPath $partial) 'worker never created a staged output'
$processInfo = Get-CimInstance Win32_Process -Filter "ProcessId = $($runner.Id)"
Assert-True ($null -ne $processInfo) 'runner disappeared before identity verification'
Assert-True (
    [IO.Path]::GetFullPath($processInfo.ExecutablePath) -eq [IO.Path]::GetFullPath($script:BinaryPath)
) 'refusing to terminate an unexpected process'
& taskkill.exe /PID $runner.Id /T /F 2>$null | Out-Null
Assert-True ($LASTEXITCODE -eq 0) 'targeted runner process-tree termination failed'
$runner.WaitForExit()
Start-Sleep -Milliseconds 300
Assert-True (-not (Test-Path -LiteralPath $output)) 'killed runner committed the destination'
Assert-True (Test-Path -LiteralPath $partial) 'killed runner left no recovery evidence'

$recovery = Invoke-Json -Arguments @('--json', '--state-db', $database, 'jobs', 'recover')
Assert-True (@($recovery.interrupted_jobs).Count -eq 1) 'recover did not interrupt one Job'
Assert-True (@($recovery.removed_staged_outputs).Count -eq 1) 'recover did not remove one partial'
Assert-True (-not (Test-Path -LiteralPath $partial)) 'partial remains after recover'
$resumed = Invoke-Json -Arguments @('--json', '--state-db', $database, 'jobs', 'resume', $jobId)
Assert-True ($resumed.state -eq 'queued') 'resume did not requeue the interrupted Job'
$run = Invoke-Json -Arguments @(
    '--json', '--state-db', $database, 'jobs', 'run', '--limit', '1', '--parallel', '1'
)
$final = Invoke-Json -Arguments @('--json', '--state-db', $database, 'jobs', 'show', $jobId)
$report = Join-Path $casePath "reports\$jobId.json"
Assert-True ($run.completed -eq 1) 'resumed window did not complete the Job'
Assert-True ($final.job.state -eq 'completed') 'final Job state is not Completed'
Assert-True (Test-Path -LiteralPath $output -PathType Leaf) 'final output is missing'
Assert-True (Test-Path -LiteralPath $report -PathType Leaf) 'final report is missing'
Assert-True (
    $inputHash -eq (Get-FileHash -LiteralPath $input -Algorithm SHA256).Hash
) 'crash/recovery modified the input'
Assert-True (
    @(Get-ChildItem -LiteralPath $casePath -Recurse -Filter '.formatwright-partial-*' -File).Count -eq 0
) 'recovery left staged output files'
foreach ($code in @('ENGINE_STARTED', 'RECOVERED_AFTER_RESTART', 'JOB_RESUMED', 'VALIDATION_FINISHED')) {
    Assert-True (@($final.events.code) -contains $code) "missing durable event $code"
}
$probe = & ffprobe -v error -show_entries format=format_name -of json $output 2>$null
Assert-True ($LASTEXITCODE -eq 0) 'independent ffprobe could not open recovered output'

$result = [ordered]@{
    schema_version = 1
    case_path = $casePath
    job_id = $jobId
    killed_while_running = $runningObserved
    recovered_jobs = @($recovery.interrupted_jobs).Count
    partials_removed = @($recovery.removed_staged_outputs).Count
    resumed_completed = $run.completed
    final_state = $final.job.state
    output_exists = $true
    report_exists = $true
    input_unchanged = $true
    staged_outputs_remaining = 0
}
$result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (
    Join-Path $casePath 'queue-crash-recovery-result.json'
) -Encoding utf8
$result | ConvertTo-Json -Depth 6
