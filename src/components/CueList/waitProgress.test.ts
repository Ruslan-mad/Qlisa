import { describe, expect, it } from "vitest";
import { waitProgressPercent } from "./waitProgress";

describe("waitProgressPercent", () => {
  it("shows pre-wait progress from event timing", () => {
    expect(waitProgressPercent({
      phase: "pre_wait", elapsedMs: 250, durationMs: 1000,
      columnPhase: "pre_wait",
      isRunning: true, isPaused: false,
    })).toBe(25);
    expect(waitProgressPercent({
      phase: "pre_wait", elapsedMs: 250, durationMs: 1000,
      columnPhase: "post_wait", isRunning: true, isPaused: false,
    })).toBeNull();
  });

  it("freezes the post-wait progress while paused", () => {
    expect(waitProgressPercent({
      phase: "post_wait", elapsedMs: 600, durationMs: 1000,
      columnPhase: "post_wait", isRunning: false, isPaused: true,
    })).toBe(60);
  });

  it("shows a pending Auto-Follow wait on a naturally completed standby cue", () => {
    expect(waitProgressPercent({
      phase: "post_wait", elapsedMs: 500, durationMs: 1000,
      columnPhase: "post_wait", isRunning: false, isPaused: false,
      isStandbyPostWait: true,
    })).toBe(50);
  });

  it("hides progress when no wait is active or the cell is edited", () => {
    const common = { elapsedMs: 250, durationMs: 1000, isRunning: true, isPaused: false };
    expect(waitProgressPercent({ ...common, phase: null })).toBeNull();
    expect(waitProgressPercent({ ...common, phase: "pre_wait", isEditing: true })).toBeNull();
  });
});
