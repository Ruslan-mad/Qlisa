import { describe, expect, it } from "vitest";
import { DEFAULT_UI_LAYOUT, normalizeUiLayout } from "./uiLayoutModel";

describe("normalizeUiLayout", () => {
  it("opens both workbench side panels by default", () => {
    expect(normalizeUiLayout(null)).toMatchObject({ activeCuesOpen: true, rightPanel: "inspector" });
    expect(DEFAULT_UI_LAYOUT.activeCuesOpen).toBe(true);
  });

  it.each([
    ["inspector", true, true],
    ["active-cues", false, true],
    ["closed", false, false],
  ] as const)("migrates legacy %s visibility", (rightPanel, inspectorOpen, activeCuesOpen) => {
    expect(normalizeUiLayout({ rightPanel })).toMatchObject({
      rightPanel: inspectorOpen ? "inspector" : "closed",
      activeCuesOpen,
    });
  });

  it("uses explicit independent visibility values on newer layouts", () => {
    expect(normalizeUiLayout({ rightPanel: "closed", inspectorOpen: true, activeCuesOpen: true }))
      .toMatchObject({ rightPanel: "inspector", activeCuesOpen: true });
  });

  it("migrates the old Timeline value once and preserves the version 2 Live tab", () => {
    expect(normalizeUiLayout({ activeClipTab: "Live" }).activeClipTab).toBe("Preview");
    expect(normalizeUiLayout({ activeClipTab: "Live", clipEditorTabVersion: 2 }).activeClipTab).toBe("Live");
  });
});
