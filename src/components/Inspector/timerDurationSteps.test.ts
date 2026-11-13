import { describe, expect, it } from "vitest";
import { applyTimerDurationDelta, createTimerDurationSaveQueue, enqueueTimerDurationSave } from "./timerDurationSteps";

describe("timer duration quick steps", () => {
  it("applies queued increments from the latest value", () => {
    let value = 60_000;
    value = applyTimerDurationDelta(value, 10_000);
    value = applyTimerDurationDelta(value, 10_000);
    value = applyTimerDurationDelta(value, 10_000);
    expect(value).toBe(90_000);
  });

  it("clamps repeated decrements at zero", () => {
    let value = 60_000;
    value = applyTimerDurationDelta(value, -60_000);
    value = applyTimerDurationDelta(value, -60_000);
    expect(value).toBe(0);
  });

  it("serializes delayed rapid saves with each click's captured duration", async () => {
    const queue = createTimerDurationSaveQueue("timer-a", 60_000);
    const saved: number[] = [];
    let unblock!: () => void;
    let entered!: () => void;
    const enteredFirstSave = new Promise<void>((resolve) => { entered = resolve; });
    const firstSaveGate = new Promise<void>((resolve) => { unblock = resolve; });
    const save = async (durationMs: number) => {
      saved.push(durationMs);
      if (saved.length === 1) { entered(); await firstSaveGate; }
    };

    enqueueTimerDurationSave(queue, applyTimerDurationDelta(queue.value, 10_000), save);
    enqueueTimerDurationSave(queue, applyTimerDurationDelta(queue.value, 10_000), save);
    enqueueTimerDurationSave(queue, applyTimerDurationDelta(queue.value, 10_000), save);
    await enteredFirstSave;
    expect(queue.value).toBe(90_000);
    unblock();
    await queue.queue;
    expect(saved).toEqual([70_000, 80_000, 90_000]);
  });

  it("keeps queued writes isolated when the selected cue changes", async () => {
    const firstCue = createTimerDurationSaveQueue("timer-a", 60_000);
    const nextCue = createTimerDurationSaveQueue("timer-b", 20_000);
    const firstSaved: number[] = [];
    const nextSaved: number[] = [];
    enqueueTimerDurationSave(firstCue, 70_000, async (ms) => { firstSaved.push(ms); });
    enqueueTimerDurationSave(nextCue, 30_000, async (ms) => { nextSaved.push(ms); });
    await Promise.all([firstCue.queue, nextCue.queue]);
    expect(firstSaved).toEqual([70_000]);
    expect(nextSaved).toEqual([30_000]);
  });
});
