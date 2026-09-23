export interface TimelineViewRange {
  startMs: number;
  endMs: number;
}

/** Zoom around a timeline coordinate while keeping the anchor under cursor. */
export function zoomTimelineView(
  durationMs: number,
  current: TimelineViewRange | null,
  factor: number,
  anchorMs: number,
): TimelineViewRange | null {
  const duration = Math.max(0, durationMs);
  if (duration <= 0 || !Number.isFinite(factor) || factor <= 0) return null;
  const oldStart = current?.startMs ?? 0;
  const oldEnd = current?.endMs ?? duration;
  const oldSpan = Math.max(1, Math.min(duration, oldEnd - oldStart));
  const anchor = Math.max(oldStart, Math.min(oldEnd, anchorMs));
  const nextSpan = Math.min(duration, oldSpan / factor);
  if (nextSpan >= duration - 0.5) return null;
  const ratio = (anchor - oldStart) / oldSpan;
  const start = Math.max(0, Math.min(duration - nextSpan, anchor - ratio * nextSpan));
  return { startMs: start, endMs: start + nextSpan };
}

export function panTimelineView(
  durationMs: number,
  current: TimelineViewRange,
  deltaMs: number,
): TimelineViewRange {
  const duration = Math.max(0, durationMs);
  const span = Math.max(1, Math.min(duration, current.endMs - current.startMs));
  const start = Math.max(0, Math.min(Math.max(0, duration - span), current.startMs + deltaMs));
  return { startMs: start, endMs: start + span };
}

/** Map a canvas pixel to source time without losing the off-screen media span. */
export function mediaSourceMsAtPixel(
  pixelX: number,
  width: number,
  mediaStartMs: number,
  mediaEndMs: number,
  viewStartMs: number,
  viewEndMs: number,
  sourceStartMs: number,
  sourceEndMs: number,
): number {
  const viewSpan = Math.max(1, viewEndMs - viewStartMs);
  const mediaSpan = Math.max(1, mediaEndMs - mediaStartMs);
  const timelineMs = viewStartMs + (Math.max(0, Math.min(width, pixelX)) / Math.max(1, width)) * viewSpan;
  const ratio = Math.max(0, Math.min(1, (timelineMs - mediaStartMs) / mediaSpan));
  return sourceStartMs + ratio * Math.max(0, sourceEndMs - sourceStartMs);
}
