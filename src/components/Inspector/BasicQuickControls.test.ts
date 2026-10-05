import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { OutputDestination } from "../../lib/types";
import { BasicQuickControls } from "./BasicQuickControls";
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
});
