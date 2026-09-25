import { describe, expect, it } from "vitest";
import {
  isEditableKeyboardTarget,
  getKeyboardShortcutKey,
  isSelectedCueStopShortcut,
  isNewWorkspaceShortcut,
  isSaveAsShortcut,
  isSaveShortcut,
  isShortcutGuardedTarget,
  isSpaceShortcutEvent,
  shouldHandleGlobalSpaceShortcut,
} from "../useKeyboardShortcuts";

function key(key: string, modifiers: Partial<Pick<KeyboardEvent, "ctrlKey" | "metaKey" | "altKey" | "shiftKey" | "code">> = {}) {
  return { key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...modifiers };
}

function target(tagName: string, type?: string): EventTarget {
  return { tagName, type } as EventTarget;
}

describe("selected cue stop shortcut", () => {
  it("accepts S and its Russian-layout Ы equivalent without modifiers", () => {
    expect(isSelectedCueStopShortcut(key("s"))).toBe(true);
    expect(isSelectedCueStopShortcut(key("S"))).toBe(true);
    expect(isSelectedCueStopShortcut(key("ы"))).toBe(true);
    expect(isSelectedCueStopShortcut(key("Ы"))).toBe(true);
  });

  it("does not steal save or other modified shortcuts", () => {
    expect(isSelectedCueStopShortcut(key("s", { ctrlKey: true }))).toBe(false);
    expect(isSelectedCueStopShortcut(key("s", { metaKey: true }))).toBe(false);
    expect(isSelectedCueStopShortcut(key("s", { altKey: true }))).toBe(false);
    expect(isSelectedCueStopShortcut(key("ы", { ctrlKey: true }))).toBe(false);
    expect(isSelectedCueStopShortcut(key("Ы", { metaKey: true }))).toBe(false);
    expect(isSelectedCueStopShortcut(key("x"))).toBe(false);
  });

  it("uses the physical key when Ctrl shortcuts are typed with Russian layout", () => {
    const bindings = [
      ["ф", "KeyA", "a"], ["с", "KeyC", "c"], ["м", "KeyV", "v"],
      ["я", "KeyZ", "z"], ["н", "KeyY", "y"], ["п", "KeyG", "g"],
      ["в", "KeyD", "d"], ["т", "KeyN", "n"], ["щ", "KeyO", "o"],
      ["ш", "KeyI", "i"], ["з", "KeyP", "p"], ["ы", "KeyS", "s"],
      ["а", "KeyF", "f"],
    ] as const;
    for (const [russian, code, expected] of bindings) {
      expect(getKeyboardShortcutKey(key(russian, { code }))).toBe(expected);
    }
  });

  it("keeps synthetic Russian key events working when code is absent", () => {
    expect(getKeyboardShortcutKey(key("ф"))).toBe("a");
    expect(getKeyboardShortcutKey(key("с"))).toBe("c");
    expect(getKeyboardShortcutKey(key("ы"))).toBe("s");
    expect(getKeyboardShortcutKey(key("я"))).toBe("z");
  });

  it("uses physical punctuation keys when Russian layout changes their text", () => {
    expect(getKeyboardShortcutKey(key("х", { code: "BracketLeft" }))).toBe("[");
    expect(getKeyboardShortcutKey(key("ъ", { code: "BracketRight" }))).toBe("]");
    expect(getKeyboardShortcutKey(key("б", { code: "Comma" }))).toBe(",");
  });
});

describe("new project shortcut", () => {
  it("maps Ctrl+N and the Russian-layout physical N key to New Project", () => {
    expect(isNewWorkspaceShortcut(key("n", { ctrlKey: true }))).toBe(true);
    expect(isNewWorkspaceShortcut(key("т", { ctrlKey: true, code: "KeyN" }))).toBe(true);
    expect(isNewWorkspaceShortcut(key("n"))).toBe(false);
    expect(isNewWorkspaceShortcut(key("n", { ctrlKey: true, shiftKey: true }))).toBe(false);
    expect(isNewWorkspaceShortcut(key("n", { ctrlKey: true, altKey: true }))).toBe(false);
  });
});

describe("Save As shortcut", () => {
  it("maps Ctrl+Shift+S and Russian physical S to Save As", () => {
    expect(isSaveAsShortcut(key("s", { ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(isSaveAsShortcut(key("ы", { ctrlKey: true, shiftKey: true, code: "KeyS" }))).toBe(true);
    expect(isSaveAsShortcut(key("s", { ctrlKey: true }))).toBe(false);
    expect(isSaveAsShortcut(key("s", { shiftKey: true }))).toBe(false);
    expect(isSaveAsShortcut(key("s", { ctrlKey: true, shiftKey: true, altKey: true }))).toBe(false);
  });

  it("matches Save only without Shift or Alt", () => {
    expect(isSaveShortcut(key("s", { ctrlKey: true }))).toBe(true);
    expect(isSaveShortcut(key("s", { ctrlKey: true, shiftKey: true }))).toBe(false);
    expect(isSaveShortcut(key("s", { ctrlKey: true, altKey: true }))).toBe(false);
  });
});

describe("global Space shortcut target handling", () => {
  it.each([
    ["checkbox", target("input", "checkbox")],
    ["radio", target("input", "radio")],
    ["button", target("button")],
    ["select", target("select")],
    ["non-control div", target("div")],
  ])("handles Space on %s as GO", (_name, element) => {
    expect(isEditableKeyboardTarget(element)).toBe(false);
    expect(shouldHandleGlobalSpaceShortcut(element, { code: "Space", key: " " })).toBe(true);
  });

  it.each([
    ["text input", target("input", "text")],
    ["search input", target("input", "search")],
    ["number input", target("input", "number")],
    ["textarea", target("textarea")],
    ["contenteditable", { isContentEditable: true } as EventTarget],
  ])("leaves Space editable in %s", (_name, element) => {
    expect(isEditableKeyboardTarget(element)).toBe(true);
    expect(shouldHandleGlobalSpaceShortcut(element, { code: "Space", key: " " })).toBe(false);
  });

  it("uses the key fallback for synthetic events without code", () => {
    expect(isSpaceShortcutEvent({ code: "", key: " " })).toBe(true);
    expect(shouldHandleGlobalSpaceShortcut(target("button"), { code: "", key: " " })).toBe(true);
    expect(isSpaceShortcutEvent({ code: "", key: "Space" })).toBe(false);
  });

  it("recognizes physical Space even when key text is unavailable", () => {
    expect(isSpaceShortcutEvent({ code: "Space", key: "" })).toBe(true);
  });

  it("keeps repeated Space events on the existing double-GO path", () => {
    expect(shouldHandleGlobalSpaceShortcut(target("button"), {
      code: "Space",
      key: " ",
      repeat: true,
    } as KeyboardEvent)).toBe(true);
  });

  it("keeps the legacy non-Space guard for inputs and editable regions", () => {
    expect(isShortcutGuardedTarget(target("input", "checkbox"))).toBe(true);
    expect(isShortcutGuardedTarget(target("input", "text"))).toBe(true);
    expect(isShortcutGuardedTarget(target("textarea"))).toBe(true);
    expect(isShortcutGuardedTarget({ isContentEditable: true } as EventTarget)).toBe(true);
    expect(isShortcutGuardedTarget(target("div"))).toBe(false);
    expect(isShortcutGuardedTarget(target("button"))).toBe(false);
    expect(isShortcutGuardedTarget(target("select"))).toBe(false);
  });
});
