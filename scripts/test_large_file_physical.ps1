#requires -Version 7.0

# E-batch physical large-file gate: unlike test_large_file.ps1 (sparse
# logical size), this harness builds a GENUINELY allocated >= 10 GiB valid
# MKV by stream-copying a real chunk repeatedly, then pushes it through the
# real CLI identify/inspect/convert remux path with control-plane memory
# gates. Proves physical sequential read/write streaming, not just logical
# size handling.
[CmdletBinding()]
param(
    [string]$Binary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts\large-file-physical'),
    [int64]$MinimumBytes = ([int64]10 * 1GB),
    [int64]$PeakControlPlaneBytes = 167772160
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "physical large-file assertion failed: $Message" }
}

function Invoke-MeasuredJson {
    param([string[]]$Arguments)

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
    Assert-True ($exitCode -eq 0) "unexpected exit code $exitCode for $($Arguments -join ' '): $stderr"
    Assert-True (-not [string]::IsNullOrWhiteSpace($stdout)) 'JSON stdout is empty'
    Assert-True ($peak -gt 0) 'process exited without a valid working-set sample'
    [pscustomobject]@{
        ExitCode = $exitCode
        PeakWorkingSetBytes = $peak
        ElapsedMilliseconds = $clock.ElapsedMilliseconds
        Data = $stdout | ConvertFrom-Json
    }
}

Assert-True ($IsWindows) 'this physical harness currently requires Windows/NTFS semantics'
Assert-True ($null -ne (Get-Command fsutil.exe -ErrorAction SilentlyContinue)) 'fsutil is required'
Assert-True ($null -ne (Get-Command ffmpeg -ErrorAction SilentlyContinue)) 'ffmpeg is required'
Assert-True ($null -ne (Get-Command ffprobe -ErrorAction SilentlyContinue)) 'ffprobe is required'
$drive = (Get-PSDrive -Name ($ArtifactsRoot.Substring(0, 1)) -ErrorAction SilentlyContinue)
if ($null -ne $drive) {
    Assert-True ($drive.Free -gt ($MinimumBytes * 3 + 2GB)) (
        "not enough free space for input + output + headroom: $($drive.Free) bytes free"
    )
}
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) ('physical-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $casePath | Out-Null

# 1. Build a real ~60s chunk, then stream-copy it N times into one valid MKV
#    so the final file carries genuinely allocated bytes end to end.
$chunkPath = Join-Path $casePath 'chunk-60s.mkv'
$concatList = Join-Path $casePath 'concat.txt'
$largeInput = Join-Path $casePath 'media-physical-10gib.mkv'
$outputPath = Join-Path $casePath 'media-physical-10gib.converted.mp4'
$databasePath = Join-Path $casePath 'jobs.sqlite3'

& ffmpeg -v error -f lavfi -i 'testsrc2=size=320x240:rate=24' `
    -f lavfi -i 'sine=frequency=440:sample_rate=48000' -t 60 `
    -c:v libx264 -preset ultrafast -c:a aac $chunkPath 2>$null
Assert-True ($LASTEXITCODE -eq 0) 'failed to generate the real media chunk'
$chunkBytes = (Get-Item -LiteralPath $chunkPath).Length
Assert-True ($chunkBytes -gt 0) 'media chunk is empty'
$repeats = [int][Math]::Ceiling($MinimumBytes / $chunkBytes) + 1
$concatLines = [System.Collections.Generic.List[string]]::new()
for ($index = 0; $index -lt $repeats; $index++) {
    $concatLines.Add("file '$($chunkPath -replace "'", "'\''")'")
}
[IO.File]::WriteAllLines($concatList, $concatLines)

$generationClock = [Diagnostics.Stopwatch]::StartNew()
& ffmpeg -v error -f concat -safe 0 -i $concatList -c copy $largeInput 2>$null
Assert-True ($LASTEXITCODE -eq 0) 'failed to assemble the physical 10 GiB container'
$generationClock.Stop()
$physicalBytes = (Get-Item -LiteralPath $largeInput).Length
Assert-True ($physicalBytes -ge $MinimumBytes) "physical container is only $physicalBytes bytes"
# Locale-independent sparse check: allocated size below logical size means
# the file is sparse (or transparently compressed); both must match here.
Add-Type -Namespace Win32 -Name DiskSizes -MemberDefinition '
[DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern uint GetCompressedFileSizeW(string fileName, out uint fileSizeHigh);
'
[uint32]$allocatedHigh = 0
[uint32]$allocatedLow = [Win32.DiskSizes]::GetCompressedFileSizeW($largeInput, [ref]$allocatedHigh)
$allocatedBytes = ([int64]$allocatedHigh -shl 32) -bor $allocatedLow
Assert-True ($allocatedBytes -ge $physicalBytes) (
    "container is not fully allocated on disk: $allocatedBytes allocated of $physicalBytes logical"
)
$sparseReport = "allocated_bytes=$allocatedBytes logical_bytes=$physicalBytes"
Remove-Item -LiteralPath $chunkPath, $concatList -Force

# 2. Push the real bytes through the CLI path with memory gates.
$identity = Invoke-MeasuredJson -Arguments @('--json', 'identify', $largeInput)
Assert-True ($identity.Data.size_bytes -eq $physicalBytes) 'physical size was not reported exactly'
Assert-True ($identity.PeakWorkingSetBytes -le $PeakControlPlaneBytes) (
    'physical identity exceeded the control-plane memory gate'
)
$inspection = Invoke-MeasuredJson -Arguments @('--json', 'inspect', $largeInput)
Assert-True ($inspection.Data.format.id -eq 'mkv') 'physical fixture was not detected as MKV'

$conversion = Invoke-MeasuredJson -Arguments @(
    '--json', '--state-db', $databasePath, 'convert', $largeInput,
    '--to', 'mp4', '--output', $outputPath
)
Assert-True ($conversion.Data.status -eq 'pass') 'physical conversion validation did not pass'
Assert-True ($conversion.PeakWorkingSetBytes -le $PeakControlPlaneBytes) (
    'physical conversion exceeded the control-plane memory gate'
)
Assert-True (Test-Path -LiteralPath $outputPath -PathType Leaf) 'physical output is missing'
$outputBytes = (Get-Item -LiteralPath $outputPath).Length
Assert-True ($outputBytes -ge ($physicalBytes - 100MB)) (
    "remux output shrank unexpectedly: $outputBytes of $physicalBytes"
)
$probeLines = & ffprobe -v error -show_entries format=format_name,duration -of json $outputPath 2>$null
Assert-True ($LASTEXITCODE -eq 0) 'independent ffprobe could not open the physical output'
$probe = ($probeLines -join "`n") | ConvertFrom-Json
Assert-True ($probe.format.format_name -match 'mp4') 'output is not MP4'
$expectedDuration = [double]$repeats * 60.0
Assert-True (
    [Math]::Abs($probe.format.duration - $expectedDuration) -le ($expectedDuration * 0.02)
) "output duration $($probe.format.duration) drifted from expected $expectedDuration"

$jobsLines = & $script:BinaryPath --json --state-db $databasePath jobs list 2>$null
Assert-True ($LASTEXITCODE -eq 0) 'jobs list failed after physical conversion'
$jobs = @(($jobsLines -join "`n") | ConvertFrom-Json)
Assert-True ($jobs.Count -eq 1 -and $jobs[0].state -eq 'completed') 'physical job is not completed'
Assert-True (
    @(Get-ChildItem -LiteralPath $casePath -Filter '.formatwright-partial-*' -File).Count -eq 0
) 'physical conversion left a staged output'

$summary = [ordered]@{
    schema_version = 1
    case_path = $casePath
    generated_utc = [DateTime]::UtcNow.ToString('o')
    binary_sha256 = (Get-FileHash -LiteralPath $script:BinaryPath -Algorithm SHA256).Hash
    physical_input_bytes = $physicalBytes
    chunk_bytes = $chunkBytes
    repeats = $repeats
    sparse_flag_report = $sparseReport
    generation = [ordered]@{
        elapsed_ms = $generationClock.ElapsedMilliseconds
        write_mib_per_s = [Math]::Round($physicalBytes / 1MB / ($generationClock.ElapsedMilliseconds / 1000), 1)
    }
    identity = [ordered]@{
        peak_control_plane_bytes = $identity.PeakWorkingSetBytes
        elapsed_ms = $identity.ElapsedMilliseconds
    }
    conversion = [ordered]@{
        status = $conversion.Data.status
        operation = 'remux'
        output_bytes = $outputBytes
        parent_peak_control_plane_bytes = $conversion.PeakWorkingSetBytes
        elapsed_ms = $conversion.ElapsedMilliseconds
        io_mib_per_s = [Math]::Round(($physicalBytes + $outputBytes) / 1MB / ($conversion.ElapsedMilliseconds / 1000), 1)
        independent_format = $probe.format.format_name
        independent_duration_s = [Math]::Round($probe.format.duration, 3)
        job_state = $jobs[0].state
    }
    gates = [ordered]@{
        absolute_peak_bytes = $PeakControlPlaneBytes
        minimum_input_bytes = $MinimumBytes
    }
}
$summaryPath = Join-Path $casePath 'physical-large-file-summary.json'
$summary | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $summaryPath -Encoding UTF8
$summary | ConvertTo-Json -Depth 4
