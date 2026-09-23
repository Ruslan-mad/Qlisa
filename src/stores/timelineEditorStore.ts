import { create } from "zustand";

const STORAGE_KEY = "qlisa.timeline-edit-locked";

function readInitialLock(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

interface TimelineEditorState {
  locked: boolean;
  setLocked: (locked: boolean) => void;
  toggle: () => void;
}

export const useTimelineEditorStore = create<TimelineEditorState>((set) => ({
  locked: readInitialLock(),
  setLocked: (locked) => {
    try { window.localStorage.setItem(STORAGE_KEY, locked ? "1" : "0"); } catch { /* storage is optional */ }
    set({ locked });
  },
  toggle: () => set((state) => {
    const locked = !state.locked;
    try { window.localStorage.setItem(STORAGE_KEY, locked ? "1" : "0"); } catch { /* storage is optional */ }
    return { locked };
  }),
}));

