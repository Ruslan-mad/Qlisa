import { describe, expect, it, vi } from "vitest";
import { inspectWorkspaceGuard, isProjectFilePath, projectFileExtension, resolveWorkspaceGuard, withDefaultProjectExtension } from "./projectFile";

describe("project file paths", () => {
  it("recognizes Qlisa and legacy project extensions on Windows paths", () => {
    expect(isProjectFilePath("C:\\Shows\\Концерт 1.QLISA")).toBe(true);
    expect(isProjectFilePath("C:\\Shows\\old.inkue")).toBe(true);
    expect(isProjectFilePath("C:\\Shows\\old.wincue")).toBe(true);
    expect(isProjectFilePath("C:\\Shows\\notes.json")).toBe(false);
    expect(projectFileExtension("C:\\Shows\\.draft")).toBe(null);
  });

  it("adds .qlisa by default, preserves .inkue, and does not write .wincue", () => {
    expect(withDefaultProjectExtension("C:\\Shows\\Концерт 1")).toBe("C:\\Shows\\Концерт 1.qlisa");
    expect(withDefaultProjectExtension("C:\\Shows\\old.inkue")).toBe("C:\\Shows\\old.inkue");
    expect(withDefaultProjectExtension("C:\\Shows\\other.wincue")).toBe("C:\\Shows\\other.qlisa");
  });
});

describe("unsaved workspace guard", () => {
  it("allows navigation for a clean workspace and prompts for a dirty one", () => {
    expect(resolveWorkspaceGuard(false, null)).toBe("execute");
    expect(resolveWorkspaceGuard(true, null)).toBe("prompt");
  });

  it("uses the asynchronous dirty-state source and fails closed on read errors", async () => {
    const readDirty = vi.fn().mockResolvedValue(true);
    await expect(inspectWorkspaceGuard(readDirty)).resolves.toEqual({ result: "prompt" });
    expect(readDirty).toHaveBeenCalledOnce();
    await expect(inspectWorkspaceGuard(async () => { throw new Error("IPC unavailable"); })).resolves.toMatchObject({ result: "error" });
  });

  it("executes only after discard or a successful save", () => {
    expect(resolveWorkspaceGuard(true, "discard")).toBe("execute");
    expect(resolveWorkspaceGuard(true, "save", true)).toBe("execute");
    expect(resolveWorkspaceGuard(true, "save", false)).toBe("cancel");
    expect(resolveWorkspaceGuard(true, "cancel")).toBe("cancel");
  });
});
