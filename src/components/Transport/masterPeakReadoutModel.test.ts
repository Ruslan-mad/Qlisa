import { describe, expect, it } from "vitest";
import {
  EMPTY_MASTER_PEAK_READOUT,
  formatMasterPeakDbfs,
  observeMasterPeaks,
  resetMasterPeakChannel,
} from "./masterPeakReadoutModel";

describe("master peak readout", () => {
  it("starts at negative infinity and keeps over-full-scale sample peaks", () => {
    expect(formatMasterPeakDbfs(EMPTY_MASTER_PEAK_READOUT.left)).toBe("−∞ dBFS");
    const observed = observeMasterPeaks(EMPTY_MASTER_PEAK_READOUT, 1.5, 0.5);
    expect(observed.left).toBe(1.5);
    expect(observed.overloadLeft).toBe(true);
    expect(observed.overloadRight).toBe(false);
    expect(formatMasterPeakDbfs(observed.left)).toBe("+3.5 dBFS");
  });

  it("latches overload, clears one channel, and rearms on a repeated equal sample", () => {
    const first = observeMasterPeaks(EMPTY_MASTER_PEAK_READOUT, 1, 0.7);
    const cleared = resetMasterPeakChannel(first, "left");
    expect(cleared.overloadLeft).toBe(false);
    expect(cleared.right).toBe(0.7);

    const repeated = observeMasterPeaks(cleared, 1, 0.7);
    expect(repeated.left).toBe(1);
    expect(repeated.overloadLeft).toBe(true);
    expect(repeated.right).toBe(0.7);
    expect(repeated.overloadRight).toBe(false);
  });

  it("keeps peak and overload latched through silence", () => {
    const clipped = observeMasterPeaks(EMPTY_MASTER_PEAK_READOUT, 1.2, 0.4);
    const silent = observeMasterPeaks(clipped, 0, 0);

    expect(silent.left).toBe(1.2);
    expect(silent.overloadLeft).toBe(true);
    expect(silent.right).toBe(0.4);
    expect(silent.overloadRight).toBe(false);
  });
});
