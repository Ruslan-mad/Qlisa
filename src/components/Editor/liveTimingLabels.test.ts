import { describe, expect, it } from "vitest";
import type { CueTiming } from "../../stores/timingStore";
import { liveTimingLabels } from "./liveTimingLabels";

const timing = (remaining_ms: number): CueTiming => ({
  elapsed_ms: 0,
  action_elapsed_ms: 0,
  remaining_ms,
  media_position_ms: null,
});

const cue = (overrides: Record<string, unknown> = {}) => ({
  cue_type: "audio" as const,
  loop_count: 1,
  duration_ms: 65_000,
  ...overrides,
}) as any;

describe("liveTimingLabels", () => {
  it("formats a finite duration and backend remaining above one minute", () => {
    expect(liveTimingLabels(cue(), 65_000, timing(61_000))).toEqual({
      duration: "1:05",
      remaining: "1:01",
    });
  });

  it("uses the cue action duration after trims, loops, and rate instead of file duration", () => {
    expect(liveTimingLabels(cue({ duration_ms: 80_000 }), 180_000, timing(20_000))).toEqual({
      duration: "1:20",
      remaining: "0:20",
    });
  });

  it("does not guess duration from media metadata when the cue duration is unknown", () => {
    expect(liveTimingLabels(cue({ duration_ms: null }), 180_000, undefined)).toEqual({
      duration: "—",
      remaining: "—",
    });
  });

  it("keeps the backend remaining value while paused", () => {
    expect(liveTimingLabels(cue({ state: "paused" }), 65_000, timing(42_000)).remaining).toBe("0:42");
  });

  it("shows zero remaining instead of treating it as missing", () => {
    expect(liveTimingLabels(cue(), 65_000, timing(0)).remaining).toBe("0:00");
  });

  it("uses the backend remaining value during pre-wait", () => {
    expect(liveTimingLabels(cue({ pre_wait_ms: 5_000 }), 65_000, timing(70_000)).remaining).toBe("1:10");
  });

  it("shows infinity for an infinite media loop", () => {
    expect(liveTimingLabels(cue({ loop_count: 0xffff_ffff, duration_ms: null }), 65_000, timing(0))).toEqual({
      duration: "∞",
      remaining: "∞",
    });
  });

  it("shows an em dash when runtime timing is not available", () => {
    expect(liveTimingLabels(cue(), 65_000, undefined).remaining).toBe("—");
  });
});
