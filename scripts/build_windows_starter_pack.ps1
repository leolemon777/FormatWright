param(
    [Parameter(Mandatory = $true)]
    [string]$PopplerRoot,
    [Parameter(Mandatory = $true)]
    [string]$FfmpegRoot,
    [string]$PopplerVersion = "26.02.0-0",
    [string]$FfmpegVersion = "9.0",
    [long]$SourceDateEpoch = 0,
    [string]$OutputRoot = "dist/engine-packs/windows-x86_64/starter"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$allowedRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "dist/engine-packs"))
$outputPath = if ([System.IO.Path]::IsPathRooted($OutputRoot)) {
    [System.IO.Path]::GetFullPath($OutputRoot)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputRoot))
}
$allowedPrefix = $allowedRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
if (-not $outputPath.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "OutputRoot must stay inside $allowedRoot"
}

$popplerPath = [System.IO.Path]::GetFullPath($PopplerRoot)
$ffmpegPath = [System.IO.Path]::GetFullPath($FfmpegRoot)
$popplerBin = Join-Path $popplerPath "Library/bin"
$popplerData = Join-Path $popplerPath "share/poppler"
$ffmpegBin = Join-Path $ffmpegPath "bin"
foreach ($required in @(
    (Join-Path $popplerBin "pdfinfo.exe"),
    (Join-Path $popplerBin "pdftoppm.exe"),
    (Join-Path $popplerData "COPYING"),
    (Join-Path $ffmpegBin "ffmpeg.exe"),
    (Join-Path $ffmpegBin "ffprobe.exe"),
    (Join-Path $ffmpegPath "LICENSE")
)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Required Starter source file is missing: $required"
    }
}

New-Item -ItemType Directory -Path $allowedRoot -Force | Out-Null
$outputParent = Split-Path -Parent $outputPath
New-Item -ItemType Directory -Path $outputParent -Force | Out-Null
$staging = Join-Path $allowedRoot (".starter.{0}.partial" -f [guid]::NewGuid().ToString("N"))
$backup = Join-Path $allowedRoot (".starter.{0}.backup" -f [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $staging | Out-Null

function Get-Sha256([string]$Path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Copy-PackFile(
    [string]$Source,
    [string]$PackRoot,
    [string]$RelativePath
) {
    $normalized = $RelativePath.Replace("\", "/")
    if ($normalized.StartsWith("/") -or $normalized.Split("/") -contains "..") {
        throw "Unsafe pack relative path: $RelativePath"
    }
    $destination = Join-Path $PackRoot $normalized
    $parent = Split-Path -Parent $destination
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    Copy-Item -LiteralPath $Source -Destination $destination
    [ordered]@{
        relative_path = $normalized
        sha256 = Get-Sha256 $destination
    }
}

function Write-Utf8File([string]$Path, [string]$Content) {
    $parent = Split-Path -Parent $Path
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
}

function Add-SupplyChainFiles(
    [string]$PackRoot,
    [System.Collections.Specialized.OrderedDictionary]$Manifest,
    [System.Collections.Specialized.OrderedDictionary]$Sources
) {
    $manifestPath = Join-Path $PackRoot "manifest.json"
    $sourcesPath = Join-Path $PackRoot "sources.json"
    $sbomPath = Join-Path $PackRoot "sbom.spdx.json"
    Write-Utf8File $sourcesPath ($Sources | ConvertTo-Json -Depth 20)
    $Manifest["supply_chain"] = [ordered]@{
        sbom_path = "sbom.spdx.json"
        sbom_sha256 = "0" * 64
        sources_path = "sources.json"
        sources_sha256 = Get-Sha256 $sourcesPath
    }
    Write-Utf8File $manifestPath ($Manifest | ConvertTo-Json -Depth 20)
    & python (Join-Path $PSScriptRoot "generate_engine_sbom.py") `
        --manifest $manifestPath `
        --output $sbomPath `
        --source-date-epoch $SourceDateEpoch
    if ($LASTEXITCODE -ne 0) {
        throw "Engine SBOM generation failed for $($Manifest.engine_id)"
    }
    $Manifest["supply_chain"].sbom_sha256 = Get-Sha256 $sbomPath
    Write-Utf8File $manifestPath ($Manifest | ConvertTo-Json -Depth 20)
    & python (Join-Path $PSScriptRoot "generate_engine_sbom.py") `
        --manifest $manifestPath `
        --verify $sbomPath
    if ($LASTEXITCODE -ne 0) {
        throw "Engine SBOM verification failed for $($Manifest.engine_id)"
    }
}

try {
    $pdfRoot = Join-Path $staging "pdf"
    New-Item -ItemType Directory -Path $pdfRoot | Out-Null
    $pdfinfoFile = Copy-PackFile (Join-Path $popplerBin "pdfinfo.exe") $pdfRoot "bin/pdfinfo.exe"
    $pdftoppmFile = Copy-PackFile (Join-Path $popplerBin "pdftoppm.exe") $pdfRoot "bin/pdftoppm.exe"
    $pdfExecutables = @(
        [ordered]@{ name = "pdfinfo"; relative_path = $pdfinfoFile.relative_path; sha256 = $pdfinfoFile.sha256 },
        [ordered]@{ name = "pdftoppm"; relative_path = $pdftoppmFile.relative_path; sha256 = $pdftoppmFile.sha256 }
    )
    $pdfRuntime = @()
    foreach ($file in Get-ChildItem -LiteralPath $popplerBin -Filter "*.dll" -File | Sort-Object Name) {
        $pdfRuntime += Copy-PackFile $file.FullName $pdfRoot ("bin/{0}" -f $file.Name)
    }
    foreach ($file in Get-ChildItem -LiteralPath $popplerData -Recurse -File | Sort-Object FullName) {
        $relative = [System.IO.Path]::GetRelativePath($popplerData, $file.FullName).Replace("\", "/")
        if ($relative -match "^COPYING(\.|$)") {
            continue
        }
        $pdfRuntime += Copy-PackFile $file.FullName $pdfRoot ("share/poppler/{0}" -f $relative)
    }
    $popplerNotice = Copy-PackFile (Join-Path $popplerData "COPYING.gpl2") $pdfRoot "licenses/POPPLER-GPL-2.0.txt"
    $adobeNotice = Copy-PackFile (Join-Path $popplerData "COPYING.adobe") $pdfRoot "licenses/POPPLER-DATA-ADOBE.txt"
    $pdfProvenancePath = Join-Path $pdfRoot "PROVENANCE.txt"
    Write-Utf8File $pdfProvenancePath @"
FormatWright Windows PDF development pack
Poppler upstream version: $PopplerVersion
Binary distributor: https://github.com/oschwartz10612/poppler-windows
Binary archive SHA-256: 993e4a94376ed712fafc7058d724ea0b943d118bbd2305cd9ed55174eb85cda5
Source project: https://poppler.freedesktop.org/
Certification status: development/unverified; transitive dependency license inventory remains a release gate.
"@
    $pdfRuntime += [ordered]@{
        relative_path = "PROVENANCE.txt"
        sha256 = Get-Sha256 $pdfProvenancePath
    }
    $pdfManifest = [ordered]@{
        schema_version = 1
        engine_id = "formatwright-pdf"
        version = $PopplerVersion
        platform = "windows"
        architecture = "x86_64"
        protocol_version = 1
        formatwright_compatibility = [ordered]@{ minimum = "0.1.0"; maximum_exclusive = "0.2.0" }
        executables = $pdfExecutables
        runtime_files = $pdfRuntime
        source = [ordered]@{
            project_url = "https://poppler.freedesktop.org/"
            source_url = "https://poppler.freedesktop.org/poppler-$($PopplerVersion.Split('-')[0]).tar.xz"
            source_revision = $PopplerVersion.Split('-')[0]
            build_configuration = "Windows binaries from oschwartz10612/poppler-windows $PopplerVersion; conda-forge dependency build; archive sha256=993e4a94376ed712fafc7058d724ea0b943d118bbd2305cd9ed55174eb85cda5"
        }
        licenses = @(
            [ordered]@{ spdx = "GPL-2.0-or-later"; notice_path = $popplerNotice.relative_path; source_offer_path = $null },
            [ordered]@{ spdx = "LicenseRef-Adobe-CMap"; notice_path = $adobeNotice.relative_path; source_offer_path = $null }
        )
        capabilities = @(
            [ordered]@{ capability_id = "poppler.pdf.inspect"; inputs = @("pdf"); outputs = @("probe/v1"); operation = "inspect"; loss_class = "none"; constraints = [ordered]@{ network_policy = "deny" } },
            [ordered]@{ capability_id = "poppler.pdf.render"; inputs = @("pdf"); outputs = @("png", "jpg"); operation = "render"; loss_class = "lossy"; constraints = [ordered]@{ network_policy = "deny"; all_pages = $true } }
        )
        signature = $null
    }
    $pdfSources = [ordered]@{
        schema_version = 1
        engine_id = "formatwright-pdf"
        version = $PopplerVersion
        review_status = "incomplete"
        artifacts = @(
            [ordered]@{
                name = "oschwartz10612/poppler-windows"
                artifact_type = "binary-distribution"
                download_url = "https://github.com/oschwartz10612/poppler-windows/releases/download/v$PopplerVersion/Release-$PopplerVersion.zip"
                sha256 = "993e4a94376ed712fafc7058d724ea0b943d118bbd2305cd9ed55174eb85cda5"
                source_url = "https://poppler.freedesktop.org/poppler-$($PopplerVersion.Split('-')[0]).tar.xz"
                source_revision = $PopplerVersion.Split('-')[0]
                license_review_status = "incomplete"
            },
            [ordered]@{
                name = "poppler-data"
                artifact_type = "runtime-data"
                download_url = "https://poppler.freedesktop.org/"
                sha256 = $null
                source_url = "https://poppler.freedesktop.org/"
                source_revision = "bundled-by-poppler-windows-$PopplerVersion"
                license_review_status = "incomplete"
            }
        )
        completeness_notes = "The file-level SPDX inventory is complete for the declared pack payload. Transitive component attribution, source-offer and legal review remain incomplete; this pack is not Certified."
    }
    Add-SupplyChainFiles $pdfRoot $pdfManifest $pdfSources

    $mediaRoot = Join-Path $staging "media"
    New-Item -ItemType Directory -Path $mediaRoot | Out-Null
    $ffmpegFile = Copy-PackFile (Join-Path $ffmpegBin "ffmpeg.exe") $mediaRoot "bin/ffmpeg.exe"
    $ffprobeFile = Copy-PackFile (Join-Path $ffmpegBin "ffprobe.exe") $mediaRoot "bin/ffprobe.exe"
    $mediaExecutables = @(
        [ordered]@{ name = "ffmpeg"; relative_path = $ffmpegFile.relative_path; sha256 = $ffmpegFile.sha256 },
        [ordered]@{ name = "ffprobe"; relative_path = $ffprobeFile.relative_path; sha256 = $ffprobeFile.sha256 }
    )
    $ffmpegNotice = Copy-PackFile (Join-Path $ffmpegPath "LICENSE") $mediaRoot "licenses/FFMPEG-GPL-3.0.txt"
    $ffmpegReadme = Copy-PackFile (Join-Path $ffmpegPath "README.txt") $mediaRoot "README.txt"
    $buildConfiguration = (& (Join-Path $ffmpegBin "ffmpeg.exe") -buildconf 2>&1 | Out-String).Trim()
    $mediaProvenancePath = Join-Path $mediaRoot "PROVENANCE.txt"
    Write-Utf8File $mediaProvenancePath @"
FormatWright Windows Media development pack
FFmpeg upstream version: $FfmpegVersion
Binary distributor: https://github.com/GyanD/codexffmpeg
Binary archive SHA-256: e6b54767a6065919048f1a098eb27211ca4e12b4348a05d88777a5855d0b6e71
Source project: https://ffmpeg.org/
Certification status: development/unverified; GPL source-offer and patent/region review remain release gates.
"@
    $mediaRuntime = @(
        $ffmpegReadme,
        [ordered]@{ relative_path = "PROVENANCE.txt"; sha256 = Get-Sha256 $mediaProvenancePath }
    )
    $mediaManifest = [ordered]@{
        schema_version = 1
        engine_id = "formatwright-media"
        version = $FfmpegVersion
        platform = "windows"
        architecture = "x86_64"
        protocol_version = 1
        formatwright_compatibility = [ordered]@{ minimum = "0.1.0"; maximum_exclusive = "0.2.0" }
        executables = $mediaExecutables
        runtime_files = $mediaRuntime
        source = [ordered]@{
            project_url = "https://ffmpeg.org/"
            source_url = "https://ffmpeg.org/releases/ffmpeg-$FfmpegVersion.tar.xz"
            source_revision = "n$FfmpegVersion"
            build_configuration = "Gyan essentials archive sha256=e6b54767a6065919048f1a098eb27211ca4e12b4348a05d88777a5855d0b6e71`n$buildConfiguration"
        }
        licenses = @(
            [ordered]@{ spdx = "GPL-3.0-or-later"; notice_path = $ffmpegNotice.relative_path; source_offer_path = $null }
        )
        capabilities = @(
            [ordered]@{ capability_id = "ffprobe.media.inspect"; inputs = @("png", "jpg", "jpeg", "mov", "mkv", "avi", "webm", "mp4", "wav", "flac", "aac", "m4a", "ogg", "opus", "mp3"); outputs = @("probe/v1"); operation = "inspect"; loss_class = "none"; constraints = [ordered]@{ network_policy = "deny" } },
            [ordered]@{ capability_id = "ffmpeg.media.convert"; inputs = @("png", "jpg", "jpeg", "mov", "mkv", "avi", "webm", "mp4", "wav", "flac", "aac", "m4a", "ogg", "opus", "mp3"); outputs = @("webp", "avif", "mp4", "gif", "mp3", "m4a", "wav"); operation = "transcode"; loss_class = "lossy"; constraints = [ordered]@{ network_policy = "deny" } }
        )
        signature = $null
    }
    $mediaSources = [ordered]@{
        schema_version = 1
        engine_id = "formatwright-media"
        version = $FfmpegVersion
        review_status = "incomplete"
        artifacts = @(
            [ordered]@{
                name = "Gyan FFmpeg essentials"
                artifact_type = "binary-distribution"
                download_url = "https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-$FfmpegVersion-essentials_build.zip"
                sha256 = "e6b54767a6065919048f1a098eb27211ca4e12b4348a05d88777a5855d0b6e71"
                source_url = "https://ffmpeg.org/releases/ffmpeg-$FfmpegVersion.tar.xz"
                source_revision = "n$FfmpegVersion"
                license_review_status = "incomplete"
            }
        )
        completeness_notes = "The file-level SPDX inventory is complete for the declared static binary pack payload. FFmpeg build-component attribution, corresponding-source mechanism, codec patent/region and legal review remain incomplete; this pack is not Certified."
    }
    Add-SupplyChainFiles $mediaRoot $mediaManifest $mediaSources

    $bundle = [ordered]@{
        schema_version = 1
        bundle_id = "formatwright-windows-starter"
        application_version = "0.1.0"
        packs = @("pdf/manifest.json", "media/manifest.json")
    }
    Write-Utf8File (Join-Path $staging "bundle.json") ($bundle | ConvertTo-Json -Depth 8)

    if (Test-Path -LiteralPath $outputPath) {
        Move-Item -LiteralPath $outputPath -Destination $backup
    }
    Move-Item -LiteralPath $staging -Destination $outputPath
    if (Test-Path -LiteralPath $backup) {
        Remove-Item -LiteralPath $backup -Recurse -Force
    }

    $files = Get-ChildItem -LiteralPath $outputPath -Recurse -File
    [ordered]@{
        output = $outputPath
        files = $files.Count
        bytes = ($files | Measure-Object Length -Sum).Sum
        pdf_manifest = Join-Path $outputPath "pdf/manifest.json"
        media_manifest = Join-Path $outputPath "media/manifest.json"
    } | ConvertTo-Json
} catch {
    if (Test-Path -LiteralPath $staging) {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
    if ((Test-Path -LiteralPath $backup) -and -not (Test-Path -LiteralPath $outputPath)) {
        Move-Item -LiteralPath $backup -Destination $outputPath
    }
    throw
}
