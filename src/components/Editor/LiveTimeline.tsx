// Read-only, transport-facing media strip used by the Live tab.  This keeps
// the same canvas painter as SliceTimeline, but never writes trim or slices.

import { useEffect, useRef, useState } from "react";
import type { SliceList, WaveformData } from "../../lib/types";
import type { TrimPainter } from "../Inspector/TrimStrip";
import type { TrimView } from "../Inspector/TrimStrip";
import { useCanvasWidth } from "../Inspector/TrimStrip";
import { applyNumberDragSnap, numberDragRange } from "./numberTimelineModel";
import { mediaSourceMsAtPixel, panTimelineView, zoomTimelineView } from "./timelineViewModel";
import { useTimelineEditorStore } from "../../stores/timelineEditorStore";
import { useLocale } from "../../i18n";
import { formatDurationMs } from "../CueList/formatDuration";

const SLICE_COLOR = "#facc15";

export interface NumberTimelineTrack {
  id: string;
  name: string;
  startMs: number;
  endMs: number;
  master?: boolean;
  cueType?: "audio" | "video" | "image" | "group";
  media?: { waveform?: WaveformData | null; tiles?: HTMLImageElement[] | null };
  sourceStartMs?: number;
  sourceEndMs?: number;
  /** Maximum one-pass source span available for edge resizing. */
  maxDurationMs?: number;
  /** Repeat the source pass until endMs instead of stretching it. */
  looped?: boolean;
  /** Group headers and nested children are display-only until nested editing is wired. */
  readonly?: boolean;
  /** Child media blocks rendered inside one container row. */
  segments?: NumberTimelineTrack[];
}

function marker(ctx: CanvasRenderingContext2D, x: number, h: number, color: string, dashed = false) {
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = dashed ? 1.5 : 2;
  if (dashed) ctx.setLineDash([5, 3]);
  ctx.beginPath();
  ctx.moveTo(x, 0);
  ctx.lineTo(x, h);
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.fillStyle = color;
  ctx.beginPath();
  if (dashed) {
    ctx.moveTo(x - 5, h);
    ctx.lineTo(x + 5, h);
    ctx.lineTo(x, h - 8);
  } else {
    ctx.moveTo(x - 6, 0);
    ctx.lineTo(x + 6, 0);
    ctx.lineTo(x, 10);
  }
  ctx.closePath();
  ctx.fill();
  ctx.restore();
}

export interface LiveTimelineProps {
  durationMs: number;
  startMs: number | null;
  endMs: number | null;
  slices: SliceList;
  height: number;
  paint: TrimPainter;
  paintKey: unknown;
  /** Changes when the selected media file changes; detail swaps do not. */
  resetKey?: unknown;
  /** File-relative playhead. Null means there is no active timing sample. */
  mediaPositionMs: number | null;
  /** Optional file-relative preview playhead, rendered separately from Live. */
  previewPositionMs?: number | null;
  /** Called for a running/paused leaf click or drag in file coordinates. */
  onSeek?: (filePositionMs: number) => void;
  /** Standby clicks are preview-position changes only. */
  onPreviewPosition?: (filePositionMs: number) => void;
  onSeekEnd?: (filePositionMs: number) => void;
  onPreviewPositionEnd?: (filePositionMs: number) => void;
  seekEnabled?: boolean;
  onZoomDetail?: () => void;
  onViewChange?: (view: TrimView | null) => void;
  /** Optional multi-track Number overlay rendered in this same native canvas. */
  numberTracks?: NumberTimelineTrack[];
  /** Number-only timing bands. The track clock remains action-relative. */
  numberPreWaitMs?: number;
  numberPostWaitMs?: number;
  /** Total Number elapsed time while it is running or paused. */
  numberRuntimeElapsedMs?: number | null;
  onNumberTrackChange?: (track: NumberTimelineTrack, kind: "move" | "start" | "end", startMs: number, endMs: number) => void;
  onNumberTrackSnap?: (track: NumberTimelineTrack, kind: "move" | "start" | "end", startMs: number, endMs: number) => { startMs: number; endMs: number };
}

export function LiveTimeline({
  durationMs,
  startMs,
  endMs,
  slices,
  height,
  paint,
  paintKey,
  resetKey,
  mediaPositionMs,
  previewPositionMs = null,
  onSeek,
  onPreviewPosition,
  onSeekEnd,
  onPreviewPositionEnd,
  seekEnabled = false,
  onZoomDetail,
  onViewChange,
  numberTracks = [],
  numberPreWaitMs = 0,
  numberPostWaitMs = 0,
  numberRuntimeElapsedMs = null,
  onNumberTrackChange,
  onNumberTrackSnap,
}: LiveTimelineProps) {
  const { t } = useLocale();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const canvasWidth = useCanvasWidth(canvasRef);
  const [dragging, setDragging] = useState(false);
  const [view, setView] = useState<{ s: number; e: number } | null>(null);
  const [hoverMs, setHoverMs] = useState<number | null>(null);
  const [numberDrag, setNumberDrag] = useState<{
    track: NumberTimelineTrack;
    kind: "move" | "start" | "end";
    originStartMs: number;
    originEndMs: number;
    anchorMs: number;
    startMs: number;
    endMs: number;
    maxDurationMs?: number;
    moved: boolean;
  } | null>(null);
  const numberMode = numberTracks.length > 0;
  const editLocked = useTimelineEditorStore((state) => state.locked);
  const toggleEditLock = useTimelineEditorStore((state) => state.toggle);
  const numberTotalTimingMs = Math.max(1, numberPreWaitMs + durationMs + numberPostWaitMs);
  const previewLabel = numberMode ? "Number preview cursor" : "Headphone preview playhead";
  const [selectedNumberTrackId, setSelectedNumberTrackId] = useState<string | null>(null);
  const viewRef = useRef(view);
  const lastPositionRef = useRef(0);
  viewRef.current = view;

  // A lock can be enabled while a drag is already active. Invalidate the
  // transient drag so a later browser mouseup cannot commit that edit.
  useEffect(() => {
    if (!editLocked) return;
    setNumberDrag(null);
    setDragging(false);
  }, [editLocked]);

  const visible = view ?? { s: 0, e: durationMs };
  const span = Math.max(1, visible.e - visible.s);

  const xToMs = (clientX: number) => {
    const canvas = canvasRef.current;
    if (!canvas || durationMs <= 0) return 0;
    const rect = canvas.getBoundingClientRect();
    const x = Math.max(0, Math.min(rect.width, clientX - rect.left));
    return Math.max(0, Math.min(durationMs, visible.s + (x / Math.max(1, rect.width)) * span));
  };

  const zoom = (factor: number, anchor: number) => {
    if (durationMs <= 0) return;
    const next = zoomTimelineView(
      durationMs,
      view ? { startMs: view.s, endMs: view.e } : null,
      factor,
      anchor,
    );
    const nextView = next ? { s: next.startMs, e: next.endMs } : null;
    setView(nextView);
    onViewChange?.(nextView ? { startMs: nextView.s, endMs: nextView.e } : null);
    if (next && durationMs / Math.max(1, next.endMs - next.startMs) >= 2) onZoomDetail?.();
  };

  useEffect(() => {
    setView(null);
    onViewChange?.(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [durationMs, resetKey]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const x = Math.max(0, Math.min(rect.width, e.clientX - rect.left));
      const anchor = visible.s + (x / Math.max(1, rect.width)) * span;
      if (e.shiftKey && viewRef.current) {
        const delta = (e.deltaY > 0 ? 1 : -1) * span * 0.15;
        const nextRange = panTimelineView(durationMs, { startMs: visible.s, endMs: visible.e }, delta);
        const nextView = { s: nextRange.startMs, e: nextRange.endMs };
        setView(nextView);
        onViewChange?.({ startMs: nextView.s, endMs: nextView.e });
      } else {
        zoom(e.deltaY < 0 ? 1.35 : 1 / 1.35, anchor);
      }
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
  });

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || durationMs <= 0) return;
    const rect = canvas.getBoundingClientRect();
    const width = rect.width || 600;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const toX = (ms: number) => ((ms - visible.s) / span) * width;
    paint(ctx, width, height, toX(startMs ?? 0), toX(endMs ?? durationMs), { startMs: visible.s, endMs: visible.e });
    if (numberMode) {
      const rowH = height / numberTracks.length;
      const renderedTracks = numberTracks.map((track) => numberDrag?.track.id === track.id
        ? { ...track, startMs: numberDrag.startMs, endMs: numberDrag.endMs }
        : track);
      renderedTracks.forEach((track, index) => {
        const top = index * rowH;
        const x0 = toX(track.startMs);
        const x1 = toX(track.endMs);
        ctx.fillStyle = track.master ? "rgba(15,23,42,.42)" : "rgba(2,6,23,.58)";
        ctx.fillRect(0, top, width, rowH);
        const drawMedia = (
          media: NumberTimelineTrack["media"],
          mediaStart: number,
          mediaEnd: number,
          sourceStart: number,
          sourceEnd: number,
          visualTop: number,
          visualHeight: number,
          waveformTop: number,
          waveformHeight: number,
        ) => {
          const tiles = media?.tiles;
          if (tiles?.length && visualHeight > 1) {
            tiles.forEach((image, tile) => {
              const t0 = toX(mediaStart + tile * (mediaEnd - mediaStart) / tiles.length), t1 = toX(mediaStart + (tile + 1) * (mediaEnd - mediaStart) / tiles.length);
              if (t1 < 0 || t0 > width) return;
              const cellW = Math.max(1, t1 - t0), scale = Math.max(cellW / image.width, visualHeight / image.height);
              const dw = image.width * scale, dh = image.height * scale;
              ctx.save(); ctx.beginPath(); ctx.rect(t0, visualTop, cellW, visualHeight); ctx.clip();
              ctx.drawImage(image, t0 + (cellW - dw) / 2, visualTop + (visualHeight - dh) / 2, dw, dh); ctx.restore();
            });
            ctx.fillStyle = "rgba(0,0,0,.55)"; ctx.fillRect(0, visualTop, width, visualHeight);
          }
          const peaks = media?.waveform?.peaks ?? [];
          if (peaks.length && waveformHeight > 1) {
            const rawX0 = toX(mediaStart), rawX1 = toX(mediaEnd);
            const fromX = Math.max(0, rawX0), toXEnd = Math.min(width, rawX1);
            const mid = waveformTop + waveformHeight / 2, amp = Math.max(2, (waveformHeight - 4) * .42);
            ctx.fillStyle = "rgba(34,197,94,.72)";
            for (let x = Math.floor(fromX); x <= Math.ceil(toXEnd); x++) {
              const sourceMs = mediaSourceMsAtPixel(x, width, mediaStart, mediaEnd, visible.s, visible.e, sourceStart, sourceEnd);
              const h = Math.max(1, (peaks[Math.min(peaks.length - 1, Math.floor((sourceMs / Math.max(1, sourceEnd)) * peaks.length))] ?? 0) * amp);
              ctx.fillRect(x, mid - h, 1, h * 2);
            }
          }
        };
        const sourceStart = track.sourceStartMs ?? 0;
        const sourceEnd = track.sourceEndMs ?? Math.max(1, track.endMs - track.startMs);
        const sourceSpan = Math.max(1, sourceEnd - sourceStart);
        const contentTop = top + 3;
        const contentHeight = Math.max(2, rowH - 6);
        if (!track.master) {
          const fill = track.cueType === "video" ? "rgba(37,99,235,.24)"
            : track.cueType === "image" ? "rgba(168,85,247,.24)"
              : track.cueType === "group" ? "rgba(234,179,8,.20)"
              : "rgba(22,163,74,.24)";
          // Paint the cue tint first. Filmstrip and waveform pixels must stay
          // above it instead of being hidden by a later solid rectangle.
          ctx.fillStyle = fill;
          ctx.fillRect(Math.max(0, x0), contentTop, Math.max(5, x1 - x0), contentHeight);
        }
        const hasVideoWaveform = track.cueType === "video" && !!track.media?.tiles?.length && !!track.media?.waveform?.peaks?.length;
        const waveformHeight = hasVideoWaveform ? Math.max(8, Math.floor(contentHeight * 0.36)) : contentHeight;
        const visualHeight = hasVideoWaveform ? Math.max(2, contentHeight - waveformHeight - 1) : contentHeight;
        const waveformTop = hasVideoWaveform ? contentTop + visualHeight + 1 : contentTop;
        const drawPass = (passStart: number, passEnd: number) => drawMedia(
          track.media,
          passStart,
          passEnd,
          sourceStart,
          sourceEnd,
          contentTop,
          visualHeight,
          waveformTop,
          waveformHeight,
        );
        if (track.looped && track.endMs > track.startMs) {
          // Keep each pass at source duration. A short looping video therefore
          // reads as repeated filmstrip blocks/waveforms instead of a fake
          // stretched clip.
          for (let passStart = track.startMs; passStart < track.endMs; passStart += sourceSpan) {
            drawPass(passStart, Math.min(track.endMs, passStart + sourceSpan));
          }
        } else {
          drawPass(track.startMs, track.endMs);
        }
        // A Group occupies one row. Paint its named Audio/Video descendants
        // as segments in that row instead of allocating one lane per child.
        if (track.segments?.length) {
          track.segments.forEach((segment) => {
            const segmentTop = contentTop;
            const segmentSourceStart = segment.sourceStartMs ?? 0;
            const segmentSourceEnd = segment.sourceEndMs ?? Math.max(1, segment.endMs - segment.startMs);
            ctx.fillStyle = segment.cueType === "video" ? "rgba(37,99,235,.28)" : "rgba(22,163,74,.28)";
            const segmentX = Math.max(0, toX(segment.startMs));
            const segmentWidth = Math.max(5, toX(segment.endMs) - toX(segment.startMs));
            ctx.fillRect(segmentX, segmentTop, segmentWidth, contentHeight);
            const segmentHasVideo = segment.cueType === "video" && !!segment.media?.tiles?.length && !!segment.media?.waveform?.peaks?.length;
            const segmentWaveformHeight = segmentHasVideo ? Math.max(8, Math.floor(contentHeight * 0.36)) : contentHeight;
            const segmentVisualHeight = segmentHasVideo ? Math.max(2, contentHeight - segmentWaveformHeight - 1) : contentHeight;
            const segmentWaveformTop = segmentHasVideo ? segmentTop + segmentVisualHeight + 1 : segmentTop;
            // Keep nested media inside its Group lane. Audio waveforms use the
            // full segment height; video splits that height below its filmstrip.
            ctx.save();
            ctx.beginPath();
            ctx.rect(segmentX, segmentTop, segmentWidth, contentHeight);
            ctx.clip();
            drawMedia(segment.media, segment.startMs, segment.endMs, segmentSourceStart, segmentSourceEnd, segmentTop, segmentVisualHeight, segmentWaveformTop, segmentWaveformHeight);
            ctx.restore();
            ctx.strokeStyle = segment.cueType === "video" ? "#93c5fd" : "#86efac";
            ctx.strokeRect(segmentX, segmentTop, segmentWidth, contentHeight);
            ctx.fillStyle = "rgba(226,232,240,.9)";
            ctx.font = "10px sans-serif";
            ctx.fillText(segment.name, Math.max(7, segmentX + 4), segmentTop + 11);
          });
        }
        if (hasVideoWaveform) {
          ctx.strokeStyle = "rgba(148,163,184,.28)";
          ctx.beginPath();
          ctx.moveTo(0, waveformTop - 0.5);
          ctx.lineTo(width, waveformTop - 0.5);
          ctx.stroke();
        }
        ctx.strokeStyle = "rgba(148,163,184,.35)";
        ctx.beginPath(); ctx.moveTo(0, top); ctx.lineTo(width, top); ctx.stroke();
        ctx.fillStyle = track.master ? "rgba(226,232,240,.92)" : "rgba(226,232,240,.78)";
        ctx.font = "11px sans-serif";
        ctx.textAlign = "left";
        ctx.fillText(track.master ? `Master · ${track.name}` : track.name, 7, top + rowH / 2 + 4);
        if (!track.master) {
          const border = track.cueType === "video" ? "#93c5fd"
            : track.cueType === "image" ? "#d8b4fe"
              : track.cueType === "group" ? "#fde047"
              : "#86efac";
          ctx.strokeStyle = border;
          ctx.strokeRect(Math.max(0, x0), top + 3, Math.max(5, x1 - x0), Math.max(2, rowH - 6));
        }
        if (selectedNumberTrackId === track.id) {
          ctx.strokeStyle = "#f8fafc"; ctx.lineWidth = 2; ctx.strokeRect(1, top + 1, width - 2, rowH - 2);
        }
      });
    }
    // Trim and slice data are deliberately overlays: they have no hit target
    // and cannot be edited from Live.
    marker(ctx, toX(startMs ?? 0), height, "#38bdf8");
    marker(ctx, toX(endMs ?? durationMs), height, "#fb923c");
    for (const m of slices.markers) {
      if (m > (startMs ?? 0) && m < (endMs ?? durationMs) && m >= visible.s && m <= visible.e) {
        marker(ctx, toX(m), height, SLICE_COLOR, true);
      }
    }
    if (mediaPositionMs != null) {
      const p = Math.max(visible.s, Math.min(visible.e, mediaPositionMs));
      const x = toX(p);
      ctx.save();
      ctx.strokeStyle = "#f8fafc";
      ctx.lineWidth = 2;
      ctx.shadowColor = "rgba(0,0,0,.8)";
      ctx.shadowBlur = 4;
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, height);
      ctx.stroke();
      ctx.fillStyle = "#f8fafc";
      ctx.beginPath();
      ctx.moveTo(x - 5, height);
      ctx.lineTo(x + 5, height);
      ctx.lineTo(x, height - 9);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
    }
    if (previewPositionMs != null) {
      const p = Math.max(visible.s, Math.min(visible.e, previewPositionMs));
      const x = toX(p);
      ctx.save();
      ctx.strokeStyle = "#facc15";
      ctx.lineWidth = 1.5;
      ctx.setLineDash([4, 3]);
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, height);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.fillStyle = "#facc15";
      ctx.beginPath();
      ctx.moveTo(x - 4, 0);
      ctx.lineTo(x + 4, 0);
      ctx.lineTo(x, 7);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
    }
  }, [paint, paintKey, startMs, endMs, slices, durationMs, height, visible.s, visible.e, span, mediaPositionMs, previewPositionMs, canvasWidth, numberMode, numberTracks, numberDrag, selectedNumberTrackId]);

  const positionFromEvent = (e: React.MouseEvent<HTMLCanvasElement>) => xToMs(e.clientX);
  const numberTrackFromEvent = (e: React.MouseEvent<HTMLCanvasElement>) => {
    if (!numberMode || numberTracks.length === 0) return null;
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return null;
    const index = Math.max(0, Math.min(numberTracks.length - 1,
      Math.floor(((e.clientY - rect.top) / Math.max(1, rect.height)) * numberTracks.length)));
    return numberTracks[index] ?? null;
  };
  const numberDragFromEvent = (e: React.MouseEvent<HTMLCanvasElement>) => {
    if (editLocked) return null;
    const track = numberTrackFromEvent(e);
    if (!track || track.master || track.readonly) return null;
    const at = positionFromEvent(e);
    const px = canvasRef.current?.getBoundingClientRect().width ?? 1;
    const edge = (span / Math.max(1, px)) * 9;
    const kind = Math.abs(at - track.startMs) <= edge ? "start"
      : Math.abs(at - track.endMs) <= edge ? "end" : "move";
    return { track, kind: kind as "move" | "start" | "end", at };
  };
  const announcePosition = (e: React.MouseEvent<HTMLCanvasElement>) => {
    const ms = Math.round(positionFromEvent(e));
    lastPositionRef.current = ms;
    if (seekEnabled) onSeek?.(ms);
    else onPreviewPosition?.(ms);
  };

  const announceNumberPosition = (ms: number) => {
    lastPositionRef.current = Math.round(ms);
    if (seekEnabled) onSeek?.(ms);
    else onPreviewPosition?.(ms);
  };

  const finishPosition = () => {
    if (seekEnabled) onSeekEnd?.(lastPositionRef.current);
    else onPreviewPositionEnd?.(lastPositionRef.current);
  };

  const fitWholeFile = () => {
    setView(null);
    // Keep the owner's range filmstrip state in lockstep with the local view;
    // otherwise stale zoomed frames can remain visible after Fit.
    onViewChange?.(null);
  };

  return (
    <div
      style={{ position: "relative" }}
      title={previewPositionMs != null ? previewLabel : undefined}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 6, marginBottom: 3, fontSize: 10, color: "var(--wc-text-faint)" }}>
        <span>Wheel: zoom · Shift+wheel: pan</span>
        <span style={{ flex: 1 }} />
        {hoverMs != null && <span style={{ fontVariantNumeric: "tabular-nums" }}>{(hoverMs / 1000).toFixed(3)}s</span>}
        <button
          type="button"
          onClick={toggleEditLock}
          aria-pressed={editLocked}
          aria-label={editLocked ? "Разблокировать редактирование таймлайна" : "Заблокировать редактирование таймлайна"}
          title={editLocked ? "Разблокировать перемещение и обрезку" : "Заблокировать перемещение и обрезку"}
          style={{ ...zoomButton, color: editLocked ? "#facc15" : "var(--wc-text-muted)" }}
        >{editLocked ? "🔒" : "🔓"}</button>
        {previewPositionMs != null && !numberMode && (
          <span
            role="status"
            aria-label={previewLabel}
            title={previewLabel}
            style={{ color: "#facc15", fontSize: 12 }}
          >🎧</span>
        )}
        {view && <button type="button" onClick={fitWholeFile} style={zoomButton} title="Fit whole file">Fit</button>}
      </div>
      {numberMode && (
        <div
          aria-label="Number timing"
          style={{ display: "flex", height: 22, marginBottom: 3, borderRadius: 3, overflow: "hidden", border: "1px solid var(--wc-border)" }}
        >
          {[
            { key: "pre", ms: numberPreWaitMs, label: "PRE-WAIT", color: "#854d0e" },
            { key: "action", ms: durationMs, label: "ACTION", color: "#166534" },
            { key: "post", ms: numberPostWaitMs, label: "POST-WAIT", color: "#1e3a8a" },
          ].filter((segment) => segment.ms > 0).map((segment) => {
            const active = numberRuntimeElapsedMs != null
              && numberRuntimeElapsedMs >= (segment.key === "pre" ? 0 : segment.key === "action" ? numberPreWaitMs : numberPreWaitMs + durationMs)
              && numberRuntimeElapsedMs < (segment.key === "pre" ? numberPreWaitMs : segment.key === "action" ? numberPreWaitMs + durationMs : numberTotalTimingMs);
            return (
              <div
                key={segment.key}
                style={{ flex: `${segment.ms} 1 0`, minWidth: 0, padding: "3px 6px", background: active ? segment.color : "var(--wc-bg-surface)", color: active ? "#fff" : "var(--wc-text-muted)", fontSize: 9, fontWeight: 600, letterSpacing: ".04em", textAlign: "center", borderRight: "1px solid var(--wc-border)", whiteSpace: "nowrap" }}
              >
                {segment.key === "action" ? t("cueList.duration") : segment.label} · {segment.key === "action" ? formatDurationMs(segment.ms) : `${(segment.ms / 1000).toFixed(2)}s`}
              </div>
            );
          })}
        </div>
      )}
      <canvas
        ref={canvasRef}
        aria-label={previewPositionMs != null ? `Live media timeline; ${previewLabel.toLowerCase()} active` : "Live media timeline"}
        style={{ width: "100%", height, display: "block", borderRadius: 4, cursor: numberMode ? (numberDrag ? "ew-resize" : "crosshair") : (seekEnabled ? "crosshair" : "default") }}
        onMouseDown={(e) => {
          if (e.button !== 0) return;
          // The timeline lock is a complete position lock.  It must prevent
          // seeking/scrubbing as well as move/trim/slice edits.  Returning at
          // the event boundary also prevents a click from selecting another
          // Number layer or changing the preview cursor.
          if (editLocked) return;
          if (numberMode) {
            const at = positionFromEvent(e);
            lastPositionRef.current = Math.round(at);
            const row = numberTrackFromEvent(e);
            if (row) setSelectedNumberTrackId(row.id);
            const hit = numberDragFromEvent(e);
            if (hit) setNumberDrag({
              track: hit.track,
              kind: hit.kind,
              originStartMs: hit.track.startMs,
              originEndMs: hit.track.endMs,
              anchorMs: hit.at,
              startMs: hit.track.startMs,
              endMs: hit.track.endMs,
              maxDurationMs: hit.track.maxDurationMs,
              moved: false,
            });
            else announceNumberPosition(at);
            return;
          }
          setDragging(true); announcePosition(e);
        }}
        onMouseMove={(e) => {
          setHoverMs(positionFromEvent(e));
          if (editLocked) {
            // A lock blocks every position-changing operation.  Cancel any
            // gesture that was started before the lock was enabled and keep
            // only the passive hover indicator alive.
            if (numberDrag) {
              setNumberDrag(null);
            }
            if (dragging) setDragging(false);
            return;
          }
          if (numberDrag) {
            const at = positionFromEvent(e);
            lastPositionRef.current = Math.round(at);
            const moved = numberDrag.moved
              || Math.abs(at - numberDrag.anchorMs) > Math.max(2, span / Math.max(1, canvasWidth) * 2);
            // A click inside a clip is a seek. Do not preview a transient
            // move until the pointer has crossed the drag threshold.
            if (!moved) {
              setNumberDrag({ ...numberDrag, moved: false });
              return;
            }
            const pointerMs = numberDrag.kind === "move"
              ? numberDrag.originStartMs + (at - numberDrag.anchorMs)
              : at;
            const proposed = numberDragRange(
              numberDrag.kind,
              numberDrag.originStartMs,
              numberDrag.originEndMs,
              pointerMs,
              durationMs,
              numberDrag.maxDurationMs,
            );
            const snapped = onNumberTrackSnap?.(numberDrag.track, numberDrag.kind, proposed.startMs, proposed.endMs) ?? proposed;
            const next = applyNumberDragSnap(numberDrag.kind, proposed, snapped);
            setNumberDrag({ ...numberDrag, startMs: next.startMs, endMs: next.endMs, moved });
          } else if (dragging) announcePosition(e);
        }}
        onMouseUp={() => {
          if (editLocked) {
            setNumberDrag(null);
            setDragging(false);
            return;
          }
          if (numberDrag) {
            const d = numberDrag;
            setNumberDrag(null);
            if (!editLocked && d.moved) onNumberTrackChange?.(d.track, d.kind, d.startMs, d.endMs);
            else announceNumberPosition(lastPositionRef.current);
            if (!d.moved) onPreviewPositionEnd?.(lastPositionRef.current);
          } else if (numberMode) onPreviewPositionEnd?.(lastPositionRef.current);
          else if (dragging) finishPosition();
          setDragging(false);
        }}
        onMouseLeave={() => {
          if (editLocked) {
            setNumberDrag(null);
            setDragging(false);
            setHoverMs(null);
            return;
          }
          if (numberDrag) {
            const d = numberDrag;
            setNumberDrag(null);
            if (d.moved) onNumberTrackChange?.(d.track, d.kind, d.startMs, d.endMs);
            else announceNumberPosition(lastPositionRef.current);
            if (!d.moved) onPreviewPositionEnd?.(lastPositionRef.current);
          } else if (numberMode) onPreviewPositionEnd?.(lastPositionRef.current);
          if (dragging) finishPosition();
          setDragging(false); setHoverMs(null);
        }}
      />
      <div style={{ display: "flex", justifyContent: "space-between", fontSize: 10, color: "var(--wc-text-faint)", marginTop: 3 }}>
        <span>{((visible.s) / 1000).toFixed(2)}s</span>
        <span>{((visible.e) / 1000).toFixed(2)}s</span>
      </div>
    </div>
  );
}

const zoomButton: React.CSSProperties = {
  background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
  borderRadius: 3, color: "var(--wc-text)", cursor: "pointer", fontSize: 10, padding: "1px 7px",
};
