import type { GainEnvelopePoint } from "../../lib/types";

export interface EnvelopeView {
  startMs: number;
  endMs: number;
  width: number;
  height: number;
}

export const GAIN_ENVELOPE_MAX_POINTS = 128;
export const GAIN_ENVELOPE_MAX_GAIN = 2;
export const ENVELOPE_VERTICAL_INSET = 12;

export type EnvelopeHit = { kind: "point"; index: number } | { kind: "segment"; index: number } | null;

/** Keep unity at 40% of the visible canvas and reserve the top 40% for boost. */
export function envelopeUnityY(height: number, inset = ENVELOPE_VERTICAL_INSET): number {
  const safeHeight = Math.max(1, height);
  return Math.max(inset, Math.min(safeHeight - inset, safeHeight * 0.4));
}

export function envelopeGainY(gain: number, height: number, inset = ENVELOPE_VERTICAL_INSET): number {
  const safeHeight = Math.max(1, height);
  const top = Math.min(inset, safeHeight / 2);
  const bottom = Math.max(top, safeHeight - inset);
  const unityY = Math.max(top, Math.min(bottom, safeHeight * 0.4));
  const boundedGain = Math.max(0, Math.min(GAIN_ENVELOPE_MAX_GAIN, gain));
  if (boundedGain > 1) {
    return unityY - ((boundedGain - 1) / (GAIN_ENVELOPE_MAX_GAIN - 1)) * (unityY - top);
  }
  return unityY + (1 - boundedGain) * (bottom - unityY);
}

export function envelopeGainFromY(y: number, height: number, inset = ENVELOPE_VERTICAL_INSET): number {
  const safeHeight = Math.max(1, height);
  const top = Math.min(inset, safeHeight / 2);
  const bottom = Math.max(top, safeHeight - inset);
  const unityY = Math.max(top, Math.min(bottom, safeHeight * 0.4));
  const boundedY = Math.max(top, Math.min(bottom, y));
  if (boundedY < unityY) {
    return 1 + ((unityY - boundedY) / Math.max(1, unityY - top)) * (GAIN_ENVELOPE_MAX_GAIN - 1);
  }
  return 1 - (boundedY - unityY) / Math.max(1, bottom - unityY);
}

/** Add a render-only unity crossing so the piecewise y scale remains true to linear-amplitude DSP. */
export function splitEnvelopeAtUnity(points: GainEnvelopePoint[]): GainEnvelopePoint[] {
  const result: GainEnvelopePoint[] = [];
  for (let index = 0; index < points.length; index++) {
    const point = points[index];
    result.push(point);
    const next = points[index + 1];
    if (!next || (point.gain - 1) * (next.gain - 1) >= 0) continue;
    const mix = (1 - point.gain) / (next.gain - point.gain);
    result.push({
      time_ms: point.time_ms + (next.time_ms - point.time_ms) * mix,
      gain: 1,
    });
  }
  return result;
}

export function displayedEnvelopePoints(points: GainEnvelopePoint[], durationMs: number): GainEnvelopePoint[] {
  const end = Math.max(1, Math.round(durationMs));
  if (points.length === 0) return [{ time_ms: 0, gain: 1 }, { time_ms: end, gain: 1 }];
  const sorted = [...points].sort((a, b) => a.time_ms - b.time_ms);
  const result = [...sorted];
  if (result[0].time_ms > 0) result.unshift({ time_ms: 0, gain: result[0].gain });
  if (result[result.length - 1].time_ms < end) result.push({ time_ms: end, gain: result[result.length - 1].gain });
  return result;
}

export function envelopeGainAt(points: GainEnvelopePoint[], timeMs: number): number {
  if (points.length === 0) return 1;
  if (points.length === 1 || timeMs <= points[0].time_ms) return points[0].gain;
  const last = points[points.length - 1];
  if (timeMs >= last.time_ms) return last.gain;
  let low = 1;
  let high = points.length - 1;
  while (low < high) {
    const mid = low + Math.floor((high - low) / 2);
    if (points[mid].time_ms < timeMs) low = mid + 1;
    else high = mid;
  }
  const a = points[low - 1];
  const b = points[low];
  const t = (timeMs - a.time_ms) / Math.max(1, b.time_ms - a.time_ms);
  return a.gain + (b.gain - a.gain) * t;
}

export function envelopePercent(gain: number): number {
  return Math.round(Math.max(0, Math.min(GAIN_ENVELOPE_MAX_GAIN, gain)) * 100);
}

export function envelopeDb(gain: number): string {
  if (gain <= 0) return "−∞ dB";
  const db = 20 * Math.log10(gain);
  return `${db > 0 ? "+" : ""}${db.toFixed(1)} dB`;
}

export function hitTestEnvelope(
  points: GainEnvelopePoint[],
  x: number,
  y: number,
  view: EnvelopeView,
  pointRadius = 11,
  lineRadius = 7,
): EnvelopeHit {
  if (view.width <= 0 || view.height <= 0 || view.endMs <= view.startMs) return null;
  const visible: Array<{ point: GainEnvelopePoint; sourceIndex: number | null }> = [];
  const end = Math.max(1, Math.round(view.endMs));
  if (points.length === 0) {
    visible.push({ point: { time_ms: 0, gain: 1 }, sourceIndex: null });
    visible.push({ point: { time_ms: end, gain: 1 }, sourceIndex: null });
  } else {
    if (points[0].time_ms > 0) visible.push({ point: { time_ms: 0, gain: points[0].gain }, sourceIndex: null });
    points.forEach((point, sourceIndex) => visible.push({ point, sourceIndex }));
    if (points[points.length - 1].time_ms < end) visible.push({ point: { time_ms: end, gain: points[points.length - 1].gain }, sourceIndex: null });
  }
  const splitVisible = visible.length < 2 ? visible : [visible[0]];
  for (let index = 0; index < visible.length - 1; index++) {
    const a = visible[index].point;
    const b = visible[index + 1].point;
    if ((a.gain - 1) * (b.gain - 1) < 0) {
      const mix = (1 - a.gain) / (b.gain - a.gain);
      splitVisible.push({
        point: { time_ms: a.time_ms + (b.time_ms - a.time_ms) * mix, gain: 1 },
        sourceIndex: null,
      });
    }
    splitVisible.push(visible[index + 1]);
  }
  const toX = (ms: number) => ((ms - view.startMs) / (view.endMs - view.startMs)) * view.width;
  const toY = (gain: number) => envelopeGainY(gain, view.height);
  let nearest: EnvelopeHit = null;
  let nearestDistance = Number.POSITIVE_INFINITY;
  points.forEach((point, index) => {
    const px = toX(point.time_ms);
    const py = toY(point.gain);
    const distance = Math.hypot(x - px, y - py);
    if (distance <= pointRadius && distance < nearestDistance) {
      nearest = { kind: "point", index };
      nearestDistance = distance;
    }
  });
  if (nearest) return nearest;
  for (let index = 0; index < splitVisible.length - 1; index++) {
    const a = splitVisible[index].point;
    const b = splitVisible[index + 1].point;
    const ax = toX(a.time_ms), ay = toY(a.gain);
    const bx = toX(b.time_ms), by = toY(b.gain);
    const dx = bx - ax, dy = by - ay;
    const lengthSquared = dx * dx + dy * dy;
    const projection = lengthSquared === 0 ? 0 : Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / lengthSquared));
    const distance = Math.hypot(x - (ax + projection * dx), y - (ay + projection * dy));
    if (distance <= lineRadius && distance < nearestDistance) {
      const left = splitVisible[index].sourceIndex;
      const right = splitVisible[index + 1].sourceIndex;
      // Only a span bounded by two adjacent authored points can move as a
      // segment. A span containing an implicit anchor is click-to-add only.
      const crossesUnity = (a.gain - 1) * (b.gain - 1) === 0
        && a.gain !== b.gain
        && ((left != null && left < points.length - 1 && (points[left].gain - 1) * (points[left + 1].gain - 1) < 0)
          || (right != null && right > 0 && (points[right - 1].gain - 1) * (points[right].gain - 1) < 0));
      const segmentIndex = left != null && right === left + 1
        ? left
        : crossesUnity && left != null ? left
          : crossesUnity && right != null ? right - 1
            : -1;
      nearest = { kind: "segment", index: segmentIndex };
      nearestDistance = distance;
    }
  }
  return nearest;
}

export function addEnvelopePoint(
  points: GainEnvelopePoint[],
  timeMs: number,
  gain: number,
  durationMs: number,
): GainEnvelopePoint[] {
  const end = Math.max(2, Math.round(durationMs));
  if (points.length >= GAIN_ENVELOPE_MAX_POINTS) return points;
  const base = points.length === 0 ? [{ time_ms: 0, gain: 1 }, { time_ms: end, gain: 1 }] : [...points];
  const sorted = [...base].sort((a, b) => a.time_ms - b.time_ms);
  let index = sorted.findIndex((point) => point.time_ms > timeMs);
  if (index < 0) index = sorted.length;
  const lower = sorted[index - 1]?.time_ms ?? -1;
  const upper = sorted[index]?.time_ms ?? end + 1;
  const at = Math.max(lower + 1, Math.min(upper - 1, Math.round(timeMs)));
  if (at <= lower || at >= upper) return sorted;
  sorted.splice(index, 0, { time_ms: at, gain: Math.max(0, Math.min(GAIN_ENVELOPE_MAX_GAIN, gain)) });
  return sorted;
}

export function moveEnvelopePoint(
  points: GainEnvelopePoint[],
  index: number,
  timeMs: number,
  gain: number,
  durationMs: number,
  minGapMs = 1,
): GainEnvelopePoint[] {
  if (index < 0 || index >= points.length) return points;
  const next = [...points];
  const first = index === 0;
  const last = index === points.length - 1;
  const low = first ? 0 : next[index - 1].time_ms + minGapMs;
  const high = last ? durationMs : next[index + 1].time_ms - minGapMs;
  next[index] = {
    time_ms: first ? 0 : last ? Math.max(0, Math.round(durationMs)) : Math.max(low, Math.min(high, Math.round(timeMs))),
    gain: Math.max(0, Math.min(GAIN_ENVELOPE_MAX_GAIN, gain)),
  };
  return next;
}

/** Move only the selected segment's two authored endpoints; outer neighbors stay put. */
export function moveEnvelopeSegment(
  points: GainEnvelopePoint[],
  segmentIndex: number,
  gainDelta: number,
): GainEnvelopePoint[] {
  if (segmentIndex < 0 || segmentIndex + 1 >= points.length) return points;
  const next = [...points];
  for (const index of [segmentIndex, segmentIndex + 1]) {
    next[index] = { ...next[index], gain: Math.max(0, Math.min(GAIN_ENVELOPE_MAX_GAIN, next[index].gain + gainDelta)) };
  }
  return next;
}

export function resetEnvelopePointGain(points: GainEnvelopePoint[], index: number): GainEnvelopePoint[] {
  if (index < 0 || index >= points.length || points[index].gain === 1) return points;
  const next = [...points];
  next[index] = { ...next[index], gain: 1 };
  return next;
}

export function deleteEnvelopePoint(points: GainEnvelopePoint[], index: number): GainEnvelopePoint[] {
  if (index <= 0 || index >= points.length - 1) return points;
  return points.filter((_, current) => current !== index);
}
