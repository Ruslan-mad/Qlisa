import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { DEFAULT_GEOMETRY } from "../../lib/types";
import { GeometryPad } from "./GeometryPad";

describe("GeometryPad", () => {
  it("keeps a portrait output frame at the correct aspect with no duplicate position sliders", () => {
    const markup = renderToStaticMarkup(createElement(GeometryPad, {
      geometry: DEFAULT_GEOMETRY,
      mode: "mpv",
      sourceAspect: 9 / 16,
      outputAspect: 9 / 16,
      estimatedAspect: false,
      contentLabel: "Portrait image",
      contextKey: "cue:main",
      onCommit: () => undefined,
    }));

    expect(markup).toContain("max-width:106.875px");
    expect(markup).toContain("aspect-ratio:0.5625 / 1");
    expect(markup.match(/type="range"/g) ?? []).toHaveLength(0);
    expect(markup).toContain("Portrait image");
  });

  it("clips a narrow source strip without changing the full-source layout box", () => {
    const geometry = { ...DEFAULT_GEOMETRY, crop_left: 0.96, crop_right: 0.02 };
    const markup = renderToStaticMarkup(createElement(GeometryPad, {
      geometry,
      mode: "mpv",
      sourceAspect: 16 / 9,
      outputAspect: 16 / 9,
      estimatedAspect: false,
      contentLabel: "Crop test",
      thumbnail: "fixture.png",
      contextKey: "cue:crop",
      onCommit: () => undefined,
    }));

    expect(markup).toContain("clip-path:inset(0% 2% 0% 96%)");
    expect(markup).toContain("background:transparent");
  });

  it("renders the full text content with its selected font and color", () => {
    const markup = renderToStaticMarkup(createElement(GeometryPad, {
      geometry: DEFAULT_GEOMETRY,
      mode: "output",
      sourceAspect: 16 / 9,
      outputAspect: 16 / 9,
      contentLabel: "First line\nSecond line",
      contextKey: "text:main",
      isOverlay: true,
      hideRotation: true,
      textFont: "Arial",
      textFontSize: 72,
      textColor: "#f0a0ff",
      onCommit: () => undefined,
    }));

    expect(markup).toContain("First line\nSecond line");
    expect(markup).toContain("font-family:Arial");
    expect(markup).toContain("color:#f0a0ff");
  });
});
