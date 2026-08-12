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
    if (-not $Condition) { throw "zero-network assertion failed: $Message" }
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

foreach ($tool in @('ffmpeg', 'ffprobe', 'Get-NetTCPConnection', 'Get-NetUDPEndpoint')) {
    Assert-True ($null -ne (Get-Command $tool -ErrorAction SilentlyContinue)) "$tool is required"
}
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'zero-network-suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath | Out-Null
$input = Join-Path $casePath 'local-input.mkv'
$output = Join-Path $casePath 'local-output.mp4'
$database = Join-Path $casePath 'jobs.sqlite3'
$stdout = Join-Path $casePath 'convert.stdout.json'
$stderr = Join-Path $casePath 'convert.stderr.log'

& ffmpeg -hide_banner -loglevel error -y `
    -f lavfi -i 'testsrc2=size=1280x720:rate=30' `
    -f lavfi -i 'sine=frequency=880:sample_rate=48000' `
    -t 8 -c:v mpeg2video -q:v 2 -c:a mp2 $input
Assert-True ($LASTEXITCODE -eq 0) 'unable to generate the local-only media fixture'

$plan = Invoke-Json -Arguments @('--json', 'plan', $input, '--to', 'mp4', '--output', $output)
Assert-True ($plan.network_policy -eq 'deny') 'Plan network policy was not deny'

$arguments = @('--json', '--state-db', $database, 'convert', $input, '--to', 'mp4', '--output', $output)
$process = Start-Process -FilePath $script:BinaryPath -ArgumentList $arguments `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WindowStyle Hidden -PassThru
$observations = [System.Collections.Generic.List[object]]::new()
$knownIds = [System.Collections.Generic.HashSet[int]]::new()
$samples = 0
$maximumTreeProcesses = 0
do {
    $process.Refresh()
    $tree = @(Get-DescendantProcessIds -RootId $process.Id)
    foreach ($processId in $tree) { $knownIds.Add($processId) | Out-Null }
    $maximumTreeProcesses = [Math]::Max($maximumTreeProcesses, $tree.Count)
    foreach ($processId in $tree) {
        foreach ($connection in @(Get-NetTCPConnection -OwningProcess $processId -ErrorAction SilentlyContinue)) {
            $observations.Add([pscustomobject]@{
                protocol = 'tcp'
                process_id = $processId
                local = "$($connection.LocalAddress):$($connection.LocalPort)"
                remote = "$($connection.RemoteAddress):$($connection.RemotePort)"
                state = [string]$connection.State
            })
        }
        foreach ($endpoint in @(Get-NetUDPEndpoint -OwningProcess $processId -ErrorAction SilentlyContinue)) {
            $observations.Add([pscustomobject]@{
                protocol = 'udp'
                process_id = $processId
                local = "$($endpoint.LocalAddress):$($endpoint.LocalPort)"
                remote = $null
                state = 'bound'
            })
        }
    }
    $samples++
    if (-not $process.HasExited) { Start-Sleep -Milliseconds 50 }
} while (-not $process.HasExited)
$process.WaitForExit()
$process.Refresh()

Assert-True ($process.ExitCode -eq 0) (
    "conversion exited $($process.ExitCode): " + (Get-Content -LiteralPath $stderr -Raw)
)
Assert-True (Test-Path -LiteralPath $output -PathType Leaf) 'validated output was not committed'
Assert-True ($observations.Count -eq 0) 'the application or a descendant opened a TCP/UDP endpoint'

$result = [ordered]@{
    schema_version = 1
    case_path = $casePath
    plan_network_policy = $plan.network_policy
    sampling_interval_ms = 50
    samples = $samples
    observed_process_ids = @($knownIds)
    maximum_tree_processes = $maximumTreeProcesses
    tcp_udp_observations = @($observations)
    output_committed = $true
    limitation = 'Polling evidence cannot detect a socket opened and closed entirely between samples or mapped-network drives.'
}
$summary = Join-Path $casePath 'zero-network-result.json'
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summary -Encoding utf8
$result | ConvertTo-Json -Depth 8
