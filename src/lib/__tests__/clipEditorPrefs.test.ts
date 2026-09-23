import { describe, expect, it } from "vitest";
import { CLIP_EDITOR_TAB_LABELS, clipEditorDockVisible, normalizeClipEditorVisibility, previewPlayheadForCue, resolveClipEditorCueId, resolveClipEditorTargetCueId, toggleClipEditorPanel } from "../clipEditorPrefs";

describe("Clip Editor workspace visibility", () => {
  it("migrates old projects to both panels visible with Live active", () => {
    expect(normalizeClipEditorVisibility({})).toEqual({
      show_live_panel: true,
      show_slice_panel: true,
      clip_editor_active_tab: "Live",
    });
  });

  it("repairs the active tab for each visibility combination", () => {
    const both = normalizeClipEditorVisibility({});
    expect(toggleClipEditorPanel(both, "Live")).toMatchObject({ show_live_panel: false, show_slice_panel: true, clip_editor_active_tab: "Slice" });
    expect(toggleClipEditorPanel(both, "Slice")).toMatchObject({ show_live_panel: true, show_slice_panel: false, clip_editor_active_tab: "Live" });
    const onlyLive = { ...both, show_slice_panel: false, clip_editor_active_tab: "Live" as const };
    expect(toggleClipEditorPanel(onlyLive, "Live")).toMatchObject({ show_live_panel: false, show_slice_panel: false });
    const onlySlice = { ...both, show_live_panel: false, clip_editor_active_tab: "Slice" as const };
    expect(toggleClipEditorPanel(onlySlice, "Slice")).toMatchObject({ show_live_panel: false, show_slice_panel: false });
  });

  it("loads a hidden Slice preference without changing it during migration", () => {
    expect(normalizeClipEditorVisibility({ show_live_panel: false, show_slice_panel: true, clip_editor_active_tab: "Slice" })).toEqual({
      show_live_panel: false,
      show_slice_panel: true,
      clip_editor_active_tab: "Slice",
    });
  });

  it("keeps the dock shell visible and follows selected cue changes", () => {
    expect(CLIP_EDITOR_TAB_LABELS).toEqual({ timeline: "Timeline", slice: "Slice" });
    expect(clipEditorDockVisible(true, false)).toBe(true);
    expect(clipEditorDockVisible(false, true)).toBe(true);
    expect(clipEditorDockVisible(false, false)).toBe(false);
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
});
