#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Cargo = "cargo"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

Push-Location $repoRoot
try {
    & $Cargo test -p formatwright-core 'preset::tests'
    if ($LASTEXITCODE -ne 0) { throw 'core preset contract tests failed' }

    & $Cargo test -p formatwright-core 'rust_preset_library_matches_public_schema'
    if ($LASTEXITCODE -ne 0) { throw 'preset JSON Schema contract test failed' }

    & $Cargo test -p formatwright-desktop 'preset_library_write_is_recoverable_from_backup'
    if ($LASTEXITCODE -ne 0) { throw 'desktop preset recovery test failed' }

    Write-Output 'preset sandbox: version/bounds/unknown-field rejection, atomic mutation, public schema, and backup recovery passed'
}
finally {
    Pop-Location
}
