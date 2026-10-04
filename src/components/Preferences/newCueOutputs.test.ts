import { describe, expect, it } from "vitest";
import { DEFAULT_OUTPUT_DESTINATION } from "../../lib/types";
import { reconcileNewCueOutputIds, toggleNewCueOutputId } from "./newCueOutputs";

describe("new cue display defaults", () => {
  it("keeps independent selections and permits clearing every checkbox", () => {
    const both = toggleNewCueOutputId(toggleNewCueOutputId([], "led", true), "tv", true);
    expect(both).toEqual(["led", "tv"]);
    expect(toggleNewCueOutputId(both, "led", false)).toEqual(["tv"]);
    expect(toggleNewCueOutputId(["tv"], "tv", false)).toEqual([]);
  });

  it("drops deleted and non-display destinations while retaining configured displays", () => {
    const tv = { ...DEFAULT_OUTPUT_DESTINATION, id: "tv", name: "TV" };
    const ndi = { ...tv, id: "ndi", name: "NDI", sink_kind: "ndi" as const };
    expect(reconcileNewCueOutputIds(["tv", "deleted", "ndi", "tv"], [tv, ndi]))
      .toEqual(["tv"]);
  });
});
