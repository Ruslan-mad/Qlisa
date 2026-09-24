# Local updater end-to-end test

This test uses two signed builds and a loopback HTTP feed. It creates no GitHub
Release, tag, or production configuration change. Installers use the isolated
`QlisaUpdaterTest` identity. Do not distribute them.

Run this test after the private signed production 1.5.3 installer is built.
Before running `scripts/publish.ps1`, record the current commit SHA as
`$baselineCommit`; it must be the clean source commit whose four version fields
are 1.5.2. Enter that exact SHA below after the production build. The updater
test uses a detached worktree from that commit and does not change the
production checkout. FFmpeg and libmpv runtime files and their pinned hashes
must be ready. Keep the worktree, feed directory, and test app until checks
finish.

## Prepare an isolated worktree and config overlay

```powershell
$sourceRoot = (git rev-parse --show-toplevel).Trim()
$baselineCommit = 'PASTE_SAVED_PRE_BUMP_COMMIT_SHA_HERE'
if ($baselineCommit -notmatch '^[0-9a-fA-F]{40}$' -or $baselineCommit -match '^0+$') { throw 'Set baselineCommit to the exact 40-character SHA recorded before publish.ps1.' }
$resolvedBaseline = (git -C $sourceRoot rev-parse "$baselineCommit^{commit}").Trim()
if ($LASTEXITCODE -ne 0 -or $resolvedBaseline -ine $baselineCommit) { throw 'Saved baselineCommit does not resolve to a commit.' }
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$runId = [guid]::NewGuid().ToString('N')
$testRoot = Join-Path $tempRoot ('Qlisa-updater-local-' + $runId)
$baselineDir = Join-Path $tempRoot ('qlisa-updater-local-baseline-' + $runId)
$feedDir = Join-Path $tempRoot ('qlisa-updater-local-feed-' + $runId)
$testConfig = Join-Path $tempRoot ('qlisa-updater-local-' + $runId + '.tauri.conf.json')
$feedServer = $null
git -C $sourceRoot worktree add --detach $testRoot $baselineCommit
if ($LASTEXITCODE -ne 0) { throw 'Could not create the disposable worktree.' }
Push-Location $testRoot
$packageVersion = [string](Get-Content -Raw package.json | ConvertFrom-Json).version
$tauriVersion = [string](Get-Content -Raw src-tauri/tauri.conf.json | ConvertFrom-Json).version
$cargoVersion = [regex]::Match((Get-Content -Raw src-tauri/Cargo.toml), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
$lockVersion = [regex]::Match((Get-Content -Raw src-tauri/Cargo.lock), '(?ms)^\[\[package\]\]\s*\r?\nname\s*=\s*"qlisa"\s*\r?\nversion\s*=\s*"([^"]+)"').Groups[1].Value
$versions = @($packageVersion, $tauriVersion, $cargoVersion, $lockVersion)
if (@($versions | Where-Object { $_ -cne '1.5.2' }).Count -ne 0) { throw 'Local updater test requires HEAD version 1.5.2 in package.json, tauri.conf.json, Cargo.toml, and Cargo.lock.' }
Pop-Location
$feedUrl = 'http://127.0.0.1:8765/latest.json'
@{
  productName = 'QlisaUpdaterTest'
  identifier = 'com.qlisa.updater.test'
  plugins = @{ updater = @{
    endpoints = @($feedUrl)
    dangerousInsecureTransportProtocol = $true
  } }
} | ConvertTo-Json -Depth 8 | ForEach-Object {
  [IO.File]::WriteAllText($testConfig, $_, [Text.UTF8Encoding]::new($false))
}
$trackedConfig = Get-Content -Raw (Join-Path $sourceRoot 'src-tauri/tauri.conf.json') | ConvertFrom-Json
if ($trackedConfig.productName -eq 'QlisaUpdaterTest' -or
    $trackedConfig.identifier -eq 'com.qlisa.updater.test' -or
    @($trackedConfig.plugins.updater.endpoints | Where-Object { $_ -like 'http:*' }).Count -gt 0) {
  throw 'Production Tauri config contains test identity or HTTP endpoint.'
}
```

The insecure HTTP option exists only in this temporary overlay. Keep the
production updater public key unchanged; both builds must verify against it.
Copy runtime files and notices listed by `scripts/runtime-manifest.json`. This
local build does not need source archives:

```powershell
$manifest = Get-Content -Raw (Join-Path $testRoot 'scripts/runtime-manifest.json') | ConvertFrom-Json
$ffmpeg = @($manifest.components | Where-Object id -eq 'ffmpeg-gyan-essentials')
$mpv = @($manifest.components | Where-Object id -eq 'libmpv')
if ($ffmpeg.Count -ne 1 -or $mpv.Count -ne 1) { throw 'Expected one FFmpeg and one libmpv runtime record.' }
$runtimeFiles = @($ffmpeg[0].runtimeFiles) + @($ffmpeg[0].notices) + @($mpv[0])
foreach ($record in $runtimeFiles) {
  $relative = ([string]$record.path).Replace('/', '\')
  $source = [IO.Path]::GetFullPath((Join-Path $sourceRoot $relative))
  $sourcePrefix = $sourceRoot.TrimEnd('\') + '\'
  if (-not $source.StartsWith($sourcePrefix, [StringComparison]::OrdinalIgnoreCase)) { throw "Runtime path escapes source checkout: $relative" }
  if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ine [string]$record.sha256) { throw "Runtime hash differs from manifest: $relative" }
  $destination = Join-Path $testRoot $relative
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
  Copy-Item -LiteralPath $source -Destination $destination
  if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -ine [string]$record.sha256) { throw "Copied runtime hash differs from manifest: $relative" }
}
Push-Location $testRoot
pnpm install --frozen-lockfile
if ($LASTEXITCODE -ne 0) { throw 'Dependency install failed.' }
Pop-Location
```

## Build the two signed versions

Set the signing key path and prompt for its password in the current PowerShell
session. Do not save or print the password:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Join-Path $env:USERPROFILE '.tauri\qlisa.key'
$securePassword = Read-Host 'Tauri signing key password' -AsSecureString
$passwordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($securePassword)
try { $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($passwordPointer) }
finally {
  [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($passwordPointer)
  $securePassword.Dispose()
}
```

Build version 1.5.2 as the installed baseline:

```powershell
Push-Location $testRoot
pnpm exec tauri build --config $testConfig --bundles nsis -- --features asio-support
if ($LASTEXITCODE -ne 0) { throw 'Signed 1.5.2 baseline build failed.' }
$baseline = @(Get-ChildItem 'src-tauri/target/release/bundle/nsis' -File -Filter '*1.5.2*.exe')
if ($baseline.Count -ne 1 -or -not (Test-Path ($baseline[0].FullName + '.sig'))) { throw 'Expected one signed 1.5.2 NSIS installer.' }
New-Item -ItemType Directory -Force -Path $baselineDir | Out-Null
Copy-Item $baseline[0].FullName, ($baseline[0].FullName + '.sig') $baselineDir
```

Use the checked-in release helpers to change the four synchronized version
fields from 1.5.2 to 1.5.3. Keep the product identity and temporary updater
overlay unchanged. Run this from `$testRoot`:

```powershell
@'
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const h = await import(pathToFileURL(resolve('scripts/release-helpers.mjs')).href);
const read = (p) => readFileSync(p, 'utf8');
const write = (p, value) => writeFileSync(p, value, 'utf8');
write('package.json', h.replaceJsonVersion(read('package.json'), '1.5.2', '1.5.3'));
write('src-tauri/tauri.conf.json', h.replaceJsonVersion(read('src-tauri/tauri.conf.json'), '1.5.2', '1.5.3'));
write('src-tauri/Cargo.toml', h.replaceCargoTomlVersion(read('src-tauri/Cargo.toml'), '1.5.2', '1.5.3'));
write('src-tauri/Cargo.lock', h.replaceCargoLockVersion(read('src-tauri/Cargo.lock'), '1.5.2', '1.5.3'));
'@ | node --input-type=module -
if ($LASTEXITCODE -ne 0) { throw 'Version replacement failed.' }
```

Then build the update:

```powershell
pnpm exec tauri build --config $testConfig --bundles nsis -- --features asio-support
if ($LASTEXITCODE -ne 0) { throw 'Signed 1.5.3 update build failed.' }
$update = @(Get-ChildItem 'src-tauri/target/release/bundle/nsis' -File -Filter '*1.5.3*.exe')
if ($update.Count -ne 1 -or -not (Test-Path ($update[0].FullName + '.sig'))) { throw 'Expected one signed 1.5.3 NSIS installer.' }
New-Item -ItemType Directory -Force -Path $feedDir | Out-Null
Copy-Item $update[0].FullName, ($update[0].FullName + '.sig') $feedDir
$assetUrl = 'http://127.0.0.1:8765/' + [uri]::EscapeDataString($update[0].Name)
$signaturePath = Join-Path $feedDir ($update[0].Name + '.sig')
$latestPath = Join-Path $feedDir 'latest.json'
$signature = [IO.File]::ReadAllText($signaturePath)
if ([string]::IsNullOrWhiteSpace($signature) -or $signature -cne $signature.Trim()) { throw 'Updater signature must be non-empty and have no surrounding whitespace.' }
$feedUri = [uri]$assetUrl
if ($feedUri.Scheme -cne 'http' -or $feedUri.Host -cne '127.0.0.1' -or $feedUri.Port -ne 8765) { throw 'Test updater asset URL must use only the local loopback feed.' }
$notes = (Get-Content -Raw 'docs/RELEASE_NOTES_1.5.3.md').Trim()
if ([string]::IsNullOrWhiteSpace($notes)) { throw 'Release notes are empty.' }
$latest = [ordered]@{
  version = '1.5.3'
  notes = $notes
  pub_date = [DateTimeOffset]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ss.fffZ', [Globalization.CultureInfo]::InvariantCulture)
  platforms = [ordered]@{
    'windows-x86_64' = [ordered]@{
      url = $assetUrl
      signature = $signature
    }
  }
}
[IO.File]::WriteAllText($latestPath, ($latest | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
$feed = Get-Content -Raw $latestPath | ConvertFrom-Json
if ($feed.version -cne '1.5.3' -or $feed.platforms.'windows-x86_64'.url -cne $assetUrl -or
    $feed.platforms.'windows-x86_64'.signature -cne $signature -or
    $feed.pub_date -cnotmatch '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$') { throw 'Local latest.json does not match the installer, exact signature text, or UTC publication date.' }
Get-FileHash $update[0].FullName, $signaturePath, $latestPath -Algorithm SHA256
```

Start the loopback feed server from the current PowerShell session. The
process is hidden, uses the exact feed directory, and its PID is retained for
cleanup:

```powershell
$python = Get-Command python.exe -ErrorAction Stop
$serverArgs = '-m http.server 8765 --bind 127.0.0.1 --directory "' + $feedDir + '"'
$feedServer = Start-Process -FilePath $python.Source -ArgumentList $serverArgs -WindowStyle Hidden -PassThru
Start-Sleep -Milliseconds 800
try {
  $servedFeed = Invoke-RestMethod -Uri $feedUrl -TimeoutSec 3
  if ($servedFeed.version -cne '1.5.3' -or
      $servedFeed.platforms.'windows-x86_64'.url -cne $assetUrl -or
      $servedFeed.platforms.'windows-x86_64'.signature -cne $signature) {
    throw 'Port 8765 serves a feed other than this local signed 1.5.3 build.'
  }
}
catch {
  Stop-Process -Id $feedServer.Id -Force -ErrorAction SilentlyContinue
  throw 'Local feed server failed readiness or feed-content verification.'
}
```

Check the feed and installer URLs return HTTP 200 before installing. The server
must remain running throughout the test.

## Run the test

1. Install the 1.5.2 installer from `$baselineDir`. Launch it and confirm About
   reports 1.5.2. Confirm its install path and settings are separate from Qlisa.
   Wait five seconds: the startup update check must show no modal error and
   must not block normal use.
2. Save the valid `latest.json` text, replace its signature with invalid text,
   then manually check and download. The updater must reject the signature and
   leave the app at 1.5.2. Restore the exact saved JSON, restart the test app,
   then confirm a manual check finds 1.5.3 and its release notes. Download and
   confirm progress advances.
3. With the update ready, try Install while an audio cue runs and while it is
   paused. Repeat for a video cue. Run audio/video on multiple outputs and
   confirm installation stays blocked while any cue is active or paused.
4. Modify the workspace without saving. Confirm installation is blocked.
   Save it and confirm the guard clears.
5. Install with no active cues and a saved workspace. Confirm the app closes,
   NSIS applies 1.5.3, and QlisaUpdaterTest restarts. About must report 1.5.3;
   confirm workspace and test preferences remain intact.
6. Check for updates again; it must report no newer version. Production Qlisa
   and its settings must remain unchanged.

Use this feed mutation for step 2, then restore the saved bytes before
restarting the app:

```powershell
$validLatestBackup = Join-Path $feedDir 'latest.valid.json'
Copy-Item -LiteralPath $latestPath -Destination $validLatestBackup
$invalid = Get-Content -Raw $latestPath | ConvertFrom-Json
$invalid.platforms.'windows-x86_64'.signature = 'invalid test signature'
[IO.File]::WriteAllText($latestPath, ($invalid | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
# In QlisaUpdaterTest: manually check and download; expect signature rejection and version 1.5.2.
Copy-Item -LiteralPath $validLatestBackup -Destination $latestPath -Force
```

The signed `latest.json` tests the Tauri updater signature. If it fails, record
the exact updater error and compare the configured public key with the key that
signed the `.sig`; do not bypass signature verification.

## Recorded result — 2026-09-24

The isolated signed NSIS update from 1.5.2 to 1.5.3 passed through the local
loopback feed at `http://127.0.0.1:8765`. Both installers used the same Tauri
signing key. With an invalid signature, the downloaded update was rejected and
the app remained at 1.5.2. With the valid signature, an unsaved workspace
blocked installation after download. After saving, the signed NSIS installer
applied the update and restarted the app. The executable FileVersion and About
both reported 1.5.3, and the next update check reported that the app was
current. A workspace test file remained present and opened with Wait 1:00.

The audio/video cue installation guards were not tested in the UI. A separate
libmpv smoke test was performed earlier; it does not verify those guards. This
was a local test only: no GitHub Release, tag, or push was created. It does not
resolve the FFmpeg or libmpv source and compliance blockers for public binary
distribution.

## Cleanup

Close QlisaUpdaterTest and uninstall only the app with the `QlisaUpdaterTest`
identity. Stop the feed server and remove only paths whose resolved absolute
paths are direct children of the system temp directory:

```powershell
if ($null -ne $feedServer) { Stop-Process -Id $feedServer.Id -Force -ErrorAction SilentlyContinue }
Pop-Location
$tempPrefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
foreach ($path in @($testRoot, $baselineDir, $feedDir, $testConfig)) {
  $full = [IO.Path]::GetFullPath($path)
  if (-not $full.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase) -or
      [IO.Path]::GetDirectoryName($full).TrimEnd('\') -ine $tempRoot) { throw "Refusing cleanup outside direct temp children: $full" }
}
git -C $sourceRoot worktree remove --force $testRoot
if ($LASTEXITCODE -ne 0) { throw 'Could not remove the disposable worktree.' }
Remove-Item -LiteralPath $baselineDir, $feedDir -Recurse -Force
Remove-Item -LiteralPath $testConfig -Force
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
```

Verify the production config still contains only the HTTPS production updater
endpoint. This procedure does not create or publish a GitHub Release.
