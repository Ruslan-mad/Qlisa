import type { WorkspaceCueCatalogEntry, WorkspaceCueCatalogList } from "../../lib/types";

export interface PresentationNode {
  id: string;
  cue_type: string;
  name: string;
  state: string;
  is_disabled: boolean;
  presentation_current_child_id?: string | null;
  background_screensaver?: boolean;
  presentation_video_imported?: boolean;
  children?: PresentationNode[];
}

export function findPresentationOwner<T extends PresentationNode>(cues: T[], cueId: string | null | undefined): T | null {
  if (!cueId) return null;
  for (const cue of cues) {
    if (cue.cue_type === "presentation" && (cue.id === cueId || cue.children?.some((child) => containsCue(child, cueId)))) return cue;
    const nested = findPresentationOwner(cue.children ?? [], cueId);
    if (nested) return nested as T;
  }
  return null;
}

function containsCue(cue: PresentationNode, cueId: string): boolean {
  return cue.id === cueId || (cue.children ?? []).some((child) => containsCue(child, cueId));
}

export function presentationChildren<T extends PresentationNode>(owner: T, includeDisabled = false, mediaOnly = true): PresentationNode[] {
  return (owner.children ?? []).filter((child) =>
    (!mediaOnly || child.cue_type === "image" || (child.cue_type === "video" && child.presentation_video_imported !== false))
    && (includeDisabled || !child.is_disabled),
  );
}

export function presentationTarget(owner: PresentationNode, direction: "previous" | "next"): PresentationNode | null {
  const children = owner.children ?? [];
  const index = children.findIndex((child) => child.id === owner.presentation_current_child_id);
  if (index < 0) return null;
  const step = direction === "next" ? 1 : -1;
  for (let targetIndex = index + step; targetIndex >= 0 && targetIndex < children.length; targetIndex += step) {
    if (!children[targetIndex].is_disabled) return children[targetIndex];
  }
  return null;
}

export interface PresentationPageKeyEvent {
  key: string;
  repeat: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  preventDefault(): void;
  stopPropagation(): void;
}

export function createPresentationPageKeyHandler<T extends { id: string }>(options: {
  getTarget: () => T | null;
  step: (cueId: string, direction: "previous" | "next") => Promise<void>;
  onRefresh: () => void;
  onError: (error: unknown) => void;
}) {
  return (event: PresentationPageKeyEvent) => {
    if ((event.key !== "PageDown" && event.key !== "PageUp")
      || event.ctrlKey || event.altKey || event.metaKey || event.shiftKey) return;
    const target = options.getTarget();
    if (!target) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.repeat) return;
    const direction = event.key === "PageDown" ? "next" : "previous";
    void options.step(target.id, direction).then(options.onRefresh).catch(options.onError);
  };
}

export function latestActivePresentation(
  lists: WorkspaceCueCatalogList[],
  startOrder: Record<string, number>,
): WorkspaceCueCatalogEntry | null {
  const candidates: Array<{ cue: WorkspaceCueCatalogEntry; index: number }> = [];
  let index = 0;
  const walk = (cues: WorkspaceCueCatalogEntry[], inheritedBackground = false) => {
    for (const cue of cues) {
      const background = inheritedBackground || cue.background_screensaver === true;
      if (cue.cue_type === "presentation" && (cue.state === "running" || cue.state === "paused")
        && cue.presentation_current_child_id && !background) candidates.push({ cue, index });
      index += 1;
      walk(cue.children ?? [], background);
    }
  };
  for (const list of lists) walk(list.cues);
  candidates.sort((a, b) => (startOrder[b.cue.id] ?? -1) - (startOrder[a.cue.id] ?? -1) || b.index - a.index);
  return candidates[0]?.cue ?? null;
}

export function resolveLivePresentationOwner(
  selectedMediaId: string | null | undefined,
  lists: WorkspaceCueCatalogList[],
  startOrder: Record<string, number>,
): WorkspaceCueCatalogEntry | null {
  const catalogCues = lists.flatMap((list) => list.cues);
  if (selectedMediaId) return findPresentationOwner(catalogCues, selectedMediaId) as WorkspaceCueCatalogEntry | null;
  return latestActivePresentation(lists, startOrder);
}
