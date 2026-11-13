export type TimerTransportState = "standby" | "running" | "paused" | "completed" | string;
export type TimerPlayAction = "go" | "pause" | "resume";

export interface TimerPlayButtonModel {
  icon: "play" | "pause";
  action: TimerPlayAction;
  label: "start" | "pause" | "resume";
}

/** Use TimerCue's runtime zero latch; remaining_ms alone is ambiguous in pre-wait. */
export function timerPlayButtonModel(
  state: TimerTransportState,
  zeroReached: boolean,
  waitPhase: "pre_wait" | "post_wait" | null,
): TimerPlayButtonModel {
  const finishedCycle = zeroReached && waitPhase !== "pre_wait" && (state === "running" || state === "paused");
  if (state === "running" && !finishedCycle) return { icon: "pause", action: "pause", label: "pause" };
  if (state === "paused" && !finishedCycle) return { icon: "play", action: "resume", label: "resume" };
  return { icon: "play", action: "go", label: "start" };
}
