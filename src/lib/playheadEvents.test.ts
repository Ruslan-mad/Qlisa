import { describe, expect, it } from "vitest";
import { shouldApplyPlayheadMovedEvent } from "./playheadEvents";

describe("playhead owner events", () => {
  it("ignores events owned by another cue list and accepts active or legacy events", () => {
    expect(shouldApplyPlayheadMovedEvent("remote-list", "active-list")).toBe(false);
    expect(shouldApplyPlayheadMovedEvent("active-list", "active-list")).toBe(true);
    expect(shouldApplyPlayheadMovedEvent(undefined, "active-list")).toBe(true);
  });
});
