import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { checkRuntimeDownloadUrls, checkRuntimeDownloads } from "./runtime-download-check.mjs";
import { fileURLToPath } from "node:url";

async function withServer(handler, run) {
  const server = createServer(handler);
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  try { await run(base); } finally { await new Promise((resolve) => server.close(resolve)); }
}

function manifest(url) {
  let assetFileName = "runtime-package.zip";
  try { assetFileName = decodeURIComponent(new URL(url).pathname.split("/").pop()) || assetFileName; } catch { /* URL validation reports malformed URLs. */ }
  const component = (id, fileName) => ({
    id,
    asset: { fileName: assetFileName, url, sha256: "a".repeat(64), format: "zip" },
    runtimeFiles: [{ fileName, sha256: "b".repeat(64) }],
  });
  return { schemaVersion: 1, recommendedKitId: "kit-test", approvedKits: [{
    id: "kit-test",
    architecture: "windows-x86_64",
    components: [component("ffmpeg", "ffmpeg.exe"), component("libmpv", "libmpv-2.dll")],
  }] };
}

const okResponse = { status: 200, ok: true };

test("checks the real manifest schema and every approved kit asset URL", async () => {
  const seen = [];
  const manifestPath = fileURLToPath(new URL("./runtime-manifest.json", import.meta.url));
  await checkRuntimeDownloads(manifestPath, { fetchRequest: async (url, method) => { seen.push([url, method]); return okResponse; } });
  const manifest = JSON.parse(await (await import("node:fs/promises")).readFile(manifestPath, "utf8"));
  const expected = manifest.approvedKits.flatMap((kit) => kit.components.map((component) => [component.asset.url, "HEAD"]));
  assert.deepEqual(seen, expected);
});

test("rejects invalid schema, unapproved recommendation, duplicate kits or components, and invalid digests", async () => {
  await assert.rejects(checkRuntimeDownloadUrls({ approvedKits: [] }), /schemaVersion 1/);
  const valid = manifest("https://example.test/archive");
  await assert.rejects(checkRuntimeDownloadUrls({ ...valid, recommendedKitId: "unknown" }), /not approved/);
  await assert.rejects(checkRuntimeDownloadUrls({ ...valid, approvedKits: [...valid.approvedKits, valid.approvedKits[0]] }), /IDs must be unique/);
  const missingMpv = structuredClone(valid);
  missingMpv.approvedKits[0].components = missingMpv.approvedKits[0].components.filter((c) => c.id !== "libmpv");
  await assert.rejects(checkRuntimeDownloadUrls(missingMpv), /exactly one libmpv/);
  const duplicateFfmpeg = structuredClone(valid);
  duplicateFfmpeg.approvedKits[0].components.push(duplicateFfmpeg.approvedKits[0].components[0]);
  await assert.rejects(checkRuntimeDownloadUrls(duplicateFfmpeg), /exactly one ffmpeg/);
  const invalidDigest = structuredClone(valid);
  invalidDigest.approvedKits[0].components[0].asset.sha256 = "bad";
  await assert.rejects(checkRuntimeDownloadUrls(invalidDigest), /invalid ffmpeg asset/);
  const invalidFileDigest = structuredClone(valid);
  invalidFileDigest.approvedKits[0].components[1].runtimeFiles[0].sha256 = "bad";
  await assert.rejects(checkRuntimeDownloadUrls(invalidFileDigest), /invalid libmpv runtime-file hash/);
  const mismatchedAssetName = structuredClone(valid);
  mismatchedAssetName.approvedKits[0].components[0].asset.fileName = "other.zip";
  await assert.rejects(checkRuntimeDownloadUrls(mismatchedAssetName), /filename.*does not match/);
});

test("accepts successful responses and follows redirects", async () => {
  await withServer((req, res) => {
    if (req.url === "/redirect") { res.writeHead(302, { Location: "/ok" }).end(); return; }
    res.writeHead(200).end();
  }, async (base) => {
    await checkRuntimeDownloadUrls(manifest(`${base}/redirect`), { allowInsecureLocalhost: true });
  });
});

test("reports labeled 404 response with URL", async () => {
  await withServer((_req, res) => res.writeHead(404).end(), async (base) => {
    await assert.rejects(checkRuntimeDownloadUrls(manifest(`${base}/missing`), { allowInsecureLocalhost: true }), /FFmpeg download check failed \(HTTP 404\).*http:/);
  });
});

test("uses a one-byte ranged GET only when HEAD is unsupported", async () => {
  const methods = [];
  await withServer((req, res) => {
    methods.push([req.method, req.headers.range]);
    if (req.method === "HEAD") res.writeHead(405).end();
    else res.writeHead(206).end("x");
  }, async (base) => {
    await checkRuntimeDownloadUrls(manifest(`${base}/range`), { allowInsecureLocalhost: true });
    assert.deepEqual(methods, [["HEAD", undefined], ["GET", "bytes=0-0"], ["HEAD", undefined], ["GET", "bytes=0-0"]]);
  });
});

test("times out and rejects malformed or non-HTTPS URLs", async () => {
  await withServer((_req, _res) => {}, async (base) => {
    await assert.rejects(checkRuntimeDownloadUrls(manifest(`${base}/hang`), { timeoutMs: 30, allowInsecureLocalhost: true }), /FFmpeg download check failed \(timed out after 30 ms\)/);
  });
  for (const url of ["not a url", "http://example.test/archive", "https://user:pass@example.test/archive"]) {
    await assert.rejects(checkRuntimeDownloadUrls(manifest(url)), /FFmpeg: (malformed|expected an HTTPS)/);
  }
  const missingUrl = manifest("https://example.test/archive");
  delete missingUrl.approvedKits[0].components[0].asset.url;
  await assert.rejects(checkRuntimeDownloadUrls(missingUrl), /FFmpeg: missing download URL/);
});
