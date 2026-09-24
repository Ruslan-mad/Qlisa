# Updater end-to-end test

This procedure is for a temporary, published GitHub Release after binary
compliance is ready. It is not a production release. Do not start while
`docs/RELEASING.md` reports missing FFmpeg or libmpv corresponding source or
notices.

## Separate test feed

Use this tag and endpoint only for the test builds. Use a separate product
name and application identifier. Tauri uses the product name for the Windows
install directory and the identifier for the app identity. Keep both values
the same in the `1.5.2` and `1.5.3` builds. This isolates the test install
from Qlisa while allowing the test updater to replace its own installation.
Use the stable product name `QlisaUpdaterTest` without spaces so Windows and
GitHub preserve the asset filename. It is visibly a separate test app.

```text
tag:      v1.5.3-updater-test
endpoint: https://github.com/Ruslan-mad/Qlisa/releases/download/v1.5.3-updater-test/latest.json
```

Tauri CLI `--config` merges a JSON file over the default configuration. Create
the override in the system temporary directory, outside the repository:

```powershell
$testConfig = Join-Path ([System.IO.Path]::GetTempPath()) 'qlisa-updater-test.tauri.conf.json'
$testEndpoint = 'https://github.com/Ruslan-mad/Qlisa/releases/download/v1.5.3-updater-test/latest.json'
$invalidTestTag = 'v1.5.3-updater-invalid-test'
$invalidConfig = Join-Path ([System.IO.Path]::GetTempPath()) 'qlisa-updater-invalid-test.tauri.conf.json'
$invalidEndpoint = "https://github.com/Ruslan-mad/Qlisa/releases/download/$invalidTestTag/latest.json"
@{
  productName = 'QlisaUpdaterTest'
  identifier = 'com.qlisa.updater.test'
  plugins = @{ updater = @{ endpoints = @($testEndpoint) } }
} | ConvertTo-Json -Depth 8 | ForEach-Object {
  [System.IO.File]::WriteAllText($testConfig, $_, [System.Text.UTF8Encoding]::new($false))
}
@{
  productName = 'QlisaUpdaterTest'
  identifier = 'com.qlisa.updater.test'
  plugins = @{ updater = @{ endpoints = @($invalidEndpoint) } }
} | ConvertTo-Json -Depth 8 | ForEach-Object {
  [System.IO.File]::WriteAllText($invalidConfig, $_, [System.Text.UTF8Encoding]::new($false))
}
```

Run all command blocks in the same PowerShell session so `$testConfig`,
`$sourceRoot`, and `$testRoot` retain their values.

Before building, verify that `src-tauri/tauri.conf.json` still contains the
production product name, identifier, and only the production
`/releases/latest/download/latest.json` endpoint. Verify the overlay has the
test product name and identifier and only the matching test endpoint. Build
each test artifact with its overlay passed explicitly to Tauri:

```powershell
pnpm exec tauri build --config $testConfig --bundles nsis -- --features asio-support
```

Do not use `scripts/publish.ps1` for this test. It deliberately requires the
production endpoint and does not accept a test-feed override. Do not edit the
tracked production config to point at the test release. Keep both overlays
until all test builds finish, then remove them and recheck the tracked config
before any production build:

Build in one detached disposable worktree created from a clean source commit.
These commands do not edit the regular checkout. Run them only after the
binary-compliance gate passes:

```powershell
$sourceRoot = (git rev-parse --show-toplevel).Trim()
$sourceStatus = @(git -C $sourceRoot status --porcelain)
if ($sourceStatus.Count -ne 0) { throw 'Source checkout must be clean.' }
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('Qlisa-updater-test-' + [guid]::NewGuid().ToString('N'))
$baseCommit = (git -C $sourceRoot rev-parse HEAD).Trim()
git -C $sourceRoot worktree add --detach $testRoot $baseCommit
```

Copy the compliance-verified ignored runtime files and notices listed in the
local manifest into that worktree. The copy step uses the reviewed manifest,
so it does not assume a notice filename:

```powershell
New-Item -ItemType Directory -Force -Path "$testRoot\src-tauri\vendor\mpv", "$testRoot\src-tauri\vendor\ffmpeg" | Out-Null
Copy-Item "$sourceRoot\.release-compliance.json" $testRoot
$compliance = Get-Content "$sourceRoot\.release-compliance.json" -Raw | ConvertFrom-Json
foreach ($component in $compliance.components) {
  foreach ($file in @($component.runtimeFiles) + @($component.notices)) {
    $sourcePath = [System.IO.Path]::GetFullPath((Join-Path $sourceRoot $file.path))
    $rootPrefix = $sourceRoot.TrimEnd('\') + '\'
    if (-not $sourcePath.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) { throw "Manifest path must stay inside source repo: $($file.path)" }
    if ((Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash -ne $file.sha256) { throw "Manifest SHA-256 mismatch: $($file.path)" }
    $relativePath = $sourcePath.Substring($rootPrefix.Length)
    $destinationPath = Join-Path $testRoot $relativePath
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destinationPath) | Out-Null
    Copy-Item -LiteralPath $sourcePath -Destination $destinationPath
    if ((Get-FileHash -LiteralPath $destinationPath -Algorithm SHA256).Hash -ne $file.sha256) { throw "Copied file SHA-256 mismatch: $relativePath" }
  }
}
# Corresponding-source archives stay at the absolute paths in the manifest.
Push-Location $testRoot
pnpm install --frozen-lockfile
```

Set the key path to `%USERPROFILE%\.tauri\qlisa.key`. Read its password through
a masked prompt. Do not print or save the password:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Join-Path $env:USERPROFILE '.tauri\qlisa.key'
$securePassword = Read-Host 'Tauri signing key password' -AsSecureString
$passwordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($securePassword)
try {
  $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($passwordPointer)
} finally {
  [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($passwordPointer)
  $securePassword.Dispose()
}
```

Keep the standard Qlisa public key in both builds. Build and sign the `1.5.2`
baseline, then save its installer and `.sig` outside the worktree for local
installation:

```powershell
$baselineDir = Join-Path ([System.IO.Path]::GetTempPath()) 'qlisa-updater-baseline'
New-Item -ItemType Directory -Force -Path $baselineDir | Out-Null
$invalidBaselineDir = Join-Path ([System.IO.Path]::GetTempPath()) 'qlisa-updater-invalid-baseline'
New-Item -ItemType Directory -Force -Path $invalidBaselineDir | Out-Null
pnpm exec tauri build --config $invalidConfig --bundles nsis -- --features asio-support
if ($LASTEXITCODE -ne 0) { throw 'The invalid-feed 1.5.2 build failed.' }
$invalidBaselineInstaller = Get-ChildItem "$testRoot\src-tauri\target\release\bundle\nsis" -File -Filter '*1.5.2*.exe'
if (@($invalidBaselineInstaller).Count -ne 1) { throw 'Expected one invalid-feed 1.5.2 NSIS installer.' }
Copy-Item $invalidBaselineInstaller.FullName $invalidBaselineDir
Copy-Item "$($invalidBaselineInstaller.FullName).sig" $invalidBaselineDir
pnpm exec tauri build --config $testConfig --bundles nsis -- --features asio-support
if ($LASTEXITCODE -ne 0) { throw 'The valid-feed 1.5.2 build failed.' }
$baselineInstaller = Get-ChildItem "$testRoot\src-tauri\target\release\bundle\nsis" -File -Filter '*1.5.2*.exe'
if (@($baselineInstaller).Count -ne 1) { throw 'Expected one 1.5.2 NSIS installer.' }
Copy-Item $baselineInstaller.FullName $baselineDir
Copy-Item "$($baselineInstaller.FullName).sig" $baselineDir
```

Update all four synchronized version fields in the test worktree using the
same checked-in replacement helpers as the release pipeline. This command
fails unless every field and the Cargo lock entry are exactly `1.5.2`:

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
if ($LASTEXITCODE -ne 0) { throw 'Version sync failed.' }
Push-Location src-tauri
cargo metadata --locked --no-deps --format-version 1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata check failed.' }
Pop-Location
pnpm exec tauri build --config $testConfig --bundles nsis -- --features asio-support
if ($LASTEXITCODE -ne 0) { throw 'The 1.5.3 build failed.' }
$updateDir = Join-Path $env:TEMP 'qlisa-updater-test-assets'
New-Item -ItemType Directory -Force -Path $updateDir | Out-Null
$updateInstallers = @(Get-ChildItem "$testRoot\src-tauri\target\release\bundle\nsis" -File -Filter '*1.5.3*.exe')
if ($updateInstallers.Count -ne 1) { throw 'Expected one 1.5.3 NSIS installer.' }
$updateInstaller = $updateInstallers[0]
$signaturePath = "$($updateInstaller.FullName).sig"
if (-not (Test-Path -LiteralPath $signaturePath)) { throw 'The 1.5.3 updater signature is missing.' }
Copy-Item $updateInstaller.FullName, $signaturePath $updateDir
$assetName = [System.Uri]::EscapeDataString($updateInstaller.Name)
$assetUrl = "https://github.com/Ruslan-mad/Qlisa/releases/download/v1.5.3-updater-test/$assetName"
$latestPath = Join-Path $updateDir 'latest.json'
node scripts/release.mjs --write-updater-metadata 1.5.3 docs/RELEASE_NOTES_1.5.3.md $assetUrl $signaturePath $latestPath
if ($LASTEXITCODE -ne 0) { throw 'Could not create test latest.json.' }
Get-FileHash $updateInstaller.FullName, $signaturePath, $latestPath -Algorithm SHA256
$releaseInstaller = Join-Path $updateDir $updateInstaller.Name
$releaseSignature = "$releaseInstaller.sig"
```

Do not commit either temporary version. The current release script does not
accept a Tauri config overlay, so these builds use the explicit Tauri CLI
command. Keep the test identifier and product name unchanged for all builds.
The NSIS installer filename is derived from that product name and version;
the command above selects the unique installer produced by the build and
records hashes for the installer, signature, and feed. Add every reviewed
corresponding-source archive and notice from the compliance manifest to the
test release. The installer distributes the same FFmpeg/libmpv binaries, so
their source and notice assets must accompany it:

```powershell
$releaseAssets = @($releaseInstaller, $releaseSignature, $latestPath)
foreach ($component in $compliance.components) {
  if (-not $component.correspondingSource -or -not $component.sourceArchive.includeInRelease) { throw "Incomplete release source for $($component.name)." }
  $files = @([pscustomobject]@{ path = $component.sourceArchive.path; sha256 = $component.sourceArchive.sha256 }) + @($component.notices)
  foreach ($file in $files) {
    $sourcePath = [string]$file.path
    if (-not [System.IO.Path]::IsPathRooted($sourcePath)) { $sourcePath = Join-Path $sourceRoot $sourcePath }
    $sourcePath = [System.IO.Path]::GetFullPath($sourcePath)
    if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) { throw "Release compliance asset is missing: $sourcePath" }
    if ((Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash -ine [string]$file.sha256) { throw "Release compliance SHA-256 mismatch: $sourcePath" }
    $destinationPath = Join-Path $updateDir ([System.IO.Path]::GetFileName($sourcePath))
    if (@($releaseAssets | Where-Object { [System.IO.Path]::GetFileName($_) -ieq [System.IO.Path]::GetFileName($sourcePath) }).Count -ne 0) { throw "Duplicate release asset name: $destinationPath" }
    Copy-Item -LiteralPath $sourcePath -Destination $destinationPath
    if ((Get-FileHash -LiteralPath $destinationPath -Algorithm SHA256).Hash -ine [string]$file.sha256) { throw "Copied release compliance asset hash mismatch: $destinationPath" }
    $releaseAssets += $destinationPath
  }
}
```

Upload every path in `$releaseAssets`. Keep `$baselineDir`,
`$invalidBaselineDir`, `$updateDir`, both configs, and the test worktree until
the end-to-end test and release cleanup end.

## Test release contents

Keep the signed `1.5.2` bootstrap installer local. Attach the `1.5.3` NSIS
installer and matching `.exe.sig` to the test release. Attach `latest.json`
with SemVer `1.5.3`, the exact installer asset URL under
`/releases/download/v1.5.3-updater-test/`, and the exact content of the
installer `.exe.sig`. Use the actual asset name from the Tauri build manifest;
do not guess it. Include test notes. Never attach these files to production.
Production `latest.json` must keep its production installer URL under
`/releases/latest/download/`; do not point production metadata at the test tag.

After the test has been authorized and both test tags exist remotely, create
the valid-feed release as a prerelease and explicitly clear GitHub's Latest
flag. Use:

```powershell
$releaseArgs = @('release', 'create', 'v1.5.3-updater-test') + $releaseAssets + @(
  '--prerelease', '--latest=false', '--verify-tag',
  '--notes-file', 'docs/RELEASE_NOTES_1.5.3.md'
)
& gh @releaseArgs
if ($LASTEXITCODE -ne 0) { throw 'Could not publish the temporary test release.' }
```

Verify the release reports `isPrerelease=true` and `isLatest=false` before
installing anything:

```powershell
$release = (gh release list --json tagName,isPrerelease,isLatest | ConvertFrom-Json) |
  Where-Object tagName -eq 'v1.5.3-updater-test'
if (@($release).Count -ne 1 -or -not $release.isPrerelease -or $release.isLatest) { throw 'Test release flags are unsafe.' }
```

Verify the uploaded assets against GitHub's SHA-256 digest and check that the
installer and feed URLs work. Download the installer once and compare its
bytes with the local build:

```powershell
$remoteRelease = gh api repos/Ruslan-mad/Qlisa/releases/tags/v1.5.3-updater-test | ConvertFrom-Json
foreach ($localAsset in $releaseAssets) {
  $name = [System.IO.Path]::GetFileName($localAsset)
  $matching = @($remoteRelease.assets | Where-Object name -ceq $name)
  $localHash = (Get-FileHash -LiteralPath $localAsset -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($matching.Count -ne 1 -or $matching[0].state -ne 'uploaded' -or $matching[0].digest -cne "sha256:$localHash") { throw "Published asset missing or SHA-256 differs: $name" }
}
$publishedFeed = Invoke-RestMethod -Uri $testEndpoint
$expectedSignature = [System.IO.File]::ReadAllText($signaturePath)
if ($publishedFeed.version -cne '1.5.3' -or $publishedFeed.platforms.'windows-x86_64'.url -cne $assetUrl -or $publishedFeed.platforms.'windows-x86_64'.signature -cne $expectedSignature) { throw 'Published updater feed differs from the reviewed local latest.json.' }
$downloadedInstaller = Join-Path $env:TEMP ('qlisa-updater-published-' + [guid]::NewGuid().ToString('N') + '.exe')
try {
  Invoke-WebRequest -Uri $assetUrl -OutFile $downloadedInstaller -UseBasicParsing
  if ((Get-FileHash -LiteralPath $downloadedInstaller -Algorithm SHA256).Hash -ine (Get-FileHash -LiteralPath $releaseInstaller -Algorithm SHA256).Hash) { throw 'Downloaded installer SHA-256 differs from the local build.' }
} finally {
  Remove-Item -LiteralPath $downloadedInstaller -Force -ErrorAction SilentlyContinue
}
```

`--verify-tag` requires a pre-existing remote tag. Create and push that tag
only after authorization for the test. The GitHub CLI option
`--latest=false` sets the API's `make_latest=false`.
Before publishing either test release, confirm repository settings allow a
published release and its tag to be deleted afterward. If immutable releases
prevent cleanup, stop before publishing; this test requires deletion.

Create a separate invalid-signature feed without editing the valid release.
Its `latest.json` points to the valid `1.5.3` installer but has invalid
signature text. The invalid `1.5.2` client already uses that second tag's
endpoint. Publish only the tampered JSON asset to that tag:

```powershell
$invalidFeedDir = Join-Path $env:TEMP 'qlisa-updater-invalid-test-assets'
New-Item -ItemType Directory -Force -Path $invalidFeedDir | Out-Null
$invalidMetadata = Get-Content -LiteralPath $latestPath -Raw | ConvertFrom-Json
$invalidMetadata.platforms.'windows-x86_64'.signature = 'invalid test signature'
$invalidLatestPath = Join-Path $invalidFeedDir 'latest.json'
[System.IO.File]::WriteAllText($invalidLatestPath, ($invalidMetadata | ConvertTo-Json -Depth 8), [System.Text.UTF8Encoding]::new($false))
gh release create $invalidTestTag $invalidLatestPath `
  --prerelease --latest=false --verify-tag `
  --notes 'Temporary invalid-signature updater test'
$invalidRelease = (gh release list --json tagName,isPrerelease,isLatest | ConvertFrom-Json) |
  Where-Object tagName -eq $invalidTestTag
if (@($invalidRelease).Count -ne 1 -or -not $invalidRelease.isPrerelease -or $invalidRelease.isLatest) { throw 'Invalid-signature test release flags are unsafe.' }
```

Run the negative test first. Install the invalid-feed `1.5.2` baseline and
confirm the updater rejects the offered installer and leaves the app at
`1.5.2`. Uninstall that test app. Then install the valid-feed `1.5.2`
baseline and continue the test. Both clients use the isolated
`QlisaUpdaterTest` identity. The negative test uses a separate feed and never
replaces an asset in the valid release.

## End-to-end checks

1. Install the signed `1.5.2` baseline. Launch it and wait at least five
   seconds. The startup update check must stay silent and must not block normal
   use. Open About and confirm the installed version is `1.5.2`.
2. Start a manual update check. Confirm it reports `1.5.3` and shows the test
   release notes. Start downloading. Confirm the displayed progress value
   changes at least once before completion.
3. With the update available, try installation while an audio cue is running.
   Repeat with a paused audio cue, then a running and paused video cue. The app
   must block installation in every active state. Stop the cues and confirm
   the install action becomes available.
4. Modify the test workspace without saving and try installation. The app
   must block it. Save the workspace and confirm the guard clears. Check that
   the project contents reload correctly.
5. Download the update. Confirm the valid signature is accepted and no
   signature error appears. With no active cues and a saved workspace,
   install `1.5.3`. Confirm Windows closes Qlisa, the installer runs, and Qlisa
   restarts. About must report `1.5.3`; test preferences and workspace data
   must remain intact. The production Qlisa installation and its settings
   must remain unchanged.
6. Confirm a further update check reports no newer version. Save logs and
   screenshots as test evidence without including user workspace data.
7. Delete the temporary GitHub Releases and their test tags after recording the
   result. Verify both are absent. Confirm the production release list and
   production endpoint remain unchanged.

Delete a test release and its remote tag with:

```powershell
gh release delete v1.5.3-updater-test --cleanup-tag --yes
gh release delete v1.5.3-updater-invalid-test --cleanup-tag --yes
git -C $sourceRoot tag -d v1.5.3-updater-test v1.5.3-updater-invalid-test
```

After all test tags and releases are gone, remove local test state:

```powershell
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $testConfig, $invalidConfig
git -C $sourceRoot worktree remove --force $testRoot
```

Keep test installers and logs until the result is reviewed. Delete `$baselineDir`,
`$invalidBaselineDir`, `$updateDir`, and `$invalidFeedDir` only after that review. Check `git status` in the production
checkout and verify its updater endpoint is still the production latest feed.

Do not mark the updater ready until every check passes. A local config merge
or a successful installer build alone does not test the updater.

## References

- Tauri CLI config merge: <https://v2.tauri.app/reference/cli/>
- Tauri configuration files and overrides:
  <https://v2.tauri.app/develop/configuration-files/>
- Tauri updater signatures and static feed format:
  <https://v2.tauri.app/plugin/updater/>
- GitHub CLI release creation and `--latest=false`:
  <https://cli.github.com/manual/gh_release_create>
- GitHub Releases API `make_latest` option:
  <https://docs.github.com/en/rest/releases/releases>
- Tauri bundle product name and Windows installer identity:
  <https://v2.tauri.app/reference/config/>
