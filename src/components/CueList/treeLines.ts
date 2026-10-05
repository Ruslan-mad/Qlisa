type TreeCue = { id: string; color?: string | null };
type FlatTreeItem<T> = { cue: T; depth: number; parentGroupId: string | null };

export interface TreeLineInfo {
  ancestors: Array<{ continues: boolean; color?: string | null }>;
  hasNextSibling: boolean;
  parentColor: string | null;
}

/** Derive connector strokes from the visible flattened tree. */
export function buildTreeLineInfo<T extends TreeCue>(items: readonly FlatTreeItem<T>[]): Map<string, TreeLineInfo> {
  const nextSibling = new Map<string, boolean>();
  const lastByParent = new Map<string | null, string>();
  for (const item of items) lastByParent.set(item.parentGroupId, item.cue.id);
  for (const item of items) nextSibling.set(item.cue.id, lastByParent.get(item.parentGroupId) !== item.cue.id);

  const itemById = new Map(items.map((item) => [item.cue.id, item]));
  return new Map(items.map((item) => {
    const ancestors: TreeLineInfo["ancestors"] = [];
    let descendantId = item.parentGroupId;
    while (descendantId) {
      const ownerId = itemById.get(descendantId)?.parentGroupId;
      const owner = ownerId ? itemById.get(ownerId) : undefined;
      if (!owner) break;
      ancestors.unshift({ continues: nextSibling.get(descendantId) ?? false, color: owner.cue.color });
      descendantId = owner.cue.id;
    }
    return [item.cue.id, {
      ancestors,
      hasNextSibling: nextSibling.get(item.cue.id) ?? false,
      parentColor: item.parentGroupId ? itemById.get(item.parentGroupId)?.cue.color ?? null : null,
    }];
  }));
}
