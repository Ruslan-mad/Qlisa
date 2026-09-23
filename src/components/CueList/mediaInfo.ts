import type { CueSummary, CueType } from "../../lib/types";

const KIB = 1024;
const MIB = KIB * 1024;
const GIB = MIB * 1024;

const FILE_SIZE_LIMITS: Partial<Record<CueType, number>> = {
  image: 25 * MIB,
  audio: GIB,
  video: 8 * GIB,
  midi_file: 10 * MIB,
};

const VIDEO_BITRATE_LIMIT_MBPS = 100;
const AUDIO_BITRATE_LIMIT_MBPS = 10;
const VIDEO_4K_PIXELS = 3840 * 2160;
const IMAGE_LARGE_PIXELS = 16_000_000;
const IMAGE_MAX_SIDE = 8192;

const MEDIA_CUE_TYPES = new Set<CueType>(["audio", "video", "image", "midi_file"]);

type CueWithMediaInfo = CueSummary & {
  file_size_bytes?: number | null;
  media_width?: number | null;
  media_height?: number | null;
  media_file_missing?: boolean;
};

export type MediaInfoIssueCode =
  | "missing_file"
  | "empty_file"
  | "large_image"
  | "large_audio"
  | "large_video"
  | "large_midi_file"
  | "high_video_bitrate"
  | "high_audio_bitrate"
  | "four_k_video"
  | "huge_image";

export interface MediaInfoIssue {
  code: MediaInfoIssueCode;
  vars: Record<string, string | number>;
}

export interface MediaInfoAssessment {
  problem: boolean;
  issues: MediaInfoIssue[];
}

const OK: MediaInfoAssessment = Object.freeze({ problem: false, issues: [] });

function assessment(issues: MediaInfoIssue[]): MediaInfoAssessment {
  return issues.length === 0 ? OK : { problem: true, issues };
}

function validNonNegativeInteger(value: number | null | undefined): value is number {
  return typeof value === "number"
    && Number.isSafeInteger(value)
    && value >= 0;
}

function validDimension(value: number | null | undefined): value is number {
  return validNonNegativeInteger(value) && value > 0;
}

/** Format a byte count using binary units and locale-aware decimal punctuation. */
export function formatFileSize(
  bytes: number | null | undefined,
  locale?: string,
): string {
  if (!validNonNegativeInteger(bytes)) return "—";

  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"] as const;
  let value = bytes;
  let unitIndex = 0;
  while (value >= KIB && unitIndex < units.length - 1) {
    value /= KIB;
    unitIndex += 1;
  }

  const maximumFractionDigits = unitIndex === 0 ? 0 : value < 10 ? 2 : value < 100 ? 1 : 0;
  const formatted = new Intl.NumberFormat(locale, {
    minimumFractionDigits: 0,
    maximumFractionDigits,
  }).format(value);
  return `${formatted} ${units[unitIndex]}`;
}

/** Format known positive pixel dimensions. Partial or invalid metadata is unknown. */
export function formatResolution(
  width: number | null | undefined,
  height: number | null | undefined,
): string {
  if (!validDimension(width) || !validDimension(height)) return "—";
  return `${width}×${height}`;
}

/**
 * Compare a product with a threshold without calculating the product itself.
 * This keeps corrupt or unexpectedly huge metadata from overflowing the safe
 * integer range merely while it is being classified.
 */
function productAtLeast(a: number, b: number, threshold: number): boolean {
  return a >= Math.ceil(threshold / b);
}

function pixelCountForVars(width: number, height: number): number | string {
  const pixels = BigInt(width) * BigInt(height);
  return pixels <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(pixels) : pixels.toString();
}

function averageBitrateMbps(bytes: number, durationMs: number | null | undefined): number | null {
  if (!validNonNegativeInteger(bytes) || !validDimension(durationMs)) return null;
  // bytes * 8 / seconds / 1_000_000, rearranged to avoid a large multiplication.
  return (bytes / durationMs) * 0.008;
}

/** Assess only the Size column. Unknown metadata and non-media cues are healthy. */
export function assessFileSize(cue: CueSummary): MediaInfoAssessment {
  if (!MEDIA_CUE_TYPES.has(cue.cue_type)) return OK;

  const mediaCue = cue as CueWithMediaInfo;
  const issues: MediaInfoIssue[] = [];
  const assignedPath = typeof cue.file_path === "string" && cue.file_path.trim().length > 0;

  if (assignedPath && mediaCue.media_file_missing === true) {
    issues.push({ code: "missing_file", vars: { path: cue.file_path! } });
  }

  const bytes = mediaCue.file_size_bytes;
  if (!validNonNegativeInteger(bytes)) return assessment(issues);

  if (bytes === 0) {
    issues.push({ code: "empty_file", vars: { bytes: 0 } });
    return assessment(issues);
  }

  const sizeLimit = FILE_SIZE_LIMITS[cue.cue_type];
  if (sizeLimit !== undefined && bytes >= sizeLimit) {
    const code = `large_${cue.cue_type}` as Extract<
      MediaInfoIssueCode,
      "large_image" | "large_audio" | "large_video" | "large_midi_file"
    >;
    issues.push({ code, vars: { bytes, thresholdBytes: sizeLimit } });
  }

  const durationMs = cue.file_duration_ms;
  const bitrateMbps = averageBitrateMbps(bytes, durationMs ?? null);
  const bitrateLimit = cue.cue_type === "video"
    ? VIDEO_BITRATE_LIMIT_MBPS
    : cue.cue_type === "audio"
      ? AUDIO_BITRATE_LIMIT_MBPS
      : null;
  if (bitrateMbps !== null && bitrateLimit !== null && bitrateMbps >= bitrateLimit) {
    issues.push({
      code: cue.cue_type === "video" ? "high_video_bitrate" : "high_audio_bitrate",
      vars: { bitrateMbps, thresholdMbps: bitrateLimit },
    });
  }

  return assessment(issues);
}

/** Assess only the Resolution column. Audio/MIDI and unknown dimensions are healthy. */
export function assessResolution(cue: CueSummary): MediaInfoAssessment {
  if (cue.cue_type !== "video" && cue.cue_type !== "image") return OK;

  const mediaCue = cue as CueWithMediaInfo;
  const width = mediaCue.media_width;
  const height = mediaCue.media_height;
  if (!validDimension(width) || !validDimension(height)) return OK;

  if (cue.cue_type === "video" && productAtLeast(width, height, VIDEO_4K_PIXELS)) {
    return assessment([{
      code: "four_k_video",
      vars: { width, height, pixels: pixelCountForVars(width, height), thresholdPixels: VIDEO_4K_PIXELS },
    }]);
  }

  if (
    cue.cue_type === "image"
    && (productAtLeast(width, height, IMAGE_LARGE_PIXELS) || Math.max(width, height) > IMAGE_MAX_SIDE)
  ) {
    return assessment([{
      code: "huge_image",
      vars: { width, height, pixels: pixelCountForVars(width, height), thresholdPixels: IMAGE_LARGE_PIXELS },
    }]);
  }

  return OK;
}
