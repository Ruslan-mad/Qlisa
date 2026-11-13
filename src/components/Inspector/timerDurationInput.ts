const MAX_TIMER_DURATION_MS = 86_400_000;

export function formatTimerDurationInput(durationMs: number): string {
  const value = Math.max(0, Math.min(MAX_TIMER_DURATION_MS, Math.round(durationMs)));
  const wholeSeconds = Math.floor(value / 1_000);
  const minutes = Math.floor(wholeSeconds / 60);
  const seconds = wholeSeconds % 60;
  const milliseconds = value % 1_000;
  return `${minutes}:${String(seconds).padStart(2, "0")}${milliseconds ? `.${String(milliseconds).padStart(3, "0")}` : ""}`;
}

/** Parse M:SS, with an optional millisecond fraction, into the cue's ms value. */
export function parseTimerDurationInput(input: string): number | null {
  const match = /^(\d+):(\d{1,2})(?:\.(\d{1,3}))?$/.exec(input.trim());
  if (!match) return null;
  const minutes = Number(match[1]);
  const seconds = Number(match[2]);
  if (!Number.isSafeInteger(minutes) || seconds > 59) return null;
  const milliseconds = match[3] ? Number(match[3].padEnd(3, "0")) : 0;
  const value = minutes * 60_000 + seconds * 1_000 + milliseconds;
  return Number.isSafeInteger(value) && value <= MAX_TIMER_DURATION_MS ? value : null;
}
