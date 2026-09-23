import type { CueId } from "../../lib/types";

export interface KeyboardCueNode {
  id: CueId;
  cue_type: string;
  children?: KeyboardCueNode[];
}

export interface ArrowSelection {
  selectedCueId: CueId;
  selectedCueIds: CueId[];
  anchorCueId: CueId;
  endCueId: CueId;
}

function visibleAncestorOf(
  cueId: CueId,
  cues: readonly KeyboardCueNode[],
  visibleIds: ReadonlySet<CueId>,
): CueId | null {
  const findPath = (
    nodes: readonly KeyboardCueNode[],
    ancestors: readonly CueId[],
  ): CueId[] | null => {
    for (const cue of nodes) {
      const path = [...ancestors, cue.id];
      if (cue.id === cueId) return path;
      const nested = cue.children && findPath(cue.children, path);
      if (nested) return nested;
    }
    return null;
  };

  const path = findPath(cues, []);
  if (!path) return null;
  for (let index = path.length - 1; index >= 0; index -= 1) {
    if (visibleIds.has(path[index])) return path[index];
  }
  return null;
}

/**
 * Plain Up/Down moves the single selection one visible row. Shift+Up/Down
 * extends or shrinks the anchored range. Hidden children fall back to their
 * nearest visible group row so collapsing a group cannot strand navigation.
 */
export function moveCueSelectionByArrow(
  visibleIds: readonly CueId[],
  selectedCueId: CueId | null,
  movingEndCueId: CueId | null,
  anchorCueId: CueId | null,
  direction: "up" | "down",
  extendRange: boolean,
  cues: readonly KeyboardCueNode[],
): ArrowSelection | null {
  if (visibleIds.length === 0 || selectedCueId == null) return null;
  const visibleSet = new Set(visibleIds);
  const currentId = movingEndCueId && visibleSet.has(movingEndCueId)
    ? movingEndCueId
    : visibleSet.has(selectedCueId)
      ? selectedCueId
      : visibleAncestorOf(selectedCueId, cues, visibleSet);
  if (!currentId) return null;

  const currentIndex = visibleIds.indexOf(currentId);
  const nextIndex = currentIndex + (direction === "down" ? 1 : -1);
  if (nextIndex < 0 || nextIndex >= visibleIds.length) return null;

  const nextId = visibleIds[nextIndex];
  if (!extendRange) {
    return {
      selectedCueId: nextId,
      selectedCueIds: [nextId],
      anchorCueId: nextId,
      endCueId: nextId,
    };
  }

  const visibleAnchor = anchorCueId && visibleSet.has(anchorCueId)
    ? anchorCueId
    : currentId;
  const start = visibleIds.indexOf(visibleAnchor);
  const lo = Math.min(start, nextIndex);
  const hi = Math.max(start, nextIndex);
  return {
    selectedCueId: nextId,
    selectedCueIds: visibleIds.slice(lo, hi + 1),
    anchorCueId: visibleAnchor,
    endCueId: nextId,
  };
}

/** A selected Group already stops its running descendants recursively. */
export function topLevelSelectedStopIds(
  selectedIds: readonly CueId[],
  cues: readonly KeyboardCueNode[],
): CueId[] {
  const selected = new Set(selectedIds);
  const coveredDescendants = new Set<CueId>();
  const collect = (nodes: readonly KeyboardCueNode[]) => {
    for (const cue of nodes) {
      if (selected.has(cue.id) && (cue.cue_type === "group" || cue.cue_type === "number")) {
        const addDescendants = (children: readonly KeyboardCueNode[]) => {
          for (const child of children) {
            coveredDescendants.add(child.id);
            if (child.children) addDescendants(child.children);
          }
        };
        addDescendants(cue.children ?? []);
      } else if (cue.children) {
        collect(cue.children);
      }
    }
  };
  collect(cues);
  return selectedIds.filter((id) => !coveredDescendants.has(id));
}

/** Start every selected cue stop together; a Group stop includes its children. */
export function stopSelectedCueSet(
  selectedIds: readonly CueId[],
  cues: readonly KeyboardCueNode[],
  stopCue: (cueId: CueId) => Promise<void>,
): Promise<void[]> {
  return Promise.all(topLevelSelectedStopIds(selectedIds, cues).map(stopCue));
}

type TargetLike = {
  tagName?: string;
  isContentEditable?: boolean;
  closest?: (selector: string) => unknown;
} | null;

/** Keep document-level shortcuts out of text entry and modal widgets. */
export function isCueListShortcutBlocked(target: EventTarget | null, modalOpen: boolean): boolean {
  const element = target as TargetLike;
  if (modalOpen) return true;
  if (!element) return false;
  const tagName = element.tagName?.toUpperCase();
  if (tagName === "INPUT" || tagName === "TEXTAREA" || tagName === "SELECT") return true;
  if (element.isContentEditable) return true;
  return Boolean(element.closest?.(
    'input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="textbox"], [role="dialog"], [aria-modal="true"]',
  ));
}

export function isStopSelectionShortcut(
  event: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey"> & Partial<Pick<KeyboardEvent, "code">>,
): boolean {
  const key = event.code?.startsWith("Key") && event.code.length === 4
    ? event.code.slice(3).toLowerCase()
    : ({ ы: "s" } as Record<string, string>)[event.key.toLowerCase()] ?? event.key.toLowerCase();
  return key === "s"
    && !event.ctrlKey && !event.metaKey && !event.altKey;
}

/** Stop bubbling so the legacy window-level single-cue S shortcut cannot run too. */
export function consumeCueListStopShortcutEvent(
  event: Pick<KeyboardEvent, "preventDefault" | "stopPropagation">,
): void {
  event.preventDefault();
  event.stopPropagation();
}

/** Consume every selected-stop keydown, but dispatch the stop only once per press. */
export function consumeCueListStopShortcut(
  event: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey" | "repeat" | "preventDefault" | "stopPropagation">,
  hasSelection: boolean,
): boolean {
  if (!hasSelection || !isStopSelectionShortcut(event)) return false;
  consumeCueListStopShortcutEvent(event);
  return !event.repeat;
}
