import { describe, expect, it } from "vitest";
import { LOOP_INFINITE, loopCountAfterInfiniteToggle, loopCountAfterToggle } from "./loopModel";

describe("media loop controls", () => {
  it("enables infinite looping on the first Loop toggle", () => {
    expect(loopCountAfterToggle(true)).toBe(LOOP_INFINITE);
    expect(loopCountAfterToggle(false)).toBe(0);
  });

  it("switches the infinity control to one finite repeat and back", () => {
    expect(loopCountAfterInfiniteToggle(LOOP_INFINITE)).toBe(1);
    expect(loopCountAfterInfiniteToggle(1)).toBe(LOOP_INFINITE);
    expect(loopCountAfterInfiniteToggle(4)).toBe(LOOP_INFINITE);
  });
});
