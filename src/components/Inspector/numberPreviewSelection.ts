import type { CueSummary, NumberCueData } from "../../lib/types";
import { numberPreviewItem } from "../Editor/numberTimelineModel";

/**
 * Resolve the visual cue represented by a Number at one shared preview-clock
 * position. `null` means that the Number has no visual layer at that moment;
 * callers must not confuse that with an explicit request to clear selection.
 */
export function resolveNumberPreviewCue(cue: NumberCueData, positionMs: number): CueSummary | null {
  const item = numberPreviewItem(cue, positionMs);
  return item && (item.cue_type === "video" || item.cue_type === "image") && item.file_path
    ? item
    : null;
}

/**
 * Resolve monitor selection semantics for both ordinary cues and Numbers.
 * `undefined` deliberately means preserve the current monitor selection.
 */
export function resolveMonitorPreviewSelection(
  cue: CueSummary | null,
  number: NumberCueData | null,
  positionMs: number,
): { cueId: string } | null | undefined {
  if (!cue) return null;
  if (cue.cue_type === "video" || cue.cue_type === "image") {
    return { cueId: cue.id };
  }
  if (cue.cue_type === "number") {
    const visual = number?.id === cue.id ? resolveNumberPreviewCue(number, positionMs) : null;
    return visual ? { cueId: visual.id } : undefined;
  }
  return null;
}
