import type { CueSummary } from "./types";

/** True when an async clip-editor result still belongs to the latest request. */
export function isCurrentClipLoad(generation: number, currentGeneration: number): boolean {
  return generation === currentGeneration;
}

/** Find a cue's loading flag in the nested workspace summary tree. */
export function findCueIsLoading(cues: readonly CueSummary[], cueId: string): boolean | undefined {
  for (const cue of cues) {
    if (cue.id === cueId) return cue.is_loading;
    if (cue.children) {
      const nested = findCueIsLoading(cue.children, cueId);
      if (nested !== undefined) return nested;
    }
  }
  return undefined;
}

/** Retry media decoding only after the same cue finishes a loading phase. */
export function shouldRetryWaveform(
  previousCueId: string | null,
  previousLoading: boolean | undefined,
  cueId: string | null,
  currentLoading: boolean | undefined,
): boolean {
  return cueId !== null
    && previousCueId === cueId
    && previousLoading === true
    && currentLoading === false;
}

export interface PreviewStartTicket {
  requestSequence: number;
  cueId: string;
}

/** A pending start survives old-voice EOF, but not explicit cancellation or cue changes. */
export function isCurrentPreviewStart(
  ticket: PreviewStartTicket,
  currentRequestSequence: number,
  displayedCueId: string | null,
): boolean {
  return ticket.requestSequence === currentRequestSequence && ticket.cueId === displayedCueId;
}

export interface CueBoundPreviewUi {
  cueId: string;
  voiceId: string | null;
  positionMs: number | null;
  error: string | null;
  /** Generation learned from the authoritative preview-playhead event. */
  generation?: number | null;
}

/** Hide stale cue state and a terminal event for the exact preview generation. */
export function previewUiForCue<T extends CueBoundPreviewUi>(
  preview: T | null | undefined,
  cueId: string | null,
  activeEvent?: { cue_id: string; active: boolean; generation: number } | null,
  currentGeneration = -1,
): T | null {
  if (!cueId || preview?.cueId !== cueId) return null;
  const currentCueIsActive = activeEvent?.active === true && activeEvent.cue_id === cueId;
  const ended = preview.generation != null
    && currentGeneration === preview.generation
    && !currentCueIsActive;
  return ended && preview.voiceId ? { ...preview, voiceId: null } : preview;
}
