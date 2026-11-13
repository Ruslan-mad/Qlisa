export type SliceTimelinePointerAction = "edit" | "preview" | "ignore";

/** Slice/trim targets own their gesture; blank preview space can only seek the audition cursor. */
export function sliceTimelinePointerAction(
  editLocked: boolean,
  hasEditTarget: boolean,
  hasPreviewHandler: boolean,
): SliceTimelinePointerAction {
  if (editLocked) return "ignore";
  if (hasEditTarget) return "edit";
  return hasPreviewHandler ? "preview" : "ignore";
}

/** Do not commit a drag that began on a different cue than the current one. */
export function canCommitSliceTimelineGesture(startCueKey: unknown, currentCueKey: unknown): boolean {
  return Object.is(startCueKey, currentCueKey);
}
