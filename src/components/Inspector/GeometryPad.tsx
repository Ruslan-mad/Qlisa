import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { VideoGeometry } from "../../lib/types";
import { useLocale } from "../../i18n";
import { getMediaThumbnail } from "../../lib/commands";
import {
  geometryCropClipPath,
  geometryPanToOutputOffset,
  nextGeometryScaleWheelValue,
  outputOffsetToGeometryPan,
  type GeometryPanMode,
} from "./geometryOutputModel";

export type GeometryGestureHandle = { flush: () => void; cancel: () => void };

export function GeometryPad({
  geometry,
  mode,
  sourceAspect,
  outputAspect,
  estimatedAspect = false,
  contentLabel,
  mediaPath,
  mediaSourceRevision,
  seekInto = false,
  textFont = "Arial",
  textFontSize = 60,
  textColor = "#FFFFFF",
  contextKey,
  anchor = { x: 0.5, y: 0.5 },
  isOverlay = false,
  hideRotation = false,
  disabled = false,
  gestureHandleRef,
  syncRevision = 0,
  onPreview = () => undefined,
  onCommit,
}: {
  geometry: VideoGeometry;
  mode: GeometryPanMode;
  sourceAspect: number;
  outputAspect: number;
  estimatedAspect?: boolean;
  contentLabel: string;
  mediaPath?: string | null;
  mediaSourceRevision?: string;
  seekInto?: boolean;
  textFont?: string;
  textFontSize?: number;
  textColor?: string;
  contextKey: string;
  anchor?: { x: number; y: number };
  isOverlay?: boolean;
  hideRotation?: boolean;
  disabled?: boolean;
  gestureHandleRef?: React.MutableRefObject<GeometryGestureHandle | null>;
  syncRevision?: number;
  onPreview?: (partial: Partial<VideoGeometry>) => void;
  onCommit: (partial: Partial<VideoGeometry>) => void;
}) {
  const { locale } = useLocale();
  const stageRef = useRef<HTMLDivElement>(null);
  const elementRef = useRef<HTMLDivElement>(null);
  const draftRef = useRef(geometry);
  const dragRef = useRef<{ pointerId: number; x: number; y: number; start: VideoGeometry; preview: (patch: Partial<VideoGeometry>) => void; commit: (patch: Partial<VideoGeometry>) => void } | null>(null);
  const wheelTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingWheel = useRef<{ contextKey: string; scale: number; preview: (patch: Partial<VideoGeometry>) => void; commit: (patch: Partial<VideoGeometry>) => void } | null>(null);
  const draftFrame = useRef<number | null>(null);
  const [draft, setDraft] = useState(geometry);
  const [dragging, setDragging] = useState(false);
  const [thumbnail, setThumbnail] = useState<string | null>(null);
  const [stageSize, setStageSize] = useState({ width: 0, height: 0 });
  const labels = locale === "ru"
    ? { frame: "Кадр выхода", hint: estimatedAspect ? "Пропорции предпросмотра оценены" : "Перетащите содержимое; колесо меняет масштаб", move: "Положение", scale: "Масштаб" }
    : { frame: "Output frame", hint: estimatedAspect ? "Preview proportions are estimated" : "Drag the content; use the wheel to scale", move: "Position", scale: "Scale" };

  useEffect(() => {
    if (sameGeometry(draftRef.current, geometry)) return;
    if (dragRef.current || pendingWheel.current) return;
    gestureHandleRef?.current?.cancel();
    draftRef.current = geometry;
    setDraft(geometry);
  }, [geometry, syncRevision]);

  const flushPendingScale = useCallback(() => {
    if (wheelTimer.current) clearTimeout(wheelTimer.current);
    wheelTimer.current = null;
    const pending = pendingWheel.current;
    pendingWheel.current = null;
    if (pending) pending.commit({ scale: pending.scale });
  }, []);

  useEffect(() => {
    if (!gestureHandleRef) return;
    const handle: GeometryGestureHandle = {
      flush: () => {
        flushPendingScale();
        const drag = dragRef.current;
        if (!drag) return;
        dragRef.current = null;
        setDragging(false);
        drag.commit({ pan_x: draftRef.current.pan_x, pan_y: draftRef.current.pan_y });
      },
      cancel: () => {
        if (wheelTimer.current) clearTimeout(wheelTimer.current);
        wheelTimer.current = null;
        pendingWheel.current = null;
        dragRef.current = null;
        if (draftFrame.current != null) cancelAnimationFrame(draftFrame.current);
        draftFrame.current = null;
        draftRef.current = geometry;
        setDraft(geometry);
        setDragging(false);
      },
    };
    gestureHandleRef.current = handle;
    return () => { if (gestureHandleRef.current === handle) gestureHandleRef.current = null; };
  }, [flushPendingScale, gestureHandleRef, geometry]);

  useEffect(() => () => flushPendingScale(), [contextKey, flushPendingScale]);
  useEffect(() => {
    setDragging(false);
    return () => {
      dragRef.current = null;
      if (draftFrame.current != null) cancelAnimationFrame(draftFrame.current);
      draftFrame.current = null;
    };
  }, [contextKey]);
  useEffect(() => () => {
    if (draftFrame.current != null) cancelAnimationFrame(draftFrame.current);
  }, []);

  useEffect(() => {
    let stale = false;
    setThumbnail(null);
    if (mediaPath) getMediaThumbnail(mediaPath, seekInto).then((asset) => { if (!stale) setThumbnail(asset); }).catch(() => undefined);
    return () => { stale = true; };
  }, [mediaPath, mediaSourceRevision, seekInto]);

  useEffect(() => {
    const stage = stageRef.current;
    if (!stage || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      setStageSize({ width, height });
    });
    observer.observe(stage);
    return () => observer.disconnect();
  }, []);

  const scheduleDraft = (next: VideoGeometry) => {
    draftRef.current = next;
    if (draftFrame.current != null) return;
    draftFrame.current = requestAnimationFrame(() => {
      draftFrame.current = null;
      setDraft(draftRef.current);
    });
  };

  useEffect(() => {
    const element = elementRef.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      if (disabled || (mode === "mpv" && draftRef.current.fit_mode === "stretch")) return;
      if (event.deltaY === 0) return;
      event.preventDefault();
      const previous = draftRef.current;
      const nextScale = nextGeometryScaleWheelValue(previous.scale, event.deltaY);
      const next = { ...previous, scale: nextScale };
      scheduleDraft(next);
      onPreview({ scale: next.scale });
      pendingWheel.current = { contextKey, scale: next.scale, preview: onPreview, commit: onCommit };
      if (wheelTimer.current) clearTimeout(wheelTimer.current);
      wheelTimer.current = setTimeout(flushPendingScale, 180);
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [contextKey, disabled, flushPendingScale, mode, onCommit, onPreview]);

  const unsupportedStretch = mode === "mpv" && draft.fit_mode === "stretch";
  const offset = geometryPanToOutputOffset(draft, mode, sourceAspect, outputAspect);
  const cropClipPath = geometryCropClipPath(draft);
  const rawAspect = Number.isFinite(sourceAspect) && sourceAspect > 0 ? sourceAspect : outputAspect;
  const baseAspect = rawAspect;
  const layoutAspect = draft.rotation % 180 === 90 && mode === "mpv" ? 1 / baseAspect : baseAspect;
  const frameHeight = 100;
  let baseWidth = 1;
  let baseHeight = 1;
  {
    if (draft.fit_mode === "fit") {
      if (layoutAspect > outputAspect) baseHeight = outputAspect / layoutAspect;
      else baseWidth = layoutAspect / outputAspect;
    } else if (draft.fit_mode === "fill") {
      if (layoutAspect > outputAspect) baseWidth = layoutAspect / outputAspect;
      else baseHeight = outputAspect / layoutAspect;
    }
  }
  const left = anchor.x * 100 + offset.x * 100;
  const top = anchor.y * 100 + offset.y * frameHeight;
  const lines = contentLabel.split("\n");
  const overlayFontPx = stageSize.height > 0 ? textFontSize * stageSize.height / 720 : 0;
  const textBounds = useMemo(() => measureTextBounds(lines, textFont, overlayFontPx), [contentLabel, textFont, overlayFontPx]);
  const mediaWidth = isOverlay && stageSize.width > 0 ? textBounds.width / stageSize.width : isOverlay ? 0.3 : baseWidth;
  const mediaHeight = isOverlay && stageSize.height > 0 ? textBounds.height / stageSize.height : isOverlay ? 0.08 : baseHeight;
  const rotated = draft.rotation % 180 === 90 && mode === "mpv";
  const cssWidth = rotated ? mediaHeight / outputAspect : mediaWidth;
  const cssHeight = rotated ? mediaWidth * outputAspect : mediaHeight;
  const angle = (draft.rotation * Math.PI) / 180;
  const rotationTransform = hideRotation
    ? ""
    : mode === "output"
      ? ` matrix(${Math.cos(angle)}, ${Math.sin(angle) / outputAspect}, ${-Math.sin(angle) * outputAspect}, ${Math.cos(angle)}, 0, 0)`
      : ` rotate(${draft.rotation}deg)`;
  const style: React.CSSProperties = {
    position: "absolute",
    left: `${left}%`,
    top: `${top}%`,
    width: `${cssWidth * 100}%`,
    height: `${cssHeight * 100}%`,
    overflow: "hidden",
    border: "1px solid var(--wc-accent)",
    background: isOverlay || cropClipPath ? "transparent" : "#151923",
    color: "var(--wc-text)",
    boxSizing: "border-box",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    touchAction: "none",
    cursor: disabled ? "default" : dragging ? "grabbing" : "grab",
    transform: `translate(${-anchor.x * 100}%, ${-anchor.y * 100}%) scale(${unsupportedStretch ? 1 : draft.scale})${rotationTransform}`,
    transformOrigin: `${anchor.x * 100}% ${anchor.y * 100}%`,
  };
  const innerMediaStyle: React.CSSProperties = {
    position: "absolute",
    left: 0,
    top: 0,
    width: "100%",
    height: "100%",
    clipPath: cropClipPath,
    objectFit: draft.fit_mode === "stretch" ? "fill" : draft.fit_mode === "fill" ? "cover" : "contain",
  };
  const setPanFromDrag = (dx: number, dy: number) => {
    if (mode === "mpv" && dragRef.current?.start.fit_mode === "stretch") return;
    const start = dragRef.current?.start ?? draftRef.current;
    const nextPan = outputOffsetToGeometryPan(dx, dy, start, mode, sourceAspect, outputAspect);
    const next = { ...start, ...nextPan };
    scheduleDraft(next);
    dragRef.current?.preview(nextPan);
  };

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (disabled || (mode === "mpv" && draftRef.current.fit_mode === "stretch") || event.button !== 0 || !elementRef.current?.contains(event.target as Node)) return;
    const stage = stageRef.current;
    if (!stage) return;
    event.preventDefault();
    dragRef.current = {
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      start: draftRef.current,
      preview: onPreview,
      commit: onCommit,
    };
    stage.setPointerCapture(event.pointerId);
    setDragging(true);
  };
  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    const stage = stageRef.current;
    if (!drag || !stage || event.pointerId !== drag.pointerId) return;
    const rect = stage.getBoundingClientRect();
    setPanFromDrag((event.clientX - drag.x) / rect.width, (event.clientY - drag.y) / rect.height);
  };
  const finishDrag = (event: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || event.pointerId !== drag.pointerId) return;
    dragRef.current = null;
    setDragging(false);
    if (draftRef.current.pan_x !== drag.start.pan_x || draftRef.current.pan_y !== drag.start.pan_y) {
      drag.commit({ pan_x: draftRef.current.pan_x, pan_y: draftRef.current.pan_y });
    }
  };
  const cancelDrag = (event: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || event.pointerId !== drag.pointerId) return;
    dragRef.current = null;
    setDragging(false);
    if (draftFrame.current != null) cancelAnimationFrame(draftFrame.current);
    draftFrame.current = null;
    draftRef.current = drag.start;
    setDraft(drag.start);
    drag.commit({ pan_x: drag.start.pan_x, pan_y: drag.start.pan_y });
  };

  return (
    <div style={{ marginBottom: 12 }}>
      <div style={{ display: "flex", justifyContent: "space-between", gap: 8, fontSize: 11, color: "var(--wc-text-muted)", marginBottom: 5 }}>
        <span>{labels.frame}</span>
        <span>{unsupportedStretch
          ? (locale === "ru" ? "Для перемещения выберите «Вписать» или «Обрезать»" : "Choose Fit or Fill to position this source")
          : labels.hint}</span>
      </div>
      <div
        ref={stageRef}
        aria-label={labels.frame}
        style={{
          position: "relative",
          width: "100%",
          maxWidth: `${190 * outputAspect}px`,
          aspectRatio: `${outputAspect} / 1`,
          overflow: "hidden",
          background: "#05070b",
          border: "1px solid var(--wc-border-strong)",
          borderRadius: 4,
          boxSizing: "border-box",
          marginInline: "auto",
          touchAction: "none",
          userSelect: "none",
        }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={finishDrag}
        onPointerCancel={cancelDrag}
      >
        <div
          ref={elementRef}
          style={style}
          title={contentLabel}
          aria-label={contentLabel}
        >
          {isOverlay ? (
            <span style={{ position: "absolute", left: 0, top: 0, width: "100%", textAlign: anchor.x < 0.25 ? "left" : anchor.x > 0.75 ? "right" : "center", whiteSpace: "pre", fontFamily: textFont, fontSize: overlayFontPx || 12, color: textColor, lineHeight: 1.15, textShadow: "0 1px 2px #000" }}>{contentLabel}</span>
          ) : thumbnail ? (
            <img src={thumbnail} alt="" draggable={false} style={innerMediaStyle} />
          ) : (
            <span style={{ position: "relative", zIndex: 1, padding: 6, fontSize: 11, textAlign: "center", overflow: "hidden", textOverflow: "ellipsis", maxWidth: "100%", clipPath: cropClipPath }}>{contentLabel}</span>
          )}
        </div>
      </div>
    </div>
  );
}

function measureTextBounds(lines: string[], font: string, fontSize: number): { width: number; height: number } {
  const lineHeight = Math.max(fontSize * 1.15, 1);
  const canvas = typeof document === "undefined" ? null : document.createElement("canvas");
  const context = canvas?.getContext("2d");
  if (!context || fontSize <= 0) {
    const longest = Math.max(1, ...lines.map((line) => line.length));
    return { width: longest * lineHeight * 0.58, height: Math.max(1, lines.length) * lineHeight };
  }
  context.font = `${fontSize}px "${font.replace(/["\\]/g, "")}"`;
  const width = Math.max(1, ...lines.map((line) => context.measureText(line || " ").width));
  return { width: width + lineHeight * 0.18, height: Math.max(1, lines.length) * lineHeight };
}

function sameGeometry(left: VideoGeometry, right: VideoGeometry): boolean {
  return Object.keys(left).every((key) => left[key as keyof VideoGeometry] === right[key as keyof VideoGeometry]);
}
