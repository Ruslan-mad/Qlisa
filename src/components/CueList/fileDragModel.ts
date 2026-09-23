export interface FileDropTarget {
  assignId: string | null;
  groupId: string | null;
}

/** Media dropped on a container creates a child; other cues receive the file. */
export function fileDropTargetForCue(cueType: string | undefined, cueId: string): FileDropTarget {
  if (cueType === "group" || cueType === "number") {
    return { assignId: null, groupId: cueId };
  }
  return { assignId: cueId, groupId: null };
}

/** Number v1 children are authored Audio, Video, or Image actions. */
export function fileDropPathAllowedForTarget(targetCueType: string | undefined, fileCueType: string): boolean {
  return targetCueType !== "number" || fileCueType === "audio" || fileCueType === "video" || fileCueType === "image";
}
