import type { CueSummary, WorkspaceCueCatalogEntry, WorkspaceCueCatalogList } from "../../lib/types";
import { topLevelSelectedStopIds, type KeyboardCueNode } from "../CueList/cueListKeyboard";

export type RightPanelMode = "closed" | "inspector" | "active-cues";

export function toggleRightPanel(current: RightPanelMode, target: Exclude<RightPanelMode, "closed">): RightPanelMode {
  return current === target ? "closed" : target;
}

export function migrateRightPanelMode(value: unknown, legacyInspectorOpen: unknown): RightPanelMode {
  if (value === "closed" || value === "inspector" || value === "active-cues") return value;
  return legacyInspectorOpen === false ? "closed" : "inspector";
}

export function flattenActiveCues(cues: CueSummary[]): CueSummary[] {
  const result: CueSummary[] = [];
  for (const cue of cues) {
    if (cue.state === "running" || cue.state === "paused") result.push(cue);
    if (cue.children?.length) result.push(...flattenActiveCues(cue.children));
  }
  return result;
}

export type CatalogActiveCue = WorkspaceCueCatalogEntry & { listName: string };
export function flattenCatalogActiveCues(lists: WorkspaceCueCatalogList[]): CatalogActiveCue[] {
  const result: CatalogActiveCue[] = [];
  const visit = (cues: WorkspaceCueCatalogEntry[], listName: string) => {
    for (const cue of cues) {
      if (cue.state === "running" || cue.state === "paused") result.push({ ...cue, listName });
      if (cue.children?.length) visit(cue.children, listName);
    }
  };
  for (const list of lists) visit(list.cues, list.name);
  return result;
}

/** Active Group/Number cues summarize their descendants in the Active Cues view. */
export function getActiveCueDisplayRows(lists: WorkspaceCueCatalogList[]): CatalogActiveCue[] {
  const result: CatalogActiveCue[] = [];
  const visit = (cues: WorkspaceCueCatalogEntry[], listName: string) => {
    for (const cue of cues) {
      const active = cue.state === "running" || cue.state === "paused";
      if (active) result.push({ ...cue, listName });
      if (cue.children?.length && !(active && (cue.cue_type === "group" || cue.cue_type === "number"))) {
        visit(cue.children, listName);
      }
    }
  };
  for (const list of lists) visit(list.cues, list.name);
  return result;
}

/** Global Pause toggles every running cue, or resumes paused cues when none run. */
export function getCatalogPauseTargets(lists: WorkspaceCueCatalogList[]): CatalogActiveCue[] {
  const activeById = new Map(flattenCatalogActiveCues(lists).map((cue) => [cue.id, cue]));
  const active = [...activeById.values()];
  const running = active.filter((cue) => cue.state === "running");
  return running.length > 0 ? running : active.filter((cue) => cue.state === "paused");
}

/** Resolve the selected active cues in one list for the transport toggle. */
export function getSelectedPauseTargets(
  list: WorkspaceCueCatalogList | null | undefined,
  selectedIds: readonly string[],
): WorkspaceCueCatalogEntry[] {
  if (!list || selectedIds.length === 0) return [];
  const byId = new Map<string, WorkspaceCueCatalogEntry>();
  const collect = (cues: WorkspaceCueCatalogEntry[]) => {
    for (const cue of cues) {
      byId.set(cue.id, cue);
      if (cue.children) collect(cue.children);
    }
  };
  collect(list.cues);
  const selected = selectedIds.map((id) => byId.get(id)).filter(
    (cue): cue is WorkspaceCueCatalogEntry => cue != null && (cue.state === "running" || cue.state === "paused"),
  );
  const running = selected.filter((cue) => cue.state === "running");
  const targets = running.length > 0 ? running : selected.filter((cue) => cue.state === "paused");
  const tree: KeyboardCueNode[] = list.cues;
  const topLevelIds = new Set(topLevelSelectedStopIds(targets.map((cue) => cue.id), tree));
  return targets.filter((cue) => topLevelIds.has(cue.id));
}

export function cueProgressPercent(elapsedMs: number, durationMs: number | null | undefined): number | null {
  if (durationMs == null || !Number.isFinite(durationMs) || durationMs <= 0) return null;
  return Math.max(0, Math.min(100, (Math.max(0, elapsedMs) / durationMs) * 100));
}
