import { isTauri } from "@tauri-apps/api/core";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { create } from "zustand";

export type UpdateStatus =
  | "idle"
  | "checking"
  | "up-to-date"
  | "available"
  | "downloading"
  | "installing"
  | "error";

export type UpdateInstallBlock = "activeCuesRunning" | "unsavedWorkspace" | null;

interface UpdateState {
  status: UpdateStatus;
  version: string | null;
  notes: string | null;
  /** Download progress 0–1, or null while the total size is unknown. */
  progress: number | null;
  error: string | null;
  currentVersion: string;
  getInstallBlock: () => UpdateInstallBlock;
  setInstallGuard: (guard: () => UpdateInstallBlock) => void;
  /** True once the user dismissed the dialog for this session. */
  dismissed: boolean;

  checkForUpdates: (opts?: { silent?: boolean }) => Promise<void>;
  downloadAndInstall: () => Promise<void>;
  dismiss: () => void;
}

let pendingUpdate: Update | null = null;
let pendingUpdateDownloaded = false;

export const useUpdateStore = create<UpdateState>((set, get) => ({
  status: "idle",
  version: null,
  notes: null,
  currentVersion: import.meta.env.VITE_APP_VERSION,
  getInstallBlock: () => "unsavedWorkspace",
  setInstallGuard: (guard) => set({ getInstallBlock: guard }),
  progress: null,
  error: null,
  dismissed: false,

  checkForUpdates: async ({ silent = false } = {}) => {
    if (get().status === "checking" || get().status === "downloading" || get().status === "installing") return;
    if (!isTauri()) {
      if (!silent) set({ status: "error", error: "updates.localBuildUnavailable", dismissed: false });
      return;
    }

    await pendingUpdate?.close().catch(() => {});
    pendingUpdate = null;
    pendingUpdateDownloaded = false;
    set({
      status: "checking",
      version: null,
      notes: null,
      progress: null,
      error: null,
      dismissed: silent,
    });

    try {
      const update = await check({ timeout: 15_000 });
      if (!update) {
        set({ status: "up-to-date", dismissed: silent });
        return;
      }
      pendingUpdate = update;
      set({
        status: "available",
        version: update.version,
        notes: update.body ?? null,
        dismissed: false,
      });
    } catch (error) {
      console.warn("Could not check for Qlisa updates:", error);
      set({
        status: "error",
        error: "updates.checkFailed",
        dismissed: silent,
      });
    }
  },

  downloadAndInstall: async () => {
    const update = pendingUpdate;
    if (!update) {
      set({ status: "error", error: "updates.checkAgain", dismissed: false });
      return;
    }

    set({ status: pendingUpdateDownloaded ? "available" : "downloading", progress: pendingUpdateDownloaded ? 1 : null, error: null, dismissed: false });
    let totalBytes: number | undefined;
    let downloadedBytes = 0;
    const onEvent = (event: DownloadEvent) => {
      if (event.event === "Started") {
        totalBytes = event.data.contentLength;
        downloadedBytes = 0;
      } else if (event.event === "Progress") {
        downloadedBytes += event.data.chunkLength;
        set({ progress: totalBytes ? Math.min(1, downloadedBytes / totalBytes) : null });
      } else if (event.event === "Finished") {
        set({ progress: 1 });
      }
    };

    let keepDownloadedUpdate = false;
    try {
      // Tauri verifies the downloaded artifact against its configured public key.
      if (!pendingUpdateDownloaded) {
        await update.download(onEvent, { timeout: 120_000 });
        pendingUpdateDownloaded = true;
      }

      // Cue and workspace state can change during a long download. Check again
      // immediately before install because Windows exits Qlisa to run the MSI/NSIS installer.
      const installBlock = get().getInstallBlock();
      if (installBlock) {
        keepDownloadedUpdate = true;
        set({ status: "error", error: `updates.${installBlock}`, progress: null, dismissed: false });
        return;
      }

      set({ status: "installing", progress: 1, error: null });
      await update.install({ restartAfterInstall: true });
      // Windows launches the installer with restartAfterInstall enabled and
      // exits this process. Other platforms need an explicit app relaunch.
      if (!navigator.userAgent.includes("Windows")) await relaunch();
    } catch (error) {
      console.error("Could not install the Qlisa update:", error);
      set({ status: "error", error: "updates.installFailed", progress: null, dismissed: false });
    } finally {
      if (!keepDownloadedUpdate) {
        await update.close().catch(() => {});
        if (pendingUpdate === update) {
          pendingUpdate = null;
          pendingUpdateDownloaded = false;
        }
      }
    }
  },

  dismiss: () => set({ dismissed: true }),
}));
