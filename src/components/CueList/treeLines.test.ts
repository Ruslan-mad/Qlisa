import { describe, expect, it } from "vitest";
import { buildTreeLineInfo } from "./treeLines";

const item = (id: string, depth: number, parentGroupId: string | null, color: string | null = null) => ({
  cue: { id, color }, depth, parentGroupId,
});

describe("buildTreeLineInfo", () => {
  it("continues a nested rail for a later sibling inside its parent", () => {
    const info = buildTreeLineInfo([
      item("number", 0, null, "blue"),
      item("group", 1, "number", "red"),
      item("first", 2, "group"),
      item("last", 2, "group"),
      item("later-in-number", 1, "number"),
      item("next-root", 0, null, "green"),
    ]);

    expect(info.get("first")?.ancestors).toEqual([{ continues: true, color: "blue" }]);
    expect(info.get("last")?.ancestors).toEqual([{ continues: true, color: "blue" }]);
    expect(info.get("last")?.hasNextSibling).toBe(false);
  });

  it("does not extend a nested rail because another root container follows", () => {
    const info = buildTreeLineInfo([
      item("number", 0, null, "blue"),
      item("group", 1, "number", "red"),
      item("only-child", 2, "group"),
      item("next-root", 0, null, "green"),
    ]);

    expect(info.get("only-child")?.ancestors).toEqual([{ continues: false, color: "blue" }]);
    expect(info.get("only-child")?.hasNextSibling).toBe(false);
  });
});
