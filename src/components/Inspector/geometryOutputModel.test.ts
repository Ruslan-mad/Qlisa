import { describe, expect, it } from "vitest";
import { DEFAULT_GEOMETRY } from "../../lib/types";
import { geometryOverridesForCue, mergeGeometryOutputOverride } from "./geometryOutputModel";

describe("per-output geometry overrides", () => {
  it("keeps rapid edits to different outputs and uses legacy geometry as fallback", () => {
    const legacy = { ...DEFAULT_GEOMETRY, scale: 1.5 };
    const ledEdit = mergeGeometryOutputOverride({}, "led", legacy, { fit_mode: "fill" });
    const tvEdit = mergeGeometryOutputOverride(ledEdit, "tv", legacy, { crop_left: 0.2 });

    expect(tvEdit.led.fit_mode).toBe("fill");
    expect(tvEdit.led.scale).toBe(1.5);
    expect(tvEdit.tv.crop_left).toBe(0.2);
    expect(tvEdit.tv.scale).toBe(1.5);
  });

  it("does not show one cue's pending overrides when selection changes", () => {
    const led = { ...DEFAULT_GEOMETRY, fit_mode: "fill" as const };
    const pending = { cueId: "cue-a", values: { led } };
    const cueB = { tv: { ...DEFAULT_GEOMETRY, pan_x: 0.4 } };

    expect(geometryOverridesForCue(pending, "cue-b", cueB)).toBe(cueB);
    expect(geometryOverridesForCue(pending, "cue-a", undefined)).toBe(pending.values);
  });
});
