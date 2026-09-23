import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { CueType } from "../../lib/types";
import { CUE_TYPE_COLORS } from "../../lib/types";
import { CUE_TYPE_ICON_SPECS, CueTypeIcon } from "./CueTypeIcon";

const ALL_CUE_TYPES = [
  "audio", "memo", "wait", "group", "number", "fade", "stop", "devamp", "video",
  "image", "osc", "midi", "midi_file", "light", "mic", "timecode", "text",
  "camera", "browser", "script", "start", "pause", "resume", "load", "reset", "goto",
  "arm", "disarm",
] as const satisfies readonly CueType[];

describe("CueTypeIcon", () => {
  it("has an explicit SVG specification for every CueType", () => {
    expect(Object.keys(CUE_TYPE_ICON_SPECS).sort()).toEqual([...ALL_CUE_TYPES].sort());
    expect(ALL_CUE_TYPES).toHaveLength(28);

    for (const type of ALL_CUE_TYPES) {
      expect(CUE_TYPE_ICON_SPECS[type].paths.length, `${type} should have a shape`).toBeGreaterThan(0);
      for (const path of CUE_TYPE_ICON_SPECS[type].paths) {
        expect(path.trim(), `${type} should not contain an empty path`).not.toBe("");
      }
    }
  });

  it("gives every cue type a distinct shape", () => {
    const signatures = ALL_CUE_TYPES.map((type) => CUE_TYPE_ICON_SPECS[type].paths.join("|"));
    expect(new Set(signatures).size).toBe(ALL_CUE_TYPES.length);
  });

  it.each(ALL_CUE_TYPES)("renders %s as path-only inline SVG", (type) => {
    const html = renderToStaticMarkup(createElement(CueTypeIcon, { type }));

    expect(html).toContain("<svg");
    expect(html).toContain('viewBox="0 0 24 24"');
    expect(html).toContain(`data-cue-type="${type}"`);
    expect(html).toContain("<path");
    expect(html).not.toContain("<text");
    expect(html.replace(/<[^>]+>/g, "")).toBe("");
  });

  it("is hidden from assistive technology without a label", () => {
    const html = renderToStaticMarkup(createElement(CueTypeIcon, { type: "audio" }));

    expect(html).toContain('aria-hidden="true"');
    expect(html).toContain('focusable="false"');
    expect(html).not.toContain('role="img"');
    expect(html).not.toContain("aria-label");
  });

  it("becomes a labelled image when a label is supplied", () => {
    const html = renderToStaticMarkup(createElement(CueTypeIcon, {
      type: "video",
      label: "Video cue",
    }));

    expect(html).toContain('role="img"');
    expect(html).toContain('aria-label="Video cue"');
    expect(html).not.toContain("aria-hidden");
    expect(html).toContain('focusable="false"');
  });

  it("supports neutral, cue-type, and inherited color tones", () => {
    const neutral = renderToStaticMarkup(createElement(CueTypeIcon, { type: "audio" }));
    const typed = renderToStaticMarkup(createElement(CueTypeIcon, { type: "audio", tone: "type" }));
    const inherited = renderToStaticMarkup(createElement(CueTypeIcon, { type: "audio", tone: "inherit" }));

    expect(neutral).toContain('color="var(--wc-text-bright)"');
    expect(typed).toContain(`color="${CUE_TYPE_COLORS.audio}"`);
    expect(inherited).toContain('color="currentColor"');
  });

  it("uses 16px by default and honours an explicit size", () => {
    const normal = renderToStaticMarkup(createElement(CueTypeIcon, { type: "group" }));
    const large = renderToStaticMarkup(createElement(CueTypeIcon, { type: "group", size: 22 }));

    expect(normal).toContain('width="16"');
    expect(normal).toContain('height="16"');
    expect(large).toContain('width="22"');
    expect(large).toContain('height="22"');
  });
});
