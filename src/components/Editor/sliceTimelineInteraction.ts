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

/** Track the one pointer that owns a canvas gesture. Clearing it before commit
 *  makes pointerup/lostcapture races harmless. */
export function beginCanvasPointer(activePointerId: { current: number | null }, pointerId: number): void {
  activePointerId.current = pointerId;
}

export function ownsCanvasPointer(activePointerId: { current: number | null }, pointerId: number): boolean {
  return activePointerId.current === pointerId;
}

export function endCanvasPointer(activePointerId: { current: number | null }, pointerId: number): boolean {
  if (!ownsCanvasPointer(activePointerId, pointerId)) return false;
  activePointerId.current = null;
  return true;
}

export function cancelCanvasPointer(activePointerId: { current: number | null }): boolean {
  const hadPointer = activePointerId.current !== null;
  activePointerId.current = null;
  return hadPointer;
}
