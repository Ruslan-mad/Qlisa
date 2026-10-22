import { describe, expect, it } from "vitest";
import { activeLiveMediaCandidates, canSeekProgramFromClipEditor, CLIP_EDITOR_TAB_LABELS, clipEditorDockVisible, createManualLiveMediaSelection, normalizeClipEditorVisibility, previewPlayheadForCue, resolveClipEditorCueId, resolveClipEditorTargetCueId, resolveLiveMediaSelection, shouldFollowLatestLiveMedia, toggleClipEditorDock, toggleClipEditorPanel } from "../clipEditorPrefs";

describe("Clip Editor workspace visibility", () => {
  it("migrates old projects to both panels visible with Preview active", () => {
    expect(normalizeClipEditorVisibility({})).toEqual({
      show_live_panel: true,
      show_slice_panel: true,
      clip_editor_active_tab: "Live",
      clip_editor_active_tab_v2: "Preview",
    });
  });

  it("repairs the active tab for each visibility combination", () => {
    const both = normalizeClipEditorVisibility({});
    expect(toggleClipEditorPanel(both, "Preview")).toMatchObject({ show_live_panel: false, show_slice_panel: true, clip_editor_active_tab_v2: "Live" });
    expect(toggleClipEditorPanel(both, "Slice")).toMatchObject({ show_live_panel: true, show_slice_panel: false, clip_editor_active_tab_v2: "Preview" });
    const onlyLive = { ...both, show_slice_panel: false, clip_editor_active_tab_v2: "Live" as const };
    expect(toggleClipEditorPanel(onlyLive, "Preview")).toMatchObject({ show_live_panel: false, show_slice_panel: false, clip_editor_active_tab_v2: "Live" });
    const onlyPreview = { ...both, show_live_panel: false, clip_editor_active_tab_v2: "Live" as const };
    expect(toggleClipEditorPanel(onlyPreview, "Slice")).toMatchObject({ show_live_panel: false, show_slice_panel: false, clip_editor_active_tab_v2: "Live" });
  });

  it("migrates the legacy Live timeline tab to Preview and preserves Slice", () => {
    expect(normalizeClipEditorVisibility({ clip_editor_active_tab: "Live" }).clip_editor_active_tab_v2).toBe("Preview");
    expect(normalizeClipEditorVisibility({ show_live_panel: false, show_slice_panel: true, clip_editor_active_tab: "Slice" })).toEqual({
      show_live_panel: false,
      show_slice_panel: true,
      clip_editor_active_tab: "Slice",
      clip_editor_active_tab_v2: "Slice",
    });
    expect(normalizeClipEditorVisibility({ clip_editor_active_tab: "Live", clip_editor_active_tab_v2: "Live" }).clip_editor_active_tab_v2).toBe("Live");
  });

  it("closes the dock explicitly and restores the retained Live tab on reopen", () => {
    expect(CLIP_EDITOR_TAB_LABELS).toEqual({ preview: "Preview", live: "Live", slice: "Slice" });
    expect(clipEditorDockVisible(true, false)).toBe(true);
    expect(clipEditorDockVisible(false, true)).toBe(true);
    expect(clipEditorDockVisible(false, false)).toBe(false);
    const livePrefs = normalizeClipEditorVisibility({ clip_editor_active_tab_v2: "Live" });
    expect(livePrefs.clip_editor_active_tab_v2).toBe("Live");
    expect(clipEditorDockVisible(false, false)).toBe(false);
    expect(clipEditorDockVisible(true, false)).toBe(true);
    expect(livePrefs.clip_editor_active_tab_v2).toBe("Live");
    expect(resolveClipEditorCueId("clicked-cue", "inspector-cue")).toBe("clicked-cue");
    expect(resolveClipEditorCueId(null, "inspector-cue")).toBe("inspector-cue");
    expect(resolveClipEditorCueId(null, null)).toBeNull();
  });


  it("toggles the dock container without changing available panels or the selected tab", () => {
    const state = normalizeClipEditorVisibility({ clip_editor_active_tab_v2: "Slice" });
    const hidden = toggleClipEditorDock(true, state);
    expect(hidden.showDock).toBe(false);
    expect(hidden.visibility).toBe(state);
    expect(hidden.visibility).toEqual(state);

    const restored = toggleClipEditorDock(false, hidden.visibility);
    expect(restored.showDock).toBe(true);
    expect(restored.visibility).toBe(state);
    expect(restored.visibility).toEqual(state);
  });

  it("restores a usable Preview panel when both tabs were disabled in the View menu", () => {
    const noPanels = normalizeClipEditorVisibility({
      show_live_panel: false,
      show_slice_panel: false,
      clip_editor_active_tab_v2: "Slice",
    });
    const opened = toggleClipEditorDock(false, noPanels);
    expect(opened.showDock).toBe(true);
    expect(opened.visibility.show_live_panel).toBe(true);
    expect(opened.visibility.show_slice_panel).toBe(false);
    expect(clipEditorDockVisible(opened.visibility.show_live_panel, opened.visibility.show_slice_panel)).toBe(true);
  });

  it("keeps the Number timeline for Number selection but targets a child cue directly", () => {
    expect(resolveClipEditorTargetCueId("number-1", "editor-1", "number", "number-1")).toBe("number-1");
    expect(resolveClipEditorTargetCueId("child-1", "number-1", "audio", "number-1")).toBe("child-1");
    expect(resolveClipEditorTargetCueId("child-1", "number-1", "video", "number-1")).toBe("child-1");
  });

  it("only exposes an active preview playhead for the displayed cue", () => {
    const event = {
      cue_id: "cue-a",
      media_position_ms: 1234,
      playing: true,
      active: true,
      generation: 7,
    } as const;
    expect(previewPlayheadForCue(event, "cue-a")).toEqual(event);
    expect(previewPlayheadForCue(event, "cue-b")).toBeNull();
    expect(previewPlayheadForCue({ ...event, active: false }, "cue-a")).toBeNull();
    expect(previewPlayheadForCue(null, "cue-a")).toBeNull();
  });

  it("keeps screensaver media at the top and selects the latest running ordinary track", () => {
    const candidates = activeLiveMediaCandidates([
      { id: "old", cue_type: "audio", state: "running", file_path: "old.wav" },
      { id: "screen", cue_type: "number", state: "running", background_screensaver: true, file_path: null, children: [
        { id: "screen-child", cue_type: "video", state: "running", file_path: "screen.mp4" },
        { id: "nested", cue_type: "group", state: "running", file_path: null, children: [
          { id: "nested-child", cue_type: "audio", state: "paused", file_path: "nested.wav" },
        ] },
      ] },
      { id: "loading", cue_type: "video", state: "running", is_loading: true, file_path: "loading.mp4" },
      { id: "no-order", cue_type: "image", state: "running", file_path: "no-order.png" },
      { id: "pre-wait", cue_type: "audio", state: "running", file_path: "pre-wait.wav" },
      { id: "waiting-number", cue_type: "number", state: "running", file_path: null, children: [
        { id: "waiting-child", cue_type: "video", state: "running", file_path: "waiting.mp4" },
      ] },
      { id: "new", cue_type: "video", state: "paused", file_path: "new.mp4" },
      { id: "later", cue_type: "video", state: "running", file_path: "later.mp4" },
    ], { old: 1, "screen-child": 2, "nested-child": 3, "pre-wait": 5, "waiting-child": 6, new: 4, later: 7 }, {
      "pre-wait": { wait_phase: "pre_wait" },
      "waiting-number": { wait_phase: "pre_wait" },
    });

    expect(candidates.map(({ id }) => id)).toEqual(["screen-child", "nested-child", "no-order", "old", "new", "later"]);
    expect(candidates[0].isBackgroundScreensaver).toBe(true);
    expect(candidates[1].isBackgroundScreensaver).toBe(true);
    expect(candidates[3].isBackgroundScreensaver).toBe(false);
    expect(resolveLiveMediaSelection(candidates, null)?.id).toBe("later");
    expect(resolveLiveMediaSelection([...candidates].reverse(), null)?.id).toBe("later");
  });

  it("keeps manual choice until a newer eligible ordinary start, including a restart", () => {
    const candidates = activeLiveMediaCandidates([
      { id: "screen", cue_type: "number", state: "running", background_screensaver: true, children: [
        { id: "screen-file", cue_type: "video", state: "running", file_path: "screen.mp4" },
      ] },
      { id: "normal-a", cue_type: "audio", state: "running", file_path: "a.wav" },
      { id: "paused", cue_type: "video", state: "paused", file_path: "paused.mp4" },
    ], { "screen-file": 1, "normal-a": 2, paused: 3 });
    const manualBackground = { cueId: "screen-file", afterOrder: 3 };
    expect(resolveLiveMediaSelection(candidates, manualBackground)?.id).toBe("screen-file");
    expect(shouldFollowLatestLiveMedia(candidates, manualBackground)).toBe(false);
    expect(shouldFollowLatestLiveMedia(candidates, { cueId: "screen-file", afterOrder: 2 })).toBe(false);

    const newer = activeLiveMediaCandidates([
      { id: "screen", cue_type: "number", state: "running", background_screensaver: true, children: [
        { id: "screen-file", cue_type: "video", state: "running", file_path: "screen.mp4" },
      ] },
      { id: "normal-a", cue_type: "audio", state: "running", file_path: "a.wav" },
      { id: "normal-b", cue_type: "video", state: "running", file_path: "b.mp4" },
      { id: "paused", cue_type: "video", state: "paused", file_path: "paused.mp4" },
    ], { "screen-file": 1, "normal-a": 2, paused: 3, "normal-b": 4 });
    expect(shouldFollowLatestLiveMedia(newer, manualBackground)).toBe(true);
    expect(resolveLiveMediaSelection(newer, manualBackground)?.id).toBe("normal-b");

    const restartedSameId = activeLiveMediaCandidates([
      { id: "normal-a", cue_type: "audio", state: "running", file_path: "a.wav" },
    ], { "normal-a": 5 });
    const selectedSameIdEarlier = { cueId: "normal-a", afterOrder: 4 };
    expect(shouldFollowLatestLiveMedia(restartedSameId, selectedSameIdEarlier)).toBe(true);
    expect(resolveLiveMediaSelection(restartedSameId, selectedSameIdEarlier)?.id).toBe("normal-a");
  });

  it("waits for delayed candidates, then falls back to remaining running media or a background", () => {
    const manual = { cueId: "a", afterOrder: 2 };
    const beforeCatalogUpdate = activeLiveMediaCandidates([
      { id: "a", cue_type: "audio", state: "running", file_path: "a.wav" },
    ], { a: 2, b: 3 });
    expect(resolveLiveMediaSelection(beforeCatalogUpdate, manual)?.id).toBe("a");
    const afterCatalogUpdate = activeLiveMediaCandidates([
      { id: "a", cue_type: "audio", state: "running", file_path: "a.wav" },
      { id: "b", cue_type: "video", state: "running", file_path: "b.mp4" },
    ], { a: 2, b: 3 });
    expect(resolveLiveMediaSelection(afterCatalogUpdate, manual)?.id).toBe("b");

    const remaining = afterCatalogUpdate.filter((candidate) => candidate.id === "a");
    expect(resolveLiveMediaSelection(remaining, null)?.id).toBe("a");
    const backgroundOnly = activeLiveMediaCandidates([
      { id: "screen", cue_type: "number", state: "running", background_screensaver: true, children: [
        { id: "screen-file", cue_type: "image", state: "running", file_path: "screen.png" },
      ] },
      { id: "paused", cue_type: "video", state: "paused", file_path: "paused.mp4" },
    ], { "screen-file": 1, paused: 2 });
    expect(resolveLiveMediaSelection(backgroundOnly, null)?.id).toBe("screen-file");
    expect(resolveLiveMediaSelection([], null)).toBeNull();

    const normalWins = activeLiveMediaCandidates([
      { id: "normal", cue_type: "audio", state: "running", file_path: "normal.wav" },
      { id: "screen", cue_type: "number", state: "running", background_screensaver: true, children: [
        { id: "screen-file", cue_type: "image", state: "running", file_path: "screen.png" },
      ] },
    ], { normal: 1, "screen-file": 99 });
    expect(resolveLiveMediaSelection(normalWins, null)?.id).toBe("normal");
  });

  it("does not let newer background or paused order block a delayed ordinary start", () => {
    const tree = [
      { id: "a", cue_type: "audio", state: "running", file_path: "a.wav" },
      { id: "screen", cue_type: "number", state: "running", background_screensaver: true, children: [
        { id: "screen-file", cue_type: "video", state: "running", file_path: "screen.mp4" },
      ] },
      { id: "paused", cue_type: "audio", state: "paused", file_path: "paused.wav" },
      { id: "prewait", cue_type: "number", state: "running", children: [
        { id: "b", cue_type: "video", state: "running", file_path: "b.mp4" },
      ] },
    ];
    const orders = { a: 1, "screen-file": 99, paused: 100, b: 3 };
    const beforeStart = activeLiveMediaCandidates(tree, orders, { prewait: { wait_phase: "pre_wait" } });
    const manualBackground = createManualLiveMediaSelection(beforeStart, "screen-file");
    expect(manualBackground.afterOrder).toBe(1);
    expect(resolveLiveMediaSelection(beforeStart, manualBackground)?.id).toBe("screen-file");

    const afterStart = activeLiveMediaCandidates(tree, orders);
    expect(shouldFollowLatestLiveMedia(afterStart, manualBackground)).toBe(true);
    expect(resolveLiveMediaSelection(afterStart, manualBackground)?.id).toBe("b");
  });

  it("keeps the latest paused media visible only when no running media remains", () => {
    const pausedOnly = activeLiveMediaCandidates([
      { id: "older", cue_type: "audio", state: "paused", file_path: "older.wav" },
      { id: "newer", cue_type: "video", state: "paused", file_path: "newer.mp4" },
    ], { older: 1, newer: 2 });
    expect(resolveLiveMediaSelection(pausedOnly, null)?.id).toBe("newer");

    const withRunning = activeLiveMediaCandidates([
      { id: "paused", cue_type: "audio", state: "paused", file_path: "paused.wav" },
      { id: "running", cue_type: "image", state: "running", file_path: "running.png" },
    ], { paused: 10, running: 2 });
    expect(resolveLiveMediaSelection(withRunning, null)?.id).toBe("running");
  });

  it("never sends Preview or Slice clicks to the program seek command", () => {
    expect(canSeekProgramFromClipEditor("Preview", "running", "audio")).toBe(false);
    expect(canSeekProgramFromClipEditor("Slice", "paused", "video")).toBe(false);
    expect(canSeekProgramFromClipEditor("Live", "running", "audio")).toBe(true);
    expect(canSeekProgramFromClipEditor("Live", "paused", "image")).toBe(false);
    expect(canSeekProgramFromClipEditor("Live", "standby", "video")).toBe(false);
  });
});
