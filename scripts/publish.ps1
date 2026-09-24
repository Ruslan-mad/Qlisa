<#
.SYNOPSIS
Prepare a signed Qlisa Windows release locally.

.DESCRIPTION
Checks source and release prerequisites, runs frontend and Rust checks, updates
the version through scripts/release.mjs, and creates local NSIS/updater
artifacts for private testing. It never tags, pushes, or publishes. Missing
signing keys stop the process before a build starts. Media binaries are fetched
by Qlisa after installation and are not release build inputs.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$DryRun,
    [string]$SigningKeyPath = (Join-Path $env:USERPROFILE '.tauri\qlisa.key')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$script:Root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$script:RepoUrl = 'https://github.com/Ruslan-mad/Qlisa'
$script:UpdaterEndpoint = "$script:RepoUrl/releases/latest/download/latest.json"

function Fail([string]$Message) { throw $Message }
function Resolve-RepoPath([string]$Path) {
    if ([System.IO.Path]::IsPathRooted($Path)) { return [System.IO.Path]::GetFullPath($Path) }
    return [System.IO.Path]::GetFullPath((Join-Path $script:Root $Path))
}
function Invoke-Checked([string]$Name, [string]$Executable, [string[]]$Arguments, [string]$Directory = $script:Root) {
    Write-Host "RUN: $Name"
    Push-Location -LiteralPath $Directory
    try {
        if ([System.IO.Path]::GetExtension($Executable) -ieq '.ps1') {
            $powershell = Get-Command powershell.exe -ErrorAction SilentlyContinue
            if ($null -eq $powershell) { Fail 'Windows PowerShell was not found in PATH.' }
            & $powershell.Source -NoProfile -ExecutionPolicy Bypass -File $Executable @Arguments
            if ($LASTEXITCODE -ne 0) { Fail "$Name failed with exit code $LASTEXITCODE." }
        } else {
            & $Executable @Arguments
            if ($LASTEXITCODE -ne 0) { Fail "$Name failed with exit code $LASTEXITCODE." }
        }
    } finally { Pop-Location }
}
function Get-VersionState {
    $package = Get-Content -Raw (Join-Path $script:Root 'package.json') | ConvertFrom-Json
    $tauri = Get-Content -Raw (Join-Path $script:Root 'src-tauri/tauri.conf.json') | ConvertFrom-Json
    $cargo = Get-Content -Raw (Join-Path $script:Root 'src-tauri/Cargo.toml')
    $lock = Get-Content -Raw (Join-Path $script:Root 'src-tauri/Cargo.lock')
    $cargoMatch = [regex]::Match($cargo, '(?m)^version\s*=\s*"([^"]+)"')
    $lockMatch = [regex]::Match($lock, '(?ms)^\[\[package\]\]\s*\r?\nname\s*=\s*"qlisa"\s*\r?\nversion\s*=\s*"([^"]+)"')
    if (-not $cargoMatch.Success -or -not $lockMatch.Success) { Fail 'Could not read all project version fields.' }
    $values = @([string]$package.version, [string]$tauri.version, $cargoMatch.Groups[1].Value, $lockMatch.Groups[1].Value)
    if (@($values | Select-Object -Unique).Count -ne 1) { Fail "Project version files disagree: $($values -join ', ')." }
    return $values[0]
}
function Compare-SemVer([string]$Left, [string]$Right) {
    $a = $Left.Split('.') | ForEach-Object { [long]$_ }
    $b = $Right.Split('.') | ForEach-Object { [long]$_ }
    for ($i = 0; $i -lt 3; $i++) { if ($a[$i] -lt $b[$i]) { return -1 }; if ($a[$i] -gt $b[$i]) { return 1 } }
    return 0
}
function Get-Sha256([string]$Path) { return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Get-RuntimeManifest {
    $manifestPath = Join-Path $script:Root 'scripts/runtime-manifest.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { Fail "Tracked runtime manifest is missing: $manifestPath" }
    $runtime = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    if ($runtime.schemaVersion -ne 1 -or $runtime.components -isnot [array]) { Fail 'scripts/runtime-manifest.json must have schemaVersion=1 and a components array.' }
    $ffmpeg = @($runtime.components | Where-Object { $_.id -eq 'ffmpeg-btbn-gpl-n9.0' })
    $mpv = @($runtime.components | Where-Object { $_.id -eq 'libmpv' })
    if ($ffmpeg.Count -ne 1 -or $mpv.Count -ne 1) { Fail 'Runtime manifest must contain exactly one pinned FFmpeg and one libmpv record.' }
    if ([string]$ffmpeg[0].archive.url -notmatch '^https://github\.com/BtbN/FFmpeg-Builds/releases/download/' -or
        [string]$ffmpeg[0].archive.sha256 -notmatch '^[a-fA-F0-9]{64}$' -or
        [string]$mpv[0].build.archiveUrl -notmatch '^https://github\.com/shinchiro/mpv-winbuild-cmake/releases/download/' -or
        [string]$mpv[0].build.archiveSha256 -notmatch '^[a-fA-F0-9]{64}$') { Fail 'Runtime manifest upstream URLs or archive SHA-256 pins are incomplete.' }
    return [pscustomobject]@{ Ffmpeg = $ffmpeg[0]; Mpv = $mpv[0] }
}
function Assert-BundleConfiguration {
    $windows = Get-Content -Raw (Join-Path $script:Root 'src-tauri/tauri.windows.conf.json') | ConvertFrom-Json
    $config = Get-Content -Raw (Join-Path $script:Root 'src-tauri/tauri.conf.json') | ConvertFrom-Json
    $resourceText = $windows.bundle.resources | ConvertTo-Json -Depth 10 -Compress
    if ($resourceText -match '(?i)ffmpeg\.exe|ffprobe\.exe|libmpv-2\.dll') { Fail 'Windows bundle resources must not contain FFmpeg, ffprobe, or libmpv binaries.' }
    if ($windows.bundle.windows.nsis.installMode -ne 'perMachine') { Fail 'NSIS bundle.windows.nsis.installMode must be perMachine.' }
    if ($config.plugins.updater.windows.installMode -ne 'passive') { Fail 'Updater installMode must remain passive, separately from NSIS installMode.' }
}
function Require-Command([string]$Name, [string]$InstallHint) {
    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -eq $command) { Fail "$Name was not found in PATH. $InstallHint" }
    return $command
}
function Assert-NoNdiInTree {
    $skip = @('.git', 'node_modules', '.pnpm-store', 'target', 'dist', 'coverage', 'build')
    $stack = [System.Collections.Generic.Stack[string]]::new(); $stack.Push($script:Root)
    while ($stack.Count -gt 0) {
        $directory = $stack.Pop()
        foreach ($file in Get-ChildItem -LiteralPath $directory -File -Filter 'Processing.NDI.Lib*.dll' -ErrorAction SilentlyContinue) {
            Fail "NDI Runtime DLL must not be included: $($file.FullName)"
        }
        foreach ($child in Get-ChildItem -LiteralPath $directory -Directory -Force -ErrorAction SilentlyContinue) {
            if ($skip -notcontains $child.Name) { $stack.Push($child.FullName) }
        }
    }
}
function Assert-NoMediaBinariesInInstaller([string]$Path) {
    $archiver = Get-Command 7z.exe -ErrorAction SilentlyContinue
    if ($null -eq $archiver) { $archiver = Get-Command 7zz.exe -ErrorAction SilentlyContinue }
    if ($null -eq $archiver) {
        Write-Warning '7-Zip is unavailable; installer payload could not be listed. Tauri resource configuration was checked.'
        return
    }
    $listing = & $archiver.Source l -slt $Path 2>&1
    if ($LASTEXITCODE -ne 0) { Fail "Could not inspect installer contents with 7-Zip: $Path" }
    if (($listing -join "`n") -match '(?im)^Path = .*?(?:ffmpeg\.exe|ffprobe\.exe|libmpv-2\.dll)\s*$') {
        Fail 'Final NSIS installer contains a media runtime binary.'
    }
}
if ($Version -notmatch '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$') { Fail "Version must be strict three-part SemVer, for example 1.5.3; got '$Version'." }
if ($env:OS -ne 'Windows_NT') { Fail 'Windows release preparation must run on Windows.' }
$git = Require-Command 'git' 'Install Git for Windows with: winget install --id Git.Git -e. Then reopen PowerShell.'
$pnpm = Get-Command pnpm.cmd -ErrorAction SilentlyContinue
if ($null -eq $pnpm) { $pnpm = Get-Command pnpm -ErrorAction SilentlyContinue }
if ($null -eq $pnpm) { Fail 'pnpm was not found in PATH. Install Node.js LTS, then run `npm install --global pnpm`; reopen PowerShell.' }
$node = Get-Command node.exe -ErrorAction SilentlyContinue
if ($null -eq $node) { $node = Get-Command node -ErrorAction SilentlyContinue }
if ($null -eq $node) { Fail 'Node.js was not found in PATH. Install it with `winget install --id OpenJS.NodeJS.LTS -e`, then reopen PowerShell.' }
$cargo = Require-Command 'cargo' 'Install Rust with `winget install --id Rustlang.Rustup -e`, then run `rustup default stable-x86_64-pc-windows-msvc` and reopen PowerShell.'
$rustc = Require-Command 'rustc' 'Install Rust with `winget install --id Rustlang.Rustup -e`, then run `rustup default stable-x86_64-pc-windows-msvc` and reopen PowerShell.'
$branch = (& $git.Source -C $script:Root branch --show-current).Trim()
if ($LASTEXITCODE -ne 0 -or $branch -ne 'main') { Fail "Release preparation requires branch 'main'; current branch is '$branch'." }
$status = @(& $git.Source -C $script:Root status --porcelain --untracked-files=all)
if ($LASTEXITCODE -ne 0) { Fail 'Could not inspect Git working tree.' }
if ($status.Count -gt 0) { Fail "Git working tree must be clean before release preparation:`n$($status -join "`n")" }
$currentVersion = Get-VersionState
if ((Compare-SemVer $Version $currentVersion) -le 0) { Fail "Requested version $Version must be greater than current version $currentVersion." }
$tag = "v$Version"
$releaseNotes = Join-Path $script:Root "docs/RELEASE_NOTES_$Version.md"
if (-not (Test-Path -LiteralPath $releaseNotes -PathType Leaf)) { Fail "Release notes are missing: $releaseNotes. Create reviewed notes before preparing the installer." }

$config = Get-Content -Raw (Join-Path $script:Root 'src-tauri/tauri.conf.json') | ConvertFrom-Json
$updater = $config.plugins.updater
if ($config.bundle.createUpdaterArtifacts -ne $true) { Fail 'Tauri bundle.createUpdaterArtifacts must be true.' }
if ([string]::IsNullOrWhiteSpace([string]$updater.pubkey) -or $updater.pubkey -match '(?i)placeholder|example|inkue') { Fail 'A Qlisa updater public key must be configured in src-tauri/tauri.conf.json.' }
if (@($updater.endpoints | Where-Object { [string]$_ -eq $script:UpdaterEndpoint }).Count -ne 1) { Fail "Tauri updater endpoint must include $script:UpdaterEndpoint" }
$privateKey = Resolve-RepoPath $SigningKeyPath
$repoPrefix = $script:Root.TrimEnd('\') + '\'
if ($privateKey.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase)) { Fail 'Signing key must be outside the repository.' }
if (-not (Test-Path -LiteralPath $privateKey -PathType Leaf)) { Fail "Tauri signing key is missing: $privateKey. The script will not create or print a key." }
$runtime = Get-RuntimeManifest
Assert-BundleConfiguration
if (-not (Test-Path -LiteralPath (Join-Path $script:Root 'node_modules/.bin/tauri.cmd') -PathType Leaf)) {
    Fail 'Project-local Tauri CLI is missing. Run `pnpm install --frozen-lockfile` from the repository root, then reopen PowerShell.'
}
Invoke-Checked 'Check Tauri CLI' $pnpm.Source @('exec', 'tauri', '--version')
Assert-NoNdiInTree

Write-Host "Repository: $script:Root"
Write-Host "Branch: $branch"
Write-Host "Source commit: $((& $git.Source -C $script:Root rev-parse HEAD).Trim())"
Write-Host "Version: $currentVersion -> $Version"
Write-Host "Tag to use after review: $tag"
Write-Host "Release notes: $releaseNotes"
Write-Host 'Prerequisites: node, pnpm, cargo, rustc, local Tauri CLI, Git, signing key, and runtime pins are valid.'
if ($DryRun) {
    Write-Host 'PREFLIGHT ONLY: no version files changed and no installer was built. Media runtime binaries are not required for release packaging.'
    exit 0
}

Invoke-Checked 'Frontend tests' $pnpm.Source @('test')
Invoke-Checked 'Frontend production build' $pnpm.Source @('build')
Invoke-Checked 'Rust metadata' 'cargo' @('metadata', '--locked', '--no-deps', '--format-version', '1') (Join-Path $script:Root 'src-tauri')
Invoke-Checked 'Rust check' 'cargo' @('check', '--locked', '--features', 'asio-support') (Join-Path $script:Root 'src-tauri')
Invoke-Checked 'Rust tests' 'cargo' @('test', '--locked', '--features', 'asio-support') (Join-Path $script:Root 'src-tauri')
Invoke-Checked 'Rust clippy' 'cargo' @('clippy', '--locked', '--all-targets', '--features', 'asio-support') (Join-Path $script:Root 'src-tauri')

$oldPrivateKey = $env:TAURI_SIGNING_PRIVATE_KEY
$oldPassword = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
$oldPnpm = $env:QLISA_PNPM_EXECUTABLE
$env:QLISA_PNPM_EXECUTABLE = $pnpm.Source
try {
    $env:TAURI_SIGNING_PRIVATE_KEY = $privateKey
    if ([string]::IsNullOrEmpty($env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD)) {
        $securePassword = Read-Host 'Tauri signing key password' -AsSecureString
        if ($securePassword.Length -eq 0) { Fail 'A non-empty Tauri signing key password is required.' }
        $passwordPointer = [IntPtr]::Zero
        try {
            $passwordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($securePassword)
            $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($passwordPointer)
        } finally {
            if ($passwordPointer -ne [IntPtr]::Zero) { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($passwordPointer) }
            $securePassword.Dispose()
        }
    }
    Invoke-Checked 'Release version update and production NSIS build' $node.Source @((Join-Path $script:Root 'scripts/release.mjs'), $Version, '--prepare-release', '--bundle=nsis')
} finally {
    if ($null -eq $oldPrivateKey) { Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue } else { $env:TAURI_SIGNING_PRIVATE_KEY = $oldPrivateKey }
    if ($null -eq $oldPassword) { Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue } else { $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $oldPassword }
    if ($null -eq $oldPnpm) { Remove-Item Env:QLISA_PNPM_EXECUTABLE -ErrorAction SilentlyContinue } else { $env:QLISA_PNPM_EXECUTABLE = $oldPnpm }
}

$buildManifestPath = Join-Path $script:Root 'src-tauri/target/release/qlisa.release.json'
if (-not (Test-Path -LiteralPath $buildManifestPath -PathType Leaf)) { Fail 'release.mjs did not produce its release manifest.' }
$buildManifest = Get-Content -Raw -LiteralPath $buildManifestPath | ConvertFrom-Json
if ($buildManifest.version -ne $Version -or $null -eq $buildManifest.installer -or
    $null -eq $buildManifest.updaterBundle -or $null -eq $buildManifest.updaterSignature) {
    Fail 'release.mjs manifest is stale or lacks the fresh NSIS installer/updater signature records.'
}
$installerPath = [string]$buildManifest.installer.path
$updateBundlePath = [string]$buildManifest.updaterBundle.path
$signaturePath = [string]$buildManifest.updaterSignature.path
foreach ($artifactPath in @($installerPath, $updateBundlePath, $signaturePath)) {
    if (-not (Test-Path -LiteralPath $artifactPath -PathType Leaf)) { Fail "Release manifest artifact is missing: $artifactPath" }
}
if ($updateBundlePath -cne $installerPath -or -not $updateBundlePath.ToLowerInvariant().EndsWith('.exe') -or $signaturePath -cne "$updateBundlePath.sig") {
    Fail 'Release manifest does not identify the NSIS installer and its matching Tauri updater signature.'
}
$installer = Get-Item -LiteralPath $installerPath
Assert-NoMediaBinariesInInstaller $installerPath

$signature = Get-Content -Raw -LiteralPath $signaturePath
if ([string]::IsNullOrWhiteSpace($signature) -or $signature -cne $signature.Trim()) { Fail 'Generated updater signature is empty or has surrounding whitespace; expected exact Tauri .sig content.' }
$outDirectory = Join-Path $script:Root "src-tauri/target/release/prepared/$tag"
New-Item -ItemType Directory -Path $outDirectory -Force | Out-Null
$latestPath = Join-Path $outDirectory 'latest.json'
$assetUrl = "$script:RepoUrl/releases/latest/download/$($installer.Name)"
Invoke-Checked 'Create updater latest.json' $node.Source @((Join-Path $script:Root 'scripts/release.mjs'), '--write-updater-metadata', $Version, $releaseNotes, $assetUrl, $signaturePath, $latestPath)
$parsed = Get-Content -Raw -LiteralPath $latestPath | ConvertFrom-Json
$expectedUrl = "$script:RepoUrl/releases/latest/download/$($installer.Name)"
$expectedNotes = (Get-Content -Raw -LiteralPath $releaseNotes).Trim()
$pubDate = [datetimeoffset]$parsed.pub_date
if ($parsed.version -cne $Version -or [string]$parsed.platforms.'windows-x86_64'.signature -cne $signature -or
    [string]$parsed.platforms.'windows-x86_64'.url -cne $expectedUrl -or
    [string]$parsed.notes -cne $expectedNotes -or
    $pubDate.Offset -ne [timespan]::Zero -or
    @($parsed.platforms.PSObject.Properties.Name) -notcontains 'windows-x86_64') {
    Fail 'Generated latest.json failed version, Windows platform, exact URL, exact signature text, UTC publication date, or release-notes checks.'
}

$assets = @($installer.FullName, $signaturePath, $latestPath)
$assetNames = @($assets | ForEach-Object { [System.IO.Path]::GetFileName($_) })
if (@($assetNames | Select-Object -Unique).Count -ne $assetNames.Count) { Fail 'Release assets contain duplicate filenames; rename the colliding source or notice files before retrying.' }
Write-Host ''
Write-Host "Prepared version: $Version"
Write-Host "Tag: $tag (not created)"
Write-Host "Source commit: $((& $git.Source -C $script:Root rev-parse HEAD).Trim())"
Write-Host 'Release assets:'
foreach ($asset in $assets) {
    $file = Get-Item -LiteralPath $asset
    Write-Host "  $($file.FullName)  size=$($file.Length) sha256=$(Get-Sha256 $file.FullName)"
}
Write-Host "Release notes: $releaseNotes"
Write-Host 'Release notes content:'
Write-Host (Get-Content -Raw -LiteralPath $releaseNotes)
Write-Host "Metadata: $latestPath"
Write-Host 'LOCAL-ONLY SIGNED BUILD COMPLETE. These artifacts are not a publication-ready release. Nothing was tagged, pushed, or published.'
