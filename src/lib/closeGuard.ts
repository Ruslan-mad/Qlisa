import type { CueSummary } from "./types";

/**
 * Returns true when at least one cue (including a nested group child) still
 * owns live playback state. A paused cue is included because closing Qlisa
 * still tears down its loaded media and is therefore just as destructive as
 * closing while it is running.
 */
export function hasActivePlayback(cues: CueSummary[]): boolean {
  return cues.some((cue) =>
    cue.state === "running"
    || cue.state === "paused"
    || (cue.children ? hasActivePlayback(cue.children) : false),
  );
}
