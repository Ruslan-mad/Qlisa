export type MediaPreviewKind = "image" | "video";

export const DEFAULT_PREVIEW_FRAME_RATE = 30;
export const MIN_PREVIEW_FRAME_RATE = 1;
export const MAX_PREVIEW_FRAME_RATE = 240;
export const PREVIEW_MODAL_OVERLAY_SELECTOR =
  '[role="dialog"], [aria-modal="true"], [style*="position: fixed"][style*="inset: 0"]';

export function resolveMediaPreviewKind(isVideo: boolean): MediaPreviewKind {
  return isVideo ? "video" : "image";
}

/** Keep Tauri-specific URL conversion outside the view model and injectable in tests. */
export function toMediaAssetUrl(
  path: string,
  converter: (path: string) => string,
): string | null {
  try {
    const url = converter(path);
    return url.trim() ? url : null;
  } catch {
    return null;
  }
}

export function shouldPlayMediaPreview({
  documentVisible,
  intersecting,
  failed,
}: {
  documentVisible: boolean;
  intersecting: boolean;
  failed: boolean;
}): boolean {
  return documentVisible && intersecting && !failed;
}

export function createMediaPreviewIdentity(cueId: string, path: string): string {
  return `${cueId}\u0000${path}`;
}

export function clampPreviewPosition(positionMs: number, durationMs: number): number {
  const upperBound = Number.isFinite(durationMs) && durationMs > 0 ? durationMs : Number.MAX_SAFE_INTEGER;
  return Math.max(0, Math.min(upperBound, Number.isFinite(positionMs) ? positionMs : 0));
}

/** Snap a frame step to a frame boundary; the browser video element remains the playback clock. */
export function stepPreviewFrame(
  positionMs: number,
  direction: -1 | 1,
  durationMs: number,
  frameRate = DEFAULT_PREVIEW_FRAME_RATE,
): number {
  const fps = Number.isFinite(frameRate) && frameRate > 0 ? frameRate : DEFAULT_PREVIEW_FRAME_RATE;
  const frameMs = 1000 / fps;
  const frame = Math.round(clampPreviewPosition(positionMs, durationMs) / frameMs) + direction;
  return clampPreviewPosition(Math.max(0, frame) * frameMs, durationMs);
}

export function shouldStepPreviewFrame(
  key: string,
  target: {
    tagName?: string;
    isContentEditable?: boolean;
    closest?: (selector: string) => unknown;
  },
  modifiers: { ctrlKey?: boolean; metaKey?: boolean; altKey?: boolean; shiftKey?: boolean },
): key is "ArrowLeft" | "ArrowRight" {
  if (key !== "ArrowLeft" && key !== "ArrowRight") return false;
  if (modifiers.ctrlKey || modifiers.metaKey || modifiers.altKey || modifiers.shiftKey) return false;
  const tagName = target.tagName?.toUpperCase();
  if (target.isContentEditable || tagName === "INPUT" || tagName === "TEXTAREA" || tagName === "SELECT") return false;
  return !target.closest?.(
    '[contenteditable]:not([contenteditable="false"]), [role="textbox"], [role="dialog"], [aria-modal="true"], [style*="position: fixed"][style*="inset: 0"]',
  );
}

/** Query the whole document so a modal blocks the shortcut even when focus stayed behind its overlay. */
export function hasBlockingPreviewOverlay(querySelector: (selector: string) => unknown): boolean {
  return Boolean(querySelector(PREVIEW_MODAL_OVERLAY_SELECTOR));
}

export function measurePreviewFrameRate(
  previous: { mediaTime: number; presentedFrames: number },
  current: { mediaTime: number; presentedFrames: number },
): number | null {
  const mediaDelta = current.mediaTime - previous.mediaTime;
  const frameDelta = current.presentedFrames - previous.presentedFrames;
  if (!Number.isFinite(mediaDelta) || mediaDelta <= 0 || !Number.isFinite(frameDelta) || frameDelta <= 0) return null;
  const frameRate = frameDelta / mediaDelta;
  return frameRate >= MIN_PREVIEW_FRAME_RATE && frameRate <= MAX_PREVIEW_FRAME_RATE ? frameRate : null;
}
