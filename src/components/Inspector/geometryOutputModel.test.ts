import { describe, expect, it } from "vitest";
import { DEFAULT_GEOMETRY } from "../../lib/types";
import { buildGeometryFitPatch, cancelGeometryAxisGesture, consumeExpectedGeometryEcho, cropDisplayToGeometry, cropGeometryToDisplay, fittedFootprint, formatGeometryPanPercent, geometryCropClipPath, geometryLinkTransition, geometryOverridesForCue, geometryPanToOutputOffset, geometrySnapshotsMatch, mergeGeometryOutputOverride, nextGeometryAxisKeyboardValue, nextGeometryAxisWheelValue, nextGeometryRotationWheelValue, nextGeometryScaleWheelValue, outputAspectRatio, outputOffsetToGeometryPan, quantizeGeometryPan } from "./geometryOutputModel";

describe("per-output geometry overrides", () => {
  it("keeps quick Fit linked when no output overrides exist", () => {
    const geometry = { ...DEFAULT_GEOMETRY, pan_x: 0.2 };
    expect(buildGeometryFitPatch(geometry, {}, "led", "fill")).toEqual({
      geometry: { ...geometry, fit_mode: "fill" },
      geometry_by_output: {},
    });
  });

  it("changes only the selected output when overrides already exist", () => {
    const geometry = { ...DEFAULT_GEOMETRY, pan_x: 0.2 };
    const other = { ...DEFAULT_GEOMETRY, pan_y: 0.4 };
    expect(buildGeometryFitPatch(geometry, { tv: other }, "led", "fill")).toEqual({
      geometry_by_output: { tv: other, led: { ...geometry, fit_mode: "fill" } },
    });
  });

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

  it("links to the active effective geometry in one transition and copies when unlinked", () => {
    const base = { ...DEFAULT_GEOMETRY, pan_x: 0.1 };
    const led = { ...DEFAULT_GEOMETRY, pan_x: -0.4, scale: 1.5 };
    const linked = geometryLinkTransition(false, true, base, { led }, ["main", "led"], "led");
    expect(linked).toEqual({ geometry: led, overrides: {} });

    const unlinked = geometryLinkTransition(true, false, linked.geometry, linked.overrides, ["main", "led"]);
    expect(unlinked.overrides).toEqual({ main: led, led });
  });

  it("maps mpv pan through full-source fitted size, current scale, and rotated fit", () => {
    const geometry = { ...DEFAULT_GEOMETRY, scale: 2, crop_left: 0.25, rotation: 90 };
    const footprint = fittedFootprint(geometry, 16 / 9, 16 / 9);
    const offset = geometryPanToOutputOffset({ ...geometry, pan_x: 0.25, pan_y: -0.2 }, "mpv", 16 / 9, 16 / 9);
    expect(footprint.width).toBeCloseTo((9 / 16) / (16 / 9));
    expect(footprint.height).toBeCloseTo(1);
    expect(offset.x).toBeCloseTo(0.25 * ((9 / 16) / (16 / 9)) * 2);
    expect(offset.y).toBeCloseTo(-0.4);
    const moved = outputOffsetToGeometryPan(0.15, -0.1, geometry, "mpv", 16 / 9, 16 / 9);
    expect(moved.pan_x).toBeCloseTo(0.15 / (((9 / 16) / (16 / 9)) * 2));
    expect(moved.pan_y).toBeCloseTo(-0.05);
  });

  it("keeps external camera and overlay pan in output coordinates", () => {
    const geometry = { ...DEFAULT_GEOMETRY, pan_x: 0.2, pan_y: -0.1, scale: 1.5, rotation: 90 };
    expect(geometryPanToOutputOffset(geometry, "output", 4 / 3, 16 / 9)).toEqual({ x: 0.2, y: -0.1 });
    const moved = outputOffsetToGeometryPan(0.1, 0.05, geometry, "output", 4 / 3, 16 / 9);
    expect(moved.pan_x).toBeCloseTo(0.3);
    expect(moved.pan_y).toBeCloseTo(-0.05);
  });

  it("uses the actual monitor aspect when available and marks the fallback estimated", () => {
    const display = { id: "main", sink_kind: "display", monitor: 1 } as const;
    expect(outputAspectRatio(display as never, [{ index: 1, width: 1920, height: 1200 }])).toEqual({ ratio: 1.6, estimated: false });
    expect(outputAspectRatio(undefined, [])).toEqual({ ratio: 16 / 9, estimated: true });
  });
});

describe("geometry axis gesture helpers", () => {
  it("keeps a persisted drag value when idle cleanup runs before the next wheel gesture", () => {
    let value = 0;
    const interactionStart = 0;
    value = 0.2; // pointer drag release and persisted prop echo

    const cleanup = cancelGeometryAxisGesture(false, interactionStart, value);
    if (cleanup.cancelled) value = cleanup.value;
    value = nextGeometryAxisWheelValue(value, -1);

    expect(value).toBe(0.21);
  });

  it("restores the start value when cancelling an active axis gesture", () => {
    expect(cancelGeometryAxisGesture(true, 0.205, 0.21)).toEqual({ cancelled: true, value: 0.205 });
    expect(nextGeometryAxisWheelValue(0.197, 0)).toBe(0.197);
  });

  it("uses the same raw-value quantization for keyboard and wheel from legacy fractions", () => {
    expect(nextGeometryAxisKeyboardValue(0.205, 1)).toBe(0.22);
    expect(nextGeometryAxisWheelValue(0.205, -1)).toBe(0.22);
    expect(nextGeometryAxisKeyboardValue(0.205, -1)).toBe(0.2);
    expect(nextGeometryAxisWheelValue(0.205, 1)).toBe(0.2);
  });

  it("shows pan as signed percentages without hiding legacy sub-percent precision", () => {
    expect(formatGeometryPanPercent(0)).toBe("0%");
    expect(formatGeometryPanPercent(0.205)).toBe("+20.5%");
    expect(formatGeometryPanPercent(-0.1)).toBe("-10%");
    expect(formatGeometryPanPercent(0.00004)).toBe("+<0.01%");
    expect(formatGeometryPanPercent(-0.00004)).toBe("-<0.01%");
    expect(quantizeGeometryPan(0.205)).toBe(0.21);
    expect(quantizeGeometryPan(-0.205)).toBe(-0.21);
    expect(nextGeometryAxisWheelValue(0.2, -1)).toBe(0.21);
  });

  it("uses one-point scale wheel steps and maps the full crop range to source fractions", () => {
    expect(nextGeometryScaleWheelValue(0.679, -1)).toBe(0.689);
    expect(nextGeometryScaleWheelValue(1, 1)).toBe(0.99);
    expect(nextGeometryScaleWheelValue(1, 0)).toBe(1);
    expect(cropDisplayToGeometry(100)).toBe(1);
    expect(cropDisplayToGeometry(25)).toBe(0.25);
    expect(cropGeometryToDisplay(0.25)).toBe(25);
    expect(cropGeometryToDisplay(0.96)).toBe(96);
  });

  it("keeps the fitted footprint fixed when crop edges change", () => {
    const full = fittedFootprint(DEFAULT_GEOMETRY, 16 / 9, 16 / 9);
    const clipped = fittedFootprint({ ...DEFAULT_GEOMETRY, crop_left: 0.96, crop_right: 0.04 }, 16 / 9, 16 / 9);
    expect(clipped).toEqual(full);
  });

  it("clips a narrow source strip and keeps crossed edges empty", () => {
    expect(geometryCropClipPath({ ...DEFAULT_GEOMETRY, crop_left: 0.96, crop_right: 0.01 })).toBe("inset(0% 1% 0% 96%)");
    expect(geometryCropClipPath({ ...DEFAULT_GEOMETRY, crop_left: 0.7, crop_right: 0.3 })).toBe("inset(50%)");
    expect(geometryCropClipPath(DEFAULT_GEOMETRY)).toBeUndefined();
  });

  it("lets one-degree rotation wheel steps leave a pointer cardinal snap", () => {
    expect(nextGeometryRotationWheelValue(90, 1)).toBe(89);
    expect(nextGeometryRotationWheelValue(90, -1)).toBe(91);
    expect(nextGeometryRotationWheelValue(360, -1)).toBe(360);
    expect(nextGeometryRotationWheelValue(0, 1)).toBe(0);
    expect(nextGeometryRotationWheelValue(90, 0)).toBe(90);
  });

  it("consumes an earlier own echo without treating it as an external geometry change", () => {
    const pending = ["saved-pan-1", "saved-pan-2", "saved-pan-3"];
    expect(consumeExpectedGeometryEcho(pending, "saved-pan-1")).toBe(true);
    expect(pending).toEqual(["saved-pan-2", "saved-pan-3"]);
    expect(consumeExpectedGeometryEcho(pending, "external-undo")).toBe(false);
    expect(pending).toEqual(["saved-pan-2", "saved-pan-3"]);
  });

  it("recognizes tiny floating-point differences in linked and per-output echoes", () => {
    const linkedOptimistic = JSON.stringify([[0.10236220472440945], []]);
    const linkedCanonical = JSON.stringify([[0.10236220472440943], []]);
    const outputOptimistic = JSON.stringify([[0.2], [["display-a", [0.12345678901234567]]]]);
    const outputCanonical = JSON.stringify([[0.2], [["display-a", [0.12345678901234568]]]]);

    for (const [optimistic, canonical] of [[linkedOptimistic, linkedCanonical], [outputOptimistic, outputCanonical]]) {
      const pending = [optimistic];
      // Inspector's optimistic setCueData echo consumes the expected snapshot.
      expect(consumeExpectedGeometryEcho(pending, optimistic)).toBe(true);
      expect(pending).toEqual([]);
      // The later getCue result must match the optimistic previous snapshot too.
      expect(geometrySnapshotsMatch(optimistic, canonical)).toBe(true);
    }
  });

  it("keeps a real external geometry change distinguishable from an own echo", () => {
    const pending = [JSON.stringify([[0.25], []])];
    const external = JSON.stringify([[0.250001], []]);
    expect(consumeExpectedGeometryEcho(pending, external)).toBe(false);
    expect(geometrySnapshotsMatch(pending[0], external)).toBe(false);
    expect(pending).toEqual([JSON.stringify([[0.25], []])]);
  });
});
