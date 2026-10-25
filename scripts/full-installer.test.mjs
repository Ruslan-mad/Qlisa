import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const readJson = (path) => JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));

test("full installer payload pins runtime hashes and official redistributables", () => {
  const full = readJson("./full-installer-manifest.json");
  const runtime = readJson("./runtime-manifest.json");
  const ffmpeg = runtime.components.find((component) => component.id === "ffmpeg-gyan-full-9.0.2");
  const mpv = runtime.components.find((component) => component.id === "libmpv");
  const fullHashes = Object.fromEntries(full.runtimeFiles.map(({ fileName, sha256 }) => [fileName, sha256]));
  assert.equal(full.schemaVersion, 1);
  assert.equal(fullHashes["ffmpeg.exe"], ffmpeg.runtimeFiles.find((file) => file.fileName === "ffmpeg.exe").sha256);
  assert.equal(fullHashes["ffprobe.exe"], ffmpeg.runtimeFiles.find((file) => file.fileName === "ffprobe.exe").sha256);
  assert.equal(fullHashes["libmpv-2.dll"], mpv.sha256);
  assert.match(fullHashes["vulkan-1.dll"], /^[a-f\d]{64}$/);
  assert.equal(full.ndiRuntime.version, "6.3.2.0");
  assert.equal(full.ndiRuntime.sha256, "7ee73eedb56402bca5100868353dbab4e944b6c37d2d9881580698a3b61346cd");
  assert.equal(full.ndiRuntime.signer, "Vizrt AG");
  assert.equal(full.webview2OfflineInstaller.signer, "Microsoft Corporation");
});

test("compact and full Windows installers have distinct resource and WebView modes", () => {
  const compact = readJson("../src-tauri/tauri.windows.conf.json");
  const full = readJson("../src-tauri/tauri.full-windows.conf.json");
  const compactResources = JSON.stringify(compact.bundle.resources).toLowerCase();
  const fullResources = JSON.stringify(full.bundle.resources).toLowerCase();
  assert.equal(compact.bundle.windows.webviewInstallMode.type, "embedBootstrapper");
  assert.equal(compact.bundle.windows.nsis.installerHooks, "installer-hooks/compact.nsh");
  assert.doesNotMatch(compactResources, /ffmpeg\.exe|ffprobe\.exe|libmpv-2\.dll|ndi-6-runtime\.exe/);
  assert.equal(full.bundle.windows.webviewInstallMode.type, "offlineInstaller");
  assert.equal(full.bundle.windows.nsis.installMode, "perMachine");
  assert.match(full.bundle.windows.nsis.installerHooks, /full\.nsh$/);
  for (const name of ["full-package.json", "ffmpeg.exe", "ffprobe.exe", "libmpv-2.dll", "vulkan-1.dll", "NDI-6-Runtime.exe"]) {
    assert.ok(fullResources.toLowerCase().includes(name.toLowerCase()), `full installer resource missing: ${name}`);
  }
});

test("NDI setup hook skips newer runtimes and treats reboot-required as success", () => {
  const hook = readFileSync(new URL("../src-tauri/installer-hooks/full.nsh", import.meta.url), "utf8");
  assert.match(hook, /\$PROGRAMFILES64\\NDI\\NDI 6 Runtime\\v6\\Processing\.NDI\.Lib\.x64\.dll/);
  assert.match(hook, /VersionCompare.*6\.3\.2\.0/);
  assert.match(hook, /RESTARTEXITCODE=3010/);
  assert.match(hook, /\$R0 == 3010[\s\S]*?SetRebootFlag true/);
  assert.match(hook, /SetErrorLevel 1[\s\S]*?Quit/);
  assert.match(hook, /\/SD IDOK/);
});

test("compact updater removes only full-package payload and leaves the per-user cache alone", () => {
  const hook = readFileSync(new URL("../src-tauri/installer-hooks/compact.nsh", import.meta.url), "utf8");
  for (const path of [
    "$INSTDIR\\full-runtime\\full-package.json",
    "$INSTDIR\\full-runtime\\ffmpeg.exe",
    "$INSTDIR\\full-runtime\\ffprobe.exe",
    "$INSTDIR\\full-runtime\\libmpv-2.dll",
    "$INSTDIR\\full-runtime\\vulkan-1.dll",
    "$INSTDIR\\ndi\\NDI-6-Runtime.exe",
  ]) assert.ok(hook.includes(`Delete "${path}"`), `compact hook does not remove ${path}`);
  assert.doesNotMatch(hook, /LOCALAPPDATA|ProgramFiles|Processing\.NDI\.Lib/);
  assert.doesNotMatch(hook, /RMDir\s+\/r/i);
});
