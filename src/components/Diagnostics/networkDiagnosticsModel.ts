export type NetworkDirection = "incoming" | "outgoing";

export type NetworkTone = "good" | "warn" | "bad" | "muted";

export interface NetworkConnectionModel {
  id: string;
  name: string;
  protocol: string;
  direction: NetworkDirection | null;
  state: { label: string; tone: NetworkTone };
  problem: boolean;
  raw: Record<string, unknown>;
}

export interface NetworkSummaryModel {
  active: number | null;
  incoming: number | null;
  outgoing: number | null;
  connected: number | null;
  reconnecting: number | null;
  errors: number | null;
  incomingBitrate: number | null;
  outgoingBitrate: number | null;
  connections: NetworkConnectionModel[];
  events: unknown[];
}

type RecordValue = Record<string, unknown>;

const record = (value: unknown): RecordValue =>
  value && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : {};

const first = (value: RecordValue, keys: string[]): unknown => {
  for (const key of keys) {
    if (value[key] !== undefined && value[key] !== null) return value[key];
  }
  return null;
};

const text = (value: RecordValue, keys: string[]): string | null => {
  const candidate = first(value, keys);
  return typeof candidate === "string" && candidate.trim() ? candidate : null;
};

const number = (value: RecordValue, keys: string[]): number | null => {
  const candidate = first(value, keys);
  return typeof candidate === "number" && Number.isFinite(candidate) ? candidate : null;
};

const boolean = (value: RecordValue, keys: string[]): boolean | null => {
  const candidate = first(value, keys);
  return typeof candidate === "boolean" ? candidate : null;
};

const array = (value: RecordValue, keys: string[]): unknown[] => {
  const candidate = first(value, keys);
  return Array.isArray(candidate) ? candidate : [];
};

export const networkText = (value: unknown, keys: string[]): string | null => text(record(value), keys);
export const networkNumber = (value: unknown, keys: string[]): number | null => number(record(value), keys);
export const networkBoolean = (value: unknown, keys: string[]): boolean | null => boolean(record(value), keys);

export function networkState(value: unknown): { label: string; tone: NetworkTone } {
  const key = String(value ?? "").toLowerCase().replace(/[ _-]+/g, "");
  if (["connected", "connectedok", "streaming", "running", "receiving", "sending"].includes(key)) return { label: "Подключено", tone: "good" };
  if (["connecting", "starting", "waitingforframe", "waiting"].includes(key)) return { label: key === "starting" ? "Запуск" : "Подключение", tone: "warn" };
  if (["reconnecting", "retrying", "retry"].includes(key)) return { label: "Переподключение", tone: "warn" };
  if (["stopping"].includes(key)) return { label: "Остановка", tone: "muted" };
  if (["disconnected", "stopped", "disabled"].includes(key)) return { label: "Отключено", tone: "muted" };
  if (["failed", "error", "failure"].includes(key)) return { label: "Ошибка", tone: "bad" };
  if (["idle", "none", ""].includes(key)) return { label: "Ожидание", tone: "muted" };
  return { label: typeof value === "string" ? value : "Недоступно", tone: "muted" };
}

function protocolLabel(value: unknown): string {
  if (Array.isArray(value)) {
    const labels = value.map(protocolLabel).filter((item) => item !== "Недоступно");
    return labels.length ? labels.join(" / ") : "Недоступно";
  }
  const key = String(value ?? "").toLowerCase();
  if (key === "srt") return "SRT";
  if (key === "ndi") return "NDI";
  if (key === "ffmpeg") return "FFmpeg";
  return typeof value === "string" && value.trim() ? value : "Недоступно";
}

function direction(value: unknown): NetworkDirection | null {
  const key = String(value ?? "").toLowerCase();
  if (["input", "incoming", "in", "receive", "receiver", "входящий"].includes(key)) return "incoming";
  if (["output", "outgoing", "out", "send", "sender", "исходящий"].includes(key)) return "outgoing";
  return null;
}

function hasProblem(raw: RecordValue, state: { label: string; tone: NetworkTone }): boolean {
  if (state.tone === "bad" || state.label === "Переподключение" || state.label === "Отключено") return true;
  if (boolean(raw, ["problem", "hasProblem", "unhealthy"]) === true) return true;
  if ((number(raw, ["errors", "errorCount", "errorsCount"]) ?? 0) > 0) return true;
  if ((number(raw, ["droppedFrames", "dropped_frames", "packetsLost", "lostPackets"]) ?? 0) > 0) return true;
  return boolean(raw, ["ffmpegRunning", "processRunning"]) === false;
}

export function networkSummaryModel(value: unknown): NetworkSummaryModel {
  const root = record(value);
  const rows = array(root, ["connections", "streams", "sources", "outputs", "inputs"]);
  const connections = rows.map((item) => {
    const raw = record(item);
    const baseState = networkState(first(raw, ["state", "status", "connectionState", "runtimeState"]));
    // FFmpeg stderr is a warning while the receiver is healthy. Keep the
    // connected label, but make the non-fatal condition visible in yellow.
    const state = networkText(raw, ["lastWarning", "warning"]) && baseState.tone === "good"
      ? { label: "Подключено", tone: "warn" as NetworkTone }
      : baseState;
    return {
      id: text(raw, ["id"]) ?? "",
      name: text(raw, ["name"]) ?? "",
      protocol: protocolLabel(first(raw, ["protocols"])),
      direction: direction(raw.direction),
      state,
      problem: hasProblem(raw, state),
      raw,
    };
  });
  const active = number(root, ["activeConnections"]);
  const incoming = number(root, ["incomingStreams"]);
  const outgoing = number(root, ["outgoingStreams"]);
  const connected = number(root, ["connected"]);
  const reconnecting = number(root, ["reconnecting"]);
  const errors = number(root, ["errors"]);
  return {
    active,
    incoming,
    outgoing,
    connected,
    reconnecting,
    errors,
    incomingBitrate: number(root, ["incomingBitrateKbps"]),
    outgoingBitrate: number(root, ["outgoingBitrateKbps"]),
    connections,
    events: array(root, ["events", "history", "networkEvents"]),
  };
}
