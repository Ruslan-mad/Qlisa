import { describe, expect, it } from "vitest";
import { useNumberPreviewStore } from "../numberPreviewStore";

describe("numberPreviewStore", () => {
  it("keeps one clamped cursor shared by timeline and inspector", () => {
    useNumberPreviewStore.getState().clear();
    useNumberPreviewStore.getState().select("number-a", 5000);
    useNumberPreviewStore.getState().setPosition("number-a", 7200, 5000);
    expect(useNumberPreviewStore.getState()).toMatchObject({ numberId: "number-a", positionMs: 5000 });
  });

  it("starts a new Number at zero without affecting the previous cursor", () => {
    useNumberPreviewStore.getState().select("number-a", 5000);
    useNumberPreviewStore.getState().setPosition("number-a", 1300, 5000);
    useNumberPreviewStore.getState().select("number-b", 9000);
    expect(useNumberPreviewStore.getState()).toMatchObject({ numberId: "number-b", positionMs: 0 });
  });

  it("shares play and frame-step state with the inspector controls", () => {
    const store = useNumberPreviewStore.getState();
    store.select("number-a", 1000);
    store.setPosition("number-a", 500, 1000);
    store.toggle("number-a", 1000);
    expect(useNumberPreviewStore.getState()).toMatchObject({ playing: true, positionMs: 500 });
    store.stepFrame("number-a", 1, 30, 1000);
    expect(useNumberPreviewStore.getState().playing).toBe(false);
    expect(useNumberPreviewStore.getState().positionMs).toBeCloseTo(533.333, 2);
  });
});
