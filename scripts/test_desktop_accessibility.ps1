#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$DesktopBinary = (Join-Path $PSScriptRoot '..\target\debug\formatwright-desktop.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts\desktop-accessibility')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$Port = 9337 # Must match tauri.accessibility.conf.json.

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "Desktop accessibility assertion failed: $Message" }
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

$binaryPath = (Resolve-Path -LiteralPath $DesktopBinary).Path
$nodeAudit = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot 'cdp_accessibility_audit.mjs')).Path
Assert-True (@(Get-Process -Name 'formatwright-desktop' -ErrorAction SilentlyContinue).Count -eq 0) 'FormatWright is already running'
Assert-True (@(Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue).Count -eq 0) "port $Port is already in use"

New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) ('suite-' + [Guid]::NewGuid().ToString('N'))
$fixtureRoot = Join-Path $casePath 'fixtures RTL 空格'
New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null
$fixture = Join-Path $fixtureRoot 'مرحبا שלום 名字.json'
Set-Content -LiteralPath $fixture -Value '{"formatwright":true}' -Encoding utf8

$stateRoots = @(
    (Join-Path $env:APPDATA 'local.formatwright.desktop'),
    (Join-Path $env:LOCALAPPDATA 'local.formatwright.desktop')
)
$stateBefore = @{}
$isolatedState = @{}
foreach ($root in $stateRoots) { $stateBefore[$root] = @(Get-TreeDigest -Root $root) }
$app = $null
$stateIsolated = $false

try {
    $stateIsolated = $true
    foreach ($root in $stateRoots) {
        if (Test-Path -LiteralPath $root) {
            $isolated = "$root.formatwright-accessibility-audit-$([Guid]::NewGuid().ToString('N'))"
            Move-Item -LiteralPath $root -Destination $isolated
            $isolatedState[$root] = $isolated
        }
    }

    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $binaryPath
    $start.UseShellExecute = $false
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.ArgumentList.Add('--shell-open')
    $start.ArgumentList.Add($fixture)
    $app = [Diagnostics.Process]::Start($start)
    Assert-True ($null -ne $app) 'failed to launch FormatWright'

    & node $nodeAudit $Port $casePath
    Assert-True ($LASTEXITCODE -eq 0) "DevTools accessibility audit exited $LASTEXITCODE"
    Assert-True (Test-Path -LiteralPath (Join-Path $casePath 'accessibility-audit.json')) 'audit report was not written'
} finally {
    if ($null -ne $app -and -not $app.HasExited) {
        Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
        $app.WaitForExit(10000) | Out-Null
    }
    Get-Process -Name 'formatwright-desktop' -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue

    if ($stateIsolated) {
        Start-Sleep -Milliseconds 500
        foreach ($root in $stateRoots) {
            Remove-CheckedTree -Target $root -AllowedParent (Split-Path $root -Parent)
            if ($isolatedState.ContainsKey($root)) {
                Move-Item -LiteralPath $isolatedState[$root] -Destination $root
            }
        }
        foreach ($root in $stateRoots) {
            Assert-TreeDigestEqual -Expected $stateBefore[$root] -Actual @(Get-TreeDigest -Root $root) -Message "application state changed: $root"
        }
    }
}

[pscustomobject]@{
    schema_version = 1
    artifact_directory = $casePath
    state_isolated = $true
    process_exit_clean = @(Get-Process -Name 'formatwright-desktop' -ErrorAction SilentlyContinue).Count -eq 0
} | ConvertTo-Json -Depth 3
