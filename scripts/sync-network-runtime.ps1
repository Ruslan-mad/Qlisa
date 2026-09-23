<#
.SYNOPSIS
Prepare the pinned Windows FFmpeg runtime used by local and release builds.

.DESCRIPTION
Downloads a versioned Gyan FFmpeg essentials archive, verifies SHA-256, and
extracts ffmpeg.exe, ffprobe.exe, and the accompanying notices. NDI Runtime is
installed separately by the user and is never copied into Qlisa.
#>
[CmdletBinding()]

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$vendorRoot = Join-Path $repositoryRoot 'src-tauri\vendor'
$ffmpegDestination = Join-Path $vendorRoot 'ffmpeg'
$ffmpegVersion = '9.0.1'
$ffmpegArchiveUrl = 'https://github.com/GyanD/codexffmpeg/releases/download/9.0.1/ffmpeg-9.0.1-essentials_build.zip'
$ffmpegSha256 = 'fec81ae03971d9dd4be3ebe02e263bd2ec1d789483f931bdba5f5715e65da2e9'

New-Item -ItemType Directory -Force -Path $ffmpegDestination | Out-Null

$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ('qlisa-ffmpeg-' + [guid]::NewGuid())
try {
    New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
    $archive = Join-Path $temporaryDirectory 'ffmpeg-release-essentials.zip'
    Invoke-WebRequest -Uri $ffmpegArchiveUrl -OutFile $archive
    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
    if ($actualHash -ne $ffmpegSha256) {
        throw "FFmpeg checksum mismatch. Expected $ffmpegSha256; got $actualHash. Review the new Gyan build before updating this script."
    }
    Expand-Archive -LiteralPath $archive -DestinationPath $temporaryDirectory
    $buildRoot = Get-ChildItem -LiteralPath $temporaryDirectory -Directory | Where-Object Name -Match "^ffmpeg-$([regex]::Escape($ffmpegVersion))-essentials_build$" | Select-Object -First 1
    if ($null -eq $buildRoot) { throw 'Unexpected FFmpeg archive layout.' }
    Copy-Item -LiteralPath (Join-Path $buildRoot.FullName 'bin\ffmpeg.exe') -Destination (Join-Path $ffmpegDestination 'ffmpeg.exe') -Force
    Copy-Item -LiteralPath (Join-Path $buildRoot.FullName 'bin\ffprobe.exe') -Destination (Join-Path $ffmpegDestination 'ffprobe.exe') -Force
    Copy-Item -LiteralPath (Join-Path $buildRoot.FullName 'LICENSE') -Destination (Join-Path $ffmpegDestination 'LICENSE') -Force
    Copy-Item -LiteralPath (Join-Path $buildRoot.FullName 'README.txt') -Destination (Join-Path $ffmpegDestination 'README-Gyan-build.txt') -Force
} finally {
    if (Test-Path -LiteralPath $temporaryDirectory) { Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force }
}

$protocols = (& (Join-Path $ffmpegDestination 'ffmpeg.exe') -hide_banner -protocols 2>&1) -join "`n"
if (-not [regex]::IsMatch($protocols, '(?m)^\s*srt\s*$')) { throw 'The pinned FFmpeg does not advertise the SRT protocol.' }
Write-Host "Pinned FFmpeg $ffmpegVersion runtime prepared successfully. NDI Runtime remains a user-installed dependency."
