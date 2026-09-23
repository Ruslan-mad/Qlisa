// Zustand store: workspace data, cue list, selection, and playhead.

import { create } from "zustand";
import type { CueId, CueListSummary, CueSummary, CueValidation, DisplayPreferences, GeneralPreferences, HealthAlert, WorkspaceInfo } from "../lib/types";
import { DEFAULT_DISPLAY_PREFS, DEFAULT_GENERAL_PREFS } from "../lib/types";
import { normalizeCueSelection } from "../lib/cueSelection";
import { checkWorkspace, getAllCues, getCueLists, getHealthAlerts, getPlayhead, getPreferences, getWorkspaceInfo } from "../lib/commands";

interface WorkspaceState {
  cues: CueSummary[];
  cueLists: CueListSummary[];
  activeCueListId: string | null;
  selectedCueId: CueId | null;
  /** All cues currently highlighted (multi-selection). Always includes selectedCueId when non-null. */
  selectedCueIds: CueId[];
  playheadCueId: CueId | null;
  workspaceInfo: WorkspaceInfo | null;
  generalPrefs: GeneralPreferences;
  displayPrefs: DisplayPreferences;
  /** True after the machine-wide display preferences have been fetched. */
  displayPrefsLoaded: boolean;
  /** Latest preflight results (one entry per cue with problems). */
  validation: CueValidation[];
  /** IDs of cues with at least one error-severity problem (drives the row badge). */
  brokenCueIds: Set<CueId>;
  /** Cues fired during this app session; deliberately not persisted. */
  playedCueIds: Set<CueId>;
  /** Active runtime health alerts (device/network faults) shown in the banner. */
  healthAlerts: HealthAlert[];

  // Actions
  refreshCues: () => Promise<void>;
  refreshValidation: () => Promise<void>;
  refreshHealth: () => Promise<void>;
  refreshCueLists: () => Promise<void>;
  setCueLists: (lists: CueListSummary[], activeId: string) => void;
  refreshWorkspaceInfo: () => Promise<void>;
  setSelectedCueId: (id: CueId | null) => void;
  setSelectedCueIds: (ids: CueId[]) => void;
  /** Atomically updates primary and multi-selection with invariant normalization. */
  setCueSelection: (primary: CueId | null, ids: CueId[]) => void;
  setPlayheadCueId: (id: CueId | null) => void;
  updateCueState: (cueId: CueId, state: CueSummary["state"]) => void;
  markCuePlayed: (cueId: CueId) => void;
  clearPlayedCueHistory: () => void;
  loadGeneralPrefs: () => Promise<void>;
  setGeneralPrefs: (p: GeneralPreferences) => void;
  loadDisplayPrefs: () => Promise<void>;
  setDisplayPrefs: (p: DisplayPreferences) => void;
}

/** Update a cue row even when it is nested inside one or more Group cues. */
export function updateCueStateRecursive(
  cues: CueSummary[],
  cueId: CueId,
  state: CueSummary["state"],
): CueSummary[] {
  let changed = false;
  const next = cues.map((cue) => {
    if (cue.id === cueId) {
      changed = true;
      return { ...cue, state };
    }
    if (cue.children) {
      const children = updateCueStateRecursive(cue.children, cueId, state);
      if (children !== cue.children) {
        changed = true;
        return { ...cue, children };
      }
    }
    return cue;
  });
  return changed ? next : cues;
}

export const useWorkspaceStore = create<WorkspaceState>((set, _get) => ({
  cues: [],
  cueLists: [],
  activeCueListId: null,
  selectedCueId: null,
  selectedCueIds: [],
  playheadCueId: null,
  workspaceInfo: null,
  validation: [],
  brokenCueIds: new Set<CueId>(),
  playedCueIds: new Set<CueId>(),
  healthAlerts: [],
  generalPrefs: DEFAULT_GENERAL_PREFS,
  displayPrefsLoaded: false,
  displayPrefs: { ...DEFAULT_DISPLAY_PREFS, output_screen: null, show_output_timer: false, timer_floating: false, timer_count_down: false, timer_font: "DSEG7 Classic", timer_font_size: 120, timer_position: "center" as const, timer_show_ms: false, timer_margin: 50, theme: "system" as const, show_live_panel: true, show_slice_panel: true, clip_editor_active_tab: "Live" as const },

  refreshCues: async () => {
    try {
      const cues = await getAllCues();
      const playheadCueId = await getPlayhead();
      set({ cues, playheadCueId });
    } catch (e) {
      console.error("Failed to refresh cues:", e);
    }
  },

  refreshValidation: async () => {
    try {
      const validation = await checkWorkspace();
      const brokenCueIds = new Set<CueId>(
        validation
          .filter((v) => v.issues.some((i) => i.severity === "error"))
          .map((v) => v.cue_id),
      );
      set({ validation, brokenCueIds });
    } catch (e) {
      console.error("Failed to validate workspace:", e);
    }
  },

  refreshHealth: async () => {
    try {
      set({ healthAlerts: await getHealthAlerts() });
    } catch (e) {
      console.error("Failed to fetch health alerts:", e);
    }
  },

  refreshCueLists: async () => {
    try {
      const cueLists = await getCueLists();
      set((prev) => {
        // Keep the active ID only if it still exists in the new list.
        const validId = cueLists.some((cl) => cl.id === prev.activeCueListId)
          ? prev.activeCueListId
          : (cueLists[0]?.id ?? null);
        return { cueLists, activeCueListId: validId };
      });
    } catch (e) {
      console.error("Failed to refresh cue lists:", e);
    }
  },

  setCueLists: (lists, activeId) => set({ cueLists: lists, activeCueListId: activeId }),

  refreshWorkspaceInfo: async () => {
    try {
      const info = await getWorkspaceInfo();
      set({ workspaceInfo: info });
    } catch (e) {
      console.error("Failed to refresh workspace info:", e);
    }
  },

  setSelectedCueId: (id) => set(normalizeCueSelection(id, id == null ? [] : [id])),

  setSelectedCueIds: (ids) => set((state) => {
    const uniqueIds = [...new Set(ids)];
    const primary = state.selectedCueId != null && uniqueIds.includes(state.selectedCueId)
      ? state.selectedCueId
      : uniqueIds[uniqueIds.length - 1] ?? null;
    return normalizeCueSelection(primary, uniqueIds);
  }),

  setCueSelection: (primary, ids) => set(normalizeCueSelection(primary, ids)),

  setPlayheadCueId: (id) => set({ playheadCueId: id }),

  updateCueState: (cueId, state) => {
    set((prev) => ({ cues: updateCueStateRecursive(prev.cues, cueId, state) }));
  },

  markCuePlayed: (cueId) => set((prev) => {
    if (prev.playedCueIds.has(cueId)) return prev;
    return { playedCueIds: new Set([...prev.playedCueIds, cueId]) };
  }),

  clearPlayedCueHistory: () => set({ playedCueIds: new Set<CueId>() }),

  loadGeneralPrefs: async () => {
    try {
      const prefs = await getPreferences();
      set({ generalPrefs: { ...DEFAULT_GENERAL_PREFS, ...prefs.general } });
    } catch (e) {
      console.error("Failed to load general preferences:", e);
    }
  },

  setGeneralPrefs: (p) => set({ generalPrefs: p }),

  loadDisplayPrefs: async () => {
    try {
      const prefs = await getPreferences();
      const display = prefs.display;
      set({ displayPrefsLoaded: true, displayPrefs: { ...DEFAULT_DISPLAY_PREFS, show_live_panel: display.show_live_panel ?? true, show_slice_panel: display.show_slice_panel ?? true, clip_editor_active_tab: display.clip_editor_active_tab ?? "Live", ...display } });
    } catch (e) {
      console.error("Failed to load display preferences:", e);
    }
  },

  setDisplayPrefs: (p) => set({ displayPrefsLoaded: true, displayPrefs: p }),
}));
