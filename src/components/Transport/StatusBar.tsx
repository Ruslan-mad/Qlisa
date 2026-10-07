import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { acquireStatusBarPolling, useStatusBarStore } from "../../stores/statusBarStore";
import { useLocale } from "../../i18n";
import type { TranslationVars } from "../../i18n";
import { flattenActiveCues, flattenCatalogActiveCues } from "../ActiveCues/activeCueModel";
import type { GpuAdapterSnapshot, StatusBarMetricId, StatusBarPreferences } from "../../lib/types";
import { buildCueStatusMetrics, DEFAULT_STATUS_BAR_PREFERENCES, formatBytePair, formatBytes, formatDuration, isStatusSampleStale, normalizeStatusBarPreferences, updateLoadBand, type LoadBand, type LoadBandState } from "./statusBarModel";

type MetricView = { value: string; title: string; band?: LoadBand; usage?: number | null; stale?: boolean };
const SYSTEM_METRICS = new Set<StatusBarMetricId>(["cpu", "gpu", "vram", "ram", "app_ram"]);
const RUNTIME_METRICS = new Set<StatusBarMetricId>(["audio_gaps", "video_fps", "network_drops", "disk"]);
const percent = (value: number | null | undefined) => value == null || !Number.isFinite(value) || value < 0 ? "—" : Math.round(value) + "%";

function chosenAdapter(adapters: GpuAdapterSnapshot[], selectedId?: string | null): { adapter: GpuAdapterSnapshot | null; fallback: boolean } {
  if (selectedId) {
    const selected = adapters.find((adapter) => adapter.id === selectedId);
    if (selected) return { adapter: selected, fallback: false };
  }
  const available = adapters.filter((adapter) => adapter.dedicatedTotalBytes != null);
  available.sort((a, b) => (b.dedicatedTotalBytes ?? 0) - (a.dedicatedTotalBytes ?? 0));
  return { adapter: available[0] ?? adapters[0] ?? null, fallback: Boolean(selectedId && !adapters.some((adapter) => adapter.id === selectedId)) };
}

function getMetricView(
  id: StatusBarMetricId,
  context: {
    cues: ReturnType<typeof buildCueStatusMetrics>;
    activeLocal: number;
    activeProject: number;
    activeListName: string;
    system: ReturnType<typeof useStatusBarStore.getState>["system"];
    runtime: ReturnType<typeof useStatusBarStore.getState>["runtime"];
    adapter: GpuAdapterSnapshot | null;
    adapterFallback: boolean;
    systemStale: boolean;
    runtimeStale: boolean;
    networkStale: boolean;
    t: (key: string, vars?: TranslationVars) => string;
  },
): MetricView {
  const { cues, system, runtime, adapter, activeLocal, activeProject, activeListName, adapterFallback, systemStale, runtimeStale, networkStale, t } = context;
  const listNote = activeListName ? "\n" + t("statusBarUi.selectedList", { name: activeListName }) : "";
  const totalDuration = formatDuration(cues.knownDurationMs)
    + (cues.infiniteCount ? " +∞" : "")
    + (cues.unknownDurationCount ? " +?" : "");
  const metric = (value: string, title: string, sourceStale = false, band?: LoadBand, usage?: number | null): MetricView => ({ value, title, stale: sourceStale, band, usage });
  switch (id) {
    case "cue_count":
      return metric(String(cues.total), t("statusBarUi.summary", { groups: cues.groups, numbers: cues.numbers, disabled: cues.disabled }) + listNote);
    case "duration":
      return metric(totalDuration, t("statusBarUi.durationDetails", { unknown: cues.unknownDurationCount, infinite: cues.infiniteCount }) + listNote);
    case "active":
      return metric(String(activeLocal), t("statusBarUi.activeDetails", { local: activeLocal, project: activeProject }));
    case "problems":
      return metric(String(cues.problems), t("statusBarUi.problemsDetails", { problems: cues.problems, missing: cues.missingMedia }));
    case "cpu":
      return metric(percent(system?.systemCpuPercent), t("statusBarUi.metrics.cpu"), systemStale, undefined, system?.systemCpuPercent);
    case "gpu":
      return metric(percent(adapter?.usagePercent), (adapter?.name ?? "GPU") + (adapterFallback ? " — " + t("statusBarUi.adapterFallback") : "") + "; " + t("statusBarUi.videoDecode") + " " + percent(adapter?.videoDecodePercent), systemStale, undefined, adapter?.usagePercent);
    case "vram": {
      const used = adapter?.dedicatedUsedBytes;
      const total = adapter?.dedicatedTotalBytes;
      const ratio = used != null && Number.isFinite(used) && used >= 0 && total != null && Number.isFinite(total) && total > 0 ? (used / total) * 100 : null;
      const band: LoadBand | undefined = ratio == null ? undefined : ratio >= 97 ? "red" : ratio >= 88 ? "amber" : "neutral";
      return metric(formatBytePair(used, total), t("statusBarUi.vramDetails", { name: adapter?.name ?? "GPU" })
        + (adapterFallback ? " — " + t("statusBarUi.adapterFallback") : ""), systemStale, band, ratio);
    }
    case "ram":
      return metric(formatBytePair(system?.systemRamUsedBytes, system?.systemRamTotalBytes), t("statusBarUi.metrics.ram"), systemStale, undefined,
        system?.systemRamTotalBytes != null && Number.isFinite(system.systemRamTotalBytes) && system.systemRamTotalBytes > 0
          && system.systemRamUsedBytes != null && Number.isFinite(system.systemRamUsedBytes) && system.systemRamUsedBytes >= 0
          ? system.systemRamUsedBytes / system.systemRamTotalBytes * 100 : null);
    case "audio_gaps": {
      const events = runtime?.audio?.underrunEvents;
      const frames = runtime?.audio?.silentFrames;
      return metric(events == null || !Number.isFinite(events) || events < 0 ? "—" : String(events), t("statusBarUi.audioDetails", {
        events: events == null || !Number.isFinite(events) || events < 0 ? "—" : events,
        frames: frames == null || !Number.isFinite(frames) || frames < 0 ? "—" : frames,
      }), runtimeStale);
    }
    case "video_fps": {
      const outputs = runtime?.videoOutputs ?? [];
      const known = outputs.filter((output) => output.fps != null && Number.isFinite(output.fps) && output.fps >= 0);
      const min = known.length ? Math.min(...known.map((output) => output.fps!)) : null;
      return metric(min == null ? "—" : Math.round(min) + " fps", outputs.length
        ? outputs.map((output) => t("statusBarUi.fpsRow", {
          name: output.name,
          fps: output.fps == null || !Number.isFinite(output.fps) || output.fps < 0 ? "—" : output.fps.toFixed(1),
          target: output.targetFps == null || !Number.isFinite(output.targetFps) || output.targetFps < 0 ? "—" : output.targetFps,
          dropped: output.droppedFrames == null || !Number.isFinite(output.droppedFrames) || output.droppedFrames < 0 ? "—" : output.droppedFrames,
        })).join("\n")
        : t("statusBarUi.fpsUnavailable"), runtimeStale);
    }
    case "network_drops": {
      const rows = (runtime?.network ?? []).filter((row) => row.active);
      const sum = (values: (number | null)[]) => {
        const counts = values.filter((count): count is number => count != null && Number.isFinite(count) && count >= 0);
        return counts.length ? String(counts.reduce((total, count) => total + count, 0)) : "—";
      };
      const value = t("statusBarUi.networkValues", {
        video: sum(rows.map((row) => row.droppedFrames)),
        audioSamples: sum(rows.map((row) => row.droppedAudioSamples)),
        audioFrames: sum(rows.map((row) => row.droppedAudioFrames)),
      });
      return metric(value,
        rows.length ? t("statusBarUi.networkCounters") + "\n" + rows.map((row) => t("statusBarUi.networkRow", {
          name: row.name, kind: row.kind, direction: row.direction, frames: row.droppedFrames ?? "—", samples: row.droppedAudioSamples ?? "—", audioFrames: row.droppedAudioFrames ?? "—",
          pacing: row.supersededFrames == null ? "" : t("statusBarUi.networkPacing", { count: row.supersededFrames }),
        })).join("\n") : t("statusBarUi.networkUnavailable"),
        networkStale);
    }
    case "app_ram":
      return metric(formatBytes(system?.processRamBytes), t("statusBarUi.appRamDetails"), systemStale);
    case "disk":
      return metric(formatBytes(runtime?.projectDisk?.freeBytes), runtime?.projectDisk?.path
        ? runtime.projectDisk.path + " · " + (runtime.projectDisk.source ?? t("statusBarUi.diskDetails"))
        : t("statusBarUi.diskDetails"), runtimeStale);
  }
  return metric("—", "");
}

function Metric({ id, view, t }: {
  id: StatusBarMetricId;
  view: MetricView;
  t: (key: string, vars?: TranslationVars) => string;
}) {
  const color = view.stale ? "var(--wc-text-faint)"
    : view.band === "red" ? "color-mix(in srgb, #f87171 65%, var(--wc-text))" : view.band === "amber" ? "color-mix(in srgb, #fbbf24 65%, var(--wc-text))" : "var(--wc-text-secondary)";
  const value = view.stale ? "—" : view.value;
  const title = view.stale ? view.title + "\n" + t("statusBarUi.noFreshData") : view.title;
  return <span
    title={title}
    aria-label={t(`statusBarUi.metrics.${id}`) + ": " + value}
    style={{ display: "inline-flex", flex: "0 0 auto", alignItems: "center", gap: 4, height: 25, padding: "0 7px", borderRight: "1px solid var(--wc-border)", color, fontSize: 11, whiteSpace: "nowrap", fontVariantNumeric: "tabular-nums" }}
  >
    <span style={{ color: view.stale ? color : "var(--wc-text-faint)" }}>{t(`statusBarUi.short.${id}`)}</span>
    <strong style={{ fontWeight: 600 }}>{value}</strong>
    {view.usage != null && Number.isFinite(view.usage) && view.usage >= 0 && !view.stale && <span aria-hidden="true" style={{ width: 25, height: 3, flex: "0 0 25px", overflow: "hidden", borderRadius: 2, background: "var(--wc-border-strong)" }}>
      <span style={{ display: "block", height: "100%", width: Math.max(0, Math.min(100, view.usage)) + "%", background: color }} />
    </span>}
  </span>;
}

function enabledOnSide(prefs: StatusBarPreferences, side: "left" | "right") {
  return prefs[side].filter((item) => item.enabled);
}

function MetricStrip({ side, label, contentKey, children }: { side: "left" | "right"; label: string; contentKey: string; children: ReactNode }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState({ left: false, right: false });
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const update = () => setEdges({
      left: element.scrollLeft > 1,
      right: element.scrollLeft + element.clientWidth < element.scrollWidth - 1,
    });
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    if (element.firstElementChild) observer.observe(element.firstElementChild);
    return () => observer.disconnect();
  }, [contentKey]);
  return <div style={{ position: "relative", display: "flex", flex: "1 1 50%", minWidth: 0, overflow: "hidden" }}>
    <div ref={scrollRef} tabIndex={0} aria-label={label} className="status-metric-scroll" onKeyDown={(event) => {
      const element = scrollRef.current;
      if (!element) return;
      const distance = Math.max(80, Math.floor(element.clientWidth * 0.7));
      if (event.key === "ArrowRight" || event.key === "ArrowLeft" || event.key === "Home" || event.key === "End") {
        event.preventDefault();
        event.stopPropagation();
        if (event.key === "Home") element.scrollTo({ left: 0 });
        else if (event.key === "End") element.scrollTo({ left: element.scrollWidth });
        else element.scrollBy({ left: event.key === "ArrowRight" ? distance : -distance });
      }
    }} onScroll={() => {
      const element = scrollRef.current;
      if (element) setEdges({ left: element.scrollLeft > 1, right: element.scrollLeft + element.clientWidth < element.scrollWidth - 1 });
    }} style={{ display: "flex", width: "100%", minWidth: 0, overflowX: "auto", overflowY: "hidden", scrollbarWidth: "none" }}>
      <div style={{ display: "flex", flex: "0 0 max-content", width: "max-content", marginLeft: side === "right" ? "auto" : undefined }}>{children}</div>
    </div>
    {edges.left && <span aria-hidden="true" style={{ pointerEvents: "none", position: "absolute", left: 0, top: 0, bottom: 0, width: 8, background: "linear-gradient(90deg, var(--wc-bg-deepest), transparent)" }} />}
    {edges.right && <span aria-hidden="true" style={{ pointerEvents: "none", position: "absolute", right: 0, top: 0, bottom: 0, width: 8, background: "linear-gradient(270deg, var(--wc-bg-deepest), transparent)" }} />}
  </div>;
}

export function StatusBar() {
  const { t } = useLocale();
  const cues = useWorkspaceStore((state) => state.cues);
  const brokenCueIds = useWorkspaceStore((state) => state.brokenCueIds);
  const cueCatalog = useWorkspaceStore((state) => state.cueCatalog);
  const cueLists = useWorkspaceStore((state) => state.cueLists);
  const activeCueListId = useWorkspaceStore((state) => state.activeCueListId);
  const rawPrefs = useWorkspaceStore((state) => state.displayPrefs.status_bar);
  const system = useStatusBarStore((state) => state.system);
  const runtime = useStatusBarStore((state) => state.runtime);
  const [now, setNow] = useState(() => Date.now());
  const [bands, setBands] = useState<Record<string, LoadBandState>>({});
  const previousVideoDrops = useRef<{ timestampMs: number; counts: Map<string, number> } | null>(null);
  const [lastVideoDropAt, setLastVideoDropAt] = useState<number | null>(null);
  const prefs = useMemo(() => normalizeStatusBarPreferences(rawPrefs ?? DEFAULT_STATUS_BAR_PREFERENCES), [rawPrefs]);
  const enabled = useMemo(() => [...prefs.left, ...prefs.right].filter((item) => item.enabled), [prefs]);
  const enabledKey = enabled.map((item) => item.id).join(",");
  const needsSystem = enabled.some((item) => SYSTEM_METRICS.has(item.id));
  const needsRuntime = enabled.some((item) => RUNTIME_METRICS.has(item.id));

  useEffect(() => {
    if (!prefs.visible || (!needsSystem && !needsRuntime)) return;
    return acquireStatusBarPolling({ system: needsSystem, runtime: needsRuntime });
  }, [prefs.visible, needsSystem, needsRuntime]);

  useEffect(() => {
    if (!prefs.visible) return;
    const timer = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(timer);
  }, [prefs.visible]);

  const cueStats = useMemo(() => buildCueStatusMetrics(cues, brokenCueIds), [cues, brokenCueIds]);
  const localActives = useMemo(() => flattenActiveCues(cues), [cues]);
  const projectActives = useMemo(() => flattenCatalogActiveCues(cueCatalog), [cueCatalog]);
  const activeLocal = new Map(localActives.map((cue) => [cue.id, cue])).size;
  const activeListName = cueLists.find((list) => list.id === activeCueListId)?.name ?? "";
  const uniqueProjectActives = useMemo(() => projectActives.length
    ? new Map(projectActives.map((cue) => [cue.id, cue])).size
    : activeLocal, [projectActives, activeLocal]);
  const chosen = useMemo(() => chosenAdapter(system?.gpuAdapters ?? [], prefs.gpu_adapter_id), [system?.gpuAdapters, prefs.gpu_adapter_id]);
  const systemStale = isStatusSampleStale(system?.timestampMs, now);
  const runtimeStale = isStatusSampleStale(runtime?.timestampMs, now);
  const networkStale = isStatusSampleStale(runtime?.networkTimestampMs, now);
  useEffect(() => {
    if (!runtime || runtimeStale) {
      previousVideoDrops.current = null;
      setLastVideoDropAt(null);
      return;
    }
    const counts = new Map((runtime.videoOutputs ?? []).flatMap((output) =>
      output.droppedFrames != null && Number.isFinite(output.droppedFrames) && output.droppedFrames >= 0
        ? [[output.id, output.droppedFrames] as const] : []));
    const previous = previousVideoDrops.current;
    if (previous && runtime.timestampMs > previous.timestampMs && runtime.timestampMs - previous.timestampMs <= 3000
      && [...counts].some(([id, count]) => count > (previous.counts.get(id) ?? count))) {
      setLastVideoDropAt(Date.now());
    }
    previousVideoDrops.current = { timestampMs: runtime.timestampMs, counts };
  }, [runtime, runtimeStale]);
  const fpsIssue = !runtimeStale && lastVideoDropAt != null && now - lastVideoDropAt <= 3000;

  useEffect(() => setBands({}), [chosen.adapter?.id, prefs.gpu_adapter_id]);

  useEffect(() => {
    if (!prefs.visible || systemStale) { setBands((previous) => Object.keys(previous).length ? {} : previous); return; }
    const values: Record<string, number | null> = {
      cpu: system?.systemCpuPercent ?? null,
      gpu: chosen.adapter?.usagePercent ?? null,
      ram: system?.systemRamUsedBytes != null && system.systemRamTotalBytes != null && system.systemRamTotalBytes > 0
        ? system.systemRamUsedBytes / system.systemRamTotalBytes * 100 : null,
      vram: chosen.adapter?.dedicatedUsedBytes != null && chosen.adapter.dedicatedTotalBytes != null && chosen.adapter.dedicatedTotalBytes > 0
        ? chosen.adapter.dedicatedUsedBytes / chosen.adapter.dedicatedTotalBytes * 100 : null,
    };
    setBands((previous) => {
      const next = { ...previous };
      for (const id of ["cpu", "gpu", "ram", "vram"] as const) {
        if (!enabled.some((item) => item.id === id)) continue;
        next[id] = updateLoadBand(id, values[id], now, previous[id], fpsIssue);
      }
      return next;
    });
  }, [system, chosen.adapter, now, fpsIssue, enabledKey, systemStale, prefs.visible]);

  if (!prefs.visible) return null;
  const context = {
    cues: cueStats, activeLocal, activeProject: uniqueProjectActives, activeListName,
    system, runtime, adapter: chosen.adapter, adapterFallback: chosen.fallback,
    systemStale, runtimeStale, networkStale,
    t,
  };
  const renderSide = (side: "left" | "right") => enabledOnSide(prefs, side).map((preference) => {
    const view = getMetricView(preference.id, context);
    if (["cpu", "gpu", "ram", "vram"].includes(preference.id) && bands[preference.id]) view.band = bands[preference.id].band;
    return <Metric key={preference.id} id={preference.id} view={view} t={t} />;
  });

  return <footer aria-label={t("statusBarUi.title")} style={{ height: 26, minHeight: 26, maxHeight: 26, display: "flex", alignItems: "stretch", overflow: "hidden", borderTop: "1px solid var(--wc-border)", background: "var(--wc-bg-deepest)", boxSizing: "border-box" }}>
    <MetricStrip side="left" label={t("statusBarUi.leftSide")} contentKey={enabledKey}>{renderSide("left")}</MetricStrip>
    <MetricStrip side="right" label={t("statusBarUi.rightSide")} contentKey={enabledKey}>{renderSide("right")}</MetricStrip>
  </footer>;
}
