import { describe, expect, it } from "vitest";
import type { ContinueMode } from "../../lib/types";
import {
  buildContinueModeUpdates,
  resolveContinueModeTargets,
} from "./continueModeMenu";

describe("resolveContinueModeTargets", () => {
  it("keeps the complete multi-selection when the clicked cue is selected", () => {
    expect(resolveContinueModeTargets("cue-b", ["cue-a", "cue-b", "cue-c"]))
      .toEqual(["cue-a", "cue-b", "cue-c"]);
  });

  it("targets only an unselected clicked cue", () => {
    expect(resolveContinueModeTargets("cue-c", ["cue-a", "cue-b"]))
      .toEqual(["cue-c"]);
  });

  it("keeps a single selected clicked cue", () => {
    expect(resolveContinueModeTargets("cue-a", ["cue-a"]))
      .toEqual(["cue-a"]);
  });

  it("de-duplicates a selected set without changing its first-seen order", () => {
    expect(resolveContinueModeTargets("cue-b", ["cue-c", "cue-a", "cue-c", "cue-b", "cue-a"]))
      .toEqual(["cue-c", "cue-a", "cue-b"]);
  });
});

describe("buildContinueModeUpdates", () => {
  it.each<ContinueMode>([
    "do_not_continue",
    "auto_continue",
    "auto_follow",
  ])("builds the bulk-update payload for %s", (mode) => {
    expect(buildContinueModeUpdates(["cue-a", "cue-b"], mode)).toEqual([
      { cueId: "cue-a", properties: { continue_mode: mode } },
      { cueId: "cue-b", properties: { continue_mode: mode } },
    ]);
  });
});
