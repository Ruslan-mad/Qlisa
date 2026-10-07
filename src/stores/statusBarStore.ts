import { create } from "zustand";
import { getStatusRuntimeSnapshot, getSystemStatus } from "../lib/commands";
import type { StatusRuntimeSnapshot, SystemStatusSnapshot } from "../lib/types";

interface StatusBarState {
  system: SystemStatusSnapshot | null;
  runtime: StatusRuntimeSnapshot | null;
  setSystem: (snapshot: SystemStatusSnapshot) => void;
  setRuntime: (snapshot: StatusRuntimeSnapshot) => void;
}

export const useStatusBarStore = create<StatusBarState>((set) => ({
  system: null,
  runtime: null,
  setSystem: (system) => set({ system }),
  setRuntime: (runtime) => set({ runtime }),
}));

type PollNeeds = { system: boolean; runtime: boolean };
const pollUsers = new Map<number, PollNeeds>();
let nextPollUser = 1;
let pollTimer: number | null = null;
const pollInFlight: Record<keyof PollNeeds, boolean> = { system: false, runtime: false };
const pollGeneration: Record<keyof PollNeeds, number> = { system: 0, runtime: 0 };

function needs(kind: keyof PollNeeds) {
  return [...pollUsers.values()].some((requested) => requested[kind]);
}

async function poll(kind: keyof PollNeeds) {
  if (pollInFlight[kind] || !needs(kind)) return;
  pollInFlight[kind] = true;
  const generation = pollGeneration[kind];
  try {
    if (kind === "system") {
      const snapshot = await getSystemStatus();
      if (generation === pollGeneration.system && needs("system")) useStatusBarStore.getState().setSystem(snapshot);
    } else {
      const snapshot = await getStatusRuntimeSnapshot();
      if (generation === pollGeneration.runtime && needs("runtime")) useStatusBarStore.getState().setRuntime(snapshot);
    }
  } catch {
    // The last cached snapshot remains visible and ages into the stale state.
  } finally {
    pollInFlight[kind] = false;
  }
}

function reconcilePolling() {
  const any = pollUsers.size > 0;
  if (!any) {
    if (pollTimer != null) window.clearInterval(pollTimer);
    pollTimer = null;
    return;
  }
  if (pollTimer == null) pollTimer = window.setInterval(() => {
    if (needs("system")) void poll("system");
    if (needs("runtime")) void poll("runtime");
  }, 1000);
  if (needs("system")) void poll("system");
  if (needs("runtime")) void poll("runtime");
}

/** Shared 1Hz snapshot polling. Callers request only the data families they render. */
export function acquireStatusBarPolling(requested: PollNeeds): () => void {
  const userId = nextPollUser++;
  pollUsers.set(userId, requested);
  reconcilePolling();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    pollUsers.delete(userId);
    if (!needs("system")) pollGeneration.system += 1;
    if (!needs("runtime")) pollGeneration.runtime += 1;
    reconcilePolling();
  };
}
