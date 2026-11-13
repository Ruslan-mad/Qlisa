export const CLIP_EDITOR_DOCK_HEIGHT = 250;
export const CLIP_EDITOR_DOCK_HEADER_HEIGHT = 36;
export const CLIP_EDITOR_LIVE_HEADER_COMPACT_WIDTH = 600;
export const CLIP_EDITOR_LIVE_HEADER_COMPACT_HEIGHT = 62;
export const CLIP_EDITOR_DOCK_BORDER_HEIGHT = 1;
export const CLIP_EDITOR_TIMELINE_HEIGHT = 140;
// Preview reserves room for its zoom row, gain-envelope hint, and the slice
// play-count row inside the fixed dock.
export const CLIP_EDITOR_PREVIEW_TIMELINE_HEIGHT = 124;
export const CLIP_EDITOR_BODY_PADDING_TOP = 10;
export const CLIP_EDITOR_BODY_PADDING_BOTTOM = 6;
const CLIP_EDITOR_LIVE_ZOOM_ROW_HEIGHT = 25;
const CLIP_EDITOR_LIVE_TIME_LABEL_ROW_HEIGHT = 15;
const CLIP_EDITOR_LIVE_BOTTOM_GUTTER = 4;

export function usesCompactLiveHeader(width: number): boolean {
  return width < CLIP_EDITOR_LIVE_HEADER_COMPACT_WIDTH;
}

/** Fit the canvas and its time labels inside the fixed editor dock. */
export function liveTimelineCanvasHeight(
  headerHeight: number,
  options: { numberBands?: boolean; gainHint?: boolean; reservedHeight?: number } = {},
): number {
  const numberBandHeight = options.numberBands ? 25 : 0;
  const gainHintHeight = options.gainHint ? 15 : 0;
  const available = CLIP_EDITOR_DOCK_HEIGHT
    - CLIP_EDITOR_DOCK_BORDER_HEIGHT
    - headerHeight
    - CLIP_EDITOR_BODY_PADDING_TOP
    - CLIP_EDITOR_BODY_PADDING_BOTTOM
    - Math.max(0, options.reservedHeight ?? 0)
    - CLIP_EDITOR_LIVE_ZOOM_ROW_HEIGHT
    - numberBandHeight
    - gainHintHeight
    - CLIP_EDITOR_LIVE_TIME_LABEL_ROW_HEIGHT
    - CLIP_EDITOR_LIVE_BOTTOM_GUTTER;
  return Math.max(0, Math.min(CLIP_EDITOR_TIMELINE_HEIGHT, available));
}

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
export const SLICE_TIMELINE_GAIN_HINT_HEIGHT = 15;
export const SLICE_TIMELINE_BADGE_ROW_HEIGHT = 26;
export const CLIP_EDITOR_DOCK_SAFETY_GUTTER = 10;

export const MINIMUM_SLICE_DOCK_HEIGHT =
  CLIP_EDITOR_DOCK_BORDER_HEIGHT
  + CLIP_EDITOR_DOCK_HEADER_HEIGHT
  + CLIP_EDITOR_BODY_PADDING_TOP
  + CLIP_EDITOR_BODY_PADDING_BOTTOM
  + SLICE_TIMELINE_ZOOM_ROW_HEIGHT
  + SLICE_TIMELINE_GAIN_HINT_HEIGHT
  + CLIP_EDITOR_PREVIEW_TIMELINE_HEIGHT
  + SLICE_TIMELINE_BADGE_ROW_HEIGHT;
