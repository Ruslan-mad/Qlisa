import type { CueTiming } from "../../stores/timingStore";
import { formatDurationMs } from "../CueList/formatDuration";

type LiveCueDuration = { cue_type: string; loop_count?: number; duration_ms?: number | null };

/** Format the active Live cue's duration and backend-reported runtime remaining. */
export function liveTimingLabels(
  cue: LiveCueDuration,
  _mediaDurationMs: number | null,
  timing: CueTiming | undefined,
): { duration: string; remaining: string } {
  // Cue duration is authoritative: it accounts for trims, loops, and rate.
  // File metadata can be longer than the actual action and must never replace it.
  const duration = ((cue.cue_type === "audio" || cue.cue_type === "video") && cue.loop_count === 0xffff_ffff)
    ? "∞"
    : formatDurationMs(cue.duration_ms);
  const indefinite = duration === "∞";
  const remaining = indefinite
    ? "∞"
    : timing == null
      ? "—"
      : formatDurationMs(timing.remaining_ms);

  return { duration, remaining };
}
