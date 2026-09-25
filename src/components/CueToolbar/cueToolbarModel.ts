import type { CueType } from "../../lib/types";

export type CueToolbarDescriptor = {
  type: CueType;
  labelKey: string;
  hintKey?: string;
  primary?: boolean;
};

// This is the single inventory used for both the main row and the More menu.
export const CUE_TOOLBAR_DESCRIPTORS: CueToolbarDescriptor[] = [
  { type: "audio", labelKey: "cueTypes.audio", primary: true },
  { type: "video", labelKey: "cueTypes.video", primary: true },
  { type: "image", labelKey: "cueTypes.image", primary: true },
  { type: "start", labelKey: "actions.start", primary: true },
  { type: "stop", labelKey: "cueTypes.stop", primary: true },
  { type: "fade", labelKey: "cueTypes.fade", primary: true },
  { type: "group", labelKey: "cueTypes.group", primary: true },
  { type: "number", labelKey: "cueTypes.number", primary: true },
  { type: "memo", labelKey: "cueTypes.memo", primary: true, hintKey: "toolbar.addMemo" },
  { type: "wait", labelKey: "cueTypes.wait" },
  { type: "text", labelKey: "cueTypes.text" },
  { type: "midi", labelKey: "cueTypes.midi" },
  { type: "midi_file", labelKey: "cueTypes.midiFile", hintKey: "toolbar.addMidiFile" },
  { type: "osc", labelKey: "cueTypes.osc" },
  { type: "light", labelKey: "cueTypes.light" },
  { type: "mic", labelKey: "cueTypes.mic" },
  { type: "timecode", labelKey: "cueTypes.timecode" },
  { type: "camera", labelKey: "cueTypes.camera", hintKey: "toolbar.addCamera" },
  { type: "browser", labelKey: "cueTypes.browser", hintKey: "toolbar.addBrowser" },
  { type: "devamp", labelKey: "cueTypes.devamp", hintKey: "toolbar.addDevamp" },
  { type: "script", labelKey: "cueTypes.script" },
  { type: "pause", labelKey: "actions.pause" },
  { type: "resume", labelKey: "actions.resume" },
  { type: "load", labelKey: "actions.load" },
  { type: "reset", labelKey: "actions.reset" },
  { type: "goto", labelKey: "actions.goto" },
  { type: "arm", labelKey: "actions.arm" },
  { type: "disarm", labelKey: "actions.disarm" },
];

// Least important buttons leave first. Image, Video, Audio leave only when
// there is no way to fit the preferred set beside the fixed right controls.
export const CUE_TOOLBAR_OVERFLOW_PRIORITY: CueType[] = [
  "memo", "number", "group", "fade", "stop", "start", "image", "video", "audio",
];

export function resolveCueToolbarLayout(
  availableWidth: number,
  itemWidths: Partial<Record<CueType, number>>,
  moreWidth: number,
  gap = 6,
) {
  const primary = CUE_TOOLBAR_DESCRIPTORS.filter((item) => item.primary);
  const primaryTypes = primary.map((item) => item.type);
  const visible = new Set(primaryTypes);
  const hasSecondary = CUE_TOOLBAR_DESCRIPTORS.some((item) => !item.primary);
  const requiredMore = hasSecondary || primaryTypes.length < CUE_TOOLBAR_DESCRIPTORS.length;
  const widthOf = (types: Iterable<CueType>) => {
    const widths = [...types].map((type) => itemWidths[type] ?? 0);
    return widths.reduce((sum, width) => sum + width, 0) + Math.max(0, widths.length - 1) * gap;
  };
  const total = () => widthOf(visible) + (requiredMore ? moreWidth + (visible.size ? gap : 0) : 0);

  for (const type of CUE_TOOLBAR_OVERFLOW_PRIORITY) {
    if (total() <= availableWidth) break;
    visible.delete(type);
  }
  const overflow = CUE_TOOLBAR_DESCRIPTORS
    .filter((item) => !item.primary || !visible.has(item.type))
    .map((item) => item.type);
  return { visible: primaryTypes.filter((type) => visible.has(type)), overflow, showMore: overflow.length > 0 };
}
