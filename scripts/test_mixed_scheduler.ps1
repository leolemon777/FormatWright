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
    if (-not $Condition) { throw "mixed scheduler assertion failed: $Message" }
}

function Invoke-Json {
    param([string[]]$Arguments)
    $lines = & $script:BinaryPath @Arguments 2>$null
    Assert-True ($LASTEXITCODE -eq 0) ('command failed: ' + ($Arguments -join ' '))
    ($lines -join "`n") | ConvertFrom-Json
}

function Get-DescendantProcessIds {
    param([int]$RootId)
    $known = [System.Collections.Generic.HashSet[int]]::new()
    $frontier = [System.Collections.Generic.Queue[int]]::new()
    $known.Add($RootId) | Out-Null
    $frontier.Enqueue($RootId)
    while ($frontier.Count -gt 0) {
        $parent = $frontier.Dequeue()
        $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $parent" -ErrorAction SilentlyContinue)
        foreach ($child in $children) {
            $childId = [int]$child.ProcessId
            if ($known.Add($childId)) { $frontier.Enqueue($childId) }
        }
    }
    @($known)
}

foreach ($tool in @('ffmpeg', 'ffprobe')) {
    Assert-True ($null -ne (Get-Command $tool -ErrorAction SilentlyContinue)) "$tool is required"
}
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'mixed-scheduler-suite-' + [Guid]::NewGuid().ToString('N')
)
$inputRoot = Join-Path $casePath 'inputs'
$outputRoot = Join-Path $casePath 'outputs'
New-Item -ItemType Directory -Path $inputRoot -Force | Out-Null
New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null

$jsonInput = Join-Path $inputRoot 'records.json'
[System.IO.File]::WriteAllText($jsonInput, '[{"id":1,"name":"alpha"},{"id":2,"name":"beta"}]')
$imageInput = Join-Path $inputRoot 'image.png'
& ffmpeg -hide_banner -loglevel error -y `
    -f lavfi -i 'testsrc2=size=1600x1000:rate=1' -frames:v 1 $imageInput
Assert-True ($LASTEXITCODE -eq 0) 'unable to generate image fixture'
$videoInput = Join-Path $inputRoot 'long-source.mkv'
& ffmpeg -hide_banner -loglevel error -y `
    -f lavfi -i 'testsrc2=size=1920x1080:rate=30' `
    -f lavfi -i 'sine=frequency=660:sample_rate=48000' `
    -t 180 -c:v mpeg2video -q:v 8 -c:a mp2 $videoInput
Assert-True ($LASTEXITCODE -eq 0) 'unable to generate long media fixture'

$database = Join-Path $casePath 'jobs.sqlite3'
$jobIds = [System.Collections.Generic.List[string]]::new()
for ($index = 1; $index -le 3; $index++) {
    $queued = Invoke-Json -Arguments @(
        '--json', '--state-db', $database, 'convert', $jsonInput,
        '--to', 'yaml', '--output', (Join-Path $outputRoot "records-$index.yaml"), '--queue-only'
    )
    $jobIds.Add([string]$queued.id)
}
for ($index = 1; $index -le 3; $index++) {
    $queued = Invoke-Json -Arguments @(
        '--json', '--state-db', $database, 'convert', $imageInput,
        '--to', 'webp', '--output', (Join-Path $outputRoot "image-$index.webp"),
        '--width', '800', '--quality', '80', '--queue-only'
    )
    $jobIds.Add([string]$queued.id)
}
for ($index = 1; $index -le 3; $index++) {
    $queued = Invoke-Json -Arguments @(
        '--json', '--state-db', $database, 'convert', $videoInput,
        '--to', 'mp4', '--output', (Join-Path $outputRoot "video-$index.mp4"), '--queue-only'
    )
    $jobIds.Add([string]$queued.id)
}
Assert-True ($jobIds.Count -eq 9) 'did not queue all mixed jobs'

$stdout = Join-Path $casePath 'run.stdout.json'
$stderr = Join-Path $casePath 'run.stderr.log'
$arguments = @(
    '--json', '--state-db', $database, 'jobs', 'run', '--limit', '9', '--parallel', '4'
)
$process = Start-Process -FilePath $script:BinaryPath -ArgumentList $arguments `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WindowStyle Hidden -PassThru
$samples = 0
$peakParentRss = 0L
$peakTreeRss = 0L
$peakEngineProcesses = 0
$peakWalBytes = 0L
do {
    $process.Refresh()
    $ids = @(Get-DescendantProcessIds -RootId $process.Id)
    $treeRss = 0L
    foreach ($processId in $ids) {
        $observed = Get-Process -Id $processId -ErrorAction SilentlyContinue
        if ($null -ne $observed) {
            $treeRss += [long]$observed.WorkingSet64
        }
    }
    $engineProcesses = @(
        Get-CimInstance Win32_Process -Filter "Name = 'ffmpeg.exe'" -ErrorAction SilentlyContinue |
            Where-Object { $null -ne $_.CommandLine -and $_.CommandLine.Contains($casePath) }
    ).Count
    $parent = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
    if ($null -ne $parent) { $peakParentRss = [Math]::Max($peakParentRss, [long]$parent.WorkingSet64) }
    $peakTreeRss = [Math]::Max($peakTreeRss, $treeRss)
    $peakEngineProcesses = [Math]::Max($peakEngineProcesses, $engineProcesses)
    $walPath = "$database-wal"
    if (Test-Path -LiteralPath $walPath -PathType Leaf) {
        $peakWalBytes = [Math]::Max($peakWalBytes, [long](Get-Item -LiteralPath $walPath).Length)
    }
    $samples++
    if (-not $process.HasExited) { Start-Sleep -Milliseconds 50 }
} while (-not $process.HasExited)
$process.WaitForExit()
$process.Refresh()

Assert-True ($process.ExitCode -eq 0) (
    "queue runner exited $($process.ExitCode): " + (Get-Content -LiteralPath $stderr -Raw)
)
$run = Get-Content -LiteralPath $stdout -Raw | ConvertFrom-Json
$jobs = Invoke-Json -Arguments @('--json', '--state-db', $database, 'jobs', 'list', '--limit', '20')
$intervals = [System.Collections.Generic.List[object]]::new()
foreach ($job in $jobs) {
    $details = Invoke-Json -Arguments @('--json', '--state-db', $database, 'jobs', 'show', $job.id)
    $started = ($details.events | Where-Object code -eq 'ENGINE_STARTED').timestamp_unix_ms
    $finished = ($details.events | Where-Object code -eq 'ENGINE_FINISHED').timestamp_unix_ms
    if ($null -ne $started -and $null -ne $finished) {
        $intervals.Add([pscustomobject]@{ Start = [long]$started; Finish = [long]$finished })
    }
}
$maximumDurableRunningOverlap = 0
foreach ($interval in $intervals) {
    $overlap = @(
        $intervals | Where-Object {
            $_.Start -le $interval.Start -and $_.Finish -gt $interval.Start
        }
    ).Count
    $maximumDurableRunningOverlap = [Math]::Max($maximumDurableRunningOverlap, $overlap)
}
Assert-True ($run.selected -eq 9 -and $run.completed -eq 9) 'mixed queue did not complete all jobs'
Assert-True ($run.failed -eq 0 -and $run.blocked -eq 0 -and $run.cancelled -eq 0) 'mixed queue had terminal errors'
Assert-True ($run.parallelism -eq 4) 'requested parallelism was not applied'
Assert-True ($run.peak_active -ge 2 -and $run.peak_active -le 4) 'scheduler active count was not bounded and concurrent'
Assert-True ($peakEngineProcesses -ge 2 -and $peakEngineProcesses -le 4) (
    "real FFmpeg process concurrency was not bounded and concurrent; observed $peakEngineProcesses"
)
Assert-True (
    $maximumDurableRunningOverlap -ge 2 -and $maximumDurableRunningOverlap -le 4
) 'durable running-state intervals were not bounded and concurrent'
Assert-True (@(Get-ChildItem -LiteralPath $outputRoot -File).Count -eq 9) 'output count did not reconcile'
Assert-True (
    @(Get-ChildItem -LiteralPath $casePath -Recurse -Filter '.formatwright-partial-*' -File).Count -eq 0
) 'mixed scheduler left staged output files'

$result = [ordered]@{
    schema_version = 1
    case_path = $casePath
    queued = $jobIds.Count
    selected = $run.selected
    completed = $run.completed
    configured_parallelism = $run.parallelism
    scheduler_peak_active = $run.peak_active
    observed_peak_ffmpeg_processes = $peakEngineProcesses
    durable_peak_running_state_overlap = $maximumDurableRunningOverlap
    sampling_interval_ms = 50
    samples = $samples
    peak_parent_rss_bytes = $peakParentRss
    peak_process_tree_rss_bytes = $peakTreeRss
    peak_wal_bytes = $peakWalBytes
    final_database_bytes = [long](Get-Item -LiteralPath $database).Length
    output_count = 9
    staged_outputs_remaining = 0
    limitation = '50 ms polling can miss shorter process and RSS peaks; measurements are host-specific development evidence.'
}
$summary = Join-Path $casePath 'mixed-scheduler-result.json'
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summary -Encoding utf8
$result | ConvertTo-Json -Depth 8
