import { describe, expect, it } from "vitest";
import {
  isCueListShortcutBlocked,
  consumeCueListStopShortcut,
  consumeCueListStopShortcutEvent,
  isStopSelectionShortcut,
  moveCueSelectionByArrow,
  stopSelectedCueSet,
  topLevelSelectedStopIds,
  type KeyboardCueNode,
} from "./cueListKeyboard";

const cues: KeyboardCueNode[] = [
  { id: "a", cue_type: "audio" },
  {
    id: "group", cue_type: "group", children: [
      { id: "b", cue_type: "video" },
      { id: "nested", cue_type: "group", children: [{ id: "c", cue_type: "audio" }] },
    ],
  },
  { id: "number", cue_type: "number", children: [{ id: "number-child", cue_type: "light" }] },
  { id: "d", cue_type: "audio" },
];

describe("cue-list keyboard navigation", () => {
  it("moves plain Up/Down one row and collapses a multi-selection", () => {
    expect(moveCueSelectionByArrow(["a", "b", "d"], "b", "b", "a", "down", false, cues))
      .toEqual({ selectedCueId: "d", selectedCueIds: ["d"], anchorCueId: "d", endCueId: "d" });
    expect(moveCueSelectionByArrow(["a", "b", "d"], "b", "b", "a", "up", false, cues))
      .toEqual({ selectedCueId: "a", selectedCueIds: ["a"], anchorCueId: "a", endCueId: "a" });
  });

  it("extends and shrinks a Shift+arrow range from its fixed anchor", () => {
    expect(moveCueSelectionByArrow(["a", "b", "d"], "b", "b", "a", "down", true, cues))
      .toEqual({ selectedCueId: "d", selectedCueIds: ["a", "b", "d"], anchorCueId: "a", endCueId: "d" });
    expect(moveCueSelectionByArrow(["a", "b", "d"], "d", "d", "a", "up", true, cues))
      .toEqual({ selectedCueId: "b", selectedCueIds: ["a", "b"], anchorCueId: "a", endCueId: "b" });
  });

  it("uses the visible parent group when a selected child is hidden", () => {
    expect(moveCueSelectionByArrow(["a", "group", "d"], "c", "c", "c", "down", false, cues))
      .toEqual({ selectedCueId: "d", selectedCueIds: ["d"], anchorCueId: "d", endCueId: "d" });
  });

  it("does nothing at the ends of the visible list or with no primary selection", () => {
    expect(moveCueSelectionByArrow(["a", "b"], "a", "a", "a", "up", false, cues)).toBeNull();
    expect(moveCueSelectionByArrow(["a", "b"], null, null, null, "down", false, cues)).toBeNull();
  });
});

describe("selected cue stop targets", () => {
  it("keeps selected nested cues but avoids stopping a child twice when its group is selected", () => {
    expect(topLevelSelectedStopIds(["c", "a", "group", "b", "d"], cues)).toEqual(["a", "group", "d"]);
    expect(topLevelSelectedStopIds(["c", "d"], cues)).toEqual(["c", "d"]);
  });

  it("treats Number as a container when stopping selected descendants", () => {
    expect(topLevelSelectedStopIds(["number", "number-child"], cues)).toEqual(["number"]);
  });

  it("dispatches the full selected set without waiting between stops", async () => {
    const dispatched: string[] = [];
    await stopSelectedCueSet(["b", "d"], cues, async (id) => { dispatched.push(id); });
    expect(dispatched).toEqual(["b", "d"]);
  });
});

describe("cue-list shortcut guards", () => {
  it("blocks text fields, editable content, and modal contexts", () => {
    expect(isCueListShortcutBlocked({ tagName: "INPUT" } as EventTarget, false)).toBe(true);
    expect(isCueListShortcutBlocked({ isContentEditable: true } as EventTarget, false)).toBe(true);
    expect(isCueListShortcutBlocked({ closest: (selector) => selector.includes('[role="dialog"]') } as unknown as EventTarget, false)).toBe(true);
    expect(isCueListShortcutBlocked({ tagName: "BUTTON" } as EventTarget, true)).toBe(true);
  });

  it("allows regular non-modal controls", () => {
    expect(isCueListShortcutBlocked({ tagName: "BUTTON" } as EventTarget, false)).toBe(false);
  });
});

describe("cue-list stop shortcut", () => {
  it("consumes the stop key event before the legacy window shortcut can see it", () => {
    const event = { preventDefault: () => {}, stopPropagation: () => {} };
    let defaultPrevented = false;
    let bubblingStopped = false;
    event.preventDefault = () => { defaultPrevented = true; };
    event.stopPropagation = () => { bubblingStopped = true; };
    consumeCueListStopShortcutEvent(event);
    expect(defaultPrevented).toBe(true);
    expect(bubblingStopped).toBe(true);
  });

  it("accepts S and Russian-layout Ы while leaving modified save shortcuts alone", () => {
    const key = (value: string, modifiers = {}) => ({ key: value, ctrlKey: false, metaKey: false, altKey: false, ...modifiers });
    expect(isStopSelectionShortcut(key("s"))).toBe(true);
    expect(isStopSelectionShortcut(key("Ы"))).toBe(true);
    expect(isStopSelectionShortcut(key("s", { ctrlKey: true }))).toBe(false);
  });

  it("accepts the physical S key when the Russian layout reports Ы", () => {
    expect(isStopSelectionShortcut({ key: "ы", code: "KeyS", ctrlKey: false, metaKey: false, altKey: false })).toBe(true);
    expect(isStopSelectionShortcut({ key: "ы", code: "KeyS", ctrlKey: true, metaKey: false, altKey: false })).toBe(false);
  });

  it("consumes repeated S keydowns without dispatching another stop", () => {
    let defaultPrevented = false;
    let bubblingStopped = false;
    const repeatEvent = {
      key: "s",
      ctrlKey: false,
      metaKey: false,
      altKey: false,
      repeat: true,
      preventDefault: () => { defaultPrevented = true; },
      stopPropagation: () => { bubblingStopped = true; },
    };

    expect(consumeCueListStopShortcut(repeatEvent, true)).toBe(false);
    expect(defaultPrevented).toBe(true);
    expect(bubblingStopped).toBe(true);
    expect(consumeCueListStopShortcut(repeatEvent, false)).toBe(false);
  });
});
