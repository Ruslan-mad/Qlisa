import type { CueType } from "../../lib/types";
import { CUE_TYPE_COLORS } from "../../lib/types";

export interface CueTypeIconSpec {
  /** One or more stroked SVG paths in the shared 24 × 24 coordinate space. */
  paths: readonly string[];
}

/**
 * The single cue-icon vocabulary used throughout the application.
 *
 * Keeping the map exhaustive makes a newly-added CueType fail TypeScript
 * compilation until it has a deliberate, recognisable icon of its own.
 */
export const CUE_TYPE_ICON_SPECS = {
  audio: {
    paths: [
      "M4 10h3l4-4v12l-4-4H4z",
      "M15 9a4 4 0 0 1 0 6",
      "M17.5 6.5a7.5 7.5 0 0 1 0 11",
    ],
  },
  memo: {
    paths: [
      "M6 3h9l3 3v15H6z",
      "M15 3v4h4",
      "M9 11h6",
      "M9 15h6",
    ],
  },
  wait: {
    paths: [
      "M12 7v5l3 2",
      "M5.6 5.6a9 9 0 1 1-1.7 2.5",
      "M4 3v5h5",
    ],
  },
  group: {
    paths: [
      "M3 6h7l2 2h9v11H3z",
      "M7 13h10",
      "M9 16h6",
    ],
  },
  number: {
    paths: [
      "M5 4h14v16H5z",
      "M8 8h8",
      "M8 12h5",
      "M8 16h7",
    ],
  },
  fade: {
    paths: [
      "M4 5v14h16",
      "M6 7c4 0 4 10 12 10",
      "M16 14l2 3 2-3",
    ],
  },
  stop: {
    paths: [
      "M6 6h12v12H6z",
    ],
  },
  devamp: {
    paths: [
      "M7 8a6 6 0 0 1 10 1",
      "M17 5v4h-4",
      "M17 16a6 6 0 0 1-10-1",
      "M7 19v-4h4",
      "M12 10v4",
      "M12 17v.01",
    ],
  },
  video: {
    paths: [
      "M3 6h18v12H3z",
      "M7 6v12",
      "M17 6v12",
      "M10 9l5 3-5 3z",
    ],
  },
  image: {
    paths: [
      "M3 5h18v14H3z",
      "M6 16l4-4 3 3 2-2 3 3",
      "M16.5 8.5h.01",
    ],
  },
  osc: {
    paths: [
      "M12 12h.01",
      "M8.5 8.5a5 5 0 0 0 0 7",
      "M15.5 8.5a5 5 0 0 1 0 7",
      "M5.5 5.5a9 9 0 0 0 0 13",
      "M18.5 5.5a9 9 0 0 1 0 13",
    ],
  },
  midi: {
    paths: [
      "M5 17a8 8 0 1 1 14 0",
      "M8 9h.01",
      "M12 7.5v.01",
      "M16 9h.01",
      "M9 14v3",
      "M15 14v3",
    ],
  },
  midi_file: {
    paths: [
      "M5 3h9l4 4v14H5z",
      "M14 3v5h5",
      "M14 11v6",
      "M14 12l-4 1v5",
      "M8.5 18a1.5 1.2 0 1 0 1.5 1.2",
      "M12.5 17a1.5 1.2 0 1 0 1.5 1.2",
    ],
  },
  light: {
    paths: [
      "M9 17h6",
      "M10 21h4",
      "M8 13a6 6 0 1 1 8 0c-1 1-1 2-1 3H9c0-1 0-2-1-3z",
      "M12 1v2",
      "M4.2 4.2l1.4 1.4",
      "M19.8 4.2l-1.4 1.4",
    ],
  },
  mic: {
    paths: [
      "M9 5a3 3 0 0 1 6 0v7a3 3 0 0 1-6 0z",
      "M5.5 11.5a6.5 6.5 0 0 0 13 0",
      "M12 18v3",
      "M9 21h6",
    ],
  },
  timecode: {
    paths: [
      "M7 3H3v4",
      "M17 3h4v4",
      "M7 21H3v-4",
      "M17 21h4v-4",
      "M12 7v5l3 2",
      "M12 18a6 6 0 1 0 0-12 6 6 0 0 0 0 12z",
    ],
  },
  text: {
    paths: [
      "M4 6V4h16v2",
      "M12 4v16",
      "M8 20h8",
    ],
  },
  camera: {
    paths: [
      "M3 8h4l2-3h6l2 3h4v11H3z",
      "M12 16a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7z",
      "M18 11h.01",
    ],
  },
  browser: {
    paths: [
      "M3 5h18v14H3z",
      "M3 9h18",
      "M6 7h.01",
      "M9 7h.01",
      "M6 13h7",
      "M6 16h10",
    ],
  },
  script: {
    paths: [
      "M4 4h16v16H4z",
      "M7 9l3 3-3 3",
      "M12 16h5",
    ],
  },
  start: {
    paths: [
      "M8 5l11 7-11 7z",
      "M4 8l2 1",
      "M3 12h3",
      "M4 16l2-1",
    ],
  },
  pause: {
    paths: [
      "M7 5h3v14H7z",
      "M14 5h3v14h-3z",
    ],
  },
  resume: {
    paths: [
      "M6 8V4",
      "M6 4h4",
      "M6.5 7a8 8 0 1 1-1.5 8",
      "M10 9l5 3-5 3z",
    ],
  },
  load: {
    paths: [
      "M12 3v11",
      "M8 10l4 4 4-4",
      "M5 17v3h14v-3",
    ],
  },
  reset: {
    paths: [
      "M5 8V3",
      "M5 3h5",
      "M5.5 7a8 8 0 1 1-1 8",
      "M12 8v4l3 2",
    ],
  },
  goto: {
    paths: [
      "M4 7h7a6 6 0 0 1 6 6v5",
      "M13 15l4 4 4-4",
      "M4 4v6",
    ],
  },
  arm: {
    paths: [
      "M7 11V8a5 5 0 0 1 9-3",
      "M6 11h12v10H6z",
      "M10 16l1.5 1.5L15 14",
    ],
  },
  disarm: {
    paths: [
      "M8 11V8a4 4 0 0 1 8 0v3",
      "M6 11h12v10H6z",
      "M10 15l4 4",
      "M14 15l-4 4",
    ],
  },
} satisfies Record<CueType, CueTypeIconSpec>;

export interface CueTypeIconProps {
  type: CueType;
  size?: number;
  tone?: "neutral" | "type" | "inherit";
  label?: string;
}

export function CueTypeIcon({
  type,
  size = 16,
  tone = "neutral",
  label,
}: CueTypeIconProps) {
  const color = tone === "type"
    ? CUE_TYPE_COLORS[type]
    : tone === "inherit"
      ? "currentColor"
      : "var(--wc-text-bright)";

  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={2.2}
      strokeLinecap="round"
      strokeLinejoin="round"
      color={color}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      focusable="false"
      data-cue-type={type}
      style={{ display: "block", flexShrink: 0 }}
    >
      {CUE_TYPE_ICON_SPECS[type].paths.map((path, index) => (
        <path key={index} d={path} />
      ))}
    </svg>
  );
}
