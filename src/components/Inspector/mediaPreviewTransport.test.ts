import { beforeEach, describe, expect, it } from "vitest";
import { useVideoPreviewTransport } from "./mediaPreviewTransport";

const identityA = "cue-a\u0000C:/show/a.mov";
const identityB = "cue-b\u0000C:/show/b.mov";

function activateReady(identity = identityA) {
  const state = useVideoPreviewTransport.getState();
  state.activate(identity, 1000, 5000);
  state.setReady(identity, true);
  state.setVisible(identity, true);
}

describe("video preview transport", () => {
  beforeEach(() => {
    const state = useVideoPreviewTransport.getState();
    if (state.identity) state.deactivate(state.identity);
  });

  it("refreshes cue bounds while preserving the user's paused position", () => {
    activateReady();
    let state = useVideoPreviewTransport.getState();
    state.seek(identityA, 3200);
    state.activate(identityA, 500, 4500);
    state = useVideoPreviewTransport.getState();
    expect(state.startMs).toBe(500);
    expect(state.durationMs).toBe(4500);
    expect(state.positionMs).toBe(3200);
    expect(state.ready).toBe(true);
    expect(state.visible).toBe(true);
  });

  it("uses decoded duration only while cue metadata has no usable duration", () => {
    const state = useVideoPreviewTransport.getState();
    state.activate(identityA, 7000, 0);
    state.setDurationFallback(identityA, 4000);
    let current = useVideoPreviewTransport.getState();
    expect(current.durationMs).toBe(4000);
    expect(current.startMs).toBe(4000);
    state.activate(identityA, 1000, 6000);
    state.setDurationFallback(identityA, 9000);
    current = useVideoPreviewTransport.getState();
    expect(current.durationMs).toBe(6000);
  });

  it("steps exactly one measured frame and pauses playback", () => {
    activateReady();
    const state = useVideoPreviewTransport.getState();
    state.setFrameRate(identityA, 25);
    state.seek(identityA, 1000);
    state.toggle(identityA);
    state.stepFrame(identityA, 1);
    const current = useVideoPreviewTransport.getState();
    expect(current.positionMs).toBe(1040);
    expect(current.playing).toBe(false);
  });

  it("ignores commands and late media events from a replaced cue identity", () => {
    activateReady(identityA);
    useVideoPreviewTransport.getState().activate(identityB, 0, 8000);
    const state = useVideoPreviewTransport.getState();
    state.seek(identityA, 2500);
    state.updateFromVideo(identityA, 6000);
    state.stop(identityA);
    expect(useVideoPreviewTransport.getState().identity).toBe(identityB);
    expect(useVideoPreviewTransport.getState().positionMs).toBe(0);
  });

  it("pauses on hidden state and deactivates cleanly on unmount", () => {
    activateReady();
    const state = useVideoPreviewTransport.getState();
    state.toggle(identityA);
    state.setVisible(identityA, false);
    expect(useVideoPreviewTransport.getState().playing).toBe(false);
    state.deactivate(identityA);
    expect(useVideoPreviewTransport.getState()).toMatchObject({
      identity: null,
      mounted: false,
      ready: false,
      visible: false,
      playing: false,
    });
  });
});
