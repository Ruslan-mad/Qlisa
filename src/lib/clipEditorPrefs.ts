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
  /** Effective value inherited from Number/Group ancestors. */
  isBackgroundScreensaver: boolean;
};

export interface ManualLiveMediaSelection {
  cueId: string;
  /** Latest ordinary running start already visible when the choice was made. */
  afterOrder: number;
}

export function createManualLiveMediaSelection(
  candidates: OrderedLiveMediaCandidate[],
  cueId: string,
): ManualLiveMediaSelection {
  const afterOrder = candidates.reduce((latest, candidate) =>
    !candidate.isBackgroundScreensaver && candidate.state === "running"
      ? Math.max(latest, candidate.startOrder)
      : latest, 0);
  return { cueId, afterOrder };
}

/** Return active media leaves, ordered with screensaver media first. */
export function activeLiveMediaCandidates<T extends LiveMediaCandidate>(
  cues: T[],
  startOrder: Record<string, number>,
  timings: Record<string, { wait_phase?: string | null }> = {},
): OrderedLiveMediaCandidate<T>[] {
  const result: OrderedLiveMediaCandidate<T>[] = [];
  const visit = (items: LiveMediaCandidate[], underScreensaver: boolean, underPreWait: boolean) => {
    for (const cue of items) {
      const isBackgroundScreensaver = underScreensaver || cue.background_screensaver === true;
      const waiting = underPreWait || timings[cue.id]?.wait_phase === "pre_wait";
      if (cue.children?.length) visit(cue.children, isBackgroundScreensaver, waiting);
      if ((cue.cue_type === "audio" || cue.cue_type === "video" || cue.cue_type === "image")
        && !waiting && !!cue.file_path && !cue.is_loading
        && (cue.state === "running" || cue.state === "paused")) {
        // Runtime state is authoritative. Missing order data uses zero, then
        // the stable catalog traversal order resolves ties deterministically.
        const order = startOrder[cue.id];
        result.push({ ...cue, startOrder: Number.isFinite(order) ? order : 0, isBackgroundScreensaver } as OrderedLiveMediaCandidate<T>);
      }
    }
  };
  visit(cues, false, false);
  return result.sort((a, b) => Number(b.isBackgroundScreensaver) - Number(a.isBackgroundScreensaver)
    || a.startOrder - b.startOrder);
}

/** A manual choice lasts until a newer ordinary cue is both started and running. */
export function shouldFollowLatestLiveMedia(
  candidates: OrderedLiveMediaCandidate[],
  manualSelection: ManualLiveMediaSelection | null,
): boolean {
  return !!manualSelection && candidates.some((candidate) =>
    !candidate.isBackgroundScreensaver
    && candidate.state === "running"
    && candidate.startOrder > manualSelection.afterOrder);
}

/** Follow the newest running ordinary track; paused tracks remain manually selectable. */
export function resolveLiveMediaSelection(
  candidates: OrderedLiveMediaCandidate[],
  manualSelection: ManualLiveMediaSelection | null,
): OrderedLiveMediaCandidate | null {
  if (manualSelection && !shouldFollowLatestLiveMedia(candidates, manualSelection)) {
    const selected = candidates.find((candidate) => candidate.id === manualSelection.cueId);
    if (selected) return selected;
  }
  const latestInState = (background: boolean, state: string) => candidates.reduce<OrderedLiveMediaCandidate | null>((latest, candidate) => {
    if (candidate.isBackgroundScreensaver !== background || candidate.state !== state) return latest;
    return !latest || candidate.startOrder > latest.startOrder ? candidate : latest;
  }, null);
  return latestInState(false, "running")
    ?? latestInState(true, "running")
    ?? latestInState(false, "paused")
    ?? latestInState(true, "paused");
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

export interface ClipEditorDockToggleResult {
  showDock: boolean;
  visibility: NormalizedClipEditorVisibility;
}

/** Toggle the dock container without changing its tab set or active tab. If the
 * menu disabled every tab, opening the dock restores Preview through the same
 * panel helper used by the View menu. */
export function toggleClipEditorDock(
  currentlyVisible: boolean,
  visibility: NormalizedClipEditorVisibility,
): ClipEditorDockToggleResult {
  if (currentlyVisible) return { showDock: false, visibility };
  const nextVisibility = clipEditorDockVisible(visibility.show_live_panel, visibility.show_slice_panel)
    ? visibility
    : toggleClipEditorPanel(visibility, "Preview");
  return { showDock: true, visibility: nextVisibility };
}