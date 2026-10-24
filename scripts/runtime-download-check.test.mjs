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
  return { schemaVersion: 1, components: [
    { id: "ffmpeg-test", kind: "static-runtime-build", archive: { url } },
    { id: "libmpv", build: { archiveUrl: url } },
    { id: "ndi-runtime", kind: "external-user-installed-runtime", sourceUrl: "https://example.test/ndi" },
  ] };
}

const okResponse = { status: 200, ok: true };

test("checks the real manifest schema and its exact required component URLs", async () => {
  const seen = [];
  const manifestPath = fileURLToPath(new URL("./runtime-manifest.json", import.meta.url));
  await checkRuntimeDownloads(manifestPath, { fetchRequest: async (url, method) => { seen.push([url, method]); return okResponse; } });
  const manifest = JSON.parse(await (await import("node:fs/promises")).readFile(manifestPath, "utf8"));
  assert.deepEqual(seen, [
    [manifest.components.find((c) => c.kind === "static-runtime-build" && c.id.startsWith("ffmpeg-")).archive.url, "HEAD"],
    [manifest.components.find((c) => c.id === "libmpv").build.archiveUrl, "HEAD"],
  ]);
});

test("rejects invalid schema and duplicate or missing required components", async () => {
  await assert.rejects(checkRuntimeDownloadUrls({ components: [] }), /schemaVersion 1/);
  const valid = manifest("https://example.test/archive");
  await assert.rejects(checkRuntimeDownloadUrls({ ...valid, components: valid.components.filter((c) => c.id !== "libmpv") }), /exactly one libmpv/);
  await assert.rejects(checkRuntimeDownloadUrls({ ...valid, components: [...valid.components, valid.components[0]] }), /exactly one FFmpeg/);
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
  delete missingUrl.components[0].archive.url;
  await assert.rejects(checkRuntimeDownloadUrls(missingUrl), /FFmpeg: missing download URL/);
});
