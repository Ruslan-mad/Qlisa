import { describe, expect, it } from "vitest";
import type { CueSummary, StatusBarMetricId, StatusBarPreferences } from "../../lib/types";
import { buildCueStatusMetrics, DEFAULT_STATUS_BAR_PREFERENCES, formatBytePair, isStatusSampleStale, moveStatusMetric, normalizeStatusBarPreferences, reorderStatusMetric, updateLoadBand } from "./statusBarModel";

function cue(id: string, cue_type: string, extra: Record<string, unknown> = {}): CueSummary {
  return {
    id, cue_type: cue_type as CueSummary["cue_type"], name: id, number: null, notes: "", state: "standby",
    continue_mode: "do_not_continue", color: "none", pre_wait_ms: 0, post_wait_ms: 0, duration_ms: null,
    file_path: null, file_size_bytes: null, media_width: null, media_height: null, media_file_missing: false,
    is_loading: false, is_disabled: false, is_broken: false, is_warning: false, file_duration_ms: null,
    ...extra,
  } as CueSummary;
}

describe("status bar preferences and cue metrics", () => {
  it("defaults the core metrics on and leaves optional metrics configurable", () => {
    const prefs = normalizeStatusBarPreferences();
    expect(prefs.left.filter((item) => item.enabled).map((item) => item.id)).toEqual(["cue_count", "duration", "active", "problems"]);
    expect(prefs.right.filter((item) => item.enabled).map((item) => item.id)).toEqual(["cpu", "gpu", "vram", "ram"]);
    expect([...prefs.left, ...prefs.right]).toHaveLength(13);
  });

  it("drops duplicate and invalid IDs, appends new metrics disabled, and preserves order moves", () => {
    const partial = normalizeStatusBarPreferences({
      visible: true,
      left: [{ id: "cpu" as StatusBarMetricId, enabled: true }, { id: "cpu" as StatusBarMetricId, enabled: false }, { id: "invalid" as StatusBarMetricId, enabled: true }],
      right: [{ id: "disk" as StatusBarMetricId, enabled: true }],
    });
    expect(partial.left[0]).toEqual({ id: "cpu", enabled: true });
    expect(partial.right[0]).toEqual({ id: "disk", enabled: true });
    expect(partial.left.find((item) => item.id === "cue_count")?.enabled).toBe(false);
    const originalRightOrder = partial.right.map((item) => item.id);
    const moved = moveStatusMetric(partial, "cpu", "right");
    expect(moved.left.some((item) => item.id === "cpu")).toBe(false);
    expect(moved.right.slice(0, -1).map((item) => item.id)).toEqual(originalRightOrder.filter((id) => id !== "cpu"));
    expect(moved.right[moved.right.length - 1]?.id).toBe("cpu");
    const reordered = reorderStatusMetric(moved, "right", "cpu", -1);
    expect(reordered.right[reordered.right.length - 2]?.id).toBe("cpu");
    expect(reordered.right[reordered.right.length - 1]?.id).toBe("app_ram");
    const returned = moveStatusMetric(reordered, "cpu", "left");
    expect(returned.left[returned.left.length - 1]).toEqual({ id: "cpu", enabled: true });
    expect(returned.right.map((item) => item.id)).toEqual(reordered.right.filter((item) => item.id !== "cpu").map((item) => item.id));
  });

  it("counts nested containers once and sums only root durations", () => {
    const nestedGroup = cue("nested", "group", {
      group_mode: "sequential",
      children: [
        cue("a", "audio", { duration_ms: 1000 }),
        cue("b", "audio", { duration_ms: 2000, pre_wait_ms: 500 }),
      ],
    });
    const root = cue("root", "group", { group_mode: "simultaneous", children: [nestedGroup, cue("c", "image", { duration_ms: 5000 })] });
    const result = buildCueStatusMetrics([root]);
    expect(result.total).toBe(5);
    expect(result.groups).toBe(2);
    expect(result.knownDurationMs).toBe(5000);
    expect(result.unknownDurationCount).toBe(0);
  });

  it("excludes disabled duration, retains disabled count, and distinguishes infinite from unknown", () => {
    const result = buildCueStatusMetrics([
      cue("finite", "audio", { duration_ms: 5000, loop_count: 3 }),
      cue("disabled", "video", { duration_ms: 10000, is_disabled: true }),
      cue("loop", "audio", { loop_count: 0xffff_ffff }),
      cue("unknown", "video"),
      cue("memo", "memo"),
    ]);
    expect(result.disabled).toBe(1);
    expect(result.knownDurationMs).toBe(5000);
    expect(result.infiniteCount).toBe(1);
    expect(result.unknownDurationCount).toBe(1);
  });

  it("handles backend-shaped zero, omitted, malformed, live and Number durations", () => {
    const finiteNumber = cue("number-finite", "number", {
      duration_ms: 12000, number_master_id: "live-master",
      children: [cue("live-master", "camera", { duration_ms: null })],
    });
    const malformedWait = cue("bad-wait", "audio", { duration_ms: 3000, pre_wait_ms: Number.NaN });
    const result = buildCueStatusMetrics([
      cue("zero", "audio", { duration_ms: 0 }),
      cue("indefinite-image", "image", { duration_ms: null }),
      cue("indefinite-live", "mic", { duration_ms: null }),
      cue("omitted", "video", { duration_ms: null }),
      cue("bad", "audio", { duration_ms: Number.NaN }),
      malformedWait,
      finiteNumber,
    ]);
    expect(result.knownDurationMs).toBe(12000);
    expect(result.infiniteCount).toBe(2);
    expect(result.unknownDurationCount).toBe(3);
  });

  it("uses Rust group and Number summary wait semantics without adding waits twice", () => {
    const result = buildCueStatusMetrics([
      cue("group-summary", "group", { duration_ms: 4125, pre_wait_ms: 50, post_wait_ms: 75, group_mode: "sequential" }),
      cue("number-summary", "number", {
        duration_ms: 11000, pre_wait_ms: 2000, post_wait_ms: 1000,
        number_master_id: "number-master", children: [cue("number-master", "audio", { duration_ms: 10000 })],
      }),
      cue("number-unknown", "number", {
        duration_ms: null, number_master_id: "unknown-master", children: [cue("unknown-master", "audio")],
      }),
      cue("number-infinite", "number", {
        duration_ms: null, number_master_id: "loop-master",
        children: [cue("loop-master", "audio", { duration_ms: null, loop_count: 0xffff_ffff })],
      }),
      cue("trimmed-repeat-summary", "audio", { duration_ms: 5000, file_duration_ms: 1000, loop_count: 3 }),
    ]);
    expect(result.knownDurationMs).toBe(22125);
    expect(result.unknownDurationCount).toBe(1);
    expect(result.infiniteCount).toBe(1);
  });

  it("marks missing media and broken or warning cues once", () => {
    const missing = cue("bad", "audio", { media_file_missing: true, is_broken: true });
    const result = buildCueStatusMetrics([missing], new Set(["bad"]));
    expect(result.problems).toBe(1);
    expect(result.missingMedia).toBe(1);
  });
});

describe("status bar snapshot indicators", () => {
  it("marks absent and old samples stale", () => {
    expect(isStatusSampleStale(null, 10_000)).toBe(true);
    expect(isStatusSampleStale(6000, 10_000)).toBe(true);
    expect(isStatusSampleStale(9000, 10_000)).toBe(false);
    expect(formatBytePair(8 * 1024 ** 3, 16 * 1024 ** 3)).toBe("8.0/16 GB");
    expect(formatBytePair(1, 0)).toBe("—");
    expect(formatBytePair(Number.NaN, 10)).toBe("—");
  });

  it("requires sustained load and does not treat GPU saturation alone as failure", () => {
    let gpu = updateLoadBand("gpu", 100, 0);
    gpu = updateLoadBand("gpu", 100, 6000, gpu, false);
    expect(gpu.band).toBe("amber");
    gpu = updateLoadBand("gpu", 100, 11_000, gpu, true);
    expect(gpu.band).toBe("amber");
    gpu = updateLoadBand("gpu", 100, 17_000, gpu, true);
    expect(gpu.band).toBe("red");
    const cpu = updateLoadBand("cpu", 96, 0);
    expect(cpu.band).toBe("neutral");
    expect(DEFAULT_STATUS_BAR_PREFERENCES.visible).toBe(true);
  });

  it("promotes amber to red after sustained saturation, then recovers with hysteresis", () => {
    let state = updateLoadBand("cpu", 85, 0);
    state = updateLoadBand("cpu", 85, 3000, state);
    expect(state.band).toBe("amber");
    state = updateLoadBand("cpu", 99, 4000, state);
    state = updateLoadBand("cpu", 99, 9000, state);
    expect(state.band).toBe("red");
    state = updateLoadBand("cpu", 90, 10000, state);
    state = updateLoadBand("cpu", 90, 13000, state);
    expect(state.band).toBe("amber");
    state = updateLoadBand("cpu", 60, 14000, state);
    expect(state.band).toBe("neutral");
  });
});
