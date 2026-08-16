#requires -Version 7.0

# Batch-E resource-failure gates. Injects disk-full, write-permission loss,
# and target-volume removal into real CLI conversions and asserts the
# failure contract: a typed non-zero exit, no committed output, no staged
# partial left behind, and no fake success in the job store.
#
# Scenarios A and C create/attach a small VHD via diskpart. All diskpart
# commands select the vdisk BY FILE PATH only - no disk number is ever
# selected - so the script cannot touch a real volume.
[CmdletBinding()]
param(
    [string]$Binary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts\resource-failure-injection'),
    [switch]$SkipVhd # run only the permission-loss scenario (no elevation needed)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "resource failure injection failed: $Message" }
}

function Invoke-Cli {
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
    Assert-True $process.Start() 'unable to start FormatWright'
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    [pscustomobject]@{
        ExitCode = $process.ExitCode
        Stdout = $stdoutTask.GetAwaiter().GetResult()
        Stderr = $stderrTask.GetAwaiter().GetResult()
    }
}

function Assert-FailureContract {
    param(
        [pscustomobject]$Result,
        [string]$OutputPath,
        [string]$StagingParent,
        [string]$DatabasePath,
        [string]$Scenario
    )
    Assert-True ($Result.ExitCode -ne 0) "$Scenario unexpectedly succeeded (exit 0)"
    # 2 (INPUT_INVALID) is correct for paths on vanished volumes; the
    # contract under test is typed failure + no committed/partial output.
    Assert-True ($Result.ExitCode -in @(1, 2, 4, 5, 8)) (
        "$Scenario produced unclassified exit code $($Result.ExitCode): $($Result.Stderr)"
    )
    Assert-True (-not (Test-Path -LiteralPath $OutputPath)) "$Scenario committed an output file"
    $staged = @(Get-ChildItem -LiteralPath $StagingParent -Filter '.formatwright-partial-*' -Force -ErrorAction SilentlyContinue)
    Assert-True ($staged.Count -eq 0) "$Scenario left a staged partial behind"
    if ($null -ne $DatabasePath) {
        $jobs = Invoke-Cli @('--json', '--state-db', $DatabasePath, 'jobs', 'list')
        Assert-True ($jobs.ExitCode -eq 0) "$Scenario could not list jobs afterwards"
        $records = @($jobs.Stdout | ConvertFrom-Json)
        if ($records.Count -eq 0) {
            # Validation-stage rejections never persist a job; that is the
            # honest outcome for inputs like paths on vanished volumes.
            return
        }
        Assert-True ($records.Count -eq 1) "$Scenario expected at most one job record"
        Assert-True ($records[0].state -in @('failed', 'interrupted')) (
            "$Scenario job state is '$($records[0].state)' - a failed run must never be 'completed'"
        )
    }
}

function Get-JobState {
    param([string]$DatabasePath)
    $jobs = Invoke-Cli @('--json', '--state-db', $DatabasePath, 'jobs', 'list')
    $records = @($jobs.Stdout | ConvertFrom-Json)
    if ($records.Count -eq 0) { return 'not-persisted' }
    return $records[0].state
}

function Invoke-Diskpart {
    param([string[]]$Commands, [string]$Label)
    $script_file = Join-Path $script:CasePath ("diskpart-$Label.txt")
    $Commands | Set-Content -LiteralPath $script_file -Encoding ASCII
    $output = & diskpart.exe /s $script_file 2>&1
    return ($output -join "`n")
}

function New-TestVhd {
    param([string]$VhdPath, [int64]$SizeMB, [string]$Letter)
    $out = Invoke-Diskpart -Label 'create' -Commands @(
        "create vdisk file=`"$VhdPath`" type=fixed maximum=$SizeMB",
        "select vdisk file=`"$VhdPath`"",
        'attach vdisk',
        'create partition primary',
        'format fs=ntfs label=FWFAILTEST quick',
        "assign letter=$Letter"
    )
    Assert-True ($LASTEXITCODE -eq 0) "diskpart create failed: $out"
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not (Test-Path "$($Letter):\") -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 500
    }
    Assert-True (Test-Path "$($Letter):\") "vhdid volume $Letter`: never became available"
}

function Remove-TestVhd {
    param([string]$VhdPath, [string]$VolumeLetter)
    # Detach first, then delete; some diskpart builds reject `delete vdisk`
    # in the same script as the detach, so file removal is the fallback.
    foreach ($attempt in 1..3) {
        Invoke-Diskpart -Label ("detach-$attempt") -Commands @(
            "select vdisk file=`"$VhdPath`"",
            'detach vdisk'
        ) | Out-Null
        if ($null -ne $VolumeLetter -and (Test-Path "$($VolumeLetter):\")) {
            Start-Sleep -Seconds 2
            continue
        }
        break
    }
    if ($null -ne $VolumeLetter) {
        Assert-True (-not (Test-Path "$($VolumeLetter):\")) (
            "test volume $VolumeLetter`: never detached; refusing to continue"
        )
    }
    if (-not (Test-Path -LiteralPath $VhdPath)) { return }
    foreach ($attempt in 1..3) {
        Invoke-Diskpart -Label ("delete-$attempt") -Commands @(
            "select vdisk file=`"$VhdPath`"",
            'delete vdisk'
        ) | Out-Null
        if (-not (Test-Path -LiteralPath $VhdPath)) { return }
        Start-Sleep -Seconds 2
    }
    # Detached vdisks are plain files: direct removal is the safe fallback.
    Remove-Item -LiteralPath $VhdPath -Force -ErrorAction SilentlyContinue
    Assert-True (-not (Test-Path -LiteralPath $VhdPath)) (
        "could not remove the detached test vhd file $VhdPath"
    )
}

Assert-True ($IsWindows) 'this harness requires Windows'
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
$elevated = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Assert-True ($elevated -or $SkipVhd) 'scenarios A/C require elevation; rerun elevated or pass -SkipVhd'
$script:BinaryPath = (Resolve-Path -LiteralPath $Binary).Path
Assert-True ($null -ne (Get-Command ffmpeg -ErrorAction SilentlyContinue)) 'ffmpeg is required'
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$script:CasePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) ('suite-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $script:CasePath | Out-Null

# Shared input: a small remuxable MKV whose MP4 output needs ~ the same space.
$inputPath = Join-Path $script:CasePath 'input.mkv'
& ffmpeg -v error -f lavfi -i 'testsrc2=size=640x480:rate=24' `
    -f lavfi -i 'sine=frequency=440:sample_rate=48000' -t 12 `
    -c:v libx264 -preset ultrafast -b:v 6M -c:a aac $inputPath 2>$null
Assert-True ($LASTEXITCODE -eq 0) 'failed to generate the input media'
$inputBytes = (Get-Item -LiteralPath $inputPath).Length
$inputHash = (Get-FileHash -LiteralPath $inputPath -Algorithm SHA256).Hash
Assert-True ($inputBytes -gt 4MB) 'input media is unexpectedly small'

$results = [ordered]@{ schema_version = 1; case_path = $script:CasePath; input_bytes = $inputBytes; scenarios = @() }

# ---------------------------------------------------------------------------
# Scenario B (no elevation needed): write permission denied on output dir.
# ---------------------------------------------------------------------------
{
    $scenario = 'permission-denied'
    $dir = Join-Path $script:CasePath 'protected-output'
    New-Item -ItemType Directory -Path $dir | Out-Null
    $outputPath = Join-Path $dir 'out.mp4'
    $databasePath = Join-Path $script:CasePath 'jobs-permission.sqlite3'
    $deny = & icacls $dir /deny "$($env:USERNAME):(WD,AD)" 2>&1
    Assert-True ($LASTEXITCODE -eq 0) "icacls deny failed: $($deny -join ' ')"
    try {
        $result = Invoke-Cli @(
            '--json', '--state-db', $databasePath, 'convert', $inputPath,
            '--to', 'mp4', '--output', $outputPath
        )
        Assert-FailureContract -Result $result -OutputPath $outputPath `
            -StagingParent $dir -DatabasePath $databasePath -Scenario $scenario
        $results.scenarios += [ordered]@{
            scenario = $scenario
            exit_code = $result.ExitCode
            job_state = Get-JobState -DatabasePath $databasePath
            stderr_excerpt = ($result.Stderr -split "`n" | Select-Object -First 3) -join ' | '
        }
    }
    finally {
        & icacls $dir /remove:d "$($env:USERNAME)" 2>&1 | Out-Null
    }
    Assert-True ((Get-FileHash -LiteralPath $inputPath -Algorithm SHA256).Hash -eq $inputHash) 'input changed during permission failure'
}.Invoke()

# ---------------------------------------------------------------------------
# Scenarios A and C: a dedicated throwaway VHD volume.
# ---------------------------------------------------------------------------
if (-not $SkipVhd) {
    $letter = 'Y'
    foreach ($candidate in 'Y','X','W','V','U') {
        if (-not (Test-Path "$($candidate):\")) { $letter = $candidate; break }
    }
    Assert-True (-not (Test-Path "$($letter):\")) "no free drive letter for the test vhd"
    $vhdPath = Join-Path $script:CasePath "failvolume-$letter.vhd"
    New-TestVhd -VhdPath $vhdPath -SizeMB 96 -Letter $letter
    try {
        # Control conversion on the fresh volume proves the path works.
        $controlOutput = "$($letter):\control.mp4"
        $control = Invoke-Cli @(
            '--json', 'convert', $inputPath, '--to', 'mp4', '--output', $controlOutput
        )
        Assert-True ($control.ExitCode -eq 0) "control conversion failed: $($control.Stderr)"
        Assert-True (Test-Path -LiteralPath $controlOutput) 'control output missing'
        Remove-Item -LiteralPath $controlOutput -Force

        # --- Scenario A: fill the volume to ~1 MB free, then convert. ---
        {
            $scenario = 'disk-full'
            $outputPath = "$($letter):\full.mp4"
            $databasePath = Join-Path $script:CasePath 'jobs-diskfull.sqlite3'
            $fillerPath = "$($letter):\filler.bin"
            for ($attempt = 0; $attempt -lt 6; $attempt++) {
                $free = (Get-PSDrive -Name $letter).Free
                if ($free -le 1MB) { break }
                $target = $free - 500KB
                if ($target -gt 0) {
                    & fsutil.exe file createnew $fillerPath ([int64]$target) 2>$null | Out-Null
                    if ($LASTEXITCODE -ne 0) {
                        # Retry smaller if NTFS could not allocate the exact amount.
                        & fsutil.exe file createnew "$($letter):\filler-$attempt.bin" ([int64]($target / 2)) 2>$null | Out-Null
                    }
                }
            }
            $free = (Get-PSDrive -Name $letter).Free
            Assert-True ($free -lt $inputBytes) (
                "could not squeeze the volume below the needed output size (free=$free needed>$inputBytes)"
            )
            $result = Invoke-Cli @(
                '--json', '--state-db', $databasePath, 'convert', $inputPath,
                '--to', 'mp4', '--output', $outputPath
            )
            Assert-FailureContract -Result $result -OutputPath $outputPath `
                -StagingParent "$($letter):\" -DatabasePath $DatabasePath -Scenario $scenario
            $results.scenarios += [ordered]@{
                scenario = $scenario
                free_bytes_before = $free
                exit_code = $result.ExitCode
                job_state = Get-JobState -DatabasePath $databasePath
                stderr_excerpt = ($result.Stderr -split "`n" | Select-Object -First 3) -join ' | '
            }
        }.Invoke()

        # --- Scenario C: remove the target volume between runs. ---
        {
            $scenario = 'volume-removed'
            $outputPath = "$($letter):\vanished.mp4"
            $databasePath = Join-Path $script:CasePath 'jobs-volume.sqlite3'
            Remove-TestVhd -VhdPath $vhdPath -VolumeLetter $letter
            Assert-True (-not (Test-Path "$($letter):\")) 'test volume still present after detach'
            $result = Invoke-Cli @(
                '--json', '--state-db', $databasePath, 'convert', $inputPath,
                '--to', 'mp4', '--output', $outputPath
            )
            Assert-FailureContract -Result $result -OutputPath $outputPath `
                -StagingParent $script:CasePath -DatabasePath $databasePath -Scenario $scenario
            $results.scenarios += [ordered]@{
                scenario = $scenario
                exit_code = $result.ExitCode
                job_state = Get-JobState -DatabasePath $databasePath
                stderr_excerpt = ($result.Stderr -split "`n" | Select-Object -First 3) -join ' | '
            }
        }.Invoke()
    }
    finally {
        if (Test-Path -LiteralPath $vhdPath) {
            Remove-TestVhd -VhdPath $vhdPath -VolumeLetter $letter
        }
    }
}

Assert-True ((Get-FileHash -LiteralPath $inputPath -Algorithm SHA256).Hash -eq $inputHash) 'input file changed during any scenario'
$results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $script:CasePath 'summary.json') -Encoding UTF8
$results | ConvertTo-Json -Depth 4
