#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param(
        [Parameter(Mandatory)]
        [bool]$Condition,
        [Parameter(Mandatory)]
        [string]$Message
    )

    if (-not $Condition) {
        throw "sandbox assertion failed: $Message"
    }
}

function Invoke-FormatWrightJson {
    param(
        [Parameter(Mandatory)]
        [string[]]$Arguments,
        [int[]]$ExpectedExitCodes = @(0)
    )

    $lines = & $script:BinaryPath @Arguments 2>$null
    $exitCode = $LASTEXITCODE
    Assert-True ($ExpectedExitCodes -contains $exitCode) (
        "unexpected exit code $exitCode for: formatwright " + ($Arguments -join ' ')
    )
    $text = $lines -join "`n"
    Assert-True (-not [string]::IsNullOrWhiteSpace($text)) 'JSON command returned empty stdout'
    [pscustomobject]@{
        ExitCode = $exitCode
        Data = $text | ConvertFrom-Json
    }
}

function Invoke-Ffmpeg {
    param([Parameter(Mandatory)][string[]]$Arguments)

    & ffmpeg @Arguments 2>$null
    Assert-True ($LASTEXITCODE -eq 0) ('FFmpeg fixture command failed: ' + ($Arguments -join ' '))
}

function Get-OnlyJob {
    param([Parameter(Mandatory)][string]$Database)

    $response = Invoke-FormatWrightJson -Arguments @(
        '--json', '--state-db', $Database, 'jobs', 'list'
    )
    $jobs = @($response.Data)
    Assert-True ($jobs.Count -eq 1) "expected exactly one job in $Database"
    $jobs[0]
}

function Get-StagedPath {
    param(
        [Parameter(Mandatory)][string]$Output,
        [Parameter(Mandatory)][string]$JobId
    )

    $parent = Split-Path -Parent $Output
    $name = Split-Path -Leaf $Output
    Join-Path $parent ".formatwright-partial-$JobId-$name"
}

function Stop-VerifiedProcessTree {
    param([Parameter(Mandatory)][System.Diagnostics.Process]$Process)

    $Process.Refresh()
    if ($Process.HasExited) {
        return
    }
    $processInfo = Get-CimInstance Win32_Process -Filter "ProcessId = $($Process.Id)"
    Assert-True ($null -ne $processInfo) 'target process disappeared before identity verification'
    Assert-True (
        [IO.Path]::GetFullPath($processInfo.ExecutablePath) -eq [IO.Path]::GetFullPath($script:BinaryPath)
    ) 'refusing to terminate an unexpected process'
    & taskkill.exe /PID $Process.Id /T /F 2>$null | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'targeted process-tree termination failed'
    $Process.WaitForExit()
}

foreach ($tool in @('ffmpeg', 'ffprobe')) {
    Assert-True ($null -ne (Get-Command $tool -ErrorAction SilentlyContinue)) "$tool is required"
}
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path (
    (Resolve-Path -LiteralPath $ArtifactsRoot).Path
) ('sandbox-suite-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $casePath | Out-Null

$crashProcess = $null
try {
    $positiveInput = Join-Path $casePath '输入 sample with spaces.mkv'
    $positiveOutput = Join-Path $casePath 'verified-output.mp4'
    $positiveDb = Join-Path $casePath 'positive.sqlite3'
    Invoke-Ffmpeg @(
        '-v', 'error',
        '-f', 'lavfi', '-i', 'testsrc2=size=320x240:rate=24',
        '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000',
        '-t', '2', '-c:v', 'libx264', '-preset', 'ultrafast', '-c:a', 'aac',
        $positiveInput
    )
    $positiveInputHash = (Get-FileHash -LiteralPath $positiveInput -Algorithm SHA256).Hash
    $plan = Invoke-FormatWrightJson -Arguments @(
        '--json', 'plan', $positiveInput, '--to', 'mp4', '--output', $positiveOutput
    )
    Assert-True ($plan.Data.steps[0].operation -eq 'remux') 'compatible H.264/AAC must remux'
    Assert-True ($plan.Data.steps[0].arguments.video_mode -eq 'copy') 'remux must copy video'
    Assert-True ($plan.Data.steps[0].arguments.audio_mode -eq 'copy') 'remux must copy audio'
    $conversion = Invoke-FormatWrightJson -Arguments @(
        '--json', '--state-db', $positiveDb, 'convert', $positiveInput,
        '--to', 'mp4', '--output', $positiveOutput
    )
    $positiveJob = Get-OnlyJob -Database $positiveDb
    Assert-True ($conversion.Data.status -eq 'pass') 'positive output validation must pass'
    Assert-True ($conversion.Data.job_id -eq $positiveJob.id) 'report and queue must share job ID'
    Assert-True ($positiveJob.state -eq 'completed') 'positive job must complete'
    Assert-True (Test-Path -LiteralPath $positiveOutput -PathType Leaf) 'positive output is missing'
    Assert-True (
        $positiveInputHash -eq (Get-FileHash -LiteralPath $positiveInput -Algorithm SHA256).Hash
    ) 'positive conversion modified the input'
    Assert-True (
        @(Get-ChildItem -LiteralPath $casePath -Filter '.formatwright-partial-*' -File).Count -eq 0
    ) 'positive conversion left a staged output'
    $probeText = & ffprobe -v error -show_entries format=format_name -of json $positiveOutput 2>$null
    Assert-True ($LASTEXITCODE -eq 0) 'independent ffprobe could not open positive output'
    $externalProbe = ($probeText -join "`n") | ConvertFrom-Json
    Assert-True ($externalProbe.format.format_name -match 'mp4') 'independent probe did not detect MP4'

    $conflictOutput = Join-Path $casePath 'existing-output.mp4'
    $conflictDb = Join-Path $casePath 'conflict.sqlite3'
    [IO.File]::WriteAllText($conflictOutput, 'FORMATWRIGHT-SENTINEL')
    $conflictHash = (Get-FileHash -LiteralPath $conflictOutput -Algorithm SHA256).Hash
    $conflict = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
        '--json', '--state-db', $conflictDb, 'convert', $positiveInput,
        '--to', 'mp4', '--output', $conflictOutput
    )
    $conflictJob = Get-OnlyJob -Database $conflictDb
    Assert-True ($conflict.Data.code -eq 'OUTPUT_CONFLICT') 'existing output must be refused'
    Assert-True ($conflictJob.state -eq 'failed') 'output conflict must persist failed state'
    Assert-True (
        $conflictHash -eq (Get-FileHash -LiteralPath $conflictOutput -Algorithm SHA256).Hash
    ) 'existing output was modified'

    $disguisedInput = Join-Path $casePath 'actually-matroska.jpg'
    Copy-Item -LiteralPath $positiveInput -Destination $disguisedInput
    $disguised = Invoke-FormatWrightJson -Arguments @('--json', 'inspect', $disguisedInput)
    Assert-True ($disguised.Data.format.id -eq 'mkv') 'header-first inspection did not detect MKV'
    Assert-True ($disguised.Data.format.extension_matches -eq $false) 'extension mismatch was missed'
    Assert-True (
        @($disguised.Data.warnings.code) -contains 'EXTENSION_MISMATCH'
    ) 'extension mismatch warning was not emitted'

    $subtitleText = Join-Path $casePath 'subtitle.srt'
    $subtitleInput = Join-Path $casePath 'subtitle-source.mkv'
    [IO.File]::WriteAllText(
        $subtitleText,
        "1`r`n00:00:00,000 --> 00:00:01,500`r`nPreserve me`r`n"
    )
    Invoke-Ffmpeg @(
        '-v', 'error', '-i', $positiveInput, '-f', 'srt', '-i', $subtitleText,
        '-map', '0:v', '-map', '0:a', '-map', '1:0', '-c', 'copy', '-c:s', 'srt',
        $subtitleInput
    )
    $subtitlePlan = Invoke-FormatWrightJson -ExpectedExitCodes @(8) -Arguments @(
        '--json', 'plan', $subtitleInput, '--to', 'mp4'
    )
    Assert-True ($subtitlePlan.Data.code -eq 'POLICY_BLOCKED') 'subtitle was silently dropped'

    $longInput = Join-Path $casePath 'long-source.mkv'
    Invoke-Ffmpeg @(
        '-v', 'error',
        '-f', 'lavfi', '-i', 'testsrc2=size=1280x720:rate=30',
        '-f', 'lavfi', '-i', 'sine=frequency=1000:sample_rate=48000',
        '-t', '120', '-c:v', 'mpeg4', '-q:v', '3', '-c:a', 'libmp3lame', '-b:a', '128k',
        $longInput
    )
    $longInputHash = (Get-FileHash -LiteralPath $longInput -Algorithm SHA256).Hash

    $cancelOutput = Join-Path $casePath 'cancelled.mp4'
    $cancelDb = Join-Path $casePath 'cancel.sqlite3'
    $cancel = Invoke-FormatWrightJson -ExpectedExitCodes @(130) -Arguments @(
        '--json', '--state-db', $cancelDb, 'convert', $longInput,
        '--to', 'mp4', '--output', $cancelOutput, '--timeout-seconds', '1'
    )
    $cancelJob = Get-OnlyJob -Database $cancelDb
    Assert-True ($cancel.Data.code -eq 'CANCELLED') 'timeout must produce CANCELLED'
    Assert-True ($cancelJob.state -eq 'cancelled') 'cancelled state was not persisted'
    Assert-True (-not (Test-Path -LiteralPath $cancelOutput)) 'cancelled output was committed'
    Assert-True (
        -not (Test-Path -LiteralPath (Get-StagedPath $cancelOutput $cancelJob.id))
    ) 'cancelled job left a staged output'

    $crashOutput = Join-Path $casePath 'crash-output.mp4'
    $crashDb = Join-Path $casePath 'crash.sqlite3'
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:BinaryPath
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in @(
        '--json', '--state-db', $crashDb, 'convert', $longInput,
        '--to', 'mp4', '--output', $crashOutput
    )) {
        [void]$startInfo.ArgumentList.Add($argument)
    }
    $crashProcess = [Diagnostics.Process]::new()
    $crashProcess.StartInfo = $startInfo
    Assert-True $crashProcess.Start() 'failed to start crash-injection process'

    $crashJob = $null
    $crashPartial = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 250
        if (Test-Path -LiteralPath $crashDb) {
            $jobLines = & $script:BinaryPath --json --state-db $crashDb jobs list 2>$null
            if ($LASTEXITCODE -eq 0 -and $jobLines) {
                $jobs = @(($jobLines -join "`n") | ConvertFrom-Json)
                if ($jobs.Count -eq 1 -and $jobs[0].state -eq 'running') {
                    $crashJob = $jobs[0]
                    $crashPartial = Get-StagedPath $crashOutput $crashJob.id
                }
            }
        }
        $crashProcess.Refresh()
    } until (
        ($null -ne $crashJob -and (Test-Path -LiteralPath $crashPartial) -and
            (Get-Item -LiteralPath $crashPartial).Length -gt 0) -or
        $crashProcess.HasExited -or [DateTime]::UtcNow -ge $deadline
    )
    Assert-True (-not $crashProcess.HasExited) 'conversion exited before crash injection'
    Assert-True ($null -ne $crashJob) 'crash job never reached running state'
    Assert-True (Test-Path -LiteralPath $crashPartial) 'crash job never created a staged output'
    Stop-VerifiedProcessTree -Process $crashProcess
    Start-Sleep -Milliseconds 500
    Assert-True (-not (Test-Path -LiteralPath $crashOutput)) 'crashed job committed an output'
    Assert-True (Test-Path -LiteralPath $crashPartial) 'crash did not leave recovery evidence'

    $recovery = Invoke-FormatWrightJson -Arguments @(
        '--json', '--state-db', $crashDb, 'jobs', 'recover'
    )
    $details = Invoke-FormatWrightJson -Arguments @(
        '--json', '--state-db', $crashDb, 'jobs', 'show', $crashJob.id
    )
    Assert-True (@($recovery.Data.interrupted_jobs).Count -eq 1) 'recovery count is wrong'
    Assert-True (@($recovery.Data.removed_staged_outputs).Count -eq 1) 'staged cleanup count is wrong'
    Assert-True ($details.Data.job.state -eq 'interrupted') 'crashed job was not interrupted'
    Assert-True (
        @($details.Data.events.code) -contains 'RECOVERED_AFTER_RESTART'
    ) 'recovery event is missing'
    Assert-True (-not (Test-Path -LiteralPath $crashPartial)) 'recovery left the staged output'
    Assert-True (-not (Test-Path -LiteralPath $crashOutput)) 'recovery committed a target'
    $resumed = Invoke-FormatWrightJson -Arguments @(
        '--json', '--state-db', $crashDb, 'jobs', 'resume', $crashJob.id
    )
    Assert-True ($resumed.Data.state -eq 'queued') 'resume did not requeue interrupted job'
    $cancelledQueued = Invoke-FormatWrightJson -Arguments @(
        '--json', '--state-db', $crashDb, 'jobs', 'cancel', $crashJob.id
    )
    Assert-True ($cancelledQueued.Data.state -eq 'cancelled') 'queued cancellation did not persist'
    $retried = Invoke-FormatWrightJson -Arguments @(
        '--json', '--state-db', $crashDb, 'jobs', 'retry', $crashJob.id
    )
    Assert-True ($retried.Data.state -eq 'queued') 'retry did not requeue cancelled job'
    $actionDetails = Invoke-FormatWrightJson -Arguments @(
        '--json', '--state-db', $crashDb, 'jobs', 'show', $crashJob.id
    )
    foreach ($eventCode in @('JOB_RESUMED', 'USER_CANCELLED', 'JOB_RETRIED')) {
        Assert-True (@($actionDetails.Data.events.code) -contains $eventCode) "missing $eventCode event"
    }
    Assert-True (
        $longInputHash -eq (Get-FileHash -LiteralPath $longInput -Algorithm SHA256).Hash
    ) 'cancel/crash testing modified the source'

    $summary = [ordered]@{
        schema_version = 1
        case_path = $casePath
        generated_utc = [DateTime]::UtcNow.ToString('o')
        binary_sha256 = (Get-FileHash -LiteralPath $script:BinaryPath -Algorithm SHA256).Hash
        positive_remux = [ordered]@{
            status = 'pass'
            job_id = $positiveJob.id
            validation_status = $conversion.Data.status
            independent_format = $externalProbe.format.format_name
        }
        output_conflict = [ordered]@{
            status = 'pass'
            error_code = $conflict.Data.code
            existing_output_unchanged = $true
        }
        wrong_extension = [ordered]@{
            status = 'pass'
            detected_format = $disguised.Data.format.id
            warning = 'EXTENSION_MISMATCH'
        }
        subtitle_preservation = [ordered]@{
            status = 'pass'
            error_code = $subtitlePlan.Data.code
        }
        cancellation = [ordered]@{
            status = 'pass'
            job_id = $cancelJob.id
            final_state = $cancelJob.state
        }
        crash_recovery = [ordered]@{
            status = 'pass'
            job_id = $crashJob.id
            recovered_state = $details.Data.job.state
            final_state = $actionDetails.Data.job.state
            staged_outputs_removed = @($recovery.Data.removed_staged_outputs).Count
            actions = @('JOB_RESUMED', 'USER_CANCELLED', 'JOB_RETRIED')
        }
    }
    $summaryPath = Join-Path $casePath 'summary.json'
    $summaryJson = $summary | ConvertTo-Json -Depth 8
    [IO.File]::WriteAllText($summaryPath, $summaryJson + [Environment]::NewLine)
    $summaryJson
}
finally {
    if ($null -ne $crashProcess) {
        $crashProcess.Refresh()
        if (-not $crashProcess.HasExited) {
            Stop-VerifiedProcessTree -Process $crashProcess
        }
        $crashProcess.Dispose()
    }
}
