// Generic trim strip: a canvas with draggable start (blue) / end (orange)
// markers over a caller-painted background. The audio waveform and the video
// filmstrip trimmers share this shell so they look and behave identically.

import { useEffect, useRef, useState } from "react";
import { useLocale } from "../../i18n";
import { beginCanvasPointer, cancelCanvasPointer, endCanvasPointer, ownsCanvasPointer } from "../Editor/sliceTimelineInteraction";

export const TRIM_START_COLOR = "#60a5fa";
export const TRIM_END_COLOR = "#fb923c";
export const SLICE_MARKER_COLOR = "#facc15";
export const TRIM_HANDLE_ZONE_HEIGHT = 12;
export const TRIM_HANDLE_WIDTH = 16;
export const TRIM_HANDLE_HIT_RADIUS = 12;
export const TRIM_HANDLE_HIT_MARGIN = 4;

export function trimHandleBaseBounds(x: number, canvasWidth: number): { left: number; right: number } {
  if (canvasWidth <= TRIM_HANDLE_WIDTH) return { left: 0, right: Math.max(0, canvasWidth) };
  const left = Math.max(0, Math.min(canvasWidth - TRIM_HANDLE_WIDTH, x - TRIM_HANDLE_WIDTH / 2));
  return { left, right: left + TRIM_HANDLE_WIDTH };
}

/** Horizontal distance to the visible handle cap; Infinity means it is off-screen. */
export function trimHandleHitDistance(pointerX: number, markerX: number, canvasWidth: number): number {
  if (markerX < 0 || markerX > canvasWidth) return Number.POSITIVE_INFINITY;
  const { left, right } = trimHandleBaseBounds(markerX, canvasWidth);
  if (pointerX < left) return left - pointerX;
  if (pointerX > right) return pointerX - right;
  return 0;
}

/** Draw one trim stem and a solid handle above the media strip. */
export function drawTrimMarker(
  ctx: CanvasRenderingContext2D,
  x: number,
  canvasWidth: number,
  zoneTop: number,
  mediaTop: number,
  bottom: number,
  color: string,
) {
  if (x < 0 || x > canvasWidth) return;
  const { left: baseLeft, right: baseRight } = trimHandleBaseBounds(x, canvasWidth);
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.moveTo(x, mediaTop);
  ctx.lineTo(x, bottom);
  ctx.stroke();
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.moveTo(baseLeft, zoneTop);
  ctx.lineTo(baseRight, zoneTop);
  ctx.lineTo(x, mediaTop);
  ctx.closePath();
  ctx.fill();
  ctx.restore();
}

/** Thin dashed vertical line marking a slice boundary. */
export function drawSliceLine(
  ctx: CanvasRenderingContext2D,
  x: number,
  height: number,
) {
  ctx.strokeStyle = SLICE_MARKER_COLOR;
  ctx.lineWidth = 1.5;
  ctx.setLineDash([5, 3]);
  ctx.beginPath();
  ctx.moveTo(x, 0);
  ctx.lineTo(x, height);
  ctx.stroke();
  ctx.setLineDash([]);
}

/** Repaint trigger that follows the element's on-screen width (panel resize,
 *  window resize) — without it the canvas bitmap gets scaled and squashed. */
export function useCanvasWidth(ref: React.RefObject<HTMLCanvasElement>): number {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const observer = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width ?? 0;
      setWidth(Math.round(w));
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}

/** Visible time window of a timeline (for zoomed views). */
export interface TrimView {
  startMs: number;
  endMs: number;
}

/** Paints the strip background in CSS-pixel coordinates. `view` is the
 *  visible time window — the full clip unless the caller supports zoom. */
export type TrimPainter = (
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  startX: number,
  endX: number,
  view: TrimView,
) => void;

export function TrimStrip({
  durationMs,
  startMs,
  endMs,
  height,
  paint,
  paintKey,
  resetKey,
  onCommitStart,
  onCommitEnd,
  centerLabel,
  onExpand,
  dragPreview,
  sliceMarkersMs,
}: {
  durationMs: number;
  /** Committed start marker (ms); null = file start. */
  startMs: number | null;
  /** Committed end marker (ms); null = file end. */
  endMs: number | null;
  height: number;
  paint: TrimPainter;
  /** Changes when the background data changes (triggers a repaint). */
  paintKey: unknown;
  /** Stable cue identity. Background detail swaps do not cancel an active drag. */
  resetKey?: unknown;
  /** Called on drag release; `null` = back to the file boundary. */
  onCommitStart: (ms: number | null) => void;
  onCommitEnd: (ms: number | null) => void;
  centerLabel?: string;
  onExpand?: () => void;
  /** Rendered in a popup above the cursor while a marker is dragged
   *  (e.g. the video frame under the cursor). */
  dragPreview?: (ms: number) => React.ReactNode;
  /** Slice boundaries (ms) drawn as read-only dashed yellow lines —
   *  edited in the clip editor dock, shown here for context. */
  sliceMarkersMs?: number[];
}) {
  const { t } = useLocale();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const activePointerId = useRef<number | null>(null);
  const canvasWidth = useCanvasWidth(canvasRef);
  const [dragging, setDragging] = useState<"start" | "end" | null>(null);
  const [localStartMs, setLocalStartMs] = useState<number | null>(null);
  const [localEndMs, setLocalEndMs] = useState<number | null>(null);
  /** Cursor position during a drag, relative to the canvas (CSS px). */
  const [cursor, setCursor] = useState<{ x: number; width: number } | null>(null);

  useEffect(() => {
    setLocalStartMs(null);
    setLocalEndMs(null);
    cancelActivePointer();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resetKey]);

  const effStartMs = localStartMs ?? startMs ?? 0;
  const effEndMs = localEndMs ?? endMs ?? durationMs;

  // Repaint: background (delegated) then the trim markers, at devicePixelRatio
  // so single-pixel detail stays crisp on scaled displays.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const W = rect.width || 380;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(W * dpr);
    canvas.height = Math.round(height * dpr);
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    const mediaHeight = Math.max(1, height - TRIM_HANDLE_ZONE_HEIGHT);
    const startX = durationMs > 0 ? (effStartMs / durationMs) * W : 0;
    const endX = durationMs > 0 ? (effEndMs / durationMs) * W : W;
    ctx.save();
    ctx.translate(0, TRIM_HANDLE_ZONE_HEIGHT);
    paint(ctx, W, mediaHeight, startX, endX, { startMs: 0, endMs: durationMs });
    for (const m of sliceMarkersMs ?? []) {
      if (durationMs > 0 && m > 0 && m < durationMs) {
        drawSliceLine(ctx, (m / durationMs) * W, mediaHeight);
      }
    }
    ctx.restore();
    drawTrimMarker(ctx, startX, W, 0, TRIM_HANDLE_ZONE_HEIGHT, height, TRIM_START_COLOR);
    drawTrimMarker(ctx, endX, W, 0, TRIM_HANDLE_ZONE_HEIGHT, height, TRIM_END_COLOR);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paint, paintKey, effStartMs, effEndMs, durationMs, height, canvasWidth,
      (sliceMarkersMs ?? []).join(",")]);

  const xToMs = (clientX: number): number => {
    const canvas = canvasRef.current;
    if (!canvas || durationMs === 0) return 0;
    const rect = canvas.getBoundingClientRect();
    const relX = Math.max(0, Math.min(clientX - rect.left, rect.width));
    return (relX / rect.width) * durationMs;
  };

  const cancelActivePointer = () => {
    const pointerId = activePointerId.current;
    const hadPointer = cancelCanvasPointer(activePointerId);
    const canvas = canvasRef.current;
    if (hadPointer && pointerId !== null && canvas?.hasPointerCapture(pointerId)) {
      canvas.releasePointerCapture(pointerId);
    }
    setDragging(null);
    setCursor(null);
    setLocalStartMs(null);
    setLocalEndMs(null);
  };

  const handlePointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (durationMs === 0 || e.button !== 0 || !e.isPrimary || activePointerId.current !== null || dragging) return;
    const rect = canvasRef.current!.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const W = rect.width;
    const sX = (effStartMs / durationMs) * W;
    const eX = (effEndMs / durationMs) * W;
    const dStart = trimHandleHitDistance(x, sX, W);
    const dEnd = trimHandleHitDistance(x, eX, W);
    const startHit = dStart <= TRIM_HANDLE_HIT_MARGIN;
    const endHit = dEnd <= TRIM_HANDLE_HIT_MARGIN;
    const chooseStart = startHit && (!endHit || Math.abs(x - sX) <= Math.abs(x - eX));
    if (chooseStart) {
      e.preventDefault();
      beginCanvasPointer(activePointerId, e.pointerId);
      e.currentTarget.setPointerCapture(e.pointerId);
      setDragging("start");
      setCursor({ x: sX, width: W });
    } else if (endHit) {
      e.preventDefault();
      beginCanvasPointer(activePointerId, e.pointerId);
      e.currentTarget.setPointerCapture(e.pointerId);
      setDragging("end");
      setCursor({ x: eX, width: W });
    }
  };

  const handlePointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!dragging || !ownsCanvasPointer(activePointerId, e.pointerId)) return;
    const ms = xToMs(e.clientX);
    const rect = canvasRef.current!.getBoundingClientRect();
    setCursor({
      x: Math.max(0, Math.min(e.clientX - rect.left, rect.width)),
      width: rect.width,
    });
    if (dragging === "start") {
      setLocalStartMs(Math.max(0, Math.min(ms, effEndMs - 50)));
    } else {
      setLocalEndMs(Math.min(durationMs, Math.max(ms, effStartMs + 50)));
    }
  };

  const handlePointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!endCanvasPointer(activePointerId, e.pointerId)) return;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    setCursor(null);
    if (!dragging) return;
    if (dragging === "start" && localStartMs !== null) {
      const ms = Math.round(localStartMs);
      onCommitStart(ms <= 0 ? null : ms);
    } else if (dragging === "end" && localEndMs !== null) {
      const ms = Math.round(localEndMs);
      onCommitEnd(ms >= durationMs ? null : ms);
    }
    setDragging(null);
  };

  const handlePointerCancel = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!endCanvasPointer(activePointerId, e.pointerId)) return;
    setDragging(null);
    setCursor(null);
    setLocalStartMs(null);
    setLocalEndMs(null);
  };

  const draggedMs = dragging === "start" ? effStartMs : effEndMs;

  const fmtS = (ms: number) => (ms / 1000).toFixed(3);

  return (
    <div style={{ marginBottom: 16, position: "relative" }}>
      {/* Scrub preview above the cursor while dragging a marker. */}
      {dragging && cursor && dragPreview && (
        <div
          style={{
            position: "absolute",
            // The canvas sits below the ~20px label row; anchor the popup's
            // bottom just above it. Clamped so it never overflows the strip.
            left: Math.max(105, Math.min(cursor.x, cursor.width - 105)),
            top: 16,
            transform: "translate(-50%, -100%)",
            zIndex: 10,
            pointerEvents: "none",
          }}
        >
          {dragPreview(draggedMs)}
        </div>
      )}
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          fontSize: 11,
          marginBottom: 4,
          color: "var(--wc-text-secondary)",
        }}
      >
        <span style={{ color: TRIM_START_COLOR }}>▶ {fmtS(effStartMs)}s</span>
        <span>{centerLabel ?? (durationMs > 0 ? `${(durationMs / 1000).toFixed(2)}s` : "—")}</span>
        <span style={{ color: TRIM_END_COLOR }}>■ {fmtS(effEndMs)}s</span>
        {onExpand && (
          <button
            onClick={onExpand}
            title={t("sweepUi.openWaveform")}
            style={{
              background: "var(--wc-bg-surface)",
              border: "1px solid var(--wc-border-strong)",
              borderRadius: 3,
              color: "var(--wc-text-secondary)",
              cursor: "pointer",
              fontSize: 11,
              padding: "1px 5px",
              lineHeight: 1.4,
            }}
          >
            ⤢
          </button>
        )}
      </div>
      <canvas
        ref={canvasRef}
        style={{
          width: "100%",
          height,
          display: "block",
          borderRadius: 4,
          cursor: dragging ? "ew-resize" : "default",
          touchAction: "none",
        }}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={handlePointerCancel}
        onLostPointerCapture={handlePointerCancel}
      />
      <div
        style={{
          fontSize: 10,
          color: "var(--wc-text-faint)",
          marginTop: 3,
          textAlign: "center",
        }}
      >
        Drag blue (start) or orange (end) marker to trim
      </div>
    </div>
  );
}
