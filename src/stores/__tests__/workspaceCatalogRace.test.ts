import { beforeEach, describe, expect, it, vi } from "vitest";

const commands = vi.hoisted(() => ({
  getCueLists: vi.fn(),
  getWorkspaceCueCatalog: vi.fn(),
}));

vi.mock("../../lib/commands", async (importOriginal) => ({
  ...await importOriginal<typeof import("../../lib/commands")>(),
  getCueLists: commands.getCueLists,
  getWorkspaceCueCatalog: commands.getWorkspaceCueCatalog,
}));

import { useWorkspaceStore } from "../workspaceStore";
import type { WorkspaceCueCatalogList } from "../../lib/types";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

const catalog = (name: string, state: "standby" | "running"): WorkspaceCueCatalogList[] => [{
  id: "new-list",
  name: "New list",
  cues: [{ id: "remote-cue", cue_type: "audio", name, number: "1", state, color: "none", duration_ms: 1000, is_disabled: false }],
}];

describe("workspace catalog refresh races", () => {
  beforeEach(() => {
    commands.getCueLists.mockReset();
    commands.getWorkspaceCueCatalog.mockReset();
    useWorkspaceStore.setState({ cueLists: [], activeCueListId: null, cueCatalog: [] });
  });

  it("commits new list tabs while a state event and newer catalog refresh supersede the old snapshot", async () => {
    const oldSnapshot = deferred<WorkspaceCueCatalogList[]>();
    const freshSnapshot = deferred<WorkspaceCueCatalogList[]>();
    commands.getCueLists.mockResolvedValue([{ id: "new-list", name: "New list", mode: "sequential" }]);
    commands.getWorkspaceCueCatalog
      .mockReturnValueOnce(oldSnapshot.promise)
      .mockReturnValueOnce(freshSnapshot.promise);

    const pendingLists = useWorkspaceStore.getState().refreshCueLists();
    await Promise.resolve();
    await Promise.resolve();
    expect(commands.getWorkspaceCueCatalog).toHaveBeenCalledTimes(1);

    const pendingFreshCatalog = useWorkspaceStore.getState().refreshCueCatalog();
    useWorkspaceStore.getState().updateCueState("remote-cue", "running");
    freshSnapshot.resolve(catalog("Fresh cue", "standby"));
    await pendingFreshCatalog;
    oldSnapshot.resolve(catalog("Stale cue", "standby"));
    await pendingLists;

    const state = useWorkspaceStore.getState();
    expect(state.cueLists.map(({ id }) => id)).toEqual(["new-list"]);
    expect(state.activeCueListId).toBe("new-list");
    expect(state.cueCatalog[0].cues[0].name).toBe("Fresh cue");
    expect(state.cueCatalog[0].cues[0].state).toBe("running");
  });
});
