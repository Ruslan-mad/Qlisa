import type { CueSummary, CueTargetSummary } from "../../lib/types";

export function cueFileName(cue: Pick<CueSummary, "cue_type" | "file_path">): string {
  if (cue.cue_type === "memo" || !cue.file_path) return "";
  return cue.file_path.split(/[\\/]/).pop() ?? cue.file_path;
}

export function cueNotesText(cue: Pick<CueSummary, "cue_type" | "memo_text" | "notes">): string {
  return cue.cue_type === "memo" ? cue.memo_text ?? "" : cue.notes ?? "";
}

export function cueNotesProperty(cue: Pick<CueSummary, "cue_type">): "memo_text" | "notes" {
  return cue.cue_type === "memo" ? "memo_text" : "notes";
}

function targetLabel(target: CueTargetSummary): string {
  return [target.number?.trim(), target.name.trim()].filter(Boolean).join(" ");
}

export interface FormattedTargetText {
  text: string;
  title: string;
}

export function formatTargetCues(
  targets: CueTargetSummary[] | undefined,
  targetsAll: boolean | undefined,
  allCuesLabel: string,
  visibleLimit = 2,
): FormattedTargetText {
  if (targetsAll) return { text: allCuesLabel, title: allCuesLabel };
  const labels = (targets ?? []).map(targetLabel).filter(Boolean);
  const title = labels.join(", ");
  if (labels.length <= visibleLimit) return { text: title, title };
  return {
    text: `${labels.slice(0, visibleLimit).join(", ")}, +${labels.length - visibleLimit}`,
    title,
  };
}
