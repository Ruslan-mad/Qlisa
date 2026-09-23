import { describe, expect, it } from "vitest";
import { durationProgressPercent } from "./durationProgress";

describe("cue-list Duration progress", () => {
  it("fills against total duration and freezes while paused", () => {
    const values = { state: "paused" as const, elapsedMs: 2_500, durationMs: 10_000, fileDurationMs: 4_000 };
    expect(durationProgressPercent(values)).toBe(25);
  });

  it("uses file length as a repeating cycle for an indefinite loop", () => {
    expect(durationProgressPercent({
      state: "running",
      elapsedMs: 5_000,
      durationMs: null,
      fileDurationMs: 2_000,
    })).toBe(50);
  });

  it("does not show a bar for standby cues or unknown durations", () => {
    expect(durationProgressPercent({
      state: "standby",
      elapsedMs: 500,
      durationMs: 1_000,
      fileDurationMs: 1_000,
    })).toBeNull();
    expect(durationProgressPercent({
      state: "running",
      elapsedMs: 500,
      durationMs: null,
      fileDurationMs: null,
    })).toBeNull();
  });

  it("hides the fill while the Duration value is being edited", () => {
    expect(durationProgressPercent({
      state: "running",
      elapsedMs: 1_000,
      durationMs: 2_000,
      fileDurationMs: 2_000,
      isEditing: true,
    })).toBeNull();
  });
});
