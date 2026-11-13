<#
.SYNOPSIS
Stage the recommended pinned Windows FFmpeg runtime outside Git.

.DESCRIPTION
Downloads or accepts the recommended kit's versioned FFmpeg package, verifies
the package checksum and runtime files, validates the tracked provenance notice,
then stages ffmpeg.exe and ffprobe.exe under src-tauri/vendor/ffmpeg. Runtime
binaries are ignored by Git. Legacy upstream records remain provenance only.
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
$kit = @($runtimeManifest.approvedKits | Where-Object { $_.id -eq $runtimeManifest.recommendedKitId }) | Select-Object -First 1
if ($runtimeManifest.schemaVersion -ne 1 -or $null -eq $kit) {
    throw "Runtime manifest is missing schemaVersion=1 or recommended kit: $runtimeManifestPath"
}
$ffmpeg = @($kit.components | Where-Object { $_.id -eq 'ffmpeg' }) | Select-Object -First 1
$ffmpegProvenance = @($runtimeManifest.components | Where-Object { $_.id -eq 'ffmpeg-gyan-full-9.0.2' }) | Select-Object -First 1
if ($null -eq $ffmpeg -or $null -eq $ffmpegProvenance) {
    throw "Runtime manifest is missing schemaVersion=1, recommended kit FFmpeg, or FFmpeg provenance: $runtimeManifestPath"
}
if ([string]$ffmpeg.asset.format -ne 'zip' -or [string]$ffmpeg.asset.sha256 -notmatch '^[a-fA-F0-9]{64}$' -or
    [string]$ffmpeg.asset.url -notmatch '^https://') {
    throw "Recommended kit FFmpeg asset metadata is invalid: $runtimeManifestPath"
}

$defaultDestination = Join-Path $repositoryRoot $ffmpegProvenance.stagingDirectory
$defaultDestination = [System.IO.Path]::GetFullPath($defaultDestination)
$expectedDestination = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot 'src-tauri/vendor/ffmpeg'))
$manifestDestination = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $ffmpegProvenance.stagingDirectory))
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
$extractedRuntimeFiles = @{}
$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ('qlisa-runtime-' + [guid]::NewGuid().ToString('N'))

try {
    New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
    $archive = Join-Path $temporaryDirectory ([System.IO.Path]::GetFileName(([uri]$ffmpeg.asset.url).AbsolutePath))
    if ([string]::IsNullOrWhiteSpace($ArchivePath)) {
        Write-Host "Downloading recommended kit $($kit.id) FFmpeg package from $($ffmpeg.asset.url)"
        Invoke-WebRequest -Uri $ffmpeg.asset.url -OutFile $archive
    } else {
        $archive = (Resolve-Path -LiteralPath $ArchivePath).Path
    }

    $archiveHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($archiveHash -ne $ffmpeg.asset.sha256.ToLowerInvariant()) {
        throw "FFmpeg package SHA-256 mismatch. Expected $($ffmpeg.asset.sha256); got $archiveHash. Review the approved runtime package before changing the pin."
    }

    $extractRoot = Join-Path $temporaryDirectory 'extracted'
    Expand-Archive -LiteralPath $archive -DestinationPath $extractRoot

    foreach ($name in $requiredNames) {
        $fileCandidates = @(Get-ChildItem -LiteralPath $extractRoot -File -Recurse | Where-Object { $_.Name -eq $name })
        if ($fileCandidates.Count -ne 1) { throw "Recommended FFmpeg package must contain exactly one $name; found $($fileCandidates.Count)." }
        $source = $fileCandidates[0].FullName
        $extractedRuntimeFiles[$name] = $source
        $expected = @($ffmpeg.runtimeFiles | Where-Object { $_.fileName -eq $name }) | Select-Object -First 1
        if ($null -eq $expected) { throw "Runtime manifest has no hash record for $name." }
        $actualHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actualHash -ne $expected.sha256.ToLowerInvariant()) {
            throw "$name SHA-256 mismatch. Expected $($expected.sha256); got $actualHash. The runtime package contents changed despite the asset pin."
        }
    }

    foreach ($notice in @($ffmpegProvenance.notices)) {
        $trackedNotice = Join-Path $repositoryRoot $notice.path
        if (-not (Test-Path -LiteralPath $trackedNotice -PathType Leaf)) { throw "Tracked FFmpeg notice is missing: $($notice.path)." }
        $trackedNoticeHash = (Get-FileHash -LiteralPath $trackedNotice -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($trackedNoticeHash -ne $notice.sha256.ToLowerInvariant()) {
            throw "FFmpeg notice hash mismatch for $($notice.path). Review the pinned archive and tracked notice before staging."
        }
    }

    foreach ($record in @($ffmpeg.runtimeFiles)) {
        $versionOutput = (& ($extractedRuntimeFiles[$record.fileName]) -hide_banner -version 2>&1) -join "`n"
        if ($LASTEXITCODE -ne 0 -or $versionOutput -notmatch [regex]::Escape($ffmpeg.requiredVersionText)) {
            throw "The selected $($record.fileName) does not report the approved kit version."
        }
    }
    $protocolOutput = (& ($extractedRuntimeFiles['ffmpeg.exe']) -hide_banner -protocols 2>&1) -join "`n"
    if ($LASTEXITCODE -ne 0 -or -not [regex]::IsMatch($protocolOutput, '(?m)^\s*srt\s*$')) {
        throw 'The pinned FFmpeg does not advertise the SRT protocol.'
    }

    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    foreach ($name in $requiredNames) {
        Copy-Item -LiteralPath $extractedRuntimeFiles[$name] -Destination (Join-Path $destination $name) -Force
    }

    foreach ($notice in @($ffmpegProvenance.notices)) {
        $source = Join-Path $repositoryRoot $notice.path
        $destinationNotice = Join-Path $destination ([System.IO.Path]::GetFileName([string]$notice.path))
        if (-not [string]::Equals(
            [System.IO.Path]::GetFullPath($source),
            [System.IO.Path]::GetFullPath($destinationNotice),
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            Copy-Item -LiteralPath $source -Destination $destinationNotice -Force
        }
    }
    Remove-Item -LiteralPath (Join-Path $destination 'README-BtbN-build.txt') -Force -ErrorAction SilentlyContinue

    foreach ($name in $requiredNames) {
        $stagedPath = Join-Path $destination $name
        $stagedHash = (Get-FileHash -LiteralPath $stagedPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $expected = @($ffmpeg.runtimeFiles | Where-Object { $_.fileName -eq $name }) | Select-Object -First 1
        if ($stagedHash -ne $expected.sha256.ToLowerInvariant()) { throw "Staged $name failed its SHA-256 check." }
    }

    Write-Host "Staged kit $($kit.id) FFmpeg $($ffmpeg.version) and ffprobe in $destination"
    Write-Host "Package SHA-256: $archiveHash"
    Write-Host 'SRT protocol: present. NDI Runtime remains installed separately by the user.'
} finally {
    if (Test-Path -LiteralPath $temporaryDirectory) {
        $temporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
        $temporaryPrefix = $temporaryRoot + [System.IO.Path]::DirectorySeparatorChar
        $resolvedTemporaryDirectory = [System.IO.Path]::GetFullPath($temporaryDirectory)
        $temporaryLeaf = [System.IO.Path]::GetFileName($resolvedTemporaryDirectory)
        if (-not $resolvedTemporaryDirectory.StartsWith($temporaryPrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
            $resolvedTemporaryDirectory -eq $temporaryRoot -or
            $temporaryLeaf -notmatch '^qlisa-runtime-[0-9a-fA-F]{32}$') {
            throw "Refusing to remove unexpected runtime temporary directory: $resolvedTemporaryDirectory"
        }
        Remove-Item -LiteralPath $resolvedTemporaryDirectory -Recurse -Force
    }
}
