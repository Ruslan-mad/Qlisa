import {
  Article, ArrowBendDownRight, ArrowCounterClockwise, ArrowUUpLeft,
  ArrowsClockwise, Broadcast, Camera, Clock, DownloadSimple,
  FilmStrip, FolderSimple, Globe, ImageSquare, Lightbulb, Microphone,
  MusicNote, NotePencil, NumberSquareOne, Pause, Play, ShieldCheck, ShieldSlash,
  SlidersHorizontal, SpeakerHigh, Stop, Terminal, TextT, Timer,
} from "@phosphor-icons/react";
import type { Icon } from "@phosphor-icons/react";
import type { CueType } from "../../lib/types";
import { CUE_TYPE_COLORS } from "../../lib/types";

/** Shared, exhaustive Phosphor icon vocabulary for every cue type. */
export type CueTypeIconSpec = Icon;

export const CUE_TYPE_ICON_SPECS = {
  audio: SpeakerHigh,
  memo: NotePencil,
  wait: Timer,
  group: FolderSimple,
  number: NumberSquareOne,
  fade: SlidersHorizontal,
  stop: Stop,
  devamp: ArrowsClockwise,
  video: FilmStrip,
  image: ImageSquare,
  osc: Broadcast,
  midi: MusicNote,
  midi_file: Article,
  light: Lightbulb,
  mic: Microphone,
  timecode: Clock,
  text: TextT,
  camera: Camera,
  browser: Globe,
  script: Terminal,
  start: Play,
  pause: Pause,
  resume: ArrowCounterClockwise,
  load: DownloadSimple,
  reset: ArrowUUpLeft,
  goto: ArrowBendDownRight,
  arm: ShieldCheck,
  disarm: ShieldSlash,
} satisfies Record<CueType, Icon>;

export interface CueTypeIconProps {
  type: CueType;
  size?: number;
  tone?: "neutral" | "type" | "inherit";
  label?: string;
}

export function CueTypeIcon({ type, size = 16, tone = "neutral", label }: CueTypeIconProps) {
  const IconComponent = CUE_TYPE_ICON_SPECS[type];
  const color = tone === "type"
    ? CUE_TYPE_COLORS[type]
    : tone === "inherit"
      ? "currentColor"
      : "var(--wc-text-bright)";

  return (
    <IconComponent
      size={size}
      weight="regular"
      color={color}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      focusable="false"
      data-cue-type={type}
      style={{ display: "block", flexShrink: 0 }}
    />
  );
}
