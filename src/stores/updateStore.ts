// Qlisa's updater stays disabled until it has its own signed release feed.
//
// One store so the startup check, the About dialog button and the update
// dialog all share the same state machine:
//   idle → checking → available → downloading → ready-to-relaunch
//                   ↘ up-to-date / error

import { create } from "zustand";

export type UpdateStatus =
  | "idle"
  | "checking"
  | "up-to-date"
  | "available"
  | "downloading"
  | "installing"
  | "error";

interface UpdateState {
  status: UpdateStatus;
  version: string | null;
  notes: string | null;
  /** Download progress 0–1, or null while the total size is unknown. */
  progress: number | null;
  error: string | null;
  currentVersion: string;
  canInstall: () => boolean;
  setInstallGuard: (guard: () => boolean) => void;
  /** True once the user dismissed the dialog for this session. */
  dismissed: boolean;

  checkForUpdates: (opts?: { silent?: boolean }) => Promise<void>;
  downloadAndInstall: () => Promise<void>;
  dismiss: () => void;
}

export const useUpdateStore = create<UpdateState>((set) => ({
  status: "idle",
  version: null,
  notes: null,
  currentVersion: import.meta.env.VITE_APP_VERSION,
  // Do not permit an install until the workspace guard has been registered.
  canInstall: () => false,
  setInstallGuard: (guard) => set({ canInstall: guard }),
  progress: null,
  error: null,
  dismissed: false,

  checkForUpdates: async ({ silent = false } = {}) => {
    if (silent) return;
    set({ status: "error", error: "updates.notConfigured", dismissed: false });
  },

  downloadAndInstall: async () => {
    if (!useUpdateStore.getState().canInstall()) {
      set({ status: "error", error: "updates.activeCuesRunning" });
      return;
    }
    set({ status: "error", error: "updates.notConfigured" });
  },

  dismiss: () => set({ dismissed: true }),
}));
