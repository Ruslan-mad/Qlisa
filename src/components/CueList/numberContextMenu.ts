import type { CueSummary } from "../../lib/types";

// A Number can contain the same media actions as before, plus a Group.  A
// Group is useful as a sequential master (for example two audio cues that
// must run one after another), so keep the context menu in sync with the
// backend and Number inspector capabilities.
const NUMBER_ACTION_TYPES = new Set<CueSummary["cue_type"]>(["audio", "video", "image", "group"]);

/** Return true when every selected cue can be moved into a Number. */
export function canAddCuesToNumber(cues: readonly CueSummary[], cueIds: readonly string[]): boolean {
  return cueIds.length > 0 && cueIds.every((id) => {
    const cue = cues.find((candidate) => candidate.id === id);
    return cue !== undefined && NUMBER_ACTION_TYPES.has(cue.cue_type);
  });
}

/** Number targets are top-level cues; nested Numbers cannot be created by the UI. */
export function numberTargets(cues: readonly CueSummary[]): CueSummary[] {
  return cues.filter((cue) => cue.cue_type === "number");
}

/** Keep Number children in the same order as the cue list, independent of selection order. */
export function orderCueIdsByList(cues: readonly CueSummary[], cueIds: readonly string[]): string[] {
  const wanted = new Set(cueIds);
  return cues.filter((cue) => wanted.has(cue.id)).map((cue) => cue.id);
}

/** Find the top-level insertion point for the first selected cue in list order. */
export function rootInsertionIndexForCueIds(cues: readonly CueSummary[], cueIds: readonly string[]): number {
  const wanted = new Set(cueIds);
  const index = cues.findIndex((cue) => wanted.has(cue.id));
  return index < 0 ? -1 : index;
}
