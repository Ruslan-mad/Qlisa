import type { CueSummary, DisplayPreferences, PreviewPlayheadEvent } from "./types";

export type ClipEditorTab = "Preview" | "Live";
/** `Slice` is accepted only as a legacy panel value and maps to Preview. */
export type ClipEditorPanel = "Preview" | "Slice";
export const CLIP_EDITOR_TAB_LABELS = { preview: "Preview", live: "Live" } as const;
export type ClipEditorVisibility = Pick<DisplayPreferences, "show_live_panel" | "show_slice_panel"> & {
  clip_editor_active_tab?: "Live" | "Slice";
  clip_editor_active_tab_v2?: ClipEditorTab | "Slice" | null;
};
export type NormalizedClipEditorVisibility = {
  show_live_panel: boolean;
  /** Deprecated compatibility field; canonical state always writes false. */
  show_slice_panel: false;
  clip_editor_active_tab: "Live";
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
  number_master_id?: string | null;
  number_action_offsets_ms?: Record<string, number>;
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
}

export function createManualLiveMediaSelection(cueId: string): ManualLiveMediaSelection { return { cueId }; }

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
      if ((cue.cue_type === "number" && !waiting && (cue.state === "running" || cue.state === "paused"))
        || ((cue.cue_type === "audio" || cue.cue_type === "video" || cue.cue_type === "image")
        && !waiting && !!cue.file_path && !cue.is_loading
        && (cue.state === "running" || cue.state === "paused"))) {
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

/** Follow the newest running ordinary track; paused tracks remain manually selectable. */
export function resolveLiveMediaSelection(
  candidates: OrderedLiveMediaCandidate[],
  manualSelection: ManualLiveMediaSelection | null,
  pinnedNumberId: string | null = null,
): OrderedLiveMediaCandidate | null {
  if (manualSelection) {
    const selected = candidates.find((candidate) => candidate.id === manualSelection.cueId);
    if (selected) return selected;
  }
  const activeNumbers = candidates.filter((candidate) => candidate.cue_type === "number" && !candidate.isBackgroundScreensaver
    && (candidate.state === "running" || candidate.state === "paused"));
  const pinnedNumber = activeNumbers.find((candidate) => candidate.id === pinnedNumberId);
  if (pinnedNumber) return pinnedNumber;
  const latestActiveNumber = activeNumbers.reduce<OrderedLiveMediaCandidate | null>((latest, candidate) => {
    return !latest || candidate.startOrder > latest.startOrder ? candidate : latest;
  }, null);
  if (latestActiveNumber) return latestActiveNumber;
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

export function clipEditorDockVisible(showLivePanel: boolean, _legacyShowSlicePanel = false): boolean {
  return showLivePanel;
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

/** Resolve a Presentation parent to its current media child, then the next
 * unfinished child, without changing the selected cue. */
export function resolvePresentationPreviewCueId(presentation: CueSummary): string | null {
  if (presentation.cue_type !== "presentation") return null;
  const children = presentation.children ?? [];
  const media = children.filter((child) =>
    (child.cue_type === "image" || child.cue_type === "video") && !!child.file_path,
  );
  if (media.length === 0) return null;

  // GroupCue.active_child_id is the next child GO will fire. It is not the
  // child that is currently displayed, so runtime child states take priority.
  const running = media.filter((child) => child.state === "running");
  if (running.length > 0) return running[running.length - 1].id;

  // A child can be paused directly while its Presentation stays running. If
  // there is no running media, keep the last paused layer ahead of the next
  // child pointer. This also selects the overlaid video in AsSlide mode.
  const paused = media.filter((child) => child.state === "paused");
  if (paused.length > 0) return paused[paused.length - 1].id;

  const nextMedia = media.find((child) => child.id === presentation.active_child_id);
  return nextMedia?.id ?? media.find((child) => child.state !== "completed")?.id ?? media[0].id;
}

/** Migrate the compatibility mirror from projects written before the two-panel dock. */
export function normalizeClipEditorVisibility(
  prefs: Partial<ClipEditorVisibility> | null | undefined,
): NormalizedClipEditorVisibility {
  const legacySliceVisible = prefs?.show_slice_panel ?? true;
  const requestedV2 = prefs?.clip_editor_active_tab_v2;
  const legacyRequested = prefs?.clip_editor_active_tab;
  // Old projects had two independent panel flags. If Preview was hidden but
  // Slice was visible (or its old field was absent and defaulted visible),
  // keep the unified Preview reachable. A saved Slice tab also migrates to it.
  const showLive = (prefs?.show_live_panel ?? true)
    || legacySliceVisible
    || requestedV2 === "Slice"
    || legacyRequested === "Slice";
  const active: ClipEditorTab = requestedV2 === "Live"
    ? "Live"
    : "Preview";
  return {
    show_live_panel: showLive,
    show_slice_panel: false,
    // The legacy enum has no Preview value; `Live` was its old Preview state.
    clip_editor_active_tab: "Live",
    clip_editor_active_tab_v2: active,
  };
}

/** Apply a View-menu toggle and repair the active tab if its panel disappears. */
export function toggleClipEditorPanel(
  state: NormalizedClipEditorVisibility,
  _panel: ClipEditorPanel,
): NormalizedClipEditorVisibility {
  // The old Slice panel now shares Preview's visibility.
  const next = {
    ...state,
    show_live_panel: !state.show_live_panel,
    show_slice_panel: false as const,
  };
  if (next.show_live_panel && !state.show_live_panel) next.clip_editor_active_tab_v2 = "Preview";
  else if (!next.show_live_panel && next.clip_editor_active_tab_v2 === "Preview") next.clip_editor_active_tab_v2 = "Live";
  next.clip_editor_active_tab = "Live";
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
