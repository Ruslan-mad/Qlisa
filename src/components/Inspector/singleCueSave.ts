type Translate = (
  key: string,
  vars?: Record<string, string | number | boolean | null | undefined>,
) => string;

export type CueRequestTicket = { cueId: string; generation: number };

/** A monotonic generation disambiguates A→B→A even when the cue ID matches again. */
export function isCurrentCueRequest(
  ticket: CueRequestTicket,
  currentGeneration: number,
  currentCueId: string | null,
): boolean {
  return ticket.generation === currentGeneration && ticket.cueId === currentCueId;
}

export async function persistThenCommit(
  persist: () => Promise<unknown>,
  commit: () => void,
  onFailure: (error: unknown) => void,
): Promise<boolean> {
  try {
    await persist();
  } catch (error) {
    onFailure(error);
    return false;
  }
  commit();
  return true;
}

/** Persist and run any local effects only while the submitted selection is current. */
export async function persistCurrentThenCommit(
  persist: () => Promise<unknown>,
  isCurrent: () => boolean,
  commit: () => void,
  onFailure: (error: unknown) => void,
): Promise<boolean> {
  try {
    await persist();
  } catch (error) {
    if (isCurrent()) onFailure(error);
    return false;
  }
  if (!isCurrent()) return false;
  commit();
  return true;
}

const FADE_PROPERTY_KEYS = new Set([
  "fade_in_ms", "fade_in_curve", "fade_out_ms", "fade_out_curve",
  "video_fade_in_ms", "video_fade_in_curve", "video_fade_out_ms", "video_fade_out_curve",
]);

export function isFadeEdit(properties: Record<string, unknown>): boolean {
  return Object.keys(properties).some((key) => FADE_PROPERTY_KEYS.has(key));
}

export function formatInspectorSaveError(
  error: unknown,
  fadeEdit: boolean,
  t: Translate,
): string {
  const detail = String(error);
  if (fadeEdit && /before changing fades/i.test(detail)) {
    return t("inspector.stopCueBeforeFadeEdit");
  }
  return t("inspector.saveError", { error: detail });
}
