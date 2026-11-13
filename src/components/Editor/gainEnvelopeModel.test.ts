import { describe, expect, it } from "vitest";
import type { GainEnvelopePoint } from "../../lib/types";
import { addEnvelopePoint, deleteEnvelopePoint, displayedEnvelopePoints, ENVELOPE_VERTICAL_INSET, envelopeDb, envelopeGainAt, envelopeGainFromY, envelopeGainY, envelopePercent, envelopeUnityY, GAIN_ENVELOPE_MAX_GAIN, GAIN_ENVELOPE_MAX_POINTS, hitTestEnvelope, moveEnvelopePoint, moveEnvelopeSegment, resetEnvelopePointGain, splitEnvelopeAtUnity } from "./gainEnvelopeModel";

const points: GainEnvelopePoint[] = [
  { time_ms: 0, gain: 1 },
  { time_ms: 1_000, gain: 0.8 },
  { time_ms: 2_000, gain: 0.8 },
  { time_ms: 3_000, gain: 1 },
];

describe("gainEnvelopeModel", () => {
  it("shows implicit unity anchors for an unedited cue without persisting points", () => {
    expect(displayedEnvelopePoints([], 3_000)).toEqual([
      { time_ms: 0, gain: 1 },
      { time_ms: 3_000, gain: 1 },
    ]);
    expect(envelopeGainAt([], 1_500)).toBe(1);
  });

  it("adds a point between its neighbors and keeps explicit outer anchors", () => {
    expect(addEnvelopePoint([], 1_500, 0.4, 3_000)).toEqual([
      { time_ms: 0, gain: 1 },
      { time_ms: 1_500, gain: 0.4 },
      { time_ms: 3_000, gain: 1 },
    ]);
  });

  it("clamps a dragged point between neighbors while preserving neighbors", () => {
    expect(moveEnvelopePoint(points, 1, 2_500, 0.3, 3_000)).toEqual([
      points[0],
      { time_ms: 1_999, gain: 0.3 },
      points[2],
      points[3],
    ]);
  });

  it("moves only both boundaries of a segment, leaving its outer neighbors alone", () => {
    expect(moveEnvelopeSegment(points, 1, -0.25)).toEqual([
      points[0],
      { ...points[1], gain: 0.55 },
      { ...points[2], gain: 0.55 },
      points[3],
    ]);
  });

  it("maps unity to y=40% with a bounded 0–200% boost region", () => {
    expect(envelopeUnityY(100)).toBe(40);
    expect(envelopeGainY(GAIN_ENVELOPE_MAX_GAIN, 100)).toBe(ENVELOPE_VERTICAL_INSET);
    expect(envelopeGainY(1, 100)).toBe(40);
    expect(envelopeGainY(0, 100)).toBe(100 - ENVELOPE_VERTICAL_INSET);
    expect(envelopeGainFromY(ENVELOPE_VERTICAL_INSET, 100)).toBe(2);
    expect(envelopeGainFromY(40, 100)).toBe(1);
    expect(envelopeGainFromY(100 - ENVELOPE_VERTICAL_INSET, 100)).toBe(0);
    expect(envelopePercent(1.5)).toBe(150);
    expect(envelopePercent(2)).toBe(200);
    expect(envelopeDb(1.5)).toBe("+3.5 dB");
    expect(envelopeDb(2)).toBe("+6.0 dB");
  });

  it("resets only one point to unity without changing its source time or neighbors", () => {
    const boosted = [points[0], { time_ms: 1_250, gain: 1.6 }, points[2], points[3]];
    expect(resetEnvelopePointGain(boosted, 1)).toEqual([
      points[0], { time_ms: 1_250, gain: 1 }, points[2], points[3],
    ]);
    expect(resetEnvelopePointGain(boosted, 0)).toBe(boosted);
  });

  it("inserts only a render-only unity crossing for a boosted-to-attenuated segment", () => {
    const crossing = splitEnvelopeAtUnity([
      { time_ms: 0, gain: 0.5 },
      { time_ms: 100, gain: 1.5 },
    ]);
    expect(crossing).toEqual([
      { time_ms: 0, gain: 0.5 },
      { time_ms: 50, gain: 1 },
      { time_ms: 100, gain: 1.5 },
    ]);
    const crossingView = { startMs: 0, endMs: 100, width: 400, height: 100 };
    const crossingPoints = [
      { time_ms: 0, gain: 0.5 },
      { time_ms: 100, gain: 1.5 },
    ];
    expect(hitTestEnvelope(crossingPoints, 100, envelopeGainY(0.75, 100), crossingView))
      .toEqual({ kind: "segment", index: 0 });
    expect(hitTestEnvelope(crossingPoints, 300, envelopeGainY(1.25, 100), crossingView))
      .toEqual({ kind: "segment", index: 0 });
  });

  it("uses a 10 px minimum point hit radius and a 7 px half-width line hit area", () => {
    const view = { startMs: 0, endMs: 3_000, width: 600, height: 100 };
    const pointY = envelopeGainY(0.8, view.height);
    expect(hitTestEnvelope(points, 199, pointY, view)).toEqual({ kind: "point", index: 1 });
    expect(hitTestEnvelope(points, 210, pointY, view)).toEqual({ kind: "point", index: 1 });
    expect(hitTestEnvelope(points, 300, pointY, view)).toEqual({ kind: "segment", index: 1 });
    expect(hitTestEnvelope(points, 300, pointY + 8, view)).toBeNull();
    expect(hitTestEnvelope([], 300, envelopeGainY(1, view.height), view)).toEqual({ kind: "segment", index: -1 });
  });

  it("keeps unity and silence inside the SVG hit area and caps additions at 128 points", () => {
    const view = { startMs: 0, endMs: 100, width: 400, height: 100 };
    expect(hitTestEnvelope([{ time_ms: 0, gain: 1 }, { time_ms: 100, gain: 0 }], 0, envelopeGainY(1, view.height), view))
      .toEqual({ kind: "point", index: 0 });
    expect(hitTestEnvelope([{ time_ms: 0, gain: 1 }, { time_ms: 100, gain: 0 }], 400, 100 - ENVELOPE_VERTICAL_INSET, view))
      .toEqual({ kind: "point", index: 1 });
    const full = Array.from({ length: GAIN_ENVELOPE_MAX_POINTS }, (_, index) => ({ time_ms: index, gain: 1 }));
    expect(addEnvelopePoint(full, 50.5, 0.5, 200)).toBe(full);
  });

  it("deletes only interior points and reports percent plus dB, including silence", () => {
    expect(deleteEnvelopePoint(points, 2)).toEqual([points[0], points[1], points[3]]);
    expect(deleteEnvelopePoint(points, 0)).toBe(points);
    expect(envelopePercent(0.5)).toBe(50);
    expect(envelopeDb(0.5)).toBe("-6.0 dB");
    expect(envelopeDb(0)).toBe("−∞ dB");
  });
});
