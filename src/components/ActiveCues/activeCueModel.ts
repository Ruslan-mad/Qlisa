import type { CueSummary } from "../../lib/types";

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

export function cueProgressPercent(elapsedMs: number, durationMs: number | null | undefined): number | null {
  if (durationMs == null || !Number.isFinite(durationMs) || durationMs <= 0) return null;
  return Math.max(0, Math.min(100, (Math.max(0, elapsedMs) / durationMs) * 100));
}
