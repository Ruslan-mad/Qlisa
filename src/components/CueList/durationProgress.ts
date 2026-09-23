import type { CueSummary } from "../../lib/types";

interface DurationProgressInput {
  state: CueSummary["state"];
  elapsedMs: number | null | undefined;
  durationMs: number | null;
  fileDurationMs: number | null;
  isEditing?: boolean;
}

/** Progress shown in the Duration cell. Indefinite loops use one file cycle. */
export function durationProgressPercent({
  state,
  elapsedMs,
  durationMs,
  fileDurationMs,
  isEditing = false,
}: DurationProgressInput): number | null {
  if (isEditing) return null;
  if (state !== "running" && state !== "paused") return null;

  const totalMs = durationMs != null && durationMs > 0
    ? durationMs
    : fileDurationMs;
  if (totalMs == null || !Number.isFinite(totalMs) || totalMs <= 0) return null;

  const elapsed = Number.isFinite(elapsedMs) ? Math.max(0, elapsedMs ?? 0) : 0;
  const position = durationMs == null && fileDurationMs != null && fileDurationMs > 0
    ? elapsed % fileDurationMs
    : elapsed;
  return Math.min(100, (position / totalMs) * 100);
}
