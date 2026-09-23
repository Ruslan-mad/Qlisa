import type { BulkCueUpdate } from "../../lib/commands";
import type { ContinueMode } from "../../lib/types";

/** Resolve which cues a Continue-mode context-menu action should update. */
export function resolveContinueModeTargets(
  clickedId: string,
  selectedIds: readonly string[],
): string[] {
  if (!selectedIds.includes(clickedId)) return [clickedId];
  return [...new Set(selectedIds)];
}

/** Build the atomic bulk-update payload for a Continue-mode menu action. */
export function buildContinueModeUpdates(
  targetIds: readonly string[],
  mode: ContinueMode,
): BulkCueUpdate[] {
  return targetIds.map((cueId) => ({
    cueId,
    properties: { continue_mode: mode },
  }));
}
