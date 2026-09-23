import type { OutputMonitorFrame } from "../lib/commands";

export interface OutputMonitorFrameState {
  sourceId: string | null;
  sequence: number | null;
  dataUrl: string | null;
  status: OutputMonitorFrame["status"];
  error: string | null;
}

export const EMPTY_OUTPUT_MONITOR_FRAME: OutputMonitorFrameState = {
  sourceId: null,
  sequence: null,
  dataUrl: null,
  status: "no_frame",
  error: null,
};

/** Never paint a frame captured for the tab that was selected previously. */
export function frameForOutputSource(
  frame: OutputMonitorFrameState,
  sourceId: string,
): OutputMonitorFrameState {
  return frame.sourceId === sourceId ? frame : EMPTY_OUTPUT_MONITOR_FRAME;
}

/** Merge a poll result without blanking a perfectly usable previous frame. */
export function mergeOutputMonitorFrame(
  previous: OutputMonitorFrameState,
  next: OutputMonitorFrame,
): OutputMonitorFrameState {
  if (next.status === "frame" && next.data_url) {
    return {
      sourceId: next.source_id,
      sequence: next.sequence,
      dataUrl: next.data_url,
      status: "frame",
      error: null,
    };
  }
  if (next.status === "unchanged") {
    return {
      ...previous,
      sourceId: next.source_id,
      sequence: next.sequence,
      error: null,
    };
  }
  return {
    sourceId: next.source_id,
    sequence: next.sequence,
    dataUrl: null,
    status: next.status,
    error: next.error,
  };
}
