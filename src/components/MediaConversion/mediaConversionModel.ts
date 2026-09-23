import type { CueSummary, MediaInfo } from "../../lib/types";

export function isConvertibleMediaCue(cue: CueSummary | null | undefined): boolean {
  return !!cue && (cue.cue_type === "audio" || cue.cue_type === "video" || cue.cue_type === "image") && !!cue.file_path;
}

export function mediaInfoFromCue(cue: CueSummary): MediaInfo {
  return {
    cue_id: cue.id,
    path: cue.file_path ?? "",
    format: cue.file_path?.split(/[\\/.]/).pop()?.toUpperCase() ?? null,
    codec: null,
    duration_ms: cue.file_duration_ms ?? cue.duration_ms,
    width: cue.media_width,
    height: cue.media_height,
    bitrate_kbps: null,
    audio_channels: null,
    frame_rate: null,
    source_size_bytes: cue.file_size_bytes,
    video_codec: null,
    audio_codec: null,
    compatibility_known: false,
    compatibility_status: null,
    compatible: !cue.media_file_missing && !cue.is_broken,
    compatibility_note: cue.media_file_missing ? "Media file is missing." : cue.is_broken ? (cue.error_message ?? "Media needs attention.") : null,
  };
}

export function formatMediaBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes)) return "—";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = -1;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}

export function conversionProgress(job: { progress?: number | null } | null | undefined): number {
  const progress = Number(job?.progress ?? 0);
  return Math.max(0, Math.min(1, Number.isFinite(progress) ? progress : 0));
}

export function formatMediaDuration(durationMs: number | null | undefined): string {
  if (durationMs == null || !Number.isFinite(durationMs)) return "—";
  const total = Math.max(0, Math.round(durationMs / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}
