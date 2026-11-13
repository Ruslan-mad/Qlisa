export function applyTimerDurationDelta(currentMs: number, deltaMs: number): number {
  return Math.max(0, Math.min(86_400_000, currentMs + deltaMs));
}

export interface TimerDurationSaveQueue {
  cueId: string;
  value: number;
  pending: number;
  queue: Promise<void>;
}

export function createTimerDurationSaveQueue(cueId: string, initialMs: number): TimerDurationSaveQueue {
  return { cueId, value: initialMs, pending: 0, queue: Promise.resolve() };
}

export function enqueueTimerDurationSave(
  queue: TimerDurationSaveQueue,
  nextMs: number,
  save: (durationMs: number) => Promise<void>,
): void {
  const value = applyTimerDurationDelta(nextMs, 0);
  queue.value = value;
  queue.pending += 1;
  queue.queue = queue.queue.catch(() => undefined).then(() => save(value)).finally(() => {
    queue.pending = Math.max(0, queue.pending - 1);
  });
}
