import { describe, expect, it } from "vitest";
import { canCommitSliceTimelineGesture, sliceTimelinePointerAction } from "./sliceTimelineInteraction";

describe("merged Preview timeline interactions", () => {
  it("routes trim/slice handles to edit and blank clicks only to preview seek", () => {
    expect(sliceTimelinePointerAction(false, true, true)).toBe("edit");
    expect(sliceTimelinePointerAction(false, false, true)).toBe("preview");
    expect(sliceTimelinePointerAction(false, false, false)).toBe("ignore");
    expect(sliceTimelinePointerAction(true, false, true)).toBe("ignore");
  });

  it("cancels an in-flight gesture when the displayed cue changes", () => {
    expect(canCommitSliceTimelineGesture("cue-a", "cue-a")).toBe(true);
    expect(canCommitSliceTimelineGesture("cue-a", "cue-b")).toBe(false);
  });
});
