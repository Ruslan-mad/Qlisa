import type { CueId } from "./types";

export interface CueSelection {
  selectedCueId: CueId | null;
  selectedCueIds: CueId[];
}

/** De-duplicate without changing the first-seen ordering; always include primary. */
export function normalizeCueSelection(primary: CueId | null, ids: readonly CueId[]): CueSelection {
  const selectedCueIds = [...new Set(ids)];
  if (primary != null && !selectedCueIds.includes(primary)) selectedCueIds.push(primary);
  return { selectedCueId: primary, selectedCueIds };
}

/** Toggle one cue while keeping a useful primary focus when the old one is removed. */
export function toggleCueSelection(
  selectedIds: readonly CueId[],
  primaryId: CueId | null,
  cueId: CueId,
  visibleOrder: readonly CueId[],
): CueSelection {
  if (!selectedIds.includes(cueId)) {
    return normalizeCueSelection(cueId, [...selectedIds, cueId]);
  }

  const remaining = selectedIds.filter((id) => id !== cueId);
  if (remaining.length === 0) return normalizeCueSelection(null, []);
  if (primaryId !== cueId && primaryId != null && remaining.includes(primaryId)) {
    return normalizeCueSelection(primaryId, remaining);
  }

  const removedIndex = visibleOrder.indexOf(cueId);
  const nearest = removedIndex >= 0
    ? remaining
      .filter((id) => visibleOrder.includes(id))
      .sort((a, b) => {
        const deltaA = Math.abs(visibleOrder.indexOf(a) - removedIndex);
        const deltaB = Math.abs(visibleOrder.indexOf(b) - removedIndex);
        return deltaA - deltaB || visibleOrder.indexOf(a) - visibleOrder.indexOf(b);
      })[0]
    : undefined;
  return normalizeCueSelection(nearest ?? remaining[remaining.length - 1] ?? null, remaining);
}

/** Inclusive range from anchor to end in the current flattened visible order. */
export function cueRangeFromVisibleOrder(
  anchorId: CueId | null,
  endId: CueId,
  visibleOrder: readonly CueId[],
): CueId[] {
  const end = visibleOrder.indexOf(endId);
  if (end < 0) return [];
  const anchor = anchorId == null ? -1 : visibleOrder.indexOf(anchorId);
  if (anchor < 0) return [endId];
  const lo = Math.min(anchor, end);
  const hi = Math.max(anchor, end);
  return visibleOrder.slice(lo, hi + 1);
}

/**
 * Selection produced by sweeping the mouse from one row to another.
 *
 * A plain sweep replaces the current selection with the inclusive range.  An
 * additive sweep (started with Ctrl/Cmd) keeps the existing selection and
 * adds the range, which makes the gesture useful for building discontiguous
 * selections without making row reordering ambiguous.
 */
export function cueSelectionFromSweep(
  anchorId: CueId,
  endId: CueId,
  visibleOrder: readonly CueId[],
  existingIds: readonly CueId[] = [],
  additive = false,
): CueSelection {
  const range = cueRangeFromVisibleOrder(anchorId, endId, visibleOrder);
  const ids = additive ? [...existingIds, ...range] : range;
  return normalizeCueSelection(endId, ids);
}

export interface VisibleCueNode<T extends VisibleCueNode<T>> {
  id: CueId;
  cue_type: string;
  children?: T[];
}

export interface FlattenedCue<T> {
  cue: T;
  depth: number;
  parentGroupId: CueId | null;
}

/** Flatten expanded nested containers in render order, matching CueListView rows. */
export function flattenVisibleCueTree<T extends VisibleCueNode<T>>(
  cues: readonly T[],
  expandedGroupIds: ReadonlySet<CueId>,
): FlattenedCue<T>[] {
  const result: FlattenedCue<T>[] = [];
  const visit = (nodes: readonly T[], depth: number, parentGroupId: CueId | null) => {
    for (const cue of nodes) {
      result.push({ cue, depth, parentGroupId });
      if ((cue.cue_type === "group" || cue.cue_type === "number") && expandedGroupIds.has(cue.id) && cue.children?.length) {
        visit(cue.children, depth + 1, cue.id);
      }
    }
  };
  visit(cues, 0, null);
  return result;
}

export function isMultiSelectModifier(ctrlKey: boolean, metaKey: boolean): boolean {
  return ctrlKey || metaKey;
}
