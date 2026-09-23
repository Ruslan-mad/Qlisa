import { beforeEach, describe, expect, it } from "vitest";

import { useTimingStore } from "../timingStore";

beforeEach(() => {
  useTimingStore.setState({
    timings: {},
    previewPlayhead: null,
    previewPlayheadGeneration: -1,
  });
});

describe("headphone preview timing", () => {
  it("tracks the isolated preview cursor without creating cue timing", () => {
    useTimingStore.getState().setPreviewPlayhead({
      cue_id: "preview-cue",
      media_position_ms: 12_345,
      playing: true,
      active: true,
      generation: 4,
    });

    const state = useTimingStore.getState();
    expect(state.timings).toEqual({});
    expect(state.previewPlayhead).toMatchObject({
      cue_id: "preview-cue",
      media_position_ms: 12_345,
      playing: true,
      active: true,
    });
  });

  it("clears on a terminal event and ignores a delayed older generation", () => {
    const store = useTimingStore.getState();
    store.setPreviewPlayhead({
      cue_id: "cue-new",
      media_position_ms: 8_000,
      playing: true,
      active: true,
      generation: 9,
    });
    store.setPreviewPlayhead({
      cue_id: "cue-old",
      media_position_ms: null,
      playing: false,
      active: false,
      generation: 8,
    });
    expect(useTimingStore.getState().previewPlayhead?.cue_id).toBe("cue-new");

    useTimingStore.getState().setPreviewPlayhead({
      cue_id: "cue-new",
      media_position_ms: null,
      playing: false,
      active: false,
      generation: 9,
    });
    expect(useTimingStore.getState().previewPlayhead).toBeNull();
    expect(useTimingStore.getState().previewPlayheadGeneration).toBe(9);

    // A queued active packet from the voice that just ended is stale too.
    useTimingStore.getState().setPreviewPlayhead({
      cue_id: "cue-new",
      media_position_ms: 8_100,
      playing: true,
      active: true,
      generation: 9,
    });
    expect(useTimingStore.getState().previewPlayhead).toBeNull();
  });
});
