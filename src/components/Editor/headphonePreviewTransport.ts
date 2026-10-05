/** Resolve the editor cursor used when a headphone audition starts. */
export function headphonePreviewStartPosition(
  isNumber: boolean,
  numberPositionMs: number,
  previewPositionMs: number | null,
  runtimePositionMs: number | null,
  trimStartMs: number | null | undefined,
): number {
  if (isNumber) return Math.max(0, numberPositionMs);
  return Math.max(0, previewPositionMs ?? runtimePositionMs ?? trimStartMs ?? 0);
}

export function editorPreviewCursorPosition(
  isVideo: boolean,
  videoPositionMs: number | null,
  fallbackPositionMs: number | null,
): number | null {
  return isVideo ? videoPositionMs ?? fallbackPositionMs : fallbackPositionMs;
}

/** Mirror a visual play/pause edge to the active headphone transport. */
export function mirrorHeadphonePlayback(
  currentlyPlaying: boolean,
  hasHeadphoneSession: boolean,
  send: (action: "pause" | "resume") => unknown,
): boolean {
  if (!hasHeadphoneSession) return false;
  send(currentlyPlaying ? "pause" : "resume");
  return true;
}

/** Send one explicit seek edge. Pointer-move updates do not call this helper. */
export function seekHeadphonePreview(
  hasHeadphoneSession: boolean,
  positionMs: number,
  send: (positionMs: number, pauseBeforeSeek: boolean) => unknown,
  pauseBeforeSeek = false,
): boolean {
  if (!hasHeadphoneSession) return false;
  send(Math.max(0, Math.round(positionMs)), pauseBeforeSeek);
  return true;
}

/** Run a frame-step state transition, then send the resulting paused cursor. */
export function stepPreviewAndSyncHeadphones(
  step: () => number,
  onSeek?: (positionMs: number, pauseBeforeSeek: boolean) => void,
): number {
  const positionMs = step();
  onSeek?.(positionMs, true);
  return positionMs;
}
