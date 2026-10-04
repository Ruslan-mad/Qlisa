import { describe, expect, it } from "vitest";
import { formatCueDuration, formatDurationMs } from "./formatDuration";

describe("formatDurationMs", () => {
  it("formats whole minutes and seconds", () => {
    expect(formatDurationMs(157_000)).toBe("2:37");
    expect(formatDurationMs(61_000)).toBe("1:01");
    expect(formatDurationMs(0)).toBe("0:00");
  });

  it("keeps total minutes for durations longer than an hour", () => {
    expect(formatDurationMs(3_661_000)).toBe("61:01");
  });

  it("floors fractional seconds and clamps negative values to zero", () => {
    expect(formatDurationMs(1_999)).toBe("0:01");
    expect(formatDurationMs(-1_000)).toBe("0:00");
  });

  it("uses a dash when duration is unknown or invalid", () => {
    expect(formatDurationMs(null)).toBe("—");
    expect(formatDurationMs(undefined)).toBe("—");
    expect(formatDurationMs(Number.NaN)).toBe("—");
  });
});

describe("formatCueDuration", () => {
  it("shows infinity for infinite Audio and Video loops", () => {
    expect(formatCueDuration({ cue_type: "audio", loop_count: 0xffff_ffff, duration_ms: null })).toBe("∞");
    expect(formatCueDuration({ cue_type: "video", loop_count: 0xffff_ffff, duration_ms: null })).toBe("∞");
  });

  it("keeps finite loop durations and non-media indefinite durations unchanged", () => {
    expect(formatCueDuration({ cue_type: "audio", loop_count: 2, duration_ms: 12_000 })).toBe("0:12");
    expect(formatCueDuration({ cue_type: "image", loop_count: 0xffff_ffff, duration_ms: null })).toBe("—");
  });
});
