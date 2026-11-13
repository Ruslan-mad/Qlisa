import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

function requiredDownloads(manifest) {
  if (manifest?.schemaVersion !== 1) throw new Error("Runtime manifest: expected schemaVersion 1");
  if (typeof manifest.recommendedKitId !== "string" || !manifest.recommendedKitId.trim()) {
    throw new Error("Runtime manifest: recommendedKitId is required");
  }
  if (!Array.isArray(manifest.approvedKits) || manifest.approvedKits.length === 0) {
    throw new Error("Runtime manifest: approvedKits must be a non-empty array");
  }
  const ids = new Set();
  let recommendedFound = false;
  const downloads = [];
  for (const kit of manifest.approvedKits) {
    if (!kit || typeof kit.id !== "string" || !kit.id.trim() || ids.has(kit.id)) {
      throw new Error("Runtime manifest: approved kit IDs must be unique non-empty strings");
    }
    ids.add(kit.id);
    if (kit.id === manifest.recommendedKitId) recommendedFound = true;
    if (kit.architecture !== "windows-x86_64") throw new Error(`Runtime manifest: unsupported architecture for kit ${kit.id}`);
    if (!Array.isArray(kit.components)) throw new Error(`Runtime manifest: components must be an array for kit ${kit.id}`);
    for (const id of ["ffmpeg", "libmpv"]) {
      const components = kit.components.filter((component) => component?.id === id);
      if (components.length !== 1) throw new Error(`Runtime manifest: expected exactly one ${id} component in kit ${kit.id}, found ${components.length}`);
      const component = components[0];
      const asset = component.asset;
      if (!asset || typeof asset.fileName !== "string" || !asset.fileName.trim() ||
          typeof asset.sha256 !== "string" || !/^[a-f\d]{64}$/i.test(asset.sha256) ||
          asset.format !== "zip" || !Array.isArray(component.runtimeFiles) || component.runtimeFiles.length === 0) {
        throw new Error(`Runtime manifest: invalid ${id} asset or runtime-file metadata in kit ${kit.id}`);
      }
      const fileNames = new Set();
      for (const file of component.runtimeFiles) {
        if (!file || typeof file.fileName !== "string" || !file.fileName.trim() || fileNames.has(file.fileName) ||
            typeof file.sha256 !== "string" || !/^[a-f\d]{64}$/i.test(file.sha256)) {
          throw new Error(`Runtime manifest: invalid ${id} runtime-file hash metadata in kit ${kit.id}`);
        }
        fileNames.add(file.fileName);
      }
      downloads.push([`${kit.id} ${id === "ffmpeg" ? "FFmpeg" : "libmpv"}`, asset.url, asset.fileName]);
    }
  }
  if (!recommendedFound) throw new Error(`Runtime manifest: recommended kit ${manifest.recommendedKitId} is not approved`);
  return downloads;
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
  for (const [label, value, fileName] of requiredDownloads(manifest)) {
    const url = validateUrl(label, value, allowInsecureLocalhost);
    const urlFileName = decodeURIComponent(new URL(url).pathname.split("/").pop() ?? "");
    if (urlFileName !== fileName) throw new Error(`${label}: asset URL filename '${urlFileName}' does not match manifest filename '${fileName}'`);
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
