import { beforeEach, describe, expect, it, vi } from "vitest";

const commands = vi.hoisted(() => ({
  getWorkspaceSnapshot: vi.fn(), getWorkspaceInfo: vi.fn(), checkWorkspace: vi.fn(),
}));

vi.mock("../../lib/commands", async (importOriginal) => ({
  ...await importOriginal<typeof import("../../lib/commands")>(),
  getWorkspaceSnapshot: commands.getWorkspaceSnapshot,
  getWorkspaceInfo: commands.getWorkspaceInfo,
  checkWorkspace: commands.checkWorkspace,
}));

import { useWorkspaceStore } from "../workspaceStore";
import type { CueSummary, WorkspaceCueCatalogList, WorkspaceSnapshot } from "../../lib/types";
import { useTimingStore } from "../timingStore";
import { reconcileWorkspaceAfterReplacement } from "../../lib/reconcileWorkspaceReplacement";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function snapshot(listId: string, cueName: string, state: "standby" | "running" = "standby"): WorkspaceSnapshot {
  const cue = {
    id: "cue-1", cue_type: "audio", name: cueName, number: "1", state,
    color: "none", duration_ms: 1000, children: undefined,
  } as unknown as CueSummary;
  const entry = {
    id: "cue-1", cue_type: "audio", name: cueName, number: "1", state,
    color: "none", duration_ms: 1000, is_disabled: false,
  } as WorkspaceCueCatalogList["cues"][number];
  return {
    cue_lists: [{ id: listId, name: "Cue List", mode: "sequential" }],
    active_cue_list_id: listId,
    cues: [cue],
    playhead_cue_id: "cue-1",
    cue_catalog: [{ id: listId, name: "Cue List", cues: [entry] }],
  };
}

describe("workspace snapshot refresh races", () => {
  beforeEach(() => {
    commands.getWorkspaceSnapshot.mockReset();
    commands.getWorkspaceInfo.mockResolvedValue({} as never);
    commands.checkWorkspace.mockResolvedValue([]);
    useWorkspaceStore.setState({
      cues: [], cueCatalog: [], cueLists: [], activeCueListId: null,
      selectedCueId: null, selectedCueIds: [], cueSelectionRevision: 0,
      workspaceContextRevision: 0, playheadCueId: null,
      workspaceReplacementRevision: 0,
      playedCueIds: new Set(), cueStartOrder: {}, cueStartOrderCounter: 0,
    });
  });

  it("commits one coherent active-list/cues/catalog snapshot and preserves runtime events", async () => {
    const pending = deferred<WorkspaceSnapshot>();
    commands.getWorkspaceSnapshot.mockReturnValue(pending.promise);
    useWorkspaceStore.getState().setCueSelection("cue-1", ["cue-1"]);

    const refresh = useWorkspaceStore.getState().refreshCues();
    useWorkspaceStore.getState().updateCueState("cue-1", "running");
    pending.resolve(snapshot("list-new", "Fresh", "standby"));
    await refresh;

    const state = useWorkspaceStore.getState();
    expect(state.activeCueListId).toBe("list-new");
    expect(state.cueLists[0].id).toBe("list-new");
    expect(state.cues[0].name).toBe("Fresh");
    expect(state.cues[0].state).toBe("running");
    expect(state.cueCatalog[0].cues[0].state).toBe("running");
    // The old cue was not present before this snapshot, so it is not retained.
    expect(state.selectedCueId).toBeNull();
  });

  it("clears visible rows on active-list change but keeps the cross-list runtime catalog", () => {
    const allLists: WorkspaceCueCatalogList[] = [
      ...snapshot("list-a", "A").cue_catalog,
      ...snapshot("list-b", "B").cue_catalog,
    ];
    useWorkspaceStore.setState({
      cues: snapshot("list-a", "A").cues,
      cueLists: [
        { id: "list-a", name: "A", mode: "sequential" },
        { id: "list-b", name: "B", mode: "sequential" },
      ],
      activeCueListId: "list-a",
      cueCatalog: allLists,
    });

    useWorkspaceStore.getState().setCueLists(
      [
        { id: "list-a", name: "A", mode: "sequential" },
        { id: "list-b", name: "B", mode: "sequential" },
      ],
      "list-b",
    );

    const state = useWorkspaceStore.getState();
    expect(state.cues).toEqual([]);
    expect(state.cueCatalog).toEqual(allLists);
  });

  it("does not apply an old workspace reply after replacement, even when list IDs can repeat", async () => {
    const old = deferred<WorkspaceSnapshot>();
    const fresh = deferred<WorkspaceSnapshot>();
    commands.getWorkspaceSnapshot.mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
    const oldRefresh = useWorkspaceStore.getState().refreshCues();
    useWorkspaceStore.getState().setCueSelection("cue-1", ["cue-1"]);
    useWorkspaceStore.getState().markCuePlayed("cue-1");
    useWorkspaceStore.getState().resetWorkspaceView();
    const newRefresh = useWorkspaceStore.getState().refreshCues();

    fresh.resolve(snapshot("same-list-id", "New project"));
    await newRefresh;
    old.resolve(snapshot("same-list-id", "Old project"));
    await oldRefresh;

    const state = useWorkspaceStore.getState();
    expect(state.cues[0].name).toBe("New project");
    expect(state.selectedCueIds).toEqual([]);
    expect(state.playedCueIds.size).toBe(0);
  });

  it("keeps valid manual selection on an ordinary refresh and clears invalid IDs", async () => {
    useWorkspaceStore.setState({
      cues: [snapshot("list-a", "Existing").cues[0]],
      cueLists: [{ id: "list-a", name: "Cue List", mode: "sequential" }],
      activeCueListId: "list-a",
    });
    useWorkspaceStore.getState().setCueSelection("cue-1", ["cue-1"]);
    commands.getWorkspaceSnapshot.mockResolvedValue(snapshot("list-a", "Refresh"));
    await useWorkspaceStore.getState().refreshCues();
    expect(useWorkspaceStore.getState().selectedCueId).toBe("cue-1");

    commands.getWorkspaceSnapshot.mockResolvedValue({
      ...snapshot("list-a", "No cues"), cues: [], cue_catalog: [{ id: "list-a", name: "Cue List", cues: [] }],
    });
    await useWorkspaceStore.getState().refreshCues();
    expect(useWorkspaceStore.getState().selectedCueId).toBeNull();
    expect(useWorkspaceStore.getState().selectedCueIds).toEqual([]);
  });

  it("does not rewind a manual playhead move when an earlier snapshot returns", async () => {
    const pending = deferred<WorkspaceSnapshot>();
    useWorkspaceStore.setState({
      cueLists: [{ id: "list-a", name: "Cue List", mode: "sequential" }],
      activeCueListId: "list-a",
      playheadCueId: "cue-1",
    });
    commands.getWorkspaceSnapshot.mockReturnValue(pending.promise);
    const refresh = useWorkspaceStore.getState().refreshCues();
    useWorkspaceStore.getState().setPlayheadCueId("manual-cue");
    pending.resolve(snapshot("list-a", "Fresh"));
    await refresh;
    expect(useWorkspaceStore.getState().playheadCueId).toBe("manual-cue");
  });

  it("reconciles a successful replacement when no replacement event arrived", async () => {
    useWorkspaceStore.setState({
      cues: [snapshot("same-id", "Previous").cues[0]],
      cueCatalog: snapshot("same-id", "Previous").cue_catalog,
      cueLists: [{ id: "same-id", name: "Cue List", mode: "sequential" }],
      activeCueListId: "same-id",
      playedCueIds: new Set(["cue-1"]), cueStartOrder: { "cue-1": 1 }, cueStartOrderCounter: 1,
      selectedCueId: "cue-1", selectedCueIds: ["cue-1"],
      workspaceReplacementRevision: 8,
    });
    useTimingStore.setState({
      timings: { "cue-1": { elapsed_ms: 1, action_elapsed_ms: 1, remaining_ms: 2, media_position_ms: null, wait_phase: null, wait_elapsed_ms: null, wait_duration_ms: null } },
      previewPlayhead: null, previewPlayheadGeneration: 21,
    });
    commands.getWorkspaceSnapshot.mockResolvedValue(snapshot("same-id", "Replaced project"));

    await reconcileWorkspaceAfterReplacement(8);

    const state = useWorkspaceStore.getState();
    expect(state.cues[0].name).toBe("Replaced project");
    expect(state.workspaceReplacementRevision).toBe(9);
    expect(state.selectedCueIds).toEqual([]);
    expect(state.playedCueIds.size).toBe(0);
    expect(useTimingStore.getState().timings).toEqual({});
    expect(useTimingStore.getState().previewPlayheadGeneration).toBe(21);
  });
});
