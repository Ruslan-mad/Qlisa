const SEMVER_PATTERN = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

export function parseVersion(version) {
  const match = SEMVER_PATTERN.exec(version);
  if (!match) {
    throw new Error(`Version must be SemVer X.Y.Z without prerelease or build metadata: ${version}`);
  }

  return match.slice(1).map((part) => BigInt(part));
}

export function compareVersions(left, right) {
  const leftParts = parseVersion(left);
  const rightParts = parseVersion(right);

  for (let index = 0; index < leftParts.length; index += 1) {
    if (leftParts[index] < rightParts[index]) return -1;
    if (leftParts[index] > rightParts[index]) return 1;
  }

  return 0;
}

export function parseReleaseArgs(args) {
  if (args[0] === "--") args = args.slice(1);
  let version;
  let dryRun = false;
  let bundle;

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];

    if (arg === "--dry-run") {
      if (dryRun) throw new Error("--dry-run may only be specified once");
      dryRun = true;
      continue;
    }

    if (arg === "--bundle" || arg.startsWith("--bundle=")) {
      if (bundle !== undefined) throw new Error("--bundle may only be specified once");
      const value = arg === "--bundle" ? args[++index] : arg.slice("--bundle=".length);
      if (value !== "nsis" && value !== "msi") {
        throw new Error("--bundle must be exactly one of: nsis, msi");
      }
      bundle = value;
      continue;
    }

    if (arg.startsWith("-")) throw new Error(`Unknown option: ${arg}`);
    if (version !== undefined) throw new Error("Provide exactly one target version");
    version = arg;
  }

  if (version === undefined) throw new Error("Usage: pnpm release -- X.Y.Z [--dry-run] [--bundle nsis|msi]");
  parseVersion(version);

  return { version, dryRun, bundle };
}

export function buildCommandArgs(releaseOptions) {
  const args = ["exec", "tauri", "build"];
  if (releaseOptions.bundle) args.push("--bundles", releaseOptions.bundle);
  else args.push("--no-bundle");
  args.push("--", "--features", "asio-support");
  return args;
}

export function createReleaseManifest({ version, commit, executable, installer, builtAt, bundle }) {
  return {
    version,
    commit,
    sha256: executable.sha256,
    builtAt,
    bundledTarget: bundle ?? null,
    executable,
    installer: installer ?? null,
  };
}

export function decideReleaseRollback(initialHead, currentHead) {
  if (typeof initialHead !== "string" || initialHead.length === 0) return "unknown";
  if (typeof currentHead !== "string" || currentHead.length === 0) return "unknown";
  return currentHead === initialHead ? "rollback" : "preserve";
}

export function artifactChanged(before, after) {
  if (!after) return false;
  if (!before) return true;
  return before.size !== after.size || before.mtimeMs !== after.mtimeMs;
}

function skipWhitespace(contents, index) {
  while (/\s/.test(contents[index] ?? "")) index += 1;
  return index;
}

function scanJsonString(contents, start) {
  if (contents[start] !== '"') throw new Error("Expected a JSON string");
  let escaped = false;
  for (let index = start + 1; index < contents.length; index += 1) {
    const character = contents[index];
    if (escaped) escaped = false;
    else if (character === "\\") escaped = true;
    else if (character === '"') return index + 1;
  }
  throw new Error("Unterminated JSON string");
}

function skipJsonValue(contents, start) {
  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let index = start; index < contents.length; index += 1) {
    const character = contents[index];
    if (inString) {
      if (escaped) escaped = false;
      else if (character === "\\") escaped = true;
      else if (character === '"') inString = false;
      continue;
    }
    if (character === '"') inString = true;
    else if (character === "{" || character === "[") depth += 1;
    else if (character === "}" || character === "]") {
      if (depth === 0) return index;
      depth -= 1;
    } else if (character === "," && depth === 0) return index;
  }
  return contents.length;
}

function findJsonTopLevelStringField(contents, fieldName) {
  let index = skipWhitespace(contents, 0);
  if (contents[index] !== "{") throw new Error("Expected a JSON object");
  index += 1;
  const fields = [];

  while (index < contents.length) {
    index = skipWhitespace(contents, index);
    if (contents[index] === "}" || index >= contents.length) break;
    if (contents[index] === ",") {
      index += 1;
      continue;
    }

    const keyEnd = scanJsonString(contents, index);
    const key = JSON.parse(contents.slice(index, keyEnd));
    index = skipWhitespace(contents, keyEnd);
    if (contents[index] !== ":") throw new Error("Expected a colon after a JSON object key");
    index = skipWhitespace(contents, index + 1);
    const valueStart = index;

    if (key === fieldName && contents[valueStart] === '"') {
      const valueEnd = scanJsonString(contents, valueStart);
      fields.push({ valueStart, valueEnd, value: JSON.parse(contents.slice(valueStart, valueEnd)) });
      index = valueEnd;
    } else {
      index = skipJsonValue(contents, valueStart);
    }
  }
  return fields;
}

function replaceExactlyOne(text, expression, description, replacement) {
  const matches = [...text.matchAll(expression)];
  if (matches.length !== 1) {
    throw new Error(`Expected exactly one ${description}; found ${matches.length}`);
  }
  return text.replace(expression, replacement);
}

export function replaceJsonVersion(contents, currentVersion, nextVersion) {
  const document = JSON.parse(contents);
  if (document.version !== currentVersion) {
    throw new Error(`JSON version is ${document.version}, expected ${currentVersion}`);
  }
  const fields = findJsonTopLevelStringField(contents, "version");
  if (fields.length !== 1) throw new Error(`Expected exactly one top-level JSON version field; found ${fields.length}`);
  const [{ valueStart, valueEnd, value }] = fields;
  if (value !== currentVersion) throw new Error(`JSON version field is ${value}, expected ${currentVersion}`);
  return contents.slice(0, valueStart) + JSON.stringify(nextVersion) + contents.slice(valueEnd);
}

function sectionBounds(contents, headerPattern, description) {
  const headers = [...contents.matchAll(headerPattern)];
  const matching = headers.filter((match) => match[0].trim() === description);
  if (matching.length !== 1) {
    throw new Error(`Expected exactly one ${description} section; found ${matching.length}`);
  }

  const start = matching[0].index;
  const next = headers.find((match) => match.index > start);
  return { start, end: next ? next.index : contents.length };
}

function replaceCargoPackageVersion(contents, currentVersion, nextVersion, sectionHeader, versionLabel) {
  const bounds = sectionBounds(contents, sectionHeader, versionLabel);
  const section = contents.slice(bounds.start, bounds.end);
  const expression = /^([ \t]*version[ \t]*=[ \t]*)"([^"\r\n]+)"([ \t]*(?:#.*)?)$/gm;
  const updatedSection = replaceExactlyOne(
    section,
    expression,
    `${versionLabel} version field`,
    (_match, prefix, foundVersion, suffix) => {
      if (foundVersion !== currentVersion) {
        throw new Error(`${versionLabel} version is ${foundVersion}, expected ${currentVersion}`);
      }
      return `${prefix}"${nextVersion}"${suffix}`;
    },
  );
  return contents.slice(0, bounds.start) + updatedSection + contents.slice(bounds.end);
}

function readCargoPackageVersion(contents, sectionHeader, versionLabel) {
  const bounds = sectionBounds(contents, sectionHeader, versionLabel);
  const section = contents.slice(bounds.start, bounds.end);
  const matches = [...section.matchAll(/^[ \t]*version[ \t]*=[ \t]*"([^"\r\n]+)"[ \t]*(?:#.*)?$/gm)];
  if (matches.length !== 1) {
    throw new Error(`Expected exactly one ${versionLabel} version field; found ${matches.length}`);
  }
  return matches[0][1];
}

export function readCargoTomlVersion(contents) {
  return readCargoPackageVersion(contents, /^\[package\][ \t]*$/gm, "[package]");
}

export function replaceCargoTomlVersion(contents, currentVersion, nextVersion) {
  return replaceCargoPackageVersion(contents, currentVersion, nextVersion, /^\[package\][ \t]*$/gm, "[package]");
}

export function readCargoLockVersion(contents) {
  const headers = [...contents.matchAll(/^\[\[package\]\][ \t]*$/gm)];
  const packageSections = headers.map((header, index) => {
    const start = header.index;
    const next = headers[index + 1];
    const end = next ? next.index : contents.length;
    return { text: contents.slice(start, end) };
  }).filter(({ text }) => /^name = "qlisa"[ \t]*$/m.test(text));

  if (packageSections.length !== 1) {
    throw new Error(`Expected exactly one qlisa package in Cargo.lock; found ${packageSections.length}`);
  }
  const matches = [...packageSections[0].text.matchAll(/^[ \t]*version[ \t]*=[ \t]*"([^"\r\n]+)"[ \t]*$/gm)];
  if (matches.length !== 1) throw new Error("Expected exactly one qlisa version field in Cargo.lock");
  return matches[0][1];
}

export function replaceCargoLockVersion(contents, currentVersion, nextVersion) {
  const headers = [...contents.matchAll(/^\[\[package\]\][ \t]*$/gm)];
  const packageSections = headers.map((header, index) => {
    const start = header.index;
    const next = headers[index + 1];
    return { start, end: next ? next.index : contents.length, text: contents.slice(start, next ? next.index : contents.length) };
  }).filter(({ text }) => /^name = "qlisa"[ \t]*$/m.test(text));

  if (packageSections.length !== 1) {
    throw new Error(`Expected exactly one qlisa package in Cargo.lock; found ${packageSections.length}`);
  }

  const { start, end } = packageSections[0];
  const section = contents.slice(start, end);
  const updatedSection = replaceExactlyOne(
    section,
    /^([ \t]*version[ \t]*=[ \t]*)"([^"\r\n]+)"([ \t]*)$/gm,
    "qlisa Cargo.lock version field",
    (_match, prefix, foundVersion, suffix) => {
      if (foundVersion !== currentVersion) {
        throw new Error(`Cargo.lock qlisa version is ${foundVersion}, expected ${currentVersion}`);
      }
      return `${prefix}"${nextVersion}"${suffix}`;
    },
  );

  return contents.slice(0, start) + updatedSection + contents.slice(end);
}
