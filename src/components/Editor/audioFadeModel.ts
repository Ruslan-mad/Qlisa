import type { FadeCurve } from "../../lib/types";
import { sampleCurve } from "../../lib/curve";
import type { GainEnvelopePoint } from "../../lib/types";
import { envelopeGainAt } from "./gainEnvelopeModel";

export interface AudioFadeOverlay {
  fadeInMs: number | null;
  fadeInCurve: FadeCurve | null;
  fadeOutMs: number | null;
  fadeOutCurve: FadeCurve | null;
  trimStartMs: number;
  trimEndMs: number;
  rate?: number;
  loopCount?: number;
  sliced?: boolean;
}

export type FadeDirection = "in" | "out";
export const FADE_HANDLE_HIT_RADIUS = 7;

export interface FadeWindow {
  direction: FadeDirection;
  startMs: number;
  endMs: number;
  durationMs: number;
  curve: FadeCurve;
}

/** A fade duration is action time; the editor axis is source-file time. */
export function audioFadeWindows(fade: AudioFadeOverlay, fileDurationMs: number): FadeWindow[] {
  const fileEnd = Math.max(0, fileDurationMs);
  const start = Math.max(0, Math.min(fileEnd, fade.trimStartMs));
  const end = Math.max(start, Math.min(fileEnd, fade.trimEndMs));
  const rate = Number.isFinite(fade.rate) && (fade.rate ?? 0) > 0 ? fade.rate! : 1;
  const result: FadeWindow[] = [];

  if (!fade.sliced && fade.fadeInMs != null) {
    const duration = Math.max(0, fade.fadeInMs);
    const span = duration * rate;
    result.push({
      direction: "in",
      startMs: start,
      endMs: start + span,
      durationMs: duration,
      curve: fade.fadeInCurve ?? "s_curve",
    });
  }

  // The backend only starts the natural EOF fade for finite, non-sliced cues.
  if (!fade.sliced && fade.loopCount !== 0xffff_ffff && fade.fadeOutMs != null) {
    const duration = Math.max(0, fade.fadeOutMs);
    const finitePlays = Math.max(1, Math.floor(fade.loopCount ?? 0) + 1);
    const totalSpan = (end - start) * finitePlays;
    const span = Math.min(totalSpan, duration * rate);
    result.push({
      direction: "out",
      startMs: end - span,
      endMs: end,
      durationMs: duration,
      curve: fade.fadeOutCurve ?? "s_curve",
    });
  }
  return result;
}

export function fadeMultiplierAt(sourceMs: number, windows: FadeWindow[]): number {
  // The audio callback owns one FadeState. Its natural EOF fade replaces an
  // in-progress fade-in, so an overlap must show the outgoing curve alone.
  const activeOut = windows.find((fade) => fade.direction === "out"
    && fade.durationMs > 0 && sourceMs >= fade.startMs && sourceMs <= fade.endMs);
  const active = activeOut ?? windows.find((fade) => fade.direction === "in"
    && fade.durationMs > 0 && sourceMs >= fade.startMs && sourceMs <= fade.endMs);
  if (!active || active.endMs <= active.startMs) return 1;
  const progress = (sourceMs - active.startMs) / (active.endMs - active.startMs);
  const amount = sampleCurve({ kind: active.curve, intensity: 0, points: [], bends: [] }, progress);
  return Math.max(0, Math.min(1, active.direction === "in" ? amount : 1 - amount));
}

export function effectiveGainAt(
  points: GainEnvelopePoint[],
  sourceMs: number,
  windows: FadeWindow[],
): number {
  const gain = envelopeGainAt(points, sourceMs);
  if (!Number.isFinite(gain)) return 0;
  return Math.max(0, Math.min(2, gain * fadeMultiplierAt(sourceMs, windows)));
}

/** Keep explicit zero fades editable; null means no authored fade. */
export function clampFadeDurationFromHandle(
  direction: FadeDirection,
  boundaryMs: number,
  pointerMs: number,
  rate: number,
): number {
  const sourceSpan = direction === "in" ? pointerMs - boundaryMs : boundaryMs - pointerMs;
  const safeRate = Number.isFinite(rate) && rate > 0 ? rate : 1;
  return Math.max(0, Math.round(sourceSpan / safeRate));
}

export function isFadeHandleHit(x: number, y: number, handleX: number, handleY: number): boolean {
  return Math.hypot(x - handleX, y - handleY) <= FADE_HANDLE_HIT_RADIUS;
}

/** Return a value only for a completed, moved drag. Pointer-cancel never commits. */
export function fadeDragCommit(
  originalMs: number,
  nextMs: number,
  moved: boolean,
  cancelled = false,
): number | null {
  if (cancelled || !moved || Math.round(originalMs) === Math.round(nextMs)) return null;
  return Math.max(0, Math.round(nextMs));
}

export function fadeHandleSourceMs(fade: AudioFadeOverlay, direction: FadeDirection): number {
  return direction === "in" ? fade.trimStartMs : fade.trimEndMs;
}

export function formatAudioFadeDuration(durationMs: number, locale: "ru" | "en"): string {
  const safe = Math.max(0, Math.round(durationMs));
  if (safe < 1000) return `${safe}${locale === "ru" ? " мс" : " ms"}`;
  if (safe >= 60_000) {
    const totalSeconds = Math.floor(safe / 1000);
    return `${Math.floor(totalSeconds / 60)}:${String(totalSeconds % 60).padStart(2, "0")}`;
  }
  const seconds = (safe / 1000).toFixed(1);
  return `${locale === "ru" ? seconds.replace(".", ",") : seconds}${locale === "ru" ? " с" : " s"}`;
}

/** The sliced voice can jump between source ranges while one fade-in clock runs. */
export function fadeNotice(fade: AudioFadeOverlay, locale: "ru" | "en"): string | null {
  const hasFadeOut = fade.fadeOutMs != null;
  const notes: string[] = [];
  if (hasFadeOut && (fade.sliced || fade.loopCount === 0xffff_ffff)) {
    notes.push(locale === "ru" ? "Фейд выхода — при STOP; изменить в инспекторе" : "Fade out is on STOP; edit in Inspector");
  }
  if (fade.sliced && fade.fadeInMs != null) {
    notes.push(locale === "ru" ? "Срезы: кривая входа недоступна на шкале файла; изменить в инспекторе" : "Slices: fade-in is not shown on file time; edit in Inspector");
  }
  if ((fade.loopCount ?? 0) > 0 && fade.loopCount !== 0xffff_ffff && (hasFadeOut || fade.fadeInMs != null)) {
    notes.push(locale === "ru" ? "Вход — первый проход · выход — последний" : "Fade-in: first pass · fade-out: last pass");
  }
  return notes.length > 0 ? notes.join(" · ") : null;
}
