import { describe, expect, it, vi } from "vitest";
import { prepareBackgroundSelection } from "./backgroundSelection";

describe("background cue selection", () => {
  it("blurs the active editor before preventing the mousedown default", () => {
    const order: string[] = [];
    const target = { closest: vi.fn(() => null) } as unknown as HTMLElement;
    const activeElement = { blur: () => order.push("blur") } as unknown as HTMLElement;

    expect(prepareBackgroundSelection(target, activeElement, () => order.push("preventDefault"))).toBe(true);
    expect(order).toEqual(["blur", "preventDefault"]);
  });

  it("leaves interactive targets alone", () => {
    const preventDefault = vi.fn();
    const target = {
      closest: (selector: string) => selector.includes("input") ? {} : null,
    } as unknown as HTMLElement;
    const activeElement = { blur: vi.fn() } as unknown as HTMLElement;

    expect(prepareBackgroundSelection(target, activeElement, preventDefault)).toBe(false);
    expect(activeElement.blur).not.toHaveBeenCalled();
    expect(preventDefault).not.toHaveBeenCalled();
  });
});
