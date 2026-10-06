import { describe, expect, it } from "vitest";
import { commitIfCurrent, createCatalogRequestGuard } from "./catalogRequestGuard";

describe("catalog request guard", () => {
  it("rejects an older catalog response after a newer request completes", async () => {
    const guard = createCatalogRequestGuard();
    let resolveOld!: (value: string) => void;
    const oldResponse = new Promise<string>((resolve) => { resolveOld = resolve; });
    const oldGeneration = guard.begin();
    const newGeneration = guard.begin();
    const applied: string[] = [];
    const newValue = await Promise.resolve("fresh");
    commitIfCurrent(guard, newGeneration, newValue, (value) => applied.push(value));
    resolveOld("stale");
    const oldValue = await oldResponse;
    commitIfCurrent(guard, oldGeneration, oldValue, (value) => applied.push(value));
    expect(applied).toEqual(["fresh"]);
  });

  it("invalidates a pending snapshot after a runtime state event", () => {
    const guard = createCatalogRequestGuard();
    const pending = guard.begin();
    guard.invalidate();
    expect(guard.isCurrent(pending)).toBe(false);
  });
});
