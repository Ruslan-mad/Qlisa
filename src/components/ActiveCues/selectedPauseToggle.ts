import type { WorkspaceCueCatalogEntry } from "../../lib/types";

/** Apply the selected-cue transport toggle used by both the toolbar and P. */
export async function toggleSelectedPauseTargets(
  targets: readonly WorkspaceCueCatalogEntry[],
  pause: (cueId: string) => Promise<unknown>,
  resume: (cueId: string) => Promise<unknown>,
  repeat = false,
): Promise<void> {
  if (repeat) return;
  if (targets.some((cue) => cue.state === "running")) {
    for (const cue of targets) await pause(cue.id);
  } else if (targets.some((cue) => cue.state === "paused")) {
    for (const cue of targets) await resume(cue.id);
  }
}
