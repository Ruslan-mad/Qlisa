import type { CueSummary } from "../../lib/types";

/** Format milliseconds as elapsed minutes and seconds without wrapping hours. */
export function formatDurationMs(durationMs: number | null | undefined): string {
  if (durationMs == null || !Number.isFinite(durationMs)) return "—";
  const wholeSeconds = Math.floor(Math.max(0, durationMs) / 1_000);
  const minutes = Math.floor(wholeSeconds / 60);
  const seconds = wholeSeconds % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

/** Infinite Audio/Video loops have no finite duration to format. */
export function formatCueDuration(cue: Pick<CueSummary, "cue_type" | "loop_count" | "duration_ms">): string {
  if ((cue.cue_type === "audio" || cue.cue_type === "video") && cue.loop_count === 0xffff_ffff) {
    return "∞";
  }
  return formatDurationMs(cue.duration_ms);
}
