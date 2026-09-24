<#
.SYNOPSIS
Stage the pinned Windows FFmpeg runtime outside Git.

.DESCRIPTION
Downloads or accepts the pinned BtbN FFmpeg archive, verifies its checksum,
runtime files, license, and tracked provenance notice, then stages ffmpeg.exe
and ffprobe.exe under src-tauri/vendor/ffmpeg. Runtime binaries are ignored by Git.
#>
[CmdletBinding()]
param(
    [string]$StagingDirectory,
    [string]$ArchivePath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$runtimeManifestPath = Join-Path $PSScriptRoot 'runtime-manifest.json'
$runtimeManifest = Get-Content -Raw -LiteralPath $runtimeManifestPath | ConvertFrom-Json
$ffmpeg = $runtimeManifest.components | Where-Object { $_.id -eq 'ffmpeg-btbn-gpl-n9.0' } | Select-Object -First 1
if ($null -eq $ffmpeg -or $runtimeManifest.schemaVersion -ne 1) {
    throw "Runtime manifest is missing schemaVersion=1 or the ffmpeg component: $runtimeManifestPath"
}

$defaultDestination = Join-Path $repositoryRoot $ffmpeg.stagingDirectory
$defaultDestination = [System.IO.Path]::GetFullPath($defaultDestination)
$expectedDestination = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot 'src-tauri/vendor/ffmpeg'))
$manifestDestination = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $ffmpeg.stagingDirectory))
if (-not [string]::Equals($manifestDestination, $expectedDestination, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Runtime manifest stagingDirectory must resolve to src-tauri/vendor/ffmpeg inside this repository; got '$manifestDestination'."
}
if ([string]::IsNullOrWhiteSpace($StagingDirectory)) {
    $destination = $defaultDestination
} else {
    $destination = [System.IO.Path]::GetFullPath($StagingDirectory)
    $temporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $destination.StartsWith($temporaryRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "An explicit StagingDirectory must be a new directory below the system temporary directory; got '$destination'."
    }
    if (Test-Path -LiteralPath $destination) {
        throw "Explicit StagingDirectory already exists; refusing to overwrite it: $destination"
    }
}
$requiredNames = @('ffmpeg.exe', 'ffprobe.exe')
$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ('qlisa-runtime-' + [guid]::NewGuid().ToString('N'))

try {
    New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
    $archive = Join-Path $temporaryDirectory ([System.IO.Path]::GetFileName(([uri]$ffmpeg.archive.url).AbsolutePath))
    if ([string]::IsNullOrWhiteSpace($ArchivePath)) {
        Write-Host "Downloading $($ffmpeg.archive.url)"
        Invoke-WebRequest -Uri $ffmpeg.archive.url -OutFile $archive
    } else {
        $archive = (Resolve-Path -LiteralPath $ArchivePath).Path
    }

    $archiveHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($archiveHash -ne $ffmpeg.archive.sha256.ToLowerInvariant()) {
        throw "FFmpeg archive SHA-256 mismatch. Expected $($ffmpeg.archive.sha256); got $archiveHash. Review the upstream build before changing the pin."
    }

    $extractRoot = Join-Path $temporaryDirectory 'extracted'
    Expand-Archive -LiteralPath $archive -DestinationPath $extractRoot
    $buildRootName = [string]$ffmpeg.archive.rootDirectory
    $buildRoot = Join-Path $extractRoot $buildRootName
    if (-not (Test-Path -LiteralPath $buildRoot -PathType Container)) {
        throw "Unexpected FFmpeg archive layout; expected directory '$buildRootName'."
    }

    foreach ($name in $requiredNames) {
        $source = Join-Path $buildRoot (Join-Path 'bin' $name)
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "FFmpeg archive is missing bin/$name." }
        $expected = @($ffmpeg.runtimeFiles | Where-Object { $_.fileName -eq $name }) | Select-Object -First 1
        if ($null -eq $expected) { throw "Runtime manifest has no hash record for $name." }
        $actualHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actualHash -ne $expected.sha256.ToLowerInvariant()) {
            throw "$name SHA-256 mismatch. Expected $($expected.sha256); got $actualHash. The archive contents changed despite the archive pin."
        }
    }

    foreach ($notice in @($ffmpeg.notices)) {
        $trackedNotice = Join-Path $repositoryRoot $notice.path
        if (-not (Test-Path -LiteralPath $trackedNotice -PathType Leaf)) { throw "Tracked FFmpeg notice is missing: $($notice.path)." }
        $trackedNoticeHash = (Get-FileHash -LiteralPath $trackedNotice -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($trackedNoticeHash -ne $notice.sha256.ToLowerInvariant()) {
            throw "FFmpeg notice hash mismatch for $($notice.path). Review the pinned archive and tracked notice before staging."
        }
        if ($notice.PSObject.Properties['archivePath'] -and $notice.archivePath) {
            $archiveNotice = Join-Path $buildRoot $notice.archivePath
            if (-not (Test-Path -LiteralPath $archiveNotice -PathType Leaf)) { throw "FFmpeg archive is missing $($notice.archivePath)." }
            $archiveNoticeHash = (Get-FileHash -LiteralPath $archiveNotice -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($archiveNoticeHash -ne $notice.sha256.ToLowerInvariant()) { throw "FFmpeg archive notice SHA-256 mismatch for $($notice.archivePath)." }
        }
    }

    foreach ($record in @($ffmpeg.runtimeFiles)) {
        $versionOutput = (& (Join-Path $buildRoot (Join-Path 'bin' $record.fileName)) -hide_banner -version 2>&1) -join "`n"
        if ($LASTEXITCODE -ne 0 -or $versionOutput -notmatch [regex]::Escape($record.versionText)) {
            throw "The selected $($record.fileName) does not report the pinned BtbN version."
        }
    }
    $protocolOutput = (& (Join-Path $buildRoot 'bin\ffmpeg.exe') -hide_banner -protocols 2>&1) -join "`n"
    if ($LASTEXITCODE -ne 0 -or -not [regex]::IsMatch($protocolOutput, '(?m)^\s*srt\s*$')) {
        throw 'The pinned FFmpeg does not advertise the SRT protocol.'
    }

    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    foreach ($name in $requiredNames) {
        Copy-Item -LiteralPath (Join-Path $buildRoot (Join-Path 'bin' $name)) -Destination (Join-Path $destination $name) -Force
    }

    foreach ($notice in @($ffmpeg.notices)) {
        if ($notice.PSObject.Properties['archivePath'] -and $notice.archivePath) {
            $source = Join-Path $buildRoot $notice.archivePath
        } else {
            $source = Join-Path $repositoryRoot $notice.path
        }
        $destinationNotice = Join-Path $destination ([System.IO.Path]::GetFileName([string]$notice.path))
        if (-not [string]::Equals(
            [System.IO.Path]::GetFullPath($source),
            [System.IO.Path]::GetFullPath($destinationNotice),
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            Copy-Item -LiteralPath $source -Destination $destinationNotice -Force
        }
    }
    Remove-Item -LiteralPath (Join-Path $destination 'README-Gyan-build.txt') -Force -ErrorAction SilentlyContinue

    foreach ($name in $requiredNames) {
        $stagedPath = Join-Path $destination $name
        $stagedHash = (Get-FileHash -LiteralPath $stagedPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $expected = @($ffmpeg.runtimeFiles | Where-Object { $_.fileName -eq $name }) | Select-Object -First 1
        if ($stagedHash -ne $expected.sha256.ToLowerInvariant()) { throw "Staged $name failed its SHA-256 check." }
    }

    Write-Host "Staged BtbN FFmpeg $($ffmpeg.version) and ffprobe in $destination"
    Write-Host "Archive SHA-256: $archiveHash"
    Write-Host 'SRT protocol: present. NDI Runtime remains installed separately by the user.'
} finally {
    if (Test-Path -LiteralPath $temporaryDirectory) { Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force }
}
