import type { CueSummary, OutputControlStatus, OutputDestination } from "../../lib/types";

export type OutputFtbVisualState = "ok" | "ftb" | "unavailable";

export function outputFtbVisualState(output: Pick<OutputControlStatus, "available" | "healthy" | "ftb">): OutputFtbVisualState {
  if (!output.available || !output.healthy) return "unavailable";
  return output.ftb ? "ftb" : "ok";
}

export function outputFtbLedColor(output: Pick<OutputControlStatus, "available" | "healthy" | "ftb">): string {
  switch (outputFtbVisualState(output)) {
    case "ftb": return "#ef4444";
    case "ok": return "#4ade80";
    default: return "#f59e0b";
  }
}

/**
 * Preferences order is the canonical operator order for every output list:
 * display screens first, then live/network outputs. Sorting is stable inside
 * both sections, so Settings remains the source of the order within a group.
 */
export function orderOutputDestinations<T extends Pick<OutputDestination, "sink_kind">>(
  destinations: readonly T[],
): T[] {
  return destinations
    .map((destination, index) => ({ destination, index }))
    .sort((a, b) => {
      const categoryA = a.destination.sink_kind === "display" ? 0 : 1;
      const categoryB = b.destination.sink_kind === "display" ? 0 : 1;
      return categoryA - categoryB || a.index - b.index;
    })
    .map(({ destination }) => destination);
}

/**
 * The runtime registry is keyed by id, so its status snapshot is not a
 * reliable presentation order. Preferences are the canonical operator order:
 * display screens first, then live/network outputs, preserving each section's
 * order from Settings.
 */
export function orderOutputStatuses<T extends Pick<OutputControlStatus, "output_id">>(
  statuses: readonly T[],
  destinations: readonly OutputDestination[],
): T[] {
  const destinationOrder = new Map(destinations.map((destination, index) => [destination.id, { index, kind: destination.sink_kind }]));
  return [...statuses].sort((a, b) => {
    const da = destinationOrder.get(a.output_id);
    const db = destinationOrder.get(b.output_id);
    const categoryA = da?.kind === "display" ? 0 : 1;
    const categoryB = db?.kind === "display" ? 0 : 1;
    if (categoryA !== categoryB) return categoryA - categoryB;
    if (da && db && da.index !== db.index) return da.index - db.index;
    if (da) return -1;
    if (db) return 1;
    return 0;
  });
}

const VISUAL_CUE_TYPES = new Set(["video", "image", "browser", "camera", "text"]);

/** One assignment bit per FTB output, in the already-canonical output order. */
export function cueOutputAssignments(
  cue: Pick<CueSummary, "cue_type" | "visual_output_ids">,
  outputs: readonly Pick<OutputControlStatus, "output_id">[],
  defaultOutputId: string | null | undefined,
): boolean[] {
  if (!VISUAL_CUE_TYPES.has(cue.cue_type)) return outputs.map(() => false);
  const explicit = cue.visual_output_ids ?? [];
  const assigned = new Set(explicit.length > 0 ? explicit : defaultOutputId ? [defaultOutputId] : []);
  return outputs.map((output) => assigned.has(output.output_id));
}
