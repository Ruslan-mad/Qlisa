import { describe, expect, it } from "vitest";
import { applyNumberDragSnap, composeGroupWaveform, groupAudioSegments, groupDurationMs, numberActionDuration, numberActionLooped, numberChildTimelineStartMs, numberDragRange, numberMasterDuration, numberPreviewItem, numberPreviewSourcePosition, numberPreviewSourceWindow, numberVisualActions, shouldRetainNumberPreviewAsset, snapNumberTime } from "./numberTimelineModel";

const child = (id: string, type: "video" | "image", extra: Record<string, unknown> = {}) => ({
  id, cue_type: type, name: id, duration_ms: 4000, file_duration_ms: 5000,
  ...extra,
}) as any;

describe("numberTimelineModel", () => {
  it("includes audio actions and snaps to nearby master/action edges", () => {
    const cue = { number_master_id: "master", number_action_offsets_ms: { a: 1000 }, children: [
      child("master", "video", { duration_ms: 5000, file_duration_ms: 5000 }),
      { ...child("a", "video"), cue_type: "audio" },
    ] } as any;
    expect(numberVisualActions(cue).map((a) => a.id)).toEqual(["a"]);
    expect(snapNumberTime(995, 5000, [1000, 2200], 20)).toBe(1000);
    expect(snapNumberTime(-5, 5000, [], 20)).toBe(0);
    expect(snapNumberTime(3100, 5000, [3000], 20)).toBe(3100);
  });
  it("uses the master duration and clips overlapping visual actions", () => {
    const cue = { number_master_id: "master", number_action_offsets_ms: { v: 3000, i: 1000 }, children: [
      child("master", "video", { duration_ms: 10000, file_duration_ms: 10000 }),
      child("v", "video", { start_time_ms: 1000, end_time_ms: 9000 }),
      child("i", "image", { display_duration_ms: 2000 }),
    ] } as any;
    expect(numberMasterDuration(cue)).toBe(10000);
    expect(numberVisualActions(cue).map((a) => [a.id, a.timeline_start_ms, a.timeline_end_ms])).toEqual([
      ["v", 3000, 10000], ["i", 1000, 3000],
    ]);
  });

  it("converts a trimmed video into a duration", () => {
    expect(numberActionDuration(child("v", "video", { start_time_ms: 500, end_time_ms: 2500 }), 5000)).toBe(2000);
    expect(numberActionDuration(child("i", "image", { display_duration_ms: null }), 5000)).toBe(5000);
    expect(numberActionDuration({ ...child("g", "image"), cue_type: "group", duration_ms: 3200 } as any, 5000)).toBe(3200);
  });

  it("extends a looping action to the Number and keeps a non-looping action finite", () => {
    const cue = { number_master_id: "master", number_action_offsets_ms: { short: 1000, loop: 1500 }, children: [
      child("master", "video", { duration_ms: 10000, file_duration_ms: 10000 }),
      child("short", "video", { file_duration_ms: 2500 }),
      child("loop", "video", { file_duration_ms: 2500, loop_count: 4294967295 }),
    ] } as any;
    expect(numberVisualActions(cue).map((action) => [action.id, action.timeline_start_ms, action.timeline_end_ms, action.looped])).toEqual([
      ["short", 1000, 3500, false], ["loop", 1500, 10000, true],
    ]);
    expect(numberActionLooped(cue.children[2])).toBe(true);
    expect(numberPreviewSourcePosition(numberVisualActions(cue)[1], 4000)).toBe(0);
  });

  it("uses cropped source bounds for a manual preview loop", () => {
    const action = child("loop", "video", { start_time_ms: 1200, end_time_ms: 3200, loop_count: 4294967295 });
    expect(numberPreviewSourceWindow(action)).toEqual({ startMs: 1200, endMs: 3200 });
    expect(numberPreviewSourcePosition({ ...action, timeline_start_ms: 500, looped: true }, 2600)).toBe(1300);
    expect(numberPreviewSourcePosition({ ...action, timeline_start_ms: 500, looped: true }, 3700)).toBe(2400);
  });

  it("retains a decoded layer while the next layer is loading", () => {
    expect(shouldRetainNumberPreviewAsset("next", "previous", "asset://previous")).toBe(true);
    expect(shouldRetainNumberPreviewAsset("next", "next", "asset://next")).toBe(false);
    expect(shouldRetainNumberPreviewAsset("next", "previous", null)).toBe(false);
    expect(shouldRetainNumberPreviewAsset(null, "previous", "asset://previous")).toBe(false);
  });

  it("bounds a resize to the available source span", () => {
    expect(numberDragRange("end", 1000, 3000, 9000, 10000, 2500)).toEqual({ startMs: 1000, endMs: 3500 });
    expect(numberDragRange("start", 1000, 5000, -1000, 10000, 2500)).toEqual({ startMs: 2500, endMs: 5000 });
  });

  it("defaults omitted action offsets to zero", () => {
    const cue = { number_master_id: "master", children: [
      child("master", "video", { duration_ms: 10000, file_duration_ms: 10000 }),
      child("v", "video"),
    ] } as any;

    expect(numberVisualActions(cue).map((action) => [action.id, action.timeline_start_ms])).toEqual([["v", 0]]);
  });

  it("uses cached duration from raw get_cue child serialization", () => {
    const cue = { number_master_id: "master", children: [
      child("master", "video", { duration_ms: undefined, file_duration_ms: null, cached_duration_ms: 2500 }),
    ] } as any;
    expect(numberMasterDuration(cue)).toBe(2500);
  });

  it("moves a block without changing its duration", () => {
    expect(numberDragRange("move", 1000, 3000, 1500, 5000)).toEqual({ startMs: 1500, endMs: 3500 });
    expect(numberDragRange("move", 1000, 3000, -500, 5000)).toEqual({ startMs: 0, endMs: 2000 });
    expect(numberDragRange("move", 1000, 3000, 4900, 5000)).toEqual({ startMs: 3000, endMs: 5000 });
  });

  it("trims only the selected edge", () => {
    expect(numberDragRange("start", 1000, 3000, 1800, 5000)).toEqual({ startMs: 1800, endMs: 3000 });
    expect(numberDragRange("end", 1000, 3000, 2200, 5000)).toEqual({ startMs: 1000, endMs: 2200 });
  });

  it("does not let snapping move the opposite trim edge", () => {
    expect(applyNumberDragSnap("start", { startMs: 1800, endMs: 3000 }, { startMs: 2000, endMs: 2800 })).toEqual({ startMs: 2000, endMs: 3000 });
    expect(applyNumberDragSnap("end", { startMs: 1000, endMs: 2200 }, { startMs: 800, endMs: 2000 })).toEqual({ startMs: 1000, endMs: 2000 });
    expect(applyNumberDragSnap("move", { startMs: 1800, endMs: 3800 }, { startMs: 2000, endMs: 4000 })).toEqual({ startMs: 2000, endMs: 4000 });
  });

  it("uses the Number timeline offset as a nested child's displayed Pre-Wait", () => {
    const cue = { number_master_id: "master", number_action_offsets_ms: { action: 1250 }, children: [
      child("master", "video"), child("action", "image"),
    ] } as any;
    expect(numberChildTimelineStartMs(cue, "master")).toBe(0);
    expect(numberChildTimelineStartMs(cue, "action")).toBe(1250);
    expect(numberChildTimelineStartMs(cue, "missing")).toBeNull();
  });

  it("chooses the last active visual action and falls back to a visual master", () => {
    const cue = { number_master_id: "master", number_action_offsets_ms: { first: 500, second: 800 }, children: [
      child("master", "video", { duration_ms: 5000, file_duration_ms: 5000 }),
      child("first", "video", { start_time_ms: 100, end_time_ms: 2100 }),
      child("second", "image", { display_duration_ms: 1600 }),
    ] } as any;
    expect(numberPreviewItem(cue, 100)).toMatchObject({ id: "master" });
    expect(numberPreviewItem(cue, 900)).toMatchObject({ id: "second" });
    expect(numberPreviewItem(cue, 3000)).toMatchObject({ id: "master" });
    expect(numberPreviewSourcePosition(numberPreviewItem(cue, 900)!, 900)).toBe(100);
    const video = numberPreviewItem(cue, 600);
    expect(video?.id).toBe("first");
    expect(numberPreviewSourcePosition(video!, 1600)).toBe(1200);
  });

  it("does not invent a visual preview for an audio-only Number", () => {
    const cue = { number_master_id: "master", children: [
      { ...child("master", "video"), cue_type: "audio" },
    ] } as any;
    expect(numberPreviewItem(cue, 0)).toBeNull();
  });

  it("lays out Group audio children on a sequential Number track", () => {
    const group = {
      id: "group", cue_type: "group", group_mode: "sequential", pre_wait_ms: 100, post_wait_ms: 200,
      duration_ms: 3500, children: [
        { ...child("a", "video", { cue_type: "audio", duration_ms: 1000, file_duration_ms: 1000, pre_wait_ms: 50, post_wait_ms: 25 }) },
        { ...child("b", "video", { cue_type: "audio", duration_ms: 2000, file_duration_ms: 2000, pre_wait_ms: 75, post_wait_ms: 0 }) },
      ],
    } as any;
    expect(groupAudioSegments(group)).toMatchObject([
      { cueId: "a", startMs: 150, endMs: 1150 },
      { cueId: "b", startMs: 1250, endMs: 3250 },
    ]);
  });

  it("uses trim bounds from raw Number children for Group timing", () => {
    // get_cue(Number) returns nested media in its serialized form: there is
    // cached_duration_ms, not CueSummary.duration_ms. This is the exact data
    // shape used by the Number timeline after an individual Cue trim.
    const group = {
      id: "group", cue_type: "group", group_mode: "sequential", duration_ms: null,
      children: [
        { id: "a", cue_type: "audio", name: "a", cached_duration_ms: 10_000, start_time_ms: 1_000, end_time_ms: 3_000, pre_wait_ms: 0, post_wait_ms: 0 },
        { id: "b", cue_type: "audio", name: "b", cached_duration_ms: 8_000, start_time_ms: 2_000, end_time_ms: 5_000, pre_wait_ms: 0, post_wait_ms: 0 },
      ],
    } as any;
    expect(groupDurationMs(group)).toBe(5_000);
    expect(groupAudioSegments(group)).toMatchObject([
      { cueId: "a", startMs: 0, endMs: 2_000, sourceStartMs: 1_000, sourceEndMs: 3_000 },
      { cueId: "b", startMs: 2_000, endMs: 5_000, sourceStartMs: 2_000, sourceEndMs: 5_000 },
    ]);
  });

  it("derives a Group master duration when its summary omits duration", () => {
    const group = {
      id: "group", cue_type: "group", group_mode: "sequential", pre_wait_ms: 100, post_wait_ms: 200,
      duration_ms: null, children: [
        child("a", "video", { cue_type: "audio", duration_ms: 1000, file_duration_ms: 1000, pre_wait_ms: 50, post_wait_ms: 25 }),
        child("b", "video", { cue_type: "audio", duration_ms: 2000, file_duration_ms: 2000, pre_wait_ms: 75, post_wait_ms: 0 }),
      ],
    } as any;
    expect(groupDurationMs(group)).toBe(3450);
    expect(numberMasterDuration({ number_master_id: "group", children: [group] } as any)).toBe(3450);
  });

  it("combines overlapping Group waveforms without losing silent gaps", () => {
    const group = {
      id: "group", cue_type: "group", group_mode: "simultaneous", pre_wait_ms: 0, post_wait_ms: 0,
      duration_ms: 1000, children: [child("a", "video", { cue_type: "audio", duration_ms: 1000, file_duration_ms: 1000 })],
    } as any;
    const waveform = { peaks: [0.5, 0.25], rms: [0.2, 0.1], file_duration_s: 1 };
    const combined = composeGroupWaveform(group, { a: waveform }, 4)!;
    expect(combined.peaks).toEqual([0.5, 0.5, 0.25, 0.25]);
    expect(combined.rms).toEqual([0.2, 0.2, 0.1, 0.1]);
  });

  it("samples the trimmed source range in a raw Number Group waveform", () => {
    const group = {
      id: "group", cue_type: "group", group_mode: "simultaneous", duration_ms: null,
      children: [{ id: "a", cue_type: "audio", name: "a", cached_duration_ms: 4000, start_time_ms: 1000, end_time_ms: 3000 }],
    } as any;
    const combined = composeGroupWaveform(group, {
      a: { peaks: [0.1, 0.2, 0.3, 0.4], rms: [0.01, 0.02, 0.03, 0.04], file_duration_s: 4 },
    }, 2)!;
    expect(combined.file_duration_s).toBe(2);
    expect(combined.peaks).toEqual([0.2, 0.3]);
    expect(combined.rms).toEqual([0.02, 0.03]);
  });

  it("composes a Group waveform when its stored duration is absent", () => {
    const group = {
      id: "group", cue_type: "group", group_mode: "simultaneous", duration_ms: null,
      children: [child("a", "video", { cue_type: "audio", duration_ms: 1000, file_duration_ms: 1000 })],
    } as any;
    const combined = composeGroupWaveform(group, {
      a: { peaks: [0.75], rms: [0.4], file_duration_s: 1 },
    }, 2)!;
    expect(combined.file_duration_s).toBe(1);
    expect(combined.peaks).toEqual([0.75, 0.75]);
  });

});
