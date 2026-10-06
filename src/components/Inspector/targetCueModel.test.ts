import { describe, expect, it } from "vitest";
import type { WorkspaceCueCatalogEntry, WorkspaceCueCatalogList } from "../../lib/types";
import { flattenCueCatalog, singleTargetSelection } from "./targetCueModel";

const cue = (id: string, children?: WorkspaceCueCatalogEntry[]): WorkspaceCueCatalogEntry => ({
  id, cue_type: children ? "group" : "audio", name: id, number: "1", state: "standby", color: "none", duration_ms: null, is_disabled: false, children,
});

describe("cross-list target catalog", () => {
  const lists: WorkspaceCueCatalogList[] = [
    { id: "a", name: "Act I", cues: [cue("a-group", [cue("nested")])] },
    { id: "b", name: "Act II", cues: [cue("b-target")] },
  ];

  it("keeps same-number cues distinct and exposes nested targets under their owner list", () => {
    const flat = flattenCueCatalog(lists);
    expect(flat.map(({ id, listId }) => [id, listId])).toEqual([
      ["a-group", "a"], ["nested", "a"], ["b-target", "b"],
    ]);
    expect(flat.filter((item) => item.number === "1")).toHaveLength(3);
  });

  it("keeps only the latest Goto choice across tabs", () => {
    expect(singleTargetSelection(["a-group", "b-target"])).toEqual(["b-target"]);
    expect(singleTargetSelection([])).toEqual([]);
  });
});
