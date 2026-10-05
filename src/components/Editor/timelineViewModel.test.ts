import { describe, expect, it } from "vitest";
import { mediaSourceMsAtPixel, panTimelineView, sourceRangeForTimelinePass, waveformBinAtSourceMs, zoomTimelineView } from "./timelineViewModel";

describe("timeline viewport", () => {
  it("zooms around the pointer and exposes a smaller visible range", () => {
    const view = zoomTimelineView(10000, null, 2, 5000);
    expect(view?.startMs).toBe(2500);
    expect(view?.endMs).toBe(7500);
  });

  it("pans a zoomed range without leaving the media bounds", () => {
    expect(panTimelineView(10000, { startMs: 2000, endMs: 6000 }, 3000)).toEqual({ startMs: 5000, endMs: 9000 });
    expect(panTimelineView(10000, { startMs: 2000, endMs: 6000 }, -3000)).toEqual({ startMs: 0, endMs: 4000 });
  });

  it("maps a zoomed pixel to the visible source range, not the clamped clip range", () => {
    expect(mediaSourceMsAtPixel(0, 1000, 0, 10000, 5000, 7500, 0, 10000)).toBe(5000);
    expect(mediaSourceMsAtPixel(1000, 1000, 0, 10000, 5000, 7500, 0, 10000)).toBe(7500);
  });

  it("samples trimmed ranges against cached full-file peaks", () => {
    const peaks = [1, 1, 1, 1, 1, 1, 0, 0, 0, 0];
    const binAt = (sourceMs: number) => waveformBinAtSourceMs(sourceMs, 10_000, peaks.length);

    // End trim sees only the loud 0–6s range. Start trim still maps to the
    // silent 6–10s source tail. A crop on both sides keeps absolute mapping.
    const endTrimSource = mediaSourceMsAtPixel(1000, 1000, 0, 6000, 0, 6000, 0, 6000);
    const startTrimSource = mediaSourceMsAtPixel(1000, 1000, 0, 4000, 0, 4000, 6000, 10_000);
    const bothTrimSource = mediaSourceMsAtPixel(1000, 1000, 0, 4000, 0, 4000, 2000, 6000);
    expect(peaks[binAt(endTrimSource - 1)]).toBe(1);
    expect(peaks[binAt(startTrimSource - 1)]).toBe(0);
    expect(peaks[binAt(bothTrimSource - 1)]).toBe(1);
  });

  it("maps a short final repeat pass to only the matching part of the crop", () => {
    expect(sourceRangeForTimelinePass(12_000, 14_500, 2_000, 6_000)).toEqual({ startMs: 2_000, endMs: 4_500 });
    expect(sourceRangeForTimelinePass(12_000, 16_000, 2_000, 6_000)).toEqual({ startMs: 2_000, endMs: 6_000 });
  });
});
