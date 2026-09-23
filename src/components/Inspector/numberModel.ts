import type { CueSummary, NumberCueData } from "../../lib/types";

/** Normalize the persisted Number payload returned by get_cue to inspector fields. */
export function normalizeNumberCueData(data: NumberCueData & {
  master_child_id?: string | null;
  action_offsets_ms?: Record<string, number>;
}): NumberCueData {
  return {
    ...data,
    number_master_id: data.number_master_id ?? data.master_child_id ?? null,
    number_action_offsets_ms: data.number_action_offsets_ms ?? data.action_offsets_ms,
    fade_in_ms: data.fade_in_ms ?? null,
    fade_in_curve: data.fade_in_curve ?? null,
    fade_out_ms: data.fade_out_ms ?? null,
    fade_out_curve: data.fade_out_curve ?? null,
  };
}

export function numberActionConfigured(child: CueSummary): boolean {
  if (!isNumberActionSupported(child)) return false;
  if (child.number_action_ready !== undefined) return child.number_action_ready;
  if (child.cue_type === "audio" || child.cue_type === "video" || child.cue_type === "image") {
    return Boolean(child.file_path) && !child.media_file_missing;
  }
  if (child.cue_type === "group") return Boolean(child.children?.length);
  return false;
}

export function isNumberActionSupported(child: CueSummary): boolean {
  return child.cue_type === "audio" || child.cue_type === "video" || child.cue_type === "image" || child.cue_type === "group";
}

export function isNumberMasterCandidate(child: CueSummary): boolean {
  return child.cue_type === "audio" || child.cue_type === "video" || child.cue_type === "group";
}
