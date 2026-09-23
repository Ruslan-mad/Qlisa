export const CLIP_EDITOR_DOCK_HEIGHT = 250;
export const CLIP_EDITOR_DOCK_HEADER_HEIGHT = 36;
export const CLIP_EDITOR_DOCK_BORDER_HEIGHT = 1;
export const CLIP_EDITOR_TIMELINE_HEIGHT = 140;
export const CLIP_EDITOR_BODY_PADDING_TOP = 10;
export const CLIP_EDITOR_BODY_PADDING_BOTTOM = 6;

// Video timelines share the fixed canvas between the filmstrip and the
// audio waveform. Keep the split in one place so Timeline and Slice render
// the same geometry without changing the dock height.
export const VIDEO_TIMELINE_FILMSTRIP_RATIO = 0.65;
export const VIDEO_TIMELINE_WAVEFORM_RATIO = 0.35;

export function splitVideoTimelineHeight(totalHeight: number): {
  filmstripHeight: number;
  waveformHeight: number;
} {
  const safeHeight = Math.max(0, totalHeight);
  const filmstripHeight = Math.floor(safeHeight * VIDEO_TIMELINE_FILMSTRIP_RATIO);
  return {
    filmstripHeight,
    waveformHeight: safeHeight - filmstripHeight,
  };
}

// SliceTimeline has a 18px control row with a 3px bottom gap and, when there
// are multiple segments, a 24px badge row with a 2px top gap.
export const SLICE_TIMELINE_ZOOM_ROW_HEIGHT = 21;
export const SLICE_TIMELINE_BADGE_ROW_HEIGHT = 26;
export const CLIP_EDITOR_DOCK_SAFETY_GUTTER = 10;

export const MINIMUM_SLICE_DOCK_HEIGHT =
  CLIP_EDITOR_DOCK_BORDER_HEIGHT
  + CLIP_EDITOR_DOCK_HEADER_HEIGHT
  + CLIP_EDITOR_BODY_PADDING_TOP
  + CLIP_EDITOR_BODY_PADDING_BOTTOM
  + SLICE_TIMELINE_ZOOM_ROW_HEIGHT
  + CLIP_EDITOR_TIMELINE_HEIGHT
  + SLICE_TIMELINE_BADGE_ROW_HEIGHT;
