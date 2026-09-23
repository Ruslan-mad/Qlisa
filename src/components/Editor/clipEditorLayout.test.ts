import { describe, expect, it } from "vitest";
import {
  CLIP_EDITOR_DOCK_HEIGHT,
  CLIP_EDITOR_DOCK_SAFETY_GUTTER,
  MINIMUM_SLICE_DOCK_HEIGHT,
  splitVideoTimelineHeight,
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
});
