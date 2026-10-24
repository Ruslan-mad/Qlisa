import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

function requiredDownloads(manifest) {
  if (manifest?.schemaVersion !== 1) throw new Error("Runtime manifest: expected schemaVersion 1");
  if (!Array.isArray(manifest.components)) throw new Error("Runtime manifest: components must be an array");
  const ffmpeg = manifest.components.filter((component) => component?.kind === "static-runtime-build" && typeof component.id === "string" && component.id.startsWith("ffmpeg-"));
  const mpv = manifest.components.filter((component) => component?.id === "libmpv");
  if (ffmpeg.length !== 1) throw new Error(`Runtime manifest: expected exactly one FFmpeg static-runtime-build component, found ${ffmpeg.length}`);
  if (mpv.length !== 1) throw new Error(`Runtime manifest: expected exactly one libmpv component, found ${mpv.length}`);
  return [
    ["FFmpeg", ffmpeg[0].archive?.url],
    ["libmpv", mpv[0].build?.archiveUrl],
  ];
}

function validateUrl(label, value, allowInsecureLocalhost = false) {
  if (typeof value !== "string" || !value.trim()) throw new Error(`${label}: missing download URL`);
  let url;
  try { url = new URL(value); } catch { throw new Error(`${label}: malformed download URL: ${value}`); }
  const localHttp = allowInsecureLocalhost && url.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
  if ((!localHttp && url.protocol !== "https:") || !url.hostname || url.username || url.password) {
    throw new Error(`${label}: expected an HTTPS URL without credentials: ${value}`);
  }
  return url.href;
}

async function request(url, method, timeoutMs) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, {
      method,
      redirect: "follow",
      signal: controller.signal,
      ...(method === "GET" ? { headers: { Range: "bytes=0-0" } } : {}),
    });
    if (response.body) await response.body.cancel();
    return response;
  } finally {
    clearTimeout(timer);
  }
}

export async function checkRuntimeDownloadUrls(manifest, { fetchRequest = request, timeoutMs = 10_000, allowInsecureLocalhost = false } = {}) {
  for (const [label, value] of requiredDownloads(manifest)) {
    const url = validateUrl(label, value, allowInsecureLocalhost);
    try {
      let response = await fetchRequest(url, "HEAD", timeoutMs);
      if (response.status === 405 || response.status === 501) response = await fetchRequest(url, "GET", timeoutMs);
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
    } catch (error) {
      const reason = error?.name === "AbortError" ? `timed out after ${timeoutMs} ms` : error.message;
      throw new Error(`${label} download check failed (${reason}): ${url}`, { cause: error });
    }
  }
  return true;
}

export async function checkRuntimeDownloads(manifestPath, options) {
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  return checkRuntimeDownloadUrls(manifest, options);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  try {
    await checkRuntimeDownloads(new URL("./runtime-manifest.json", import.meta.url));
    console.log("Runtime download URLs are reachable.");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
