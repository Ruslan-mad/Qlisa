export interface FileDropTarget {
  assignId: string | null;
  groupId: string | null;
}

export interface DocumentDropNode {
  id: string;
  cue_type: string;
  children?: DocumentDropNode[];
}

/** A document dropped on a slide becomes a sibling Presentation cue. */
export function presentationSiblingLocationForChild(
  cues: readonly DocumentDropNode[],
  childId: string,
): { parentId: string | null; position: number } | null {
  const find = (
    nodes: readonly DocumentDropNode[],
    parent: DocumentDropNode | null = null,
    grandparent: DocumentDropNode | null = null,
    siblings: readonly DocumentDropNode[] = nodes,
  ): { parent: DocumentDropNode | null; grandparent: DocumentDropNode | null; siblings: readonly DocumentDropNode[]; index: number } | null => {
    for (let index = 0; index < nodes.length; index++) {
      const cue = nodes[index];
      if (cue.id === childId) return { parent, grandparent, siblings, index };
      const found = find(cue.children ?? [], cue, parent, cue.children ?? []);
      if (found) return found;
    }
    return null;
  };
  const found = find(cues);
  if (found?.parent?.cue_type !== "presentation") return null;
  const presentationSiblings = found.grandparent?.children ?? cues;
  const index = presentationSiblings.findIndex((cue) => cue.id === found.parent!.id);
  return { parentId: found.grandparent?.id ?? null, position: index + 1 };
}

/** Media dropped on a container creates a child; other cues receive the file. */
export function fileDropTargetForCue(cueType: string | undefined, cueId: string): FileDropTarget {
  if (cueType === "group" || cueType === "number" || cueType === "presentation") {
    return { assignId: null, groupId: cueId };
  }
  return { assignId: cueId, groupId: null };
}

/** Number v1 children are authored Audio, Video, or Image actions. */
export function fileDropPathAllowedForTarget(targetCueType: string | undefined, fileCueType: string): boolean {
  if (targetCueType === "presentation") return fileCueType === "image" || fileCueType === "video";
  return targetCueType !== "number" || fileCueType === "audio" || fileCueType === "video" || fileCueType === "image";
}
