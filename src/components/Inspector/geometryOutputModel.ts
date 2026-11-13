import { useEffect, useRef, useState } from "react";
import type { OutputDestination, VideoGeometry } from "../../lib/types";

export type GeometryOutputOverrides = Record<string, VideoGeometry>;

export function buildGeometryFitPatch(
  geometry: VideoGeometry,
  overrides: GeometryOutputOverrides | undefined,
  outputId: string,
  fitMode: VideoGeometry["fit_mode"],
): { geometry?: VideoGeometry; geometry_by_output: GeometryOutputOverrides } {
  const current = overrides ?? {};
  if (Object.keys(current).length === 0) {
    return {
      geometry: { ...geometry, fit_mode: fitMode },
      geometry_by_output: {},
    };
  }
  const existing = { ...geometry, ...(current[outputId] ?? {}) };
  return {
    geometry_by_output: { ...current, [outputId]: { ...existing, fit_mode: fitMode } },
  };
}

export interface CueGeometryOverridesSnapshot {
  cueId: string;
  values: GeometryOutputOverrides;
}

export type GeometryPanMode = "mpv" | "output";

export function cancelGeometryAxisGesture(active: boolean, start: number, current: number): { cancelled: boolean; value: number } {
  return active ? { cancelled: true, value: start } : { cancelled: false, value: current };
}

export function nextGeometryAxisWheelValue(current: number, deltaY: number): number {
  if (deltaY === 0) return current;
  const direction = deltaY < 0 ? 1 : -1;
  return quantizeGeometryPan(current + direction * 0.01);
}

export function nextGeometryAxisKeyboardValue(current: number, direction: number): number {
  if (direction === 0) return current;
  return quantizeGeometryPan(current + Math.sign(direction) * 0.01);
}

/** Pan is stored as a fraction; interactive changes snap to whole percent. */
export function quantizeGeometryPan(value: number): number {
  const rounded = Math.sign(value) * Math.round(Math.abs(value) * 100 + 1e-9) / 100;
  return clamp(Number(rounded.toFixed(2)), -1, 1);
}

/** Keep authored sub-percent precision visible until the next interaction. */
export function formatGeometryPanPercent(value: number): string {
  if (!Number.isFinite(value)) return "0%";
  if (value === 0) return "0%";
  const percent = value * 100;
  const rounded = Number(percent.toFixed(2));
  const sign = percent > 0 ? "+" : "";
  if (rounded === 0) return `${percent < 0 ? "-" : "+"}<0.01%`;
  return `${sign}${Number.isInteger(rounded) ? rounded : rounded.toString()}%`;
}

export function nextGeometryScaleWheelValue(current: number, deltaY: number): number {
  if (deltaY === 0) return current;
  const direction = deltaY < 0 ? 1 : -1;
  return clamp(Number((current + direction * 0.01).toFixed(3)), 0.05, 8);
}

export function nextGeometryRotationWheelValue(current: number, deltaY: number, min = 0, max = 360): number {
  if (deltaY === 0) return current;
  return clamp(current + (deltaY < 0 ? 1 : -1), min, max);
}

export function cropDisplayToGeometry(value: number): number {
  return clamp(value, 0, 100) / 100;
}

export function cropGeometryToDisplay(value: number): number {
  return clamp(value * 100, 0, 100);
}

/** Clip full-source pixels in place; crossed opposite edges hide the source. */
export function geometryCropClipPath(geometry: VideoGeometry): string | undefined {
  const edge = (value: number | null | undefined) => Number.isFinite(value) ? clamp(value ?? 0, 0, 1) : 0;
  const left = edge(geometry.crop_left);
  const right = edge(geometry.crop_right);
  const top = edge(geometry.crop_top);
  const bottom = edge(geometry.crop_bottom);
  if (left + right + top + bottom === 0) return undefined;
  if (left + right >= 1 || top + bottom >= 1) return "inset(50%)";
  return `inset(${top * 100}% ${right * 100}% ${bottom * 100}% ${left * 100}%)`;
}

export function consumeExpectedGeometryEcho(expected: string[], incoming: string): boolean {
  const index = expected.findIndex((signature) => geometrySnapshotsMatch(signature, incoming));
  if (index < 0) return false;
  expected.splice(0, index + 1);
  return true;
}

/** Match serialized f64 geometry snapshots across harmless JSON round trips. */
export function geometrySnapshotsMatch(expected: string, incoming: string): boolean {
  if (expected === incoming) return true;
  try {
    return sameGeometrySnapshot(JSON.parse(expected), JSON.parse(incoming));
  } catch {
    return false;
  }
}

function sameGeometrySnapshot(expected: unknown, incoming: unknown): boolean {
  if (typeof expected === "number" && typeof incoming === "number") {
    return Math.abs(expected - incoming) <= 1e-12;
  }
  if (Array.isArray(expected) && Array.isArray(incoming)) {
    return expected.length === incoming.length && expected.every((value, index) => sameGeometrySnapshot(value, incoming[index]));
  }
  return expected === incoming;
}

/** Keep quick per-output edits optimistic while their serialized saves complete. */
export function useGeometryOutputOverrides(
  cueId: string,
  persisted: GeometryOutputOverrides | undefined,
  geometry: VideoGeometry,
  onSave: (patch: { geometry?: VideoGeometry; geometry_by_output: GeometryOutputOverrides }) => void | Promise<void>,
): { values: GeometryOutputOverrides; saveOutput: (outputId: string, partial: Partial<VideoGeometry>) => void; saveFit: (outputId: string, fitMode: VideoGeometry["fit_mode"]) => void } {
  const initial = persisted ?? {};
  const [snapshot, setSnapshot] = useState<CueGeometryOverridesSnapshot>({ cueId, values: initial });
  const currentRef = useRef({ cueId, geometry, overrides: initial });
  const pendingSaves = useRef(0);
  const saveQueue = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    if (currentRef.current.cueId === cueId && pendingSaves.current > 0) return;
    const next = { cueId, geometry, overrides: persisted ?? {} };
    currentRef.current = next;
    setSnapshot({ cueId, values: next.overrides });
  }, [cueId, geometry, persisted]);

  const saveOutput = (outputId: string, partial: Partial<VideoGeometry>) => {
    const current = currentRef.current.cueId === cueId
      ? currentRef.current
      : { cueId, geometry, overrides: persisted ?? {} };
    const next = mergeGeometryOutputOverride(current.overrides, outputId, current.geometry, partial);
    currentRef.current = { ...current, overrides: next };
    setSnapshot({ cueId, values: next });
    pendingSaves.current += 1;
    saveQueue.current = saveQueue.current.catch(() => undefined).then(() => Promise.resolve(onSave({ geometry_by_output: next }))).then(() => undefined).finally(() => {
      pendingSaves.current = Math.max(0, pendingSaves.current - 1);
    });
  };

  const saveFit = (outputId: string, fitMode: VideoGeometry["fit_mode"]) => {
    const current = currentRef.current.cueId === cueId
      ? currentRef.current
      : { cueId, geometry, overrides: persisted ?? {} };
    const patch = buildGeometryFitPatch(current.geometry, current.overrides, outputId, fitMode);
    currentRef.current = { ...current, geometry: patch.geometry ?? current.geometry, overrides: patch.geometry_by_output };
    setSnapshot({ cueId, values: patch.geometry_by_output });
    pendingSaves.current += 1;
    saveQueue.current = saveQueue.current.catch(() => undefined).then(() => Promise.resolve(onSave(patch))).then(() => undefined).finally(() => {
      pendingSaves.current = Math.max(0, pendingSaves.current - 1);
    });
  };

  return { values: snapshot.cueId === cueId ? snapshot.values : persisted ?? {}, saveOutput, saveFit };
}

/** Resolve the aspect ratio used by the editor pad without opening a live output. */
export function outputAspectRatio(
  output: OutputDestination | undefined,
  screens: readonly { index: number; width: number; height: number }[],
): { ratio: number; estimated: boolean } {
  if (output?.sink_kind === "srt") {
    const width = output.network?.srt.width;
    const height = output.network?.srt.height;
    if (width && height && width > 0 && height > 0) return { ratio: width / height, estimated: false };
  }
  if (output?.sink_kind === "display" && output.monitor != null) {
    const screen = screens.find((candidate) => candidate.index === output.monitor);
    if (screen?.width && screen.height && screen.width > 0 && screen.height > 0) return { ratio: screen.width / screen.height, estimated: false };
  }
  const floating = output?.floating_window;
  if (floating?.width && floating.height && floating.width > 0 && floating.height > 0) return { ratio: floating.width / floating.height, estimated: false };
  return { ratio: 16 / 9, estimated: true };
}

/**
 * Compute the unscaled media footprint in output-normalized units. mpv fits
 * the unrotated source to the output first, then applies the cue scale.
 */
export function fittedFootprint(
  geometry: VideoGeometry,
  sourceAspect: number,
  outputAspect: number,
): { width: number; height: number } {
  const rawSource = Number.isFinite(sourceAspect) && sourceAspect > 0 ? sourceAspect : outputAspect;
  const source = geometry.rotation % 180 === 90 ? 1 / rawSource : rawSource;
  const output = Number.isFinite(outputAspect) && outputAspect > 0 ? outputAspect : 16 / 9;
  let width = 1;
  let height = 1;
  if (geometry.fit_mode === "fit") {
    if (source > output) height = output / source;
    else width = source / output;
  } else if (geometry.fit_mode === "fill") {
    if (source > output) width = source / output;
    else height = output / source;
  }
  return { width, height };
}

/** Map stored geometry pan to its visible output-relative displacement. */
export function geometryPanToOutputOffset(
  geometry: VideoGeometry,
  mode: GeometryPanMode,
  sourceAspect: number,
  outputAspect: number,
): { x: number; y: number } {
  if (mode === "output") return { x: geometry.pan_x, y: geometry.pan_y };
  if (geometry.fit_mode === "stretch") return { x: 0, y: 0 };
  const footprint = fittedFootprint(geometry, sourceAspect, outputAspect);
  const scale = Math.max(0.01, geometry.scale);
  return {
    x: geometry.pan_x * footprint.width * scale,
    y: geometry.pan_y * footprint.height * scale,
  };
}

/** Convert a drag in output-normalized coordinates back to the cue's pan units. */
export function outputOffsetToGeometryPan(
  dx: number,
  dy: number,
  geometry: VideoGeometry,
  mode: GeometryPanMode,
  sourceAspect: number,
  outputAspect: number,
): { pan_x: number; pan_y: number } {
  if (mode === "output") {
    return {
      pan_x: clamp(geometry.pan_x + dx, -1, 1),
      pan_y: clamp(geometry.pan_y + dy, -1, 1),
    };
  }
  if (geometry.fit_mode === "stretch") return { pan_x: geometry.pan_x, pan_y: geometry.pan_y };
  const footprint = fittedFootprint(geometry, sourceAspect, outputAspect);
  const scale = Math.max(0.01, geometry.scale);
  return {
    pan_x: clamp(geometry.pan_x + dx / (footprint.width * scale), -1, 1),
    pan_y: clamp(geometry.pan_y + dy / (footprint.height * scale), -1, 1),
  };
}

export function geometryOverridesForCue(
  local: CueGeometryOverridesSnapshot,
  cueId: string,
  persisted: GeometryOutputOverrides | undefined,
): GeometryOutputOverrides {
  return local.cueId === cueId ? local.values : persisted ?? {};
}

export function mergeGeometryOutputOverride(
  overrides: GeometryOutputOverrides,
  outputId: string,
  fallback: VideoGeometry,
  partial: Partial<VideoGeometry>,
): GeometryOutputOverrides {
  return {
    ...overrides,
    [outputId]: { ...(overrides[outputId] ?? fallback), ...partial },
  };
}

/** Atomically switch between a shared base transform and output-specific copies. */
export function geometryLinkTransition(
  linked: boolean,
  nextLinked: boolean,
  geometry: VideoGeometry,
  overrides: GeometryOutputOverrides,
  outputIds: readonly string[],
  activeOutputId?: string,
): { geometry: VideoGeometry; overrides: GeometryOutputOverrides } {
  if (linked === nextLinked) return { geometry, overrides };
  if (nextLinked) {
    const effective = (activeOutputId && overrides[activeOutputId]) || geometry;
    return { geometry: { ...effective }, overrides: {} };
  }
  return {
    geometry: { ...geometry },
    overrides: Object.fromEntries(outputIds.map((id) => [id, { ...(overrides[id] ?? geometry) }])),
  };
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
