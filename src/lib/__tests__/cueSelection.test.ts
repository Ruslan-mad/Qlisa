import { describe, expect, it } from "vitest";
import { cueRangeFromVisibleOrder, cueSelectionFromSweep, flattenVisibleCueTree, isMultiSelectModifier, toggleCueSelection } from "../cueSelection";

describe("cue selection helpers", () => {
  it("moves primary focus to the nearest remaining cue when toggled off", () => {
    expect(toggleCueSelection(["a", "b", "c"], "b", "b", ["a", "b", "c"]))
      .toEqual({ selectedCueId: "a", selectedCueIds: ["a", "c"] });
  });

  it("clears primary and IDs when the last selected cue is toggled off", () => {
    expect(toggleCueSelection(["only"], "only", "only", ["only"]))
      .toEqual({ selectedCueId: null, selectedCueIds: [] });
  });

  it("uses the expanded nested tree order for inclusive shift ranges", () => {
    type Node = { id: string; cue_type: string; children?: Node[] };
    const cues: Node[] = [
      { id: "before", cue_type: "audio" },
      { id: "group", cue_type: "group", children: [
        { id: "first", cue_type: "audio" },
        { id: "nested", cue_type: "group", children: [{ id: "inner-leaf", cue_type: "audio" }] },
        { id: "last", cue_type: "audio" },
      ] },
      { id: "after", cue_type: "audio" },
    ];
    const visible = flattenVisibleCueTree(cues, new Set(["group", "nested"])).map(({ cue }) => cue.id);

    expect(visible).toEqual(["before", "group", "first", "nested", "inner-leaf", "last", "after"]);
    expect(cueRangeFromVisibleOrder("first", "last", visible)).toEqual(["first", "nested", "inner-leaf", "last"]);
  });

  it("flattens expanded Number children like expanded Group children", () => {
    const cues = [{ id: "number", cue_type: "number", children: [{ id: "child", cue_type: "audio" }] }];
    expect(flattenVisibleCueTree(cues, new Set(["number"])).map(({ cue }) => cue.id))
      .toEqual(["number", "child"]);
  });

  it("treats Ctrl and Command as equivalent multi-select modifiers", () => {
    expect(isMultiSelectModifier(true, false)).toBe(true);
    expect(isMultiSelectModifier(false, true)).toBe(true);
    expect(isMultiSelectModifier(false, false)).toBe(false);
  });

  it("supports desktop click, Ctrl-toggle, and Shift-range selection semantics", () => {
    const order = ["a", "b", "c", "d"];
    expect(toggleCueSelection([], null, "b", order))
      .toEqual({ selectedCueId: "b", selectedCueIds: ["b"] });
    expect(toggleCueSelection(["b"], "b", "d", order))
      .toEqual({ selectedCueId: "d", selectedCueIds: ["b", "d"] });
    expect(cueRangeFromVisibleOrder("b", "d", order)).toEqual(["b", "c", "d"]);
  });

  it("selects an inclusive range while sweeping the mouse from an anchor row", () => {
    const order = ["a", "b", "c", "d", "e"];
    expect(cueSelectionFromSweep("b", "d", order))
      .toEqual({ selectedCueId: "d", selectedCueIds: ["b", "c", "d"] });
    expect(cueSelectionFromSweep("d", "b", order, ["a"], true))
      .toEqual({ selectedCueId: "b", selectedCueIds: ["a", "b", "c", "d"] });
  });
});
