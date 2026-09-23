import { createHash, randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  artifactChanged,
  buildCommandArgs,
  compareVersions,
  createReleaseManifest,
  createUpdaterMetadata,
  decideReleaseRollback,
  parseReleaseArgs,
  readCargoLockVersion,
  readCargoTomlVersion,
  replaceCargoLockVersion,
  replaceCargoTomlVersion,
  replaceJsonVersion,
  validateReleasePreparationOptions,
  validateUpdaterMetadata,
} from "./release-helpers.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const FILES = {
  packageJson: join(ROOT, "package.json"),
  cargoToml: join(ROOT, "src-tauri", "Cargo.toml"),
  tauriConfig: join(ROOT, "src-tauri", "tauri.conf.json"),
  cargoLock: join(ROOT, "src-tauri", "Cargo.lock"),
};
const VERSION_PATHS = Object.values(FILES).map((path) => path.slice(ROOT.length + 1));
const NETWORK_RUNTIME_FILES = [
  "src-tauri/vendor/mpv/libmpv-2.dll",
  "src-tauri/vendor/ffmpeg/ffmpeg.exe",
  "src-tauri/vendor/ffmpeg/ffprobe.exe",
  "src-tauri/vendor/ffmpeg/LICENSE",
  "src-tauri/vendor/ffmpeg/README-Gyan-build.txt",
];

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? ROOT,
    encoding: "utf8",
    stdio: options.inherit ? "inherit" : "pipe",
    shell: options.shell ?? false,
    windowsHide: true,
  });

  if (result.error) throw result.error;
  if (result.status !== 0) {
    const output = [result.stdout, result.stderr].filter(Boolean).join("\n").trim();
    throw new Error(`${command} ${args.join(" ")} failed with exit code ${result.status}${output ? `:\n${output}` : ""}`);
  }
  return result.stdout?.trim() ?? "";
}

function git(args) {
  return run("git", args, { cwd: ROOT });
}

function ensureCleanTree() {
  const status = git(["status", "--porcelain", "--untracked-files=all"]);
  if (status) {
    throw new Error(`Git working tree must be clean before a release. Current changes:\n${status}`);
  }
}

function readVersionState() {
  const packageJson = readFileSync(FILES.packageJson, "utf8");
  const cargoToml = readFileSync(FILES.cargoToml, "utf8");
  const tauriConfig = readFileSync(FILES.tauriConfig, "utf8");
  const cargoLock = readFileSync(FILES.cargoLock, "utf8");
  const versions = {
    packageJson: JSON.parse(packageJson).version,
    cargoToml: readCargoTomlVersion(cargoToml),
    tauriConfig: JSON.parse(tauriConfig).version,
    cargoLock: readCargoLockVersion(cargoLock),
  };

  const unique = new Set(Object.values(versions));
  if (unique.size !== 1) {
    throw new Error(`Version mismatch: ${Object.entries(versions).map(([name, version]) => `${name}=${version}`).join(", ")}`);
  }
  return { version: versions.packageJson, contents: { packageJson, cargoToml, tauriConfig, cargoLock } };
}

function printDryRun(currentVersion, release) {
  const buildArgs = buildCommandArgs(release);

  console.log("Dry run: no files will be changed, committed, or built.");
  console.log(`Current version: ${currentVersion}`);
  console.log(`Requested version: ${release.version}`);
  console.log("Planned actions:");
  console.log("1. Update package.json, src-tauri/Cargo.toml, src-tauri/tauri.conf.json, and the qlisa entry in src-tauri/Cargo.lock.");
  console.log("2. Validate the Cargo lockfile: cargo metadata --locked --no-deps --format-version 1 (from src-tauri/).");
  console.log(`3. Commit only those version files: git add -- ${VERSION_PATHS.join(" ")} && git commit -m "release: v${release.version}".`);
  console.log(`4. Build from that release commit: pnpm ${buildArgs.join(" ")}`);
  console.log(`5. Record executable, installer, and updater artifacts in src-tauri/target/release/qlisa.release.json.`);
}

function ensureWindowsAsioPrerequisites() {
  if (process.platform !== "win32") throw new Error("Qlisa release builds are Windows-only and must include asio-support.");

  const sdkDirectory = join(ROOT, "vendor", "asiosdk");
  if (!existsSync(sdkDirectory)) throw new Error(`ASIO SDK is missing: ${sdkDirectory}`);

  for (const relativePath of NETWORK_RUNTIME_FILES) {
    const path = join(ROOT, relativePath);
    if (!existsSync(path) || statSync(path).size === 0) {
      const preparation = relativePath.includes("/ffmpeg/")
        ? " Run scripts/sync-network-runtime.ps1 to download the pinned Gyan archive, verify its SHA-256, and extract FFmpeg/ffprobe."
        : "";
      throw new Error(`Required Windows release runtime file is missing or empty: ${path}.${preparation}`);
    }
  }
  const ndiRuntime = join(ROOT, "src-tauri", "vendor", "ndi", "Processing.NDI.Lib.x64.dll");
  if (existsSync(ndiRuntime)) {
    throw new Error(`NDI Runtime DLL must not be bundled with Qlisa: ${ndiRuntime}. Remove this stale local copy; users install NDI Runtime separately.`);
  }
}

function writeAtomically(path, contents) {
  const temporaryPath = `${path}.release-${process.pid}-${randomBytes(4).toString("hex")}.tmp`;
  try {
    writeFileSync(temporaryPath, contents, "utf8");
    renameSync(temporaryPath, path);
  } finally {
    if (existsSync(temporaryPath)) unlinkSync(temporaryPath);
  }
}

function runCargoMetadata(expectedVersion) {
  const output = run("cargo", ["metadata", "--locked", "--no-deps", "--format-version", "1"], {
    cwd: join(ROOT, "src-tauri"),
  });
  const metadata = JSON.parse(output);
  const rootPackage = metadata.packages.find((pkg) => pkg.name === "qlisa" && pkg.manifest_path === FILES.cargoToml);
  if (!rootPackage || rootPackage.version !== expectedVersion) {
    throw new Error(`cargo metadata did not resolve qlisa at ${expectedVersion}`);
  }
}

function fileInfo(path) {
  const contents = readFileSync(path);
  const stats = statSync(path);
  return {
    path,
    size: stats.size,
    timestamp: stats.mtime.toISOString(),
    sha256: createHash("sha256").update(contents).digest("hex"),
  };
}

function snapshotFile(path) {
  if (!existsSync(path)) return undefined;
  const stats = statSync(path);
  return { size: stats.size, mtimeMs: stats.mtimeMs };
}

function collectFiles(directory, extension) {
  if (!existsSync(directory)) return [];
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...collectFiles(path, extension));
    else if (entry.isFile() && extname(entry.name).toLowerCase() === extension) files.push(path);
  }
  return files;
}

function artifactSnapshot(bundle) {
  if (!bundle) return new Map();
  const directory = join(ROOT, "src-tauri", "target", "release", "bundle", bundle);
  const extension = bundle === "msi" ? ".msi" : ".exe";
  return new Map(collectFiles(directory, extension).map((path) => {
    return [path, snapshotFile(path)];
  }));
}

function updaterArtifactSnapshot(bundle) {
  if (bundle !== "nsis") return new Map();
  const directory = join(ROOT, "src-tauri", "target", "release", "bundle", "nsis");
  const installers = collectFiles(directory, ".exe");
  const paths = [...installers, ...installers.map((path) => `${path}.sig`).filter((path) => existsSync(path))];
  return new Map(paths.map((path) => [path, snapshotFile(path)]));
}

function findNewInstaller(bundle, before) {
  if (!bundle) return undefined;
  const directory = join(ROOT, "src-tauri", "target", "release", "bundle", bundle);
  const extension = bundle === "msi" ? ".msi" : ".exe";
  const changed = collectFiles(directory, extension)
    .filter((path) => {
      const previous = before.get(path);
      return artifactChanged(previous, snapshotFile(path));
    })
    .map((path) => {
      return fileInfo(path);
    });

  if (changed.length !== 1) {
    throw new Error(`Expected exactly one newly built ${bundle.toUpperCase()} installer, found ${changed.length} in ${directory}`);
  }
  return changed[0];
}

function findNewUpdaterArtifacts(before, installer) {
  const signaturePath = `${installer.path}.sig`;
  if (!existsSync(signaturePath) || !artifactChanged(before.get(signaturePath), snapshotFile(signaturePath))) {
    throw new Error(`NSIS installer updater signature was not freshly generated: ${signaturePath}`);
  }
  return { updaterBundle: installer, updaterSignature: fileInfo(signaturePath) };
}

function printArtifact(label, artifact) {
  console.log(`${label}: ${artifact.path}`);
  console.log(`  Size: ${artifact.size} bytes`);
  console.log(`  Timestamp: ${artifact.timestamp}`);
  console.log(`  SHA256: ${artifact.sha256}`);
}

function rollbackVersionFiles(originals, written, staged, initialHead) {
  let currentHead;
  try {
    currentHead = git(["rev-parse", "HEAD"]);
  } catch (error) {
    console.error(`Cannot read Git HEAD (${error.message}); skipping version rollback to avoid changing files with unknown history state.`);
    return;
  }
  const decision = decideReleaseRollback(initialHead, currentHead);
  if (decision === "unknown") {
    console.error("Git HEAD state is unknown; skipping version rollback to avoid changing files with uncertain history state.");
    return;
  }
  if (decision === "preserve") {
    console.error("Git HEAD changed since the release started; leaving history and version files untouched.");
    return;
  }

  for (const path of [...written].reverse()) {
    try {
      const current = readFileSync(path, "utf8");
      if (current === written.get(path)) writeAtomically(path, originals.get(path));
      else console.error(`Rollback skipped ${path}: its contents changed after this script wrote it.`);
    } catch (error) {
      console.error(`Could not restore ${path}: ${error.message}`);
    }
  }

  if (staged) {
    try {
      git(["restore", "--staged", "--", ...VERSION_PATHS]);
    } catch (error) {
      console.error(`Could not unstage release version files: ${error.message}`);
    }
  }
}

function release(releaseOptions) {
  validateReleasePreparationOptions(releaseOptions);
  ensureCleanTree();
  const initialHead = git(["rev-parse", "HEAD"]);
  const state = readVersionState();
  if (compareVersions(releaseOptions.version, state.version) <= 0) {
    throw new Error(`Requested version ${releaseOptions.version} must be greater than current version ${state.version}`);
  }

  if (releaseOptions.dryRun) {
    printDryRun(state.version, releaseOptions);
    return;
  }

  ensureWindowsAsioPrerequisites();

  const originals = new Map(Object.entries(FILES).map(([key, path]) => [path, state.contents[key]]));
  const nextContents = new Map([
    [FILES.packageJson, replaceJsonVersion(state.contents.packageJson, state.version, releaseOptions.version)],
    [FILES.cargoToml, replaceCargoTomlVersion(state.contents.cargoToml, state.version, releaseOptions.version)],
    [FILES.tauriConfig, replaceJsonVersion(state.contents.tauriConfig, state.version, releaseOptions.version)],
    [FILES.cargoLock, replaceCargoLockVersion(state.contents.cargoLock, state.version, releaseOptions.version)],
  ]);
  const written = new Map();
  let stagingAttempted = false;

  try {
    for (const [path, contents] of nextContents) {
      writeAtomically(path, contents);
      written.set(path, contents);
    }

    const updated = readVersionState();
    if (updated.version !== releaseOptions.version) throw new Error("Updated version files did not stay synchronized.");
    runCargoMetadata(releaseOptions.version);

    stagingAttempted = true;
    git(["add", "--", ...VERSION_PATHS]);
    run("git", ["commit", "-m", `release: v${releaseOptions.version}`], { cwd: ROOT, inherit: true });
    const commit = git(["rev-parse", "HEAD"]);
    if (commit === initialHead) throw new Error("Git reported success but did not create a release commit.");
    stagingAttempted = false;
    ensureCleanTree();

    const executablePath = join(ROOT, "src-tauri", "target", "release", "qlisa.exe");
    const executableBefore = snapshotFile(executablePath);
    const beforeBundle = artifactSnapshot(releaseOptions.bundle);
    const beforeUpdater = updaterArtifactSnapshot(releaseOptions.bundle);
    const buildArgs = buildCommandArgs(releaseOptions);

    console.log(`Building release from commit ${commit} with ASIO support...`);
    run(process.env.QLISA_PNPM_EXECUTABLE ?? "pnpm.cmd", buildArgs, { cwd: ROOT, shell: true, inherit: true });

    const executableAfter = snapshotFile(executablePath);
    if (!executableAfter) throw new Error(`Release executable was not created: ${executablePath}`);
    if (!artifactChanged(executableBefore, executableAfter)) {
      throw new Error(`Release executable was not refreshed by this build; refusing to record a stale artifact: ${executablePath}`);
    }
    const executable = fileInfo(executablePath);
    const installer = findNewInstaller(releaseOptions.bundle, beforeBundle);
    const { updaterBundle, updaterSignature } = findNewUpdaterArtifacts(beforeUpdater, installer);
    const builtAt = new Date().toISOString();
    const manifest = createReleaseManifest({
      version: releaseOptions.version,
      commit,
      executable,
      installer,
      updaterBundle,
      updaterSignature,
      builtAt,
      bundle: releaseOptions.bundle,
    });
    const manifestPath = join(ROOT, "src-tauri", "target", "release", "qlisa.release.json");
    mkdirSync(dirname(manifestPath), { recursive: true });
    writeAtomically(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

    console.log(`Release commit: ${commit}`);
    console.log(`Build completed at: ${builtAt}`);
    printArtifact("Executable", executable);
    if (installer) printArtifact("Installer", installer);
    if (updaterBundle) printArtifact("Updater bundle", updaterBundle);
    if (updaterSignature) printArtifact("Updater signature", updaterSignature);
    else console.log("Installer: not built (--no-bundle)");
    console.log(`Manifest: ${manifestPath}`);
  } catch (error) {
    rollbackVersionFiles(originals, written, stagingAttempted, initialHead);
    throw error;
  }
}

function main() {
  if (process.platform !== "win32") throw new Error("Qlisa release builds are Windows-only and must include asio-support.");
  const args = process.argv.slice(2);
  if (args[0] === "--write-updater-metadata") {
    if (args.length !== 6) throw new Error("Usage: release.mjs --write-updater-metadata X.Y.Z NOTES_FILE HTTPS_URL SIGNATURE_FILE OUTPUT_FILE");
    const [, version, notesPath, url, signaturePath, outputPath] = args;
    const notes = readFileSync(notesPath, "utf8");
    const signature = readFileSync(signaturePath, "utf8");
    const metadata = createUpdaterMetadata({ version, notes, pubDate: new Date().toISOString(), url, signature });
    validateUpdaterMetadata(metadata, { version, url, signature });
    writeAtomically(resolve(outputPath), `${JSON.stringify(metadata, null, 2)}\n`);
    return;
  }
  const options = parseReleaseArgs(process.argv.slice(2));
  validateReleasePreparationOptions(options);
  if (!options.prepareRelease && !options.dryRun) {
    throw new Error("Use scripts/publish.ps1 for release preparation, or pass --dry-run to inspect version/build arguments.");
  }
  release(options);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(`Release failed: ${error.message}`);
    process.exitCode = 1;
  }
}
