import { clampSliderValue, stepSliderValue } from "../Inspector/sliderScaleModel";

export const MASTER_VOLUME_MIN_DB = -60;
export const MASTER_VOLUME_MAX_DB = 6;
export const MASTER_VOLUME_UNITY_DB = 0;
export const MASTER_VOLUME_STEP_DB = 0.5;
export const MASTER_VOLUME_UNITY_SNAP_DB = 0.75;

export function masterVolumeDbToRatio(db: number): number {
  return (clampSliderValue(db, MASTER_VOLUME_MIN_DB, MASTER_VOLUME_MAX_DB) - MASTER_VOLUME_MIN_DB)
    / (MASTER_VOLUME_MAX_DB - MASTER_VOLUME_MIN_DB);
}

export function normalizeMasterVolumeInput(db: number, source: "pointer" | "keyboard"): number {
  const bounded = clampSliderValue(db, MASTER_VOLUME_MIN_DB, MASTER_VOLUME_MAX_DB);
  return source === "pointer" && Math.abs(bounded - MASTER_VOLUME_UNITY_DB) <= MASTER_VOLUME_UNITY_SNAP_DB
    ? MASTER_VOLUME_UNITY_DB
    : bounded;
}

export function stepMasterVolume(db: number, direction: number): number {
  return stepSliderValue(db, direction, MASTER_VOLUME_MIN_DB, MASTER_VOLUME_MAX_DB, MASTER_VOLUME_STEP_DB);
}
