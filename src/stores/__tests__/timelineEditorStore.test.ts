import { beforeEach, describe, expect, it } from "vitest";
import { useTimelineEditorStore } from "../timelineEditorStore";

describe("timelineEditorStore", () => {
  beforeEach(() => useTimelineEditorStore.setState({ locked: false }));

  it("toggles the shared edit lock", () => {
    useTimelineEditorStore.getState().toggle();
    expect(useTimelineEditorStore.getState().locked).toBe(true);
    useTimelineEditorStore.getState().toggle();
    expect(useTimelineEditorStore.getState().locked).toBe(false);
  });
});

