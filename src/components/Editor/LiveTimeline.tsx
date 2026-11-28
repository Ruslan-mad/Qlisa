// Read-only, transport-facing media strip used by the Live tab.  This keeps
// the same canvas painter as SliceTimeline, but never writes trim or slices.

import { useEffect, useRef, useState } from "react";
import type { GainEnvelopePoint, SliceList, WaveformData } from "../../lib/types";
import type { TrimPainter, TrimView } from "../Inspector/TrimStrip";
import { TRIM_END_COLOR, TRIM_HANDLE_HIT_MARGIN, TRIM_HANDLE_HIT_RADIUS, TRIM_HANDLE_ZONE_HEIGHT, TRIM_START_COLOR, drawTrimMarker, trimHandleHitDistance, useCanvasWidth } from "../Inspector/TrimStrip";
import { applyNumberDragSnap, numberDragRange } from "./numberTimelineModel";
import { mediaSourceMsAtPixel, panTimelineView, sourceRangeForTimelinePass, waveformBinAtSourceMs, zoomTimelineView } from "./timelineViewModel";
import { useTimelineEditorStore } from "../../stores/timelineEditorStore";
import { useLocale } from "../../i18n";
import { formatDurationMs } from "../CueList/formatDuration";
import { GainEnvelopeOverlay } from "./GainEnvelopeOverlay";
import type { AudioFadeOverlay, FadeDirection } from "./audioFadeModel";
import { drawVideoFilmstripWindows, type VideoFilmstripWindow } from "./videoFilmstripModel";
import { beginCanvasPointer, cancelCanvasPointer, endCanvasPointer, ownsCanvasPointer } from "./sliceTimelineInteraction";

const SLICE_COLOR = "#facc15";

export interface NumberTimelineTrack {
  id: string;
  name: string;
  startMs: number;
  endMs: number;
  master?: boolean;
  cueType?: "audio" | "video" | "image" | "group";
  media?: { waveform?: WaveformData | null; tiles?: HTMLImageElement[] | null; filmstrips?: VideoFilmstripWindow[] };
  sourcePath?: string | null;
  sourceRevision?: string | null;
  sourceStartMs?: number;
  sourceEndMs?: number;
  /** Maximum one-pass source span available for edge resizing. */
  maxDurationMs?: number;
  /** Repeat the source pass until endMs instead of stretching it. */
  looped?: boolean;
  /** Source media runs at this speed on the Number clock. */
  rate?: number;
  /** Group headers and nested children are display-only until nested editing is wired. */
  readonly?: boolean;
  /** Child media blocks rendered inside one container row. */
  segments?: NumberTimelineTrack[];
}

function fillEllipsizedLabel(ctx: CanvasRenderingContext2D, label: string, x: number, y: number, maxWidth: number) {
  if (maxWidth < 24) return;
  let text = label;
  while (text.length > 1 && ctx.measureText(text).width > maxWidth) text = `${text.slice(0, -2)}…`;
  if (ctx.measureText(text).width <= maxWidth) ctx.fillText(text, x, y);
}

function drawSliceMarker(ctx: CanvasRenderingContext2D, x: number, top: number, bottom: number, color: string) {
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.5;
  ctx.setLineDash([5, 3]);
  ctx.beginPath();
  ctx.moveTo(x, top);
  ctx.lineTo(x, bottom);
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.moveTo(x - 5, bottom);
  ctx.lineTo(x + 5, bottom);
  ctx.lineTo(x, bottom - 8);
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
  gainEnvelope?: GainEnvelopePoint[] | null;
  gainEnvelopeEditable?: boolean;
  /** Render the cue's authored curve as a passive Live overlay. */
  gainEnvelopeReadOnly?: boolean;
  onGainEnvelopeChange?: (points: GainEnvelopePoint[] | null) => void;
  audioFade?: AudioFadeOverlay | null;
  audioFadeEditable?: boolean;
  audioFadeContextKey?: string;
  onAudioFadeDurationChange?: (direction: FadeDirection, durationMs: number) => void;
  /** Optional multi-track Number overlay rendered in this same native canvas. */
  numberTracks?: NumberTimelineTrack[];
  /** Disable operator track selection while preserving Number timeline seek. */
  selectNumberTracks?: boolean;
  /** Keep Number timing bands out of non-Number views that reuse its track overlay. */
  showNumberTimingBands?: boolean;
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
  gainEnvelope,
  gainEnvelopeEditable = false,
  gainEnvelopeReadOnly = false,
  onGainEnvelopeChange,
  audioFade,
  audioFadeEditable = false,
  audioFadeContextKey,
  onAudioFadeDurationChange,
  numberTracks = [],
  selectNumberTracks = true,
  showNumberTimingBands = true,
  numberPreWaitMs = 0,
  numberPostWaitMs = 0,
  numberRuntimeElapsedMs = null,
  onNumberTrackChange,
  onNumberTrackSnap,
}: LiveTimelineProps) {
  const { t, locale } = useLocale();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const activePointerId = useRef<number | null>(null);
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
  const timelineContentTop = numberMode ? 0 : TRIM_HANDLE_ZONE_HEIGHT;
  const timelineContentHeight = Math.max(1, height - timelineContentTop);
  const editLocked = useTimelineEditorStore((state) => state.locked);
  const toggleEditLock = useTimelineEditorStore((state) => state.toggle);
  const numberTotalTimingMs = Math.max(1, numberPreWaitMs + durationMs + numberPostWaitMs);
  const previewLabel = numberMode ? "Number preview cursor" : "Headphone preview playhead";
  const [selectedNumberTrackId, setSelectedNumberTrackId] = useState<string | null>(null);
  const viewRef = useRef(view);
  const lastPositionRef = useRef(0);
  viewRef.current = view;

  const cancelActivePointer = () => {
    const pointerId = activePointerId.current;
    const hadPointer = cancelCanvasPointer(activePointerId);
    const canvas = canvasRef.current;
    if (hadPointer && pointerId !== null && canvas?.hasPointerCapture(pointerId)) canvas.releasePointerCapture(pointerId);
    setNumberDrag(null);
    setDragging(false);
  };

  // A lock can be enabled while a drag is already active. Invalidate the
  // transient drag so a later browser mouseup cannot commit that edit.
  useEffect(() => {
    if (!editLocked) return;
    cancelActivePointer();
  }, [editLocked]);

  useEffect(() => {
    cancelActivePointer();
    // A new cue/Number invalidates an in-flight gesture. Paint detail updates
    // are deliberately excluded from resetKey.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resetKey]);

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
    if (timelineContentTop > 0) {
      ctx.save();
      ctx.translate(0, timelineContentTop);
      paint(ctx, width, timelineContentHeight, toX(startMs ?? 0), toX(endMs ?? durationMs), { startMs: visible.s, endMs: visible.e });
      ctx.restore();
    } else {
      paint(ctx, width, timelineContentHeight, toX(startMs ?? 0), toX(endMs ?? durationMs), { startMs: visible.s, endMs: visible.e });
    }
    if (numberMode) {
      const rowH = height / numberTracks.length;
      const renderedTracks = numberTracks.map((track) => numberDrag?.track.id === track.id
        ? { ...track, startMs: numberDrag.startMs, endMs: numberDrag.endMs }
        : track);
      renderedTracks.forEach((track, index) => {
        const top = index * rowH;
        const x0 = toX(track.startMs);
        const x1 = toX(track.endMs);
        const hasChildHandles = !track.master && !track.readonly;
        const trackContentTop = top + 3;
        const trackContentHeight = Math.max(2, rowH - 6);
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
          const strips = media?.filmstrips?.length
            ? media.filmstrips
            : tiles?.length
              ? [{ sourceStartMs: sourceStart, sourceEndMs: sourceEnd, tiles }]
              : [];
          if (strips.length && visualHeight > 1) {
            drawVideoFilmstripWindows(ctx, strips, {
              timelineStartMs: mediaStart,
              timelineEndMs: mediaEnd,
              sourceStartMs: sourceStart,
              sourceEndMs: sourceEnd,
              viewStartMs: visible.s,
              viewEndMs: visible.e,
              width,
              top: visualTop,
              height: visualHeight,
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
              const fileDurationMs = (media?.waveform?.file_duration_s ?? 0) * 1000;
              const bin = waveformBinAtSourceMs(sourceMs, fileDurationMs, peaks.length);
              const h = Math.max(1, (peaks[bin] ?? 0) * amp);
              ctx.fillRect(x, mid - h, 1, h * 2);
            }
          }
        };
        const sourceStart = track.sourceStartMs ?? 0;
        const sourceEnd = track.sourceEndMs ?? Math.max(1, track.endMs - track.startMs);
        const sourceSpan = Math.max(1, sourceEnd - sourceStart);
        const rate = Number.isFinite(track.rate) && (track.rate ?? 0) > 0 ? track.rate! : 1;
        const timelinePassDuration = sourceSpan / rate;
        if (!track.master) {
          const fill = track.cueType === "video" ? "rgba(37,99,235,.24)"
            : track.cueType === "image" ? "rgba(168,85,247,.24)"
              : track.cueType === "group" ? "rgba(234,179,8,.20)"
              : "rgba(22,163,74,.24)";
          // Paint the cue tint first. Filmstrip and waveform pixels must stay
          // above it instead of being hidden by a later solid rectangle.
          ctx.fillStyle = fill;
          ctx.fillRect(Math.max(0, x0), trackContentTop, Math.max(5, x1 - x0), trackContentHeight);
        }
        const hasVideoWaveform = track.cueType === "video" && !!track.media?.tiles?.length && !!track.media?.waveform?.peaks?.length;
        const waveformHeight = hasVideoWaveform ? Math.max(8, Math.floor(trackContentHeight * 0.36)) : trackContentHeight;
        const visualHeight = hasVideoWaveform ? Math.max(2, trackContentHeight - waveformHeight - 1) : trackContentHeight;
        const waveformTop = hasVideoWaveform ? trackContentTop + visualHeight + 1 : trackContentTop;
        const drawPass = (passStart: number, passEnd: number) => {
          const sourceRange = sourceRangeForTimelinePass(passStart, passEnd, sourceStart, sourceEnd, rate);
          drawMedia(
            track.media,
            passStart,
            passEnd,
            sourceRange.startMs,
            sourceRange.endMs,
            trackContentTop,
            visualHeight,
            waveformTop,
            waveformHeight,
          );
        };
        if (track.looped && track.endMs > track.startMs) {
          // Keep each pass at source duration. A short looping video therefore
          // reads as repeated filmstrip blocks/waveforms instead of a fake
          // stretched clip.
          for (let passStart = track.startMs; passStart < track.endMs; passStart += timelinePassDuration) {
            drawPass(passStart, Math.min(track.endMs, passStart + timelinePassDuration));
          }
        } else {
          drawPass(track.startMs, track.endMs);
        }
        // A Group occupies one row. Paint its named Audio/Video descendants
        // as segments in that row instead of allocating one lane per child.
        if (track.segments?.length) {
          track.segments.forEach((segment) => {
            const segmentTop = trackContentTop;
            const segmentSourceStart = segment.sourceStartMs ?? 0;
            const segmentSourceEnd = segment.sourceEndMs ?? Math.max(1, segment.endMs - segment.startMs);
            ctx.fillStyle = segment.cueType === "video" ? "rgba(37,99,235,.28)" : "rgba(22,163,74,.28)";
            const segmentX = Math.max(0, toX(segment.startMs));
            const segmentWidth = Math.max(5, toX(segment.endMs) - toX(segment.startMs));
            ctx.fillRect(segmentX, segmentTop, segmentWidth, trackContentHeight);
            const segmentHasVideo = segment.cueType === "video" && !!segment.media?.tiles?.length && !!segment.media?.waveform?.peaks?.length;
            const segmentWaveformHeight = segmentHasVideo ? Math.max(8, Math.floor(trackContentHeight * 0.36)) : trackContentHeight;
            const segmentVisualHeight = segmentHasVideo ? Math.max(2, trackContentHeight - segmentWaveformHeight - 1) : trackContentHeight;
            const segmentWaveformTop = segmentHasVideo ? segmentTop + segmentVisualHeight + 1 : segmentTop;
            // Keep nested media inside its Group lane. Audio waveforms use the
            // full segment height; video splits that height below its filmstrip.
            ctx.save();
            ctx.beginPath();
            ctx.rect(segmentX, segmentTop, segmentWidth, trackContentHeight);
            ctx.clip();
            const segmentRate = Number.isFinite(segment.rate) && (segment.rate ?? 0) > 0 ? segment.rate! : 1;
            const segmentPassDuration = Math.max(1, (segmentSourceEnd - segmentSourceStart) / segmentRate);
            const drawSegmentPass = (passStart: number, passEnd: number) => {
              const sourceRange = sourceRangeForTimelinePass(passStart, passEnd, segmentSourceStart, segmentSourceEnd, segmentRate);
              drawMedia(segment.media, passStart, passEnd, sourceRange.startMs, sourceRange.endMs, segmentTop, segmentVisualHeight, segmentWaveformTop, segmentWaveformHeight);
            };
            if (segment.looped) {
              for (let passStart = segment.startMs; passStart < segment.endMs; passStart += segmentPassDuration) {
                drawSegmentPass(passStart, Math.min(segment.endMs, passStart + segmentPassDuration));
              }
            } else {
              drawSegmentPass(segment.startMs, segment.endMs);
            }
            ctx.restore();
            ctx.strokeStyle = segment.cueType === "video" ? "#93c5fd" : "#86efac";
            ctx.strokeRect(segmentX, segmentTop, segmentWidth, trackContentHeight);
            ctx.fillStyle = "rgba(226,232,240,.9)";
            ctx.font = "10px sans-serif";
            if (trackContentHeight >= 40 && segmentWidth >= 48) {
              ctx.save();
              ctx.beginPath();
              ctx.rect(segmentX + 3, segmentTop + 2, Math.max(0, segmentWidth - 6), 14);
              ctx.clip();
              fillEllipsizedLabel(ctx, segment.name, segmentX + 4, segmentTop + 11, segmentWidth - 8);
              ctx.restore();
            }
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
        const masterLabel = t("numberUi.masterShort");
        const trackLabel = track.master
          ? `${masterLabel.charAt(0).toUpperCase()}${masterLabel.slice(1)} · ${track.name}`
          : track.name;
        if (!track.master) {
          const border = track.cueType === "video" ? "#93c5fd"
            : track.cueType === "image" ? "#d8b4fe"
              : track.cueType === "group" ? "#fde047"
              : "#86efac";
          ctx.strokeStyle = border;
          ctx.strokeRect(Math.max(0, x0), trackContentTop, Math.max(5, x1 - x0), trackContentHeight);
          if (hasChildHandles) {
            drawTrimMarker(ctx, x0, width, top, top + TRIM_HANDLE_ZONE_HEIGHT, top + rowH, TRIM_START_COLOR);
            drawTrimMarker(ctx, x1, width, top, top + TRIM_HANDLE_ZONE_HEIGHT, top + rowH, TRIM_END_COLOR);
          }
        }
        const labelY = hasChildHandles ? top + rowH - 3 : top + rowH / 2 + 4;
        const visibleLeft = track.master ? 0 : Math.max(0, Math.min(width, x0));
        const visibleRight = track.master ? width : Math.max(visibleLeft, Math.min(width, x1));
        const labelX = track.master ? 7 : Math.max(visibleLeft + 4, Math.min(visibleRight - 4, x0 + 4));
        const labelWidth = Math.max(0, visibleRight - visibleLeft - (track.master ? 14 : 8));
        ctx.save();
        ctx.beginPath();
        ctx.rect(visibleLeft, top, Math.max(0, visibleRight - visibleLeft), rowH);
        ctx.clip();
        fillEllipsizedLabel(ctx, trackLabel, labelX, labelY, labelWidth);
        ctx.restore();
        if (selectNumberTracks && selectedNumberTrackId === track.id) {
          ctx.strokeStyle = "#f8fafc"; ctx.lineWidth = 2; ctx.strokeRect(1, top + 1, width - 2, rowH - 2);
        }
      });
    }
    // Trim and slice data are deliberately overlays: they have no hit target
    // and cannot be edited from Live.
    if (numberMode) {
      ctx.save();
      ctx.strokeStyle = TRIM_START_COLOR;
      ctx.lineWidth = 2;
      ctx.beginPath(); ctx.moveTo(toX(startMs ?? 0), 0); ctx.lineTo(toX(startMs ?? 0), height); ctx.stroke();
      ctx.strokeStyle = TRIM_END_COLOR;
      ctx.beginPath(); ctx.moveTo(toX(endMs ?? durationMs), 0); ctx.lineTo(toX(endMs ?? durationMs), height); ctx.stroke();
      ctx.restore();
    } else {
      drawTrimMarker(ctx, toX(startMs ?? 0), width, 0, timelineContentTop, height, TRIM_START_COLOR);
      drawTrimMarker(ctx, toX(endMs ?? durationMs), width, 0, timelineContentTop, height, TRIM_END_COLOR);
    }
    for (const m of slices.markers) {
      if (m > (startMs ?? 0) && m < (endMs ?? durationMs) && m >= visible.s && m <= visible.e) {
        drawSliceMarker(ctx, toX(m), timelineContentTop, height, SLICE_COLOR);
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
      ctx.moveTo(x, timelineContentTop);
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
      ctx.moveTo(x, timelineContentTop);
      ctx.lineTo(x, height);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.fillStyle = "#facc15";
      ctx.beginPath();
      ctx.moveTo(x - 4, timelineContentTop);
      ctx.lineTo(x + 4, timelineContentTop);
      ctx.lineTo(x, timelineContentTop + 7);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
    }
  }, [t, locale, paint, paintKey, startMs, endMs, slices, durationMs, height, visible.s, visible.e, span, mediaPositionMs, previewPositionMs, canvasWidth, numberMode, numberTracks, numberDrag, selectedNumberTrackId, selectNumberTracks]);

  const positionFromEvent = (e: React.PointerEvent<HTMLCanvasElement>) => xToMs(e.clientX);
  const numberTrackFromEvent = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!numberMode || numberTracks.length === 0) return null;
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect) return null;
    const index = Math.max(0, Math.min(numberTracks.length - 1,
      Math.floor(((e.clientY - rect.top) / Math.max(1, rect.height)) * numberTracks.length)));
    return numberTracks[index] ?? null;
  };
  const numberDragFromEvent = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (editLocked) return null;
    const track = numberTrackFromEvent(e);
    if (!track || track.master || track.readonly) return null;
    const at = positionFromEvent(e);
    const rect = canvasRef.current?.getBoundingClientRect();
    const px = rect?.width ?? 1;
    const pointerX = rect ? e.clientX - rect.left : 0;
    const startX = ((track.startMs - visible.s) / span) * px;
    const endX = ((track.endMs - visible.s) / span) * px;
    const startCenterDistance = Math.abs(pointerX - startX);
    const endCenterDistance = Math.abs(pointerX - endX);
    const startCapDistance = trimHandleHitDistance(pointerX, startX, px);
    const endCapDistance = trimHandleHitDistance(pointerX, endX, px);
    const startCapHit = startCapDistance <= TRIM_HANDLE_HIT_MARGIN;
    const endCapHit = endCapDistance <= TRIM_HANDLE_HIT_MARGIN;
    const startHit = startCapHit || startCenterDistance <= TRIM_HANDLE_HIT_RADIUS;
    const endHit = endCapHit || endCenterDistance <= TRIM_HANDLE_HIT_RADIUS;
    const kind = startHit && (!endHit || startCenterDistance <= endCenterDistance) ? "start"
      : endHit ? "end" : "move";
    return { track, kind: kind as "move" | "start" | "end", at };
  };
  const announcePosition = (e: React.PointerEvent<HTMLCanvasElement>) => {
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
  const gainMasterIndex = gainEnvelopeReadOnly && numberMode
    ? numberTracks.findIndex((track) => track.master)
    : -1;
  const gainOverlayHeight = gainMasterIndex >= 0 ? height / Math.max(1, numberTracks.length) : timelineContentHeight;
  const gainOverlay = (
    <GainEnvelopeOverlay
      points={gainEnvelope}
      durationMs={durationMs}
      view={{ startMs: visible.s, endMs: visible.e }}
      width={canvasWidth}
      height={gainOverlayHeight}
      editable={gainEnvelopeEditable && !editLocked}
      strokeWidth={gainEnvelopeReadOnly ? 2 : 4}
      alwaysShowLabels={gainEnvelopeReadOnly}
      fade={audioFade}
      fadeEditable={audioFadeEditable && !editLocked}
      fadeContextKey={audioFadeContextKey}
      onFadeDurationChange={onAudioFadeDurationChange}
      onCommit={onGainEnvelopeChange}
    />
  );

  return (
    <div
      style={{ position: "relative" }}
      title={previewPositionMs != null ? previewLabel : undefined}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 6, marginBottom: 3, fontSize: 10, color: "var(--wc-text-faint)" }}>
        <span>{t("editorUi.zoomPanHint")}</span>
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
        {view && <button type="button" onClick={fitWholeFile} style={zoomButton} title={t("editorUi.fitTimeline")}>{t("editorUi.fitTimeline")}</button>}
      </div>
      {numberMode && showNumberTimingBands && (
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
                {segment.key === "action" ? t("cueList.duration") : segment.label} · {formatDurationMs(segment.ms)}
              </div>
            );
          })}
        </div>
      )}
      {gainEnvelopeEditable && <div style={{ color: "var(--wc-text-faint)", fontSize: 10, margin: "0 0 3px" }}>{t("editorUi.gainEnvelopeHint")}</div>}
      <div style={{ position: "relative", height }}>
        <canvas
          ref={canvasRef}
          aria-label={previewPositionMs != null ? `Live media timeline; ${previewLabel.toLowerCase()} active` : "Live media timeline"}
          style={{ width: "100%", height, display: "block", borderRadius: 4, cursor: numberMode ? (numberDrag ? "ew-resize" : "crosshair") : (seekEnabled ? "crosshair" : "default"), touchAction: "none" }}
          onPointerDown={(e) => {
          if (e.button !== 0 || !e.isPrimary || activePointerId.current !== null) return;
          // The timeline lock is a complete position lock.  It must prevent
          // seeking/scrubbing as well as move/trim/slice edits.  Returning at
          // the event boundary also prevents a click from selecting another
          // Number layer or changing the preview cursor.
          if (editLocked) return;
          e.preventDefault();
          beginCanvasPointer(activePointerId, e.pointerId);
          e.currentTarget.setPointerCapture(e.pointerId);
          if (numberMode) {
            const at = positionFromEvent(e);
            lastPositionRef.current = Math.round(at);
            const row = numberTrackFromEvent(e);
            if (row && selectNumberTracks) setSelectedNumberTrackId(row.id);
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
          onPointerMove={(e) => {
          if (!ownsCanvasPointer(activePointerId, e.pointerId)) {
            setHoverMs(positionFromEvent(e));
            return;
          }
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
              numberDrag.track.cueType === "image" ? Math.min(100, durationMs) : 0,
            );
            const snapped = onNumberTrackSnap?.(numberDrag.track, numberDrag.kind, proposed.startMs, proposed.endMs) ?? proposed;
            const snappedRange = applyNumberDragSnap(numberDrag.kind, proposed, snapped);
            const next = numberDrag.track.cueType === "image" && numberDrag.kind !== "move"
              ? numberDragRange(
                  numberDrag.kind,
                  proposed.startMs,
                  proposed.endMs,
                  numberDrag.kind === "start" ? snappedRange.startMs : snappedRange.endMs,
                  durationMs,
                  numberDrag.maxDurationMs,
                  Math.min(100, durationMs),
                )
              : snappedRange;
            setNumberDrag({ ...numberDrag, startMs: next.startMs, endMs: next.endMs, moved });
          } else if (dragging) announcePosition(e);
          }}
          onPointerUp={(e) => {
          if (!endCanvasPointer(activePointerId, e.pointerId)) return;
          if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
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
          onPointerCancel={(e) => {
            if (!endCanvasPointer(activePointerId, e.pointerId)) return;
            setNumberDrag(null);
            setDragging(false);
          }}
          onLostPointerCapture={(e) => {
            if (!endCanvasPointer(activePointerId, e.pointerId)) return;
            setNumberDrag(null);
            setDragging(false);
          }}
          onPointerLeave={() => setHoverMs(null)}
        />
        {(gainEnvelopeEditable || gainEnvelopeReadOnly) && (!numberMode || gainEnvelopeReadOnly) && (
          gainMasterIndex >= 0 ? (
            <div style={{ position: "absolute", left: 0, right: 0, top: gainMasterIndex * gainOverlayHeight, height: gainOverlayHeight, zIndex: 2, pointerEvents: "none" }}>
              {gainOverlay}
            </div>
          ) : numberMode && gainEnvelopeReadOnly ? null : (
            <div style={{ position: "absolute", left: 0, right: 0, top: timelineContentTop, height: timelineContentHeight, zIndex: 2, pointerEvents: "none" }}>
              {gainOverlay}
            </div>
          )
        )}
      </div>
      <div style={{ display: "flex", justifyContent: "space-between", fontSize: 10, color: "var(--wc-text-faint)", marginTop: 3 }}>
        <span>{formatDurationMs(visible.s)}</span>
        <span>{formatDurationMs(visible.e)}</span>
      </div>
    </div>
  );
}

const zoomButton: React.CSSProperties = {
  background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
  borderRadius: 3, color: "var(--wc-text)", cursor: "pointer", fontSize: 10, padding: "1px 7px",
};
