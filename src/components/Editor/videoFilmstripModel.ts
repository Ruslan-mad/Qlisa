/** A strip whose frames cover an explicit interval in source-file time. */
export interface VideoFilmstripWindow {
  sourceStartMs: number;
  sourceEndMs: number;
  tiles: HTMLImageElement[];
}

export interface VideoFilmstripRequest {
  startMs: number;
  endMs: number;
  tiles: number;
}

/** Pick the most detailed source-window strip containing a timestamp. */
export function filmstripFrameAtSourceTime(
  windows: readonly VideoFilmstripWindow[],
  sourceMs: number,
): HTMLImageElement | null {
  let frame: HTMLImageElement | null = null;
  let bestStep = Number.POSITIVE_INFINITY;
  for (const window of windows) {
    if (window.tiles.length === 0 || sourceMs < window.sourceStartMs || sourceMs > window.sourceEndMs) continue;
    const span = window.sourceEndMs - window.sourceStartMs;
    if (span <= 0) continue;
    const step = span / window.tiles.length;
    if (step >= bestStep) continue;
    const index = Math.min(window.tiles.length - 1, Math.floor(((sourceMs - window.sourceStartMs) / span) * window.tiles.length));
    frame = window.tiles[index] ?? frame;
    bestStep = step;
  }
  return frame;
}

const clamp = (value: number, min: number, max: number) => Math.max(min, Math.min(max, value));

/**
 * Build a bounded, cache-friendly request for a zoomed filmstrip. The window
 * stays close to the visible interval, while its frame count follows the
 * canvas width instead of remaining fixed at twelve.
 */
export function videoFilmstripRequest(
  visibleStartMs: number,
  visibleEndMs: number,
  fileDurationMs: number,
  viewportWidth: number,
): VideoFilmstripRequest | null {
  if (!Number.isFinite(fileDurationMs) || fileDurationMs <= 0) return null;
  const start = clamp(Math.min(visibleStartMs, visibleEndMs), 0, fileDurationMs);
  const end = clamp(Math.max(visibleStartMs, visibleEndMs), 0, fileDurationMs);
  if (end <= start) return null;

  // Avoid 500 ms cache cells at deep zoom. Coarser rounding at wide views
  // still lets small pans reuse the same cached window.
  const span = end - start;
  const quantumMs = clamp(span / 8, 20, 250);
  const requestStart = Math.max(0, Math.floor(start / quantumMs) * quantumMs);
  const requestEnd = Math.min(fileDurationMs, Math.ceil(end / quantumMs) * quantumMs);
  if (requestEnd <= requestStart) return null;

  const width = Math.max(1, Number.isFinite(viewportWidth) ? viewportWidth : 1);
  const tiles = clamp(Math.ceil(width / 72), 4, 24);
  return { startMs: requestStart, endMs: requestEnd, tiles };
}

/** Map a source-time coordinate into one rendered timeline pass. */
export function sourceMsToPassTimelineMs(
  sourceMs: number,
  passStartMs: number,
  passEndMs: number,
  passSourceStartMs: number,
  passSourceEndMs: number,
): number {
  const sourceSpan = Math.max(1, passSourceEndMs - passSourceStartMs);
  const ratio = clamp((sourceMs - passSourceStartMs) / sourceSpan, 0, 1);
  return passStartMs + ratio * Math.max(0, passEndMs - passStartMs);
}

/** Resolve the source-time intersection of a timeline pass and visible range. */
export function visiblePassSourceWindow(
  visibleStartMs: number,
  visibleEndMs: number,
  passStartMs: number,
  passEndMs: number,
  passSourceStartMs: number,
  rate = 1,
): { startMs: number; endMs: number } | null {
  const start = Math.max(visibleStartMs, passStartMs);
  const end = Math.min(visibleEndMs, passEndMs);
  const safeRate = Number.isFinite(rate) && rate > 0 ? rate : 1;
  if (end <= start) return null;
  return {
    startMs: passSourceStartMs + (start - passStartMs) * safeRate,
    endMs: passSourceStartMs + (end - passStartMs) * safeRate,
  };
}

export interface FilmstripPassGeometry {
  timelineStartMs: number;
  timelineEndMs: number;
  sourceStartMs: number;
  sourceEndMs: number;
  viewStartMs: number;
  viewEndMs: number;
  width: number;
  top: number;
  height: number;
  maxTileWidth?: number;
}

export interface FilmstripTimelinePass {
  id: string;
  startMs: number;
  endMs: number;
  sourceStartMs: number;
  sourceEndMs: number;
  rate?: number;
  looped?: boolean;
}

/** Resolve at most two source windows for the visible part of a Number track. */
export function visibleFilmstripSourceWindows(
  pass: FilmstripTimelinePass,
  viewStartMs: number,
  viewEndMs: number,
  maxWindows = 2,
): Array<{ sourceStartMs: number; sourceEndMs: number }> {
  const clipStart = Math.max(viewStartMs, pass.startMs);
  const clipEnd = Math.min(viewEndMs, pass.endMs);
  const sourceSpan = pass.sourceEndMs - pass.sourceStartMs;
  if (clipEnd <= clipStart || sourceSpan <= 0) return [];
  const rate = Number.isFinite(pass.rate) && (pass.rate ?? 0) > 0 ? pass.rate! : 1;
  if (!pass.looped) {
    const visible = visiblePassSourceWindow(clipStart, clipEnd, pass.startMs, pass.endMs, pass.sourceStartMs, rate);
    return visible ? [{
      sourceStartMs: clamp(visible.startMs, pass.sourceStartMs, pass.sourceEndMs),
      sourceEndMs: clamp(visible.endMs, pass.sourceStartMs, pass.sourceEndMs),
    }] : [];
  }

  const passDuration = sourceSpan / rate;
  if (!Number.isFinite(passDuration) || passDuration <= 0) return [];
  const firstPass = Math.max(0, Math.floor((clipStart - pass.startMs) / passDuration));
  const lastPass = Math.max(firstPass, Math.floor((Math.max(clipStart, clipEnd - 0.001) - pass.startMs) / passDuration));
  if (lastPass - firstPass + 1 > maxWindows) {
    return [{ sourceStartMs: pass.sourceStartMs, sourceEndMs: pass.sourceEndMs }];
  }

  const windows: Array<{ sourceStartMs: number; sourceEndMs: number }> = [];
  for (let index = firstPass; index <= lastPass; index++) {
    const passStart = pass.startMs + index * passDuration;
    const passEnd = Math.min(pass.endMs, passStart + passDuration);
    const visible = visiblePassSourceWindow(clipStart, clipEnd, passStart, passEnd, pass.sourceStartMs, rate);
    if (!visible) continue;
    const sourceStartMs = clamp(visible.startMs, pass.sourceStartMs, pass.sourceEndMs);
    const sourceEndMs = clamp(visible.endMs, pass.sourceStartMs, pass.sourceEndMs);
    if (sourceEndMs > sourceStartMs) windows.push({ sourceStartMs, sourceEndMs });
  }
  return windows;
}

/** Paint time-anchored strips without scaling a sparse frame across a huge cell. */
export function drawVideoFilmstripWindows(
  ctx: CanvasRenderingContext2D,
  windows: readonly VideoFilmstripWindow[],
  geometry: FilmstripPassGeometry,
): void {
  const sourceSpan = geometry.sourceEndMs - geometry.sourceStartMs;
  const timelineSpan = geometry.timelineEndMs - geometry.timelineStartMs;
  const viewSpan = geometry.viewEndMs - geometry.viewStartMs;
  if (sourceSpan <= 0 || timelineSpan <= 0 || viewSpan <= 0 || geometry.width <= 0 || geometry.height <= 0) return;
  const maxTileWidth = geometry.maxTileWidth ?? 160;
  const xAtSource = (sourceMs: number) => {
    const timelineMs = sourceMsToPassTimelineMs(
      sourceMs,
      geometry.timelineStartMs,
      geometry.timelineEndMs,
      geometry.sourceStartMs,
      geometry.sourceEndMs,
    );
    return ((timelineMs - geometry.viewStartMs) / viewSpan) * geometry.width;
  };

  // Base/source strips paint first. Detail windows then replace only the
  // source interval they actually cover.
  const byCoarseToFine = [...windows].sort((left, right) => {
    const leftStep = (left.sourceEndMs - left.sourceStartMs) / Math.max(1, left.tiles.length);
    const rightStep = (right.sourceEndMs - right.sourceStartMs) / Math.max(1, right.tiles.length);
    return rightStep - leftStep;
  });
  for (const window of byCoarseToFine) {
    if (!window.tiles.length || window.sourceEndMs <= window.sourceStartMs) continue;
    const tileSpan = (window.sourceEndMs - window.sourceStartMs) / window.tiles.length;
    for (let index = 0; index < window.tiles.length; index++) {
      const tileStart = window.sourceStartMs + index * tileSpan;
      const tileEnd = tileStart + tileSpan;
      const visibleSourceStart = Math.max(tileStart, geometry.sourceStartMs);
      const visibleSourceEnd = Math.min(tileEnd, geometry.sourceEndMs);
      if (visibleSourceEnd <= visibleSourceStart) continue;
      const rawX0 = xAtSource(visibleSourceStart);
      const rawX1 = xAtSource(visibleSourceEnd);
      if (rawX1 <= 0 || rawX0 >= geometry.width) continue;
      const cellX = Math.max(0, rawX0);
      const cellEnd = Math.min(geometry.width, rawX1);
      const cellWidth = cellEnd - cellX;
      if (cellWidth <= 0) continue;
      const image = window.tiles[index];
      const heightScale = geometry.height / image.height;
      const scale = Math.min(heightScale, maxTileWidth / image.width);
      const drawImageWidth = image.width * scale;
      const drawImageHeight = image.height * scale;
      ctx.save();
      ctx.beginPath();
      ctx.rect(cellX, geometry.top, cellWidth, geometry.height);
      ctx.clip();
      // Clear lower-resolution pixels for this source interval, then repeat
      // the frame at a stable physical width while denser frames load.
      ctx.fillStyle = "#000";
      ctx.fillRect(cellX, geometry.top, cellWidth, geometry.height);
      const repeatWidth = Math.max(1, drawImageWidth);
      if (repeatWidth >= cellWidth) {
        ctx.drawImage(image, cellX + (cellWidth - repeatWidth) / 2, geometry.top + (geometry.height - drawImageHeight) / 2, drawImageWidth, drawImageHeight);
      } else {
        for (let x = cellX; x < cellEnd; x += repeatWidth) {
          ctx.drawImage(image, x, geometry.top + (geometry.height - drawImageHeight) / 2, drawImageWidth, drawImageHeight);
        }
      }
      ctx.restore();
    }
  }
}
