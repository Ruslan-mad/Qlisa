import { beforeEach, describe, expect, it, vi } from "vitest";

const { updaterCheck } = vi.hoisted(() => ({ updaterCheck: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: updaterCheck }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));

import { useUpdateStore } from "../updateStore";

describe("updateStore install guard", () => {
  beforeEach(() => {
    updaterCheck.mockReset();
    vi.stubGlobal("navigator", { userAgent: "Windows" });
  });

  it("downloads while a cue is active, blocks install, then installs the retained download after stop", async () => {
    let installBlock: "activeCuesRunning" | null = null;
    const download = vi.fn(async (onEvent?: (event: { event: string; data?: { contentLength?: number; chunkLength?: number } }) => void) => {
      onEvent?.({ event: "Started", data: { contentLength: 100 } });
      onEvent?.({ event: "Progress", data: { chunkLength: 100 } });
      installBlock = "activeCuesRunning";
      onEvent?.({ event: "Finished" });
    });
    const install = vi.fn(async () => {});
    const close = vi.fn(async () => {});
    updaterCheck.mockResolvedValue({
      version: "1.5.3",
      body: "Release notes",
      download,
      install,
      close,
    });

    useUpdateStore.getState().setInstallGuard(() => installBlock);
    await useUpdateStore.getState().checkForUpdates();
    expect(useUpdateStore.getState().status).toBe("available");

    await useUpdateStore.getState().downloadAndInstall();
    expect(download).toHaveBeenCalledTimes(1);
    expect(install).not.toHaveBeenCalled();
    expect(useUpdateStore.getState().error).toBe("updates.activeCuesRunning");
    expect(close).not.toHaveBeenCalled();

    installBlock = null;
    await useUpdateStore.getState().downloadAndInstall();
    expect(download).toHaveBeenCalledTimes(1);
    expect(install).toHaveBeenCalledTimes(1);
    expect(close).toHaveBeenCalledTimes(1);
  });
});
