# Local per-machine updater test

This document records the local signed update from **1.5.5 to 1.5.6** and the procedure to repeat it. It creates no GitHub Release and does not change the production endpoint. The old local 1.5.3 → 1.5.4 test also passed.

The test uses a detached worktree and the `QlisaUpdaterTest` product identity. Both builds use the checked-in NSIS `perMachine` setting and the updater's `passive` setting. The updater artifact contains Qlisa only. FFmpeg, ffprobe, and libmpv are downloaded by the app into `%LOCALAPPDATA%\Qlisa\runtime`; do not copy vendor binaries into the worktree or installer.

## Verified local smoke checks (1.5.5)

These earlier checks used an isolated profile. Per-machine results are recorded below.

- A clean launch downloaded FFmpeg, ffprobe, and libmpv and verified their pinned SHA256 hashes. A second launch reused the files without downloading them again.
- After ffprobe was removed, the app restored it and kept the already loaded libmpv DLL unchanged.
- Qlisa's media conversion reached 100% and created the converted output file.
- A 32-second video cue ran on the Main output. Dragging the Time slider from about 24.8 seconds to about 4.7 seconds changed playback position, confirming seek.
- An Audio cue loaded `tone.wav`, started, and completed in Qlisa. Physical sound was confirmed during the per-machine verification below.
- A video cue was assigned to Main and the isolated Local SRT Smoke output at `127.0.0.1:19077`. The loopback receiver received MPEG-TS with H.264/AAC and decoded sampled frames from the actual color-bar cue; the receiver exited successfully. This also exercised simultaneous Main and SRT destinations.
- A clean per-machine installation, offline runtime retry, and signed updater update are verified below.

## Verified per-machine install and updater (1.5.5 to 1.5.6)

- The signed 1.5.5 installer completed a clean per-machine install under `C:\Program Files\Qlisa`, including UAC and first launch.
- An offline bootstrap check blocked the pinned runtime download with a temporary outbound firewall rule. Qlisa displayed the download error and Retry action. After removing the rule, Retry installed FFmpeg, ffprobe, and libmpv with their pinned hashes, then opened the main window.
- The signed 1.5.6 update completed under the `QlisaUpdaterTest` identity. About, the registry, and the executable reported 1.5.6. A saved Memo workspace reopened with its SHA-256 unchanged. The light theme and runtime remained available.
- The `QlisaUpdaterTest` installation was removed after the check. The production installation remained intact.
- The user heard the physical Audio Cue during the audio check.
- Settings runtime reinstallation passed on the installed 1.5.6 build. Clicking
  Reinstall in Preferences relaunched Qlisa and rewrote the runtime files under
  `%TEMP%\qlisa-settings-candidate-156\LocalAppData\Qlisa\runtime` at
  00:48:19–00:48:22. The `.reinstall-pending` marker was absent after restart,
  and Verify Integrity reported all components as Installed. Verified SHA-256
  prefixes: FFmpeg `288EF710…`, ffprobe `8603CD025…`, and libmpv `751131F81…`.
  Commit `7e68180` grants restart permission to the standalone Preferences
  window.

The production installer candidate is `src-tauri/target/release/bundle/nsis/Qlisa_1.5.6_x64-setup.exe` (SHA-256 `929b56a162af2d85adaf390e1fc12bf9b2cf5e42d924350206ca935c3afe6add`). Its updater signature is `src-tauri/target/release/bundle/nsis/Qlisa_1.5.6_x64-setup.exe.sig` (SHA-256 `862136e5395130c539602f786be8dd6f70cf9dd0027ef75bd16fccef06cfb267`). Local feed metadata is `src-tauri/target/release/prepared/v1.5.6/latest.json` (SHA-256 `259db74935cc772143b2d7f2c9668caaba672c4b8be598410ab6c6fc4b305a04`). These are local artifacts; they have not been published.

## Build the signed baseline and update

Run these blocks in one PowerShell session from a clean checkout at version 1.5.5. Keep the signing key and password in the current session only.

```powershell
$sourceRoot = (git rev-parse --show-toplevel).Trim()
if (@(git -C $sourceRoot status --porcelain).Count -ne 0) { throw 'Start from a clean checkout.' }
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('Qlisa-updater-' + [guid]::NewGuid().ToString('N'))
$testConfig = Join-Path ([IO.Path]::GetTempPath()) ('qlisa-updater-' + [guid]::NewGuid().ToString('N') + '.json')
$feedDir = Join-Path ([IO.Path]::GetTempPath()) ('qlisa-feed-' + [guid]::NewGuid().ToString('N'))
$testProfile = Join-Path ([IO.Path]::GetTempPath()) ('qlisa-updater-profile-' + [guid]::NewGuid().ToString('N'))
$feedUrl = 'http://127.0.0.1:8765/latest.json'
if (@(Get-NetTCPConnection -State Listen -LocalPort 8765 -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Port 8765 is already in use; free it before continuing.' }
$baseCommit = (git -C $sourceRoot rev-parse HEAD).Trim()
git -C $sourceRoot worktree add --detach $testRoot $baseCommit
if ($LASTEXITCODE -ne 0) { throw 'Could not create the test worktree.' }
Push-Location $testRoot
$versions = @(
  [string](Get-Content -Raw package.json | ConvertFrom-Json).version,
  [string](Get-Content -Raw src-tauri/tauri.conf.json | ConvertFrom-Json).version,
  [regex]::Match((Get-Content -Raw src-tauri/Cargo.toml), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value,
  [regex]::Match((Get-Content -Raw src-tauri/Cargo.lock), '(?ms)^\[\[package\]\]\s*\r?\nname\s*=\s*"qlisa"\s*\r?\nversion\s*=\s*"([^"]+)"').Groups[1].Value
)
if (@($versions | Where-Object { $_ -cne '1.5.5' }).Count -ne 0) { throw 'All four version fields must be 1.5.5.' }
$windowsConfig = Get-Content -Raw src-tauri/tauri.windows.conf.json | ConvertFrom-Json
if ($windowsConfig.bundle.windows.nsis.installMode -cne 'perMachine') { throw 'NSIS must use perMachine.' }
$defaultConfig = Get-Content -Raw src-tauri/tauri.conf.json | ConvertFrom-Json
if ($defaultConfig.plugins.updater.windows.installMode -cne 'passive') { throw 'Updater installMode must remain passive.' }
Pop-Location
@{
  productName = 'QlisaUpdaterTest'
  identifier = 'com.qlisa.updater.test'
  plugins = @{ updater = @{
    endpoints = @($feedUrl)
    dangerousInsecureTransportProtocol = $true
  } }
} | ConvertTo-Json -Depth 8 | ForEach-Object { [IO.File]::WriteAllText($testConfig, $_, [Text.UTF8Encoding]::new($false)) }
Push-Location $testRoot
pnpm install --frozen-lockfile
if ($LASTEXITCODE -ne 0) { throw 'Dependency install failed.' }
$env:TAURI_SIGNING_PRIVATE_KEY = Join-Path $env:USERPROFILE '.tauri\qlisa.key'
$securePassword = Read-Host 'Tauri signing key password' -AsSecureString
$passwordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($securePassword)
try { $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($passwordPointer) }
finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($passwordPointer); $securePassword.Dispose() }
pnpm exec tauri build --config $testConfig --bundles nsis -- --features asio-support
if ($LASTEXITCODE -ne 0) { throw 'Signed 1.5.5 baseline build failed.' }
$baselineExe = @(Get-ChildItem src-tauri/target/release/bundle/nsis -File -Filter '*1.5.5*.exe' | Where-Object Name -notlike '*.sig')
if ($baselineExe.Count -ne 1 -or -not (Test-Path -LiteralPath ($baselineExe[0].FullName + '.sig'))) { throw 'Expected one signed 1.5.5 NSIS installer.' }
$baselineDir = Join-Path $feedDir 'baseline'
New-Item -ItemType Directory -Force -Path $baselineDir | Out-Null
Copy-Item -LiteralPath $baselineExe[0].FullName -Destination $baselineDir
Copy-Item -LiteralPath ($baselineExe[0].FullName + '.sig') -Destination $baselineDir
```

Change only the four synchronized version fields in the disposable worktree, using the checked-in helper. This does not commit either version:

```powershell
@'
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const h = await import(pathToFileURL(resolve('scripts/release-helpers.mjs')).href);
const read = p => readFileSync(p, 'utf8');
const write = (p, value) => writeFileSync(p, value, 'utf8');
write('package.json', h.replaceJsonVersion(read('package.json'), '1.5.5', '1.5.6'));
write('src-tauri/tauri.conf.json', h.replaceJsonVersion(read('src-tauri/tauri.conf.json'), '1.5.5', '1.5.6'));
write('src-tauri/Cargo.toml', h.replaceCargoTomlVersion(read('src-tauri/Cargo.toml'), '1.5.5', '1.5.6'));
write('src-tauri/Cargo.lock', h.replaceCargoLockVersion(read('src-tauri/Cargo.lock'), '1.5.5', '1.5.6'));
'@ | node --input-type=module -
if ($LASTEXITCODE -ne 0) { throw 'Version update failed.' }
pnpm exec tauri build --config $testConfig --bundles nsis -- --features asio-support
if ($LASTEXITCODE -ne 0) { throw 'Signed 1.5.6 update build failed.' }
$updateExe = @(Get-ChildItem src-tauri/target/release/bundle/nsis -File -Filter '*1.5.6*.exe')
if ($updateExe.Count -ne 1 -or -not (Test-Path -LiteralPath ($updateExe[0].FullName + '.sig'))) { throw 'Expected one signed 1.5.6 NSIS updater installer.' }
New-Item -ItemType Directory -Force -Path $feedDir | Out-Null
Copy-Item -LiteralPath $updateExe[0].FullName -Destination $feedDir
Copy-Item -LiteralPath ($updateExe[0].FullName + '.sig') -Destination $feedDir
$assetUrl = 'http://127.0.0.1:8765/' + [uri]::EscapeDataString($updateExe[0].Name)
$signature = [IO.File]::ReadAllText($updateExe[0].FullName + '.sig')
if ([string]::IsNullOrWhiteSpace($signature) -or $signature -cne $signature.Trim()) { throw 'Updater signature is empty or has extra whitespace.' }
$latest = [ordered]@{
  version = '1.5.6'
  notes = 'Local updater test 1.5.5 to 1.5.6.'
  pub_date = [DateTimeOffset]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ss.fffZ')
  platforms = @{ 'windows-x86_64' = @{ url = $assetUrl; signature = $signature } }
}
$latestPath = Join-Path $feedDir 'latest.json'
[IO.File]::WriteAllText($latestPath, ($latest | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
$validLatest = [IO.File]::ReadAllText($latestPath)
Get-FileHash $baselineExe[0].FullName, $updateExe[0].FullName, ($updateExe[0].FullName + '.sig'), (Join-Path $feedDir 'latest.json') -Algorithm SHA256
```

Start a loopback-only server and verify the feed before launching the baseline:

```powershell
if (@(Get-NetTCPConnection -State Listen -LocalPort 8765 -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Port 8765 is already in use; stop the process and restart the procedure.' }
$python = (Get-Command python.exe -ErrorAction Stop).Source
$serverArgs = '-m http.server 8765 --bind 127.0.0.1 --directory "' + $feedDir + '"'
$feedServer = Start-Process -FilePath $python -ArgumentList $serverArgs -WindowStyle Hidden -PassThru
$served = $null
for ($attempt = 0; $attempt -lt 20; $attempt++) {
  if ($feedServer.HasExited) { throw 'Loopback feed server exited during startup.' }
  try { $served = Invoke-RestMethod -Uri $feedUrl -TimeoutSec 2; break }
  catch { Start-Sleep -Milliseconds 250 }
}
if ($null -eq $served) { throw 'Loopback feed server did not become ready.' }
if ($served.version -cne '1.5.6' -or $served.platforms.'windows-x86_64'.url -cne $assetUrl -or $served.platforms.'windows-x86_64'.signature -cne $signature) { throw 'Loopback feed does not match the signed 1.5.6 artifact.' }
```

## Install and verify

1. Create an isolated profile before installing or launching the test app. These environment variables apply to child processes started from this PowerShell session. The updater restart should inherit them:

   ```powershell
   $testRoaming = Join-Path $testProfile 'Roaming'
   $testLocal = Join-Path $testProfile 'Local'
   New-Item -ItemType Directory -Force -Path $testRoaming, $testLocal | Out-Null
   $originalAppData = $env:APPDATA
   $originalLocalAppData = $env:LOCALAPPDATA
   $env:APPDATA = $testRoaming
   $env:LOCALAPPDATA = $testLocal
   if (Test-Path -LiteralPath (Join-Path $testLocal 'Qlisa\runtime')) { throw 'Expected an empty isolated runtime directory before first launch.' }
   ```

   Install the baseline from `$baselineDir`. Accept the Windows UAC prompt. Check that it installs under `C:\Program Files\QlisaUpdaterTest`, not the per-user Programs directory. On the final installer page, clear **Run QlisaUpdaterTest** so it cannot start outside the controlled launch below. Keep the feed server running.

2. Launch the installed executable from this same PowerShell session:

   ```powershell
   $testExe = 'C:\Program Files\QlisaUpdaterTest\qlisa.exe'
   Start-Process -FilePath $testExe
   ```

   Let automatic Media Runtime preparation finish. Check that FFmpeg, ffprobe, and libmpv are under `$testLocal\Qlisa\runtime`, and that none are under `C:\Program Files\QlisaUpdaterTest`. Confirm About reports 1.5.5. Set a recognizable test preference and save a test workspace.
3. Test signature rejection while still on 1.5.5. In the same PowerShell session, replace the feed signature, request an update check, then attempt to download and install the offered 1.5.6 update:

   ```powershell
   $metadata = Get-Content -LiteralPath $latestPath -Raw | ConvertFrom-Json
   $metadata.platforms.'windows-x86_64'.signature = 'invalid test signature'
   [IO.File]::WriteAllText($latestPath, ($metadata | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
   ```

   The updater must reject the signature during download or installation, must not run the installer, and must keep 1.5.5. Then restore the valid feed:

   ```powershell
   [IO.File]::WriteAllText($latestPath, $validLatest, [Text.UTF8Encoding]::new($false))
   ```

   Confirm that the valid JSON is restored before continuing.
4. Check for update again. Confirm it offers 1.5.6 and the local notes. Download and install. Accept UAC if Windows asks. Confirm the app restarts, About reports 1.5.6, the install remains under Program Files, the preference and workspace remain, and the runtime stays under `$testLocal\Qlisa\runtime` without a second download.
5. Close QlisaUpdaterTest, then uninstall it from Windows Settings and accept UAC if requested. Confirm its Program Files directory is removed. Confirm production Qlisa and its settings remain intact. Record the result; remove temporary files only after reviewing evidence.

The test uses `$testProfile` for `%APPDATA%` and `%LOCALAPPDATA%`. Never point these variables at production data. Restore their original values before continuing to use the shell.

## Cleanup

```powershell
Stop-Process -Id $feedServer.Id -Force -ErrorAction SilentlyContinue
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
$env:APPDATA = $originalAppData
$env:LOCALAPPDATA = $originalLocalAppData
Pop-Location
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
foreach ($tempPath in @($testRoot, $testProfile, $feedDir, $testConfig)) {
  $fullPath = [IO.Path]::GetFullPath($tempPath)
  if ([IO.Path]::GetDirectoryName($fullPath).TrimEnd('\') -ine $tempRoot) { throw "Refusing cleanup outside the temp directory: $fullPath" }
}
git -C $sourceRoot worktree remove --force $testRoot
Remove-Item -LiteralPath $feedDir -Recurse -Force
Remove-Item -LiteralPath $testConfig -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $testProfile -Recurse -Force
```

Keep the installer and test evidence until the result is reviewed. Never push test tags or publish a GitHub Release for this procedure.
