import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { SliceList } from "../../lib/types";
import { SliceTimeline } from "./SliceTimeline";

vi.mock("../Inspector/TrimStrip", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../Inspector/TrimStrip")>();
  return { ...actual, useCanvasWidth: () => 600 };
});

describe("unified Preview SliceTimeline", () => {
  it("renders trim/slice controls together with preview, gain, and fade overlays", () => {
    const slices: SliceList = { markers: [500], play_counts: [1, 2] };
    const markup = renderToStaticMarkup(createElement(SliceTimeline, {
      durationMs: 1000,
      resetKey: "audio-a",
      startMs: 100,
      endMs: 900,
      slices,
      height: 124,
      paint: vi.fn(),
      paintKey: "waveform-a",
      onCommitStart: vi.fn(),
      onCommitEnd: vi.fn(),
      onSlicesChange: vi.fn(),
      previewPositionMs: 250,
      onPreviewPosition: vi.fn(),
      onPreviewPositionEnd: vi.fn(),
      gainEnvelope: [{ time_ms: 0, gain: 1 }, { time_ms: 1000, gain: 0.5 }],
      gainEnvelopeEditable: true,
      audioFade: {
        fadeInMs: 200,
        fadeInCurve: "linear",
        fadeOutMs: 100,
        fadeOutCurve: "s_curve",
        trimStartMs: 100,
        trimEndMs: 900,
        rate: 1,
        loopCount: 0,
      },
      audioFadeEditable: true,
    }));

    expect(markup).toContain('aria-label="Шкала предпросмотра"');
    expect(markup).toContain('aria-label="Огибающая громкости"');
    expect(markup).toContain("Вход 200 мс");
    expect(markup).toContain("Выход 100 мс");
    expect(markup).toContain("×1");
    expect(markup).toContain("×2");
    expect(markup).toContain("двойной клик: добавить срез");
  });
});
