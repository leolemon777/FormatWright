#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Cargo = 'cargo',
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "mixed 10,000 assertion failed: $Message" }
}

function Get-DescendantProcessIds {
    param([int]$RootId)
    $known = [System.Collections.Generic.HashSet[int]]::new()
    $frontier = [System.Collections.Generic.Queue[int]]::new()
    $known.Add($RootId) | Out-Null
    $frontier.Enqueue($RootId)
    while ($frontier.Count -gt 0) {
        $parent = $frontier.Dequeue()
        $children = @(
            Get-CimInstance Win32_Process -Filter "ParentProcessId = $parent" `
                -ErrorAction SilentlyContinue
        )
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
$cargoPath = (Get-Command $Cargo -ErrorAction Stop).Source
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'mixed-10000-suite-' + [Guid]::NewGuid().ToString('N')
)
$fixtureRoot = Join-Path $casePath 'fixtures'
$suiteRoot = Join-Path $casePath 'suite'
New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null
New-Item -ItemType Directory -Path $suiteRoot -Force | Out-Null
$imageFixture = Join-Path $fixtureRoot 'image.png'
$mediaFixture = Join-Path $fixtureRoot 'media.mkv'
$changedImageFixture = Join-Path $fixtureRoot 'changed-image.png'
$changedMediaFixture = Join-Path $fixtureRoot 'changed-media.mkv'
$mediaPackManifest = Join-Path $PSScriptRoot '..\target\release\engine-packs\starter\media\manifest.json'
Assert-True (Test-Path -LiteralPath $mediaPackManifest -PathType Leaf) (
    'Release Starter media pack is missing; run scripts/prepare_windows_starter_pack.ps1 first'
)

& ffmpeg -hide_banner -loglevel error -y `
    -f lavfi -i 'testsrc2=size=640x400:rate=1' -frames:v 1 $imageFixture
Assert-True ($LASTEXITCODE -eq 0) 'unable to generate image fixture'
& ffmpeg -hide_banner -loglevel error -y `
    -f lavfi -i 'color=c=blue:size=640x400:rate=1' -frames:v 1 $changedImageFixture
Assert-True ($LASTEXITCODE -eq 0) 'unable to generate changed image fixture'
& ffmpeg -hide_banner -loglevel error -y `
    -f lavfi -i 'testsrc2=size=640x360:rate=12' `
    -f lavfi -i 'sine=frequency=660:sample_rate=48000' `
    -t 1 -c:v mpeg2video -q:v 12 -c:a mp2 $mediaFixture
Assert-True ($LASTEXITCODE -eq 0) 'unable to generate media fixture'
& ffmpeg -hide_banner -loglevel error -y `
    -f lavfi -i 'color=c=red:size=640x360:rate=12' `
    -f lavfi -i 'sine=frequency=880:sample_rate=48000' `
    -t 1 -c:v mpeg2video -q:v 12 -c:a mp2 $changedMediaFixture
Assert-True ($LASTEXITCODE -eq 0) 'unable to generate changed media fixture'

$stdout = Join-Path $casePath 'release-gate.stdout.log'
$stderr = Join-Path $casePath 'release-gate.stderr.log'
$testName = 'converts_ten_thousand_mixed_files_with_fair_bounded_scheduling'
& $cargoPath test -p formatwright-core --test mixed_ten_thousand_conversions `
    --release --no-run
Assert-True ($LASTEXITCODE -eq 0) 'unable to prebuild the mixed release gate'
$environment = @{
    FORMATWRIGHT_MIXED_SUITE_ROOT = $suiteRoot
    FORMATWRIGHT_MIXED_IMAGE_FIXTURE = $imageFixture
    FORMATWRIGHT_MIXED_MEDIA_FIXTURE = $mediaFixture
    FORMATWRIGHT_MIXED_CHANGED_IMAGE_FIXTURE = $changedImageFixture
    FORMATWRIGHT_MIXED_CHANGED_MEDIA_FIXTURE = $changedMediaFixture
    FORMATWRIGHT_MIXED_MEDIA_PACK_MANIFEST = (Resolve-Path -LiteralPath $mediaPackManifest).Path
}
$process = Start-Process -FilePath $cargoPath -ArgumentList @(
    'test', '-p', 'formatwright-core', '--test', 'mixed_ten_thousand_conversions',
    '--release', '--', '--ignored', '--exact',
    $testName, '--nocapture'
) -Environment $environment -RedirectStandardOutput $stdout -RedirectStandardError $stderr `
    -WindowStyle Hidden -PassThru

$samples = 0
$peakControlPlaneRss = 0L
$peakTreeRss = 0L
$peakWalBytes = 0L
$peakStagedBytes = 0L
$peakStagedCount = 0
do {
    $process.Refresh()
    $harnessIds = @(Get-DescendantProcessIds -RootId $process.Id)
    $testProcess = @($harnessIds | ForEach-Object {
        Get-Process -Id $_ -ErrorAction SilentlyContinue
    } | Where-Object {
        $_.ProcessName -like 'mixed_ten_thousand_conversions-*'
    } | Select-Object -First 1)
    if ($testProcess.Count -eq 1) {
        $peakControlPlaneRss = [Math]::Max(
            $peakControlPlaneRss,
            [long]$testProcess[0].WorkingSet64
        )
        $ids = @(Get-DescendantProcessIds -RootId $testProcess[0].Id)
        $treeRss = 0L
        foreach ($processId in $ids) {
            $observed = Get-Process -Id $processId -ErrorAction SilentlyContinue
            if ($null -ne $observed) { $treeRss += [long]$observed.WorkingSet64 }
        }
        $peakTreeRss = [Math]::Max($peakTreeRss, $treeRss)
    }
    $wal = Join-Path $suiteRoot 'jobs.sqlite3-wal'
    if (Test-Path -LiteralPath $wal -PathType Leaf) {
        $peakWalBytes = [Math]::Max($peakWalBytes, [long](Get-Item -LiteralPath $wal).Length)
    }
    $partials = @(
        Get-ChildItem -LiteralPath (Join-Path $suiteRoot 'output') `
            -Filter '.formatwright-partial-*' `
            -File -ErrorAction SilentlyContinue
    )
    $stagedBytes = 0L
    if ($partials.Count -gt 0) {
        $stagedBytes = [long](($partials | Measure-Object Length -Sum).Sum)
    }
    $peakStagedBytes = [Math]::Max($peakStagedBytes, $stagedBytes)
    $peakStagedCount = [Math]::Max($peakStagedCount, $partials.Count)
    $samples++
    if (-not $process.HasExited) { Start-Sleep -Milliseconds 100 }
} while (-not $process.HasExited)
$process.WaitForExit()
$process.Refresh()
Assert-True ($process.ExitCode -eq 0) (
    "release gate exited $($process.ExitCode): " + (Get-Content -LiteralPath $stderr -Raw)
)

$resultLine = Get-Content -LiteralPath $stdout | Where-Object {
    $_.StartsWith('FORMATWRIGHT_MIXED_10000_RESULT ')
} | Select-Object -Last 1
Assert-True (-not [string]::IsNullOrWhiteSpace($resultLine)) 'Rust result line is missing'
$core = $resultLine.Substring('FORMATWRIGHT_MIXED_10000_RESULT '.Length) | ConvertFrom-Json
Assert-True ($core.jobs -eq 10000 -and $core.completed -eq 10000) 'completion count mismatch'
Assert-True (
    $core.injected_blocked -eq 20 -and $core.resumed_after_repair -eq 20
) 'injected failure/recovery distribution mismatch'
Assert-True ($core.outputs -eq 10000 -and $core.reports -eq 10000) 'artifact reconciliation failed'
Assert-True ($core.maximum_hydrated -le 256) 'hydrated window exceeded 256'
Assert-True ($core.staged_outputs_remaining -eq 0) 'partial outputs remain after the run'
Assert-True ($core.early_window_structured -gt 0) 'structured batch absent from first window'
Assert-True ($core.early_window_image -gt 0) 'image batch absent from first window'
Assert-True ($core.early_window_media -gt 0) 'media batch absent from first window'
$probePath = (Resolve-Path -LiteralPath (
    Join-Path (Split-Path -Parent $mediaPackManifest) 'bin\ffprobe.exe'
)).Path
$probeTargets = @(
    Get-ChildItem -LiteralPath (Join-Path $suiteRoot 'output') -Filter 'image-*.webp' -File
) + @(
    Get-ChildItem -LiteralPath (Join-Path $suiteRoot 'output') -Filter 'media-*.mp4' -File
)
$probeResults = @($probeTargets | ForEach-Object -Parallel {
    $format = & $using:probePath -v error -show_entries format=format_name `
        -of 'default=noprint_wrappers=1:nokey=1' $_.FullName 2>$null
    [pscustomobject]@{
        Path = $_.FullName
        ExitCode = $LASTEXITCODE
        Format = ($format -join '').Trim()
    }
} -ThrottleLimit 4)
Assert-True ($probeResults.Count -eq 400) 'independent image/media probe count mismatch'
Assert-True (
    @($probeResults | Where-Object { $_.ExitCode -ne 0 -or [string]::IsNullOrWhiteSpace($_.Format) }).Count -eq 0
) 'an independent image/media probe failed'
$inputBytes = [long]((
    Get-ChildItem -LiteralPath (Join-Path $suiteRoot 'input') -File |
        Measure-Object Length -Sum
).Sum)
$outputBytes = [long]((
    Get-ChildItem -LiteralPath (Join-Path $suiteRoot 'output') -File |
        Measure-Object Length -Sum
).Sum)
$reportBytes = [long]((
    Get-ChildItem -LiteralPath (Join-Path $suiteRoot 'reports') -File |
        Measure-Object Length -Sum
).Sum)

$result = [ordered]@{
    schema_version = 1
    case_path = $casePath
    core = $core
    sampling_interval_ms = 100
    samples = $samples
    peak_control_plane_rss_bytes = $peakControlPlaneRss
    peak_process_tree_rss_bytes = $peakTreeRss
    peak_wal_bytes = $peakWalBytes
    peak_staged_bytes = $peakStagedBytes
    peak_staged_count = $peakStagedCount
    input_bytes = $inputBytes
    output_bytes = $outputBytes
    report_bytes = $reportBytes
    independent_probe_count = $probeResults.Count
    limitation = '100 ms host polling can miss shorter RSS/WAL/partial peaks; release results are platform and engine-build specific.'
}
$result | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (
    Join-Path $casePath 'mixed-10000-result.json'
) -Encoding utf8
$result | ConvertTo-Json -Depth 10
