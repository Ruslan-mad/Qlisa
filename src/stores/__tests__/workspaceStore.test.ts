import { beforeEach, describe, expect, it } from "vitest";
import type { CueSummary } from "../../lib/types";
import type { WorkspaceCueCatalogList } from "../../lib/types";
import { useWorkspaceStore } from "../workspaceStore";

const summary = (id: string, children?: CueSummary[]): CueSummary => ({
  id,
  cue_type: children ? "group" : "audio",
  name: id,
  number: null,
  notes: "",
  state: "standby",
  continue_mode: "do_not_continue",
  color: "none",
  pre_wait_ms: 0,
  post_wait_ms: 0,
  duration_ms: null,
  file_path: null,
  file_size_bytes: null,
  media_width: null,
  media_height: null,
  media_file_missing: false,
  is_loading: false,
  is_disabled: false,
  is_broken: false,
  is_warning: false,
  file_duration_ms: null,
  children,
});

describe("workspace cue state updates", () => {
  beforeEach(() => {
    useWorkspaceStore.setState({ cues: [], selectedCueId: null, selectedCueIds: [], playedCueIds: new Set() });
  });

  it("updates a leaf nested inside a Group", () => {
    const leaf = summary("leaf");
    useWorkspaceStore.setState({ cues: [summary("group", [leaf])] });

    useWorkspaceStore.getState().updateCueState("leaf", "running");

    const group = useWorkspaceStore.getState().cues[0];
    expect(group.state).toBe("standby");
    expect(group.children?.[0].state).toBe("running");
  });

  it("preserves the existing tree when the cue is absent", () => {
    const group = summary("group", [summary("leaf")]);
    useWorkspaceStore.setState({ cues: [group] });

    useWorkspaceStore.getState().updateCueState("missing", "running");

    expect(useWorkspaceStore.getState().cues[0]).toBe(group);
  });

  it("updates remote runtime state in the read-only catalog, including nested cues", () => {
    const catalog: WorkspaceCueCatalogList[] = [{
      id: "remote", name: "Remote", cues: [{
        id: "parent", cue_type: "group", name: "Parent", number: "1", state: "standby", color: "none", duration_ms: null, is_disabled: false,
        children: [{ id: "child", cue_type: "audio", name: "Child", number: "1.1", state: "standby", color: "none", duration_ms: 1000, is_disabled: false }],
      }],
    }];
    useWorkspaceStore.setState({ cueCatalog: catalog });

    useWorkspaceStore.getState().updateCueState("child", "running");

    expect(useWorkspaceStore.getState().cueCatalog[0].cues[0].state).toBe("standby");
    expect(useWorkspaceStore.getState().cueCatalog[0].cues[0].children?.[0].state).toBe("running");
  });
});

describe("workspace selection invariant", () => {
  beforeEach(() => {
    useWorkspaceStore.setState({ selectedCueId: null, selectedCueIds: [] });
  });

  it("deduplicates IDs in stable order and includes the primary atomically", () => {
    useWorkspaceStore.getState().setCueSelection("primary", ["a", "a", "b"]);

    expect(useWorkspaceStore.getState().selectedCueIds).toEqual(["a", "b", "primary"]);
    expect(useWorkspaceStore.getState().selectedCueId).toBe("primary");
  });

  it("keeps legacy setters inside the same invariant", () => {
    const store = useWorkspaceStore.getState();
    store.setSelectedCueId("a");
    store.setSelectedCueIds(["b", "b", "c"]);

    expect(useWorkspaceStore.getState().selectedCueId).toBe("c");
    expect(useWorkspaceStore.getState().selectedCueIds).toEqual(["b", "c"]);

    useWorkspaceStore.getState().setSelectedCueId(null);
    expect(useWorkspaceStore.getState().selectedCueId).toBeNull();
    expect(useWorkspaceStore.getState().selectedCueIds).toEqual([]);
  });

  it("applies a GO selection only while the captured selection and cue list remain current", () => {
    useWorkspaceStore.setState({ activeCueListId: "list-a", playheadCueId: "old" });
    const revision = useWorkspaceStore.getState().cueSelectionRevision;

    expect(useWorkspaceStore.getState().applyGoSelection("list-a", "next", revision)).toBe(true);
    expect(useWorkspaceStore.getState().selectedCueId).toBe("next");
    expect(useWorkspaceStore.getState().playheadCueId).toBe("next");

    const staleRevision = useWorkspaceStore.getState().cueSelectionRevision;
    useWorkspaceStore.getState().setCueSelection("manual", ["manual", "other"]);
    expect(useWorkspaceStore.getState().applyGoSelection("list-a", "stale", staleRevision)).toBe(false);
    expect(useWorkspaceStore.getState().selectedCueIds).toEqual(["manual", "other"]);
    expect(useWorkspaceStore.getState().playheadCueId).toBe("next");

    const currentRevision = useWorkspaceStore.getState().cueSelectionRevision;
    expect(useWorkspaceStore.getState().applyGoSelection("list-b", "foreign", currentRevision)).toBe(false);
    expect(useWorkspaceStore.getState().playheadCueId).toBe("next");
    expect(useWorkspaceStore.getState().applyGoSelection("list-a", null, currentRevision)).toBe(true);
    expect(useWorkspaceStore.getState().selectedCueId).toBeNull();
    expect(useWorkspaceStore.getState().selectedCueIds).toEqual([]);
    expect(useWorkspaceStore.getState().playheadCueId).toBeNull();
  });
});

describe("session-only fired cue history", () => {
  beforeEach(() => {
    useWorkspaceStore.setState({ playedCueIds: new Set(), cueStartOrder: {}, cueStartOrderCounter: 0 });
  });

  it("keeps unique fired history, updates repeat start order, and clears on workspace replacement", () => {
    const { markCuePlayed, resetWorkspaceView } = useWorkspaceStore.getState();
    markCuePlayed("cue-a");
    const firstOrder = useWorkspaceStore.getState().cueStartOrder["cue-a"];
    markCuePlayed("cue-a");
    markCuePlayed("cue-b");
    expect([...useWorkspaceStore.getState().playedCueIds]).toEqual(["cue-a", "cue-b"]);
    expect(useWorkspaceStore.getState().cueStartOrder["cue-a"]).toBeGreaterThan(firstOrder);
    expect(useWorkspaceStore.getState().cueStartOrder).toEqual({ "cue-a": 2, "cue-b": 3 });
    expect(useWorkspaceStore.getState().cueStartOrderCounter).toBe(3);

    const revisionBeforeWorkspaceReplacement = useWorkspaceStore.getState().cueSelectionRevision;
    resetWorkspaceView();
    expect(useWorkspaceStore.getState().playedCueIds.size).toBe(0);
    expect(useWorkspaceStore.getState().cueStartOrder).toEqual({});
    expect(useWorkspaceStore.getState().cueStartOrderCounter).toBe(0);
    expect(useWorkspaceStore.getState().cueSelectionRevision).toBeGreaterThan(revisionBeforeWorkspaceReplacement);
  });
});
