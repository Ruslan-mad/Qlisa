import { describe, expect, it } from "vitest";
import type { CueSummary } from "../../lib/types";
import { cueProgressPercent, flattenActiveCues, migrateRightPanelMode, toggleRightPanel } from "./activeCueModel";

const cue = (id: string, state: CueSummary["state"], children?: CueSummary["children"]): CueSummary => ({
  id, cue_type: "group", number: null, name: id, color: "none", state,
  duration_ms: null, notes: "", continue_mode: "do_not_continue", pre_wait_ms: 0, post_wait_ms: 0,
  file_path: null, file_size_bytes: null, media_width: null, media_height: null,
  media_file_missing: false, is_loading: false, is_disabled: false, is_broken: false,
  is_warning: false, children: children ?? [],
} as CueSummary);

describe("Active Cues model", () => {
  it("migrates the old Inspector flag and keeps one right panel open", () => {
    expect(migrateRightPanelMode(undefined, false)).toBe("closed");
    expect(migrateRightPanelMode(undefined, true)).toBe("inspector");
    expect(toggleRightPanel("inspector", "active-cues")).toBe("active-cues");
    expect(toggleRightPanel("active-cues", "active-cues")).toBe("closed");
  });

  it("flattens active descendants and omits inactive cues", () => {
    const tree = [cue("parent", "running", [cue("child", "paused"), cue("idle", "standby")])];
    expect(flattenActiveCues(tree).map((item) => item.id)).toEqual(["parent", "child"]);
  });

  it("does not invent progress for indefinite cues", () => {
    expect(cueProgressPercent(100, null)).toBeNull();
    expect(cueProgressPercent(500, 1000)).toBe(50);
    expect(cueProgressPercent(1500, 1000)).toBe(100);
  });
});
