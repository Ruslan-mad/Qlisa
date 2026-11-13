import { describe, expect, it } from "vitest";
import {
  CLIP_EDITOR_DOCK_HEIGHT,
  CLIP_EDITOR_DOCK_SAFETY_GUTTER,
  CLIP_EDITOR_DOCK_HEADER_HEIGHT,
  CLIP_EDITOR_LIVE_HEADER_COMPACT_HEIGHT,
  liveTimelineCanvasHeight,
  MINIMUM_SLICE_DOCK_HEIGHT,
  splitVideoTimelineHeight,
  usesCompactLiveHeader,
  VIDEO_TIMELINE_FILMSTRIP_RATIO,
  VIDEO_TIMELINE_WAVEFORM_RATIO,
} from "./clipEditorLayout";

describe("clip editor dock layout", () => {
  it("fits the Slice timeline zoom row, canvas, segment badges, and a safety gutter", () => {
    expect(CLIP_EDITOR_DOCK_HEIGHT).toBeGreaterThanOrEqual(
      MINIMUM_SLICE_DOCK_HEIGHT + CLIP_EDITOR_DOCK_SAFETY_GUTTER,
    );
  });

  it("keeps the video canvas fixed while reserving 65/35 for filmstrip and waveform", () => {
    const totalHeight = 140;
    const split = splitVideoTimelineHeight(totalHeight);

    expect(split.filmstripHeight + split.waveformHeight).toBe(totalHeight);
    expect(split.filmstripHeight / totalHeight).toBeCloseTo(VIDEO_TIMELINE_FILMSTRIP_RATIO, 1);
    expect(split.waveformHeight / totalHeight).toBeCloseTo(VIDEO_TIMELINE_WAVEFORM_RATIO, 1);
  });

  it("preserves non-negative boundary heights without inventing pixels", () => {
    expect(splitVideoTimelineHeight(0)).toEqual({ filmstripHeight: 0, waveformHeight: 0 });
    expect(splitVideoTimelineHeight(0.5).filmstripHeight + splitVideoTimelineHeight(0.5).waveformHeight).toBe(0.5);
    expect(splitVideoTimelineHeight(-10)).toEqual({ filmstripHeight: 0, waveformHeight: 0 });
  });

  it("switches the Live header to a second row before its controls can overlap", () => {
    expect(usesCompactLiveHeader(600)).toBe(false);
    expect(usesCompactLiveHeader(599)).toBe(true);
  });

  it("reserves room for bottom time labels and Number bands inside the fixed dock", () => {
    const ordinary = liveTimelineCanvasHeight(CLIP_EDITOR_DOCK_HEADER_HEIGHT);
    const compact = liveTimelineCanvasHeight(CLIP_EDITOR_LIVE_HEADER_COMPACT_HEIGHT);
    const number = liveTimelineCanvasHeight(CLIP_EDITOR_DOCK_HEADER_HEIGHT, { numberBands: true });

    expect(ordinary).toBe(140);
    expect(compact).toBeLessThan(ordinary);
    expect(number).toBeLessThan(ordinary);
    expect(number + 25 + 25 + 15 + 16 + CLIP_EDITOR_DOCK_HEADER_HEIGHT + 1 + 4)
      .toBeLessThanOrEqual(CLIP_EDITOR_DOCK_HEIGHT);
  });

  it("subtracts measured presentation navigation rows from the canvas budget", () => {
    const base = liveTimelineCanvasHeight(CLIP_EDITOR_DOCK_HEADER_HEIGHT);
    const singleRow = liveTimelineCanvasHeight(CLIP_EDITOR_DOCK_HEADER_HEIGHT, { reservedHeight: 44 });
    const wrappedRows = liveTimelineCanvasHeight(CLIP_EDITOR_LIVE_HEADER_COMPACT_HEIGHT, { reservedHeight: 72 });
    expect(singleRow).toBeLessThan(base);
    expect(wrappedRows).toBeLessThan(singleRow);
    expect(liveTimelineCanvasHeight(CLIP_EDITOR_DOCK_HEADER_HEIGHT, { reservedHeight: 500 })).toBe(0);
  });
});
