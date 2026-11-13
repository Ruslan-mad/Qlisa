import { afterEach, describe, expect, it, vi } from "vitest";
import { GeometryLiveQueue } from "./geometryLiveQueue";

describe("GeometryLiveQueue", () => {
  afterEach(() => vi.useRealTimers());

  it("merges rapid drag updates and flushes only the latest patch at release", async () => {
    vi.useFakeTimers();
    const queue = new GeometryLiveQueue();
    const submit = vi.fn();
    queue.enqueue("cue:main:linked", { geometry: { pan_x: 0.1 } }, submit);
    queue.enqueue("cue:main:linked", { geometry: { pan_y: 0.2 } }, submit);
    queue.enqueue("cue:main:linked", { geometry: { pan_x: 0.4, scale: 1.2 } }, submit);

    const flushed = queue.flush("cue:main:linked");
    await vi.advanceTimersByTimeAsync(0);
    await flushed;

    expect(submit).toHaveBeenCalledTimes(1);
    expect(submit).toHaveBeenCalledWith({ geometry: { pan_x: 0.4, pan_y: 0.2, scale: 1.2 } });
    expect(queue.hasPending).toBe(false);
  });

  it("preserves an explicit empty output map when the linked mode clears overrides", async () => {
    vi.useFakeTimers();
    const queue = new GeometryLiveQueue();
    const submit = vi.fn();
    queue.enqueue("cue:main", { geometry_by_output: { main: { pan_x: 0.4 } } }, submit);
    queue.enqueue("cue:main", { geometry: { pan_x: 0.2 }, geometry_by_output: {} }, submit, true);
    const flushed = queue.flush("cue:main");
    await vi.advanceTimersByTimeAsync(0);
    await flushed;
    expect(submit).toHaveBeenCalledWith({ geometry: { pan_x: 0.2 }, geometry_by_output: {} });
  });

  it("releases waiters and continues after a failed submit", async () => {
    vi.useFakeTimers();
    const queue = new GeometryLiveQueue();
    const rejected = vi.fn(async () => { throw new Error("save failed"); });
    const accepted = vi.fn();
    queue.enqueue("cue-a:main", { geometry: { pan_x: 0.1 } }, rejected, true);
    const first = queue.flush("cue-a:main");
    await vi.advanceTimersByTimeAsync(0);
    await first;
    queue.enqueue("cue-b:main", { geometry: { pan_y: 0.2 } }, accepted, true);
    const second = queue.flush("cue-b:main");
    await vi.advanceTimersByTimeAsync(0);
    await second;
    expect(accepted).toHaveBeenCalledTimes(1);
  });

  it("keeps a new context behind an in-flight save and preserves its captured submit target", async () => {
    vi.useFakeTimers();
    const queue = new GeometryLiveQueue();
    let releaseFirst!: () => void;
    const firstGate = new Promise<void>((resolve) => { releaseFirst = resolve; });
    const submissions: string[] = [];
    queue.enqueue("cue-a:main", { geometry: { pan_x: 0.2 } }, async () => {
      submissions.push("cue-a:main");
      await firstGate;
    }, true);
    const firstFlush = queue.flush("cue-a:main");
    await vi.advanceTimersByTimeAsync(0);

    queue.enqueue("cue-b:projector", { geometry_by_output: { projector: { pan_y: 0.3 } } }, async () => {
      submissions.push("cue-b:projector");
    }, true);
    const secondFlush = queue.flush("cue-b:projector");
    releaseFirst();
    await vi.advanceTimersByTimeAsync(0);
    await Promise.all([firstFlush, secondFlush]);

    expect(submissions).toEqual(["cue-a:main", "cue-b:projector"]);
    expect(queue.hasPending).toBe(false);
  });
});
