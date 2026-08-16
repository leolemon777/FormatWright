#requires -Version 7.0

# Batch D automation: run INSIDE a clean, offline Windows 11 VM that has no
# FFmpeg/Poppler/LibreOffice/Pandoc/libvips and no FormatWright development
# caches. Proves out-of-box installed conversions from the real UI with a
# deliberately polluted PATH, then verifies uninstall leaves nothing behind.
# See docs/testing/CLEAN_VM_CERTIFICATION.md for the full runbook, including
# the manual steps (cancel, restart recovery, upgrade) this suite does not
# automate.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Installer,
    # Release desktop binary built with src-tauri/tauri.release-e2e.conf.json
    # (DevTools port 9338) used for the automated UI conversions. The standard
    # installed application must never carry this overlay.
    [Parameter(Mandatory = $true)]
    [string]$E2EBinary,
    [Parameter(Mandatory = $true)]
    [string]$SourcePdf,
    [string]$InstallRoot = "$env:LOCALAPPDATA\Programs\FormatWright",
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts\clean-vm-certification')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "clean VM certification failed: $Message" }
}

$forbiddenTools = @('ffmpeg', 'ffprobe', 'pdftoppm', 'pdfinfo', 'soffice', 'pandoc', 'vips')
foreach ($tool in $forbiddenTools) {
    Assert-True ($null -eq (Get-Command $tool -ErrorAction SilentlyContinue)) (
        "VM is not clean: '$tool' is on PATH; rebuild the VM without conversion tools"
    )
}
Assert-True (-not (Test-Path "$env:APPDATA\local.formatwright.desktop")) 'user app-state already exists; VM is not clean'
Assert-True (-not (Test-Path "$env:LOCALAPPDATA\local.formatwright.desktop")) 'user local app-state already exists; VM is not clean'
Assert-True (-not (Test-Path $InstallRoot)) 'install root already exists; VM is not clean'

$installerPath = (Resolve-Path -LiteralPath $Installer).Path
$e2ePath = (Resolve-Path -LiteralPath $E2EBinary).Path
$pdfPath = (Resolve-Path -LiteralPath $SourcePdf).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) ('vm-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $casePath | Out-Null
$pollutedBin = Join-Path $casePath 'polluted-bin'
New-Item -ItemType Directory -Path $pollutedBin | Out-Null
# Negative probes: hostile wrappers that must never be selected in Release.
foreach ($tool in @('pdftoppm', 'pdfinfo', 'ffmpeg')) {
    Set-Content -LiteralPath (Join-Path $pollutedBin "$tool.cmd") -Value "@echo off`r`nexit /b 1`r`n"
}

$installed = $false
try {
    # 1. Silent current-user install with a polluted PATH visible to children.
    $env:PATH = "$pollutedBin;$env:PATH"
    $installClock = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $installerPath -ArgumentList @('/S') -PassThru -Wait
    $installClock.Stop()
    Assert-True ($process.ExitCode -eq 0) "installer exited $($process.ExitCode)"
    $installed = $true
    Assert-True (Test-Path -LiteralPath (Join-Path $InstallRoot 'formatwright-desktop.exe')) 'installed executable is missing'
    $installedExe = (Resolve-Path -LiteralPath (Join-Path $InstallRoot 'formatwright-desktop.exe')).Path
    $installedBytes = (Get-Item -LiteralPath $installedExe).Length
    Assert-True ((Get-Content -LiteralPath $installedExe -AsByteStream -TotalCount 200MB -ReadCount 0) -is [byte[]]) 'installed binary unreadable'
    # The standard build must not embed the release-e2e DevTools argument.
    $embeddedArgs = Select-String -LiteralPath $installedExe -Pattern 'remote-debugging-port' -SimpleMatch -Quiet
    Assert-True (-not $embeddedArgs) 'standard installer embeds the test DevTools argument'

    # 2. First launch of the INSTALLED app installs engine packs from embedded
    #    resources even with the polluted PATH, then exits cleanly.
    $app = Start-Process -FilePath $installedExe -PassThru
    $engineStore = "$env:LOCALAPPDATA\local.formatwright.desktop\engines"
    $deadline = [DateTime]::UtcNow.AddSeconds(180)
    do {
        Start-Sleep -Seconds 2
        $packs = @(Get-ChildItem -LiteralPath $engineStore -Directory -ErrorAction SilentlyContinue)
    } while ($packs.Count -lt 2 -and [DateTime]::UtcNow -lt $deadline)
    Assert-True ($packs.Count -ge 2) "first launch did not install the starter packs (found $($packs.Count))"
    Start-Sleep -Seconds 5
    Assert-True (-not $app.HasExited) 'installed app exited during observation'
    Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
    $app.WaitForExit(10000) | Out-Null
    Get-Process -Name 'formatwright-desktop' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500

    # 3. Real UI conversions from the e2e-overlay binary (isolated per format).
    & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'test_desktop_release_conversion.ps1') `
        -SourcePdf $pdfPath -DesktopBinary $e2ePath `
        -ArtifactsRoot (Join-Path $casePath 'ui-conversions') `
        2>&1 | Tee-Object -FilePath (Join-Path $casePath 'ui-conversions.log') | Out-Null
    Assert-True ($LASTEXITCODE -eq 0) 'release UI conversion gate failed inside the VM'

    $summary = [ordered]@{
        schema_version = 1
        case_path = $casePath
        generated_utc = [DateTime]::UtcNow.ToString('o')
        installer = $installerPath
        installer_sha256 = (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash
        install_elapsed_ms = $installClock.ElapsedMilliseconds
        installed_exe_bytes = $installedBytes
        polluted_path_probes = $forbiddenTools
        installed_engine_packs = @($packs.Name)
        ui_conversion_gate = 'pass'
        host = $env:COMPUTERNAME
    }
    $summary | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $casePath 'clean-vm-summary.json') -Encoding UTF8
    $summary | ConvertTo-Json -Depth 3
}
finally {
    if ($installed -and (Test-Path -LiteralPath (Join-Path $InstallRoot 'uninstall.exe'))) {
        $uninstall = Start-Process -FilePath (Join-Path $InstallRoot 'uninstall.exe') -ArgumentList @('/S') -PassThru -Wait
        Start-Sleep -Seconds 3
        Assert-True (-not (Test-Path -LiteralPath $InstallRoot)) 'uninstall left the install root behind'
        Assert-True (-not (Test-Path "$env:APPDATA\local.formatwright.desktop")) 'uninstall left user app-state behind'
        Assert-True (-not (Test-Path "$env:LOCALAPPDATA\local.formatwright.desktop")) 'uninstall left user local app-state behind'
        $shellKeys = @(
            'HKCU:\Software\Classes\*\shell\FormatWrightConvert',
            'HKCU:\Software\Classes\Directory\shell\FormatWrightConvert'
        )
        foreach ($key in $shellKeys) {
            Assert-True (-not (Test-Path $key)) "uninstall left owned shell key: $key"
        }
    }
    Get-Process -Name 'formatwright-desktop' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}
