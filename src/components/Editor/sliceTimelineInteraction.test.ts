import { describe, expect, it } from "vitest";
import { beginCanvasPointer, cancelCanvasPointer, canCommitSliceTimelineGesture, endCanvasPointer, ownsCanvasPointer, sliceTimelinePointerAction } from "./sliceTimelineInteraction";

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

  it("keeps a captured drag active outside the canvas and commits only on its pointerup", () => {
    const active = { current: null as number | null };
    beginCanvasPointer(active, 7);
    // Leaving the canvas does not alter the captured pointer session.
    expect(ownsCanvasPointer(active, 7)).toBe(true);
    expect(endCanvasPointer(active, 7)).toBe(true);
    expect(endCanvasPointer(active, 7)).toBe(false);
  });

  it("cancels a pointer session without allowing a later release to commit", () => {
    const active = { current: null as number | null };
    beginCanvasPointer(active, 4);
    expect(cancelCanvasPointer(active)).toBe(true);
    expect(endCanvasPointer(active, 4)).toBe(false);
    expect(cancelCanvasPointer(active)).toBe(false);
  });
});
