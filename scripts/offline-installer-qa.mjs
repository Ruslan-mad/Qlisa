import { createHash, createPublicKey, verify } from "node:crypto";
import { readFile } from "node:fs/promises";

const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

function requireBase64Line(line, expectedBytes, label) {
  if (!line || !/^[A-Za-z0-9+/]+={0,2}$/.test(line)) throw new Error(`${label} is not valid base64`);
  const value = Buffer.from(line, "base64");
  if (value.length !== expectedBytes) throw new Error(`${label} has an unexpected binary size`);
  return value;
}

function outerBase64Text(value, label) {
  if (typeof value !== "string" || !/^[A-Za-z0-9+/]+={0,2}$/.test(value.trim())) throw new Error(`${label} is not valid base64`);
  const text = Buffer.from(value.trim(), "base64").toString("utf8");
  if (text.includes("\uFFFD")) throw new Error(`${label} does not decode as UTF-8`);
  return text.replaceAll("\r\n", "\n").trimEnd();
}

function parsePublicKey(encoded) {
  const lines = outerBase64Text(encoded, "Tauri public key").split("\n").filter(Boolean);
  const keyLine = lines.find((line) => /^[A-Za-z0-9+/]+={0,2}$/.test(line));
  const keyBytes = requireBase64Line(keyLine, 42, "Minisign public key");
  if (keyBytes.toString("ascii", 0, 2) !== "Ed") throw new Error("Unsupported public key algorithm");
  return {
    keyId: keyBytes.subarray(2, 10),
    key: createPublicKey({
      key: Buffer.concat([ED25519_SPKI_PREFIX, keyBytes.subarray(10)]),
      format: "der",
      type: "spki",
    }),
  };
}

function parseSignature(encoded) {
  const lines = outerBase64Text(encoded, "Tauri signature").split("\n").filter(Boolean);
  const signatureLine = lines.find((line) => {
    if (!/^[A-Za-z0-9+/]+={0,2}$/.test(line)) return false;
    try { return Buffer.from(line, "base64").length === 74; } catch { return false; }
  });
  const trustedLine = lines.find((line) => line.startsWith("trusted comment: "));
  const globalLine = lines.at(-1);
  const signatureBytes = requireBase64Line(signatureLine, 74, "Minisign signature");
  const globalSignature = requireBase64Line(globalLine, 64, "Minisign trusted-comment signature");
  if (signatureBytes.toString("ascii", 0, 2) !== "ED" && signatureBytes.toString("ascii", 0, 2) !== "Ed") {
    throw new Error("Unsupported Minisign signature algorithm");
  }
  if (!trustedLine) throw new Error("Minisign trusted comment is missing");
  return {
    algorithm: signatureBytes.toString("ascii", 0, 2),
    keyId: signatureBytes.subarray(2, 10),
    signature: signatureBytes.subarray(10),
    signatureStruct: signatureBytes,
    trustedComment: trustedLine.slice("trusted comment: ".length),
    globalSignature,
  };
}

export async function verifyTauriSignature({ artifactPath, signaturePath, configPath, expectedVersion }) {
  const config = JSON.parse(await readFile(configPath, "utf8"));
  const publicKey = parsePublicKey(config?.plugins?.updater?.pubkey);
  const signature = parseSignature(await readFile(signaturePath, "utf8"));
  if (!signature.keyId.equals(publicKey.keyId)) throw new Error("Signature key ID does not match the app public key");
  const artifact = await readFile(artifactPath);
  const signedBytes = signature.algorithm === "ED"
    ? createHash("blake2b512").update(artifact).digest()
    : artifact;
  if (!verify(null, signedBytes, publicKey.key, signature.signature)) throw new Error("Installer signature is invalid");
  const trustedComment = Buffer.from(signature.trustedComment, "utf8");
  if (!verify(null, Buffer.concat([signature.signature, trustedComment]), publicKey.key, signature.globalSignature)) {
    throw new Error("Trusted-comment signature is invalid");
  }
  if (expectedVersion) {
    const version = signature.trustedComment.match(/(?:^|\s)app-version:([^\s]+)/)?.[1];
    if (version && version.replace(/^v/, "") !== expectedVersion.replace(/^v/, "")) {
      throw new Error(`Signed app version ${version} does not match ${expectedVersion}`);
    }
  }
  return { verified: true, algorithm: signature.algorithm, file: artifactPath };
}

function parseArgs(argv) {
  const [command, ...rest] = argv;
  const options = {};
  for (let i = 0; i < rest.length; i += 1) {
    if (!rest[i].startsWith("--") || i + 1 >= rest.length) throw new Error(`Invalid argument: ${rest[i]}`);
    options[rest[i].slice(2)] = rest[++i];
  }
  return { command, options };
}

function cdpClient(endpoint, timeoutMs) {
  return new Promise((resolveClient, rejectClient) => {
    const socket = new WebSocket(endpoint);
    const pending = new Map();
    let nextId = 1;
    const timer = setTimeout(() => {
      socket.close();
      rejectClient(new Error("CDP WebSocket connection timed out"));
    }, timeoutMs);
    socket.addEventListener("open", () => {
      clearTimeout(timer);
      resolveClient({
        async send(method, params = {}) {
          const id = nextId++;
          return new Promise((resolve, reject) => {
            const responseTimer = setTimeout(() => {
              pending.delete(id);
              reject(new Error(`CDP request timed out: ${method}`));
            }, timeoutMs);
            pending.set(id, { resolve, reject, responseTimer });
            socket.send(JSON.stringify({ id, method, params }));
          });
        },
        close() { socket.close(); },
      });
    });
    socket.addEventListener("message", (event) => {
      try {
        const message = JSON.parse(typeof event.data === "string" ? event.data : String(event.data));
        const request = pending.get(message.id);
        if (!request) return;
        clearTimeout(request.responseTimer);
        pending.delete(message.id);
        if (message.error) request.reject(new Error(message.error.message));
        else request.resolve(message.result);
      } catch { /* Ignore unrelated CDP events. */ }
    });
    socket.addEventListener("error", () => {
      clearTimeout(timer);
      rejectClient(new Error("CDP WebSocket connection failed"));
    });
    socket.addEventListener("close", () => {
      clearTimeout(timer);
      for (const request of pending.values()) {
        clearTimeout(request.responseTimer);
        request.reject(new Error("CDP target closed"));
      }
      pending.clear();
    });
  });
}

async function discoverTarget(port, timeoutMs) {
  const response = await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(timeoutMs) });
  if (!response.ok) throw new Error(`CDP target list returned HTTP ${response.status}`);
  const targets = await response.json();
  const pages = targets.filter((target) => target.type === "page" && target.webSocketDebuggerUrl);
  let target;
  for (const page of pages) {
    let client;
    try {
      client = await cdpClient(page.webSocketDebuggerUrl, Math.min(timeoutMs, 2_000));
      await client.send("Runtime.enable");
      const response = await client.send("Runtime.evaluate", {
        expression: "window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? null",
        returnByValue: true,
      });
      if (response.result?.value === "main") {
        target = page;
        break;
      }
    } catch {
      // Hidden browser targets may not expose Tauri metadata.
    } finally {
      client?.close();
    }
  }
  if (!target) throw new Error("No WebView2 page target reports Tauri window label 'main'");
  return target;
}

async function invoke(client, command) {
  const expression = `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)})`;
  const response = await client.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (response.exceptionDetails) throw new Error(`Tauri command failed: ${command}`);
  return response.result?.value;
}

async function probe(options) {
  const port = Number(options.port ?? 9222);
  const timeoutMs = Number(options["timeout-ms"] ?? 90_000);
  const deadline = Date.now() + timeoutMs;
  let lastError = "WebView2 DevTools endpoint is not ready";
  let screenshotWritten = false;
  while (Date.now() < deadline) {
    let client;
    try {
      const target = await discoverTarget(port, 2_000);
      client = await cdpClient(target.webSocketDebuggerUrl, 5_000);
      await client.send("Runtime.enable");
      const backendReady = await invoke(client, "is_backend_ready");
      const media = await invoke(client, "get_media_runtime_status");
      const network = await invoke(client, "get_network_io_status");
      const mainUiReady = await client.send("Runtime.evaluate", {
        expression: "Boolean(document.querySelector('.stage-transport-go'))",
        returnByValue: true,
      }).then((response) => response.result?.value === true);
      const result = { backendReady, media, network, mainUiReady };
      if (backendReady === true && media?.ready === true && media.ffmpeg && media.ffprobe && media.libmpv && network?.ndi?.availability === "ready" && mainUiReady) {
        if (options.screenshot) {
          try {
            await client.send("Page.enable");
            const capture = await client.send("Page.captureScreenshot", { format: "png", fromSurface: true, captureBeyondViewport: false });
            await import("node:fs/promises").then(({ writeFile }) => writeFile(options.screenshot, Buffer.from(capture.data, "base64")));
            screenshotWritten = true;
          } catch { /* Screenshot is optional; status checks remain authoritative. */ }
        }
        process.stdout.write(`${JSON.stringify({ ok: true, ...result, screenshotWritten })}\n`);
        return;
      }
      lastError = JSON.stringify(result);
    } catch (error) {
      const causes = [];
      let cause = error;
      while (cause && causes.length < 4) {
        const details = [cause.code, cause.message].filter(Boolean).join(" ");
        if (details) causes.push(details);
        cause = cause.cause;
      }
      lastError = causes.join(" <- ") || String(error);
    } finally {
      client?.close();
    }
    await new Promise((resolve) => setTimeout(resolve, 1_000));
  }
  throw new Error(`App did not become ready within ${timeoutMs} ms: ${lastError}`);
}

async function main() {
  const { command, options } = parseArgs(process.argv.slice(2));
  if (command === "verify-signature") {
    const result = await verifyTauriSignature({
      artifactPath: options.artifact,
      signaturePath: options.signature,
      configPath: options.config,
      expectedVersion: options.version,
    });
    process.stdout.write(`${JSON.stringify(result)}\n`);
    return;
  }
  if (command === "probe") return probe(options);
  throw new Error("Usage: offline-installer-qa.mjs verify-signature|probe --name value ...");
}

main().catch((error) => {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
});
