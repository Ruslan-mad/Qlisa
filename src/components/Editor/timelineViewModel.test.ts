import { describe, expect, it } from "vitest";
import { mediaSourceMsAtPixel, panTimelineView, zoomTimelineView } from "./timelineViewModel";

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
});
