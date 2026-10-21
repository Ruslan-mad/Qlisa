/** Small queue that keeps visible Cue List navigation and GO in the same order. */
let queuedPlayheadUpdate: Promise<void> = Promise.resolve();
let queuedPlayheadUpdateGeneration = 0;
let cueListContextGeneration = 0;
let pendingGoGeneration = 0;
let playheadUpdateError: unknown = null;
let contextTransitionError: unknown = null;

/** Serialize Playhead writes so GO sees the latest completed navigation. */
export function queueCueListPlayheadUpdate(update: () => Promise<void>): Promise<void> {
  const contextGeneration = cueListContextGeneration;
  queuedPlayheadUpdateGeneration += 1;
  const pending = queuedPlayheadUpdate.catch(() => undefined).then(async () => {
    // A workspace/list transition can invalidate work that has not reached IPC.
    if (contextGeneration !== cueListContextGeneration) return;
    try {
      await update();
      playheadUpdateError = null;
      contextTransitionError = null;
    } catch (error) {
      playheadUpdateError = error;
      throw error;
    }
  });
  queuedPlayheadUpdate = pending;
  return pending;
}

export function waitForCueListPlayheadUpdate(): Promise<void> {
  const pending = queuedPlayheadUpdate;
  const generation = queuedPlayheadUpdateGeneration;
  return pending.catch((error) => {
    // Consume only this completed failure. A newer update remains queued.
    if (queuedPlayheadUpdate === pending && queuedPlayheadUpdateGeneration === generation) {
      queuedPlayheadUpdate = Promise.resolve();
    }
    throw error;
  });
}

/** Place GO in the same queue as Playhead writes so later clicks cannot overtake it. */
export function queueCueListGo<T>(command: () => Promise<T>): Promise<T | undefined> {
  const generation = pendingGoGeneration;
  const contextGeneration = cueListContextGeneration;
  const pending = queuedPlayheadUpdate.catch(() => undefined).then(() => {
    if (playheadUpdateError != null) throw playheadUpdateError;
    if (contextTransitionError != null) throw contextTransitionError;
    if (generation !== pendingGoGeneration || contextGeneration !== cueListContextGeneration) return undefined;
    return command();
  });
  // GO failures must reject for the caller, but must not poison later navigation.
  queuedPlayheadUpdate = pending.then(() => undefined, () => undefined);
  return pending;
}

/** Cancel a GO that is still waiting for Cue List navigation to finish. */
export function cancelPendingCueListGo(): void {
  pendingGoGeneration += 1;
}

/** Run a workspace/list change after prior Playhead writes and invalidate old work. */
export function runCueListContextTransition<T>(command: () => Promise<T>): Promise<T> {
  cueListContextGeneration += 1;
  cancelPendingCueListGo();
  const transition = queuedPlayheadUpdate.catch(() => undefined).then(command);
  queuedPlayheadUpdate = transition.then(
    () => {
      contextTransitionError = null;
      playheadUpdateError = null;
    },
    (error) => {
      contextTransitionError = error;
      throw error;
    },
  );
  // The queue retains its rejection for GO, while the command caller receives
  // the original result or error from `transition`.
  void queuedPlayheadUpdate.catch(() => undefined);
  return transition;
}
