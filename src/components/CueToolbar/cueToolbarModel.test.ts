import { describe, expect, it } from "vitest";
import { CUE_TOOLBAR_DESCRIPTORS, resolveCueToolbarLayout } from "./cueToolbarModel";

const widths = Object.fromEntries(CUE_TOOLBAR_DESCRIPTORS.map(({ type }) => [type, 90]));

describe("cue toolbar layout", () => {
  it("keeps the requested primary order and puts all specialist cues in More", () => {
    const layout = resolveCueToolbarLayout(2000, widths, 70);
    expect(layout.visible).toEqual(["audio", "video", "image", "start", "stop", "fade", "group", "number", "memo"]);
    expect(layout.overflow).toEqual(CUE_TOOLBAR_DESCRIPTORS.filter((item) => !item.primary).map((item) => item.type));
    expect(new Set([...layout.visible, ...layout.overflow]).size).toBe(CUE_TOOLBAR_DESCRIPTORS.length);
    expect(layout.showMore).toBe(true);
  });

  it("moves low priority controls first and restores them when width returns", () => {
    const narrow = resolveCueToolbarLayout(480, widths, 70);
    expect(narrow.visible).toEqual(["audio", "video", "image", "start"]);
    expect(narrow.overflow.slice(0, 5)).toEqual(["stop", "fade", "group", "number", "memo"]);
    const wide = resolveCueToolbarLayout(2000, widths, 70);
    expect(wide.visible).toContain("memo");
    expect(wide.visible).toContain("number");
  });

  it("preserves More and eventually overflows media types at extreme widths", () => {
    const tiny = resolveCueToolbarLayout(150, widths, 70);
    expect(tiny.visible).toEqual([]);
    expect(tiny.showMore).toBe(true);
    expect(tiny.overflow).toContain("audio");
    expect(tiny.overflow).toContain("image");
  });
});
