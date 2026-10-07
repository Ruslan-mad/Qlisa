import type { CueSummary, StatusBarMetricId, StatusBarMetricPreference, StatusBarPreferences, NumberCueData } from "../../lib/types";
import { groupDurationMs, numberMasterDuration } from "../Editor/numberTimelineModel";

export const STATUS_BAR_METRICS: StatusBarMetricId[] = [
  "cue_count", "duration", "active", "problems", "cpu", "gpu", "vram", "ram",
  "audio_gaps", "video_fps", "network_drops", "app_ram", "disk",
];
const DEFAULT_LEFT: StatusBarMetricId[] = ["cue_count", "duration", "active", "problems", "audio_gaps", "video_fps", "network_drops"];
const DEFAULT_RIGHT: StatusBarMetricId[] = ["cpu", "gpu", "vram", "ram", "app_ram", "disk"];

export const DEFAULT_STATUS_BAR_PREFERENCES: StatusBarPreferences = {
  visible: true,
  left: DEFAULT_LEFT.map((id, index) => ({ id, enabled: index < 4 })),
  right: DEFAULT_RIGHT.map((id, index) => ({ id, enabled: index < 4 })),
  gpu_adapter_id: null,
};

const VALID_METRICS = new Set<string>(STATUS_BAR_METRICS);
const clonePrefs = (source: StatusBarPreferences): StatusBarPreferences => ({
  visible: source.visible,
  left: source.left.map((metric) => ({ ...metric })),
  right: source.right.map((metric) => ({ ...metric })),
  gpu_adapter_id: source.gpu_adapter_id ?? null,
});

/** Normalize older or hand-edited preferences without enabling new metrics. */
export function normalizeStatusBarPreferences(value?: Partial<StatusBarPreferences> | null): StatusBarPreferences {
  if (!value) return clonePrefs(DEFAULT_STATUS_BAR_PREFERENCES);
  const left: StatusBarMetricPreference[] = [];
  const right: StatusBarMetricPreference[] = [];
  const seen = new Set<string>();
  const take = (items: StatusBarMetricPreference[] | undefined, into: StatusBarMetricPreference[]) => {
    for (const item of items ?? []) {
      if (!item || !VALID_METRICS.has(item.id) || seen.has(item.id)) continue;
      seen.add(item.id);
      into.push({ id: item.id, enabled: item.enabled === true });
    }
  };
  take(value.left, left);
  take(value.right, right);
  for (const id of DEFAULT_LEFT) if (!seen.has(id)) left.push({ id, enabled: false });
  for (const id of DEFAULT_RIGHT) if (!seen.has(id)) right.push({ id, enabled: false });
  return { visible: value.visible !== false, left, right, gpu_adapter_id: value.gpu_adapter_id ?? null };
}

export function moveStatusMetric(prefs: StatusBarPreferences, id: StatusBarMetricId, destination: "left" | "right"): StatusBarPreferences {
  const next = clonePrefs(prefs);
  const from = next.left.some((metric) => metric.id === id) ? next.left : next.right;
  const to = destination === "left" ? next.left : next.right;
  if (from === to) return next;
  const index = from.findIndex((metric) => metric.id === id);
  if (index >= 0) to.push(from.splice(index, 1)[0]);
  return next;
}

export function reorderStatusMetric(prefs: StatusBarPreferences, side: "left" | "right", id: StatusBarMetricId, delta: -1 | 1): StatusBarPreferences {
  const next = clonePrefs(prefs);
  const items = side === "left" ? next.left : next.right;
  const index = items.findIndex((metric) => metric.id === id);
  const target = index + delta;
  if (index < 0 || target < 0 || target >= items.length) return next;
  [items[index], items[target]] = [items[target], items[index]];
  return next;
}

export interface CueStatusMetrics {
  total: number; groups: number; numbers: number; disabled: number;
  knownDurationMs: number; unknownDurationCount: number; infiniteCount: number;
  problems: number; missingMedia: number;
}

type DurationResult = { kind: "finite"; ms: number } | { kind: "unknown" } | { kind: "infinite" };
const INFINITE_LOOP = 0xffff_ffff;

function effectiveDuration(cue: CueSummary): DurationResult {
  if (cue.is_disabled) return { kind: "finite", ms: 0 };
  const preWait = cue.pre_wait_ms ?? 0;
  const postWait = cue.post_wait_ms ?? 0;
  if (!Number.isFinite(preWait) || !Number.isFinite(postWait)) return { kind: "unknown" };
  const waits = Math.max(0, preWait) + Math.max(0, postWait);
  if (cue.duration_ms === Number.POSITIVE_INFINITY) return { kind: "infinite" };
  if (cue.duration_ms != null && (!Number.isFinite(cue.duration_ms) || cue.duration_ms < 0)) return { kind: "unknown" };
  if ((cue.cue_type === "audio" || cue.cue_type === "video") && cue.loop_count === INFINITE_LOOP) return { kind: "infinite" };
  if (cue.cue_type === "group" && cue.group_mode === "playlist" && cue.playlist_loop) return { kind: "infinite" };
  // Group summaries already include their own and child waits. Number summaries
  // include the master and post-wait, but omit the Number pre-wait. Leaf
  // summaries include trim/repeat and still need both waits added.
  if (cue.duration_ms != null && Number.isFinite(cue.duration_ms) && cue.duration_ms >= 0) {
    if (cue.cue_type === "group") return { kind: "finite", ms: cue.duration_ms };
    if (cue.cue_type === "number") return { kind: "finite", ms: cue.duration_ms + Math.max(0, preWait) };
    return { kind: "finite", ms: cue.duration_ms + waits };
  }
  if (["image", "text", "camera", "mic", "browser"].includes(cue.cue_type)) return { kind: "infinite" };
  if (cue.cue_type === "group") {
    if (cue.group_mode === "start_random") return { kind: "unknown" };
    const children = (cue.children ?? []).filter((child) => !child.is_disabled);
    if (children.length === 0) return { kind: "finite", ms: waits };
    const durations = children.map(effectiveDuration);
    if (durations.some((item) => item.kind === "infinite")) return { kind: "infinite" };
    if (durations.some((item) => item.kind === "unknown")) return { kind: "unknown" };
    const normalized = children.map((child, index) => ({
      ...child,
      pre_wait_ms: 0,
      post_wait_ms: 0,
      duration_ms: (durations[index] as { kind: "finite"; ms: number }).ms,
      file_duration_ms: null,
    }));
    const safeGroup = { ...cue, duration_ms: null, children: normalized };
    const helperDuration = groupDurationMs(safeGroup);
    const sequential = cue.group_mode === "sequential" || cue.group_mode === "playlist";
    const derived = helperDuration > 0
      ? helperDuration
      : (sequential
        ? durations.reduce((sum, item) => sum + (item.kind === "finite" ? item.ms : 0), 0)
        : Math.max(...durations.map((item) => item.kind === "finite" ? item.ms : 0)));
    return { kind: "finite", ms: helperDuration > 0 ? derived : derived + waits };
  }
  if (cue.cue_type === "number") {
    const master = cue.children?.find((child) => child.id === cue.number_master_id);
    if (master?.is_disabled) return { kind: "unknown" };
    if (!master) return { kind: "unknown" };
    if (["image", "text", "camera", "mic", "browser"].includes(master.cue_type) && master.duration_ms == null) return { kind: "infinite" };
    if (master.duration_ms === Number.POSITIVE_INFINITY) return { kind: "infinite" };
    if ((master.cue_type === "audio" || master.cue_type === "video") && master.loop_count === INFINITE_LOOP) return { kind: "infinite" };
    if (master.cue_type === "group") {
      const groupDuration = effectiveDuration(master);
      if (groupDuration.kind !== "finite") return groupDuration;
      return { kind: "finite", ms: groupDuration.ms + waits };
    }
    const numberDuration = numberMasterDuration(cue as NumberCueData);
    if (Number.isFinite(numberDuration) && numberDuration > 0) return { kind: "finite", ms: numberDuration + waits };
    const hasKnownMasterDuration = [master.duration_ms, master.file_duration_ms, master.cached_duration_ms]
      .some((duration) => duration != null && Number.isFinite(duration) && duration >= 0);
    if (numberDuration === 0 && hasKnownMasterDuration) return { kind: "finite", ms: waits };
    return { kind: "unknown" };
  }
  if (["memo", "stop", "fade", "pause", "resume", "load", "reset", "goto", "arm", "disarm", "devamp", "osc", "midi", "light"].includes(cue.cue_type)) {
    return { kind: "finite", ms: waits };
  }
  return { kind: "unknown" };
}

export function buildCueStatusMetrics(cues: CueSummary[], brokenIds: ReadonlySet<string> = new Set()): CueStatusMetrics {
  const all: CueSummary[] = [];
  const visit = (items: CueSummary[]) => items.forEach((cue) => { all.push(cue); if (cue.children?.length) visit(cue.children); });
  visit(cues);
  const duration = cues.filter((cue) => !cue.is_disabled).map(effectiveDuration);
  const issues = new Set<string>();
  const missing = new Set<string>();
  for (const cue of all) {
    if (cue.is_broken || cue.is_warning || brokenIds.has(cue.id)) issues.add(cue.id);
    if (cue.media_file_missing) missing.add(cue.id);
  }
  return {
    total: all.length,
    groups: all.filter((cue) => cue.cue_type === "group").length,
    numbers: all.filter((cue) => cue.cue_type === "number").length,
    disabled: all.filter((cue) => cue.is_disabled).length,
    knownDurationMs: duration.reduce((sum, item) => sum + (item.kind === "finite" ? item.ms : 0), 0),
    unknownDurationCount: duration.filter((item) => item.kind === "unknown").length,
    infiniteCount: duration.filter((item) => item.kind === "infinite").length,
    problems: issues.size,
    missingMedia: missing.size,
  };
}

export function isStatusSampleStale(timestampMs: number | null | undefined, nowMs: number, maxAgeMs = 3000): boolean {
  return timestampMs == null || !Number.isFinite(timestampMs) || nowMs - timestampMs > maxAgeMs || timestampMs > nowMs + 1000;
}

export type LoadBand = "neutral" | "amber" | "red";
export interface LoadBandState { band: LoadBand; candidate: LoadBand; sinceMs: number; }

/** Require sustained saturation; GPU saturation becomes red only with an FPS issue. */
export function updateLoadBand(metric: "cpu" | "gpu" | "ram" | "vram", value: number | null, nowMs: number, previous?: LoadBandState, fpsIssue = false): LoadBandState {
  if (value == null || !Number.isFinite(value)) return { band: "neutral", candidate: "neutral", sinceMs: nowMs };
  const amberAt = metric === "vram" ? 88 : metric === "gpu" ? 90 : 82;
  const redAt = metric === "vram" ? 97 : metric === "gpu" ? 97 : 95;
  let candidate: LoadBand = value >= redAt && (metric !== "gpu" || fpsIssue) ? "red" : value >= amberAt ? "amber" : "neutral";
  if (previous?.band === "red" && candidate === "amber" && value >= redAt - 4 && (metric !== "gpu" || fpsIssue)) candidate = "red";
  else if (previous?.band === "amber" && candidate === "neutral" && value >= amberAt - 5) candidate = "amber";
  const sinceMs = previous?.candidate === candidate ? previous.sinceMs : nowMs;
  const holdMs = candidate === "red" ? 5000 : candidate === "amber" ? 3000 : 0;
  const band = candidate === "neutral" || nowMs - sinceMs >= holdMs ? candidate : previous?.band ?? "neutral";
  return { band, candidate, sinceMs };
}

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return Math.round(bytes) + " B";
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = -1;
  do { value /= 1024; unit += 1; } while (value >= 1024 && unit < units.length - 1);
  return value.toFixed(value >= 10 ? 0 : 1) + " " + units[unit];
}

export function formatBytePair(used: number | null | undefined, total: number | null | undefined): string {
  if (used == null || total == null || !Number.isFinite(used) || !Number.isFinite(total) || used < 0 || total <= 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const scale = Math.min(units.length - 1, Math.floor(Math.log(Math.max(used, total, 1)) / Math.log(1024)));
  const divisor = 1024 ** scale;
  const format = (value: number) => scale === 0 ? String(Math.round(value)) : (value / divisor).toFixed(value / divisor >= 10 ? 0 : 1);
  return `${format(used)}/${format(total)} ${units[scale]}`;
}

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  const seconds = Math.round(ms / 1000);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remaining = seconds % 60;
  return hours > 0 ? hours + ":" + String(minutes).padStart(2, "0") + ":" + String(remaining).padStart(2, "0") : minutes + ":" + String(remaining).padStart(2, "0");
}
