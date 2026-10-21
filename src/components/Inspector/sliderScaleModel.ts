export function clampSliderValue(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** Piecewise-linear position mapping with the neutral value at the track center. */
export function sliderValueToPosition(value: number, min: number, max: number, neutral: number): number {
  const bounded = clampSliderValue(value, min, max);
  return bounded <= neutral
    ? 0.5 * (bounded - min) / (neutral - min)
    : 0.5 + 0.5 * (bounded - neutral) / (max - neutral);
}

export function sliderPositionToValue(position: number, min: number, max: number, neutral: number, snapTolerance = 0): number {
  const bounded = Math.min(1, Math.max(0, position));
  const value = bounded <= 0.5
    ? min + (neutral - min) * bounded * 2
    : neutral + (max - neutral) * (bounded - 0.5) * 2;
  if (snapTolerance > 0 && Math.abs(value - neutral) <= snapTolerance) return neutral;
  return value;
}

export function stepSliderValue(value: number, direction: number, min: number, max: number, step: number): number {
  const count = Math.round((clampSliderValue(value, min, max) - min) / step) + Math.sign(direction);
  const precision = Math.max(0, (String(step).split(".")[1] ?? "").length);
  return Number(clampSliderValue(min + count * step, min, max).toFixed(precision));
}
