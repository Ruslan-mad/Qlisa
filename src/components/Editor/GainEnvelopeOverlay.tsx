import { useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent, MouseEvent as ReactUiMouseEvent } from "react";
import type { GainEnvelopePoint } from "../../lib/types";
import { useLocale } from "../../i18n";
import {
  addEnvelopePoint,
  deleteEnvelopePoint,
  displayedEnvelopePoints,
  envelopeDb,
  envelopeGainFromY,
  envelopeGainY,
  envelopeGainAt,
  envelopePercent,
  GAIN_ENVELOPE_MAX_POINTS,
  hitTestEnvelope,
  moveEnvelopePoint,
  moveEnvelopeSegment,
  resetEnvelopePointGain,
  splitEnvelopeAtUnity,
} from "./gainEnvelopeModel";
import {
  audioFadeWindows,
  clampFadeDurationFromHandle,
  fadeDragCommit,
  effectiveGainAt,
  fadeHandleSourceMs,
  fadeMultiplierAt,
  fadeNotice,
  formatAudioFadeDuration,
  isFadeHandleHit,
  FADE_HANDLE_HIT_RADIUS,
  type AudioFadeOverlay,
  type FadeDirection,
} from "./audioFadeModel";

export interface GainEnvelopeView {
  startMs: number;
  endMs: number;
}

interface DragState {
  kind: "point" | "segment";
  index: number;
  startX: number;
  startY: number;
  startGain: number;
  original: GainEnvelopePoint[];
  moved: boolean;
}

interface FadeDragState {
  direction: FadeDirection;
  boundaryMs: number;
  startDurationMs: number;
  startX: number;
  startY: number;
  moved: boolean;
}

type OverlayHover = { kind: "gain" | "fade"; x: number; y: number; timeMs: number; gain: number; fadeDirection?: FadeDirection };

export function GainEnvelopeOverlay({
  points,
  durationMs,
  view,
  width,
  height,
  editable,
  strokeWidth = 4,
  alwaysShowLabels = false,
  fade,
  fadeEditable = false,
  fadeContextKey,
  onFadeDurationChange,
  onCommit,
}: {
  points?: GainEnvelopePoint[] | null;
  durationMs: number;
  view: GainEnvelopeView;
  width: number;
  height: number;
  editable: boolean;
  strokeWidth?: number;
  alwaysShowLabels?: boolean;
  fade?: AudioFadeOverlay | null;
  fadeEditable?: boolean;
  fadeContextKey?: string;
  onFadeDurationChange?: (direction: FadeDirection, durationMs: number) => void;
  onCommit?: (points: GainEnvelopePoint[] | null) => void;
}) {
  const { t, locale } = useLocale();
  const svgRef = useRef<SVGSVGElement>(null);
  const dragRef = useRef<DragState | null>(null);
  const fadeDragRef = useRef<FadeDragState | null>(null);
  const lastAddRef = useRef<{ time: number; x: number; y: number } | null>(null);
  const sourcePoints = points ?? [];
  const [draft, setDraft] = useState<GainEnvelopePoint[] | null>(null);
  const [hover, setHover] = useState<OverlayHover | null>(null);
  const [fadeDraft, setFadeDraft] = useState<{ direction: FadeDirection; durationMs: number } | null>(null);
  const fadeDraftRef = useRef<{ direction: FadeDirection; durationMs: number } | null>(null);
  const [maxReached, setMaxReached] = useState(false);
  const activePoints = draft ?? sourcePoints;
  const fadeSignature = fade
    ? `${fade.fadeInMs}:${fade.fadeInCurve}:${fade.fadeOutMs}:${fade.fadeOutCurve}:${fade.trimStartMs}:${fade.trimEndMs}:${fade.rate}:${fade.loopCount}:${fade.sliced}`
    : "none";
  useEffect(() => setDraft(null), [points]);
  useEffect(() => {
    if (editable && fadeEditable) return;
    fadeDragRef.current = null;
    fadeDraftRef.current = null;
    setFadeDraft(null);
    setHover(null);
  }, [editable, fadeEditable]);
  useEffect(() => {
    fadeDragRef.current = null;
    fadeDraftRef.current = null;
    setFadeDraft(null);
    setHover(null);
  }, [fadeContextKey, fadeSignature]);
  const shown = useMemo(
    () => displayedEnvelopePoints(activePoints, durationMs),
    [activePoints, durationMs],
  );
  const splitShown = useMemo(() => splitEnvelopeAtUnity(shown), [shown]);
  const safeWidth = Math.max(1, width);
  const safeHeight = Math.max(1, height);
  const visibleSpan = Math.max(1, view.endMs - view.startMs);
  const xAt = (timeMs: number) => ((timeMs - view.startMs) / visibleSpan) * safeWidth;
  const yAt = (gain: number) => envelopeGainY(gain, safeHeight);
  const activeFade = useMemo(() => fade ? {
    ...fade,
    fadeInMs: fadeDraft?.direction === "in" ? fadeDraft.durationMs : fade.fadeInMs,
    fadeOutMs: fadeDraft?.direction === "out" ? fadeDraft.durationMs : fade.fadeOutMs,
  } : null, [fade, fadeDraft]);
  const fadeWindows = useMemo(
    () => activeFade ? audioFadeWindows(activeFade, durationMs) : [],
    [activeFade, durationMs],
  );
  const coords = (event: ReactPointerEvent<SVGSVGElement>): { x: number; y: number; timeMs: number; gain: number } => {
    const rect = event.currentTarget.getBoundingClientRect();
    const x = Math.max(0, Math.min(safeWidth, event.clientX - rect.left));
    const y = Math.max(0, Math.min(safeHeight, event.clientY - rect.top));
    return {
      x,
      y,
      timeMs: Math.max(0, Math.min(durationMs, view.startMs + (x / safeWidth) * visibleSpan)),
      gain: envelopeGainFromY(y, safeHeight),
    };
  };
  const emitCommit = (next: GainEnvelopePoint[]) => {
    const unchanged = next.length === sourcePoints.length && next.every((point, index) =>
      point.time_ms === sourcePoints[index].time_ms && point.gain === sourcePoints[index].gain);
    if (unchanged) {
      setMaxReached(sourcePoints.length >= GAIN_ENVELOPE_MAX_POINTS);
      return;
    }
    setMaxReached(false);
    setDraft(next);
    onCommit?.(next.length ? next : null);
  };

  const beginDrag = (event: ReactPointerEvent<SVGElement>, kind: DragState["kind"], index: number) => {
    if (!editable || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const svg = svgRef.current;
    if (!svg) return;
    const rect = svg.getBoundingClientRect();
    dragRef.current = {
      kind,
      index,
      startX: event.clientX - rect.left,
      startY: event.clientY - rect.top,
      startGain: envelopeGainFromY(event.clientY - rect.top, safeHeight),
      original: [...activePoints],
      moved: false,
    };
    try { svg.setPointerCapture(event.pointerId); } catch { /* Older browsers may lack capture. */ }
    setHover(null);
  };

  const onPointerMove = (event: ReactPointerEvent<SVGSVGElement>) => {
    const position = coords(event);
    const fadeDrag = fadeDragRef.current;
    if (fadeDrag && activeFade) {
      event.preventDefault();
      event.stopPropagation();
      const moved = fadeDrag.moved || Math.hypot(position.x - fadeDrag.startX, position.y - fadeDrag.startY) >= 2;
      if (!moved) return;
      const rate = activeFade.rate && activeFade.rate > 0 ? activeFade.rate : 1;
      const rawDuration = clampFadeDurationFromHandle(fadeDrag.direction, fadeDrag.boundaryMs, position.timeMs, rate);
      const maxSourceSpan = fadeDrag.direction === "in"
        ? Math.max(0, activeFade.trimEndMs - fadeDrag.boundaryMs)
        : Math.max(0, fadeDrag.boundaryMs - activeFade.trimStartMs);
      const fadeDurationMs = Math.min(rawDuration, Math.round(maxSourceSpan / rate));
      fadeDrag.moved = fadeDurationMs !== fadeDrag.startDurationMs;
      const nextFadeDraft = { direction: fadeDrag.direction, durationMs: fadeDurationMs };
      fadeDraftRef.current = nextFadeDraft;
      setFadeDraft(nextFadeDraft);
      const sourceMs = fadeDrag.direction === "in"
        ? fadeDrag.boundaryMs + fadeDurationMs * rate
        : fadeDrag.boundaryMs - fadeDurationMs * rate;
      const previewFade = {
        ...activeFade,
        ...(fadeDrag.direction === "in" ? { fadeInMs: fadeDurationMs } : { fadeOutMs: fadeDurationMs }),
      };
      setHover({
        kind: "fade",
        ...position,
        timeMs: sourceMs,
        gain: effectiveGainAt(activePoints, sourceMs, audioFadeWindows(previewFade, durationMs)),
        fadeDirection: fadeDrag.direction,
      });
      return;
    }
    const drag = dragRef.current;
    if (!drag) {
      const hit = hitTestEnvelope(activePoints, position.x, position.y, {
        startMs: view.startMs,
        endMs: view.endMs,
        width: safeWidth,
        height: safeHeight,
      });
      const fadeHandle = fadeWindows.find((window) => {
        const time = Math.max(0, Math.min(durationMs, window.direction === "in" ? window.endMs : window.startMs));
        const x = xAt(time);
        const y = yAt(effectiveGainAt(activePoints, time, fadeWindows));
        return isFadeHandleHit(position.x, position.y, x, y);
      });
      const fadeAt = fadeWindows.find((window) => position.timeMs >= window.startMs && position.timeMs <= window.endMs && window.durationMs > 0);
      const effectiveY = yAt(effectiveGainAt(activePoints, position.timeMs, fadeWindows));
      if (fadeHandle) {
        setHover({ kind: "fade", ...position, gain: effectiveGainAt(activePoints, position.timeMs, fadeWindows), fadeDirection: fadeHandle.direction });
      } else if (fadeAt && Math.abs(position.y - effectiveY) <= 14 && hit?.kind !== "point") {
        setHover({ kind: "fade", ...position, gain: effectiveGainAt(activePoints, position.timeMs, fadeWindows), fadeDirection: fadeAt.direction });
      } else if (hit) {
        const gain = envelopeGainAt(activePoints, position.timeMs);
        setHover({ kind: "gain", ...position, gain });
      } else setHover(null);
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    const moved = drag.moved || Math.hypot(position.x - drag.startX, position.y - drag.startY) >= 2;
    if (!moved) return;
    drag.moved = true;
    const next = drag.kind === "point"
      ? moveEnvelopePoint(drag.original, drag.index, position.timeMs, position.gain, durationMs)
      : drag.index >= 0
        ? moveEnvelopeSegment(drag.original, drag.index, position.gain - drag.startGain)
        : addEnvelopePoint(drag.original, position.timeMs, position.gain, durationMs);
    setDraft(next);
    setHover({ kind: "gain", ...position, gain: drag.kind === "segment" && drag.index >= 0
      ? envelopeGainAt(next, position.timeMs)
      : position.gain });
  };

  const beginFadeDrag = (event: ReactPointerEvent<SVGElement>, direction: FadeDirection) => {
    if (!editable || !fadeEditable || !activeFade || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const boundaryMs = fadeHandleSourceMs(activeFade, direction);
    const startDurationMs = direction === "in" ? activeFade.fadeInMs ?? 0 : activeFade.fadeOutMs ?? 0;
    const rect = svgRef.current?.getBoundingClientRect();
    fadeDragRef.current = {
      direction,
      boundaryMs,
      startDurationMs,
      startX: rect ? event.clientX - rect.left : 0,
      startY: rect ? event.clientY - rect.top : 0,
      moved: false,
    };
    const nextFadeDraft = { direction, durationMs: startDurationMs };
    fadeDraftRef.current = nextFadeDraft;
    setFadeDraft(nextFadeDraft);
    const svg = svgRef.current;
    if (svg) {
      try { svg.setPointerCapture(event.pointerId); } catch { /* Older browsers may lack capture. */ }
    }
    setHover(null);
  };

  const onPointerUp = (event: ReactPointerEvent<SVGSVGElement>) => {
    const fadeDrag = fadeDragRef.current;
    if (fadeDrag) {
      event.preventDefault();
      event.stopPropagation();
      const next = fadeDraftRef.current;
      fadeDragRef.current = null;
      fadeDraftRef.current = null;
      setFadeDraft(null);
      const committed = next ? fadeDragCommit(fadeDrag.startDurationMs, next.durationMs, fadeDrag.moved) : null;
      if (committed != null) onFadeDurationChange?.(next!.direction, committed);
      setHover(null);
      return;
    }
    const drag = dragRef.current;
    if (!drag) return;
    event.preventDefault();
    event.stopPropagation();
    const position = coords(event);
    let next = activePoints;
    const isAdd = drag.kind === "segment" && (!drag.moved || drag.index < 0);
    if (isAdd) {
      const previousAdd = lastAddRef.current;
      const duplicateDoubleClick = previousAdd != null && event.timeStamp - previousAdd.time < 350 && Math.hypot(position.x - previousAdd.x, position.y - previousAdd.y) < 8;
      if (duplicateDoubleClick) {
        dragRef.current = null;
        setDraft(null);
        return;
      }
    }
    if (drag.kind === "segment" && !drag.moved) {
      next = addEnvelopePoint(drag.original, position.timeMs, position.gain, durationMs);
    } else if (drag.kind === "segment" && drag.index < 0 && drag.moved) {
      next = addEnvelopePoint(drag.original, position.timeMs, position.gain, durationMs);
    }
    dragRef.current = null;
    if (drag.moved || (drag.kind === "segment" && !drag.moved)) emitCommit(next);
    if (isAdd && next !== drag.original) lastAddRef.current = { time: event.timeStamp, x: position.x, y: position.y };
    setHover({ kind: "gain", ...position, gain: envelopeGainAt(next, position.timeMs) });
  };

  const onPointerCancel = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (fadeDragRef.current) {
      event.preventDefault();
      event.stopPropagation();
      fadeDragRef.current = null;
      fadeDraftRef.current = null;
      setFadeDraft(null);
      setHover(null);
      return;
    }
    if (!dragRef.current) return;
    event.preventDefault();
    event.stopPropagation();
    dragRef.current = null;
    setDraft(null);
    setHover(null);
  };

  const onDoubleClick = (event: ReactUiMouseEvent<SVGSVGElement>) => {
    event.preventDefault();
    event.stopPropagation();
    if (!editable) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const hit = hitTestEnvelope(activePoints, event.clientX - rect.left, event.clientY - rect.top, {
      startMs: view.startMs,
      endMs: view.endMs,
      width: safeWidth,
      height: safeHeight,
    });
    if (hit?.kind !== "point") return;
    const next = resetEnvelopePointGain(activePoints, hit.index);
    if (next !== activePoints) emitCommit(next);
  };

  const onContextMenuPoint = (event: ReactMouseEvent<SVGCircleElement>, index: number) => {
    if (!editable) return;
    event.preventDefault();
    event.stopPropagation();
    const next = deleteEnvelopePoint(activePoints, index);
    if (next !== activePoints) emitCommit(next);
  };

  if (durationMs <= 0 || safeWidth <= 1 || safeHeight <= 1 || shown.length < 2) return null;

  const hasRenderedFade = fadeWindows.some((window) => window.durationMs > 0 && window.endMs > window.startMs);
  const fadeReadoutWidths = fadeWindows.map((window) => {
    const inDirection = window.direction === "in";
    const label = `${locale === "ru" ? (inDirection ? "Фейд входа" : "Фейд выхода") : (inDirection ? "Fade in" : "Fade out")} ${formatAudioFadeDuration(window.durationMs, locale)}`;
    return Math.min(Math.max(1, safeWidth - 4), Math.max(52, label.length * 5 + 10));
  });
  const stackFadeReadouts = fadeReadoutWidths.length > 1
    && fadeReadoutWidths.reduce((sum, width) => sum + width, 0) + 8 > safeWidth - 4;
  const fadeReadoutLaneHeight = fadeReadoutWidths.length === 0 ? 0 : stackFadeReadouts ? 35 : 18;
  const canShowGainLabels = !fadeReadoutLaneHeight || safeHeight >= fadeReadoutLaneHeight + 20;
  const effectiveSamples = Math.max(2, Math.ceil(safeWidth / 2) + 1);
  const effectivePath = hasRenderedFade
    ? Array.from({ length: effectiveSamples }, (_, index) => {
      const x = safeWidth * index / (effectiveSamples - 1);
      const timeMs = view.startMs + x / safeWidth * visibleSpan;
      return `${index === 0 ? "M" : "L"}${x.toFixed(2)},${yAt(effectiveGainAt(activePoints, timeMs, fadeWindows)).toFixed(2)}`;
    }).join(" ")
    : null;
  const hasAuthoredFade = !!activeFade && (activeFade.fadeInMs != null || activeFade.fadeOutMs != null);
  const fadeSummaryParts = activeFade ? [fadeNotice(activeFade, locale)] : [];
  if (editable && hasAuthoredFade && !fadeEditable) {
    fadeSummaryParts.push(locale === "ru" ? "Остановите cue, чтобы менять фейд" : "Stop the cue to edit fades");
  }
  const fadeSummary = fadeSummaryParts.filter(Boolean).join(" · ");

  return (
    <svg
      ref={svgRef}
      aria-label={t("editorUi.gainEnvelope")}
      viewBox={`0 0 ${safeWidth} ${safeHeight}`}
      preserveAspectRatio="none"
      style={{
        position: "absolute",
        inset: 0,
        width: "100%",
        height: safeHeight,
        overflow: "hidden",
        pointerEvents: "none",
        zIndex: 2,
      }}
      onPointerMove={editable ? onPointerMove : undefined}
      onPointerUp={editable ? onPointerUp : undefined}
      onPointerCancel={editable ? onPointerCancel : undefined}
      onClick={editable ? (event) => event.stopPropagation() : undefined}
      onDoubleClick={editable ? onDoubleClick : undefined}
      onPointerLeave={editable ? () => { if (!dragRef.current && !fadeDragRef.current) setHover(null); } : undefined}
    >
      {effectivePath && editable && (
        <path
          d={effectivePath}
          fill="none"
          stroke="transparent"
          strokeWidth={12}
          pointerEvents="stroke"
          onPointerDown={(event) => event.stopPropagation()}
        />
      )}
      {splitShown.slice(0, -1).map((point, index) => {
        const next = splitShown[index + 1];
        const fromIndex = activePoints.indexOf(point);
        const toIndex = activePoints.indexOf(next);
        const crossingSegment = point.gain === 1 || next.gain === 1;
        const segmentIndex = fromIndex >= 0 && toIndex === fromIndex + 1
          ? fromIndex
          : crossingSegment && fromIndex >= 0 && fromIndex < activePoints.length - 1
            && (activePoints[fromIndex].gain - 1) * (activePoints[fromIndex + 1].gain - 1) < 0
            ? fromIndex
            : crossingSegment && toIndex > 0
              && (activePoints[toIndex - 1]?.gain - 1) * (activePoints[toIndex]?.gain - 1) < 0
              ? toIndex - 1
              : -1;
        const midTime = (point.time_ms + next.time_ms) / 2;
        const gain = envelopeGainAt(activePoints, midTime);
        const segmentWidth = Math.abs(xAt(next.time_ms) - xAt(point.time_ms));
        const hoveredSegment = hover != null && hover.timeMs >= point.time_ms && hover.timeMs <= next.time_ms;
        const displayLabel = `${locale === "ru" ? "Уровень" : "Gain"} ${envelopePercent(gain)}% · ${envelopeDb(gain)}`;
        const labelWidth = Math.min(Math.max(1, safeWidth - 4), Math.max(68, displayLabel.length * 5.2 + 12));
        const labelX = safeWidth <= labelWidth + 4
          ? safeWidth / 2
          : Math.max(labelWidth / 2 + 2, Math.min(safeWidth - labelWidth / 2 - 2, xAt(midTime)));
        const maxLabelY = fadeReadoutLaneHeight ? safeHeight - fadeReadoutLaneHeight - 5 : safeHeight - 5;
        const labelY = Math.max(10, Math.min(maxLabelY, yAt(gain) - 10));
        return (
          <g key={`gain-segment-${index}`}>
            <line
              x1={xAt(point.time_ms)} y1={yAt(point.gain)}
              x2={xAt(next.time_ms)} y2={yAt(next.gain)}
              stroke="transparent" strokeWidth={14} strokeLinecap="round"
              pointerEvents={editable ? "stroke" : "none"}
              style={editable ? { touchAction: "none", cursor: "ns-resize" } : undefined}
              onPointerDown={editable ? (event) => beginDrag(event, "segment", segmentIndex) : undefined}
            />
            <line
              x1={xAt(point.time_ms)} y1={yAt(point.gain)}
              x2={xAt(next.time_ms)} y2={yAt(next.gain)}
              stroke={effectivePath ? "rgba(248,113,113,.42)" : "#ef4444"} strokeWidth={strokeWidth} strokeLinecap="round"
              pointerEvents="none"
            />
            {canShowGainLabels && (alwaysShowLabels || segmentWidth > 70 || hoveredSegment) && (
              <g pointerEvents="none">
                <rect x={labelX - labelWidth / 2} y={Math.max(1, Math.min(safeHeight - 16, labelY - 11))} width={labelWidth} height={15} rx={4} fill="rgba(15,23,42,.9)" />
                <text x={labelX} y={labelY} textAnchor="middle" fill={effectivePath ? "#fecaca" : "#fca5a5"} fontSize={9} fontFamily="system-ui, sans-serif">{displayLabel}</text>
              </g>
            )}
          </g>
        );
      })}
      {effectivePath && <path d={effectivePath} fill="none" stroke="#ef4444" strokeWidth={Math.max(2, strokeWidth)} strokeLinecap="round" strokeLinejoin="round" pointerEvents="none" />}
      {activePoints.map((point, index) => (
        <g key={`gain-point-${index}`}>
          <circle
            cx={xAt(point.time_ms)} cy={yAt(point.gain)} r={10}
            fill="transparent" pointerEvents={editable ? "all" : "none"}
            style={editable ? { touchAction: "none", cursor: "ns-resize" } : undefined}
            onPointerDown={editable ? (event) => beginDrag(event, "point", index) : undefined}
            onContextMenu={editable ? (event) => onContextMenuPoint(event, index) : undefined}
          />
          <circle cx={xAt(point.time_ms)} cy={yAt(point.gain)} r={4.5} fill="#ef4444" stroke="#7f1d1d" strokeWidth={1.5} pointerEvents="none" />
        </g>
      ))}
      {activeFade && fadeWindows.map((window, fadeIndex) => {
        const boundary = fadeHandleSourceMs(activeFade, window.direction);
        const rawHandleTime = window.direction === "in" ? window.endMs : window.startMs;
        const handleTime = Math.max(0, Math.min(durationMs, rawHandleTime));
        const boundaryGain = effectiveGainAt(activePoints, boundary, fadeWindows);
        const handleGain = effectiveGainAt(activePoints, handleTime, fadeWindows);
        const boundaryX = xAt(boundary), boundaryY = yAt(boundaryGain);
        const handleX = xAt(handleTime), handleY = yAt(handleGain);
        const zero = window.durationMs === 0;
        const inDirection = window.direction === "in";
        const directionLabel = locale === "ru" ? (inDirection ? "Вход" : "Выход") : (inDirection ? "In" : "Out");
        const formattedDuration = formatAudioFadeDuration(window.durationMs, locale);
        const fadeLabel = `${locale === "ru" ? (inDirection ? "Фейд входа" : "Фейд выхода") : (inDirection ? "Fade in" : "Fade out")} ${formattedDuration}`;
        const readoutWidth = fadeReadoutWidths[fadeIndex] ?? Math.min(Math.max(1, safeWidth - 4), Math.max(52, fadeLabel.length * 5 + 10));
        const readoutX = stackFadeReadouts
          ? Math.max(2, (safeWidth - readoutWidth) / 2)
          : inDirection ? 2 : Math.max(2, safeWidth - readoutWidth - 2);
        // Keep fade durations in their own bottom lane. Gain-level labels
        // are clamped above this lane, so dragging a handle cannot cover them.
        const readoutY = safeHeight - 18 - (stackFadeReadouts ? fadeIndex * 17 : 0);
        return (
          <g key={`fade-handle-${window.direction}`} aria-label={fadeLabel}>
            {!zero && <rect x={boundaryX - 3} y={boundaryY - 3} width={6} height={6} fill="rgba(254,202,202,.75)" stroke="#7f1d1d" strokeWidth={1} pointerEvents="none" />}
            <rect
              x={handleX - FADE_HANDLE_HIT_RADIUS} y={handleY - FADE_HANDLE_HIT_RADIUS}
              width={FADE_HANDLE_HIT_RADIUS * 2} height={FADE_HANDLE_HIT_RADIUS * 2}
              fill="transparent" pointerEvents={editable && fadeEditable ? "all" : "none"}
              style={editable && fadeEditable ? { touchAction: "none", cursor: "ew-resize" } : undefined}
              onPointerDown={editable && fadeEditable ? (event) => beginFadeDrag(event, window.direction) : undefined}
            />
            {inDirection
              ? <rect x={handleX - 4} y={handleY - 4} width={8} height={8} rx={1} fill="#fb7185" stroke="#7f1d1d" strokeWidth={1.5} pointerEvents="none" />
              : <path d={`M${handleX - 5},${handleY - 4} L${handleX + 5},${handleY - 4} L${handleX},${handleY + 5} Z`} fill="#fb7185" stroke="#7f1d1d" strokeWidth={1.5} pointerEvents="none" />}
            <g pointerEvents="none">
              <rect x={readoutX} y={readoutY} width={readoutWidth} height={15} rx={4} fill="rgba(15,23,42,.94)" />
              <text x={readoutX + 5} y={readoutY + 11} fill="#fecaca" fontSize={9} fontFamily="system-ui, sans-serif">{directionLabel} {formattedDuration}</text>
            </g>
          </g>
        );
      })}
      {hover && (
        <g pointerEvents="none">
          {(() => {
            const fadeWindow = hover.fadeDirection
              ? fadeWindows.find((window) => window.direction === hover.fadeDirection)
              : undefined;
            const hoverText = hover.kind === "fade"
              ? `${locale === "ru" ? (hover.fadeDirection === "in" ? "Вход" : "Выход") : (hover.fadeDirection === "in" ? "In" : "Out")} ${formatAudioFadeDuration(fadeWindow?.durationMs ?? 0, locale)} · ${envelopePercent(fadeMultiplierAt(hover.timeMs, fadeWindows))}% → ${envelopePercent(hover.gain)}%`
              : `${locale === "ru" ? "Уровень" : "Gain"} · ${envelopePercent(hover.gain)}% · ${envelopeDb(hover.gain)}`;
            const boxWidth = Math.min(Math.max(1, safeWidth - 4), Math.max(80, hoverText.length * 5.5 + 12));
            const boxX = Math.max(2, Math.min(safeWidth - boxWidth - 2, hover.x + 8));
            const textX = Math.max(boxX + 5, Math.min(boxX + boxWidth - 5, hover.x + 12));
            const y = Math.max(14, Math.min(safeHeight - 5, hover.y - 9));
            return (
              <>
                <rect x={boxX} y={Math.max(2, y - 13)} width={boxWidth} height={17} rx={4} fill="rgba(15,23,42,.94)" />
                <text x={textX} y={y} fill="#fca5a5" fontSize={10} fontFamily="system-ui, sans-serif">{hoverText}</text>
              </>
            );
          })()}
        </g>
      )}
      {fadeSummary && (
        <g pointerEvents="none">
          <rect x={3} y={3} width={Math.min(safeWidth - 6, Math.max(174, fadeSummary.length * 5.4))} height={16} rx={4} fill="rgba(15,23,42,.86)" />
          <text x={8} y={14} fill="#fecaca" fontSize={8} fontFamily="system-ui, sans-serif">{fadeSummary.slice(0, Math.max(20, Math.floor((safeWidth - 18) / 4.8)))}</text>
        </g>
      )}
      {maxReached && (
        <g pointerEvents="none">
          <rect x={Math.max(2, (safeWidth - 174) / 2)} y={2} width={Math.min(174, safeWidth - 4)} height={18} rx={4} fill="rgba(15,23,42,.94)" />
          <text x={safeWidth / 2} y={15} textAnchor="middle" fill="#fca5a5" fontSize={10} fontFamily="system-ui, sans-serif">{t("editorUi.gainEnvelopeMaxPoints")}</text>
        </g>
      )}
    </svg>
  );
}
