import { describe, expect, it } from "vitest";
import type { CueSummary, CueType } from "../../lib/types";
import {
  assessFileSize,
  assessResolution,
  formatFileSize,
  formatResolution,
} from "./mediaInfo";

const MIB = 1024 * 1024;
const GIB = 1024 * MIB;

function cue(
  cueType: CueType,
  media: {
    fileSize?: number | null;
    width?: number | null;
    height?: number | null;
    durationMs?: number | null;
    filePath?: string | null;
    broken?: boolean;
    missing?: boolean;
  } = {},
): CueSummary {
  return ({
    id: "00000000-0000-0000-0000-000000000001",
    cue_type: cueType,
    name: "test",
    number: null,
    notes: "",
    state: "stopped",
    continue_mode: "do_not_continue",
    color: "none",
    pre_wait_ms: 0,
    post_wait_ms: 0,
    duration_ms: media.durationMs ?? null,
    file_duration_ms: media.durationMs ?? null,
    file_path: media.filePath === undefined ? "C:/show/media.bin" : media.filePath,
    file_size_bytes: media.fileSize,
    media_width: media.width,
    media_height: media.height,
    media_file_missing: media.missing ?? false,
    is_loading: false,
    is_disabled: false,
    is_broken: media.broken ?? false,
    is_warning: false,
  } as unknown) as CueSummary;
}

describe("media info formatting", () => {
  it("formats binary file sizes with locale-aware punctuation", () => {
    expect(formatFileSize(0, "en-US")).toBe("0 B");
    expect(formatFileSize(1536, "en-US")).toBe("1.5 KiB");
    expect(formatFileSize(1572864, "ru-RU")).toBe("1,5 MiB");
  });

  it("shows unknown or unsafe file sizes as a dash", () => {
    expect(formatFileSize(null)).toBe("—");
    expect(formatFileSize(-1)).toBe("—");
    expect(formatFileSize(Number.MAX_SAFE_INTEGER + 1)).toBe("—");
  });

  it("formats only complete positive resolutions", () => {
    expect(formatResolution(1920, 1080)).toBe("1920×1080");
    expect(formatResolution(1920, null)).toBe("—");
    expect(formatResolution(0, 1080)).toBe("—");
  });
});

describe("file-size assessment", () => {
  it.each([
    ["image", 25 * MIB, "large_image"],
    ["audio", GIB, "large_audio"],
    ["video", 8 * GIB, "large_video"],
    ["midi_file", 10 * MIB, "large_midi_file"],
  ] as const)("flags %s at its exact threshold", (type, threshold, code) => {
    expect(assessFileSize(cue(type, { fileSize: threshold - 1 })).problem).toBe(false);
    const result = assessFileSize(cue(type, { fileSize: threshold }));
    expect(result.problem).toBe(true);
    expect(result.issues.map((issue) => issue.code)).toContain(code);
  });

  it("flags zero-byte media", () => {
    expect(assessFileSize(cue("audio", { fileSize: 0 })).issues[0]).toEqual({
      code: "empty_file",
      vars: { bytes: 0 },
    });
  });

  it("flags a missing assigned file even when size metadata is unknown", () => {
    const result = assessFileSize(cue("video", {
      fileSize: null,
      filePath: "D:/show/missing.mov",
      missing: true,
    }));
    expect(result.problem).toBe(true);
    expect(result.issues[0]).toEqual({
      code: "missing_file",
      vars: { path: "D:/show/missing.mov" },
    });
  });

  it("does not treat an unrelated runtime error as a missing file", () => {
    expect(assessFileSize(cue("video", {
      fileSize: null,
      filePath: "D:/show/valid.mov",
      broken: true,
    })).problem).toBe(false);
  });

  it("flags average video bitrate at 100 Mbps without multiplication overflow", () => {
    const durationMs = 1_000;
    const atThreshold = 12_500_000;
    expect(assessFileSize(cue("video", {
      fileSize: atThreshold - 1,
      durationMs,
    })).problem).toBe(false);
    expect(assessFileSize(cue("video", {
      fileSize: atThreshold,
      durationMs,
    })).issues.map((issue) => issue.code)).toContain("high_video_bitrate");
  });

  it("flags average audio bitrate at 10 Mbps", () => {
    const result = assessFileSize(cue("audio", {
      fileSize: 1_250_000,
      durationMs: 1_000,
    }));
    expect(result.issues.map((issue) => issue.code)).toContain("high_audio_bitrate");
  });

  it("ignores unknown metadata and non-media cues", () => {
    expect(assessFileSize(cue("audio", { fileSize: null })).problem).toBe(false);
    expect(assessFileSize(cue("wait", { fileSize: 20 * GIB })).problem).toBe(false);
  });
});

describe("resolution assessment", () => {
  it("keeps 1080p video normal", () => {
    expect(assessResolution(cue("video", { width: 1920, height: 1080 })).problem).toBe(false);
  });

  it("flags 4K video at the exact pixel boundary", () => {
    const result = assessResolution(cue("video", { width: 3840, height: 2160 }));
    expect(result.problem).toBe(true);
    expect(result.issues[0].code).toBe("four_k_video");
  });

  it("flags portrait 4K by pixel count", () => {
    expect(assessResolution(cue("video", { width: 2160, height: 3840 })).problem).toBe(true);
  });

  it("checks image pixel count immediately below and at 16 MP", () => {
    expect(assessResolution(cue("image", { width: 3999, height: 4000 })).problem).toBe(false);
    expect(assessResolution(cue("image", { width: 4000, height: 4000 })).problem).toBe(true);
  });

  it("flags a huge image side even below 16 MP", () => {
    expect(assessResolution(cue("image", { width: 8193, height: 1000 })).problem).toBe(true);
  });

  it("does not flag the exact 8192 side by itself", () => {
    expect(assessResolution(cue("image", { width: 8192, height: 1000 })).problem).toBe(false);
  });

  it("ignores unknown dimensions and non-visual cues", () => {
    expect(assessResolution(cue("image", { width: null, height: null })).problem).toBe(false);
    expect(assessResolution(cue("audio", { width: 10000, height: 10000 })).problem).toBe(false);
  });

  it("handles enormous safe dimensions without overflow in threshold comparison", () => {
    expect(assessResolution(cue("video", {
      width: Number.MAX_SAFE_INTEGER,
      height: 1,
    })).problem).toBe(true);
  });
});
