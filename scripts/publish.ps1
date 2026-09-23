<#
.SYNOPSIS
Read-only preflight for a future Qlisa release.

.DESCRIPTION
This script does not create an installer, create a key, tag a commit, or
publish anything by default. Use -RunChecks to run local frontend and Rust
checks. Pass explicit paths with -InstallerPath and -ReleaseAssetPath to inspect
already prepared artifacts. Use -WriteInventoryPath to write a local source
inventory. -PrepareRelease is reserved for a future local-only preparation
pipeline. It currently fails closed because Qlisa updater configuration and
signing credentials are not available. This script never publishes, tags, or
contacts GitHub.
#>
[CmdletBinding()]
param(
    [string]$ExpectedVersion = '1.5.2',
    [switch]$SourceOnly,
    [switch]$RunChecks,
    [switch]$PrepareRelease,
    [ValidateSet('None', 'Nsis', 'Msi')]
    [string]$InstallerType = 'None',
    [string]$ReleaseNotesPath,
    [string]$UpdateMetadataUrl,
    [string]$UpdateArtifactUrl,
    [string]$InstallerDownloadUrl,
    [string]$PreparedBuildPath,
    [string]$InstallerPath,
    [string]$PreparedSourceCommit,
    [string[]]$ReleaseAssetPath = @(),
    [string]$UpdaterMetadataPath,
    [string]$UpdaterSignaturePath,
    [string]$SigningKeyPath,
    [string]$WriteInventoryPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:Root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$script:SourceBlockers = [System.Collections.Generic.List[string]]::new()
$script:BinaryBlockers = [System.Collections.Generic.List[string]]::new()
$script:Warnings = [System.Collections.Generic.List[string]]::new()
$script:Inventory = [System.Collections.Generic.List[object]]::new()

function Add-Blocker([string]$Message, [ValidateSet('Source', 'Binary')][string]$Readiness = 'Binary') {
    if ($Readiness -eq 'Source') { $script:SourceBlockers.Add($Message) }
    else { $script:BinaryBlockers.Add($Message) }
    Write-Host "BLOCKER: $Message" -ForegroundColor Red
}

function Add-Warning([string]$Message) {
    $script:Warnings.Add($Message)
    Write-Host "WARNING: $Message" -ForegroundColor Yellow
}

function Get-PropertyValue($Object, [string]$Name) {
    if ($null -ne $Object -and $Object.PSObject.Properties.Name -contains $Name) {
        return $Object.$Name
    }
    return $null
}

function Get-RelativePath([string]$Path) {
    $full = [System.IO.Path]::GetFullPath($Path)
    $rootPrefix = [System.IO.Path]::GetFullPath($script:Root).TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    if ($full.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        return $full.Substring($rootPrefix.Length).Replace('\', '/')
    }
    return $full
}

function Invoke-Git([string[]]$Arguments) {
    Push-Location -LiteralPath $script:Root
    try {
        $output = & git @Arguments 2>&1
        if ($LASTEXITCODE -ne 0) { throw "git $($Arguments -join ' ') failed: $output" }
        return @($output | ForEach-Object { [string]$_ })
    }
    finally { Pop-Location }
}

function Add-InventoryFile([string]$Path, [string]$Origin) {
    $relative = Get-RelativePath $Path
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        $script:Inventory.Add([pscustomobject]@{ path = $relative; origin = $Origin; size = $null; sha256 = $null; state = 'missing from working tree' })
        return
    }
    $item = Get-Item -LiteralPath $Path
    $hash = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    $script:Inventory.Add([pscustomobject]@{ path = $relative; origin = $Origin; size = $item.Length; sha256 = $hash; state = 'present' })
}

function Get-CandidatePaths {
    $headPaths = Invoke-Git @('ls-tree', '-r', '--name-only', 'HEAD')
    $indexPaths = Invoke-Git @('ls-files', '--cached')
    $untrackedPaths = Invoke-Git @('ls-files', '--others', '--exclude-standard')

    foreach ($path in $headPaths) {
        $full = Join-Path $script:Root $path
        Add-InventoryFile -Path $full -Origin 'HEAD'
    }

    foreach ($path in $indexPaths) {
        if ($headPaths -notcontains $path) {
            $full = Join-Path $script:Root $path
            Add-InventoryFile -Path $full -Origin 'index'
        }
    }

    foreach ($path in $untrackedPaths) {
        $full = Join-Path $script:Root $path
        Add-InventoryFile -Path $full -Origin 'untracked'
    }

    return [pscustomobject]@{
        Head = @($headPaths)
        Index = @($indexPaths)
        Untracked = @($untrackedPaths)
        Candidate = @($headPaths + $indexPaths + $untrackedPaths | Sort-Object -Unique)
    }
}

function Test-PrivateFileName([string]$Path) {
    $name = [System.IO.Path]::GetFileName($Path)
    if ($name -ieq '.env.example' -or $name -like '*.example') { return $false }
    return $name -match '(?i)(^\.env(?:\..*)?$|\.key$|\.pem$|\.pfx$|\.p12$|\.cer$|\.crt$|\.jks$|\.keystore$|(^|[._-])id_(rsa|ed25519|ecdsa)([._-]|$)|(^|[._-])secrets?([._-]|$)|credentials?)'
}

function Get-NdiFiles([string[]]$Paths) {
    return @($Paths | Where-Object { [System.IO.Path]::GetFileName([string]$_) -match '(?i)^Processing\.NDI\.Lib.*\.dll$' })
}

function Test-SourceTreeNdiFiles {
    $skipNames = @('.git', 'node_modules', '.pnpm-store', 'target', 'dist', 'coverage', 'build', 'tmp')
    $found = [System.Collections.Generic.List[string]]::new()
    $pending = [System.Collections.Generic.Stack[string]]::new()
    $pending.Push($script:Root)
    while ($pending.Count -gt 0) {
        $directory = $pending.Pop()
        foreach ($file in Get-ChildItem -LiteralPath $directory -File -Filter 'Processing.NDI.Lib*.dll' -ErrorAction SilentlyContinue) {
            $found.Add((Get-RelativePath $file.FullName))
        }
        foreach ($child in Get-ChildItem -LiteralPath $directory -Directory -Force -ErrorAction SilentlyContinue) {
            $relative = Get-RelativePath $child.FullName
            if ($skipNames -notcontains $child.Name -and $relative -ne 'src-tauri/vendor/network') { $pending.Push($child.FullName) }
        }
    }
    return @($found | Sort-Object -Unique)
}

function Test-SourceTreeRuntimeExes {
    $skipNames = @('.git', 'node_modules', '.pnpm-store', 'target', 'dist', 'coverage', 'build', 'tmp')
    $found = [System.Collections.Generic.List[string]]::new()
    $pending = [System.Collections.Generic.Stack[string]]::new()
    $pending.Push($script:Root)
    while ($pending.Count -gt 0) {
        $directory = $pending.Pop()
        foreach ($file in Get-ChildItem -LiteralPath $directory -File -ErrorAction SilentlyContinue) {
            $relative = Get-RelativePath $file.FullName
            if ($relative -notmatch '(?i)^src-tauri/vendor/ffmpeg/' -and $file.Name -match '(?i)^(ffmpeg|ffprobe)\.exe$') { $found.Add($relative) }
        }
        foreach ($child in Get-ChildItem -LiteralPath $directory -Directory -Force -ErrorAction SilentlyContinue) {
            $relative = Get-RelativePath $child.FullName
            if ($skipNames -notcontains $child.Name -and $relative -ne 'src-tauri/vendor/network') { $pending.Push($child.FullName) }
        }
    }
    return @($found | Sort-Object -Unique)
}

function Get-AssetFiles([string[]]$Paths) {
    $files = [System.Collections.Generic.List[string]]::new()
    foreach ($path in $Paths) {
        $full = if ([System.IO.Path]::IsPathRooted($path)) { $path } else { Join-Path $script:Root $path }
        if (-not (Test-Path -LiteralPath $full)) {
            Add-Blocker "Prepared release asset is missing: $(Get-RelativePath $full)"
            continue
        }
        $item = Get-Item -LiteralPath $full
        if ($item.PSIsContainer) {
            foreach ($file in Get-ChildItem -LiteralPath $full -File -Recurse) { $files.Add($file.FullName) }
        } else {
            $files.Add($item.FullName)
        }
    }
    return @($files | Sort-Object -Unique)
}

function Test-ArchiveForNdi([string]$Path) {
    $extension = [System.IO.Path]::GetExtension($Path).ToLowerInvariant()
    if ($extension -notin @('.exe', '.msi', '.zip', '.7z', '.tar', '.gz')) { return }
    $sevenZip = Get-Command '7z.exe' -ErrorAction SilentlyContinue
    if ($null -eq $sevenZip) { $sevenZip = Get-Command '7z' -ErrorAction SilentlyContinue }
    if ($null -eq $sevenZip) {
        Add-Blocker "Cannot inspect archive contents without 7-Zip: $(Get-RelativePath $Path)"
        return
    }
    $listing = & $sevenZip.Source 'l' '-ba' $Path 2>&1
    if ($LASTEXITCODE -ne 0) {
        Add-Blocker "Could not inspect prepared asset: $(Get-RelativePath $Path)"
        return
    }
    if (($listing | Out-String) -match '(?i)Processing\.NDI\.Lib.*\.dll') {
        Add-Blocker "Prepared asset contains an NDI Runtime DLL: $(Get-RelativePath $Path)"
    }
}

function Test-PreparedFreshness([string]$Path, [string]$Label) {
    if ([string]::IsNullOrWhiteSpace($PreparedSourceCommit)) {
        Add-Blocker "$Label freshness cannot be established without -PreparedSourceCommit."
        return
    }
    $head = (Invoke-Git @('rev-parse', 'HEAD'))[0]
    if ($PreparedSourceCommit -ne $head) {
        Add-Blocker "$Label was prepared from a different commit than HEAD."
        return
    }
    $headTimestampText = (Invoke-Git @('show', '-s', '--format=%cI', 'HEAD'))[0]
    $headTimestamp = [datetimeoffset]::Parse($headTimestampText).UtcDateTime
    $item = Get-Item -LiteralPath $Path
    if ($item.LastWriteTimeUtc -lt $headTimestamp) {
        Add-Blocker "$Label is older than the source commit timestamp."
    } else {
        Write-Host "OK: $Label timestamp is at or after HEAD commit $head." -ForegroundColor Green
    }
}

function Write-InventoryArtifact {
    if (-not $WriteInventoryPath) { return }
    $destination = if ([System.IO.Path]::IsPathRooted($WriteInventoryPath)) { [System.IO.Path]::GetFullPath($WriteInventoryPath) } else { [System.IO.Path]::GetFullPath((Join-Path $script:Root $WriteInventoryPath)) }
    if (Test-Path -LiteralPath $destination) {
        Add-Blocker "Inventory output already exists; choose a new path: $(Get-RelativePath $destination)"
        return
    }
    $rootPrefix = [System.IO.Path]::GetFullPath($script:Root).TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    if ($destination.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        $relative = Get-RelativePath $destination
        & git -C $script:Root check-ignore -q -- $relative
        if ($LASTEXITCODE -ne 0) {
            Add-Blocker 'An in-repository inventory output must be in a Git-ignored directory.'
            return
        }
    }
    $parent = Split-Path -Parent $destination
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    $script:Inventory | Sort-Object path, origin | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $destination -Encoding utf8
    Write-Host "Wrote local inventory: $(Get-RelativePath $destination)"
}

function Invoke-CheckCommand([string]$Name, [string]$Executable, [string[]]$Arguments, [string]$WorkingDirectory) {
    Write-Host "RUN: $Name"
    Push-Location -LiteralPath $WorkingDirectory
    try {
        $null = & $Executable @Arguments 2>&1
        $exitCode = $LASTEXITCODE
    }
    catch {
        $exitCode = 1
    }
    finally { Pop-Location }

    if ($exitCode -ne 0) {
        if ($Name -eq 'cargo test' -and $exitCode -in @(-1073741511, 3221225785)) {
            Add-Blocker "$Name failed with Windows status 0xc0000139; Rust tests did not run."
        } else {
            Add-Blocker "$Name failed with exit code $exitCode."
        }
    } else {
        Write-Host "OK: $Name passed." -ForegroundColor Green
    }
}

Write-Host "Qlisa release preflight (read-only; no installer or publication actions)."
Write-Host "Repository: $script:Root"
Write-Host "Expected version: $ExpectedVersion"

# Require the currently approved version until a separate release task changes it.
if ($ExpectedVersion -ne '1.5.2') {
    Add-Blocker 'This preflight is pinned to version 1.5.2. Update the release plan before changing the expected version.'
}
if ($PrepareRelease -and $InstallerType -eq 'None') {
    Add-Blocker 'Prepare mode requires exactly one installer type: Nsis or Msi.'
}
if ($SourceOnly -and ($RunChecks -or $PrepareRelease -or $WriteInventoryPath)) {
    Add-Blocker '-SourceOnly is read-only and cannot be combined with -RunChecks, -PrepareRelease, or -WriteInventoryPath.' 'Source'
}
if ($PrepareRelease -and $ExpectedVersion -ne '1.5.2') {
    Add-Blocker 'Prepare mode cannot change the project version; version 1.5.2 must remain synchronized.'
}
if ($PrepareRelease) {
    foreach ($entry in @(
        @{ Name = 'updater metadata URL'; Value = $UpdateMetadataUrl },
        @{ Name = 'update bundle URL'; Value = $UpdateArtifactUrl },
        @{ Name = 'installer download URL'; Value = $InstallerDownloadUrl }
    )) {
        $uri = $null
        if (-not [uri]::TryCreate([string]$entry.Value, [System.UriKind]::Absolute, [ref]$uri) -or $uri.Scheme -ne 'https') {
            Add-Blocker "Prepare mode requires an explicit HTTPS $($entry.Name)."
        }
    }
    if (-not $ReleaseNotesPath) { $ReleaseNotesPath = "docs/RELEASE_NOTES_$ExpectedVersion.md" }
    $releaseNotesResolved = if ([System.IO.Path]::IsPathRooted($ReleaseNotesPath)) { $ReleaseNotesPath } else { Join-Path $script:Root $ReleaseNotesPath }
    if (-not (Test-Path -LiteralPath $releaseNotesResolved -PathType Leaf)) { Add-Blocker 'Release notes file is missing.' }
}

# The worktree must be clean before a release commit is prepared.
try {
    $status = Invoke-Git @('status', '--porcelain', '--untracked-files=all')
    if (@($status).Count -gt 0) {
        Add-Blocker "Git working tree is not clean ($(@($status).Count) changed path entries)." 'Source'
        $status | ForEach-Object { Write-Host "  $_" }
    } else { Write-Host 'OK: Git working tree is clean.' -ForegroundColor Green }
} catch { Add-Blocker 'Could not read Git working tree status.' 'Source' }

# Read and compare all four version declarations without changing them.
try {
    $package = Get-Content -LiteralPath (Join-Path $script:Root 'package.json') -Raw | ConvertFrom-Json
    $tauri = Get-Content -LiteralPath (Join-Path $script:Root 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
    $cargoToml = Get-Content -LiteralPath (Join-Path $script:Root 'src-tauri/Cargo.toml') -Raw
    $cargoLock = Get-Content -LiteralPath (Join-Path $script:Root 'src-tauri/Cargo.lock') -Raw
    $cargoVersionMatch = [regex]::Match($cargoToml, '(?m)^version\s*=\s*"([^"]+)"')
    $lockPackageMatch = [regex]::Match($cargoLock, '(?ms)^\[\[package\]\]\s*\r?\nname\s*=\s*"qlisa"\s*\r?\nversion\s*=\s*"([^"]+)"')
    $versions = [ordered]@{
        'package.json' = [string]$package.version
        'src-tauri/Cargo.toml' = if ($cargoVersionMatch.Success) { $cargoVersionMatch.Groups[1].Value } else { '' }
        'src-tauri/tauri.conf.json' = [string]$tauri.version
        'src-tauri/Cargo.lock (qlisa)' = if ($lockPackageMatch.Success) { $lockPackageMatch.Groups[1].Value } else { '' }
    }
    foreach ($entry in $versions.GetEnumerator()) {
        if ($entry.Value -ne $ExpectedVersion) { Add-Blocker "Version mismatch: $($entry.Key) is '$($entry.Value)', expected '$ExpectedVersion'." 'Source' }
        else { Write-Host "OK: $($entry.Key) = $ExpectedVersion" -ForegroundColor Green }
    }
} catch { Add-Blocker 'Could not parse all four version files.' 'Source' }

# Build the source inventory from HEAD plus non-ignored untracked files.
try {
    $paths = Get-CandidatePaths
    Write-Host "Source inventory: $($paths.Head.Count) files in HEAD; $($paths.Index.Count) in index; $($paths.Untracked.Count) untracked files."
    if ($paths.Untracked.Count -gt 0) { Add-Warning 'Untracked source files are included in the candidate inventory and must be reviewed.' }

    $headNdi = Get-NdiFiles $paths.Head
    $indexNdi = Get-NdiFiles $paths.Index
    $headRuntimeExe = @($paths.Head | Where-Object { [System.IO.Path]::GetFileName([string]$_) -match '(?i)^(ffmpeg|ffprobe)\.exe$' })
    $indexRuntimeExe = @($paths.Index | Where-Object { [System.IO.Path]::GetFileName([string]$_) -match '(?i)^(ffmpeg|ffprobe)\.exe$' })
    $headPrivate = @($paths.Head | Where-Object { Test-PrivateFileName ([string]$_) })
    $indexPrivate = @($paths.Index | Where-Object { Test-PrivateFileName ([string]$_) })
    foreach ($path in @($headNdi + $indexNdi | Sort-Object -Unique)) { Add-Blocker "NDI DLL is tracked in HEAD or index: $path" 'Source' }
    foreach ($path in @($headRuntimeExe + $indexRuntimeExe | Sort-Object -Unique)) { Add-Blocker "FFmpeg executable is tracked in HEAD or index: $path" 'Source' }
    foreach ($path in @($headPrivate + $indexPrivate | Sort-Object -Unique)) { Add-Blocker "Private or environment file is tracked in HEAD or index: $path" 'Source' }

    $sourceNdi = Test-SourceTreeNdiFiles
    foreach ($path in $sourceNdi) { Add-Blocker "NDI DLL is present in source tree: $path" 'Source' }
    $sourceRuntimeExes = Test-SourceTreeRuntimeExes
    foreach ($path in $sourceRuntimeExes) { Add-Blocker "FFmpeg executable is present in source tree: $path" 'Source' }
} catch { Add-Blocker 'Could not build or inspect the source inventory.' 'Source' }

# Source-only mode validates the checked-in workflow surface without inspecting
# or requiring locally staged binary runtimes.
if ($SourceOnly) {
    try {
        $workflowFiles = @(Get-ChildItem -LiteralPath (Join-Path $script:Root '.github/workflows') -File | ForEach-Object { $_.Name })
        if ($workflowFiles.Count -ne 1 -or $workflowFiles[0] -ne 'ci.yml') {
            Add-Blocker 'Workflow hygiene failed: only the read-only ci.yml workflow is allowed.' 'Source'
            foreach ($workflowFile in $workflowFiles) { Write-Host "  workflow: $workflowFile" }
        } else {
            $ciText = Get-Content -LiteralPath (Join-Path $script:Root '.github/workflows/ci.yml') -Raw
            if ($ciText -notmatch '(?m)^permissions:\s*\r?\n\s+contents:\s*read\s*$' -or
                $ciText -match '(?i)contents:\s*write|gh\s+release|tauri-action|discord|release\.yml') {
                Add-Blocker 'Workflow hygiene failed: CI must be read-only and must not contain release or publication actions.' 'Source'
            } else {
                Write-Host 'OK: only read-only ci.yml workflow is present; legacy release workflows are absent.' -ForegroundColor Green
            }
        }
    } catch { Add-Blocker 'Could not verify GitHub workflow hygiene.' 'Source' }
}

# Verify that the locally staged FFmpeg package is pinned, licensed, and bundle-mapped.
$skipBinaryChecks = $SourceOnly
if (-not $skipBinaryChecks) {
$ffmpegRoot = Join-Path $script:Root 'src-tauri/vendor/ffmpeg'
$ffmpegPaths = @('ffmpeg.exe', 'ffprobe.exe', 'LICENSE', 'README-Gyan-build.txt') | ForEach-Object { Join-Path $ffmpegRoot $_ }
$ffmpegMissing = @($ffmpegPaths | Where-Object { -not (Test-Path -LiteralPath $_ -PathType Leaf) })
if ($ffmpegMissing.Count -gt 0) {
    Add-Blocker 'Pinned FFmpeg package is not staged locally (ffmpeg.exe, ffprobe.exe, LICENSE, and README-Gyan-build.txt are required for an installer).'
} else {
    $ffmpegLicense = Get-Content -LiteralPath (Join-Path $ffmpegRoot 'LICENSE') -Raw
    $ffmpegNotice = Get-Content -LiteralPath (Join-Path $ffmpegRoot 'README-Gyan-build.txt') -Raw
    if ($ffmpegLicense -notmatch 'GNU GENERAL PUBLIC LICENSE\s+Version 3') { Add-Blocker 'Staged FFmpeg license is not the expected GPLv3 license.' }
    if ($ffmpegNotice -notmatch '9\.0\.1' -or $ffmpegNotice -notmatch 'bf1b838f2a' -or $ffmpegNotice -notmatch 'srt v1\.5\.6-2-gfcae571') {
        Add-Blocker 'Staged FFmpeg build notice does not match the documented 9.0.1 / FFmpeg commit / libsrt versions.'
    }
    $bundleConfig = Get-Content -LiteralPath (Join-Path $script:Root 'src-tauri/tauri.windows.conf.json') -Raw
    foreach ($required in @('vendor/ffmpeg/ffmpeg.exe', 'vendor/ffmpeg/ffprobe.exe', 'vendor/ffmpeg/LICENSE', 'vendor/ffmpeg/README-Gyan-build.txt')) {
        if ($bundleConfig -notmatch [regex]::Escape($required)) { Add-Blocker "Windows bundle config omits $required." }
    }
    $syncScript = Get-Content -LiteralPath (Join-Path $script:Root 'scripts/sync-network-runtime.ps1') -Raw
    if ($syncScript -notmatch "\`$ffmpegVersion\s*=\s*'9\.0\.1'" -or
        $syncScript -notmatch 'fec81ae03971d9dd4be3ebe02e263bd2ec1d789483f931bdba5f5715e65da2e9') {
        Add-Blocker 'FFmpeg staging script version or archive checksum differs from the documented package.'
    }
    Write-Host 'OK: FFmpeg payload and matching license/build notice are staged. The preflight does not execute the FFmpeg binary.' -ForegroundColor Green
}

# Updater readiness is deliberately a blocker until Qlisa has its own key and endpoint.
try {
    $updaterConfig = Get-PropertyValue (Get-PropertyValue $tauri 'plugins') 'updater'
    $configuredPubKey = [string](Get-PropertyValue $updaterConfig 'pubkey')
    $configuredEndpoints = @(Get-PropertyValue $updaterConfig 'endpoints')
    if (-not $tauri.bundle.createUpdaterArtifacts) { Add-Blocker 'Updater artifacts are disabled in tauri.conf.json.' }
    if ([string]::IsNullOrWhiteSpace($configuredPubKey)) { Add-Blocker 'Qlisa updater public key is not configured.' }
    if ($configuredPubKey -match '(?i)inkue|placeholder|example') { Add-Blocker 'Configured updater public key is not Qlisa-specific.' }
    $validEndpoint = @($configuredEndpoints | Where-Object { [string]$_ -match '^https://' })
    if ($validEndpoint.Count -eq 0) { Add-Blocker 'Qlisa updater HTTPS endpoint is not configured.' }
    if ($PrepareRelease -and $configuredEndpoints -notcontains $UpdateMetadataUrl) {
        Add-Blocker 'The configured Qlisa updater feed does not match -UpdateMetadataUrl.'
    }

    if (-not $SigningKeyPath -or -not (Test-Path -LiteralPath $SigningKeyPath -PathType Leaf)) {
        Add-Blocker 'A Qlisa updater signing key was not supplied for local preflight.'
    } else {
        $keyFullPath = [System.IO.Path]::GetFullPath($SigningKeyPath)
        $rootPrefix = [System.IO.Path]::GetFullPath($script:Root).TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
        if ($keyFullPath.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) { Add-Blocker 'The updater private key must be stored outside the repository.' }
        else { Write-Host 'OK: private signing key path exists outside the repository; key contents were not read.' -ForegroundColor Green }
    }

    if (-not $PrepareRelease -and (-not $UpdaterMetadataPath -or -not (Test-Path -LiteralPath $UpdaterMetadataPath -PathType Leaf))) {
        Add-Blocker 'Updater metadata (for example latest.json) was not supplied.'
    } elseif (-not $PrepareRelease) {
        $metadata = Get-Content -LiteralPath $UpdaterMetadataPath -Raw | ConvertFrom-Json
        if ([string]$metadata.version -ne $ExpectedVersion) { Add-Blocker 'Updater metadata version does not match the release version.' }
        $platforms = Get-PropertyValue $metadata 'platforms'
        if ($null -eq $platforms) {
            Add-Blocker 'Updater metadata has no platforms map.'
        } else {
            foreach ($platformName in $platforms.PSObject.Properties.Name) {
                $platform = Get-PropertyValue $platforms $platformName
                if ([string]::IsNullOrWhiteSpace([string](Get-PropertyValue $platform 'url')) -or
                    [string]::IsNullOrWhiteSpace([string](Get-PropertyValue $platform 'signature'))) {
                    Add-Blocker "Updater metadata is missing URL or signature for $platformName."
                }
            }
        }
    }

    if (-not $PrepareRelease -and (-not $UpdaterSignaturePath -or -not (Test-Path -LiteralPath $UpdaterSignaturePath -PathType Leaf))) {
        Add-Blocker 'A detached updater signature file was not supplied.'
    } elseif (-not $PrepareRelease) {
        $signature = Get-Content -LiteralPath $UpdaterSignaturePath -Raw
        if ([string]::IsNullOrWhiteSpace($signature)) { Add-Blocker 'Updater signature file is empty.' }
        else {
            Write-Host 'OK: updater signature file is present.' -ForegroundColor Green
            Add-Blocker 'This preflight checks signature presence only; cryptographic verification must pass with the configured Qlisa public key before release.'
        }
    }
} catch { Add-Blocker 'Could not validate updater configuration or metadata.' }

if ($PrepareRelease -and $null -eq (Get-Command 'minisign.exe' -ErrorAction SilentlyContinue)) {
    if ($null -eq (Get-Command 'minisign' -ErrorAction SilentlyContinue)) {
        Add-Blocker 'Prepare mode requires minisign to verify the updater bundle signature.'
    }
}

if ($PrepareRelease) {
    $mpvPath = Join-Path $script:Root 'src-tauri/vendor/mpv/libmpv-2.dll'
    if (-not (Test-Path -LiteralPath $mpvPath -PathType Leaf) -or (Get-Item -LiteralPath $mpvPath).Length -eq 0) {
        Add-Blocker 'Prepare mode requires a non-empty local src-tauri/vendor/mpv/libmpv-2.dll.'
    }
}

# Inspect only explicitly supplied artifacts. This script never creates them.
$assetPaths = [System.Collections.Generic.List[string]]::new()
if ($InstallerPath) { $assetPaths.Add($InstallerPath) }
foreach ($path in $ReleaseAssetPath) { $assetPaths.Add($path) }
if ($UpdaterMetadataPath) { $assetPaths.Add($UpdaterMetadataPath) }
if ($UpdaterSignaturePath) { $assetPaths.Add($UpdaterSignaturePath) }
$assetFiles = @(Get-AssetFiles -Paths $assetPaths.ToArray())
if ($assetFiles.Count -eq 0) {
    Add-Warning 'No installer or release assets were explicitly prepared for inspection.'
} else {
    Write-Host 'Prepared release assets:'
    foreach ($file in $assetFiles) {
        Add-InventoryFile -Path $file -Origin 'release asset'
        $asset = Get-Item -LiteralPath $file
        $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
        Write-Host "  $(Get-RelativePath $file)  size=$($asset.Length) sha256=$hash"
        Test-ArchiveForNdi $file
    }
    $installerResolved = if ($InstallerPath) {
        if ([System.IO.Path]::IsPathRooted($InstallerPath)) { $InstallerPath } else { Join-Path $script:Root $InstallerPath }
    }
    if ($installerResolved -and (Test-Path -LiteralPath $installerResolved -PathType Leaf)) {
        Test-PreparedFreshness -Path $installerResolved -Label 'Prepared installer'
    }
    $looseNdi = Get-NdiFiles $assetFiles
    foreach ($path in $looseNdi) { Add-Blocker "Release asset contains an NDI DLL: $path" }
}

if ($PrepareRelease) {
    if ($script:SourceBlockers.Count -gt 0 -or $script:BinaryBlockers.Count -gt 0) {
        Add-Warning 'Prepare mode stopped before checks or build because preflight blockers exist.'
    } else {
        Add-Blocker 'Local build/installer/metadata preparation is not enabled in this script yet; no build command was executed.'
    }
} elseif ($RunChecks) {
    Invoke-CheckCommand 'pnpm test' 'pnpm' @('test') $script:Root
    Invoke-CheckCommand 'pnpm build' 'pnpm' @('build') $script:Root
    Invoke-CheckCommand 'cargo metadata' 'cargo' @('metadata', '--locked', '--no-deps', '--format-version', '1') (Join-Path $script:Root 'src-tauri')
    Invoke-CheckCommand 'cargo check' 'cargo' @('check', '--locked', '--features', 'asio-support') (Join-Path $script:Root 'src-tauri')
    Invoke-CheckCommand 'cargo test' 'cargo' @('test', '--locked', '--features', 'asio-support') (Join-Path $script:Root 'src-tauri')
    Invoke-CheckCommand 'cargo clippy' 'cargo' @('clippy', '--locked', '--all-targets', '--features', 'asio-support', '--', '-D', 'warnings') (Join-Path $script:Root 'src-tauri')
    Invoke-CheckCommand 'cargo fmt --check' 'cargo' @('fmt', '--check') (Join-Path $script:Root 'src-tauri')
} else {
    Add-Warning 'Checks were not run. Pass -RunChecks to run frontend and Rust checks; this may create build/test outputs but never an installer.'
}

if ($PreparedBuildPath) {
    $buildRoot = if ([System.IO.Path]::IsPathRooted($PreparedBuildPath)) { $PreparedBuildPath } else { Join-Path $script:Root $PreparedBuildPath }
    $indexPath = Join-Path $buildRoot 'index.html'
    if (-not (Test-Path -LiteralPath $indexPath -PathType Leaf)) {
        Add-Blocker 'Prepared production build is missing its index.html.'
    } else {
        Test-PreparedFreshness -Path $indexPath -Label 'Prepared production frontend build'
    }
}
}

if (-not $SourceOnly) { Write-InventoryArtifact }

Write-Host ''
Write-Host "Source readiness: $(if ($script:SourceBlockers.Count -eq 0) { 'READY' } else { 'BLOCKED' }) ($($script:SourceBlockers.Count) blocker(s))."
Write-Host "Preflight summary: $($script:SourceBlockers.Count + $script:BinaryBlockers.Count) blocker(s), $($script:Warnings.Count) warning(s)."
if ($SourceOnly) {
    Write-Host 'Binary readiness: NOT CHECKED (-SourceOnly skips local runtime and updater payload checks).'
    if ($script:SourceBlockers.Count -gt 0) {
        Write-Host 'SOURCE NOT READY. No files were written; no build or publication action was performed.' -ForegroundColor Red
        exit 1
    }
    Write-Host 'SOURCE READY. Binary readiness was not checked. No files were written or published.' -ForegroundColor Green
    exit 0
}
Write-Host "Binary readiness: $(if ($script:BinaryBlockers.Count -eq 0) { 'READY' } else { 'BLOCKED' }) ($($script:BinaryBlockers.Count) blocker(s))."
if ($script:SourceBlockers.Count -gt 0 -or $script:BinaryBlockers.Count -gt 0) {
    Write-Host 'NOT READY. This script performed no publication action.' -ForegroundColor Red
    exit 1
}
Write-Host 'Preflight checks passed. This script performed no publication action.' -ForegroundColor Green
exit 0
