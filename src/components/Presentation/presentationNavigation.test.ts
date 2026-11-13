import { describe, expect, it, vi } from "vitest";
import type { WorkspaceCueCatalogList } from "../../lib/types";
import { createPresentationPageKeyHandler, findPresentationOwner, latestActivePresentation, presentationChildren, presentationTarget, resolveLivePresentationOwner } from "./presentationNavigation";

const child = (id: string, extra: Record<string, unknown> = {}) => ({
  id, cue_type: "image", name: id, state: "standby", is_disabled: false, ...extra,
});
const presentation = (id: string, current: string | null, state = "running", children = [child("a"), child("b"), child("c")]) => ({
  id, cue_type: "presentation", name: id, state, is_disabled: false,
  presentation_current_child_id: current, children,
});

describe("presentation navigation", () => {
  it("finds a parent from itself or a nested imported video child", () => {
    const video = child("video", { cue_type: "video", presentation_video_imported: true });
    const owner = presentation("deck", "page", "running", [child("page"), video]);
    expect(findPresentationOwner([owner], "deck")?.id).toBe("deck");
    expect(findPresentationOwner([owner], "video")?.id).toBe("deck");
  });

  it("steps relative to the runtime current child, skips disabled runtime children, and clamps at bounds", () => {
    const owner = presentation("deck", "b", "paused", [child("a"), child("disabled", { is_disabled: true }), child("b"), child("c")]);
    expect(presentationTarget(owner, "previous")?.id).toBe("a");
    expect(presentationTarget(owner, "next")?.id).toBe("c");
    expect(presentationTarget(presentation("first", "a"), "previous")).toBeNull();
    expect(presentationTarget(presentation("last", "c"), "next")).toBeNull();
    expect(presentationTarget(presentation("unknown", null), "next")).toBeNull();
  });

  it("steps from a disabled runtime current child and accepts non-media runtime children", () => {
    const owner = presentation("deck", "disabled", "running", [
      child("before"), child("disabled", { is_disabled: true }), child("wait", { cue_type: "wait" }), child("after"),
    ]);
    expect(presentationTarget(owner, "previous")?.id).toBe("before");
    expect(presentationTarget(owner, "next")?.id).toBe("wait");
  });

  it("keeps disabled media available in offline preview browsing", () => {
    const owner = presentation("deck", "a", "standby", [child("a"), child("disabled", { is_disabled: true }), child("audio", { cue_type: "audio" })]);
    expect(presentationChildren(owner, false, true).map((item) => item.id)).toEqual(["a"]);
    expect(presentationChildren(owner, true, true).map((item) => item.id)).toEqual(["a", "disabled"]);
  });

  it("targets the most recently started active presentation across lists, with stable catalog fallback", () => {
    const lists = [{ id: "one", name: "One", cues: [presentation("older", "a")] }, { id: "two", name: "Two", cues: [presentation("newer", "a", "paused")] }] as unknown as WorkspaceCueCatalogList[];
    expect(latestActivePresentation(lists, { older: 8, newer: 9 })?.id).toBe("newer");
    expect(latestActivePresentation(lists, {})?.id).toBe("newer");
  });

  it("uses the current media's owner first and falls back to an active presentation only with no live media", () => {
    const lists = [{ id: "one", name: "One", cues: [presentation("deck", "page", "running", [child("page")]), child("other-video", { cue_type: "video" })] }] as unknown as WorkspaceCueCatalogList[];
    expect(resolveLivePresentationOwner("page", lists, {})?.id).toBe("deck");
    expect(resolveLivePresentationOwner("other-video", lists, {})?.id).toBeUndefined();
    expect(resolveLivePresentationOwner(undefined, lists, {})?.id).toBe("deck");
  });

  it("ignores inactive, current-less, and background presentations", () => {
    const cues = [presentation("stopped", "a", "standby"), presentation("empty", null), { ...presentation("background", "a"), background_screensaver: true }];
    const lists = [{ id: "one", name: "One", cues }] as unknown as WorkspaceCueCatalogList[];
    expect(latestActivePresentation(lists, {})).toBeNull();
  });

  it("handles Pg keys in input focus, rejects modifiers and repeats, and leaves page scroll alone with no target", () => {
    const step = vi.fn(async () => {});
    const handler = createPresentationPageKeyHandler({ getTarget: () => ({ id: "active" }), step, onRefresh: vi.fn(), onError: vi.fn() });
    const event = (overrides: Record<string, unknown> = {}) => ({
      key: "PageDown", repeat: false, ctrlKey: false, altKey: false, metaKey: false, shiftKey: false,
      target: { tagName: "INPUT" }, preventDefault: vi.fn(), stopPropagation: vi.fn(), ...overrides,
    });
    const inputKey = event();
    handler(inputKey);
    expect(inputKey.preventDefault).toHaveBeenCalledOnce();
    expect(step).toHaveBeenCalledWith("active", "next");
    const repeat = event({ repeat: true });
    handler(repeat);
    expect(repeat.preventDefault).toHaveBeenCalledOnce();
    expect(step).toHaveBeenCalledTimes(1);
    const modified = event({ ctrlKey: true });
    handler(modified);
    expect(modified.preventDefault).not.toHaveBeenCalled();
    const noTarget = event({ key: "PageUp" });
    createPresentationPageKeyHandler({ getTarget: () => null, step, onRefresh: vi.fn(), onError: vi.fn() })(noTarget);
    expect(noTarget.preventDefault).not.toHaveBeenCalled();
  });

  it("dispatches distinct key presses while an earlier RPC is pending and reports failures", async () => {
    const resolvers: Array<(value: void) => void> = [];
    const calls: Array<[string, "previous" | "next"]> = [];
    let target = { id: "first" };
    const onError = vi.fn();
    const onRefresh = vi.fn();
    const handler = createPresentationPageKeyHandler({
      getTarget: () => target,
      step: (id, direction) => {
        calls.push([id, direction]);
        return new Promise<void>((resolve) => resolvers.push(resolve));
      },
      onRefresh,
      onError,
    });
    const event = (key: string) => ({ key, repeat: false, ctrlKey: false, altKey: false, metaKey: false, shiftKey: false, preventDefault() {}, stopPropagation() {} });
    handler(event("PageDown"));
    target = { id: "second" };
    handler(event("PageUp"));
    expect(calls).toEqual([["first", "next"], ["second", "previous"]]);
    resolvers[0]();
    resolvers[1]();
    await Promise.resolve();
    expect(onRefresh).toHaveBeenCalledTimes(2);

    const failing = createPresentationPageKeyHandler({
      getTarget: () => ({ id: "bad" }), step: async () => { throw new Error("backend rejected"); },
      onRefresh: vi.fn(), onError,
    });
    failing(event("PageDown"));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(onError).toHaveBeenCalledWith(expect.objectContaining({ message: "backend rejected" }));
  });
});
