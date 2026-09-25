const OPEN_PROJECT_EXTENSIONS = new Set(["qlisa", "inkue", "wincue"]);

/** Return the existing project extension, regardless of Windows path casing. */
export function projectFileExtension(path: string): string | null {
  const pathParts = path.split(/[\\/]/);
  const fileName = pathParts[pathParts.length - 1] ?? "";
  const dot = fileName.lastIndexOf(".");
  if (dot <= 0 || dot === fileName.length - 1) return null;
  return fileName.slice(dot + 1).toLowerCase();
}

export function isProjectFilePath(path: string): boolean {
  const extension = projectFileExtension(path);
  return extension !== null && OPEN_PROJECT_EXTENSIONS.has(extension);
}

/** Save As defaults to Qlisa while retaining an explicitly chosen legacy extension. */
export function withDefaultProjectExtension(path: string): string {
  const extension = projectFileExtension(path);
  if (extension === "qlisa" || extension === "inkue") return path;
  if (extension === "wincue") return `${path.slice(0, path.length - extension.length)}qlisa`;
  return `${path}.qlisa`;
}

export type WorkspaceGuardChoice = "save" | "discard" | "cancel" | null;

export type WorkspaceGuardResult = "execute" | "prompt" | "cancel";

export type WorkspaceGuardInspection =
  | { result: "execute" | "prompt" }
  | { result: "error"; error: unknown };

/** Read dirty state asynchronously and fail closed if the source is unavailable. */
export async function inspectWorkspaceGuard(
  readIsModified: () => Promise<boolean>,
): Promise<WorkspaceGuardInspection> {
  try {
    return { result: await readIsModified() ? "prompt" : "execute" };
  } catch (error) {
    return { result: "error", error };
  }
}

/** Keep destructive workspace navigation behind the same dirty-state decision. */
export function resolveWorkspaceGuard(
  isModified: boolean,
  choice: WorkspaceGuardChoice,
  saveSucceeded = false,
): WorkspaceGuardResult {
  if (!isModified) return "execute";
  if (choice === null) return "prompt";
  if (choice === "discard") return "execute";
  if (choice === "save" && saveSucceeded) return "execute";
  return "cancel";
}
