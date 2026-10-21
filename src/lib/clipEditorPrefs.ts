import type { DisplayPreferences, PreviewPlayheadEvent } from "./types";

export type ClipEditorTab = "Preview" | "Live" | "Slice";
export type ClipEditorPanel = "Preview" | "Slice";
export const CLIP_EDITOR_TAB_LABELS = { preview: "Preview", live: "Live", slice: "Slice" } as const;
export type ClipEditorVisibility = Pick<DisplayPreferences, "show_live_panel" | "show_slice_panel"> & {
  clip_editor_active_tab?: "Live" | "Slice";
  clip_editor_active_tab_v2?: ClipEditorTab | null;
};
export type NormalizedClipEditorVisibility = {
  show_live_panel: boolean;
  show_slice_panel: boolean;
  clip_editor_active_tab: "Live" | "Slice";
  clip_editor_active_tab_v2: ClipEditorTab;
};

export interface LiveMediaCandidate {
  id: string;
  cue_type: string;
  state: string;
  name?: string;
  number?: string | null;
  background_screensaver?: boolean;
  is_loading?: boolean;
  file_path?: string | null;
  duration_ms?: number | null;
  file_duration_ms?: number | null;
  cached_duration_ms?: number | null;
  media_source_revision?: string;
  children?: LiveMediaCandidate[];
}

export type OrderedLiveMediaCandidate<T extends LiveMediaCandidate = LiveMediaCandidate> = T & {
  startOrder: number;
};

/** Return active media leaves, excluding a background screensaver and its full subtree. */
export function activeLiveMediaCandidates<T extends LiveMediaCandidate>(
  cues: T[],
  startOrder: Record<string, number>,
  timings: Record<string, { wait_phase?: string | null }> = {},
): OrderedLiveMediaCandidate<T>[] {
  const result: OrderedLiveMediaCandidate<T>[] = [];
  const visit = (items: LiveMediaCandidate[], underScreensaver: boolean, underPreWait: boolean) => {
    for (const cue of items) {
      const excluded = underScreensaver || cue.background_screensaver === true;
      const waiting = underPreWait || timings[cue.id]?.wait_phase === "pre_wait";
      if (cue.children?.length) visit(cue.children, excluded, waiting);
      if (!excluded && (cue.cue_type === "audio" || cue.cue_type === "video" || cue.cue_type === "image")
        && !waiting && !!cue.file_path && !cue.is_loading
        && (cue.state === "running" || cue.state === "paused")) {
        // Runtime state is authoritative. Missing order data uses zero, then
        // the stable catalog traversal order resolves ties deterministically.
        const order = startOrder[cue.id];
        result.push({ ...cue, startOrder: Number.isFinite(order) ? order : 0 } as OrderedLiveMediaCandidate<T>);
      }
    }
  };
  visit(cues, false, false);
  return result.sort((a, b) => a.startOrder - b.startOrder);
}

/** Keep an operator's choice while it remains active; otherwise follow latest start. */
export function resolveLiveMediaSelection(
  candidates: OrderedLiveMediaCandidate[],
  manualCueId: string | null,
): OrderedLiveMediaCandidate | null {
  if (manualCueId) {
    const selected = candidates.find((candidate) => candidate.id === manualCueId);
    if (selected) return selected;
  }
  return candidates[candidates.length - 1] ?? null;
}

/** Program seek is allowed only from Live and only for active Audio/Video cues. */
export function canSeekProgramFromClipEditor(tab: ClipEditorTab, state: string | undefined, cueType: string | undefined): boolean {
  return tab === "Live"
    && (state === "running" || state === "paused")
    && (cueType === "audio" || cueType === "video");
}

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
): NormalizedClipEditorVisibility {
  const showLive = prefs?.show_live_panel ?? true;
  const showSlice = prefs?.show_slice_panel ?? true;
  const requestedV2 = prefs?.clip_editor_active_tab_v2;
  const legacyRequested = prefs?.clip_editor_active_tab;
  const active: ClipEditorTab = requestedV2 === "Live"
    ? "Live"
    : requestedV2 === "Slice" && showSlice
      ? "Slice"
      : requestedV2 === "Preview" && showLive
        ? "Preview"
        : legacyRequested === "Slice" && showSlice
          ? "Slice"
          : showLive
            ? "Preview"
            : "Live";
  return {
    show_live_panel: showLive,
    show_slice_panel: showSlice,
    clip_editor_active_tab: active === "Slice" ? "Slice" : "Live",
    clip_editor_active_tab_v2: active,
  };
}

/** Apply a View-menu toggle and repair the active tab if its panel disappears. */
export function toggleClipEditorPanel(
  state: NormalizedClipEditorVisibility,
  panel: ClipEditorPanel,
): NormalizedClipEditorVisibility {
  const next = {
    ...state,
    show_live_panel: panel === "Preview" ? !state.show_live_panel : state.show_live_panel,
    show_slice_panel: panel === "Slice" ? !state.show_slice_panel : state.show_slice_panel,
  };
  if (next.clip_editor_active_tab_v2 === "Preview" && !next.show_live_panel) next.clip_editor_active_tab_v2 = "Live";
  if (next.clip_editor_active_tab_v2 === "Slice" && !next.show_slice_panel) next.clip_editor_active_tab_v2 = next.show_live_panel ? "Preview" : "Live";
  next.clip_editor_active_tab = next.clip_editor_active_tab_v2 === "Slice" ? "Slice" : "Live";
  return next;
}
