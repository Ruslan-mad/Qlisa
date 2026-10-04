import type { OutputDestination } from "../../lib/types";

/** Keep stored defaults attached only to configured physical display outputs. */
export function reconcileNewCueOutputIds(ids: readonly string[], outputs: readonly OutputDestination[]): string[] {
  const displayIds = new Set(outputs.filter((output) => output.sink_kind === "display").map((output) => output.id));
  return ids.filter((id, index) => displayIds.has(id) && ids.indexOf(id) === index);
}

/** Toggle one display's new-cue default without changing the other outputs. */
export function toggleNewCueOutputId(ids: readonly string[], id: string, checked: boolean): string[] {
  return checked
    ? ids.includes(id) ? [...ids] : [...ids, id]
    : ids.filter((selectedId) => selectedId !== id);
}
