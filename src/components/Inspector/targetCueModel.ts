import type { WorkspaceCueCatalogEntry, WorkspaceCueCatalogList } from "../../lib/types";

export type CatalogCue = WorkspaceCueCatalogEntry & { listId: string; listName: string };

export function flattenCueCatalog(lists: WorkspaceCueCatalogList[]): CatalogCue[] {
  const result: CatalogCue[] = [];
  const visit = (cues: WorkspaceCueCatalogEntry[], listId: string, listName: string) => {
    for (const cue of cues) {
      result.push({ ...cue, listId, listName });
      if (cue.children) visit(cue.children, listId, listName);
    }
  };
  for (const list of lists) visit(list.cues, list.id, list.name);
  return result;
}

/** A Goto remembers only the latest selected cue across every catalog tab. */
export const singleTargetSelection = (ids: string[]) => ids.slice(-1);
