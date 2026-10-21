import { describe, expect, it } from "vitest";
import { sliderPositionToValue, sliderValueToPosition, stepSliderValue } from "./sliderScaleModel";

const min = 0.05;
const max = 8;
const neutral = 1;

describe("centered geometry scale mapping", () => {
  it("keeps the endpoints and neutral point exact in both directions", () => {
    expect(sliderPositionToValue(0, min, max, neutral)).toBe(min);
    expect(sliderPositionToValue(0.5, min, max, neutral)).toBe(neutral);
    expect(sliderPositionToValue(1, min, max, neutral)).toBe(max);
    expect(sliderValueToPosition(min, min, max, neutral)).toBe(0);
    expect(sliderValueToPosition(neutral, min, max, neutral)).toBe(0.5);
    expect(sliderValueToPosition(max, min, max, neutral)).toBe(1);
  });

  it("is monotonic and snaps only values inside the actual-value tolerance", () => {
    const samples = Array.from({ length: 101 }, (_, index) => sliderPositionToValue(index / 100, min, max, neutral));
    expect(samples.every((value, index) => index === 0 || value >= samples[index - 1])).toBe(true);
    expect(sliderPositionToValue(sliderValueToPosition(0.95, min, max, neutral), min, max, neutral, 0.06)).toBe(neutral);
    expect(sliderPositionToValue(sliderValueToPosition(1.1, min, max, neutral), min, max, neutral, 0.06)).toBeCloseTo(1.1);
  });

  it("steps by actual values and stops at both endpoints", () => {
    expect(stepSliderValue(neutral, -1, min, max, 0.05)).toBe(0.95);
    expect(stepSliderValue(neutral, 1, min, max, 0.05)).toBe(1.05);
    expect(stepSliderValue(min, -1, min, max, 0.05)).toBe(min);
    expect(stepSliderValue(max, 1, min, max, 0.05)).toBe(max);
  });
});
