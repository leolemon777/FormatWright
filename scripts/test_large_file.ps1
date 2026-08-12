#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts'),
    [int64]$PeakControlPlaneBytes = 167772160,
    [int64]$MaximumGrowthBytes = 33554432
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) {
        throw "large-file assertion failed: $Message"
    }
}

function Set-SparseLength {
    param([string]$Path, [int64]$Length, [switch]$PreservePrefix)

    if (-not $PreservePrefix) {
        $stream = [IO.File]::Open($Path, 'CreateNew', 'ReadWrite', 'Read')
        $stream.Dispose()
    }
    & fsutil.exe sparse setflag $Path 2>$null | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) "filesystem refused sparse flag for $Path"
    $stream = [IO.File]::Open($Path, 'Open', 'ReadWrite', 'Read')
    try {
        $stream.SetLength($Length)
        [void]$stream.Seek([int64]($Length / 2), [IO.SeekOrigin]::Begin)
        $stream.WriteByte(0x5a)
        [void]$stream.Seek(-1, [IO.SeekOrigin]::End)
        $stream.WriteByte(0xa5)
        $stream.Flush($true)
    }
    finally {
        $stream.Dispose()
    }
}

function Invoke-MeasuredJson {
    param([string[]]$Arguments, [int[]]$ExpectedExitCodes = @(0))

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:BinaryPath
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        [void]$startInfo.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $clock = [Diagnostics.Stopwatch]::StartNew()
    Assert-True $process.Start() 'unable to start FormatWright'
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $peak = [int64]0
    while (-not $process.HasExited) {
        $process.Refresh()
        $peak = [Math]::Max($peak, $process.WorkingSet64)
        Start-Sleep -Milliseconds 1
    }
    $process.WaitForExit()
    $process.Refresh()
    $peak = [Math]::Max($peak, $process.PeakWorkingSet64)
    $clock.Stop()
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    $exitCode = $process.ExitCode
    $process.Dispose()
    Assert-True ($ExpectedExitCodes -contains $exitCode) (
        "unexpected exit code $exitCode for $($Arguments -join ' '): $stderr"
    )
    Assert-True (-not [string]::IsNullOrWhiteSpace($stdout)) 'JSON stdout is empty'
    Assert-True ($peak -gt 0) 'process exited without a valid working-set sample'
    [pscustomobject]@{
        ExitCode = $exitCode
        PeakWorkingSetBytes = $peak
        ElapsedMilliseconds = $clock.ElapsedMilliseconds
        Data = $stdout | ConvertFrom-Json
    }
}

Assert-True ($IsWindows) 'this sparse-file harness currently requires Windows/NTFS semantics'
Assert-True ($null -ne (Get-Command fsutil.exe -ErrorAction SilentlyContinue)) 'fsutil is required'
Assert-True ($null -ne (Get-Command ffmpeg -ErrorAction SilentlyContinue)) 'ffmpeg is required'
Assert-True ($null -ne (Get-Command ffprobe -ErrorAction SilentlyContinue)) 'ffprobe is required'
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path (
    (Resolve-Path -LiteralPath $ArtifactsRoot).Path
) ('large-file-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $casePath | Out-Null

$oneGiB = [int64]1 * 1024 * 1024 * 1024
$tenGiB = [int64]10 * 1024 * 1024 * 1024
$baselinePath = Join-Path $casePath 'baseline-1gib.sparse'
$largeInput = Join-Path $casePath 'media-10gib.mkv'
$outputPath = Join-Path $casePath 'media-10gib.converted.mp4'
$databasePath = Join-Path $casePath 'jobs.sqlite3'

Set-SparseLength -Path $baselinePath -Length $oneGiB
& ffmpeg -v error -f lavfi -i 'testsrc2=size=320x240:rate=24' `
    -f lavfi -i 'sine=frequency=440:sample_rate=48000' -t 2 `
    -c:v libx264 -preset ultrafast -c:a aac $largeInput 2>$null
Assert-True ($LASTEXITCODE -eq 0) 'failed to generate the media prefix'
Set-SparseLength -Path $largeInput -Length $tenGiB -PreservePrefix

$baseline = Invoke-MeasuredJson -Arguments @('--json', 'identify', $baselinePath)
$largeIdentity = Invoke-MeasuredJson -Arguments @('--json', 'identify', $largeInput)
Assert-True ($baseline.Data.size_bytes -eq $oneGiB) '1 GiB logical size is wrong'
Assert-True ($largeIdentity.Data.size_bytes -eq $tenGiB) '10 GiB logical size is wrong'
Assert-True (
    $largeIdentity.PeakWorkingSetBytes -le $PeakControlPlaneBytes
) '10 GiB identity exceeded the absolute control-plane memory gate'
Assert-True (
    ($largeIdentity.PeakWorkingSetBytes - $baseline.PeakWorkingSetBytes) -le $MaximumGrowthBytes
) 'control-plane memory grew with logical file size beyond the allowed delta'

$inspection = Invoke-MeasuredJson -Arguments @('--json', 'inspect', $largeInput)
Assert-True ($inspection.Data.artifact.size_bytes -eq $tenGiB) 'inspection lost the 10 GiB size'
Assert-True ($inspection.Data.format.id -eq 'mkv') '10 GiB fixture was not detected as MKV'
$conversion = Invoke-MeasuredJson -Arguments @(
    '--json', '--state-db', $databasePath, 'convert', $largeInput,
    '--to', 'mp4', '--output', $outputPath
)
Assert-True ($conversion.Data.status -eq 'pass') '10 GiB path validation did not pass'
Assert-True ($conversion.PeakWorkingSetBytes -le $PeakControlPlaneBytes) (
    '10 GiB conversion exceeded the parent control-plane memory gate'
)
Assert-True (Test-Path -LiteralPath $outputPath -PathType Leaf) '10 GiB output is missing'
$externalProbeLines = & ffprobe -v error -show_entries format=format_name -of json $outputPath 2>$null
Assert-True ($LASTEXITCODE -eq 0) 'independent ffprobe could not open 10 GiB path output'
$externalProbe = ($externalProbeLines -join "`n") | ConvertFrom-Json
Assert-True ($externalProbe.format.format_name -match 'mp4') 'output is not MP4'
$jobsLines = & $script:BinaryPath --json --state-db $databasePath jobs list 2>$null
Assert-True ($LASTEXITCODE -eq 0) 'jobs list failed after 10 GiB conversion'
$jobs = @(($jobsLines -join "`n") | ConvertFrom-Json)
Assert-True ($jobs.Count -eq 1 -and $jobs[0].state -eq 'completed') '10 GiB job is not completed'
Assert-True (
    @(Get-ChildItem -LiteralPath $casePath -Filter '.formatwright-partial-*' -File).Count -eq 0
) '10 GiB conversion left a staged output'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    generated_utc = [DateTime]::UtcNow.ToString('o')
    binary_sha256 = (Get-FileHash -LiteralPath $script:BinaryPath -Algorithm SHA256).Hash
    baseline = [ordered]@{
        logical_bytes = $baseline.Data.size_bytes
        peak_control_plane_bytes = $baseline.PeakWorkingSetBytes
        elapsed_ms = $baseline.ElapsedMilliseconds
    }
    ten_gib_identity = [ordered]@{
        logical_bytes = $largeIdentity.Data.size_bytes
        peak_control_plane_bytes = $largeIdentity.PeakWorkingSetBytes
        growth_over_1gib_bytes = $largeIdentity.PeakWorkingSetBytes - $baseline.PeakWorkingSetBytes
        elapsed_ms = $largeIdentity.ElapsedMilliseconds
    }
    ten_gib_conversion = [ordered]@{
        status = $conversion.Data.status
        operation = 'remux'
        parent_peak_control_plane_bytes = $conversion.PeakWorkingSetBytes
        elapsed_ms = $conversion.ElapsedMilliseconds
        independent_format = $externalProbe.format.format_name
        job_state = $jobs[0].state
    }
    gates = [ordered]@{
        absolute_peak_bytes = $PeakControlPlaneBytes
        maximum_growth_bytes = $MaximumGrowthBytes
    }
}
$summaryJson = $summary | ConvertTo-Json -Depth 8
[IO.File]::WriteAllText(
    (Join-Path $casePath 'summary.json'),
    $summaryJson + [Environment]::NewLine
)
$summaryJson
