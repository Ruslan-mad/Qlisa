import { describe, expect, it } from "vitest";
import type { CueSummary, WorkspaceCueCatalogList } from "../../lib/types";
import { cueProgressPercent, flattenActiveCues, flattenCatalogActiveCues, getActiveCueDisplayRows, getCatalogPauseTargets, getSelectedPauseTargets, migrateRightPanelMode, toggleRightPanel } from "./activeCueModel";

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

  it("shows active containers without descendants but keeps children active under inactive parents", () => {
    const lists: WorkspaceCueCatalogList[] = [
      { id: "one", name: "One", cues: [
        { id: "running-group", cue_type: "group", name: "Running group", number: "1", state: "running", color: "none", duration_ms: null, is_disabled: false, children: [
          { id: "hidden-running", cue_type: "audio", name: "Nested audio", number: "1.1", state: "running", color: "none", duration_ms: 10, is_disabled: false },
          { id: "hidden-paused", cue_type: "audio", name: "Nested paused", number: "1.2", state: "paused", color: "none", duration_ms: 10, is_disabled: false },
        ] },
        { id: "paused-number", cue_type: "number", name: "Paused number", number: "2", state: "paused", color: "none", duration_ms: null, is_disabled: false, children: [
          { id: "hidden-number-child", cue_type: "audio", name: "Number audio", number: "2.1", state: "running", color: "none", duration_ms: 10, is_disabled: false },
        ] },
        { id: "inactive-group", cue_type: "group", name: "Inactive group", number: "3", state: "standby", color: "none", duration_ms: null, is_disabled: false, children: [
          { id: "independent-child", cue_type: "audio", name: "Independent child", number: "3.1", state: "running", color: "none", duration_ms: 10, is_disabled: false },
        ] },
        { id: "independent", cue_type: "audio", name: "Independent", number: "4", state: "running", color: "none", duration_ms: 10, is_disabled: false },
      ] },
      { id: "two", name: "Two", cues: [
        { id: "other-list", cue_type: "audio", name: "Other list", number: "1", state: "running", color: "none", duration_ms: 10, is_disabled: false },
      ] },
    ];

    expect(getActiveCueDisplayRows(lists).map(({ id, listName }) => [id, listName])).toEqual([
      ["running-group", "One"],
      ["paused-number", "One"],
      ["independent-child", "One"],
      ["independent", "One"],
      ["other-list", "Two"],
    ]);
    // Flattening remains complete for global controls, including descendants
    // hidden by the Active Cues presentation.
    expect(flattenCatalogActiveCues(lists).map(({ id }) => id)).toEqual([
      "running-group", "hidden-running", "hidden-paused", "paused-number",
      "hidden-number-child", "independent-child", "independent", "other-list",
    ]);
    expect(getCatalogPauseTargets(lists).map(({ id }) => id)).toEqual([
      "running-group", "hidden-running", "hidden-number-child",
      "independent-child", "independent", "other-list",
    ]);
  });

  it("enables global Pause and targets remote running cues when the active list is idle", () => {
    const lists: WorkspaceCueCatalogList[] = [
      { id: "local", name: "Local", cues: [{ id: "local-idle", cue_type: "audio", name: "Idle", number: "1", state: "standby", color: "none", duration_ms: 10, is_disabled: false }] },
      { id: "remote", name: "Remote", cues: [{ id: "remote-running", cue_type: "audio", name: "Running", number: "1", state: "running", color: "none", duration_ms: 10, is_disabled: false }] },
    ];
    expect(getCatalogPauseTargets(lists).map((cue) => cue.id)).toEqual(["remote-running"]);
  });

  it("pauses only selected running cues in the active list and leaves parallel cues alone", () => {
    const lists: WorkspaceCueCatalogList[] = [
      { id: "active", name: "Active", cues: [
        { id: "selected", cue_type: "audio", name: "Selected", number: "1", state: "running", color: "none", duration_ms: 10, is_disabled: false },
        { id: "parallel", cue_type: "audio", name: "Parallel", number: "2", state: "running", color: "none", duration_ms: 10, is_disabled: false },
      ] },
      { id: "other", name: "Other", cues: [
        { id: "remote", cue_type: "audio", name: "Remote", number: "1", state: "running", color: "none", duration_ms: 10, is_disabled: false },
      ] },
    ];
    expect(getSelectedPauseTargets(lists[0], ["selected"]).map(({ id }) => id)).toEqual(["selected"]);
    expect(getSelectedPauseTargets(lists[1], ["selected"])).toEqual([]);
    expect(getSelectedPauseTargets(lists[0], [])).toEqual([]);
  });

  it("uses selected Group and Number pause state without targeting their children twice", () => {
    const list: WorkspaceCueCatalogList = { id: "active", name: "Active", cues: [
      { id: "group", cue_type: "group", name: "Group", number: "1", state: "running", color: "none", duration_ms: null, is_disabled: false, children: [
        { id: "child", cue_type: "audio", name: "Child", number: "1.1", state: "running", color: "none", duration_ms: 10, is_disabled: false },
      ] },
      { id: "number", cue_type: "number", name: "Number", number: "2", state: "paused", color: "none", duration_ms: null, is_disabled: false, children: [
        { id: "number-child", cue_type: "audio", name: "Number child", number: "2.1", state: "paused", color: "none", duration_ms: 10, is_disabled: false },
      ] },
    ] };
    expect(getSelectedPauseTargets(list, ["group", "child"]).map(({ id }) => id)).toEqual(["group"]);
    // When no selected cue runs, resume selected paused containers; pauseCue/resumeCue
    // already apply the Group/Number behavior to their descendants.
    expect(getSelectedPauseTargets(list, ["number", "number-child"]).map(({ id }) => id)).toEqual(["number"]);
  });

  it("keeps independently running children eligible under inactive or paused selected containers", () => {
    const list: WorkspaceCueCatalogList = { id: "active", name: "Active", cues: [
      { id: "group", cue_type: "group", name: "Group", number: "1", state: "standby", color: "none", duration_ms: null, is_disabled: false, children: [
        { id: "child", cue_type: "audio", name: "Child", number: "1.1", state: "running", color: "none", duration_ms: 10, is_disabled: false },
      ] },
    ] };
    expect(getSelectedPauseTargets(list, ["group", "child"]).map(({ id }) => id)).toEqual(["child"]);
    list.cues[0].state = "paused";
    expect(getSelectedPauseTargets(list, ["group", "child"]).map(({ id }) => id)).toEqual(["child"]);
  });

  it("does not invent progress for indefinite cues", () => {
    expect(cueProgressPercent(100, null)).toBeNull();
    expect(cueProgressPercent(500, 1000)).toBe(50);
    expect(cueProgressPercent(1500, 1000)).toBe(100);
  });
});
