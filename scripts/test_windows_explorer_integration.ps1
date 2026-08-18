#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Installer = (Join-Path $PSScriptRoot '..\target\release\bundle\nsis\FormatWright_0.1.0_x64-setup.exe'),
    [string]$CliBinary = (Join-Path $PSScriptRoot '..\target\debug\formatwright.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts\windows-explorer-installed-smoke')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "Windows Explorer integration assertion failed: $Message" }
}

function Get-TreeDigest {
    param([string]$Root)
    if (-not (Test-Path -LiteralPath $Root)) { return @() }
    @(
        Get-ChildItem -LiteralPath $Root -File -Recurse -Force | ForEach-Object {
            [pscustomobject]@{
                Relative = $_.FullName.Substring($Root.Length).TrimStart('\')
                Length = $_.Length
                SHA256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            }
        } | Sort-Object Relative
    )
}

function Assert-TreeDigestEqual {
    param([object[]]$Expected, [object[]]$Actual, [string]$Message)
    $expectedJson = ConvertTo-Json -InputObject @($Expected) -Depth 4 -Compress
    $actualJson = ConvertTo-Json -InputObject @($Actual) -Depth 4 -Compress
    Assert-True ($expectedJson -ceq $actualJson) $Message
}

function Remove-CheckedTree {
    param([string]$Target, [string]$AllowedParent)
    if (-not (Test-Path -LiteralPath $Target)) { return }
    $resolvedTarget = [IO.Path]::GetFullPath($Target).TrimEnd('\')
    $resolvedParent = [IO.Path]::GetFullPath($AllowedParent).TrimEnd('\')
    Assert-True (
        $resolvedTarget.StartsWith($resolvedParent + '\', [StringComparison]::OrdinalIgnoreCase)
    ) "refusing to remove a path outside $resolvedParent"
    Remove-Item -LiteralPath $resolvedTarget -Recurse -Force
}

function Get-FormatWrightProcesses {
    @(
        Get-Process -ErrorAction SilentlyContinue | Where-Object {
            $_.ProcessName -eq 'formatwright-desktop'
        }
    )
}

function Wait-ForSingleProcess {
    param([datetime]$Deadline)
    do {
        $processes = @(Get-FormatWrightProcesses)
        if ($processes.Count -eq 1) { return $processes[0] }
        Start-Sleep -Milliseconds 100
    } until ([DateTime]::UtcNow -ge $Deadline)
    throw "expected exactly one FormatWright process, observed $($processes.Count)"
}

function Get-WindowAutomation {
    param([int]$ProcessId)
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        $root = [System.Windows.Automation.AutomationElement]::RootElement
        $processCondition = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::ProcessIdProperty,
            $ProcessId
        )
        $nameCondition = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::NameProperty,
            'FormatWright'
        )
        $condition = New-Object System.Windows.Automation.AndCondition(
            $processCondition,
            $nameCondition
        )
        $window = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $condition)
        if ($null -ne $window) { return $window }
        Start-Sleep -Milliseconds 100
    } until ([DateTime]::UtcNow -ge $deadline)
    throw 'FormatWright window did not appear in UI Automation'
}

function Get-EditableValues {
    param([System.Windows.Automation.AutomationElement]$Window)
    $condition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::IsValuePatternAvailableProperty,
        $true
    )
    @(
        $Window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition) |
            ForEach-Object {
                try {
                    $pattern = $_.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
                    [string]$pattern.Current.Value
                } catch { $null }
            } | Where-Object { $null -ne $_ }
    )
}

function Wait-ForEditableValue {
    param(
        [System.Windows.Automation.AutomationElement]$Window,
        [string]$Expected,
        [datetime]$Deadline
    )
    do {
        $values = @(Get-EditableValues -Window $Window)
        if ($values -contains $Expected) { return $values }
        Start-Sleep -Milliseconds 100
    } until ([DateTime]::UtcNow -ge $Deadline)
    throw "UI Automation never exposed expected value: $Expected"
}

function Start-ShellOpen {
    param([string]$Executable, [string]$Path)
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable
    $start.UseShellExecute = $false
    $start.ArgumentList.Add('--shell-open')
    $start.ArgumentList.Add($Path)
    [Diagnostics.Process]::Start($start)
}

function Start-ExplorerVerb {
    param([string]$Path)
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Path
    $start.Verb = 'FormatWright'
    $start.UseShellExecute = $true
    [Diagnostics.Process]::Start($start)
}

function Start-ShellConvert {
    param([string]$Executable, [string]$Target, [string]$Path)
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable
    $start.UseShellExecute = $false
    $start.ArgumentList.Add('--shell-convert')
    $start.ArgumentList.Add('--to')
    $start.ArgumentList.Add($Target)
    $start.ArgumentList.Add($Path)
    [Diagnostics.Process]::Start($start)
}

function Get-ExplorerVerbTable {
    $tablePath = Join-Path $PSScriptRoot '..\apps\desktop\src-tauri\explorer-verbs.json'
    Get-Content -LiteralPath $tablePath -Raw -Encoding utf8 | ConvertFrom-Json
}

function Get-OwnedConvertKeys {
    param($Table)
    @(
        foreach ($item in $Table.convert) {
            "Registry::HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\$($item.assoc)\shell\$($item.verb)"
        }
    )
}

function Write-TinyPdf {
    param([string]$Path)
    $pdf = @"
%PDF-1.1
1 0 obj<< /Type /Catalog /Pages 2 0 R >>endobj
2 0 obj<< /Type /Pages /Kids [3 0 R] /Count 1 >>endobj
3 0 obj<< /Type /Page /Parent 2 0 R /MediaBox [0 0 72 72] >>endobj
xref
0 4
0000000000 65535 f 
0000000009 00000 n 
0000000058 00000 n 
0000000115 00000 n 
trailer<< /Size 4 /Root 1 0 R >>
startxref
190
%%EOF
"@
    [IO.File]::WriteAllText($Path, $pdf)
}

Assert-True (@(Get-FormatWrightProcesses).Count -eq 0) 'FormatWright is already running'
$installerPath = (Resolve-Path -LiteralPath $Installer).Path
$cliPath = (Resolve-Path -LiteralPath $CliBinary).Path
New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) (
    'suite-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $casePath -Force | Out-Null
$installRoot = Join-Path $casePath 'install'
$fixtureRoot = Join-Path $casePath 'fixtures 空格'
New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null
$fileFixture = Join-Path $fixtureRoot '名字 with spaces.json'
Set-Content -LiteralPath $fileFixture -Value '{"formatwright":true}' -Encoding utf8

$fileKey = 'Registry::HKEY_CURRENT_USER\Software\Classes\*\shell\FormatWright'
$directoryKey = 'Registry::HKEY_CURRENT_USER\Software\Classes\Directory\shell\FormatWright'
$siblingKey = 'Registry::HKEY_CURRENT_USER\Software\Classes\*\shell\FormatWrightSiblingSmoke'
$stateRoots = @(
    (Join-Path $env:APPDATA 'local.formatwright.desktop'),
    (Join-Path $env:LOCALAPPDATA 'local.formatwright.desktop')
)
$stateBefore = @{}
$isolatedState = @{}
foreach ($root in $stateRoots) {
    $stateBefore[$root] = @(Get-TreeDigest -Root $root)
}
$installed = $false
$siblingCreated = $false
$app = $null
$uninstallResult = $null
$stateIsolated = $false

try {
    Assert-True (-not (Test-Path -LiteralPath $fileKey)) 'file verb already exists'
    Assert-True (-not (Test-Path -LiteralPath $directoryKey)) 'directory verb already exists'

    $stateIsolated = $true
    foreach ($root in $stateRoots) {
        if (Test-Path -LiteralPath $root) {
            $isolated = $root + '.formatwright-shell-smoke-' + [Guid]::NewGuid().ToString('N')
            Move-Item -LiteralPath $root -Destination $isolated
            $isolatedState[$root] = $isolated
        }
    }

    $installerProcess = Start-Process -FilePath $installerPath -ArgumentList @(
        '/S', ('/D=' + $installRoot)
    ) -Wait -PassThru -WindowStyle Hidden
    Assert-True ($installerProcess.ExitCode -eq 0) 'installer returned non-zero'
    $installed = $true

    $executable = Join-Path $installRoot 'formatwright-desktop.exe'
    $uninstaller = Join-Path $installRoot 'uninstall.exe'
    Assert-True (Test-Path -LiteralPath $executable -PathType Leaf) 'installed executable missing'
    Assert-True (Test-Path -LiteralPath $uninstaller -PathType Leaf) 'uninstaller missing'
    $expectedCommand = '"' + $executable + '" --shell-open "%1"'
    $fileCommand = Get-ItemPropertyValue -LiteralPath ($fileKey + '\command') -Name '(default)'
    $directoryCommand = Get-ItemPropertyValue -LiteralPath ($directoryKey + '\command') -Name '(default)'
    Assert-True ($fileCommand -ceq $expectedCommand) "file command quoting is invalid: $fileCommand"
    Assert-True ($directoryCommand -ceq $expectedCommand) "directory command quoting is invalid: $directoryCommand"

    $verbTable = Get-ExplorerVerbTable
    Assert-True ($verbTable.convert.Count -eq 17) 'verb table does not contain 17 convert entries'
    $convertKeys = @(Get-OwnedConvertKeys -Table $verbTable)
    foreach ($item in $verbTable.convert) {
        $convertKey = "Registry::HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\$($item.assoc)\shell\$($item.verb)"
        Assert-True (Test-Path -LiteralPath $convertKey) "missing convert key: $convertKey"
        $convertCommand = Get-ItemPropertyValue -LiteralPath ($convertKey + '\command') -Name '(default)'
        $expectedConvert = '"' + $executable + '" --shell-convert --to ' + $item.target + ' "%1"'
        Assert-True ($convertCommand -ceq $expectedConvert) "convert command quoting is invalid: $convertCommand"
    }

    New-Item -Path $siblingKey -Force | Out-Null
    New-ItemProperty -LiteralPath $siblingKey -Name '(default)' -Value 'preserve-me' -Force | Out-Null
    $siblingCreated = $true

    $app = Start-ExplorerVerb -Path $fileFixture
    $process = Wait-ForSingleProcess -Deadline ([DateTime]::UtcNow.AddSeconds(30))
    $window = Get-WindowAutomation -ProcessId $process.Id
    $fileValues = Wait-ForEditableValue -Window $window -Expected $fileFixture -Deadline (
        [DateTime]::UtcNow.AddSeconds(30)
    )
    $database = Join-Path $env:APPDATA 'local.formatwright.desktop\jobs.sqlite3'
    $jobsAfterColdOpen = & $cliPath '--json' '--state-db' $database 'jobs' 'list' '--limit' '1'
    Assert-True ($LASTEXITCODE -eq 0) 'CLI could not inspect the isolated Desktop database'
    $jobsAfterColdOpenJson = ($jobsAfterColdOpen -join "`n")
    Assert-True ($jobsAfterColdOpenJson.Trim() -eq '[]') 'shell open created a durable Job without approval'

    $engineRoot = Join-Path $env:APPDATA 'local.formatwright.desktop\engines'
    $installedManifests = @(Get-ChildItem -LiteralPath $engineRoot -Filter 'manifest.json' -File -Recurse)
    Assert-True ($installedManifests.Count -eq 2) 'installed Starter does not contain exactly two engine manifests'
    $installedPackIds = @()
    foreach ($manifestFile in $installedManifests) {
        $manifest = Get-Content -LiteralPath $manifestFile.FullName -Raw | ConvertFrom-Json
        $installedPackIds += [string]$manifest.engine_id
        Assert-True ($null -ne $manifest.supply_chain) "installed pack has no supply-chain metadata: $($manifest.engine_id)"
        $packRoot = Split-Path -Parent $manifestFile.FullName
        foreach ($sidecar in @(
            [pscustomobject]@{ Path = [string]$manifest.supply_chain.sbom_path; Hash = [string]$manifest.supply_chain.sbom_sha256 },
            [pscustomobject]@{ Path = [string]$manifest.supply_chain.sources_path; Hash = [string]$manifest.supply_chain.sources_sha256 }
        )) {
            $sidecarPath = Join-Path $packRoot $sidecar.Path
            Assert-True (Test-Path -LiteralPath $sidecarPath -PathType Leaf) "installed sidecar is missing: $sidecarPath"
            $observedHash = (Get-FileHash -LiteralPath $sidecarPath -Algorithm SHA256).Hash.ToLowerInvariant()
            Assert-True ($observedHash -ceq $sidecar.Hash.ToLowerInvariant()) "installed sidecar hash differs: $sidecarPath"
        }
        $sourcesPath = Join-Path $packRoot ([string]$manifest.supply_chain.sources_path)
        $sources = Get-Content -LiteralPath $sourcesPath -Raw | ConvertFrom-Json
        Assert-True ($sources.review_status -ceq 'incomplete') 'development pack source review status is overstated'
        $null = & $cliPath '--json' 'engines' 'verify' $manifestFile.FullName
        Assert-True ($LASTEXITCODE -eq 0) "CLI rejected installed Starter pack: $($manifest.engine_id)"
    }
    $installedPackIds = @($installedPackIds | Sort-Object)
    Assert-True (($installedPackIds -join ',') -ceq 'formatwright-media,formatwright-pdf') 'installed Starter pack identities differ'

    $second = Start-ExplorerVerb -Path $fixtureRoot
    $second.WaitForExit(30000) | Out-Null
    Assert-True $second.HasExited 'second instance did not exit'
    Assert-True ($second.ExitCode -eq 0) 'second instance returned non-zero'
    $processesAfterHotOpen = @(Get-FormatWrightProcesses)
    Assert-True ($processesAfterHotOpen.Count -eq 1) 'hot open created another long-lived process'
    Assert-True ($processesAfterHotOpen[0].Id -eq $process.Id) 'hot open replaced the original process'
    $directoryValues = Wait-ForEditableValue -Window $window -Expected $fixtureRoot -Deadline (
        [DateTime]::UtcNow.AddSeconds(30)
    )

    $missing = Join-Path $fixtureRoot 'missing-does-not-exist.txt'
    $negative = Start-ShellOpen -Executable $executable -Path $missing
    $negative.WaitForExit(30000) | Out-Null
    Assert-True $negative.HasExited 'negative second instance did not exit'
    Assert-True ($negative.ExitCode -eq 0) 'negative second instance returned non-zero'
    Start-Sleep -Milliseconds 300
    $valuesAfterNegative = @(Get-EditableValues -Window $window)
    Assert-True ($valuesAfterNegative -contains $fixtureRoot) 'invalid path changed the visible selection'
    Assert-True (-not ($valuesAfterNegative -contains $missing)) 'invalid path entered the UI'

    Stop-Process -Id $process.Id -Force
    $process.WaitForExit()
    $app = $null

    $pdfFixture = Join-Path $fixtureRoot 'tiny-shell.pdf'
    Write-TinyPdf -Path $pdfFixture
    $sourceHashBefore = (Get-FileHash -LiteralPath $pdfFixture -Algorithm SHA256).Hash.ToLowerInvariant()
    $convertApp = Start-ShellConvert -Executable $executable -Target 'png' -Path $pdfFixture
    $convertProcess = Wait-ForSingleProcess -Deadline ([DateTime]::UtcNow.AddSeconds(30))
    $null = Get-WindowAutomation -ProcessId $convertProcess.Id
    $convertDeadline = [DateTime]::UtcNow.AddSeconds(180)
    $convertJobs = @()
    do {
        $jobsJson = & $cliPath '--json' '--state-db' $database 'jobs' 'list' '--limit' '10'
        Assert-True ($LASTEXITCODE -eq 0) 'CLI could not inspect jobs after convert'
        $convertJobs = @(($jobsJson -join "`n") | ConvertFrom-Json)
        if ($convertJobs.Count -ge 1 -and @('completed', 'warning', 'failed').Contains([string]$convertJobs[0].state)) {
            break
        }
        Start-Sleep -Milliseconds 500
    } until ([DateTime]::UtcNow -ge $convertDeadline)
    Assert-True ($convertJobs.Count -eq 1) "convert expected exactly one job, observed $($convertJobs.Count)"
    Assert-True ($convertJobs[0].state -ceq 'completed' -or $convertJobs[0].state -ceq 'warning') "convert job did not pass: $($convertJobs[0].state)"
    $reportPath = Join-Path $env:APPDATA ("local.formatwright.desktop\reports\" + $convertJobs[0].id + ".json")
    Assert-True (Test-Path -LiteralPath $reportPath -PathType Leaf) "convert ValidationReport missing: $reportPath"
    $report = Get-Content -LiteralPath $reportPath -Raw -Encoding utf8 | ConvertFrom-Json
    Assert-True ($report.status -ceq 'pass' -or $report.status -ceq 'warning') "convert report was not Pass: $($report.status)"
    Assert-True (Test-Path -LiteralPath $convertJobs[0].output_path) "convert output missing: $($convertJobs[0].output_path)"
    $sourceHashAfter = (Get-FileHash -LiteralPath $pdfFixture -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-True ($sourceHashBefore -ceq $sourceHashAfter) 'convert mutated the source PDF'
    Stop-Process -Id $convertProcess.Id -Force -ErrorAction SilentlyContinue
    if (-not $convertApp.HasExited) { $convertApp.WaitForExit(15000) | Out-Null }

    $uninstallProcess = Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 2
    $uninstallResult = $uninstallProcess.ExitCode
    Assert-True ($uninstallProcess.ExitCode -eq 0) 'uninstaller returned non-zero'
    Assert-True (-not (Test-Path -LiteralPath $fileKey)) 'file verb remained after uninstall'
    Assert-True (-not (Test-Path -LiteralPath $directoryKey)) 'directory verb remained after uninstall'
    foreach ($convertKey in $convertKeys) {
        Assert-True (-not (Test-Path -LiteralPath $convertKey)) "convert verb remained after uninstall: $convertKey"
    }
    Assert-True (Test-Path -LiteralPath $siblingKey) 'uninstall removed an unrelated sibling verb'
    Assert-True (-not (Test-Path -LiteralPath $installRoot)) 'install root remained after uninstall'
    $installed = $false

    $result = [ordered]@{
        schema_version = 1
        case_path = $casePath
        installer_sha256 = (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash.ToLowerInvariant()
        exact_registry_quoting = $true
        cold_file_path_visible = $fileValues -contains $fileFixture
        hot_directory_path_visible = $directoryValues -contains $fixtureRoot
        single_instance_pid = $process.Id
        durable_jobs_created_by_open_in = 0
        convert_jobs_created = 1
        convert_report_status = [string]$report.status
        source_hash_unchanged = $true
        owned_convert_keys = $convertKeys.Count
        installed_pack_ids = $installedPackIds
        installed_supply_chain_verified = $true
        installed_source_review_status = 'incomplete'
        negative_missing_path_rejected = $true
        uninstall_exit_code = $uninstallResult
        owned_keys_removed = $true
        unrelated_sibling_preserved = $true
        application_state_isolated = $true
    }
    $result | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (
        Join-Path $casePath 'windows-explorer-integration-result.json'
    ) -Encoding utf8
    $result | ConvertTo-Json -Depth 5
} finally {
    if ($null -ne $app -and -not $app.HasExited) {
        Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
    }
    foreach ($process in @(Get-FormatWrightProcesses)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
    if ($installed) {
        $uninstaller = Join-Path $installRoot 'uninstall.exe'
        if (Test-Path -LiteralPath $uninstaller) {
            Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait -WindowStyle Hidden
        }
    }
    if ($siblingCreated -and (Test-Path -LiteralPath $siblingKey)) {
        Remove-Item -LiteralPath $siblingKey -Recurse -Force
    }
    if ($stateIsolated) {
        foreach ($root in $stateRoots) {
            Remove-CheckedTree -Target $root -AllowedParent (Split-Path -Parent $root)
            if ($isolatedState.ContainsKey($root)) {
                Move-Item -LiteralPath $isolatedState[$root] -Destination $root
            }
        }
        foreach ($root in $stateRoots) {
            Assert-TreeDigestEqual -Expected @($stateBefore[$root]) -Actual @(
                Get-TreeDigest -Root $root
            ) -Message "application state was not restored exactly: $root"
        }
    }
}
