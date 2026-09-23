// Per-cue timing data updated at ~30 fps from cue-time-update events.

import { create } from "zustand";
import type { CueId, PreviewPlayheadEvent } from "../lib/types";

export interface CueTiming {
  elapsed_ms: number;
  action_elapsed_ms: number;
  remaining_ms: number;
  /** File-relative playhead for Audio/Video leaves; null for other cues. */
  media_position_ms: number | null;
}

interface TimingState {
  timings: Record<CueId, CueTiming>;
  /** The one operator-local headphone preview, separate from show cue timing. */
  previewPlayhead: PreviewPlayheadEvent | null;
  /** Retained after a terminal event so delayed older events cannot revive it. */
  previewPlayheadGeneration: number;
  setTiming: (cueId: CueId, t: CueTiming) => void;
  clearTiming: (cueId: CueId) => void;
  setPreviewPlayhead: (event: PreviewPlayheadEvent) => void;
}

export const useTimingStore = create<TimingState>((set) => ({
  timings: {},
  previewPlayhead: null,
  previewPlayheadGeneration: -1,

  setTiming: (cueId, t) =>
    set((s) => ({ timings: { ...s.timings, [cueId]: t } })),

  clearTiming: (cueId) =>
    set((s) => {
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { [cueId]: _removed, ...rest } = s.timings;
      return { timings: rest };
    }),

  setPreviewPlayhead: (event) =>
    set((state) => {
      // Once a generation has ended, an out-of-order active packet from that
      // same voice is never allowed to resurrect the preview cursor. A new
      // preview always has a higher generation.
      if (
        event.generation < state.previewPlayheadGeneration
        || (event.generation === state.previewPlayheadGeneration
          && state.previewPlayhead === null
          && event.active)
      ) return state;
      return {
        previewPlayhead: event.active ? event : null,
        previewPlayheadGeneration: event.generation,
      };
    }),
}));
