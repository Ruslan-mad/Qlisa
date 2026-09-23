import { describe, expect, it } from "vitest";
import { DEFAULT_DISPLAY_PREFS, DEFAULT_GENERAL_PREFS } from "../types";

describe("built-in general preferences", () => {
  it("enables auto-renumbering and uses the configured cue color defaults", () => {
    expect(DEFAULT_GENERAL_PREFS.auto_renumber_on_reorder).toBe(true);
    expect(DEFAULT_GENERAL_PREFS.default_cue_colors).toEqual({
      fade: "orange",
      group: "yellow",
      memo: "black",
      number: "yellow",
      pause: "red",
      resume: "green",
      start: "green",
      stop: "red",
    });
    expect(DEFAULT_GENERAL_PREFS.default_cue_colors.text).toBeUndefined();
    expect(DEFAULT_DISPLAY_PREFS.cue_color_style).toBe("full_row");
  });
});
