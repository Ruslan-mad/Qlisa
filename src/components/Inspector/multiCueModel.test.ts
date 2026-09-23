import { describe, expect, it } from "vitest";
import type { CueSummary } from "../../lib/types";
import {
  buildBulkCueUpdates,
  getCuePropertyState,
  getFadeValueState,
  getMultiCueCapabilities,
  findCueSummaryById,
  hasDirtyFadeFields,
  isCurrentApplyRequest,
  isCueLiveForFadeEdit,
  getTriState,
  getValueState,
  nextGeneration,
  isCurrentSelectionLoad,
  multiCueDraftReducer,
  shouldRefreshFadeCapabilities,
  shouldBlockFadeApply,
  withApplyError,
  withCapabilityCueRefresh,
  type MultiCueRecord,
} from "./multiCueModel";

function cue(id: string, cue_type: CueSummary["cue_type"], extra: Record<string, unknown> = {}): MultiCueRecord {
  const summary: CueSummary = {
    id,
    cue_type,
    name: id,
    number: null,
    notes: "",
    state: "standby",
    continue_mode: "do_not_continue",
    color: "none",
    pre_wait_ms: 0,
    post_wait_ms: 0,
    duration_ms: null,
    file_path: null,
    file_size_bytes: null,
    media_width: null,
    media_height: null,
    media_file_missing: false,
    is_loading: false,
    is_disabled: false,
    is_broken: false,
    is_warning: false,
    file_duration_ms: null,
  };
  return { ...summary, ...extra } as MultiCueRecord;
}

describe("multi-cue Inspector model", () => {
  it("distinguishes uniform, mixed and empty values, including null", () => {
    expect(getValueState([4, 4])).toEqual({ kind: "uniform", value: 4 });
    expect(getValueState([null, null])).toEqual({ kind: "uniform", value: null });
    expect(getValueState([null, 4])).toEqual({ kind: "mixed" });
    expect(getValueState([])).toEqual({ kind: "empty" });
  });

  it("uses true tri-state semantics for mixed booleans", () => {
    expect(getTriState([true, true])).toBe("checked");
    expect(getTriState([false, false])).toBe("unchecked");
    expect(getTriState([true, false])).toBe("mixed");
  });

  it("enables shared tabs only when every selected cue supports that property family", () => {
    expect(getMultiCueCapabilities(["audio", "video"])).toEqual({
      levels: true, audioFade: true, visualFade: false, visual: false, outputs: false,
    });
    expect(getMultiCueCapabilities(["video", "image", "camera"])).toEqual({
      levels: false, audioFade: false, visualFade: true, visual: true, outputs: true,
    });
    expect(getMultiCueCapabilities(["video"])).toEqual({
      levels: true, audioFade: true, visualFade: true, visual: true, outputs: true,
    });
    expect(getMultiCueCapabilities(["video", "text"])).toEqual({
      levels: false, audioFade: false, visualFade: false, visual: false, outputs: true,
    });
    expect(getMultiCueCapabilities(["audio", "image"])).toEqual({
      levels: false, audioFade: false, visualFade: false, visual: false, outputs: false,
    });
    expect(getMultiCueCapabilities([])).toEqual({
      levels: false, audioFade: false, visualFade: false, visual: false, outputs: false,
    });
  });

  it("maps semantic audio and visual fades onto each cue schema and sends only dirty keys", () => {
    const cues = [
      cue("a", "audio", { fade_in_ms: 1000, fade_in_curve: "linear", fade_out_ms: null, fade_out_curve: null }),
      cue("v", "video", {
        fade_in_ms: 2000, fade_in_curve: "s_curve", fade_out_ms: null, fade_out_curve: null,
        video_fade_in_ms: 3000, video_fade_in_curve: "exponential", video_fade_out_ms: null, video_fade_out_curve: null,
      }),
      cue("i", "image", { fade_in_ms: 4000, fade_in_curve: "linear", fade_out_ms: null, fade_out_curve: null }),
      cue("c", "camera", { video_fade_in_ms: 5000, video_fade_in_curve: "s_curve", video_fade_out_ms: null, video_fade_out_curve: null }),
    ];

    expect(getCuePropertyState(cues, "color")).toEqual({ kind: "uniform", value: "none" });
    expect(getFadeValueState(cues.slice(0, 2), "audio", "in", "Ms")).toEqual({ kind: "mixed" });
    expect(buildBulkCueUpdates(cues.slice(0, 2), { notes: "show note", audioFadeInMs: 750 })).toEqual([
      { cueId: "a", properties: { notes: "show note", fade_in_ms: 750 } },
      { cueId: "v", properties: { notes: "show note", fade_in_ms: 750 } },
    ]);
    expect(buildBulkCueUpdates(cues.slice(1), { visualFadeOutCurve: "linear" })).toEqual([
      { cueId: "v", properties: { video_fade_out_curve: "linear" } },
      { cueId: "i", properties: { fade_out_curve: "linear" } },
      { cueId: "c", properties: { video_fade_out_curve: "linear" } },
    ]);
  });

  it("keeps dirty fields until reset and rejects stale async selection loads", () => {
    const edited = multiCueDraftReducer({}, { type: "set", field: "pre_wait_ms", value: 1250 });
    expect(multiCueDraftReducer(edited, { type: "set", field: "notes", value: "note" })).toEqual({ pre_wait_ms: 1250, notes: "note" });
    expect(multiCueDraftReducer(edited, { type: "reset" })).toEqual({});
    expect(isCurrentSelectionLoad(true, 4, 4)).toBe(true);
    expect(isCurrentSelectionLoad(true, 3, 4)).toBe(false);
    expect(isCurrentSelectionLoad(false, 4, 4)).toBe(false);
  });

  it("rejects an old Apply completion after an A to B to A selection cycle", () => {
    const firstApplyGeneration = nextGeneration(0);
    const switchedToBGeneration = nextGeneration(firstApplyGeneration);
    const returnedToAGeneration = nextGeneration(switchedToBGeneration);

    expect(isCurrentApplyRequest(firstApplyGeneration, returnedToAGeneration, "A", "A")).toBe(false);
    expect(isCurrentApplyRequest(returnedToAGeneration, returnedToAGeneration, "A", "A")).toBe(true);
  });

  it("blocks dirty fades for live, loading, preloaded or fading cues, but permits common edits", () => {
    const liveStates = ["running", "paused", "loading", "preloaded", "fading"];
    for (const state of liveStates) {
      const liveCue = cue(`cue-${state}`, "audio", { state });
      expect(isCueLiveForFadeEdit(liveCue)).toBe(true);
      expect(shouldBlockFadeApply([liveCue], { audioFadeInMs: 500 })).toBe(true);
      expect(shouldBlockFadeApply([liveCue], { notes: "safe while running" })).toBe(false);
    }

    const loadingFlagCue = cue("loading", "audio", { is_loading: true });
    const readyCue = cue("ready", "audio");
    expect(hasDirtyFadeFields({ audioFadeOutCurve: "linear" })).toBe(true);
    expect(hasDirtyFadeFields({ color: "blue", notes: "shared" })).toBe(false);
    expect(shouldBlockFadeApply([loadingFlagCue], { audioFadeOutCurve: "linear" })).toBe(true);
    expect(shouldBlockFadeApply([readyCue], { audioFadeOutCurve: "linear" })).toBe(false);
  });

  it("honours backend fade editability for a preloaded standby cue without blocking common edits", () => {
    const preloadedStandby = cue("preloaded-video", "video", {
      _bulk_edit: { fade_editable: false },
    });
    const ordinaryStandby = cue("ready-video", "video", {
      _bulk_edit: { fade_editable: true },
    });

    expect(shouldBlockFadeApply([preloadedStandby], { visualFadeInMs: 400 })).toBe(true);
    expect(shouldBlockFadeApply([ordinaryStandby], { visualFadeInMs: 400 })).toBe(false);
    expect(shouldBlockFadeApply([preloadedStandby], { notes: "still safe" })).toBe(false);
  });

  it("refreshes fade capability after live/loading cues become terminal and debounces unchanged state", () => {
    const liveSummary = { id: "cue", state: "running", is_loading: false } as CueSummary;
    const stoppedSummary = { id: "cue", state: "standby", is_loading: false } as CueSummary;
    const live = [{ cueId: "cue", state: "running", isLoading: false, hasTiming: true, summaryRevision: liveSummary }];
    const stopped = [{ cueId: "cue", state: "standby", isLoading: false, hasTiming: false, summaryRevision: stoppedSummary }];

    expect(shouldRefreshFadeCapabilities(live, stopped, ["cue"])).toBe(true);
    expect(shouldRefreshFadeCapabilities(live, stopped, [])).toBe(false);
    expect(shouldRefreshFadeCapabilities(stopped, stopped, ["cue"])).toBe(false);

    const preloadedBefore = { id: "preloaded", state: "standby", is_loading: true } as CueSummary;
    const preloadedAfter = { id: "preloaded", state: "standby", is_loading: true } as CueSummary;
    const stillPreloaded = [{ cueId: "preloaded", state: "standby", isLoading: true, hasTiming: false, summaryRevision: preloadedBefore }];
    expect(shouldRefreshFadeCapabilities(stillPreloaded, stillPreloaded, ["preloaded"])).toBe(false);
    expect(shouldRefreshFadeCapabilities(
      stillPreloaded,
      [{ cueId: "preloaded", state: "standby", isLoading: true, hasTiming: false, summaryRevision: preloadedAfter }],
      ["preloaded"],
    )).toBe(true);
  });

  it("replaces complete capability records without resetting a dirty draft", () => {
    const previousCue = cue("preloaded", "video", { _bulk_edit: { fade_editable: false }, name: "Old name" });
    const refreshedCue = cue("preloaded", "video", { _bulk_edit: { fade_editable: true }, name: "Fresh name" });
    const preloadedSummary = { id: "preloaded", state: "standby", is_loading: true } as CueSummary;
    const stoppedSummary = { id: "preloaded", state: "standby", is_loading: true } as CueSummary;
    const draft = multiCueDraftReducer({}, { type: "set", field: "visualFadeInMs", value: 700 });
    expect(shouldBlockFadeApply([previousCue], draft)).toBe(true);
    expect(shouldRefreshFadeCapabilities(
      [{ cueId: "preloaded", state: "standby", isLoading: true, hasTiming: false, summaryRevision: preloadedSummary }],
      [{ cueId: "preloaded", state: "standby", isLoading: true, hasTiming: false, summaryRevision: stoppedSummary }],
      ["preloaded"],
    )).toBe(true);
    const refreshed = withCapabilityCueRefresh(
      { selectionKey: "preloaded", cues: [previousCue], loading: false, error: null },
      "preloaded",
      [refreshedCue],
    );

    expect(refreshed.cues?.[0]).toMatchObject({ name: "Fresh name", _bulk_edit: { fade_editable: true } });
    expect(draft).toEqual({ visualFadeInMs: 700 });
    expect(shouldBlockFadeApply(refreshed.cues ?? [], draft)).toBe(false);
  });

  it("finds nested cue runtime state and preserves drafts on Apply errors", () => {
    const nested = cue("leaf", "video", { state: "paused" });
    const group = cue("group", "group", { children: [nested] });
    expect(findCueSummaryById([group], "leaf")?.state).toBe("paused");

    const dirtyDraft = { notes: "new note", audioFadeInMs: 600 };
    const failedLoad = withApplyError(
      { selectionKey: "leaf", cues: [nested], loading: false, error: null },
      "leaf",
      "backend rejected update",
    );
    expect(failedLoad.error).toBe("backend rejected update");
    expect(failedLoad.cues).toEqual([nested]);
    expect(multiCueDraftReducer(dirtyDraft, { type: "applyFailed" })).toBe(dirtyDraft);
  });
});
