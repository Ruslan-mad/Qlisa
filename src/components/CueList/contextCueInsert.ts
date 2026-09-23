import type { CueType } from "../../lib/types";

/** Cue kinds whose whole purpose is to act on one or more other cues. */
export const TARGETED_CUE_TYPES: ReadonlySet<CueType> = new Set([
  "stop",
  "fade",
  "devamp",
  "start",
  "pause",
  "resume",
  "load",
  "reset",
  "goto",
  "arm",
  "disarm",
]);

export function isTargetedCueType(cueType: CueType): boolean {
  return TARGETED_CUE_TYPES.has(cueType);
}

/** Whether a row-local targeted cue can meaningfully act on this cue type. */
export function canTargetCueType(cueType: CueType, targetType: CueType): boolean {
  if (cueType === "fade") {
    return ["audio", "video", "image", "camera", "group"].includes(targetType);
  }
  if (cueType === "devamp") {
    return ["audio", "video", "group"].includes(targetType);
  }
  return true;
}

export interface ContextCueInsert {
  position: number;
  targetCueId: string | null;
}

/**
 * Resolve an add-above/add-below action from a cue row's context menu.
 *
 * The clicked row is deliberately the target even when it is only one member
 * of a larger selection. Selection controls bulk-edit actions; it must not
 * silently turn a row-local "add control cue" action into a multi-target cue.
 */
export function resolveContextCueInsert(
  cueType: CueType,
  clickedCueId: string,
  topLevelCueIds: readonly string[],
  offset: 0 | 1,
): ContextCueInsert | null {
  const clickedIndex = topLevelCueIds.indexOf(clickedCueId);
  if (clickedIndex < 0) return null;
  return {
    position: clickedIndex + offset,
    targetCueId: isTargetedCueType(cueType) ? clickedCueId : null,
  };
}
