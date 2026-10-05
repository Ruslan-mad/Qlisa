import type { VideoGeometry } from "../../lib/types";
import { useEffect, useRef, useState } from "react";

export type GeometryOutputOverrides = Record<string, VideoGeometry>;

export interface CueGeometryOverridesSnapshot {
  cueId: string;
  values: GeometryOutputOverrides;
}

export function useGeometryOutputOverrides(
  cueId: string,
  persisted: GeometryOutputOverrides | undefined,
  fallback: VideoGeometry,
  onSave: (overrides: GeometryOutputOverrides) => void,
) {
  const initial = { cueId, values: persisted ?? {} };
  const [local, setLocal] = useState(initial);
  const current = useRef(initial);
  useEffect(() => {
    const synced = { cueId, values: persisted ?? {} };
    current.current = synced;
    setLocal(synced);
  }, [cueId, persisted]);

  const values = geometryOverridesForCue(local, cueId, persisted);
  const saveOutput = (outputId: string, partial: Partial<VideoGeometry>) => {
    const existing = geometryOverridesForCue(current.current, cueId, persisted);
    const next = mergeGeometryOutputOverride(existing, outputId, fallback, partial);
    const pending = { cueId, values: next };
    current.current = pending;
    setLocal(pending);
    onSave(next);
  };
  return { values, saveOutput };
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
