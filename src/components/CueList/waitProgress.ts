export interface WaitProgressInput {
  phase: "pre_wait" | "post_wait" | null | undefined;
  columnPhase?: "pre_wait" | "post_wait";
  elapsedMs: number | null | undefined;
  durationMs: number | null | undefined;
  isPaused: boolean;
  isRunning: boolean;
  isStandbyPostWait?: boolean;
  isEditing?: boolean;
}

/** Return progress for the current transport wait. Time comes from cue timing events. */
export function waitProgressPercent({
  phase,
  columnPhase,
  elapsedMs,
  durationMs,
  isPaused,
  isRunning,
  isStandbyPostWait = false,
  isEditing = false,
}: WaitProgressInput): number | null {
  if (isEditing || (!isRunning && !isPaused && !(phase === "post_wait" && isStandbyPostWait)) || !phase) return null;
  if (columnPhase && phase !== columnPhase) return null;
  if (durationMs == null || !Number.isFinite(durationMs) || durationMs <= 0) return null;
  const elapsed = Number.isFinite(elapsedMs) ? Math.max(0, elapsedMs ?? 0) : 0;
  return Math.min(100, (elapsed / durationMs) * 100);
}
