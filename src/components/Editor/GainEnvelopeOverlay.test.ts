import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { GainEnvelopePoint } from "../../lib/types";
import { GainEnvelopeOverlay } from "./GainEnvelopeOverlay";
import type { AudioFadeOverlay } from "./audioFadeModel";

function render(
  points: GainEnvelopePoint[],
  options: { editable?: boolean; strokeWidth?: number; alwaysShowLabels?: boolean; fade?: AudioFadeOverlay; fadeEditable?: boolean } = {},
) {
  return renderToStaticMarkup(createElement(GainEnvelopeOverlay, {
    points,
    durationMs: 100,
    view: { startMs: 0, endMs: 100 },
    width: 400,
    height: 100,
    editable: options.editable ?? true,
    strokeWidth: options.strokeWidth,
    alwaysShowLabels: options.alwaysShowLabels,
    fade: options.fade,
    fadeEditable: options.fadeEditable,
    fadeContextKey: "cue-a",
  }));
}

describe("GainEnvelopeOverlay rendering", () => {
  it("renders a normal authored span", () => {
    const markup = render([
      { time_ms: 0, gain: 1 },
      { time_ms: 100, gain: 0.5 },
    ]);
    expect(markup).toContain("aria-label=\"Огибающая громкости\"");
    expect(markup).toContain("stroke=\"#ef4444\"");
  });

  it("renders both halves of a span crossing unity without persisting a new point", () => {
    const markup = render([
      { time_ms: 0, gain: 0.5 },
      { time_ms: 100, gain: 1.5 },
    ]);
    expect(markup.match(/stroke=\"#ef4444\"/g)).toHaveLength(2);
    expect(markup).toContain("x1=\"200\"");
    expect(markup).not.toContain("undefined");
  });

  it("renders a thin, labeled, pointer-transparent read-only curve", () => {
    const markup = render([
      { time_ms: 0, gain: 0.5 },
      { time_ms: 50, gain: 0.5 },
      { time_ms: 100, gain: 0.5 },
    ], { editable: false, strokeWidth: 2, alwaysShowLabels: true });
    const boostedMarkup = render([
      { time_ms: 0, gain: 1.5 },
      { time_ms: 100, gain: 1.5 },
    ], { editable: false, strokeWidth: 2, alwaysShowLabels: true });
    expect(markup.match(/stroke="#ef4444" stroke-width="2"/g)).toHaveLength(2);
    expect(markup).toContain("50% · -6.0 dB");
    expect(boostedMarkup).toContain("150% · +3.5 dB");
    expect(markup).toContain('pointer-events="none"');
    expect(markup).not.toContain("touch-action");
  });

  it("draws the composed fade curve and distinct editable boundary handles", () => {
    const markup = render([{ time_ms: 0, gain: 0.5 }, { time_ms: 100, gain: 0.5 }], {
      fade: {
        fadeInMs: 50, fadeInCurve: "linear", fadeOutMs: 25, fadeOutCurve: "s_curve",
        trimStartMs: 0, trimEndMs: 100, rate: 1, loopCount: 0,
      },
      fadeEditable: true,
    });
    expect(markup).toContain("stroke=\"transparent\" stroke-width=\"12\"");
    expect(markup).toContain("pointer-events=\"all\"");
    expect(markup).toContain("Вход 50 мс");
    expect(markup).toContain("Выход 25 мс");
    expect(markup).not.toContain("s_curve");
    expect(markup).toContain("#ef4444");
  });

  it("keeps fade duration readouts in a separate lane from gain labels", () => {
    const markup = render([{ time_ms: 0, gain: 0.75 }, { time_ms: 100, gain: 0.75 }], {
      alwaysShowLabels: true,
      fade: {
        fadeInMs: 2000, fadeInCurve: "linear", fadeOutMs: null, fadeOutCurve: null,
        trimStartMs: 0, trimEndMs: 100, rate: 1, loopCount: 0,
      },
    });
    expect(markup).toContain('aria-label="Фейд входа 2,0 с"');
    expect(markup).toContain('<rect x="2" y="82"');
    expect(markup).toContain('>Вход 2,0 с</text>');
    // The level label remains in the chart area, at least 18 px above the
    // bottom fade-duration lane.
    expect(markup).toContain('>Уровень 75% · -2.5 dB</text>');
    expect(markup).toContain('y="31" width="121.2" height="15"');
    expect(markup).toContain('x="200" y="42" text-anchor="middle"');
  });

  it("keeps fade rendering read-only while runtime edits are locked", () => {
    const markup = render([{ time_ms: 0, gain: 1 }, { time_ms: 100, gain: 1 }], {
      fade: {
        fadeInMs: 20, fadeInCurve: "linear", fadeOutMs: null, fadeOutCurve: null,
        trimStartMs: 0, trimEndMs: 100, loopCount: 0,
      },
      fadeEditable: false,
    });
    expect(markup).toContain("Остановите cue, чтобы менять фейд");
    expect(markup).toContain("pointer-events=\"none\"");
  });
});
