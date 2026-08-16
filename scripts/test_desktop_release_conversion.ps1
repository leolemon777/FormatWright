#requires -Version 7.0

# Real Release UI conversion E2E: converts the source PDF to each target
# format through the actual desktop interface (inspect -> plan -> run ->
# validation report) driven over CDP. Each format runs in its own application
# process with its own isolated application state, so one format can never
# inherit another format's React or engine state.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SourcePdf,
    [string]$DesktopBinary = (Join-Path $PSScriptRoot '..\target\release\formatwright-desktop.exe'),
    [string]$ArtifactsRoot = (Join-Path $PSScriptRoot '..\.artifacts\desktop-release-conversion'),
    [string[]]$TargetFormats = @('png', 'jpg')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$Port = 9338

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "Desktop Release conversion assertion failed: $Message" }
}

function Remove-CheckedTree {
    param([string]$Target, [string]$AllowedParent)
    if (-not (Test-Path -LiteralPath $Target)) { return }
    $resolvedTarget = [IO.Path]::GetFullPath($Target).TrimEnd('\')
    $resolvedParent = [IO.Path]::GetFullPath($AllowedParent).TrimEnd('\')
    Assert-True ($resolvedTarget.StartsWith($resolvedParent + '\', [StringComparison]::OrdinalIgnoreCase)) "refusing to remove a path outside $AllowedParent"
    Remove-Item -LiteralPath $Target -Recurse -Force
}

$binaryPath = (Resolve-Path -LiteralPath $DesktopBinary).Path
$sourcePath = (Resolve-Path -LiteralPath $SourcePdf).Path
$nodeDriver = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot 'cdp_desktop_conversion_e2e.mjs')).Path

New-Item -ItemType Directory -Path $ArtifactsRoot -Force | Out-Null
$casePath = Join-Path ((Resolve-Path -LiteralPath $ArtifactsRoot).Path) ('suite-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $casePath | Out-Null
$input = Join-Path $casePath '输入 PDF 空格.pdf'
Copy-Item -LiteralPath $sourcePath -Destination $input
$stateRoots = @(
    (Join-Path $env:APPDATA 'local.formatwright.desktop'),
    (Join-Path $env:LOCALAPPDATA 'local.formatwright.desktop')
)
$formatSummaries = @()

foreach ($format in $TargetFormats) {
    $formatUpper = $format.ToUpperInvariant()
    $output = Join-Path $casePath "输出 $formatUpper 页面"
    $isolatedState = @{}
    $app = $null
    try {
        Assert-True (@(Get-Process -Name 'formatwright-desktop' -ErrorAction SilentlyContinue).Count -eq 0) "FormatWright is already running before the $format round"
        Assert-True (@(Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue).Count -eq 0) "port $Port is already in use before the $format round"

        foreach ($root in $stateRoots) {
            if (Test-Path -LiteralPath $root) {
                $isolated = "$root.formatwright-release-conversion-$([Guid]::NewGuid().ToString('N'))"
                Move-Item -LiteralPath $root -Destination $isolated
                $isolatedState[$root] = $isolated
            }
        }

        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = $binaryPath
        $start.UseShellExecute = $false
        $start.ArgumentList.Add('--shell-open')
        $start.ArgumentList.Add($input)
        $app = [Diagnostics.Process]::Start($start)
        Assert-True ($null -ne $app) "failed to launch Release desktop for the $format round"

        & node $nodeDriver $Port $casePath $input $format $output
        Assert-True ($LASTEXITCODE -eq 0) "DevTools conversion driver exited $LASTEXITCODE for the $format round"
        Assert-True (Test-Path -LiteralPath $output -PathType Container) "output directory is missing: $output"
        $pages = @(Get-ChildItem -LiteralPath $output -File | Sort-Object Name)
        Assert-True ($pages.Count -eq 3) "output does not contain three pages: $output"
        Assert-True (($pages.Name -join ',') -match "^page-000001\.$format,page-000002\.$format,page-000003\.$format$") "page names are not deterministic: $output"
        $formatSummaries += [pscustomobject]@{
            target_format = $format
            report = 'pass'
            pages = 3
            output = $output
        }
    }
    finally {
        if ($null -ne $app -and -not $app.HasExited) {
            Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
            $app.WaitForExit(10000) | Out-Null
        }
        Get-Process -Name 'formatwright-desktop' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 500
        foreach ($root in $stateRoots) {
            Remove-CheckedTree -Target $root -AllowedParent (Split-Path $root -Parent)
            if ($isolatedState.ContainsKey($root)) {
                Move-Item -LiteralPath $isolatedState[$root] -Destination $root
            }
        }
    }
}

[pscustomobject]@{
    schema_version = 1
    artifact_directory = $casePath
    input = $input
    conversions = $formatSummaries
    application_state_restored = $true
} | ConvertTo-Json -Depth 4
