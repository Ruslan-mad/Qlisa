import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getDiagnosticsSnapshot, resetDiagnosticsStatistics } from "../../lib/commands";
import type { DiagnosticsSnapshot } from "../../lib/types";
import { videoSummaryModel, type VideoCueDiagnostics, type VideoSummaryDiagnostics } from "./videoDiagnosticsModel";
import { networkBoolean, networkNumber, networkState, networkSummaryModel, networkText, type NetworkConnectionModel, type NetworkSummaryModel } from "./networkDiagnosticsModel";

type Tab = "overview" | "audio" | "video" | "network";
type Filter = "all" | "playing" | "paused" | "problems" | "completed";
type SortKey = "problems" | "name" | "state" | "buffer" | "underruns" | "wait";
type AnyRecord = Record<string, unknown>;

const nf = new Intl.NumberFormat("ru-RU");
const df = new Intl.NumberFormat("ru-RU", { minimumFractionDigits: 2, maximumFractionDigits: 2 });

const asRecord = (value: unknown): AnyRecord => value && typeof value === "object" && !Array.isArray(value) ? value as AnyRecord : {};
const num = (obj: AnyRecord, ...keys: string[]): number | null => {
  for (const key of keys) {
    const value = obj[key];
    if (typeof value === "number" && Number.isFinite(value)) return value;
  }
  return null;
};
const text = (obj: AnyRecord, ...keys: string[]): string | null => {
  for (const key of keys) {
    const value = obj[key];
    if (typeof value === "string" && value.trim()) return value;
  }
  return null;
};
const bool = (obj: AnyRecord, ...keys: string[]): boolean | null => {
  for (const key of keys) if (typeof obj[key] === "boolean") return obj[key] as boolean;
  return null;
};
const arr = (obj: AnyRecord, ...keys: string[]): unknown[] => {
  for (const key of keys) if (Array.isArray(obj[key])) return obj[key] as unknown[];
  return [];
};
const show = (value: number | null | undefined, suffix = "") => value == null ? "" : `${nf.format(value)}${suffix}`;

function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes)) return "";
  const value = Math.abs(bytes);
  if (value >= 1024 ** 3) return `${df.format(bytes / 1024 ** 3)} ГиБ`;
  if (value >= 1024 ** 2) return `${df.format(bytes / 1024 ** 2)} МиБ`;
  if (value >= 1024) return `${df.format(bytes / 1024)} КиБ`;
  return `${nf.format(Math.round(bytes))} Б`;
}

function formatSeconds(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds)) return "";
  return `${df.format(seconds)} сек`;
}

function formatMilliseconds(milliseconds: number | null | undefined): string {
  if (milliseconds == null || !Number.isFinite(milliseconds)) return "";
  return `${df.format(milliseconds)} мс`;
}

function formatVideoTime(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds)) return "";
  const whole = Math.max(0, Math.floor(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}

function translateState(value: unknown): { label: string; tone: "good" | "warn" | "bad" | "muted" } {
  const state = String(value ?? "").toLowerCase().replace(/[ _-]+/g, "");
  if (["playing", "воспроизводится", "running"].includes(state)) return { label: "Воспроизводится", tone: "good" };
  if (["paused", "пауза"].includes(state)) return { label: "Пауза", tone: "warn" };
  if (["ready", "готов"].includes(state)) return { label: "Готов", tone: "good" };
  if (["preload", "preloading", "initialbuffering", "buffering", "подготовказвука"].includes(state)) return { label: "Подготовка звука", tone: "warn" };
  if (["lowbuffer", "мало данных вбуфере", "starved"].includes(state)) return { label: "Мало данных в буфере", tone: "bad" };
  if (["refilling", "refill"].includes(state)) return { label: "Заполнение буфера", tone: "warn" };
  if (["seeking", "seek"].includes(state)) return { label: "Переход к позиции", tone: "warn" };
  if (["eof", "ended", "endoffile"].includes(state)) return { label: "Файл закончился", tone: "muted" };
  if (["completed", "complete", "завершено"].includes(state)) return { label: "Завершено", tone: "muted" };
  if (["error", "failed", "ошибка"].includes(state)) return { label: "Ошибка", tone: "bad" };
  if (["cancelled", "canceled", "отменено"].includes(state)) return { label: "Отменено", tone: "muted" };
  if (["idle", "ожидание"].includes(state)) return { label: "Ожидание", tone: "muted" };
  return value == null || String(value).trim() === "" ? { label: "Нет данных", tone: "muted" } : { label: "Состояние не определено", tone: "muted" };
}

function translateHealth(value: unknown): { label: string; tone: "good" | "warn" | "bad" | "muted" } {
  const state = String(value ?? "").toLowerCase();
  if (["ok", "healthy", "normal", "норма"].includes(state)) return { label: "Норма", tone: "good" };
  if (["warning", "warn", "предупреждение"].includes(state)) return { label: "Предупреждение", tone: "warn" };
  if (["error", "critical", "ошибка", "problem", "problems"].includes(state)) return { label: "Есть проблемы", tone: "bad" };
  return { label: "Нет данных", tone: "muted" };
}

function cleanDiagnosticText(value: unknown): string {
  if (typeof value !== "string" || !value.trim()) return "—";
  return value
    .replace(/initial starvation/gi, "ожидание данных при запуске")
    .replace(/silent frames?/gi, "кадры тишины")
    .replace(/underrun/gi, "пропуск звука")
    .replace(/low watermark/gi, "нижний порог буфера")
    .replace(/pending jobs?/gi, "задачи в очереди")
    .replace(/active workers?/gi, "работающие декодеры")
    .replace(/decoder session/gi, "сеанс декодирования")
    .replace(/\bplaying\b/gi, "воспроизведение")
    .replace(/\bpaused\b/gi, "пауза")
    .replace(/\bqueued\b/gi, "в очереди")
    .replace(/\brunning\b/gi, "работает");
}

function sourceModel(value: unknown, index: number) {
  const raw = asRecord(value);
  const buffer = num(raw, "bufferedSeconds", "bufferSeconds", "buffered_seconds");
  const underruns = num(raw, "underruns", "underRuns");
  const errors = num(raw, "decodeFailures", "decodeErrors", "errors");
  const problem = (buffer != null && buffer < 0.75) || (underruns ?? 0) > 0 || (errors ?? 0) > 0 || ["error", "low_buffer", "low buffer", "starvation"].includes(String(raw.state ?? "").toLowerCase());
  return {
    raw,
    id: text(raw, "sourceId", "id", "voiceId") ?? `source-${index}`,
    name: text(raw, "label", "name", "cueName", "cueLabel") ?? (text(raw, "path")?.split(/[\\/]/).pop() || `Источник ${index + 1}`),
    cueNumber: text(raw, "cueNumber", "number", "cue_number"),
    state: translateState(raw.state ?? raw.status),
    buffer,
    bufferBytes: num(raw, "bufferedBytes", "bufferBytes", "buffered_bytes"),
    capacityBytes: num(raw, "capacityBytes"),
    minBuffer: num(raw, "minimumBufferedSeconds", "minBufferedSeconds"),
    maxBuffer: num(raw, "maximumBufferedSeconds", "maxBufferedSeconds"),
    capacity: (() => { const frames = num(raw, "capacityFrames"); const rate = num(raw, "sampleRate"); return frames != null && rate ? frames / rate : num(raw, "capacitySeconds", "targetBufferSeconds"); })(),
    underruns, errors,
    silentFrames: num(raw, "silentFrames", "silent_frames"),
    silenceSeconds: num(raw, "silenceSeconds", "silentSeconds"),
    waitMs: (() => { const value = num(raw, "lastRefillWaitUs"); return value == null ? num(raw, "lastRefillWaitMs") : value / 1000; })(),
    maxWaitMs: (() => { const value = num(raw, "maximumRefillWaitUs"); return value == null ? num(raw, "maxRefillWaitMs") : value / 1000; })(),
    decodeMs: (() => { const value = num(raw, "lastDecodeUs"); return value == null ? num(raw, "lastDecodeMs") : value / 1000; })(),
    maxDecodeMs: (() => { const value = num(raw, "maximumDecodeUs"); return value == null ? num(raw, "maxDecodeMs") : value / 1000; })(),
    ready: bool(raw, "ready", "isReady"),
    decoding: bool(raw, "jobRunning", "workerActive", "decoding"),
    queued: bool(raw, "jobRequested", "queued", "waitingForDecoder"),
    eof: bool(raw, "eof", "endOfFile"),
    problem,
  };
}

const toneColor: Record<string, string> = { good: "#3dba6f", warn: "#eab308", bad: "#ef4444", muted: "var(--wc-text-secondary)" };

function Card({ title, value, hint, tone = "muted" }: { title: string; value: React.ReactNode; hint?: string; tone?: string }) {
  if (value == null || value === "" || value === "—" || value === "недоступно" || value === "Недоступно") return null;
  return <div title={hint} style={{ background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border)", borderRadius: 7, padding: "11px 13px", minWidth: 0, minHeight: 64, boxSizing: "border-box" }}>
    <div style={{ color: "var(--wc-text-secondary)", fontSize: 11, marginBottom: 6 }}>{title}{hint && <span style={{ marginLeft: 5, color: "var(--wc-text-muted)" }}>ⓘ</span>}</div>
    <div style={{ color: toneColor[tone] ?? "var(--wc-text-bright)", fontWeight: 600, fontSize: 17 }}>{value}</div>
  </div>;
}

function Section({ title, children, actions }: { title: string; children: React.ReactNode; actions?: React.ReactNode }) {
  return <section style={{ background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border)", borderRadius: 8, overflow: "hidden", flexShrink: 0, minWidth: 0 }}>
    <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "10px 13px", borderBottom: "1px solid var(--wc-border)" }}>
      <h2 style={{ margin: 0, color: "var(--wc-text-bright)", fontSize: 13, letterSpacing: ".04em", textTransform: "uppercase" }}>{title}</h2>
      {actions}
    </div>
    <div style={{ padding: 13, minWidth: 0 }}>{children}</div>
  </section>;
}

function MetricList({ items }: { items: Array<[string, React.ReactNode, string?]> }) {
  return <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(min(215px, 100%), 1fr))", gap: "7px 24px", minWidth: 0 }}>
    {items.filter(([, value]) => value != null && value !== "" && value !== "—" && value !== "недоступно" && value !== "Недоступно").map(([label, value, hint]) => <div key={label} title={hint} style={{ display: "flex", justifyContent: "space-between", gap: 12, borderBottom: "1px solid var(--wc-border)", padding: "5px 0", fontSize: 12 }}><span style={{ color: "var(--wc-text-secondary)" }}>{label}{hint && <span style={{ marginLeft: 4, color: "var(--wc-text-muted)" }}>ⓘ</span>}</span><strong style={{ color: "var(--wc-text-bright)", textAlign: "right" }}>{value}</strong></div>)}
  </div>;
}

function BufferBar({ source }: { source: ReturnType<typeof sourceModel> }) {
  if (source.buffer == null && source.capacity == null) return null;
  const max = source.capacity ?? 10;
  const value = Math.max(0, Math.min(max, source.buffer ?? 0));
  const percentage = max > 0 ? value / max * 100 : 0;
  const color = value < 0.75 ? "#ef4444" : value < 2 ? "#eab308" : "var(--wc-accent)";
  return <div title="Меньше 0,75 сек — критически мало; от 0,75 до 2 сек — предупреждение; выше 2 сек — норма">
    <div style={{ height: 9, background: "var(--wc-bg-input)", borderRadius: 5, overflow: "hidden", border: "1px solid var(--wc-border)" }}><div style={{ width: `${percentage}%`, height: "100%", background: color, transition: "width .2s" }} /></div>
    <div style={{ display: "flex", justifyContent: "space-between", color: "var(--wc-text-muted)", fontSize: 10, marginTop: 3 }}><span>{formatSeconds(source.buffer)}</span><span>цель {formatSeconds(source.capacity ?? 4)}</span></div>
  </div>;
}

function SourceDetail({ source }: { source: ReturnType<typeof sourceModel> }) {
  const raw = source.raw;
  return <div style={{ borderTop: "1px solid var(--wc-border)", paddingTop: 12, marginTop: 10 }}>
    <div style={{ fontSize: 14, color: "var(--wc-text-bright)", fontWeight: 600, marginBottom: 10 }}>{source.name}</div>
    <MetricList items={[
      ["Состояние", <span style={{ color: toneColor[source.state.tone] }}>{source.state.label}</span>],
      ["Файл", text(raw, "filePath", "path", "file") ?? "—"],
      ["Декодер", text(raw, "decoder", "decoderName") ?? "—"],
      ["Частота дискретизации", show(num(raw, "sampleRate", "sample_rate"), " Гц")],
      ["Каналы", show(num(raw, "channels"))],
      ["Текущий буфер", formatSeconds(source.buffer)],
      ["Объём данных в буфере", formatBytes(source.bufferBytes)],
      ["Ёмкость буфера", formatBytes(source.capacityBytes)],
      ["Минимальный буфер за сеанс", formatSeconds(source.minBuffer)],
      ["Максимальный буфер", formatSeconds(source.maxBuffer)],
      ["Готов к воспроизведению", source.ready == null ? "—" : source.ready ? "Да" : "Нет"],
      ["Сейчас декодируется", source.decoding == null ? "—" : source.decoding ? "Да" : "Нет"],
      ["Ожидает декодирования", source.queued == null ? "—" : source.queued ? "Да" : "Нет"],
      ["Конец файла достигнут", source.eof == null ? "—" : source.eof ? "Да" : "Нет"],
      ["Пропуски звука", show(source.underruns)],
      ["Кадры тишины", show(source.silentFrames)],
      ["Время тишины", formatSeconds(source.silenceSeconds)],
      ["Ошибки декодирования", show(source.errors)],
      ["Последнее ожидание декодера", source.waitMs == null ? "—" : `${df.format(source.waitMs)} мс`],
      ["Максимальное ожидание", source.maxWaitMs == null ? "—" : `${df.format(source.maxWaitMs)} мс`],
      ["Последнее декодирование", source.decodeMs == null ? "—" : `${df.format(source.decodeMs)} мс`],
      ["Максимальное время декодирования", source.maxDecodeMs == null ? "—" : `${df.format(source.maxDecodeMs)} мс`],
    ]} />
    {(source.buffer != null || source.capacity != null) && <><div style={{ marginTop: 13, color: "var(--wc-text-secondary)", fontSize: 11, marginBottom: 5 }}>Буфер</div>
    <BufferBar source={source} />
    <div style={{ color: "var(--wc-text-muted)", fontSize: 10, marginTop: 7 }}>Минимум для старта: 0,75 сек · нижний порог: 2 сек · максимум: {formatSeconds(source.capacity ?? 10)}</div></>}
  </div>;
}

export function DiagnosticsPage({ onClose }: { onClose?: () => void }) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const [tab, setTab] = useState<Tab>("overview");
  const [snapshot, setSnapshot] = useState<DiagnosticsSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const [sort, setSort] = useState<SortKey>("problems");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [networkFilter, setNetworkFilter] = useState<"all" | "incoming" | "outgoing" | "problems">("all");
  const [selectedNetworkId, setSelectedNetworkId] = useState<string | null>(null);
  const [resetting, setResetting] = useState(false);

  const refresh = useCallback(async () => {
    try { setSnapshot(await getDiagnosticsSnapshot()); setError(null); }
    catch (e) { setError(String(e)); }
  }, []);

  useEffect(() => {
    let disposed = false;
    const poll = async () => {
      if (disposed) return;
      // The pre-created native window remains mounted while hidden. Avoid IPC
      // polling in that state; the next visible poll refreshes immediately.
      if (!(await getCurrentWindow().isVisible()) || disposed) return;
      await refresh();
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 500);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [refresh]);

  // The page is mounted only in the diagnostics window. Keep the Escape
  // listener local to that lifetime so hiding and reopening stays clean.
  useEffect(() => {
    dialogRef.current?.focus();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose?.();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const audio = asRecord(snapshot?.audio);
  const system = asRecord(snapshot?.system);
  const video = videoSummaryModel(snapshot?.video);
  const network = networkSummaryModel(snapshot?.network);
  const scheduler = asRecord(audio.scheduler);
  const memory = asRecord(audio.memory);
  const counts = asRecord(audio.sourceCounts);
  const sources = useMemo(() => arr(audio, "sources").map(sourceModel), [audio.sources]);
  const health = translateHealth(audio.health);
  const activeWorkers = num(scheduler, "workersBusy", "activeWorkers", "busyWorkers");
  const workerTotal = num(scheduler, "workersTotal", "workerCount", "totalWorkers");
  const pendingJobs = num(scheduler, "queuedJobs", "pendingJobs", "queueLength");
  const selected = sources.find((source) => source.id === selectedId) ?? null;
  const sourceNames = useMemo(() => new Map(sources.map((source) => [source.id, source.name])), [sources]);
  const sortedSources = useMemo(() => {
    const filtered = sources.filter((source) => {
      if (filter === "all") return true;
      if (filter === "playing") return source.state.label === "Воспроизводится";
      if (filter === "paused") return source.state.label === "Пауза";
      if (filter === "completed") return ["Завершено", "Файл закончился"].includes(source.state.label);
      return source.problem;
    });
    return filtered.sort((a, b) => {
      if (sort === "problems") return Number(b.problem) - Number(a.problem) || (b.underruns ?? 0) - (a.underruns ?? 0);
      if (sort === "name") return a.name.localeCompare(b.name, "ru");
      if (sort === "state") return a.state.label.localeCompare(b.state.label, "ru");
      if (sort === "buffer") return (a.buffer ?? -1) - (b.buffer ?? -1);
      if (sort === "underruns") return (b.underruns ?? -1) - (a.underruns ?? -1);
      return (b.waitMs ?? -1) - (a.waitMs ?? -1);
    });
  }, [sources, filter, sort]);

  const reset = async () => {
    setResetting(true);
    try { await resetDiagnosticsStatistics(); await refresh(); }
    catch (e) { setError(String(e)); }
    finally { setResetting(false); }
  };

  const buttonStyle = (active = false): React.CSSProperties => ({ background: active ? "var(--wc-accent-dim)" : "var(--wc-bg-surface)", color: active ? "var(--wc-accent)" : "var(--wc-text-secondary)", border: `1px solid ${active ? "var(--wc-accent)" : "var(--wc-border-strong)"}`, borderRadius: 5, padding: "5px 10px", cursor: "pointer", fontSize: 12 });
  const eventList = arr(audio, "events").slice(-100).reverse();
  const metric = (key: string, ...aliases: string[]) => num(audio, key, ...aliases) ?? num(counts, key, ...aliases);
  const sourceCount = (key: string, ...aliases: string[]) => num(counts, key, ...aliases);
  const startWindowDrag = (event: React.MouseEvent<HTMLDivElement>) => {
    if (event.button !== 0 || (event.target as HTMLElement).closest("button")) return;
    event.preventDefault();
    void getCurrentWindow().startDragging();
  };

  return <div
    style={{
      display: "flex", flexDirection: "column", width: "100vw", height: "100vh",
      minWidth: 0, minHeight: 0, background: "var(--wc-bg-app)", color: "var(--wc-text)",
    }}
  >
    <div
      ref={dialogRef}
      role="main"
      aria-labelledby="diagnostics-dialog-title"
      tabIndex={-1}
      style={{
        display: "flex", flexDirection: "column",
        width: "100%", height: "100%",
        minWidth: 0, minHeight: 0,
        overflow: "hidden",
        outline: "none",
      }}
    >
    <div
      onMouseDown={startWindowDrag}
      style={{ display: "flex", alignItems: "center", gap: 12, padding: "9px 14px", borderBottom: "1px solid var(--wc-border)", background: "var(--wc-bg-surface)", flexShrink: 0, cursor: "default", userSelect: "none" }}
    >
      <div style={{ flex: 1 }}><div id="diagnostics-dialog-title" style={{ color: "var(--wc-text-bright)", fontSize: 15, fontWeight: 600 }}>Диагностика</div><div style={{ color: "var(--wc-text-muted)", fontSize: 11 }}>Обновление данных каждые 0,5 сек</div></div>
      <button onClick={() => void reset()} disabled={resetting} title="Сбрасывает счётчики и историю, но не меняет воспроизведение" style={buttonStyle()}>{resetting ? "Сброс…" : "Сбросить статистику"}</button>
      <button onClick={onClose} title="Закрыть диагностику (Esc)" aria-label="Закрыть диагностику" style={{ ...buttonStyle(), fontSize: 20, lineHeight: 1, padding: "1px 8px" }}>×</button>
    </div>
    <div style={{ display: "flex", gap: 5, padding: "9px 14px 0", flexShrink: 0 }}>
      <button onClick={() => setTab("overview")} style={buttonStyle(tab === "overview")}>Общее</button>
      <button onClick={() => setTab("audio")} style={buttonStyle(tab === "audio")}>Аудио</button>
      <button onClick={() => setTab("video")} style={buttonStyle(tab === "video")} title="Статистика VideoCue и контекстов mpv">Видео</button>
      <button onClick={() => setTab("network")} style={buttonStyle(tab === "network")} title="Диагностика сетевых входов и выходов">Сеть</button>
    </div>
    <div style={{ flex: "1 1 auto", minHeight: 0, minWidth: 0, overflowY: "auto", overflowX: "hidden", padding: 14, display: "flex", flexDirection: "column", gap: 13, boxSizing: "border-box" }}>
      {error && <div style={{ border: "1px solid #eab308", background: "rgba(234,179,8,.08)", color: "#fde68a", borderRadius: 6, padding: "8px 10px", fontSize: 12 }}>Не удалось получить диагностику. Повторная попытка выполняется автоматически.</div>}
      {tab === "overview" ? <>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(min(220px, 100%), 1fr))", gap: 9, minWidth: 0 }}>
          <Card title="Состояние аудио" value={health.label} tone={health.tone} hint="Оценка по фактическим событиям и состоянию источников." />
          <Card title="Рабочая память процесса" value={formatBytes(num(system, "processWorkingSetBytes"))} hint="Физическая память, которую сейчас занимает процесс Qlisa." />
          <Card title="Частная память процесса" value={formatBytes(num(system, "processPrivateBytes"))} hint="Память процесса, выделенная только для Qlisa." />
          <Card title="Память потокового аудио" value={formatBytes(num(memory, "pcmBufferedBytes", "usedBytes", "bufferedBytes"))} hint="Только PCM и кольцевые буферы потокового аудио." />
        </div>
        <Section title="Состояние системы"><MetricList items={[
          ["Активных аудиопотоков", show(sourceCount("total", "totalSources", "registeredSources"))],
          ["Воспроизводятся", show(sourceCount("playing", "playingSources"))],
          ["На паузе", show(sourceCount("paused", "pausedSources"))],
          ["Завершены", show(sourceCount("completed", "completedSources"))],
          ["Работающих декодеров", workerTotal == null ? "—" : `${show(activeWorkers)} из ${show(workerTotal)}`],
          ["Задач декодирования в очереди", show(pendingJobs)],
          ["Пропусков звука", show(metric("underruns", "totalUnderruns"), "")],
          ["Ошибок декодирования", show(metric("decodeFailures", "decodeErrors", "totalDecodeFailures"))],
        ]} /></Section>
        <NetworkOverview summary={network} />
      </> : tab === "audio" ? <>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(min(220px, 100%), 1fr))", gap: 9, minWidth: 0 }}>
          <Card title="Состояние аудио" value={health.label} tone={health.tone} />
          <Card title="Потоковых источников" value={show(sourceCount("total", "totalSources"))} />
          <Card title="Сейчас воспроизводятся" value={show(sourceCount("playing", "playingSources"))} />
          <Card title="Активных декодеров" value={workerTotal == null ? "—" : `${show(activeWorkers)} из ${show(workerTotal)}`} />
          <Card title="Задач в очереди" value={show(pendingJobs)} />
          <Card title="Всего пропусков звука" value={show(metric("underruns", "totalUnderruns"))} tone={(metric("underruns", "totalUnderruns") ?? 0) > 0 ? "bad" : "good"} />
        </div>
        <Section title="Планировщик декодирования"><MetricList items={[
          ["Декодеров всего", show(workerTotal)], ["Сейчас занято", show(activeWorkers)], ["Свободно", activeWorkers == null || workerTotal == null ? "—" : show(Math.max(0, workerTotal - activeWorkers))], ["Задач ожидает", show(pendingJobs)],
          ["Срочных задач", show(num(asRecord(scheduler.priority), "urgent"))], ["Обычных задач воспроизведения", show(num(asRecord(scheduler.priority), "playback"))], ["Предзагрузка", show(num(asRecord(scheduler.priority), "preload"))], ["На паузе", show(num(asRecord(scheduler.priority), "paused"))],
        ]} /></Section>
        <Section title="Память аудиодвижка"><MetricList items={[
          ["Общая ёмкость кольцевых буферов", formatBytes(num(memory, "ringCapacityBytes", "capacityBytes"))], ["Фактически заполнено PCM", formatBytes(num(memory, "pcmBufferedBytes", "usedBytes", "bufferedBytes"))], ["Источников", show(num(memory, "sourceCount", "sources"))], ["Кадров тишины", show(metric("silentFrames", "totalSilentFrames"))], ["Время тишины", formatSeconds(num(audio, "silenceSeconds", "totalSilenceSeconds"))], ["Ошибок декодирования", show(metric("decodeErrors", "decodeFailures", "totalDecodeFailures"))],
        ]} /></Section>
        <Peaks peaks={asRecord(audio.peaks)} />
        <Workers workers={arr(scheduler, "workers")} />
        <AudioSources sources={sortedSources} selected={selected} filter={filter} setFilter={setFilter} sort={sort} setSort={setSort} onSelect={(id) => setSelectedId(selectedId === id ? null : id)} />
        <Section title="Последние события"><Events events={eventList} networkEvents={network.events} networkNames={new Map(network.connections.map((connection) => [connection.id, connection.name]))} sourceNames={sourceNames} /></Section>
      </> : tab === "video" ? <VideoDiagnostics summary={video} system={system} /> : <NetworkDiagnostics summary={network} filter={networkFilter} setFilter={setNetworkFilter} selectedId={selectedNetworkId} onSelect={(id) => setSelectedNetworkId(selectedNetworkId === id ? null : id)} />}
    </div>
    </div>
  </div>;
}

function Peaks({ peaks }: { peaks: AnyRecord }) {
  const items: Array<[string, React.ReactNode]> = [
    ["Максимум одновременно воспроизводимых источников", show(num(peaks, "maximumPlayingSources", "maxPlayingSources"))],
    ["Максимум активных декодеров", show(num(peaks, "maximumWorkersBusy", "maxActiveWorkers"))],
    ["Максимум задач в очереди", show(num(peaks, "maximumQueuedJobs", "maxPendingJobs"))],
    ["Максимальная память потокового аудио", formatBytes(num(peaks, "maximumStreamingMemoryBytes", "maxMemoryBytes"))],
  ];
  if (items.every(([, value]) => value === "" || value == null)) return null;
  return <Section title="Пиковые значения с момента сброса"><MetricList items={items.map(([label, value]) => [label, value])} /></Section>;
}

function Workers({ workers }: { workers: unknown[] }) {
  if (workers.length === 0) return null;
  return <Section title="Декодеры"><div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(210px, 1fr))", gap: 7 }}>{workers.map((value, index) => { const worker = asRecord(value); const running = bool(worker, "busy", "active", "running"); const workerId = num(worker, "id", "index"); const workerName = text(worker, "sourceName", "cueName", "source"); const status = running == null ? translateState(worker.state).label : running ? "Работает" : "Свободен"; if (running == null && workerId == null && workerName == null) return null; return <div key={String(worker.id ?? index)} style={{ border: "1px solid var(--wc-border)", borderRadius: 5, padding: "8px 10px", display: "flex", justifyContent: "space-between", gap: 10 }}><span style={{ color: "var(--wc-text-secondary)", fontSize: 12 }}>{workerId != null ? `№${show(workerId)} ` : ""}{workerName ?? ""}</span><span style={{ color: running ? "var(--wc-accent)" : "var(--wc-text-muted)", fontSize: 12 }}>{status}</span></div>; })}</div></Section>;
}

function AudioSources({ sources, selected, filter, setFilter, sort, setSort, onSelect }: { sources: ReturnType<typeof sourceModel>[]; selected: ReturnType<typeof sourceModel> | null; filter: Filter; setFilter: (value: Filter) => void; sort: SortKey; setSort: (value: SortKey) => void; onSelect: (id: string) => void }) {
  const buttonStyle = (active = false): React.CSSProperties => ({ background: active ? "var(--wc-accent-dim)" : "transparent", color: active ? "var(--wc-accent)" : "var(--wc-text-secondary)", border: `1px solid ${active ? "var(--wc-accent)" : "var(--wc-border-strong)"}`, borderRadius: 4, padding: "4px 8px", cursor: "pointer", fontSize: 11 });
  const filters: Array<[Filter, string]> = [["all", "Все"], ["playing", "Воспроизводятся"], ["paused", "Пауза"], ["problems", "Есть проблемы"], ["completed", "Завершены"]];
  const hasBuffer = sources.some((source) => source.buffer != null || source.capacity != null);
  const hasMinBuffer = sources.some((source) => source.minBuffer != null);
  const hasUnderruns = sources.some((source) => source.underruns != null);
  const hasErrors = sources.some((source) => source.errors != null);
  const columnCount = 2 + Number(hasBuffer) + Number(hasMinBuffer) + Number(hasUnderruns) + Number(hasErrors);
  return <Section title="Источники потокового аудио" actions={<div style={{ display: "flex", gap: 4, flexWrap: "wrap", justifyContent: "flex-end" }}>{filters.map(([key, label]) => <button key={key} onClick={() => setFilter(key)} style={buttonStyle(filter === key)}>{label}</button>)}<select value={sort} onChange={(e) => setSort(e.target.value as SortKey)} title="Порядок строк" style={{ ...buttonStyle(), background: "var(--wc-bg-input)" }}><option value="problems">Проблемы сверху</option><option value="name">По названию</option><option value="state">По состоянию</option><option value="buffer">По буферу</option><option value="underruns">По пропускам</option><option value="wait">По ожиданию</option></select></div>}>
    <div style={{ maxWidth: "100%", overflowX: "auto", overflowY: "hidden" }}><table style={{ width: "100%", borderCollapse: "collapse", fontSize: 12, minWidth: 500 }}><thead><tr>{["Источник", "Состояние", ...(hasBuffer ? ["Буфер"] : []), ...(hasMinBuffer ? ["Мин. буфер"] : []), ...(hasUnderruns ? ["Пропуски"] : []), ...(hasErrors ? ["Ошибки"] : [])].map((title) => <th key={title} style={{ textAlign: title === "Источник" ? "left" : "right", color: "var(--wc-text-muted)", fontWeight: 500, padding: "0 7px 7px", whiteSpace: "nowrap" }}>{title}</th>)}</tr></thead><tbody>{sources.map((source) => <tr key={source.id} onClick={() => onSelect(source.id)} style={{ cursor: "pointer", background: selected?.id === source.id ? "var(--wc-bg-selected)" : "transparent" }}><td style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", color: "var(--wc-text-bright)", maxWidth: 280, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={source.name}>{source.cueNumber ? `${source.cueNumber} · ` : ""}{source.name}</td><td style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", textAlign: "right", color: toneColor[source.state.tone], whiteSpace: "nowrap" }}>{source.state.label}</td>{hasBuffer && <td style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", width: 180 }}><BufferBar source={source} /></td>}{hasMinBuffer && <td style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", textAlign: "right" }}>{formatSeconds(source.minBuffer)}</td>}{hasUnderruns && <td style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", textAlign: "right", color: (source.underruns ?? 0) > 0 ? "#ef4444" : "var(--wc-text)" }}>{show(source.underruns)}</td>}{hasErrors && <td style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", textAlign: "right", color: (source.errors ?? 0) > 0 ? "#ef4444" : "var(--wc-text)" }}>{show(source.errors)}</td>}</tr>)}{sources.length === 0 && <tr><td colSpan={columnCount} style={{ color: "var(--wc-text-muted)", textAlign: "center", padding: 20 }}>Нет источников потокового аудио</td></tr>}</tbody></table></div>
    {selected && <SourceDetail source={selected} />}
  </Section>;
}

function formatBitrate(value: number | null): string {
  if (value == null || !Number.isFinite(value)) return "";
  const bits = value > 100_000 ? value : value * 1000;
  if (bits >= 1_000_000) return `${df.format(bits / 1_000_000)} Мбит/с`;
  if (bits >= 1_000) return `${df.format(bits / 1_000)} кбит/с`;
  return `${nf.format(Math.round(bits))} бит/с`;
}

function formatDateTime(value: unknown): string | null {
  if (typeof value === "number" && Number.isFinite(value)) {
    const timestamp = value < 10_000_000_000 ? value * 1000 : value;
    return new Date(timestamp).toLocaleTimeString("ru-RU");
  }
  return typeof value === "string" && value.trim() ? value : null;
}

function networkDirectionLabel(value: NetworkConnectionModel["direction"]): string | null {
  return value === "incoming" ? "Входящий" : value === "outgoing" ? "Исходящий" : null;
}

function NetworkOverview({ summary }: { summary: NetworkSummaryModel }) {
  const items: Array<[string, React.ReactNode, string?]> = [];
  const add = (label: string, value: number | null, tooltip?: string) => {
    if (value != null) items.push([label, show(value), tooltip]);
  };
  add("Активных подключений", summary.active);
  add("Входящих потоков", summary.incoming);
  add("Исходящих потоков", summary.outgoing);
  add("Подключено", summary.connected);
  add("Переподключение", summary.reconnecting);
  add("Ошибок", summary.errors);
  if (summary.incomingBitrate != null) items.push(["Входящий поток", formatBitrate(summary.incomingBitrate), "Количество данных, принимаемых сетевыми потоками за одну секунду."]);
  if (summary.outgoingBitrate != null) items.push(["Исходящий поток", formatBitrate(summary.outgoingBitrate), "Количество данных, передаваемых сетевыми потоками за одну секунду."]);
  return <Section title="Сеть"><MetricList items={items} /></Section>;
}

function NetworkDetail({ connection }: { connection: NetworkConnectionModel }) {
  const raw = connection.raw;
  const ffmpegRunning = networkBoolean(raw, ["ffmpegRunning", "processRunning"]);
  const endpoint = networkText(raw, ["endpoint"]);
  const srtMode = networkText(raw, ["srtMode"]);
  const latency = networkNumber(raw, ["latencyMs"]);
  const items: Array<[string, React.ReactNode, string?]> = [
    ["Тип", connection.protocol], ["Направление", networkDirectionLabel(connection.direction)], ["Состояние", <span style={{ color: toneColor[connection.state.tone] }}>{connection.state.label}</span>],
  ];
  if (endpoint) items.push(["Адрес", endpoint]);
  if (srtMode) items.push(["Режим SRT", translateSrtMode(srtMode)]);
  if (latency != null) items.push(["Задержка SRT", `${df.format(latency)} мс`, "Буфер задержки SRT для компенсации проблем сети."]);
  const addNumber = (label: string, keys: string[], tooltip?: string) => {
    const value = networkNumber(raw, keys);
    if (value != null) items.push([label, show(value), tooltip]);
  };
  addNumber("Получено кадров", ["receivedFrames"]);
  addNumber("Отправлено кадров", ["submittedFrames"]);
  addNumber("Пропущено кадров", ["droppedFrames"]);
  addNumber("Кадры, заменённые в буфере", ["supersededFrames"]);
  addNumber("Получено аудиосэмплов", ["receivedAudioSamples"]);
  addNumber("Пропущено аудиосэмплов", ["droppedAudioSamples"]);
  if (ffmpegRunning != null) items.push(["FFmpeg", ffmpegRunning ? "Работает" : "Не запущен"]);
  addNumber("PID FFmpeg", ["ffmpegPid"]);
  const lastError = networkText(raw, ["lastError"]);
  const lastWarning = networkText(raw, ["lastWarning"]);
  if (lastError) items.push(["Последняя ошибка", lastError]);
  if (lastWarning) items.push(["Последнее предупреждение", lastWarning]);
  return <div style={{ borderTop: "1px solid var(--wc-border)", marginTop: 10, paddingTop: 12 }}>
    <div style={{ color: "var(--wc-text-bright)", fontSize: 14, fontWeight: 600, marginBottom: 9 }}>{connection.name}</div>
    <MetricList items={items} />
  </div>;
}

function translateSrtMode(value: string | null): string {
  if (!value) return "";
  const key = value.toLowerCase();
  if (key === "listener") return "Слушатель";
  if (key === "caller") return "Инициатор";
  if (key === "rendezvous") return "Встречное соединение";
  return value;
}

function NetworkEvents({ events, connectionNames }: { events: unknown[]; connectionNames: Map<string, string> }) {
  if (events.length === 0) return <div style={{ color: "var(--wc-text-muted)", fontSize: 12 }}>Сетевых событий пока нет</div>;
  return <div style={{ display: "flex", flexDirection: "column", gap: 5, maxHeight: 250, overflow: "auto" }}>{events.slice(-100).reverse().map((event, index) => {
    const item = event && typeof event === "object" ? event as Record<string, unknown> : {};
    const state = networkState(item.kind ?? item.state ?? item.status);
    const detail = translateNetworkEventDetail(item, networkText(item, ["detail", "message", "description", "error"]) ?? state.label);
    return <div key={`${String(item.atMs ?? item.timestamp ?? index)}-${index}`} style={{ display: "grid", gridTemplateColumns: "75px minmax(100px, 190px) 1fr", gap: 9, padding: "5px 0", borderBottom: "1px solid var(--wc-border)", fontSize: 11 }}>
      <span style={{ color: "var(--wc-text-muted)" }}>{formatDateTime(item.atMs ?? item.timestamp)}</span><span style={{ color: toneColor[state.tone] }}>{connectionNames.get(String(item.connectionId ?? "")) ?? networkText(item, ["sourceName", "name", "label"]) ?? "Сеть"}</span><span style={{ color: "var(--wc-text-secondary)" }}>{detail}</span>
    </div>;
  })}</div>;
}

function translateNetworkEventDetail(item: AnyRecord, detail: string): string {
  const stateLabel = (value: string): string => {
    const translated = networkState(value).label;
    return translated.toLowerCase() === value.trim().toLowerCase() ? value : translated;
  };
  const raw = detail.trim();
  if (!raw || raw === "—") return networkState(item.kind ?? item.state ?? item.status).label;
  if (raw.includes("→") || raw.includes("->")) {
    return raw.split(/(→|->)/).map((part) => part === "→" || part === "->" ? part : stateLabel(part)).join("");
  }
  return stateLabel(raw);
}

function NetworkDiagnostics({ summary, filter, setFilter, selectedId, onSelect }: { summary: NetworkSummaryModel; filter: "all" | "incoming" | "outgoing" | "problems"; setFilter: (value: "all" | "incoming" | "outgoing" | "problems") => void; selectedId: string | null; onSelect: (id: string) => void }) {
  const filtered = summary.connections.filter((connection) => filter === "all" || filter === "problems" && connection.problem || filter === connection.direction);
  const buttonStyle = (active = false): React.CSSProperties => ({ background: active ? "var(--wc-accent-dim)" : "transparent", color: active ? "var(--wc-accent)" : "var(--wc-text-secondary)", border: `1px solid ${active ? "var(--wc-accent)" : "var(--wc-border-strong)"}`, borderRadius: 4, padding: "4px 8px", cursor: "pointer", fontSize: 11 });
  const selected = summary.connections.find((connection) => connection.id === selectedId) ?? null;
  const hasBitrate = summary.connections.some((connection) => networkNumber(connection.raw, ["bitrateKbps"]) != null);
  return <>
    <NetworkOverview summary={summary} />
    <Section title="Сетевые потоки" actions={<div style={{ display: "flex", gap: 4, flexWrap: "wrap", justifyContent: "flex-end" }}>{(["all", "incoming", "outgoing", "problems"] as const).map((key) => <button key={key} onClick={() => setFilter(key)} style={buttonStyle(filter === key)}>{key === "all" ? "Все" : key === "incoming" ? "Входящие" : key === "outgoing" ? "Исходящие" : "Есть проблемы"}</button>)}</div>}>
      <div style={{ maxWidth: "100%", overflowX: "auto" }}><table style={{ width: "100%", borderCollapse: "collapse", fontSize: 12, minWidth: 680 }}><thead><tr>{["Поток", "Тип", "Направление", "Состояние", ...(hasBitrate ? ["Битрейт"] : []), "Ошибки"].map((title) => <th key={title} style={{ textAlign: title === "Поток" ? "left" : "right", color: "var(--wc-text-muted)", fontWeight: 500, padding: "0 7px 7px", whiteSpace: "nowrap" }}>{title}</th>)}</tr></thead><tbody>{filtered.map((connection) => <tr key={connection.id} onClick={() => onSelect(connection.id)} style={{ cursor: "pointer", background: selected?.id === connection.id ? "var(--wc-bg-selected)" : "transparent" }}>
        <td style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", color: "var(--wc-text-bright)", maxWidth: 260, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={connection.name}>{connection.name}</td>
        <td style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", textAlign: "right" }}>{connection.protocol}</td><td style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", textAlign: "right" }}>{networkDirectionLabel(connection.direction)}</td><td style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", textAlign: "right", color: toneColor[connection.state.tone], whiteSpace: "nowrap" }}>{connection.state.label}</td>
        {hasBitrate && <td title="Количество данных, передаваемых потоком за одну секунду." style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", textAlign: "right" }}>{formatBitrate(networkNumber(connection.raw, ["bitrateKbps"]))}</td>}<td style={{ padding: "7px", borderTop: "1px solid var(--wc-border)", textAlign: "right", color: connection.problem ? "#ef4444" : "var(--wc-text)" }}>{connection.problem ? "Есть" : "Нет"}</td>
      </tr>)}{filtered.length === 0 && <tr><td colSpan={hasBitrate ? 6 : 5} style={{ color: "var(--wc-text-muted)", textAlign: "center", padding: 20 }}>Сетевых потоков нет</td></tr>}</tbody></table></div>
      {selected && <NetworkDetail connection={selected} />}
    </Section>
    <Section title="Последние сетевые события"><NetworkEvents events={summary.events} connectionNames={new Map(summary.connections.map((connection) => [connection.id, connection.name]))} /></Section>
  </>;
}

function translateVideoState(cue: VideoCueDiagnostics): { label: string; tone: "good" | "warn" | "bad" | "muted" } {
  const state = String(cue.state ?? "").toLowerCase().replace(/[ _-]+/g, "");
  if (cue.eof || ["eof", "ended", "complete", "completed"].includes(state)) return { label: "Конец файла", tone: "muted" };
  if (cue.playing || ["playing", "running", "воспроизводится"].includes(state)) return { label: "Воспроизводится", tone: "good" };
  if (cue.paused || ["paused", "pause", "пауза"].includes(state)) return { label: "Пауза", tone: "warn" };
  if (cue.preload || ["preload", "preloading", "buffering", "готовится"].includes(state)) return { label: "Предзагрузка", tone: "warn" };
  if (["error", "failed", "ошибка"].includes(state)) return { label: "Ошибка", tone: "bad" };
  return translateState(cue.state);
}

function videoPercent(value: number | null): string {
  return value == null ? "" : `${df.format(value)} %`;
}

function VideoCueCard({ cue }: { cue: VideoCueDiagnostics }) {
  const state = translateVideoState(cue);
  const duplicated = (cue.outputs ?? 0) > 1 || (cue.mpvContexts ?? 0) > 1;
  const desyncProblem = (cue.desyncMs ?? 0) > 5;
  const displayName = cue.cueNumber ? `${cue.cueNumber} · ${cue.name}` : cue.name;
  return <div style={{ border: `1px solid ${duplicated || desyncProblem ? "rgba(234,179,8,.7)" : "var(--wc-border)"}`, borderRadius: 7, padding: 12, background: duplicated || desyncProblem ? "rgba(234,179,8,.04)" : "transparent" }}>
    <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", gap: 10, flexWrap: "wrap", marginBottom: 9 }}>
      <div style={{ color: "var(--wc-text-bright)", fontSize: 14, fontWeight: 600, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={cue.file ?? undefined}>{displayName}</div>
      <span style={{ color: toneColor[state.tone], whiteSpace: "nowrap", fontSize: 12 }}>{state.label}</span>
    </div>
    {(duplicated || desyncProblem) && <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 9, color: "#fde68a", fontSize: 11 }}>
      {duplicated && <span>Выходов: {show(cue.outputs)} · Декодеров: {show(cue.mpvContexts)}</span>}
      {desyncProblem && <span>Рассинхрон выходов: {formatMilliseconds(cue.desyncMs)}</span>}
    </div>}
    <MetricList items={[
      ["Файл", cue.file ?? "недоступно"],
      ["Разрешение", cue.resolution ?? "недоступно"],
      ["FPS", cue.fps == null ? "недоступно" : df.format(cue.fps)],
      ["Выходов", show(cue.outputs)],
      ["Создано контекстов mpv", show(cue.mpvContexts)],
      ["Контекстов mpv для отрисовки", show(cue.renderContexts)],
      ["Аппаратное декодирование", cue.hardwareDecode == null ? "недоступно" : cue.hardwareDecode ? `Да${cue.hardwareBackend ? ` · ${cue.hardwareBackend}` : ""}` : "Нет"],
      ["Формат декодера", cue.decoderFormat ?? "недоступно"],
      ["Пропущено кадров", show(cue.droppedFrames)],
      ["Задержано кадров", show(cue.delayedFrames)],
      ["Текущий time-pos", formatVideoTime(cue.timePosSeconds)],
      ["Рассинхрон выходов", formatMilliseconds(cue.desyncMs)],
      ["Вызовов render", show(cue.renderCalls)],
      ["Среднее время render", formatMilliseconds(cue.averageRenderMs)],
      ["Максимальное время render", formatMilliseconds(cue.maximumRenderMs)],
      ["Предзагрузка", cue.preload == null ? "недоступно" : cue.preload ? "Да" : "Нет"],
      ["Воспроизведение", cue.playing == null ? "недоступно" : cue.playing ? "Да" : "Нет"],
      ["Пауза", cue.paused == null ? "недоступно" : cue.paused ? "Да" : "Нет"],
      ["Конец файла", cue.eof == null ? "недоступно" : cue.eof ? "Да" : "Нет"],
    ]} />
  </div>;
}

function VideoDiagnostics({ summary, system }: { summary: VideoSummaryDiagnostics; system: AnyRecord }) {
  const videoSystem = Object.keys(summary.system).length > 0 ? summary.system : system;
  const processCpu = num(videoSystem, "processCpuPercent", "cpuPercent", "cpuUsage", "process_cpu_percent");
  const processRam = num(videoSystem, "processWorkingSetBytes", "processRamBytes", "ramBytes", "process_memory_bytes");
  const gpu = num(videoSystem, "gpuUsagePercent", "gpuPercent", "gpu_usage_percent");
  const gpuDecode = num(videoSystem, "gpuVideoDecodePercent", "videoDecodeUsagePercent", "gpuVideoDecodeUsage", "gpu_video_decode_percent");
  const vram = num(videoSystem, "vramBytes", "gpuMemoryBytes", "gpu_memory_bytes");
  const systemItems: Array<[string, React.ReactNode]> = [
    ["CPU процесса", videoPercent(processCpu)], ["RAM процесса", formatBytes(processRam)],
    ["Использование GPU", videoPercent(gpu)], ["GPU Video Decode", videoPercent(gpuDecode)], ["VRAM", formatBytes(vram)],
  ];
  const hasSystemMetrics = systemItems.some(([, value]) => value !== "" && value != null);
  return <>
    <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(min(190px, 100%), 1fr))", gap: 9, minWidth: 0 }}>
      <Card title="Активных VideoCue" value={show(summary.activeCues)} hint="VideoCue, которые сейчас зарегистрированы как активные." />
      <Card title="Активных видеодекодеров" value={show(summary.activeDecoders)} hint="Число декодеров, сообщённое видеодвижком." />
      <Card title="Контекстов mpv" value={show(summary.mpvContexts)} hint="Созданные контексты mpv для текущих VideoCue." />
      <Card title="Выходов" value={show(summary.outputs)} hint="Активные выходы, на которые направлено видео." />
      <Card title="Пропущено кадров" value={show(summary.droppedFrames)} tone={(summary.droppedFrames ?? 0) > 0 ? "bad" : "good"} />
      <Card title="VideoCue с повторным декодированием" value={show(summary.multiDecodeCues)} tone={(summary.multiDecodeCues ?? 0) > 0 ? "warn" : "good"} hint="VideoCue, для которых создано более одного независимого mpv context." />
    </div>
    {hasSystemMetrics && <Section title="Системная статистика видео" actions={<span title="GPU-метрики показываются только при достоверных данных от платформы" style={{ color: "var(--wc-text-muted)", fontSize: 11 }}>только доступные данные</span>}><MetricList items={systemItems} /></Section>}
    <Section title="VideoCue">
      {summary.cues.length === 0 ? <div style={{ color: "var(--wc-text-muted)", fontSize: 12 }}>Активных VideoCue нет или видеопайплайн пока не передал данные.</div> : <div style={{ display: "flex", flexDirection: "column", gap: 9 }}>{summary.cues.map((cue) => <VideoCueCard key={cue.id} cue={cue} />)}</div>}
    </Section>
  </>;
}

function Events({ events, networkEvents = [], networkNames = new Map(), sourceNames }: { events: unknown[]; networkEvents?: unknown[]; networkNames?: Map<string, string>; sourceNames: Map<string, string> }) {
  const merged = [...events.map((event) => ({ event, network: false })), ...networkEvents.map((event) => ({ event, network: true }))]
    .sort((a, b) => (num(asRecord(b.event), "atMs") ?? 0) - (num(asRecord(a.event), "atMs") ?? 0))
    .slice(0, 100);
  if (merged.length === 0) return <div style={{ color: "var(--wc-text-muted)", fontSize: 12 }}>Событий пока нет</div>;
  return <div style={{ display: "flex", flexDirection: "column", gap: 5, maxHeight: 250, overflow: "auto" }}>{merged.map(({ event, network }, index) => { const item = asRecord(event); const rawDetail = networkText(item, ["detail", "message", "description"]); const detail = network ? translateNetworkEventDetail(item, rawDetail ?? "") : cleanDiagnosticText(rawDetail); const kind = String(item.kind ?? "").toLowerCase(); const networkStateValue = network ? networkState(item.kind ?? item.state ?? item.status) : null; const tone = network ? networkStateValue?.tone ?? "muted" : kind.includes("error") ? "bad" : kind.includes("underrun") ? "warn" : "muted"; const atMs = num(item, "atMs"); const sourceId = text(item, "sourceId"); const label = network ? networkNames.get(String(item.connectionId ?? "")) ?? networkText(item, ["sourceName", "connectionName", "name", "label"]) ?? "Сеть" : sourceNames.get(sourceId ?? "") ?? "Аудиодвижок"; const fallback = network ? networkStateValue?.label ?? "Сетевое событие" : kind.includes("error") ? "Ошибка декодирования" : kind.includes("underrun") ? "Пропуск звука" : "Событие аудиодвижка"; return <div key={`${network ? "network" : "audio"}-${String(atMs ?? index)}-${index}`} style={{ display: "grid", gridTemplateColumns: "75px minmax(100px, 190px) 1fr", gap: 9, padding: "5px 0", borderBottom: "1px solid var(--wc-border)", fontSize: 11 }}><span style={{ color: "var(--wc-text-muted)" }}>{atMs == null ? "—" : new Date(atMs).toLocaleTimeString("ru-RU")}</span><span style={{ color: toneColor[tone] }}>{label}</span><span style={{ color: "var(--wc-text-secondary)" }}>{detail === "—" ? fallback : detail}</span></div>; })}</div>;
}
