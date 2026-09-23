import type { OutputDestination } from "../../lib/types";

/**
 * Merge monitor assignments received from the backend into an open draft.
 * Every other draft field stays untouched, including unsaved names, enabled
 * flags, routing and calibration edits.
 */
export function mergeOutputMonitorAssignments(
  draftOutputs: readonly OutputDestination[],
  latestOutputs: readonly OutputDestination[],
): OutputDestination[] {
  const latestById = new Map(latestOutputs.map((output) => [output.id, output.monitor]));
  return draftOutputs.map((output) => latestById.has(output.id)
    ? { ...output, monitor: latestById.get(output.id) ?? null }
    : output);
}
