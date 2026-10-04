import type { VideoGeometry } from "../../lib/types";

export type GeometryOutputOverrides = Record<string, VideoGeometry>;

export interface CueGeometryOverridesSnapshot {
  cueId: string;
  values: GeometryOutputOverrides;
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
