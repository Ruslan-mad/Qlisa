import { describe, expect, it } from "vitest";
import { cueOutputAssignments, orderOutputDestinations, orderOutputStatuses, outputFtbLedColor, outputFtbVisualState } from "./outputFtbModel";

describe("per-output FTB indicator state", () => {
  it("shows green only for an available healthy output", () => {
    expect(outputFtbVisualState({ available: true, healthy: true, ftb: false })).toBe("ok");
    expect(outputFtbLedColor({ available: true, healthy: true, ftb: false })).toBe("#4ade80");
  });

  it("shows red for an active FTB latch", () => {
    expect(outputFtbVisualState({ available: true, healthy: true, ftb: true })).toBe("ftb");
    expect(outputFtbLedColor({ available: true, healthy: true, ftb: true })).toBe("#ef4444");
  });

  it("shows amber for unavailable or unhealthy output", () => {
    expect(outputFtbVisualState({ available: false, healthy: false, ftb: false })).toBe("unavailable");
    expect(outputFtbVisualState({ available: true, healthy: false, ftb: true })).toBe("unavailable");
    expect(outputFtbLedColor({ available: false, healthy: false, ftb: false })).toBe("#f59e0b");
  });

  it("orders screens before live outputs using Settings order", () => {
    const statuses = ["live-2", "screen-2", "live-1", "screen-1"].map((output_id) => ({ output_id }));
    const destinations = [
      { id: "screen-1", sink_kind: "display" },
      { id: "live-1", sink_kind: "ndi" },
      { id: "screen-2", sink_kind: "display" },
      { id: "live-2", sink_kind: "srt" },
    ];
    expect(orderOutputStatuses(statuses, destinations).map((status) => status.output_id))
      .toEqual(["screen-1", "screen-2", "live-1", "live-2"]);
  });

  it("uses the same screen-then-live order for destination lists", () => {
    const destinations = [
      { id: "live-2", sink_kind: "srt" as const },
      { id: "screen-2", sink_kind: "display" as const },
      { id: "live-1", sink_kind: "ndi" as const },
      { id: "screen-1", sink_kind: "display" as const },
    ];
    expect(orderOutputDestinations(destinations).map((destination) => destination.id))
      .toEqual(["screen-2", "screen-1", "live-2", "live-1"]);
  });

  it("marks explicit visual routes and uses the default for an empty route", () => {
    const outputs = [{ output_id: "screen-1" }, { output_id: "screen-2" }, { output_id: "live" }];
    expect(cueOutputAssignments({ cue_type: "video", visual_output_ids: ["screen-1", "screen-2"] }, outputs, "live"))
      .toEqual([true, true, false]);
    expect(cueOutputAssignments({ cue_type: "image", visual_output_ids: [] }, outputs, "screen-2"))
      .toEqual([false, true, false]);
    expect(cueOutputAssignments({ cue_type: "audio", visual_output_ids: undefined }, outputs, "screen-1"))
      .toEqual([false, false, false]);
  });
});
