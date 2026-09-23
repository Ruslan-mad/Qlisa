import test from "node:test";
import assert from "node:assert/strict";
import {
  artifactChanged,
  buildCommandArgs,
  compareVersions,
  createReleaseManifest,
  decideReleaseRollback,
  parseReleaseArgs,
  parseVersion,
  readCargoLockVersion,
  readCargoTomlVersion,
  replaceCargoLockVersion,
  replaceCargoTomlVersion,
  replaceJsonVersion,
} from "./release-helpers.mjs";

test("accepts strict three-part SemVer and rejects prerelease or padded components", () => {
  assert.deepEqual(parseVersion("1.20.3"), [1n, 20n, 3n]);
  for (const version of ["v1.2.3", "1.2", "1.2.3-beta.1", "1.02.3", "1.2.3+build"]) {
    assert.throws(() => parseVersion(version));
  }
});

test("compares versions by numeric components, including large values", () => {
  assert.equal(compareVersions("1.10.0", "1.9.99"), 1);
  assert.equal(compareVersions("1.3.3", "1.3.3"), 0);
  assert.equal(compareVersions("999999999999999999.0.0", "2.0.0"), 1);
});

test("parses dry-run and one explicit installer target", () => {
  assert.deepEqual(parseReleaseArgs(["1.4.0", "--dry-run"]), {
    version: "1.4.0",
    dryRun: true,
    bundle: undefined,
  });
  assert.deepEqual(parseReleaseArgs(["--", "1.4.0", "--dry-run"]), {
    version: "1.4.0",
    dryRun: true,
    bundle: undefined,
  });
  assert.deepEqual(parseReleaseArgs(["--bundle=nsis", "1.4.0"]), {
    version: "1.4.0",
    dryRun: false,
    bundle: "nsis",
  });
  assert.throws(() => parseReleaseArgs(["1.4.0", "--bundle", "all"]));
  assert.throws(() => parseReleaseArgs(["1.4.0", "--bundle", "msi", "--bundle", "nsis"]));
});

test("build commands forward asio-support through Cargo args and bundle only one target", () => {
  assert.deepEqual(buildCommandArgs({}), [
    "exec", "tauri", "build", "--no-bundle", "--", "--features", "asio-support",
  ]);
  assert.deepEqual(buildCommandArgs({ bundle: "nsis" }), [
    "exec", "tauri", "build", "--bundles", "nsis", "--", "--features", "asio-support",
  ]);
});

test("manifest contains full executable and installer file metadata", () => {
  const executable = { path: "C:/Qlisa/qlisa.exe", size: 1024, timestamp: "2026-09-18T12:00:00.000Z", sha256: "exe-hash" };
  const installer = { path: "C:/Qlisa/qlisa.msi", size: 2048, timestamp: "2026-09-18T12:01:00.000Z", sha256: "msi-hash" };
  assert.deepEqual(createReleaseManifest({
    version: "1.4.0",
    commit: "abc123",
    executable,
    installer,
    builtAt: "2026-09-18T12:02:00.000Z",
    bundle: "msi",
  }), {
    version: "1.4.0",
    commit: "abc123",
    sha256: "exe-hash",
    builtAt: "2026-09-18T12:02:00.000Z",
    bundledTarget: "msi",
    executable,
    installer,
  });
  assert.equal(createReleaseManifest({
    version: "1.4.0", commit: "abc123", executable, builtAt: "2026-09-18T12:02:00.000Z",
  }).installer, null);
});

test("rollback proceeds only when HEAD was read and is unchanged", () => {
  assert.equal(decideReleaseRollback("abc123", "abc123"), "rollback");
  assert.equal(decideReleaseRollback("abc123", "def456"), "preserve");
  assert.equal(decideReleaseRollback("abc123", undefined), "unknown");
  assert.equal(decideReleaseRollback("abc123", ""), "unknown");
  assert.equal(decideReleaseRollback(undefined, "abc123"), "unknown");
});

test("artifact freshness requires a newly created or metadata-changed file", () => {
  const before = { size: 1024, mtimeMs: 10 };
  assert.equal(artifactChanged(undefined, before), true);
  assert.equal(artifactChanged(before, { ...before }), false);
  assert.equal(artifactChanged(before, { size: 2048, mtimeMs: 10 }), true);
  assert.equal(artifactChanged(before, { size: 1024, mtimeMs: 11 }), true);
  assert.equal(artifactChanged(before, undefined), false);
});

test("updates only the top-level JSON version field", () => {
  const source = '{\n  "version": "1.3.3",\n  "metadata": {\n    "version": "0.1.0"\n  }\n}\n';
  assert.equal(
    replaceJsonVersion(source, "1.3.3", "1.4.0"),
    '{\n  "version": "1.4.0",\n  "metadata": {\n    "version": "0.1.0"\n  }\n}\n',
  );
});

test("updates Cargo package and lock versions without touching dependencies", () => {
  const cargo = '[package]\nname = "qlisa"\nversion = "1.3.3"\n\n[dependencies]\nother = "1.3.3"\n';
  assert.equal(readCargoTomlVersion(cargo), "1.3.3");
  assert.equal(
    replaceCargoTomlVersion(cargo, "1.3.3", "1.4.0"),
    '[package]\nname = "qlisa"\nversion = "1.4.0"\n\n[dependencies]\nother = "1.3.3"\n',
  );

  const lock = '[[package]]\nname = "other"\nversion = "1.3.3"\n\n[[package]]\nname = "qlisa"\nversion = "1.3.3"\ndependencies = [\n "other",\n]\n';
  assert.equal(readCargoLockVersion(lock), "1.3.3");
  assert.equal(
    replaceCargoLockVersion(lock, "1.3.3", "1.4.0"),
    '[[package]]\nname = "other"\nversion = "1.3.3"\n\n[[package]]\nname = "qlisa"\nversion = "1.4.0"\ndependencies = [\n "other",\n]\n',
  );
});
