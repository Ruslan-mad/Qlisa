import { describe, expect, it } from "vitest";
import {
  masterVolumeDbToRatio,
  MASTER_VOLUME_MAX_DB,
  MASTER_VOLUME_MIN_DB,
  normalizeMasterVolumeInput,
  stepMasterVolume,
} from "./masterVolumeModel";

describe("master volume range", () => {
  it("keeps -60 and +6 endpoints and places unity before the right edge", () => {
    expect(masterVolumeDbToRatio(MASTER_VOLUME_MIN_DB)).toBe(0);
    expect(masterVolumeDbToRatio(0)).toBeCloseTo(60 / 66);
    expect(masterVolumeDbToRatio(MASTER_VOLUME_MAX_DB)).toBe(1);
  });

  it("snaps pointer input near unity but leaves keyboard values unchanged", () => {
    expect(normalizeMasterVolumeInput(-0.5, "pointer")).toBe(0);
    expect(normalizeMasterVolumeInput(0.5, "pointer")).toBe(0);
    expect(normalizeMasterVolumeInput(-1, "pointer")).toBe(-1);
    expect(normalizeMasterVolumeInput(0.5, "keyboard")).toBe(0.5);
    expect(normalizeMasterVolumeInput(-0.5, "keyboard")).toBe(-0.5);
  });

  it("steps by 0.5 dB through unity without snapping", () => {
    expect(stepMasterVolume(-0.5, 1)).toBe(0);
    expect(stepMasterVolume(0, -1)).toBe(-0.5);
    expect(stepMasterVolume(0, 1)).toBe(0.5);
    expect(stepMasterVolume(0.5, -1)).toBe(0);
  });
});
