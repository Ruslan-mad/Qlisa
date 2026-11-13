import { describe, expect, it } from "vitest";
import { audioFadeWindows, clampFadeDurationFromHandle, effectiveGainAt, fadeDragCommit, fadeNotice, formatAudioFadeDuration, isFadeHandleHit } from "./audioFadeModel";

const base = {
  fadeInMs: 1000,
  fadeInCurve: "linear" as const,
  fadeOutMs: 1000,
  fadeOutCurve: "linear" as const,
  trimStartMs: 500,
  trimEndMs: 4500,
  rate: 2,
  loopCount: 0,
};

describe("audio fade preview model", () => {
  it("maps action-time fades to trimmed source coordinates at playback rate", () => {
    expect(audioFadeWindows(base, 5000)).toEqual([
      { direction: "in", startMs: 500, endMs: 2500, durationMs: 1000, curve: "linear" },
      { direction: "out", startMs: 2500, endMs: 4500, durationMs: 1000, curve: "linear" },
    ]);
  });

  it("keeps long fade-in progress instead of compressing it to the trimmed clip", () => {
    const windows = audioFadeWindows({ ...base, trimEndMs: 1500, fadeInMs: 5000, fadeOutMs: null }, 5000);
    expect(windows[0].endMs).toBe(10_500);
    expect(effectiveGainAt([], 1500, windows)).toBeCloseTo(0.1);
  });

  it("compresses an EOF fade longer than its clip to the remaining clip window", () => {
    const windows = audioFadeWindows({ ...base, fadeInMs: null, fadeOutMs: 5000 }, 5000);
    expect(windows[0]).toMatchObject({ startMs: 500, endMs: 4500, durationMs: 5000 });
    expect(effectiveGainAt([], 500, windows)).toBe(1);
    expect(effectiveGainAt([], 4500, windows)).toBe(0);
  });

  it("projects a long finite-repeat EOF fade from before the final pass", () => {
    const repeated = audioFadeWindows({ ...base, loopCount: 1, fadeInMs: null, fadeOutMs: 3000 }, 5000)[0];
    const single = audioFadeWindows({ ...base, loopCount: 0, fadeInMs: null, fadeOutMs: 3000 }, 5000)[0];
    expect(repeated).toMatchObject({ startMs: -1500, endMs: 4500 });
    expect(effectiveGainAt([], 500, [repeated])).toBeCloseTo(2 / 3);
    expect(single.startMs).toBe(500);
    expect(effectiveGainAt([], 500, [single])).toBe(1);
  });

  it("keeps null and explicit zero distinct, and does not divide by zero", () => {
    const windows = audioFadeWindows({ ...base, fadeInMs: 0, fadeOutMs: null }, 5000);
    expect(windows).toHaveLength(1);
    expect(windows[0].durationMs).toBe(0);
    expect(effectiveGainAt([{ time_ms: 1000, gain: 0 }], 1000, windows)).toBe(0);
    expect(clampFadeDurationFromHandle("in", 500, 500, 2)).toBe(0);
  });

  it("shows only the first-entry and last-exit projection for finite repeats", () => {
    expect(fadeNotice({ ...base, loopCount: 2 }, "en")).toContain("first pass");
  });

  it("does not invent EOF fade curves for infinite loops or slices", () => {
    expect(audioFadeWindows({ ...base, loopCount: 0xffff_ffff }, 5000).map((fade) => fade.direction)).toEqual(["in"]);
    expect(audioFadeWindows({ ...base, sliced: true }, 5000)).toEqual([]);
    expect(fadeNotice({ ...base, loopCount: 0xffff_ffff }, "ru")).toContain("STOP");
    expect(fadeNotice({ ...base, sliced: true }, "en")).toContain("STOP");
    expect(fadeNotice({ ...base, loopCount: 0xffff_ffff }, "en")).not.toContain("last pass");
    expect(fadeNotice({ ...base, sliced: true }, "ru")).toContain("в инспекторе");
  });

  it("composes fade with the authored gain envelope without changing its values", () => {
    const points = [{ time_ms: 500, gain: 0.5 }, { time_ms: 4500, gain: 0.5 }];
    const windows = audioFadeWindows(base, 5000);
    expect(effectiveGainAt(points, 1500, windows)).toBeCloseTo(0.25);
    expect(effectiveGainAt(points, 3500, windows)).toBeCloseTo(0.25);
    expect(points).toEqual([{ time_ms: 500, gain: 0.5 }, { time_ms: 4500, gain: 0.5 }]);
  });

  it("gives the EOF fade priority when it overlaps the initial fade", () => {
    const windows = audioFadeWindows({ ...base, trimEndMs: 2500 }, 5000);
    expect(windows).toHaveLength(2);
    // At the midpoint both windows overlap. The engine's single FadeState is
    // replaced by the EOF fade, rather than multiplying both fades.
    expect(effectiveGainAt([], 1500, windows)).toBeCloseTo(0.5);
  });

  it("uses a small dedicated fade-handle target and never commits a canceled drag", () => {
    expect(isFadeHandleHit(106, 100, 100, 100)).toBe(true);
    expect(isFadeHandleHit(108, 100, 100, 100)).toBe(false);
    expect(fadeDragCommit(500, 700, true)).toBe(700);
    expect(fadeDragCommit(500, 700, true, true)).toBeNull();
    expect(fadeDragCommit(500, 500, false)).toBeNull();
  });

  it("formats the duration beside a handle without exposing the raw curve id", () => {
    expect(formatAudioFadeDuration(2000, "ru")).toBe("2,0 с");
    expect(formatAudioFadeDuration(125_000, "en")).toBe("2:05");
    expect(formatAudioFadeDuration(25, "ru")).toBe("25 мс");
  });
});
