<#
.SYNOPSIS
Prepare hash-verified local inputs for the full Windows installer.

.DESCRIPTION
Stages the pinned media runtime, Vulkan loader, and official NDI redist under
src-tauri/target. The full Tauri config embeds these inputs and an offline
WebView2 installer. No runtime binaries are written to tracked source files.
#>
[CmdletBinding()]
param(
    [string]$NdiInstallerPath,
    [string]$RuntimeDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$manifestPath = Join-Path $PSScriptRoot 'full-installer-manifest.json'
$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
if ($manifest.schemaVersion -ne 1) { throw "Unsupported full-installer manifest: $manifestPath" }

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}
function Assert-Hash([string]$Path, [string]$Expected, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "$Label is missing: $Path" }
    $actual = Get-Sha256 $Path
    if ($actual -ne $Expected.ToLowerInvariant()) { throw "$Label SHA-256 mismatch. Expected $Expected; got $actual." }
}
function Get-PinnedFile([string]$Url, [string]$ExpectedHash, [string]$CachePath, [string]$Label) {
    if (Test-Path -LiteralPath $CachePath -PathType Leaf) {
        if ((Get-Sha256 $CachePath) -eq $ExpectedHash.ToLowerInvariant()) { return $CachePath }
        Remove-Item -LiteralPath $CachePath -Force
    }
    Write-Host "Downloading pinned $Label from $Url"
    Invoke-WebRequest -Uri $Url -OutFile $CachePath -TimeoutSec 600 -UseBasicParsing
    Assert-Hash $CachePath $ExpectedHash $Label
    return $CachePath
}
function Assert-Authenticode([string]$Path, [string]$ExpectedSigner, [string]$Label) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch [regex]::Escape($ExpectedSigner)) {
        throw "$Label must have a valid Authenticode signature from $ExpectedSigner; status=$($signature.Status), subject=$($signature.SignerCertificate.Subject)."
    }
}

$payload = Join-Path $root 'src-tauri/target/full-installer-payload'
$runtimePayload = Join-Path $payload 'full-runtime'
$ndiPayload = Join-Path $payload 'ndi'
New-Item -ItemType Directory -Path $runtimePayload -Force | Out-Null
New-Item -ItemType Directory -Path $ndiPayload -Force | Out-Null

$runtimeManifest = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'runtime-manifest.json') | ConvertFrom-Json
$ffmpeg = @($runtimeManifest.components | Where-Object { $_.id -eq 'ffmpeg-gyan-full-9.0.2' })
$mpv = @($runtimeManifest.components | Where-Object { $_.id -eq 'libmpv' })
if ($ffmpeg.Count -ne 1 -or $mpv.Count -ne 1) { throw 'Expected one pinned FFmpeg and one pinned libmpv entry.' }

$ffmpegDirectory = Join-Path $root 'src-tauri/vendor/ffmpeg'
if (-not (Test-Path -LiteralPath (Join-Path $ffmpegDirectory 'ffmpeg.exe') -PathType Leaf) -or
    -not (Test-Path -LiteralPath (Join-Path $ffmpegDirectory 'ffprobe.exe') -PathType Leaf)) {
    Write-Host 'Staging the pinned FFmpeg build before preparing the full installer.'
    & (Join-Path $PSScriptRoot 'prepare-runtime.ps1')
}

$runtimeDirectoryCandidates = @()
if (-not [string]::IsNullOrWhiteSpace($RuntimeDirectory)) { $runtimeDirectoryCandidates += [System.IO.Path]::GetFullPath($RuntimeDirectory) }
if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) { $runtimeDirectoryCandidates += (Join-Path $env:LOCALAPPDATA 'Qlisa/runtime') }
$runtimeDirectoryCandidates = @($runtimeDirectoryCandidates | Select-Object -Unique)
$vendorFiles = @{
    'ffmpeg.exe' = Join-Path $root 'src-tauri/vendor/ffmpeg/ffmpeg.exe'
    'ffprobe.exe' = Join-Path $root 'src-tauri/vendor/ffmpeg/ffprobe.exe'
    'libmpv-2.dll' = Join-Path $root 'src-tauri/vendor/mpv/libmpv-2.dll'
}
$records = @($manifest.runtimeFiles)
$ffmpegReady = $true
foreach ($name in @('ffmpeg.exe', 'ffprobe.exe')) {
    $record = $records | Where-Object { $_.fileName -eq $name } | Select-Object -First 1
    if (-not (Test-Path -LiteralPath $vendorFiles[$name] -PathType Leaf) -or (Get-Sha256 $vendorFiles[$name]) -ne $record.sha256.ToLowerInvariant()) {
        $ffmpegReady = $false
    }
}
foreach ($directory in $runtimeDirectoryCandidates) {
    if (-not $ffmpegReady) {
        $ffmpegReady = $true
        foreach ($name in @('ffmpeg.exe', 'ffprobe.exe')) {
            $record = $records | Where-Object { $_.fileName -eq $name } | Select-Object -First 1
            $candidate = Join-Path $directory $name
            if (-not (Test-Path -LiteralPath $candidate -PathType Leaf) -or (Get-Sha256 $candidate) -ne $record.sha256.ToLowerInvariant()) { $ffmpegReady = $false }
        }
        if ($ffmpegReady) { foreach ($name in @('ffmpeg.exe', 'ffprobe.exe')) { $vendorFiles[$name] = Join-Path $directory $name }; break }
    }
}
if (-not $ffmpegReady) {
    Write-Host 'Staging the pinned FFmpeg build before preparing the full installer.'
    & (Join-Path $PSScriptRoot 'prepare-runtime.ps1')
    foreach ($name in @('ffmpeg.exe', 'ffprobe.exe')) { $vendorFiles[$name] = Join-Path $root "src-tauri/vendor/ffmpeg/$name" }
}
$sourceFiles = @{
    'ffmpeg.exe' = $vendorFiles['ffmpeg.exe']
    'ffprobe.exe' = $vendorFiles['ffprobe.exe']
    'libmpv-2.dll' = $vendorFiles['libmpv-2.dll']
    'vulkan-1.dll' = Join-Path $env:WINDIR 'System32/vulkan-1.dll'
}
foreach ($record in @($manifest.runtimeFiles)) {
    $name = [string]$record.fileName
    $source = $sourceFiles[$name]
    if ($name -eq 'libmpv-2.dll' -and
        (-not (Test-Path -LiteralPath $source -PathType Leaf) -or (Get-Sha256 $source) -ne $record.sha256.ToLowerInvariant())) {
        foreach ($directory in $runtimeDirectoryCandidates) {
            $candidate = Join-Path $directory $name
            if ((Test-Path -LiteralPath $candidate -PathType Leaf) -and (Get-Sha256 $candidate) -eq $record.sha256.ToLowerInvariant()) {
                $source = $candidate
                break
            }
        }
    }
    Assert-Hash $source $record.sha256 $name
    if ($name -eq 'vulkan-1.dll') {
        $version = (Get-Item -LiteralPath $source).VersionInfo.FileVersion
        if ($version -ne $record.version) { throw "Vulkan loader version mismatch: expected $($record.version); got $version." }
        Assert-Authenticode $source 'Microsoft Corporation' 'Vulkan loader'
    }
    Copy-Item -LiteralPath $source -Destination (Join-Path $runtimePayload $name) -Force
}

$ndiCache = Join-Path $payload $manifest.ndiRuntime.fileName
if (-not [string]::IsNullOrWhiteSpace($NdiInstallerPath)) {
    $ndiSource = (Resolve-Path -LiteralPath $NdiInstallerPath).Path
    Assert-Hash $ndiSource $manifest.ndiRuntime.sha256 'NDI Runtime installer'
} else {
    $ndiSource = Get-PinnedFile $manifest.ndiRuntime.url $manifest.ndiRuntime.sha256 $ndiCache 'NDI Runtime installer'
}
Assert-Authenticode $ndiSource $manifest.ndiRuntime.signer 'NDI Runtime installer'
Copy-Item -LiteralPath $ndiSource -Destination (Join-Path $ndiPayload $manifest.ndiRuntime.fileName) -Force
Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $runtimePayload 'full-package.json') -Force

foreach ($record in @($manifest.runtimeFiles)) {
    Assert-Hash (Join-Path $runtimePayload $record.fileName) $record.sha256 $record.fileName
}
Assert-Hash (Join-Path $ndiPayload $manifest.ndiRuntime.fileName) $manifest.ndiRuntime.sha256 'Staged NDI Runtime installer'

Write-Host "Full installer payload prepared: $payload"
Write-Host "Runtime seed files: $(@($manifest.runtimeFiles).Count)"
Write-Host "NDI Runtime: $($manifest.ndiRuntime.version), Authenticode valid ($($manifest.ndiRuntime.signer))"
Write-Host 'WebView2: Tauri full config uses offlineInstaller mode.'
