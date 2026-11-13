// Geometry tab for visual cues: fit mode, position, scale, rotation and crop.
// Edits apply live when the cue is on the output window. Compositing controls
// (layer order / opacity / blend) live in the Layer tab.

import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import type { CameraCueData, FitMode, ImageCueData, OutputDestination, ScreenInfo, TextCueData, TimerCueData, VideoCueData, VideoGeometry } from "../../lib/types";
import { DEFAULT_GEOMETRY } from "../../lib/types";
import { Grid2, MiniField, NumberInput, Section, Segmented, SliderRow } from "./Field";
import { useLocale } from "../../i18n";
import type { ValueState } from "./multiCueModel";
import { getCue, listVideoScreens } from "../../lib/commands";
import { GeometryPad, type GeometryGestureHandle } from "./GeometryPad";
import { sliderPositionToValue, sliderValueToPosition } from "./sliderScaleModel";
import { advanceGeometryCenterDetent, geometryEffectivePointerX, geometrySliderPositionFromPointer, geometryValueAtDetentPosition, idleGeometryCenterDetent, releaseGeometryCenterDetent, type GeometryCenterDetentState } from "./geometrySnapModel";
import { cancelGeometryAxisGesture, consumeExpectedGeometryEcho, cropDisplayToGeometry, cropGeometryToDisplay, formatGeometryPanPercent, geometryLinkTransition, geometryOverridesForCue, geometrySnapshotsMatch, mergeGeometryOutputOverride, nextGeometryAxisKeyboardValue, nextGeometryAxisWheelValue, nextGeometryRotationWheelValue, nextGeometryScaleWheelValue, outputAspectRatio, quantizeGeometryPan, type CueGeometryOverridesSnapshot, type GeometryPanMode } from "./geometryOutputModel";
import { GeometryLiveQueue, type LiveGeometryPatch } from "./geometryLiveQueue";

const FIT_MODES: { value: FitMode; label: string; hint: string }[] = [
  { value: "fit", label: "Fit", hint: "keep aspect, letterbox" },
  { value: "fill", label: "Fill", hint: "keep aspect, crop overflow" },
  { value: "stretch", label: "Stretch", hint: "ignore aspect ratio" },
];

export type GeometryEditorState = {
  [K in keyof VideoGeometry]: ValueState<VideoGeometry[K]>;
};
type GeometryCue = VideoCueData | ImageCueData | CameraCueData | TextCueData | TimerCueData;
type GeometryNumberKey = Exclude<keyof VideoGeometry, "fit_mode">;
type GeometryCropKey = "crop_left" | "crop_right" | "crop_top" | "crop_bottom";

function isCameraCue(cue: GeometryCue): cue is CameraCueData { return cue.cue_type === "camera"; }
function isTextCue(cue: GeometryCue): cue is TextCueData { return cue.cue_type === "text"; }
function isTimerCue(cue: GeometryCue): cue is TimerCueData { return cue.cue_type === "timer"; }

function stateValue<T>(state: ValueState<T>): T | null {
  return state.kind === "uniform" ? state.value : null;
}

function uniform<T>(value: T): ValueState<T> {
  return { kind: "uniform", value };
}

export function snapRotationToCardinal(value: number, toleranceDegrees = 4): number {
  let nearest = value;
  let distance = toleranceDegrees;
  for (const cardinal of [0, 90, 180, 270, 360]) {
    const nextDistance = Math.abs(value - cardinal);
    if (nextDistance <= distance) {
      nearest = cardinal;
      distance = nextDistance;
    }
  }
  return nearest;
}

export function GeometryEditor({
  geometry,
  disabled = false,
  mixedLabel,
  commitSlidersOnRelease = false,
  resetDisabled = false,
  hideFit = false,
  hidePosition = false,
  hideScale = false,
  hideRotationCrop = false,
  scaleDisabled = false,
  onPatch,
  onReset,
}: {
  geometry: GeometryEditorState;
  disabled?: boolean;
  mixedLabel?: string;
  commitSlidersOnRelease?: boolean;
  resetDisabled?: boolean;
  hideFit?: boolean;
  hidePosition?: boolean;
  hideScale?: boolean;
  hideRotationCrop?: boolean;
  scaleDisabled?: boolean;
  onPatch: (partial: Partial<VideoGeometry>) => void;
  onReset: () => void;
}) {
  const { t, locale } = useLocale();
  const fitLabels: Record<FitMode, string> = locale === "ru" ? { fit: "Вписать", fill: "Вписать с обрезкой", stretch: "Растянуть" } : { fit: "Fit", fill: "Fill", stretch: "Stretch" };
  const fitHints: Record<FitMode, string> = locale === "ru" ? { fit: "сохранить пропорции, добавить поля", fill: "сохранить пропорции, обрезать выходящее за границы", stretch: "игнорировать соотношение сторон" } : { fit: "keep aspect, letterbox", fill: "keep aspect, crop overflow", stretch: "ignore aspect ratio" };
  const fitMode = stateValue(geometry.fit_mode);
  const rotationPointerActive = useRef(false);
  const scalePointerRef = useRef({
    active: false,
    previousX: 0,
    currentX: 0,
    detent: idleGeometryCenterDetent(),
  });
  const beginScalePointer = (event: React.PointerEvent<HTMLDivElement>) => {
    const input = event.target instanceof HTMLInputElement ? event.target : null;
    if (!input) return;
    const rect = input.getBoundingClientRect();
    const centerX = rect.left + rect.width / 2;
    const activeTrackWidth = Math.max(1, rect.width - 16);
    const position = event.clientX;
    scalePointerRef.current = {
      active: true,
      previousX: position,
      currentX: position,
      detent: advanceGeometryCenterDetent(idleGeometryCenterDetent(), position, position, centerX, activeTrackWidth),
    };
    input.setPointerCapture(event.pointerId);
  };
  const moveScalePointer = (event: React.PointerEvent<HTMLDivElement>) => {
    const current = scalePointerRef.current;
    if (!current.active) return;
    const input = event.target instanceof HTMLInputElement ? event.target : null;
    if (!input) return;
    const rect = input.getBoundingClientRect();
    const centerX = rect.left + rect.width / 2;
    const activeTrackWidth = Math.max(1, rect.width - 16);
    const nextX = event.clientX;
    scalePointerRef.current = {
      active: true,
      previousX: current.currentX,
      currentX: nextX,
      detent: advanceGeometryCenterDetent(current.detent, current.currentX, nextX, centerX, activeTrackWidth),
    };
  };
  const endScalePointer = (event: React.PointerEvent<HTMLDivElement>) => {
    const current = scalePointerRef.current;
    if (!current.active) return;
    const input = event.target instanceof HTMLInputElement ? event.target : null;
    if (!input) { scalePointerRef.current = { active: false, previousX: 0, currentX: 0, detent: idleGeometryCenterDetent() }; return; }
    const rect = input.getBoundingClientRect();
    const centerX = rect.left + rect.width / 2;
    const activeTrackWidth = Math.max(1, rect.width - 16);
    const finalX = event.clientX;
    const moved = advanceGeometryCenterDetent(current.detent, current.currentX, finalX, centerX, activeTrackWidth);
    scalePointerRef.current = {
      active: true,
      previousX: current.currentX,
      currentX: finalX,
      detent: releaseGeometryCenterDetent(moved, finalX, centerX, activeTrackWidth),
    };
  };
  const cancelScalePointer = () => { scalePointerRef.current = { active: false, previousX: 0, currentX: 0, detent: idleGeometryCenterDetent() }; };
  const numberField = (key: GeometryNumberKey, step: number, min: number, max: number) => (
    <NumberInput
      value={stateValue(geometry[key])}
      placeholder={geometry[key].kind === "mixed" ? mixedLabel : undefined}
      disabled={disabled}
      step={step}
      min={min}
      max={max}
      onCommit={(value) => onPatch({ [key]: key === "rotation" ? Math.round(value) : value })}
    />
  );
  const cropNumberField = (key: GeometryCropKey) => (
    <NumberInput
      value={stateValue(geometry[key]) == null ? null : cropGeometryToDisplay(stateValue(geometry[key])!)}
      placeholder={geometry[key].kind === "mixed" ? mixedLabel : undefined}
      disabled={disabled}
      step={1}
      min={0}
      max={100}
      onCommit={(value) => onPatch({ [key]: cropDisplayToGeometry(value) })}
    />
  );

  return (
    <>
      {!hideFit && <Section title="Fit" hint={t("help.outputSelection")}>
        <Segmented
          options={FIT_MODES.map((mode) => ({ ...mode, label: fitLabels[mode.value], hint: fitHints[mode.value] }))}
          value={fitMode}
          disabled={disabled}
          onChange={(value) => onPatch({ fit_mode: value })}
        />
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginTop: -4, marginBottom: 6 }}>
          {fitMode ? fitHints[fitMode] : mixedLabel}
        </div>
      </Section>}

      {(!hidePosition || !hideScale || !hideRotationCrop) && <Section title={locale === "ru" ? "Положение и масштаб" : "Position & Scale"}>
        {!hidePosition && <Grid2>
          <MiniField label={locale === "ru" ? "Положение X (± ширины)" : "Position X (± of width)"}>
            {numberField("pan_x", 0.01, -1, 1)}
          </MiniField>
          <MiniField label={locale === "ru" ? "Положение Y (± высоты)" : "Position Y (± of height)"}>
            {numberField("pan_y", 0.01, -1, 1)}
          </MiniField>
        </Grid2>}
        {!hideScale && <div
          onPointerDownCapture={beginScalePointer}
          onPointerMoveCapture={moveScalePointer}
          onPointerUpCapture={endScalePointer}
          onPointerCancelCapture={cancelScalePointer}
          onLostPointerCapture={cancelScalePointer}
        ><SliderRow
          label={t("output.scale")}
          value={stateValue(geometry.scale)}
          mixedLabel={mixedLabel}
          disabled={disabled || scaleDisabled}
          commitOnRelease={commitSlidersOnRelease}
          min={0.05}
          max={8}
          step={0.05}
          format={(value) => `${Math.round(value * 100)}%`}
          centered={{ value: 1, snapTolerance: 0 }}
          onChange={(value) => {
            const pointer = scalePointerRef.current;
            const position = sliderValueToPosition(value, 0.05, 8, 1);
            onPatch({ scale: pointer.active ? geometryValueAtDetentPosition(position, 0.05, 8, 1, pointer.detent) : value });
          }}
        /></div>}
        {!hideRotationCrop && <div
          onPointerDownCapture={(event) => {
            rotationPointerActive.current = true;
            if (event.target instanceof HTMLInputElement) event.target.setPointerCapture(event.pointerId);
          }}
          onPointerUp={() => { rotationPointerActive.current = false; }}
          onPointerCancel={() => { rotationPointerActive.current = false; }}
          onLostPointerCapture={() => { rotationPointerActive.current = false; }}
        >
          <SliderRow
            label={t("output.rotation")}
            value={stateValue(geometry.rotation)}
            mixedLabel={mixedLabel}
            disabled={disabled}
            commitOnRelease={commitSlidersOnRelease}
            min={0}
            max={360}
            step={1}
            format={(value) => `${Math.round(value)}°`}
            onChange={(value) => onPatch({ rotation: Math.round(rotationPointerActive.current ? snapRotationToCardinal(value) : value) })}
          />
        </div>}
      </Section>}

      {!hideRotationCrop && <Section title={t("output.crop")} hint="0 – 100">
        <Grid2>
          <MiniField label={locale === "ru" ? "Слева" : "Left"}>{cropNumberField("crop_left")}</MiniField>
          <MiniField label={locale === "ru" ? "Справа" : "Right"}>{cropNumberField("crop_right")}</MiniField>
          <MiniField label={locale === "ru" ? "Сверху" : "Top"}>{cropNumberField("crop_top")}</MiniField>
          <MiniField label={locale === "ru" ? "Снизу" : "Bottom"}>{cropNumberField("crop_bottom")}</MiniField>
        </Grid2>
      </Section>}

      <button
        disabled={disabled || resetDisabled}
        onClick={onReset}
        style={{
          padding: "5px 14px",
          fontSize: 12,
          borderRadius: 4,
          border: "1px solid var(--wc-border-strong)",
          background: "var(--wc-bg-surface)",
          color: disabled || resetDisabled ? "var(--wc-text-faint)" : "var(--wc-text)",
          cursor: disabled || resetDisabled ? "default" : "pointer",
        }}
      >
        {t("actions.reset")}
      </button>
    </>
  );
}

export function FitByOutputRows({
  outputs,
  geometryByOutput,
  fallback,
  disabled = false,
  mixedOutputIds = [],
  mixedLabel,
  onSelect,
}: {
  outputs: Array<{ id: string; name: string }>;
  geometryByOutput: Record<string, VideoGeometry>;
  fallback: VideoGeometry;
  disabled?: boolean;
  mixedOutputIds?: readonly string[];
  mixedLabel?: string;
  onSelect: (outputId: string, fitMode: FitMode) => void;
}) {
  const { locale } = useLocale();
  const buttonStyle = (selected: boolean): CSSProperties => ({
    flex: 1, minWidth: 0, padding: "4px 3px", fontSize: 10,
    border: `1px solid ${selected ? "var(--wc-accent)" : "var(--wc-border)"}`,
    borderRadius: 4, background: selected ? "var(--wc-accent-soft)" : "var(--wc-bg-surface)",
    color: selected ? "var(--wc-text)" : "var(--wc-text-muted)", cursor: disabled ? "default" : "pointer",
    opacity: disabled ? 0.55 : 1,
  });
  const modeTitle: Record<FitMode, string> = locale === "ru"
    ? { fit: "Сохранить пропорции, добавить поля", fill: "Сохранить пропорции, обрезать выходящее за границы", stretch: "Игнорировать соотношение сторон" }
    : { fit: "Keep aspect ratio and add bars", fill: "Keep aspect ratio and crop overflow", stretch: "Ignore aspect ratio" };
  const mixed = new Set(mixedOutputIds);
  return <div style={{ display: "grid", gap: 6 }}>
    {outputs.map((output) => {
      const value = geometryByOutput[output.id] ?? fallback;
      const isMixed = mixed.has(output.id);
      return <div key={output.id} style={{ display: "grid", gridTemplateColumns: "minmax(64px, 0.8fr) 2fr", gap: 6, alignItems: "center" }}>
        <span title={output.name} style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 10, color: "var(--wc-text-muted)" }}>{output.name}</span>
        <div style={{ display: "flex", gap: 3 }}>
          {FIT_MODES.map((mode) => <button key={mode.value} type="button" disabled={disabled} aria-label={`${output.name}: ${localeLabel(mode.value, locale)}`} title={`${localeLabel(mode.value, locale)} — ${modeTitle[mode.value]}`} aria-pressed={!isMixed && value.fit_mode === mode.value} style={buttonStyle(!isMixed && value.fit_mode === mode.value)} onClick={() => onSelect(output.id, mode.value)}>{localeLabel(mode.value, locale)}</button>)}
          {isMixed && mixedLabel && <span role="status" style={{ alignSelf: "center", fontSize: 10, color: "var(--wc-text-muted)" }}>{mixedLabel}</span>}
        </div>
      </div>;
    })}
  </div>;
}

export function GeometryTab({
  cue,
  onSave,
  outputs = [],
  outputIds = [],
  sourceWidth,
  sourceHeight,
  cameraActive = false,
  overlayMargin = 0,
}: {
  cue: GeometryCue;
  onSave: (p: Partial<GeometryCue>) => void | Promise<void>;
  outputs?: OutputDestination[];
  outputIds?: string[];
  sourceWidth?: number | null;
  sourceHeight?: number | null;
  cameraActive?: boolean;
  overlayMargin?: number;
}) {
  const { locale } = useLocale();
  const geometry: VideoGeometry = cue.geometry ?? DEFAULT_GEOMETRY;
  const persistedOverrides = cue.geometry_by_output ?? {};
  const [localOverrides, setLocalOverrides] = useState<CueGeometryOverridesSnapshot>({ cueId: cue.id, values: persistedOverrides });
  const currentRef = useRef({ cueId: cue.id, geometry, overrides: persistedOverrides });
  const geometryQueueRef = useRef(new GeometryLiveQueue());
  const expectedEchoesRef = useRef<string[]>([]);
  const serverSnapshotRef = useRef({ cueId: cue.id, geometry, overrides: persistedOverrides });
  const [syncRevision, setSyncRevision] = useState(0);
  const [screens, setScreens] = useState<ScreenInfo[]>([]);
  const padGestureHandleRef = useRef<GeometryGestureHandle | null>(null);
  const controlsGestureHandleRef = useRef<GeometryGestureHandle | null>(null);
  const cropGestureHandleRef = useRef<GeometryGestureHandle | null>(null);
  const flushGestures = () => { padGestureHandleRef.current?.flush(); controlsGestureHandleRef.current?.flush(); cropGestureHandleRef.current?.flush(); };
  const cancelGestures = () => { padGestureHandleRef.current?.cancel(); controlsGestureHandleRef.current?.cancel(); cropGestureHandleRef.current?.cancel(); };
  useEffect(() => () => {
    flushGestures();
    void geometryQueueRef.current.flush();
  }, []);
  const [retriedCameraDimensions, setRetriedCameraDimensions] = useState<{ width: number; height: number } | undefined>();
  useEffect(() => { listVideoScreens().then(setScreens).catch(() => setScreens([])); }, []);
  const cameraSourceKey = isCameraCue(cue) ? JSON.stringify(cue.source) : "";
  useEffect(() => {
    setRetriedCameraDimensions(undefined);
    if (!isCameraCue(cue) || !cameraActive || cue._live_source_dimensions) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const deadline = Date.now() + 5000;
    const retry = async () => {
      while (!cancelled && Date.now() < deadline) {
        try {
          const latest = await getCue(cue.id);
          if (cancelled) return;
          const dimensions = (latest as CameraCueData)._live_source_dimensions;
          if (dimensions && typeof dimensions.width === "number" && dimensions.width > 0 && typeof dimensions.height === "number" && dimensions.height > 0) {
            setRetriedCameraDimensions(dimensions);
            return;
          }
        } catch { /* Keep the bounded retry local to the open Geometry tab. */ }
        await new Promise<void>((resolve) => { timer = setTimeout(resolve, 250); });
      }
    };
    void retry();
    return () => { cancelled = true; if (timer) clearTimeout(timer); };
  }, [cue.id, cameraActive, cameraSourceKey, isCameraCue(cue) ? cue._live_source_dimensions?.width : undefined, isCameraCue(cue) ? cue._live_source_dimensions?.height : undefined]);
  useEffect(() => {
    const incoming = { cueId: cue.id, geometry: cue.geometry ?? DEFAULT_GEOMETRY, overrides: cue.geometry_by_output ?? {} };
    const signature = geometrySnapshotSignature(incoming.geometry, incoming.overrides);
    const previous = serverSnapshotRef.current;
    if (previous.cueId === incoming.cueId && geometrySnapshotsMatch(geometrySnapshotSignature(previous.geometry, previous.overrides), signature)) return;
    serverSnapshotRef.current = incoming;
    if (consumeExpectedGeometryEcho(expectedEchoesRef.current, signature)) return;
    geometryQueueRef.current.cancelPending();
    cancelGestures();
    currentRef.current = incoming;
    setLocalOverrides({ cueId: incoming.cueId, values: incoming.overrides });
    setSyncRevision((revision) => revision + 1);
  }, [cue.id, cue.geometry, cue.geometry_by_output]);
  const activeOverrides = geometryOverridesForCue(localOverrides, cue.id, persistedOverrides);
  const linked = Object.keys(activeOverrides).length === 0;
  const [activeOutput, setActiveOutput] = useState(outputIds[0] ?? "");
  useEffect(() => {
    if (outputIds.length > 0 && !outputIds.includes(activeOutput)) setActiveOutput(outputIds[0]);
  }, [outputIds.join("\u0000"), activeOutput]);
  const selectedOutputs = outputIds.map((id) => ({ id, name: outputs.find((output) => output.id === id)?.name ?? id }));
  const active = selectedOutputs.find((output) => output.id === activeOutput) ?? selectedOutputs[0];
  const currentBase = currentRef.current.cueId === cue.id ? currentRef.current.geometry : geometry;
  const outputGeometry = (id: string) => activeOverrides[id] ?? currentBase;
  const activeGeometry = active ? outputGeometry(active.id) : currentBase;
  const saveGeometry = async (payload: LiveGeometryPatch) => {
    const nextGeometry = { ...serverSnapshotRef.current.geometry, ...payload.geometry } as VideoGeometry;
    const nextOverrides = (payload.geometry_by_output ?? serverSnapshotRef.current.overrides) as Record<string, VideoGeometry>;
    const signature = geometrySnapshotSignature(nextGeometry, nextOverrides);
    expectedEchoesRef.current.push(signature);
    if (expectedEchoesRef.current.length > 32) expectedEchoesRef.current.shift();
    try {
      await onSave(payload as Partial<GeometryCue>);
    } catch (error) {
      const index = expectedEchoesRef.current.lastIndexOf(signature);
      if (index >= 0) expectedEchoesRef.current.splice(index, 1);
      throw error;
    }
  };
  const enqueueSave = (payload: Partial<GeometryCue>, target = `${cue.id}:${active?.id ?? "none"}`) => {
    geometryQueueRef.current.enqueue(target, payload, saveGeometry, true);
  };
  const enqueueLiveSave = (payload: Partial<GeometryCue>, target: string, final: boolean) => {
    geometryQueueRef.current.enqueue(target, payload, saveGeometry, final);
  };
  const setLinkedGeometry = (next: VideoGeometry) => {
    currentRef.current = { ...currentRef.current, geometry: next, overrides: {} };
    setLocalOverrides({ cueId: cue.id, values: {} });
    enqueueSave({ geometry: next, geometry_by_output: {} });
  };
  const setOutputGeometry = (outputId: string, partial: Partial<VideoGeometry>) => {
    const next = mergeGeometryOutputOverride(currentRef.current.overrides, outputId, currentRef.current.geometry, partial);
    currentRef.current = { ...currentRef.current, overrides: next };
    setLocalOverrides({ cueId: cue.id, values: next });
    enqueueSave({ geometry_by_output: next });
  };
  const patchActiveGeometry = (
    partial: Partial<VideoGeometry>,
    gesture?: { target: string; outputId?: string; linked: boolean; final: boolean },
  ) => {
    const outputId = gesture ? gesture.outputId : active?.id;
    let payload: Partial<GeometryCue>;
    if (!outputId) {
      const next = { ...currentRef.current.geometry, ...partial };
      currentRef.current = { ...currentRef.current, geometry: next };
      payload = { geometry: next };
    } else if (gesture?.linked ?? linked) {
      const next = { ...currentRef.current.geometry, ...partial };
      currentRef.current = { ...currentRef.current, geometry: next, overrides: {} };
      setLocalOverrides({ cueId: cue.id, values: {} });
      payload = { geometry: next, geometry_by_output: {} };
    } else {
      const next = mergeGeometryOutputOverride(currentRef.current.overrides, outputId, currentRef.current.geometry, partial);
      currentRef.current = { ...currentRef.current, overrides: next };
      setLocalOverrides({ cueId: cue.id, values: next });
      payload = { geometry_by_output: next };
    }
    if (gesture) enqueueLiveSave(payload, gesture.target, gesture.final);
    else enqueueSave(payload);
  };
  const gestureTarget = `${cue.id}:${active?.id ?? "none"}`;
  const previewGeometry = (partial: Partial<VideoGeometry>) =>
    patchActiveGeometry(partial, { target: gestureTarget, outputId: active?.id, linked, final: false });
  const commitGeometry = (partial: Partial<VideoGeometry>) =>
    patchActiveGeometry(partial, { target: gestureTarget, outputId: active?.id, linked, final: true });
  if (selectedOutputs.length > 0) {
    const fitButtonStyle = (selected: boolean): CSSProperties => ({
      flex: 1, minWidth: 0, padding: "5px 4px", fontSize: 11,
      border: `1px solid ${selected ? "var(--wc-accent)" : "var(--wc-border)"}`,
      borderRadius: 4, background: selected ? "var(--wc-accent-soft)" : "var(--wc-bg-surface)",
      color: selected ? "var(--wc-text)" : "var(--wc-text-muted)", cursor: "pointer",
    });
    const aspect = outputAspectRatio(outputs.find((output) => output.id === active.id), screens);
    const liveCameraSize = isCameraCue(cue) ? cue._live_source_dimensions ?? retriedCameraDimensions : undefined;
    const actualSourceWidth = sourceWidth ?? liveCameraSize?.width ?? null;
    const actualSourceHeight = sourceHeight ?? liveCameraSize?.height ?? null;
    const sourceAspect = actualSourceWidth && actualSourceHeight ? actualSourceWidth / actualSourceHeight : aspect.ratio;
    const panMode: GeometryPanMode = isTextCue(cue) || isTimerCue(cue)
      ? "output"
      : isCameraCue(cue) && cue.source.kind === "network"
        && (cue.source.input.protocol === "ndi" || cue.source.input.protocol === "srt")
        ? "output" : "mpv";
    const overlay = isTextCue(cue) || isTimerCue(cue);
    const overlayFont = isTextCue(cue) ? cue.font : isTimerCue(cue) ? cue.font : "Arial";
    const overlayFontSize = isTextCue(cue) ? cue.font_size : isTimerCue(cue) ? cue.font_size : 60;
    const overlayTextColor = isTextCue(cue) ? cue.text_color : isTimerCue(cue) ? cue.text_color : "#FFFFFF";
    const contentLabel = isTimerCue(cue)
      ? timerPreviewValue(cue)
      : isTextCue(cue)
        ? cue.text || cue.name
        : cue.name;
    const mediaPath = cue.cue_type === "video" || cue.cue_type === "image" ? cue.file_path : null;
    const overlayMargins = isTextCue(cue) ? cue._overlay_margin_by_output : isTimerCue(cue) ? cue._overlay_margin_by_output : undefined;
    const activeOverlayMargin = overlayMargins ? overlayMargins[active.id] ?? 0 : overlayMargin;
    const switchLink = (nextLinked: boolean) => {
      flushGestures();
      void geometryQueueRef.current.flush(gestureTarget);
      const latestLinked = Object.keys(currentRef.current.overrides).length === 0;
      if (nextLinked === latestLinked) return;
      const next = geometryLinkTransition(latestLinked, nextLinked, currentRef.current.geometry, currentRef.current.overrides, outputIds, active?.id);
      currentRef.current = { ...currentRef.current, ...next };
      setLocalOverrides({ cueId: cue.id, values: next.overrides });
      enqueueSave({ geometry: next.geometry, geometry_by_output: next.overrides });
    };
    return <>
      {!overlay && <Section title={localeLabel("fit-title", locale)}>
        <FitByOutputRows outputs={selectedOutputs} geometryByOutput={activeOverrides} fallback={currentRef.current.geometry} onSelect={(id, fitMode) => {
          if (linked) setLinkedGeometry({ ...currentRef.current.geometry, fit_mode: fitMode });
          else setOutputGeometry(id, { fit_mode: fitMode });
        }} />
      </Section>}
      <Section title={localeLabel("output", locale)}>
        <div role="tablist" aria-label={localeLabel("output", locale)} style={{ display: "flex", gap: 5, overflowX: "auto", marginBottom: 8 }}>
          {selectedOutputs.map((output) => <button key={output.id} type="button" role="tab" aria-selected={active.id === output.id} onClick={() => {
            flushGestures();
            void geometryQueueRef.current.flush(gestureTarget);
            setActiveOutput(output.id);
          }} style={{ ...fitButtonStyle(active.id === output.id), flex: "0 0 auto", maxWidth: 140 }}>{output.name}</button>)}
        </div>
        <label title={selectedOutputs.length === 0 ? (locale === "ru" ? "Сначала выберите выход для cue." : "Select an output for this cue first.") : undefined} style={{ display: "flex", alignItems: "center", gap: 7, fontSize: 12, marginBottom: 8, opacity: selectedOutputs.length === 0 ? 0.55 : 1 }}>
          <input type="checkbox" checked={linked} disabled={selectedOutputs.length === 0} onChange={(event) => switchLink(event.target.checked)} />
          <span>{locale === "ru" ? "Одинаково на всех выходах" : "Same on all outputs"}</span>
        </label>
        <GeometryPad
          key={cue.id}
          gestureHandleRef={padGestureHandleRef}
          geometry={activeGeometry}
          mode={panMode}
          sourceAspect={sourceAspect}
          outputAspect={aspect.ratio}
          estimatedAspect={aspect.estimated || (!overlay && (!actualSourceWidth || !actualSourceHeight))}
          contentLabel={contentLabel}
          contextKey={`${cue.id}:${active.id}:${linked ? "linked" : "output"}`}
          syncRevision={syncRevision}
          mediaPath={mediaPath}
          mediaSourceRevision={cue.media_source_revision}
          seekInto={cue.cue_type === "video"}
          textFont={overlay ? overlayFont : undefined}
          textFontSize={overlay ? overlayFontSize : undefined}
          textColor={overlay ? overlayTextColor : undefined}
          anchor={isTextCue(cue) ? textPositionAnchor(cue.position, activeOverlayMargin, aspect.ratio) : undefined}
          isOverlay={overlay}
          hideRotation={overlay}
          disabled={panMode === "mpv" && activeGeometry.fit_mode === "stretch"}
          onPreview={previewGeometry}
          onCommit={commitGeometry}
        />
        <GeometryControlGrid
          key={gestureTarget + (linked ? ":linked" : ":output")}
          geometry={activeGeometry}
          contextKey={`${cue.id}:${active.id}:${linked ? "linked" : "output"}`}
          disabled={panMode === "mpv" && activeGeometry.fit_mode === "stretch"}
          overlay={overlay}
          panMode={panMode}
          gestureHandleRef={controlsGestureHandleRef}
          onPreview={previewGeometry}
          onCommit={commitGeometry}
        />
        <button type="button" disabled={JSON.stringify(outputGeometry(active.id)) === JSON.stringify(DEFAULT_GEOMETRY)} onClick={() => patchActiveGeometry({ ...DEFAULT_GEOMETRY })}
          style={{ marginTop: 8, padding: "4px 10px", fontSize: 11, borderRadius: 4, border: "1px solid var(--wc-border-strong)", background: "var(--wc-bg-surface)", color: "var(--wc-text-secondary)", cursor: "pointer" }}>
          {locale === "ru" ? "Сбросить геометрию" : "Reset geometry"}
        </button>
      </Section>
      {!overlay && <Section title={locale === "ru" ? "Кадрирование" : "Crop"}>
        <GeometryCropControls
          key={gestureTarget + (linked ? ":linked" : ":output")}
          geometry={outputGeometry(active.id)}
          contextKey={`${cue.id}:${active.id}:${linked ? "linked" : "output"}`}
          gestureHandleRef={cropGestureHandleRef}
          onPreview={previewGeometry}
          onCommit={commitGeometry}
        />
      </Section>}
    </>;
  }
  const liveCameraSize = isCameraCue(cue) ? cue._live_source_dimensions ?? retriedCameraDimensions : undefined;
  const actualSourceWidth = sourceWidth ?? liveCameraSize?.width ?? null;
  const actualSourceHeight = sourceHeight ?? liveCameraSize?.height ?? null;
  const fallbackDisplay = outputs.find((output) => output.enabled && output.sink_kind === "display");
  const defaultAspect = outputAspectRatio(fallbackDisplay, screens);
  const sourceAspect = actualSourceWidth && actualSourceHeight ? actualSourceWidth / actualSourceHeight : defaultAspect.ratio;
  const overlay = isTextCue(cue) || isTimerCue(cue);
  const overlayFont = isTextCue(cue) ? cue.font : isTimerCue(cue) ? cue.font : "Arial";
  const overlayFontSize = isTextCue(cue) ? cue.font_size : isTimerCue(cue) ? cue.font_size : 60;
  const overlayTextColor = isTextCue(cue) ? cue.text_color : isTimerCue(cue) ? cue.text_color : "#FFFFFF";
  const panMode: GeometryPanMode = overlay || (isCameraCue(cue) && cue.source.kind === "network" && (cue.source.input.protocol === "ndi" || cue.source.input.protocol === "srt")) ? "output" : "mpv";
  const contentLabel = isTimerCue(cue) ? timerPreviewValue(cue) : isTextCue(cue) ? cue.text || cue.name : cue.name;
  const mediaPath = cue.cue_type === "video" || cue.cue_type === "image" ? cue.file_path : null;
  const noOutputEditorState = Object.fromEntries(Object.keys(DEFAULT_GEOMETRY).map((key) => [key, uniform(currentBase[key as keyof VideoGeometry])])) as GeometryEditorState;
  return <>
    <GeometryEditor
      geometry={noOutputEditorState}
      hideFit={overlay}
      hidePosition
      hideScale
      hideRotationCrop
      resetDisabled={JSON.stringify(currentBase) === JSON.stringify(DEFAULT_GEOMETRY)}
      onPatch={patchActiveGeometry}
      onReset={() => patchActiveGeometry({ ...DEFAULT_GEOMETRY })}
    />
    <Section title={localeLabel("output", locale)}>
      <GeometryPad key={cue.id} gestureHandleRef={padGestureHandleRef} geometry={currentBase} mode={panMode} sourceAspect={sourceAspect} outputAspect={defaultAspect.ratio} estimatedAspect={defaultAspect.estimated || (!overlay && (!actualSourceWidth || !actualSourceHeight))} contentLabel={contentLabel} contextKey={`${cue.id}:base`} syncRevision={syncRevision} mediaPath={mediaPath} mediaSourceRevision={cue.media_source_revision} seekInto={cue.cue_type === "video"} textFont={overlay ? overlayFont : undefined} textFontSize={overlay ? overlayFontSize : undefined} textColor={overlay ? overlayTextColor : undefined} isOverlay={overlay} hideRotation={overlay} disabled={panMode === "mpv" && currentBase.fit_mode === "stretch"} anchor={isTextCue(cue) ? textPositionAnchor(cue.position, overlayMargin, defaultAspect.ratio) : undefined} onPreview={previewGeometry} onCommit={commitGeometry} />
      <GeometryControlGrid key={`${cue.id}:base`} geometry={currentBase} contextKey={`${cue.id}:base`} disabled={panMode === "mpv" && currentBase.fit_mode === "stretch"} overlay={overlay} panMode={panMode} gestureHandleRef={controlsGestureHandleRef} onPreview={previewGeometry} onCommit={commitGeometry} />
    </Section>
    {!overlay && <Section title={locale === "ru" ? "Кадрирование" : "Crop"}>
      <GeometryCropControls geometry={currentBase} contextKey={`${cue.id}:base`} gestureHandleRef={cropGestureHandleRef} onPreview={previewGeometry} onCommit={commitGeometry} />
    </Section>}
  </>;
}

export function GeometryControlGrid({
  geometry,
  contextKey,
  disabled,
  overlay,
  panMode = "mpv",
  gestureHandleRef,
  onPreview,
  onCommit,
}: {
  geometry: VideoGeometry;
  contextKey: string;
  disabled: boolean;
  overlay: boolean;
  panMode?: GeometryPanMode;
  gestureHandleRef: React.MutableRefObject<GeometryGestureHandle | null>;
  onPreview: (partial: Partial<VideoGeometry>) => void;
  onCommit: (partial: Partial<VideoGeometry>) => void;
}) {
  const { locale } = useLocale();
  const scaleRef = useRef<GeometryGestureHandle | null>(null);
  const rotationRef = useRef<GeometryGestureHandle | null>(null);
  const xRef = useRef<GeometryGestureHandle | null>(null);
  const yRef = useRef<GeometryGestureHandle | null>(null);
  useEffect(() => {
    const handle: GeometryGestureHandle = {
      flush: () => { scaleRef.current?.flush(); rotationRef.current?.flush(); xRef.current?.flush(); yRef.current?.flush(); },
      cancel: () => { scaleRef.current?.cancel(); rotationRef.current?.cancel(); xRef.current?.cancel(); yRef.current?.cancel(); },
    };
    gestureHandleRef.current = handle;
    return () => { if (gestureHandleRef.current === handle) gestureHandleRef.current = null; };
  }, [gestureHandleRef]);
  const labels = locale === "ru"
    ? { scale: "Масштаб", rotation: "Поворот", x: "X (%)", y: "Y (%)" }
    : { scale: "Scale", rotation: "Rotation", x: "X (%)", y: "Y (%)" };
  const panTooltip = locale === "ru"
    ? panMode === "output" ? "Смещение относительно размера экрана; шаг 1%." : "Смещение относительно размера масштабированного изображения; шаг 1%."
    : panMode === "output" ? "Offset relative to the output size; 1% steps." : "Offset relative to the scaled image size; 1% steps.";
  return <div style={{ display: "grid", gridTemplateColumns: "minmax(0,1fr) minmax(0,1fr)", gap: "8px 12px", marginTop: 8 }}>
    <GeometryCompactSlider
      refHandle={scaleRef} contextKey={contextKey} label={labels.scale} value={geometry.scale} min={0.05} max={8} step={0.01}
      wheelStep={0.01} neutral={1} snapTolerance={0} snapToCenter resetValue={1} disabled={disabled}
      format={(value) => Math.round(value * 100) + "%"}
      onPreview={(value) => onPreview({ scale: value })} onCommit={(value) => onCommit({ scale: value })}
    />
    <GeometryCompactSlider
      refHandle={rotationRef} contextKey={contextKey} label={labels.rotation} value={geometry.rotation} min={0} max={360} step={1}
      wheelStep={1} snapCardinalRotation resetValue={0} disabled={overlay} hint={overlay ? (locale === "ru" ? "Недоступно для текста и таймера" : "Unavailable for Text and Timer") : undefined} format={(value) => Math.round(value) + "°"}
      onPreview={(value) => onPreview({ rotation: Math.round(value) })} onCommit={(value) => onCommit({ rotation: Math.round(value) })}
    />
    <GeometryCompactSlider
      refHandle={xRef} contextKey={contextKey} label={labels.x} tooltip={panTooltip} value={geometry.pan_x} min={-1} max={1} step={0.01} rangeStep="any" quantizeStep={0.01}
      wheelStep={0.01} neutral={0} resetValue={0} disabled={disabled} format={formatGeometryPanPercent}
      onPreview={(value) => onPreview({ pan_x: value })} onCommit={(value) => onCommit({ pan_x: value })} snapToCenter
    />
    <GeometryCompactSlider
      refHandle={yRef} contextKey={contextKey} label={labels.y} tooltip={panTooltip} value={geometry.pan_y} min={-1} max={1} step={0.01} rangeStep="any" quantizeStep={0.01}
      wheelStep={0.01} neutral={0} resetValue={0} disabled={disabled} format={formatGeometryPanPercent}
      onPreview={(value) => onPreview({ pan_y: value })} onCommit={(value) => onCommit({ pan_y: value })} snapToCenter
    />
    {disabled && <div style={{ gridColumn: "1 / -1", fontSize: 10, lineHeight: 1.3, color: "var(--wc-text-faint)" }}>
      {locale === "ru" ? "Для перемещения и масштаба выберите «Вписать» или «Вписать с обрезкой»." : "Choose Fit or Fill to position and scale this source."}
    </div>}
  </div>;
}

export function GeometryCropControls({
  geometry,
  contextKey,
  gestureHandleRef,
  onPreview,
  onCommit,
}: {
  geometry: VideoGeometry;
  contextKey: string;
  gestureHandleRef: React.MutableRefObject<GeometryGestureHandle | null>;
  onPreview: (partial: Partial<VideoGeometry>) => void;
  onCommit: (partial: Partial<VideoGeometry>) => void;
}) {
  const { locale } = useLocale();
  const refs = useRef<Record<string, GeometryGestureHandle | null>>({});
  useEffect(() => {
    const handle: GeometryGestureHandle = {
      flush: () => Object.values(refs.current).forEach((ref) => ref?.flush()),
      cancel: () => Object.values(refs.current).forEach((ref) => ref?.cancel()),
    };
    gestureHandleRef.current = handle;
    return () => { if (gestureHandleRef.current === handle) gestureHandleRef.current = null; };
  }, [gestureHandleRef]);
  const keys: Array<{ key: GeometryNumberKey; label: string }> = locale === "ru"
    ? [{ key: "crop_left", label: "Слева" }, { key: "crop_right", label: "Справа" }, { key: "crop_top", label: "Сверху" }, { key: "crop_bottom", label: "Снизу" }]
    : [{ key: "crop_left", label: "Left" }, { key: "crop_right", label: "Right" }, { key: "crop_top", label: "Top" }, { key: "crop_bottom", label: "Bottom" }];
  return <div style={{ display: "grid", gridTemplateColumns: "minmax(0,1fr) minmax(0,1fr)", gap: "8px 12px" }}>
      {keys.map(({ key, label }) => <GeometryCompactSlider
        key={key}
        refHandle={{ get current() { return refs.current[key] ?? null; }, set current(value) { refs.current[key] = value; } }}
        contextKey={contextKey}
        label={label}
        value={cropGeometryToDisplay(geometry[key])}
        min={0}
        max={100}
        step={1}
        wheelStep={1}
        resetValue={0}
        disabled={false}
        format={(value) => String(Math.round(value))}
        onPreview={(value) => onPreview({ [key]: cropDisplayToGeometry(value) })}
        onCommit={(value) => onCommit({ [key]: cropDisplayToGeometry(value) })}
      />)}
    </div>;
}

function GeometryCompactSlider({
  refHandle,
  contextKey,
  label,
  value,
  min,
  max,
  step,
  wheelStep,
  neutral,
  snapTolerance = 0,
  snapToCenter = false,
  snapCardinalRotation = false,
  resetValue,
  disabled,
  format,
  onPreview,
  onCommit,
  hint,
  tooltip,
  rangeStep,
  quantizeStep,
}: {
  refHandle: React.MutableRefObject<GeometryGestureHandle | null>;
  contextKey: string;
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  wheelStep: number;
  neutral?: number;
  snapTolerance?: number;
  snapToCenter?: boolean;
  snapCardinalRotation?: boolean;
  resetValue: number;
  disabled: boolean;
  format: (value: number) => string;
  onPreview: (value: number) => void;
  onCommit: (value: number) => void;
  hint?: string;
  tooltip?: string;
  rangeStep?: number | "any";
  quantizeStep?: number;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const valueRef = useRef(value);
  const startRef = useRef(value);
  const activeRef = useRef(false);
  const pointerRef = useRef(false);
  const keyboardRef = useRef(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingWheel = useRef<{ value: number; commit: (value: number) => void } | null>(null);
  const callbacksRef = useRef({ onPreview, onCommit });
  const pointerPositionRef = useRef<number | null>(null);
  const pointerOffsetRef = useRef(0);
  const detentRef = useRef<GeometryCenterDetentState>(idleGeometryCenterDetent());
  const [draft, setDraft] = useState(value);
  const flushWheel = useCallback(() => {
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = null;
    const pending = pendingWheel.current;
    pendingWheel.current = null;
    if (pending) pending.commit(pending.value);
  }, []);
  const cancelGesture = () => {
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = null;
    pendingWheel.current = null;
    const cancelled = cancelGeometryAxisGesture(activeRef.current, startRef.current, valueRef.current);
    activeRef.current = false;
    pointerRef.current = false;
    keyboardRef.current = false;
    pointerPositionRef.current = null;
    pointerOffsetRef.current = 0;
    detentRef.current = idleGeometryCenterDetent();
    if (cancelled.cancelled) {
      valueRef.current = cancelled.value;
      setDraft(cancelled.value);
    }
  };
  useEffect(() => {
    const handle: GeometryGestureHandle = { flush: flushWheel, cancel: cancelGesture };
    refHandle.current = handle;
    return () => { if (refHandle.current === handle) refHandle.current = null; };
  }, [contextKey, flushWheel, refHandle]);
  useEffect(() => () => {
    flushWheel();
    if (activeRef.current) {
      activeRef.current = false;
      pointerRef.current = false;
      keyboardRef.current = false;
      callbacksRef.current.onCommit(valueRef.current);
    }
  }, [contextKey, flushWheel]);
  useEffect(() => {
    if (activeRef.current || pendingWheel.current) return;
    valueRef.current = value;
    setDraft(value);
  }, [value, contextKey]);

  useEffect(() => {
    const element = inputRef.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      if (disabled || event.deltaY === 0) return;
      event.preventDefault();
      const direction = event.deltaY < 0 ? 1 : -1;
      pointerPositionRef.current = null;
      pointerOffsetRef.current = 0;
      detentRef.current = idleGeometryCenterDetent();
      const next = snapCardinalRotation
        ? nextGeometryRotationWheelValue(valueRef.current, event.deltaY, min, max)
        : neutral === 0
        ? nextGeometryAxisWheelValue(valueRef.current, event.deltaY)
        : neutral === 1
          ? nextGeometryScaleWheelValue(valueRef.current, event.deltaY)
          : Math.min(max, Math.max(min, Number((valueRef.current + direction * wheelStep).toFixed(3))));
      valueRef.current = next;
      setDraft(next);
      onPreview(next);
      pendingWheel.current = { value: next, commit: onCommit };
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(flushWheel, 180);
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [contextKey, disabled, flushWheel, max, min, neutral, onCommit, onPreview, snapCardinalRotation, wheelStep]);

  const positionFromPointer = (clientX: number) => {
    const rect = inputRef.current?.getBoundingClientRect();
    return rect ? geometrySliderPositionFromPointer(clientX - pointerOffsetRef.current, rect.left, rect.width) : 0.5;
  };
  const advancePointerDetent = (clientX: number) => {
    const input = inputRef.current;
    const previousX = pointerPositionRef.current;
    pointerPositionRef.current = clientX;
    if (!input || !snapToCenter || neutral == null || previousX == null) return;
    const rect = input.getBoundingClientRect();
    const before = detentRef.current;
    const previousEffectiveX = geometryEffectivePointerX(previousX, pointerOffsetRef.current);
    const currentEffectiveX = geometryEffectivePointerX(clientX, pointerOffsetRef.current);
    const after = advanceGeometryCenterDetent(before, previousEffectiveX, currentEffectiveX, rect.left + rect.width / 2, Math.max(1, rect.width - 16));
    detentRef.current = after;
    if (!before.latched && after.latched) update(neutral);
  };
  const toValue = (raw: number, pointer: boolean) => {
    if (pointer && snapToCenter && neutral != null && pointerPositionRef.current != null) {
      return geometryValueAtDetentPosition(positionFromPointer(pointerPositionRef.current), min, max, neutral, detentRef.current);
    }
    const next = neutral == null
      ? raw
    : sliderPositionToValue(raw, min, max, neutral, pointer ? snapTolerance : 0);
    return pointer && snapCardinalRotation ? snapRotationToCardinal(next) : next;
  };
  const update = (next: number) => {
    if (quantizeStep === 0.01) next = quantizeGeometryPan(next);
    if (Object.is(valueRef.current, next)) return;
    valueRef.current = next;
    setDraft(next);
    (activeRef.current ? callbacksRef.current.onPreview : onPreview)(next);
  };
  const finish = (event?: React.PointerEvent<HTMLInputElement>) => {
    if (!activeRef.current) return;
    if (pointerRef.current && snapToCenter && neutral != null && event) {
      advancePointerDetent(event.clientX);
      const input = inputRef.current;
      if (input) {
        const rect = input.getBoundingClientRect();
        const rawX = pointerPositionRef.current ?? event.clientX;
        const effectiveX = geometryEffectivePointerX(rawX, pointerOffsetRef.current);
        const released = releaseGeometryCenterDetent(detentRef.current, effectiveX, rect.left + rect.width / 2, Math.max(1, rect.width - 16));
        detentRef.current = released;
        update(geometryValueAtDetentPosition(positionFromPointer(rawX), min, max, neutral, released));
      }
    }
    activeRef.current = false;
    pointerRef.current = false;
    keyboardRef.current = false;
    pointerPositionRef.current = null;
    pointerOffsetRef.current = 0;
    detentRef.current = idleGeometryCenterDetent();
    if (valueRef.current !== startRef.current) callbacksRef.current.onCommit(valueRef.current);
  };
  const reset = () => {
    if (disabled) return;
    pointerPositionRef.current = null;
    pointerOffsetRef.current = 0;
    detentRef.current = idleGeometryCenterDetent();
    valueRef.current = resetValue;
    setDraft(resetValue);
    onPreview(resetValue);
    onCommit(resetValue);
  };
  const mappedValue = neutral == null ? (draft - min) / (max - min) : sliderValueToPosition(draft, min, max, neutral);
  return <div style={{ minWidth: 0 }}>
    <label title={tooltip} style={{ display: "block", fontSize: 10, color: "var(--wc-text-secondary)", marginBottom: 2, whiteSpace: "nowrap" }}>{label}</label>
    {hint && <div style={{ fontSize: 8, lineHeight: "10px", color: "var(--wc-text-faint)", marginBottom: 1 }}>{hint}</div>}
    <div style={{ display: "flex", alignItems: "center", gap: 5, minWidth: 0 }}>
      <div style={{ position: "relative", flex: 1, minWidth: 0 }}>
        <input ref={inputRef} type="range" min={neutral == null ? min : 0} max={neutral == null ? max : 1} step={neutral == null ? step : rangeStep ?? 0.001} title={tooltip}
          value={neutral == null ? draft : mappedValue} disabled={disabled} aria-label={label} onDoubleClick={reset}
          onPointerDown={(event) => {
            flushWheel();
            activeRef.current = true;
            pointerRef.current = true;
            startRef.current = valueRef.current;
            callbacksRef.current = { onPreview, onCommit };
            if (snapToCenter && neutral != null) {
              const rect = event.currentTarget.getBoundingClientRect();
              const thumbWidth = 16;
              const thumbX = rect.left + thumbWidth / 2 + Number(event.currentTarget.value) * Math.max(1, rect.width - thumbWidth);
              pointerOffsetRef.current = Math.abs(event.clientX - thumbX) <= thumbWidth / 2 + 2 ? event.clientX - thumbX : 0;
              const effectiveX = geometryEffectivePointerX(event.clientX, pointerOffsetRef.current);
              pointerPositionRef.current = event.clientX;
              detentRef.current = advanceGeometryCenterDetent(idleGeometryCenterDetent(), effectiveX, effectiveX, rect.left + rect.width / 2, Math.max(1, rect.width - thumbWidth));
              if (detentRef.current.latched) update(neutral);
            }
            event.currentTarget.setPointerCapture(event.pointerId);
          }}
          onPointerMove={(event) => { if (pointerRef.current && snapToCenter) advancePointerDetent(event.clientX); }}
          onChange={(event) => update(toValue(Number(event.target.value), pointerRef.current))}
          onPointerUp={(event) => finish(event)}
          onPointerCancel={() => { if (activeRef.current) { valueRef.current = startRef.current; setDraft(startRef.current); callbacksRef.current.onPreview(startRef.current); callbacksRef.current.onCommit(startRef.current); activeRef.current = false; pointerRef.current = false; keyboardRef.current = false; pointerPositionRef.current = null; pointerOffsetRef.current = 0; detentRef.current = idleGeometryCenterDetent(); } }}
          onKeyDown={(event) => {
            if (!["ArrowLeft", "ArrowDown", "ArrowRight", "ArrowUp"].includes(event.key)) return;
            event.preventDefault();
            if (!activeRef.current) { activeRef.current = true; pointerRef.current = false; keyboardRef.current = true; pointerPositionRef.current = null; pointerOffsetRef.current = 0; detentRef.current = idleGeometryCenterDetent(); startRef.current = valueRef.current; callbacksRef.current = { onPreview, onCommit }; }
            const direction = event.key === "ArrowLeft" || event.key === "ArrowDown" ? -1 : 1;
            const precision = Math.max(0, (String(step).split(".")[1] ?? "").length);
            const next = quantizeStep === 0.01
              ? nextGeometryAxisKeyboardValue(valueRef.current, direction)
              : Number(Math.min(max, Math.max(min, valueRef.current + direction * step)).toFixed(precision));
            update(next);
          }}
          onKeyUp={() => finish()}
          onBlur={() => { if (keyboardRef.current) finish(); }}
          style={{ width: "100%", minWidth: 0, margin: 0, opacity: disabled ? 0.45 : 1 }}
        />
      </div>
      <span style={{ width: 38, flexShrink: 0, textAlign: "right", fontSize: 10, fontVariantNumeric: "tabular-nums", color: "var(--wc-text-muted)" }}>{format(draft)}</span>
    </div>
  </div>;
}

function textPositionAnchor(position: TextCueData["position"], margin: number, outputAspect: number): { x: number; y: number } {
  const marginX = Math.min(0.5, margin / (720 * outputAspect));
  const marginY = Math.min(0.5, margin / 720);
  const x = position.endsWith("left") ? marginX : position.endsWith("right") ? 1 - marginX : 0.5;
  const y = position.startsWith("top") ? marginY : position.startsWith("bottom") ? 1 - marginY : 0.5;
  return { x, y };
}

function timerPreviewValue(cue: TimerCueData): string {
  const seconds = cue.mode === "countup" ? 0 : Math.ceil(cue.timer_duration_ms / 1000);
  return cue.format === "seconds" ? String(seconds) : `${String(Math.floor(seconds / 60)).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;
}

function geometrySnapshotSignature(geometry: VideoGeometry, overrides: Record<string, VideoGeometry>): string {
  const fields = Object.keys(DEFAULT_GEOMETRY) as Array<keyof VideoGeometry>;
  const normalized = (value: VideoGeometry) => fields.map((field) => value[field]);
  return JSON.stringify([
    normalized(geometry),
    Object.entries(overrides).sort(([left], [right]) => left.localeCompare(right)).map(([id, value]) => [id, normalized(value)]),
  ]);
}

function localeLabel(key: string, locale: string): string {
  if (key === "fit-title") return locale === "ru" ? "Вписать по выходам" : "Fit by output";
  if (key === "output") return locale === "ru" ? "Выход" : "Output";
  if (locale !== "ru") return ({ fit: "Fit", fill: "Fill", stretch: "Stretch" } as Record<string, string>)[key] ?? key;
  if (key === "fit") return "Вписать";
  if (key === "fill") return "Обрезать";
  if (key === "stretch") return "Растянуть";
  return key;
}
