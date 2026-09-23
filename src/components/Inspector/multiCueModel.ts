import type { BulkEditMetadata, CueColor, CueSummary, CueType, FadeCurve } from "../../lib/types";

export type MultiCueRecord = CueSummary & Record<string, unknown> & {
  _bulk_edit?: BulkEditMetadata;
};

export interface MultiCueDraft {
  color?: CueColor;
  notes?: string;
  continue_mode?: CueSummary["continue_mode"];
  is_disabled?: boolean;
  pre_wait_ms?: number;
  post_wait_ms?: number;
  audioFadeInMs?: number | null;
  audioFadeInCurve?: FadeCurve;
  audioFadeOutMs?: number | null;
  audioFadeOutCurve?: FadeCurve;
  visualFadeInMs?: number | null;
  visualFadeInCurve?: FadeCurve;
  visualFadeOutMs?: number | null;
  visualFadeOutCurve?: FadeCurve;
}

export type MultiCueDraftAction =
  | { type: "set"; field: keyof MultiCueDraft; value: unknown }
  | { type: "applyFailed" }
  | { type: "reset" };

export function multiCueDraftReducer(
  state: MultiCueDraft,
  action: MultiCueDraftAction,
): MultiCueDraft {
  if (action.type === "reset") return {};
  // A failed Apply is intentionally a no-op so the operator can correct or retry the draft.
  if (action.type === "applyFailed") return state;
  return { ...state, [action.field]: action.value };
}

export type ValueState<T> =
  | { kind: "empty" }
  | { kind: "uniform"; value: T }
  | { kind: "mixed" };

export function getValueState<T>(values: readonly T[]): ValueState<T> {
  if (values.length === 0) return { kind: "empty" };
  const first = values[0];
  return values.every((value) => Object.is(value, first))
    ? { kind: "uniform", value: first }
    : { kind: "mixed" };
}

export function getCuePropertyState<K extends keyof CueSummary>(
  cues: readonly MultiCueRecord[],
  key: K,
): ValueState<CueSummary[K]> {
  return getValueState(cues.map((cue) => cue[key] as CueSummary[K]));
}

export type TriState = "checked" | "unchecked" | "mixed";

export function getTriState(values: readonly boolean[]): TriState {
  if (values.length === 0) return "mixed";
  const first = values[0];
  return values.every((value) => value === first)
    ? (first ? "checked" : "unchecked")
    : "mixed";
}

export function isCurrentSelectionLoad(
  isActive: boolean,
  requestGeneration: number,
  currentGeneration: number,
): boolean {
  return isActive && requestGeneration === currentGeneration;
}

export function nextGeneration(currentGeneration: number): number {
  return currentGeneration + 1;
}

export function isCurrentApplyRequest(
  requestGeneration: number,
  currentGeneration: number,
  requestSelectionKey: string,
  currentSelectionKey: string,
): boolean {
  return requestGeneration === currentGeneration && requestSelectionKey === currentSelectionKey;
}

export function withApplyError<T extends { selectionKey: string; error: string | null }>(
  previous: T,
  selectionKey: string,
  error: string,
): T {
  return previous.selectionKey === selectionKey ? { ...previous, error } : previous;
}

export function findCueSummaryById(cues: readonly CueSummary[], cueId: string): CueSummary | undefined {
  for (const cue of cues) {
    if (cue.id === cueId) return cue;
    const nested = cue.children && findCueSummaryById(cue.children, cueId);
    if (nested) return nested;
  }
  return undefined;
}

export interface MultiCueCapabilities {
  levels: boolean;
  audioFade: boolean;
  visualFade: boolean;
  visual: boolean;
  outputs: boolean;
}

export interface FadeEditRuntimeCue {
  state?: unknown;
  is_loading?: unknown;
  loading?: unknown;
  preloaded?: unknown;
  is_preloaded?: unknown;
  fading?: unknown;
  is_fading?: unknown;
  _bulk_edit?: BulkEditMetadata;
}

export function getMultiCueCapabilities(types: readonly CueType[]): MultiCueCapabilities {
  const hasSelection = types.length > 0;
  return {
    levels: hasSelection && types.every((type) => type === "audio" || type === "video"),
    audioFade: hasSelection && types.every((type) => type === "audio" || type === "video"),
    visualFade: hasSelection && types.every((type) => type === "video" || type === "image" || type === "camera"),
    visual: hasSelection && types.every((type) => type === "video" || type === "image" || type === "camera"),
    outputs: hasSelection && types.every((type) => type === "video" || type === "image" || type === "camera" || type === "text"),
  };
}

export type FadeFamily = "audio" | "visual";
export type FadeDirection = "in" | "out";
export type FadePart = "Ms" | "Curve";

const DIRECT_FIELDS = [
  "color",
  "notes",
  "continue_mode",
  "is_disabled",
  "pre_wait_ms",
  "post_wait_ms",
] as const satisfies readonly (keyof MultiCueDraft)[];

function fadeField(
  cue: MultiCueRecord,
  family: FadeFamily,
  direction: FadeDirection,
  part: FadePart,
): string | null {
  if (family === "audio") {
    if (cue.cue_type !== "audio" && cue.cue_type !== "video") return null;
    return `fade_${direction}_${part === "Ms" ? "ms" : "curve"}`;
  }

  if (cue.cue_type === "video" || cue.cue_type === "camera") {
    return `video_fade_${direction}_${part === "Ms" ? "ms" : "curve"}`;
  }
  if (cue.cue_type === "image") {
    return `fade_${direction}_${part === "Ms" ? "ms" : "curve"}`;
  }
  return null;
}

export function getFadeValueState(
  cues: readonly MultiCueRecord[],
  family: FadeFamily,
  direction: FadeDirection,
  part: FadePart,
): ValueState<unknown> {
  const keys = cues.map((cue) => fadeField(cue, family, direction, part));
  if (keys.some((key) => key == null) || keys.some((key, index) => !(key! in cues[index]))) {
    return { kind: "empty" };
  }
  return getValueState(cues.map((cue, index) => cue[keys[index]!]));
}

const FADE_DRAFT_FIELDS: readonly {
  draft: keyof MultiCueDraft;
  family: FadeFamily;
  direction: FadeDirection;
  part: FadePart;
}[] = [
  { draft: "audioFadeInMs", family: "audio", direction: "in", part: "Ms" },
  { draft: "audioFadeInCurve", family: "audio", direction: "in", part: "Curve" },
  { draft: "audioFadeOutMs", family: "audio", direction: "out", part: "Ms" },
  { draft: "audioFadeOutCurve", family: "audio", direction: "out", part: "Curve" },
  { draft: "visualFadeInMs", family: "visual", direction: "in", part: "Ms" },
  { draft: "visualFadeInCurve", family: "visual", direction: "in", part: "Curve" },
  { draft: "visualFadeOutMs", family: "visual", direction: "out", part: "Ms" },
  { draft: "visualFadeOutCurve", family: "visual", direction: "out", part: "Curve" },
];

export function hasDirtyFadeFields(draft: MultiCueDraft): boolean {
  return FADE_DRAFT_FIELDS.some(({ draft: field }) => Object.prototype.hasOwnProperty.call(draft, field));
}

export function isCueLiveForFadeEdit(cue: FadeEditRuntimeCue): boolean {
  const state = String(cue.state ?? "").toLowerCase();
  return ["running", "paused", "loading", "preloaded", "fading"].includes(state)
    || cue.is_loading === true
    || cue.loading === true
    || cue.preloaded === true
    || cue.is_preloaded === true
    || cue.fading === true
    || cue.is_fading === true;
}

export function shouldBlockFadeApply(
  cues: readonly FadeEditRuntimeCue[],
  draft: MultiCueDraft,
): boolean {
  return hasDirtyFadeFields(draft) && cues.some((cue) =>
    cue._bulk_edit?.fade_editable === false || isCueLiveForFadeEdit(cue));
}

export interface CueRuntimeRefreshSnapshot {
  cueId: string;
  state: string;
  isLoading: boolean;
  hasTiming: boolean;
  summaryRevision: object;
}

function isTerminalCueState(state: string): boolean {
  return state === "standby" || state === "completed";
}

/** Detect a selected cue becoming safe, including preloaded standby cues whose
 * summary is replaced with the same state when Stop clears backend readiness. */
export function shouldRefreshFadeCapabilities(
  previous: readonly CueRuntimeRefreshSnapshot[],
  current: readonly CueRuntimeRefreshSnapshot[],
  staleCapabilityCueIds: readonly string[],
): boolean {
  const previousById = new Map(previous.map((snapshot) => [snapshot.cueId, snapshot]));
  const stale = new Set(staleCapabilityCueIds);
  return current.some((snapshot) => {
    const prior = previousById.get(snapshot.cueId);
    if (!stale.has(snapshot.cueId) || !isTerminalCueState(snapshot.state) || snapshot.hasTiming) return false;
    if (!prior) return !snapshot.isLoading;
    const becameClearlySafe = !snapshot.isLoading && (prior.hasTiming
      || prior.isLoading
      || !isTerminalCueState(prior.state));
    const terminalSummaryChanged = prior.summaryRevision !== snapshot.summaryRevision;
    return becameClearlySafe || terminalSummaryChanged;
  });
}

/** Replace a capability snapshot only when it is a complete match for the
 * current selection; never mix records from different selection requests. */
export function mergeCapabilityCueRecords(
  current: readonly MultiCueRecord[],
  refreshed: readonly MultiCueRecord[],
): MultiCueRecord[] | null {
  if (current.length === 0 || current.length !== refreshed.length) return null;
  const byId = new Map(refreshed.map((cue) => [cue.id, cue]));
  if (byId.size !== refreshed.length || current.some((cue) => !byId.has(cue.id))) return null;
  return current.map((cue) => byId.get(cue.id)!);
}

export function withCapabilityCueRefresh<T extends {
  selectionKey: string;
  cues: MultiCueRecord[] | null;
}>(
  previous: T,
  selectionKey: string,
  refreshedCues: MultiCueRecord[],
): T {
  if (previous.selectionKey !== selectionKey || !previous.cues) return previous;
  const cues = mergeCapabilityCueRecords(previous.cues, refreshedCues);
  return cues ? { ...previous, cues } : previous;
}

export interface BulkCueUpdate {
  cueId: string;
  properties: Record<string, unknown>;
}

/** Map semantic, dirty UI fields onto only the supported persisted fields. */
export function buildBulkCueUpdates(
  cues: readonly MultiCueRecord[],
  draft: MultiCueDraft,
): BulkCueUpdate[] {
  return cues.flatMap((cue) => {
    const properties: Record<string, unknown> = {};
    for (const field of DIRECT_FIELDS) {
      if (Object.prototype.hasOwnProperty.call(draft, field) && field in cue) {
        properties[field] = draft[field];
      }
    }
    for (const mapping of FADE_DRAFT_FIELDS) {
      if (!Object.prototype.hasOwnProperty.call(draft, mapping.draft)) continue;
      const key = fadeField(cue, mapping.family, mapping.direction, mapping.part);
      if (key && key in cue) properties[key] = draft[mapping.draft];
    }
    return Object.keys(properties).length > 0 ? [{ cueId: cue.id, properties }] : [];
  });
}
