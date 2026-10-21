import { describe, expect, it } from "vitest";
import { cancelPendingCueListGo, queueCueListGo, runCueListContextTransition } from "../../lib/cueListGoQueue";
import {
  isCueListShortcutBlocked,
  consumeCueListStopShortcut,
  consumeCueListStopShortcutEvent,
  isStopSelectionShortcut,
  moveCueSelectionByArrow,
  queueCueListPlayheadUpdate,
  stopSelectedCueSet,
  topLevelSelectedStopIds,
  waitForCueListPlayheadUpdate,
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
  it("queues Playhead updates in key order so GO can await the final selection", async () => {
    const order: string[] = [];
    let releaseFirst!: () => void;
    const first = queueCueListPlayheadUpdate(() => new Promise<void>((resolve) => {
      releaseFirst = () => { order.push("first"); resolve(); };
    }));
    const second = queueCueListPlayheadUpdate(async () => { order.push("second"); });
    const waiting = waitForCueListPlayheadUpdate();
    expect(order).toEqual([]);
    await new Promise((resolve) => setTimeout(resolve, 0));
    releaseFirst();
    await Promise.all([first, second, waiting]);
    expect(order).toEqual(["first", "second"]);
  });

  it("rejects GO for a failed arrow update, then clears the failure for a later Playhead placement", async () => {
    const failure = new Error("Playhead update failed");
    const settledFailure = queueCueListPlayheadUpdate(async () => { throw failure; });
    await expect(settledFailure).rejects.toBe(failure);
    // The backend failure has already settled before the first GO checks it.
    await expect(waitForCueListPlayheadUpdate()).rejects.toBe(failure);
    // Manual row clicks use the normal setPlayhead path and must not inherit a
    // stale rejection from the previous arrow navigation.
    await expect(queueCueListPlayheadUpdate(async () => {})).resolves.toBeUndefined();
    await expect(waitForCueListPlayheadUpdate()).resolves.toBeUndefined();
  });

  it("keeps GO blocked after a failed Playhead write until a later write succeeds", async () => {
    await queueCueListPlayheadUpdate(async () => {});
    const failure = new Error("mouse Playhead update failed");
    await expect(queueCueListPlayheadUpdate(async () => { throw failure; })).rejects.toBe(failure);
    await expect(queueCueListGo(async () => {})).rejects.toBe(failure);
    // The queue's completed rejection is consumed, but GO still cannot use the
    // old Playhead until a new positioning succeeds.
    await expect(queueCueListGo(async () => {})).rejects.toBe(failure);
    await queueCueListPlayheadUpdate(async () => {});
    await expect(queueCueListGo(async () => "go")).resolves.toBe("go");
  });

  it("cancels a waiting GO without cancelling its Playhead update", async () => {
    await queueCueListPlayheadUpdate(async () => {});
    let finish!: () => void;
    const update = queueCueListPlayheadUpdate(() => new Promise<void>((resolve) => { finish = resolve; }));
    const pendingGo = queueCueListGo(async () => "go");
    cancelPendingCueListGo();
    await new Promise((resolve) => setTimeout(resolve, 0));
    finish();
    await update;
    await expect(pendingGo).resolves.toBeUndefined();
    await expect(queueCueListGo(async () => "go")).resolves.toBe("go");
  });

  it("drops unsent old Playhead writes and propagates workspace transition failure", async () => {
    const order: string[] = [];
    const oldWrite = queueCueListPlayheadUpdate(async () => { order.push("old-playhead"); });
    const failure = new Error("workspace load failed");
    const transition = runCueListContextTransition(async () => {
      order.push("load");
      throw failure;
    });
    await expect(oldWrite).resolves.toBeUndefined();
    await expect(transition).rejects.toBe(failure);
    expect(order).toEqual(["load"]);
    await expect(queueCueListGo(async () => {})).rejects.toBe(failure);
    await queueCueListPlayheadUpdate(async () => { order.push("new-playhead"); });
    await expect(queueCueListGo(async () => "go")).resolves.toBe("go");
    expect(order).toEqual(["load", "new-playhead"]);
  });

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

  it("does nothing at the ends and starts from the first visible cue without a selection", () => {
    expect(moveCueSelectionByArrow(["a", "b"], "a", "a", "a", "up", false, cues)).toBeNull();
    expect(moveCueSelectionByArrow(["a", "b"], null, null, null, "down", false, cues))
      .toEqual({ selectedCueId: "a", selectedCueIds: ["a"], anchorCueId: "a", endCueId: "a" });
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
