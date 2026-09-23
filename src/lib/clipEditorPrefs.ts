import type { DisplayPreferences, PreviewPlayheadEvent } from "./types";

export type ClipEditorTab = "Live" | "Slice";
export const CLIP_EDITOR_TAB_LABELS = { timeline: "Timeline", slice: "Slice" } as const;
export type ClipEditorVisibility = Pick<DisplayPreferences, "show_live_panel" | "show_slice_panel"> & {
  clip_editor_active_tab?: ClipEditorTab;
};

/**
 * The preview engine is shared by the app, so only the event for the cue
 * currently displayed by the dock may become a visible second playhead.
 * Keeping this gate outside the component also makes cue-switch/stop behavior
 * explicit and easy to exercise without a browser canvas.
 */
export function previewPlayheadForCue(
  preview: PreviewPlayheadEvent | null | undefined,
  cueId: string | null,
): PreviewPlayheadEvent | null {
  return cueId && preview?.active && preview.cue_id === cueId ? preview : null;
}

export function clipEditorDockVisible(showLivePanel: boolean, showSlicePanel: boolean): boolean {
  return showLivePanel || showSlicePanel;
}

/** Selected cues take precedence over an inspector-opened cue so a click in
 * the list updates the dock immediately; the latter remains a useful
 * fallback while no cue is selected. */
export function resolveClipEditorCueId(selectedCueId: string | null, editorCueId: string | null): string | null {
  return selectedCueId ?? editorCueId;
}

/**
 * A selected Number opens its multi-track timeline. Selecting one of its
 * children must instead address that child directly so ordinary Slice/trim
 * edits are persisted against the child's ID.
 */
export function resolveClipEditorTargetCueId(
  selectedCueId: string | null,
  editorCueId: string | null,
  selectedCueType: string | null | undefined,
  numberTimelineCueId: string | null,
): string | null {
  if (selectedCueType === "number") return numberTimelineCueId ?? resolveClipEditorCueId(selectedCueId, editorCueId);
  return resolveClipEditorCueId(selectedCueId, editorCueId);
}

/** Migrate the compatibility mirror from projects written before the two-panel dock. */
export function normalizeClipEditorVisibility(
  prefs: Partial<ClipEditorVisibility> | null | undefined,
): Required<ClipEditorVisibility> {
  const showLive = prefs?.show_live_panel ?? true;
  const showSlice = prefs?.show_slice_panel ?? true;
  const requested = prefs?.clip_editor_active_tab;
  const active = requested === "Slice" && showSlice
    ? "Slice"
    : requested === "Live" && showLive
      ? "Live"
      : showLive
        ? "Live"
        : "Slice";
  return { show_live_panel: showLive, show_slice_panel: showSlice, clip_editor_active_tab: active };
}

/** Apply a View-menu toggle and repair the active tab if its panel disappears. */
export function toggleClipEditorPanel(
  state: Required<ClipEditorVisibility>,
  panel: ClipEditorTab,
): Required<ClipEditorVisibility> {
  const next = {
    ...state,
    show_live_panel: panel === "Live" ? !state.show_live_panel : state.show_live_panel,
    show_slice_panel: panel === "Slice" ? !state.show_slice_panel : state.show_slice_panel,
  };
  if (next.clip_editor_active_tab === "Live" && !next.show_live_panel && next.show_slice_panel) next.clip_editor_active_tab = "Slice";
  if (next.clip_editor_active_tab === "Slice" && !next.show_slice_panel && next.show_live_panel) next.clip_editor_active_tab = "Live";
  return next;
}
