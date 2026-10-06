import { describe, expect, it } from "vitest";
import type { CueSummary, WorkspaceCueCatalogList } from "../../lib/types";
import { cueProgressPercent, flattenActiveCues, flattenCatalogActiveCues, getCatalogPauseTargets, migrateRightPanelMode, toggleRightPanel } from "./activeCueModel";

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

  it("counts active remote cues and active nested children from the global catalog", () => {
    const lists: WorkspaceCueCatalogList[] = [
      { id: "local", name: "Local", cues: [{ id: "local-run", cue_type: "audio", name: "Local", number: "1", state: "running", color: "none", duration_ms: 10, is_disabled: false }] },
      { id: "remote", name: "Remote", cues: [{ id: "parent", cue_type: "group", name: "Parent", number: "1", state: "standby", color: "none", duration_ms: null, is_disabled: false, children: [{ id: "remote-child", cue_type: "audio", name: "Child", number: "1.1", state: "running", color: "none", duration_ms: 20, is_disabled: false }] }] },
    ];
    expect(flattenCatalogActiveCues(lists).map(({ id, listName }) => [id, listName])).toEqual([
      ["local-run", "Local"], ["remote-child", "Remote"],
    ]);
  });

  it("enables global Pause and targets remote running cues when the active list is idle", () => {
    const lists: WorkspaceCueCatalogList[] = [
      { id: "local", name: "Local", cues: [{ id: "local-idle", cue_type: "audio", name: "Idle", number: "1", state: "standby", color: "none", duration_ms: 10, is_disabled: false }] },
      { id: "remote", name: "Remote", cues: [{ id: "remote-running", cue_type: "audio", name: "Running", number: "1", state: "running", color: "none", duration_ms: 10, is_disabled: false }] },
    ];
    expect(getCatalogPauseTargets(lists).map((cue) => cue.id)).toEqual(["remote-running"]);
  });

  it("does not invent progress for indefinite cues", () => {
    expect(cueProgressPercent(100, null)).toBeNull();
    expect(cueProgressPercent(500, 1000)).toBe(50);
    expect(cueProgressPercent(1500, 1000)).toBe(100);
  });
});
