import { describe, expect, it } from "vitest";
import { formatDurationMs } from "./formatDuration";

describe("formatDurationMs", () => {
  it("formats whole minutes and seconds", () => {
    expect(formatDurationMs(157_000)).toBe("2:37");
    expect(formatDurationMs(61_000)).toBe("1:01");
    expect(formatDurationMs(0)).toBe("0:00");
  });

  it("keeps total minutes for durations longer than an hour", () => {
    expect(formatDurationMs(3_661_000)).toBe("61:01");
  });

  it("floors fractional seconds and clamps negative values to zero", () => {
    expect(formatDurationMs(1_999)).toBe("0:01");
    expect(formatDurationMs(-1_000)).toBe("0:00");
  });

  it("uses a dash when duration is unknown or invalid", () => {
    expect(formatDurationMs(null)).toBe("—");
    expect(formatDurationMs(undefined)).toBe("—");
    expect(formatDurationMs(Number.NaN)).toBe("—");
  });
});
