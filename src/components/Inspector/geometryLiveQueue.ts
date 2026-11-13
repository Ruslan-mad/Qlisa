import type { VideoGeometry } from "../../lib/types";

export type LiveGeometryPatch = {
  geometry?: Partial<VideoGeometry>;
  geometry_by_output?: Record<string, Partial<VideoGeometry>>;
};

type PendingGeometry = {
  patch: LiveGeometryPatch;
  submit: (patch: LiveGeometryPatch) => void | Promise<void>;
  force: boolean;
};

/** One in-flight save and one merged latest patch per captured cue/output target. */
export class GeometryLiveQueue {
  private readonly pending = new Map<string, PendingGeometry>();
  private readonly waiters = new Map<string, Array<() => void>>();
  private inFlightTarget: string | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private lastDispatch = 0;

  enqueue(
    target: string,
    patch: LiveGeometryPatch,
    submit: (patch: LiveGeometryPatch) => void | Promise<void>,
    final = false,
  ): void {
    const current = this.pending.get(target);
    this.pending.set(target, {
      patch: mergeLiveGeometryPatch(current?.patch, patch),
      submit,
      force: final || current?.force === true,
    });
    this.schedule(final);
  }

  flush(target?: string): Promise<void> {
    if (target) {
      const item = this.pending.get(target);
      if (item) item.force = true;
      if (!item && this.inFlightTarget !== target) return Promise.resolve();
      const promise = new Promise<void>((resolve) => {
        const list = this.waiters.get(target) ?? [];
        list.push(resolve);
        this.waiters.set(target, list);
      });
      this.schedule(true);
      return promise;
    }

    for (const item of this.pending.values()) item.force = true;
    const targets = new Set([...this.pending.keys(), ...(this.inFlightTarget ? [this.inFlightTarget] : [])]);
    if (targets.size === 0) return Promise.resolve();
    const promise = Promise.all([...targets].map((key) => this.flush(key))).then(() => undefined);
    this.schedule(true);
    return promise;
  }

  cancelPending(target?: string): void {
    if (target) this.pending.delete(target);
    else this.pending.clear();
    this.resolveIdleWaiters();
    if (this.pending.size === 0 && this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }

  get hasPending(): boolean {
    return this.pending.size > 0 || this.inFlightTarget !== null;
  }

  private schedule(force: boolean): void {
    if (this.inFlightTarget || this.pending.size === 0) return;
    if (this.timer) {
      if (!force) return;
      clearTimeout(this.timer);
      this.timer = null;
    }
    const next = [...this.pending.entries()].find(([, item]) => item.force);
    const delay = force || next ? 0 : Math.max(0, 34 - (Date.now() - this.lastDispatch));
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.drain();
    }, delay);
  }

  private async drain(): Promise<void> {
    if (this.inFlightTarget || this.pending.size === 0) return;
    const entry = this.pending.entries().next().value as [string, PendingGeometry] | undefined;
    if (!entry) return;
    const [target, item] = entry;
    this.pending.delete(target);
    this.inFlightTarget = target;
    this.lastDispatch = Date.now();
    try {
      await item.submit(item.patch);
    } catch {
      // Inspector save reports the failure; the queue must still release waiters and drain later edits.
    } finally {
      this.inFlightTarget = null;
      this.resolveTargetWaiters(target);
      this.resolveIdleWaiters();
      if (this.pending.size > 0) this.schedule([...this.pending.values()].some((pending) => pending.force));
    }
  }

  private resolveTargetWaiters(target: string): void {
    if (this.pending.has(target) || this.inFlightTarget === target) return;
    const list = this.waiters.get(target);
    this.waiters.delete(target);
    list?.forEach((resolve) => resolve());
  }

  private resolveIdleWaiters(): void {
    for (const target of [...this.waiters.keys()]) this.resolveTargetWaiters(target);
  }
}

function mergeLiveGeometryPatch(current: LiveGeometryPatch | undefined, next: LiveGeometryPatch): LiveGeometryPatch {
  const merged: LiveGeometryPatch = { ...current, ...next };
  if (current?.geometry || next.geometry) merged.geometry = { ...current?.geometry, ...next.geometry };
  if (next.geometry_by_output !== undefined) merged.geometry_by_output = next.geometry_by_output;
  return merged;
}
