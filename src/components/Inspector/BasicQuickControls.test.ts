import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { OutputDestination } from "../../lib/types";
import { BasicQuickControls, MultiBasicQuickControls } from "./BasicQuickControls";
import { FitByOutputRows } from "./GeometryTab";
import { DEFAULT_GEOMETRY } from "../../lib/types";
import type { MultiCueRecord } from "./multiCueModel";
import { LOOP_INFINITE, loopQuickToggleAfterCount } from "./loopModel";

describe("Basics quick controls", () => {
  it("renders finite loops as not infinitely active and locks Fit while saving", () => {
    const output = { id: "screen-1", name: "Main" } as OutputDestination;
    const html = renderToStaticMarkup(createElement(BasicQuickControls, {
      cue: { id: "cue-1", cue_type: "video", continue_mode: "do_not_continue", loop_count: 3 },
      outputs: [output],
      outputIds: [output.id],
      disabled: true,
      onSave: vi.fn(),
    }));

    expect(html).toMatch(/aria-label="(?:Loop infinitely|Зациклить бесконечно)"/);
    expect(html).toContain('aria-pressed="false"');
    expect(html).toMatch(/aria-label="Main: (?:Fit|Вписать)"/);
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*aria-label="Main: (?:Fit|Вписать)"/);
  });

  it("toggles finite and infinite loop counts between infinity and off", () => {
    expect(loopQuickToggleAfterCount(3)).toBe(LOOP_INFINITE);
    expect(loopQuickToggleAfterCount(LOOP_INFINITE)).toBe(0);
  });

  it("keeps shared Fit buttons unpressed for mixed rows and preserves uniform single rows", () => {
    const html = renderToStaticMarkup(createElement(FitByOutputRows, {
      outputs: [{ id: "led", name: "LED" }, { id: "tv", name: "TV" }],
      geometryByOutput: { led: { ...DEFAULT_GEOMETRY, fit_mode: "fill" }, tv: DEFAULT_GEOMETRY },
      fallback: DEFAULT_GEOMETRY,
      mixedOutputIds: ["led"],
      mixedLabel: "Different values",
      onSelect: vi.fn(),
    }));
    const ledRow = html.slice(html.indexOf("LED"), html.indexOf("TV"));
    const tvRow = html.slice(html.indexOf("TV"));
    expect(ledRow).toContain("Different values");
    expect(ledRow).not.toContain('aria-pressed="true"');
    expect(tvRow).toContain('aria-pressed="true"');
  });

  it("renders mixed multi-cue actions and routed Fit rows through the shared controls", () => {
    const output = { id: "led", name: "LED" } as OutputDestination;
    const html = renderToStaticMarkup(createElement(MultiBasicQuickControls, {
      cues: [
        { id: "video", cue_type: "video", continue_mode: "do_not_continue", geometry: DEFAULT_GEOMETRY },
        { id: "image", cue_type: "image", continue_mode: "auto_follow", geometry: { ...DEFAULT_GEOMETRY, fit_mode: "fill" } },
      ] as unknown as MultiCueRecord[],
      outputs: [output],
      outputIdsByCue: new Map([["video", ["led"]], ["image", ["led"]]]),
      loopState: { kind: "empty" },
      muteState: { kind: "uniform", value: false },
      continueState: { kind: "mixed" },
      fitEnabled: true,
      mixedLabel: "Different values",
      onLoop: vi.fn(), onMute: vi.fn(), onContinue: vi.fn(), onFit: vi.fn(),
    }));

    expect(html).toContain('aria-pressed="mixed"');
    expect(html).toContain("Different values");
    expect(html).toContain('aria-label="LED: Вписать"');
    expect(html).not.toContain('aria-pressed="true"');
  });
});
