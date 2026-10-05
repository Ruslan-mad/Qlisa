import type { CueSummary } from "./types";

export function cueNotesText(cue: Pick<CueSummary, "cue_type" | "memo_text" | "notes">): string {
  return cue.cue_type === "memo" ? cue.memo_text ?? "" : cue.notes ?? "";
}

export function cueNotesUpdate(
  cue: Pick<CueSummary, "cue_type">,
  value: string,
): { memo_text: string } | { notes: string } {
  return cue.cue_type === "memo" ? { memo_text: value } : { notes: value };
}
