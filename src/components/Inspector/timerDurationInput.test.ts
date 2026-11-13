import { describe, expect, it } from "vitest";
import { formatTimerDurationInput, parseTimerDurationInput } from "./timerDurationInput";

describe("timer M:SS input", () => {
  it("parses minutes, seconds and optional milliseconds", () => {
    expect(parseTimerDurationInput("1:05")).toBe(65_000);
    expect(parseTimerDurationInput("0:01.5")).toBe(1_500);
    expect(parseTimerDurationInput("1440:00")).toBe(86_400_000);
  });

  it("formats without losing a saved millisecond fraction", () => {
    expect(formatTimerDurationInput(65_000)).toBe("1:05");
    expect(formatTimerDurationInput(65_005)).toBe("1:05.005");
  });

  it("rejects invalid seconds, fractions and values beyond 24 hours", () => {
    for (const input of ["", "1", "-1:00", "1:60", "1:2.1234", "1440:00.001", "999999999999999999999:00"]) {
      expect(parseTimerDurationInput(input), input).toBeNull();
    }
  });
});
