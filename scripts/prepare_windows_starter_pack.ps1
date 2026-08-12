param(
    [string]$DownloadRoot = ".devtools/downloads",
    [string]$SourceRoot = ".devtools/starter-sources",
    [string]$OutputRoot = "dist/engine-packs/windows-x86_64/starter"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$devtoolsRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ".devtools"))

function Resolve-RepoPath([string]$Path) {
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    [System.IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}

function Assert-WithinDevtools([string]$Path) {
    $prefix = $devtoolsRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $Path.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Dependency cache and source paths must stay inside $devtoolsRoot"
    }
}

$downloadPath = Resolve-RepoPath $DownloadRoot
$sourcePath = Resolve-RepoPath $SourceRoot
Assert-WithinDevtools $downloadPath
Assert-WithinDevtools $sourcePath
New-Item -ItemType Directory -Path $downloadPath -Force | Out-Null
New-Item -ItemType Directory -Path $sourcePath -Force | Out-Null

$dependencies = @(
    [ordered]@{
        Name = "poppler-26.02.0-0"
        Archive = "Release-26.02.0-0.zip"
        Url = "https://github.com/oschwartz10612/poppler-windows/releases/download/v26.02.0-0/Release-26.02.0-0.zip"
        Sha256 = "993e4a94376ed712fafc7058d724ea0b943d118bbd2305cd9ed55174eb85cda5"
        ExtractedDirectory = "poppler-26.02.0"
    },
    [ordered]@{
        Name = "ffmpeg-9.0-essentials"
        Archive = "ffmpeg-9.0-essentials_build.zip"
        Url = "https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-9.0-essentials_build.zip"
        Sha256 = "e6b54767a6065919048f1a098eb27211ca4e12b4348a05d88777a5855d0b6e71"
        ExtractedDirectory = "ffmpeg-9.0-essentials_build"
    }
)

function Get-Sha256([string]$Path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Get-VerifiedArchive($Dependency) {
    $archivePath = Join-Path $downloadPath $Dependency.Archive
    if ((Test-Path -LiteralPath $archivePath -PathType Leaf) -and
        (Get-Sha256 $archivePath) -eq $Dependency.Sha256) {
        return $archivePath
    }

    $partial = Join-Path $downloadPath (".{0}.{1}.partial" -f $Dependency.Archive, [guid]::NewGuid().ToString("N"))
    try {
        Invoke-WebRequest -Uri $Dependency.Url -OutFile $partial -UseBasicParsing
        $observed = Get-Sha256 $partial
        if ($observed -ne $Dependency.Sha256) {
            throw "Archive hash mismatch for $($Dependency.Name): expected $($Dependency.Sha256), observed $observed"
        }
        Move-Item -LiteralPath $partial -Destination $archivePath -Force
        return $archivePath
    } finally {
        if (Test-Path -LiteralPath $partial) {
            Remove-Item -LiteralPath $partial -Force
        }
    }
}

function Expand-VerifiedArchive($Dependency, [string]$ArchivePath) {
    $destination = Join-Path $sourcePath $Dependency.Name
    $expectedRoot = Join-Path $destination $Dependency.ExtractedDirectory
    if (Test-Path -LiteralPath $expectedRoot -PathType Container) {
        return $expectedRoot
    }

    $staging = Join-Path $sourcePath (".{0}.{1}.partial" -f $Dependency.Name, [guid]::NewGuid().ToString("N"))
    $backup = Join-Path $sourcePath (".{0}.{1}.backup" -f $Dependency.Name, [guid]::NewGuid().ToString("N"))
    try {
        Expand-Archive -LiteralPath $ArchivePath -DestinationPath $staging
        $stagedRoot = Join-Path $staging $Dependency.ExtractedDirectory
        if (-not (Test-Path -LiteralPath $stagedRoot -PathType Container)) {
            throw "Archive layout is unexpected for $($Dependency.Name)"
        }
        if (Test-Path -LiteralPath $destination) {
            Move-Item -LiteralPath $destination -Destination $backup
        }
        Move-Item -LiteralPath $staging -Destination $destination
        if (Test-Path -LiteralPath $backup) {
            Remove-Item -LiteralPath $backup -Recurse -Force
        }
        return $expectedRoot
    } catch {
        if ((Test-Path -LiteralPath $backup) -and -not (Test-Path -LiteralPath $destination)) {
            Move-Item -LiteralPath $backup -Destination $destination
        }
        throw
    } finally {
        if (Test-Path -LiteralPath $staging) {
            Remove-Item -LiteralPath $staging -Recurse -Force
        }
    }
}

$resolved = @{}
foreach ($dependency in $dependencies) {
    $archive = Get-VerifiedArchive $dependency
    $resolved[$dependency.Name] = Expand-VerifiedArchive $dependency $archive
}

& (Join-Path $PSScriptRoot "build_windows_starter_pack.ps1") `
    -PopplerRoot $resolved["poppler-26.02.0-0"] `
    -FfmpegRoot $resolved["ffmpeg-9.0-essentials"] `
    -OutputRoot $OutputRoot
