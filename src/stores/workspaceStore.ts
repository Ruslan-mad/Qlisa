// Zustand store: workspace data, cue list, selection, and playhead.

import { create } from "zustand";
import type { CueId, CueListSummary, CueSummary, CueValidation, DisplayPreferences, GeneralPreferences, HealthAlert, WorkspaceCueCatalogEntry, WorkspaceCueCatalogList, WorkspaceInfo } from "../lib/types";
import { DEFAULT_DISPLAY_PREFS, DEFAULT_GENERAL_PREFS } from "../lib/types";
import { normalizeCueSelection } from "../lib/cueSelection";
import { commitIfCurrent, createCatalogRequestGuard } from "../lib/catalogRequestGuard";
import { checkWorkspace, getHealthAlerts, getPreferences, getWorkspaceInfo, getWorkspaceSnapshot } from "../lib/commands";

const catalogRequestGuard = createCatalogRequestGuard();
let cueStateRevision = 0;
let workspaceContextGeneration = 0;
let playheadRevision = 0;
const cueStateOverrides = new Map<CueId, { revision: number; state: CueSummary["state"] }>();

function mergeCatalogStateOverrides(
  lists: WorkspaceCueCatalogList[],
  sinceRevision: number,
): WorkspaceCueCatalogList[] {
  const apply = (cues: WorkspaceCueCatalogEntry[]): WorkspaceCueCatalogEntry[] => {
    let changed = false;
    const next = cues.map((cue) => {
      const override = cueStateOverrides.get(cue.id);
      const children = cue.children ? apply(cue.children) : cue.children;
      const state = override && override.revision > sinceRevision ? override.state : cue.state;
      if (state !== cue.state || children !== cue.children) {
        changed = true;
        return { ...cue, state, ...(children ? { children } : {}) };
      }
      return cue;
    });
    return changed ? next : cues;
  };
  return lists.map((list) => ({ ...list, cues: apply(list.cues) }));
}

function cueIdsIn(cues: CueSummary[]): Set<CueId> {
  const ids = new Set<CueId>();
  const visit = (items: CueSummary[]) => items.forEach((cue) => {
    ids.add(cue.id);
    if (cue.children) visit(cue.children);
  });
  visit(cues);
  return ids;
}

function mergeCueStateOverrides(cues: CueSummary[], sinceRevision: number): CueSummary[] {
  let changed = false;
  const next = cues.map((cue) => {
    const override = cueStateOverrides.get(cue.id);
    const children = cue.children ? mergeCueStateOverrides(cue.children, sinceRevision) : cue.children;
    const state = override && override.revision > sinceRevision ? override.state : cue.state;
    if (state !== cue.state || children !== cue.children) {
      changed = true;
      return { ...cue, state, ...(children ? { children } : {}) };
    }
    return cue;
  });
  return changed ? next : cues;
}

interface WorkspaceState {
  cues: CueSummary[];
  /** Read-only cue snapshots for every workspace list, including remote runtime state. */
  cueCatalog: WorkspaceCueCatalogList[];
  cueLists: CueListSummary[];
  activeCueListId: string | null;
  selectedCueId: CueId | null;
  /** All cues currently highlighted (multi-selection). Always includes selectedCueId when non-null. */
  selectedCueIds: CueId[];
  /** Advances on each selection action so delayed GO results cannot overwrite newer operator input. */
  cueSelectionRevision: number;
  /** Advances when workspace/list identity changes, so local range anchors reset. */
  workspaceContextRevision: number;
  /** Advances only when a workspace replacement is reconciled. */
  workspaceReplacementRevision: number;
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
  /** Order of the most recent cue-fired event for each cue in this workspace. */
  cueStartOrder: Record<CueId, number>;
  /** Monotonic sequence for runtime cue starts; reset on workspace replacement. */
  cueStartOrderCounter: number;
  /** Active runtime health alerts (device/network faults) shown in the banner. */
  healthAlerts: HealthAlert[];

  // Actions
  refreshCues: () => Promise<void>;
  refreshWorkspaceSnapshot: () => Promise<void>;
  refreshCueCatalog: () => Promise<void>;
  refreshValidation: () => Promise<void>;
  refreshHealth: () => Promise<void>;
  refreshCueLists: () => Promise<void>;
  setCueLists: (lists: CueListSummary[], activeId: string) => void;
  refreshWorkspaceInfo: () => Promise<void>;
  setSelectedCueId: (id: CueId | null) => void;
  setSelectedCueIds: (ids: CueId[]) => void;
  /** Atomically updates primary and multi-selection with invariant normalization. */
  setCueSelection: (primary: CueId | null, ids: CueId[]) => void;
  /** Applies a completed GO result only if no newer selection or list change occurred. */
  applyGoSelection: (cueListId: string, cueId: CueId | null, expectedRevision: number) => boolean;
  setPlayheadCueId: (id: CueId | null) => void;
  updateCueState: (cueId: CueId, state: CueSummary["state"]) => void;
  markCuePlayed: (cueId: CueId) => void;
  clearPlayedCueHistory: () => void;
  resetWorkspaceView: () => void;
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

function updateCatalogStateRecursive(cues: WorkspaceCueCatalogEntry[], cueId: CueId, state: CueSummary["state"]): WorkspaceCueCatalogEntry[] {
  let changed = false;
  const next = cues.map((cue) => {
    if (cue.id === cueId) { changed = true; return { ...cue, state }; }
    if (cue.children) {
      const children = updateCatalogStateRecursive(cue.children, cueId, state);
      if (children !== cue.children) { changed = true; return { ...cue, children }; }
    }
    return cue;
  });
  return changed ? next : cues;
}

export const useWorkspaceStore = create<WorkspaceState>((set, _get) => ({
  cues: [],
  cueCatalog: [],
  cueLists: [],
  activeCueListId: null,
  selectedCueId: null,
  selectedCueIds: [],
  cueSelectionRevision: 0,
  workspaceContextRevision: 0,
  workspaceReplacementRevision: 0,
  playheadCueId: null,
  workspaceInfo: null,
  validation: [],
  brokenCueIds: new Set<CueId>(),
  playedCueIds: new Set<CueId>(),
  cueStartOrder: {},
  cueStartOrderCounter: 0,
  healthAlerts: [],
  generalPrefs: DEFAULT_GENERAL_PREFS,
  displayPrefsLoaded: false,
  displayPrefs: { ...DEFAULT_DISPLAY_PREFS, output_screen: null, show_output_timer: false, timer_floating: false, timer_count_down: false, timer_font: "DSEG7 Classic", timer_font_size: 120, timer_position: "center" as const, timer_show_ms: false, timer_margin: 50, show_live_panel: true, show_slice_panel: true, clip_editor_active_tab: "Live" as const },

  refreshCues: async () => _get().refreshWorkspaceSnapshot(),

  refreshWorkspaceSnapshot: async () => {
    const requestGeneration = catalogRequestGuard.begin();
    const stateRevision = cueStateRevision;
    const requestedPlayheadRevision = playheadRevision;
    try {
      const snapshot = await getWorkspaceSnapshot();
      commitIfCurrent(catalogRequestGuard, requestGeneration, snapshot, (current) => {
        const previous = _get();
        const contextChanged = previous.activeCueListId !== current.active_cue_list_id;
        if (contextChanged) {
          workspaceContextGeneration += 1;
          playheadRevision += 1;
        }
        const validCueIds = cueIdsIn(current.cues);
        const selectedCueIds = contextChanged ? [] : previous.selectedCueIds.filter((id) => validCueIds.has(id));
        const selectedCueId = contextChanged
          ? null
          : previous.selectedCueId && validCueIds.has(previous.selectedCueId)
            ? previous.selectedCueId
            : selectedCueIds[selectedCueIds.length - 1] ?? null;
        const normalizedSelection = normalizeCueSelection(selectedCueId, selectedCueIds);
        set({
          cues: mergeCueStateOverrides(current.cues, stateRevision),
          playheadCueId: contextChanged || requestedPlayheadRevision === playheadRevision
            ? current.playhead_cue_id
            : previous.playheadCueId,
          cueLists: current.cue_lists,
          activeCueListId: current.active_cue_list_id,
          cueCatalog: mergeCatalogStateOverrides(current.cue_catalog, stateRevision),
          ...normalizedSelection,
          cueSelectionRevision: previous.cueSelectionRevision + (contextChanged || selectedCueId !== previous.selectedCueId || normalizedSelection.selectedCueIds.length !== previous.selectedCueIds.length ? 1 : 0),
          workspaceContextRevision: previous.workspaceContextRevision + (contextChanged ? 1 : 0),
        });
        cueStateOverrides.clear();
      });
    } catch (e) {
      console.error("Failed to refresh workspace snapshot:", e);
    }
  },

  refreshCueCatalog: async () => _get().refreshWorkspaceSnapshot(),

  refreshValidation: async () => {
    const requestGeneration = workspaceContextGeneration;
    try {
      const validation = await checkWorkspace();
      if (requestGeneration !== workspaceContextGeneration) return;
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

  refreshCueLists: async () => _get().refreshWorkspaceSnapshot(),

  setCueLists: (lists, activeId) => {
    catalogRequestGuard.invalidate();
    const changed = _get().activeCueListId !== activeId;
    if (changed) {
      workspaceContextGeneration += 1;
      playheadRevision += 1;
    }
    set((state) => {
      return {
        cueLists: lists,
        activeCueListId: activeId,
        ...(changed ? {
          cues: [],
          selectedCueId: null,
          selectedCueIds: [],
          playheadCueId: null,
          cueSelectionRevision: state.cueSelectionRevision + 1,
          workspaceContextRevision: state.workspaceContextRevision + 1,
        } : {}),
      };
    });
  },

  refreshWorkspaceInfo: async () => {
    const requestGeneration = workspaceContextGeneration;
    try {
      const info = await getWorkspaceInfo();
      if (requestGeneration !== workspaceContextGeneration) return;
      set({ workspaceInfo: info });
    } catch (e) {
      console.error("Failed to refresh workspace info:", e);
    }
  },

  setSelectedCueId: (id) => set((state) => ({
    ...normalizeCueSelection(id, id == null ? [] : [id]),
    cueSelectionRevision: state.cueSelectionRevision + 1,
  })),

  setSelectedCueIds: (ids) => set((state) => {
    const uniqueIds = [...new Set(ids)];
    const primary = state.selectedCueId != null && uniqueIds.includes(state.selectedCueId)
      ? state.selectedCueId
      : uniqueIds[uniqueIds.length - 1] ?? null;
    return {
      ...normalizeCueSelection(primary, uniqueIds),
      cueSelectionRevision: state.cueSelectionRevision + 1,
    };
  }),

  setCueSelection: (primary, ids) => set((state) => ({
    ...normalizeCueSelection(primary, ids),
    cueSelectionRevision: state.cueSelectionRevision + 1,
  })),

  applyGoSelection: (cueListId, cueId, expectedRevision) => {
    let applied = false;
    set((state) => {
      if (state.activeCueListId !== cueListId || state.cueSelectionRevision !== expectedRevision) return state;
      applied = true;
      playheadRevision += 1;
      return {
        ...normalizeCueSelection(cueId, cueId == null ? [] : [cueId]),
        playheadCueId: cueId,
        cueSelectionRevision: state.cueSelectionRevision + 1,
      };
    });
    return applied;
  },

  setPlayheadCueId: (id) => {
    playheadRevision += 1;
    set({ playheadCueId: id });
  },

  updateCueState: (cueId, state) => {
    cueStateRevision += 1;
    cueStateOverrides.set(cueId, { revision: cueStateRevision, state });
    set((prev) => ({
      cues: updateCueStateRecursive(prev.cues, cueId, state),
      cueCatalog: prev.cueCatalog.map((list) => {
        const cues = updateCatalogStateRecursive(list.cues, cueId, state);
        return cues === list.cues ? list : { ...list, cues };
      }),
    }));
  },

  markCuePlayed: (cueId) => set((prev) => {
    const cueStartOrderCounter = prev.cueStartOrderCounter + 1;
    return {
      playedCueIds: prev.playedCueIds.has(cueId) ? prev.playedCueIds : new Set([...prev.playedCueIds, cueId]),
      cueStartOrder: { ...prev.cueStartOrder, [cueId]: cueStartOrderCounter },
      cueStartOrderCounter,
    };
  }),

  clearPlayedCueHistory: () => set((state) => ({
    playedCueIds: new Set<CueId>(),
    cueStartOrder: {},
    cueStartOrderCounter: 0,
    cueSelectionRevision: state.cueSelectionRevision + 1,
  })),

  resetWorkspaceView: () => {
    workspaceContextGeneration += 1;
    playheadRevision += 1;
    cueStateRevision += 1;
    cueStateOverrides.clear();
    catalogRequestGuard.invalidate();
    set((state) => ({
      cues: [],
      cueCatalog: [],
      cueLists: [],
      activeCueListId: null,
      playheadCueId: null,
      workspaceInfo: null,
      validation: [],
      brokenCueIds: new Set<CueId>(),
      selectedCueId: null,
      selectedCueIds: [],
      workspaceContextRevision: state.workspaceContextRevision + 1,
      workspaceReplacementRevision: state.workspaceReplacementRevision + 1,
      playedCueIds: new Set<CueId>(),
      cueStartOrder: {},
      cueStartOrderCounter: 0,
      // Workspace replacement invalidates any GO response still in flight,
      // even when the replacement reuses the same cue-list ID.
      cueSelectionRevision: state.cueSelectionRevision + 1,
    }));
  },

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
