/** Format milliseconds as elapsed minutes and seconds without wrapping hours. */
export function formatDurationMs(durationMs: number | null | undefined): string {
  if (durationMs == null || !Number.isFinite(durationMs)) return "—";
  const wholeSeconds = Math.floor(Math.max(0, durationMs) / 1_000);
  const minutes = Math.floor(wholeSeconds / 60);
  const seconds = wholeSeconds % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}
