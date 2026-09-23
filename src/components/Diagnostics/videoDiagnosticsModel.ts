type RecordValue = Record<string, unknown>;

const record = (value: unknown): RecordValue => value && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : {};

const number = (value: RecordValue, ...keys: string[]): number | null => {
  for (const key of keys) {
    const candidate = value[key];
    if (typeof candidate === "number" && Number.isFinite(candidate)) return candidate;
  }
  return null;
};

const string = (value: RecordValue, ...keys: string[]): string | null => {
  for (const key of keys) {
    const candidate = value[key];
    if (typeof candidate === "string" && candidate.trim()) return candidate.trim();
  }
  return null;
};

const boolean = (value: RecordValue, ...keys: string[]): boolean | null => {
  for (const key of keys) if (typeof value[key] === "boolean") return value[key] as boolean;
  return null;
};

const list = (value: RecordValue, ...keys: string[]): unknown[] => {
  for (const key of keys) if (Array.isArray(value[key])) return value[key] as unknown[];
  return [];
};

const count = (value: RecordValue, ...keys: string[]): number | null => {
  const scalar = number(value, ...keys);
  if (scalar != null) return scalar;
  for (const key of keys) if (Array.isArray(value[key])) return (value[key] as unknown[]).length;
  return null;
};

export type VideoStatusTone = "good" | "warn" | "bad" | "muted";

export interface VideoCueDiagnostics {
  raw: RecordValue;
  id: string;
  name: string;
  cueNumber: string | null;
  state: string | null;
  file: string | null;
  resolution: string | null;
  fps: number | null;
  outputs: number | null;
  mpvContexts: number | null;
  renderContexts: number | null;
  hardwareDecode: boolean | null;
  hardwareBackend: string | null;
  decoderFormat: string | null;
  droppedFrames: number | null;
  delayedFrames: number | null;
  timePosSeconds: number | null;
  desyncMs: number | null;
  renderCalls: number | null;
  averageRenderMs: number | null;
  maximumRenderMs: number | null;
  preload: boolean | null;
  playing: boolean | null;
  paused: boolean | null;
  eof: boolean | null;
}

export interface VideoSummaryDiagnostics {
  raw: RecordValue;
  activeCues: number | null;
  activeDecoders: number | null;
  mpvContexts: number | null;
  outputs: number | null;
  droppedFrames: number | null;
  multiDecodeCues: number | null;
  cues: VideoCueDiagnostics[];
  system: RecordValue;
}

export function videoCueModel(value: unknown, index = 0): VideoCueDiagnostics {
  const raw = record(value);
  const width = number(raw, "width", "videoWidth", "video_width");
  const height = number(raw, "height", "videoHeight", "video_height");
  const resolution = string(raw, "resolution", "displayResolution") ?? (width != null && height != null ? `${width}×${height}` : null);
  const timePositionMs = number(raw, "timePosMs", "time_pos_ms");
  const timePosition = number(raw, "timePosSeconds", "time_pos_seconds", "timePos", "time_pos", "positionSeconds") ?? (timePositionMs == null ? null : timePositionMs / 1000);
  const desync = number(raw, "maxOutputDesyncMs", "maximumOutputDesyncMs", "outputDesyncMs", "desyncMs", "desync_ms");
  return {
    raw,
    id: string(raw, "cueId", "videoCueId", "id", "sourceId") ?? `video-${index}`,
    name: string(raw, "cueName", "name", "label", "title") ?? string(raw, "file", "filePath", "path")?.split(/[\\/]/).pop() ?? `VideoCue ${index + 1}`,
    cueNumber: string(raw, "cueNumber", "number", "cue_number"),
    state: string(raw, "state", "status", "playbackState"),
    file: string(raw, "file", "filePath", "path", "uri", "sourceFile"),
    resolution,
    fps: number(raw, "fps", "frameRate", "frame_rate"),
    outputs: count(raw, "outputs", "outputCount", "outputsCount", "output_count"),
    mpvContexts: count(raw, "mpvContexts", "mpvContextCount", "contexts", "contextCount", "mpv_contexts"),
    renderContexts: count(raw, "renderContexts", "mpvRenderContexts", "renderContextCount", "render_contexts"),
    hardwareDecode: boolean(raw, "hardwareDecode", "hwDecode", "hardwareDecoding", "hardware_decoding", "hw_decode"),
    hardwareBackend: string(raw, "hardwareBackend", "hwBackend", "hwdecBackend", "decoderBackend", "hw_backend"),
    decoderFormat: string(raw, "decoderFormat", "format", "videoFormat", "decoder_format"),
    droppedFrames: number(raw, "droppedFrames", "framesDropped", "dropFrames", "dropped_frames"),
    delayedFrames: number(raw, "delayedFrames", "framesDelayed", "delayFrames", "delayed_frames"),
    timePosSeconds: timePosition,
    desyncMs: desync,
    renderCalls: number(raw, "renderCalls", "renderCallCount", "render_calls"),
    averageRenderMs: number(raw, "averageRenderMs", "avgRenderMs", "meanRenderMs", "average_render_ms") ?? ((number(raw, "averageRenderTimeUs", "average_render_time_us") ?? 0) / 1000 || null),
    maximumRenderMs: number(raw, "maximumRenderMs", "maxRenderMs", "peakRenderMs", "maximum_render_ms") ?? ((number(raw, "maximumRenderTimeUs", "maximum_render_time_us") ?? 0) / 1000 || null),
    preload: boolean(raw, "preload", "preloaded", "isPreloaded"),
    playing: boolean(raw, "playing", "isPlaying"),
    paused: boolean(raw, "paused", "isPaused"),
    eof: boolean(raw, "eof", "endOfFile", "ended"),
  };
}

export function videoSummaryModel(value: unknown): VideoSummaryDiagnostics {
  const raw = record(value);
  const rawCues = list(raw, "cues", "sources", "videoCues", "video_cues");
  const cues = rawCues.map(videoCueModel);
  return {
    raw,
    activeCues: number(raw, "activeCues", "activeVideoCues", "playingCues", "active_cues") ?? (cues.length || null),
    activeDecoders: number(raw, "activeDecoders", "activeVideoDecoders", "decoders", "active_decoders") ?? (cues.reduce((sum, cue) => sum + Math.max(0, cue.mpvContexts ?? 0), 0) || null),
    mpvContexts: number(raw, "mpvContexts", "mpvContextCount", "contexts", "mpv_contexts") ?? (cues.reduce((sum, cue) => sum + Math.max(0, cue.mpvContexts ?? 0), 0) || null),
    outputs: number(raw, "outputs", "outputCount", "activeOutputs", "output_count") ?? (cues.reduce((sum, cue) => sum + Math.max(0, cue.outputs ?? 0), 0) || null),
    droppedFrames: number(raw, "droppedFrames", "droppedFramesTotal", "totalDroppedFrames", "dropped_frames") ?? (cues.reduce((sum, cue) => sum + (cue.droppedFrames ?? 0), 0) || null),
    multiDecodeCues: number(raw, "multiDecodeCues", "multipleDecodeCues", "duplicatedDecodeCues", "cuesDecodedMultipleTimes", "multi_decode_cues") ?? (cues.filter((cue) => (cue.mpvContexts ?? 0) > 1).length || null),
    cues,
    system: record(raw.system),
  };
}
