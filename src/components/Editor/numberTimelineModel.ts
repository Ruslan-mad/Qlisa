import type { CueSummary, GroupMode, NumberCueData, WaveformData } from "../../lib/types";

export type NumberVisualAction = CueSummary & {
  start_time_ms?: number | null;
  end_time_ms?: number | null;
  display_duration_ms?: number | null;
  timeline_start_ms?: number;
  timeline_end_ms?: number;
  /** The child is configured to repeat its cropped source until the Number ends. */
  looped?: boolean;
};

export type NumberDragKind = "move" | "start" | "end";

export interface NumberAudioSegment {
  cueId: string;
  startMs: number;
  endMs: number;
  sourceStartMs: number;
  sourceEndMs: number;
}

type WaveformAsset = WaveformData | null | undefined;

/**
 * Source-file duration, not the playable Cue duration. `get_cue(Number)`
 * serializes media children with `cached_duration_ms`; Cue-list summaries use
 * `duration_ms` instead and that value has already had trim/loop rules
 * applied. Keep those two meanings separate: source coordinates must always
 * be calculated against the complete file.
 */
function cueFileDurationMs(cue: CueSummary): number {
  return Math.max(0,
    cue.file_duration_ms
      ?? (cue as CueSummary & { cached_duration_ms?: number | null }).cached_duration_ms
      ?? cue.duration_ms
      ?? 0,
  );
}

function cueMediaDurationMs(cue: CueSummary): number {
  // A Cue-list summary already contains the effective duration. A Number's
  // raw serialized children do not: they contain the full file duration plus
  // trim boundaries. Without this branch a trimmed child stayed full-width
  // in a Group waveform even though the native Group duration was correct.
  if (cue.cue_type !== "audio" && cue.cue_type !== "video") {
    return Math.max(0, cue.duration_ms ?? cueFileDurationMs(cue));
  }
  const fileDuration = cueFileDurationMs(cue);
  const timed = cue as CueSummary & { start_time_ms?: number | null; end_time_ms?: number | null };
  if (timed.start_time_ms == null && timed.end_time_ms == null) {
    return Math.max(0, cue.duration_ms ?? fileDuration);
  }
  const startMs = Math.min(fileDuration, Math.max(0, timed.start_time_ms ?? 0));
  const endMs = Math.min(fileDuration, Math.max(startMs, timed.end_time_ms ?? fileDuration));
  return endMs - startMs;
}

function cueSourceWindow(cue: CueSummary): { startMs: number; endMs: number } {
  const timed = cue as CueSummary & { start_time_ms?: number | null; end_time_ms?: number | null };
  const fileDuration = cueFileDurationMs(cue);
  const startMs = Math.min(fileDuration, Math.max(0, timed.start_time_ms ?? 0));
  const rawEnd = timed.end_time_ms ?? fileDuration;
  return { startMs, endMs: Math.min(fileDuration, Math.max(startMs, rawEnd ?? startMs)) };
}

function leafDurationWithWaits(cue: CueSummary): number {
  return Math.max(0, cue.pre_wait_ms ?? 0) + cueMediaDurationMs(cue) + Math.max(0, cue.post_wait_ms ?? 0);
}

function cueTimelineDurationMs(cue: CueSummary): number {
  return cue.cue_type === "group" ? groupDurationMs(cue) : cueMediaDurationMs(cue);
}

/** Derive a Group duration when the serialized summary has no duration. */
export function groupDurationMs(group: CueSummary): number {
  if (group.cue_type !== "group") return cueMediaDurationMs(group);
  const stored = cueMediaDurationMs(group);
  const children = group.children ?? [];
  if (children.length === 0) return stored;
  if (group.group_mode === "start_random" || (group.group_mode === "playlist" && group.playlist_loop)) return stored;
  const durations = children.map((child) => child.cue_type === "group"
    ? groupDurationMs(child)
    : leafDurationWithWaits(child));
  if (durations.some((duration) => duration <= 0)) return stored;
  const inner = group.group_mode === "sequential" || group.group_mode === "playlist"
    ? durations.reduce((sum, duration) => sum + duration, 0)
    : Math.max(...durations);
  const derived = Math.max(0, group.pre_wait_ms ?? 0) + inner + Math.max(0, group.post_wait_ms ?? 0);
  return stored > 0 ? stored : derived;
}

function directChildSegments(cue: CueSummary, baseMs: number): NumberAudioSegment[] {
  const children = cue.children ?? [];
  const mode: GroupMode = cue.group_mode ?? "simultaneous";
  const groupStart = baseMs + Math.max(0, cue.pre_wait_ms ?? 0);
  const result: NumberAudioSegment[] = [];
  let cursor = groupStart;
  for (const child of children) {
    const childStart = mode === "simultaneous" || mode === "start_random"
      ? groupStart + Math.max(0, child.pre_wait_ms ?? 0)
      : cursor + Math.max(0, child.pre_wait_ms ?? 0);
    const duration = cueTimelineDurationMs(child);
    if (child.cue_type === "audio" || child.cue_type === "video") {
      const source = cueSourceWindow(child);
      result.push({
        cueId: child.id,
        startMs: childStart,
        endMs: childStart + Math.max(0, duration),
        sourceStartMs: source.startMs,
        sourceEndMs: source.endMs,
      });
    } else if (child.cue_type === "group") {
      result.push(...directChildSegments(child, childStart));
    }
    if (mode === "sequential" || mode === "playlist") {
      cursor = childStart + duration + Math.max(0, child.post_wait_ms ?? 0);
    }
  }
  return result;
}

/** Resolve nested audio/video children onto a Group's own local clock. */
export function groupAudioSegments(group: CueSummary): NumberAudioSegment[] {
  if (group.cue_type !== "group") return [];
  return directChildSegments(group, 0);
}

/**
 * Fold nested child waveforms into one Group waveform.  Overlapping children
 * use the maximum envelope, which keeps simultaneous groups readable without
 * clipping or making the result depend on child ordering.
 */
export function composeGroupWaveform(
  group: CueSummary,
  assets: Record<string, WaveformAsset>,
  bins = 1200,
): WaveformData | null {
  const segments = groupAudioSegments(group);
  // Group summaries often have no stored duration. Derive it from their
  // descendants so a Number master Group can still paint its composed wave.
  const totalMs = groupDurationMs(group);
  if (segments.length === 0 || totalMs <= 0 || bins <= 0) return null;
  const peaks = Array.from({ length: bins }, () => 0);
  const rms = Array.from({ length: bins }, () => 0);
  let contributed = false;
  for (const segment of segments) {
    const source = assets[segment.cueId];
    if (!source?.peaks.length) continue;
    contributed = true;
    const startBin = Math.max(0, Math.floor((segment.startMs / totalMs) * bins));
    const endBin = Math.min(bins, Math.ceil((segment.endMs / totalMs) * bins));
    const sourceSpan = Math.max(1, segment.sourceEndMs - segment.sourceStartMs);
    const segmentSpan = Math.max(1, segment.endMs - segment.startMs);
    for (let bin = startBin; bin < endBin; bin++) {
      const timelineMs = ((bin + 0.5) / bins) * totalMs;
      const sourceMs = segment.sourceStartMs + Math.max(0, Math.min(sourceSpan, timelineMs - segment.startMs)) * sourceSpan / segmentSpan;
      const sourceBin = Math.min(source.peaks.length - 1, Math.max(0, Math.floor((sourceMs / Math.max(1, source.file_duration_s * 1000)) * source.peaks.length)));
      peaks[bin] = Math.max(peaks[bin], source.peaks[sourceBin] ?? 0);
      rms[bin] = Math.max(rms[bin], source.rms[sourceBin] ?? 0);
    }
  }
  return contributed ? { peaks, rms, file_duration_s: totalMs / 1000 } : null;
}

/**
 * Calculate one pointer update for an editable Number action.
 *
 * Move keeps the authored block length and clamps the whole block to the
 * master. Trim changes only the edge being dragged. Keeping this arithmetic
 * outside the canvas component makes it harder for a snap or a re-render to
 * accidentally turn a trim into a move.
 */
export function numberDragRange(
  kind: NumberDragKind,
  originStartMs: number,
  originEndMs: number,
  pointerMs: number,
  masterDurationMs: number,
  maxSpanMs = Number.POSITIVE_INFINITY,
): { startMs: number; endMs: number } {
  const duration = Math.max(0, masterDurationMs);
  const start = Math.max(0, Math.min(duration, originStartMs));
  const end = Math.max(start, Math.min(duration, originEndMs));
  const maxSpan = Number.isFinite(maxSpanMs) ? Math.max(0, maxSpanMs) : Number.POSITIVE_INFINITY;
  if (kind === "start") {
    return { startMs: Math.max(Math.max(0, end - maxSpan), Math.min(end, pointerMs)), endMs: end };
  }
  if (kind === "end") {
    return { startMs: start, endMs: Math.max(start, Math.min(Math.min(duration, start + maxSpan), pointerMs)) };
  }
  const length = Math.min(end - start, maxSpan);
  const nextStart = Math.max(0, Math.min(Math.max(0, duration - length), pointerMs));
  return { startMs: nextStart, endMs: nextStart + length };
}

/** Apply a snap result without allowing a trim to move its opposite edge. */
export function applyNumberDragSnap(
  kind: NumberDragKind,
  proposed: { startMs: number; endMs: number },
  snapped: { startMs: number; endMs: number },
): { startMs: number; endMs: number } {
  if (kind === "start") return { startMs: Math.min(snapped.startMs, proposed.endMs), endMs: proposed.endMs };
  if (kind === "end") return { startMs: proposed.startMs, endMs: Math.max(proposed.startMs, snapped.endMs) };
  const delta = snapped.startMs - proposed.startMs;
  return { startMs: snapped.startMs, endMs: proposed.endMs + delta };
}

export function snapNumberTime(value: number, duration: number, edges: number[], threshold: number): number {
  const clamped = Math.max(0, Math.min(Math.max(0, duration), value));
  let best = clamped;
  let distance = threshold + 1;
  for (const edge of [0, duration, ...edges]) {
    const candidate = Math.max(0, Math.min(duration, edge));
    const nextDistance = Math.abs(candidate - clamped);
    if (nextDistance <= threshold && nextDistance < distance) {
      best = candidate;
      distance = nextDistance;
    }
  }
  return Math.round(best);
}

export function numberMasterDuration(cue: NumberCueData): number {
  const master = cue.children.find((child) => child.id === cue.number_master_id);
  if (master?.cue_type === "group") return groupDurationMs(master);
  return Math.max(0, master?.duration_ms ?? master?.file_duration_ms ?? master?.cached_duration_ms ?? 0);
}

export function numberActionDuration(child: NumberVisualAction, masterDuration: number): number {
  if (child.cue_type === "image") return Math.max(0, child.display_duration_ms ?? masterDuration);
  if (child.cue_type === "group") return Math.max(0, groupDurationMs(child) || masterDuration);
  const file = Math.max(0, child.file_duration_ms ?? child.duration_ms ?? child.cached_duration_ms ?? 0);
  const start = Math.max(0, child.start_time_ms ?? 0);
  const end = child.end_time_ms == null ? file : Math.max(start, child.end_time_ms);
  return Math.max(0, end - start);
}

export function numberActionLooped(child: NumberVisualAction): boolean {
  return (child.cue_type === "audio" || child.cue_type === "video") && (child.loop_count ?? 0) > 0;
}

export function numberVisualActions(cue: NumberCueData): NumberVisualAction[] {
  const duration = numberMasterDuration(cue);
  // The backend omits this field when a Number has no configured offsets.
  const offsets = cue.number_action_offsets_ms ?? {};
  return cue.children
    .filter((child) => child.id !== cue.number_master_id && (child.cue_type === "audio" || child.cue_type === "video" || child.cue_type === "image" || child.cue_type === "group"))
    .map((child) => {
      const action = child as NumberVisualAction;
      const start = Math.min(duration, Math.max(0, offsets[child.id] ?? 0));
      const looped = numberActionLooped(action);
      return {
        ...action,
        looped,
        timeline_start_ms: start,
        // A looping child occupies the rest of the Number clock. The native
        // timeline paints one source pass per block instead of stretching it.
        timeline_end_ms: looped ? duration : Math.min(duration, start + numberActionDuration(action, duration)),
      };
    });
}

/**
 * Flatten a Group onto the Number clock. The Group itself is a container, so
 * its audio/video descendants get independent lanes with their real offsets
 * and source windows instead of one combined waveform sausage.
 */
export function numberGroupTimelineActions(
  group: CueSummary,
  baseMs: number,
  limitMs: number,
  prefix = group.name,
): NumberVisualAction[] {
  if (group.cue_type !== "group") return [];
  const mode: GroupMode = group.group_mode ?? "simultaneous";
  const groupStart = baseMs + Math.max(0, group.pre_wait_ms ?? 0);
  let cursor = groupStart;
  const result: NumberVisualAction[] = [];
  for (const child of group.children ?? []) {
    const childStart = mode === "simultaneous" || mode === "start_random"
      ? groupStart + Math.max(0, child.pre_wait_ms ?? 0)
      : cursor + Math.max(0, child.pre_wait_ms ?? 0);
    const duration = cueTimelineDurationMs(child);
    if (child.cue_type === "group") {
      result.push(...numberGroupTimelineActions(child, childStart, limitMs, `${prefix} · ${child.name}`));
    } else if (child.cue_type === "audio" || child.cue_type === "video") {
      const action = child as NumberVisualAction;
      const looped = numberActionLooped(action);
      const end = Math.min(limitMs, looped ? limitMs : childStart + Math.max(0, duration));
      result.push({
        ...action,
        name: `${prefix} · ${child.name}`,
        timeline_start_ms: Math.max(0, childStart),
        timeline_end_ms: Math.max(childStart, end),
        looped,
      });
    }
    if (mode === "sequential" || mode === "playlist") {
      cursor = childStart + duration + Math.max(0, child.post_wait_ms ?? 0);
    }
  }
  return result;
}

/**
 * Resolve the visual that should be visible at one Number-clock position.
 * Children are authored in list order, so the last active visual is the
 * deterministic topmost item when actions overlap. The master is the fallback
 * visual for gaps; audio-only masters deliberately return no preview item.
 */
export function numberPreviewItem(cue: NumberCueData, positionMs: number): NumberVisualAction | null {
  const position = Math.max(0, positionMs);
  const active = numberVisualActions(cue)
    .filter((item) => (item.cue_type === "video" || item.cue_type === "image")
      && position >= (item.timeline_start_ms ?? 0)
      && position < (item.timeline_end_ms ?? 0));
  const action = active.length > 0 ? active[active.length - 1] : undefined;
  if (action) return action;

  const master = cue.children.find((child) => child.id === cue.number_master_id);
  // Number masters are audio or video. Treat malformed image-master data as
  // audio-only here instead of inventing a second visual master track.
  if (!master || master.cue_type !== "video") return null;
  return {
    ...(master as NumberVisualAction),
    timeline_start_ms: 0,
    timeline_end_ms: numberMasterDuration(cue),
  };
}

/** Source-file time for the frame shown by `numberPreviewItem`. */
export function numberPreviewSourcePosition(item: NumberVisualAction, positionMs: number): number {
  const timelineStart = item.timeline_start_ms ?? 0;
  const { startMs: sourceStart, endMs: sourceEnd } = numberPreviewSourceWindow(item);
  const elapsed = Math.max(0, positionMs - timelineStart);
  const span = sourceEnd - sourceStart;
  if (item.looped && span > 0) return sourceStart + (elapsed % span);
  return Math.max(sourceStart, Math.min(sourceEnd, sourceStart + elapsed));
}

/** Absolute source bounds used by the preview player when it repeats a crop. */
export function numberPreviewSourceWindow(item: NumberVisualAction): { startMs: number; endMs: number } {
  const startMs = Math.max(0, item.start_time_ms ?? 0);
  const endMs = Math.max(startMs, item.end_time_ms ?? item.file_duration_ms ?? item.duration_ms ?? item.cached_duration_ms ?? startMs);
  return { startMs, endMs };
}

/** Keep the last decoded layer visible while another layer is loading. */
export function shouldRetainNumberPreviewAsset(
  activeItemKey: string | null,
  loadedItemKey: string | null,
  loadedAssetUrl: string | null,
): boolean {
  return activeItemKey !== null && loadedItemKey !== activeItemKey && loadedAssetUrl !== null;
}

/**
 * The visible Pre-Wait of a Number child is its position on the Number clock.
 * Child cue pre-wait is deliberately forced to zero by the backend, so using
 * that field in the list would hide the action's actual timeline position.
 */
export function numberChildTimelineStartMs(cue: NumberCueData, childId: string): number | null {
  if (!cue.children.some((child) => child.id === childId)) return null;
  if (cue.number_master_id === childId) return 0;
  return Math.max(0, cue.number_action_offsets_ms?.[childId] ?? 0);
}
