import { describe, expect, it } from "vitest";
import {
  displayedOutputIds,
  outputIdsAfterToggle,
  selectedOutputIds,
  type OutputSelectableCue,
} from "./OutputSelector";

const cue = (overrides: Partial<OutputSelectableCue> = {}): OutputSelectableCue => ({
  id: "cue-1",
  cue_type: "video",
  ...overrides,
});

describe("OutputSelector routing state", () => {
  const named = new Set(["main", "projector"]);

  it("shows the effective default as selected for an implicit cue", () => {
    expect(displayedOutputIds(cue(), named, "main")).toEqual(["main"]);
    expect(displayedOutputIds(cue({ output_id: "projector" }), named, "main")).toEqual(["projector"]);
  });

  it("materializes the displayed default when an implicit cue adds another output", () => {
    expect(outputIdsAfterToggle(cue(), "projector", true, "main", named)).toEqual(["main", "projector"]);
  });

  it("adds Main to an explicit route without clearing the existing output", () => {
    expect(outputIdsAfterToggle(cue({ output_ids: ["projector"] }), "main", true, "main", named)).toEqual(["projector", "main"]);
  });

  it("removes Main from an explicit multi-output route without clearing the rest", () => {
    expect(outputIdsAfterToggle(cue({ output_ids: ["main", "projector"] }), "main", false, "main", named)).toEqual(["projector"]);
  });

  it("preserves explicit multi-output routes and missing IDs", () => {
    const multi = cue({ output_id: "main", output_ids: ["main", "projector"] });
    expect(selectedOutputIds(multi, named)).toEqual(["main", "projector"]);
    expect(outputIdsAfterToggle(multi, "projector", false, "main", named)).toEqual(["main"]);

    const missing = cue({ output_id: "offline", output_ids: ["offline"] });
    expect(displayedOutputIds(missing, named, "main")).toEqual(["offline"]);
    expect(outputIdsAfterToggle(missing, "offline", false, "main", named)).toEqual([]);
  });
});
