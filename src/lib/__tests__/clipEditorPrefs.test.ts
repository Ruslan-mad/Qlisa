import { describe, expect, it } from "vitest";
import { activeLiveMediaCandidates, canSeekProgramFromClipEditor, CLIP_EDITOR_TAB_LABELS, clipEditorDockVisible, normalizeClipEditorVisibility, previewPlayheadForCue, resolveClipEditorCueId, resolveClipEditorTargetCueId, resolveLiveMediaSelection, toggleClipEditorPanel } from "../clipEditorPrefs";

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

  it("filters screensaver descendants and resolves newest or sticky manual Live media", () => {
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
    ], { old: 1, "screen-child": 2, "nested-child": 3, "pre-wait": 5, "waiting-child": 6, new: 4 }, {
      "pre-wait": { wait_phase: "pre_wait" },
      "waiting-number": { wait_phase: "pre_wait" },
    });

    expect(candidates.map(({ id }) => id)).toEqual(["no-order", "old", "new"]);
    expect(resolveLiveMediaSelection(candidates, null)?.id).toBe("new");
    expect(resolveLiveMediaSelection(candidates, "old")?.id).toBe("old");
    expect(resolveLiveMediaSelection(candidates, "old")).toBe(resolveLiveMediaSelection(candidates, "old"));
    expect(resolveLiveMediaSelection(candidates, "ended")?.id).toBe("new");
    expect(resolveLiveMediaSelection([], "old")).toBeNull();
  });

  it("never sends Preview or Slice clicks to the program seek command", () => {
    expect(canSeekProgramFromClipEditor("Preview", "running", "audio")).toBe(false);
    expect(canSeekProgramFromClipEditor("Slice", "paused", "video")).toBe(false);
    expect(canSeekProgramFromClipEditor("Live", "running", "audio")).toBe(true);
    expect(canSeekProgramFromClipEditor("Live", "paused", "image")).toBe(false);
    expect(canSeekProgramFromClipEditor("Live", "standby", "video")).toBe(false);
  });
});
