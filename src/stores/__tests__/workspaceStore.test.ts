import { beforeEach, describe, expect, it } from "vitest";
import type { CueSummary } from "../../lib/types";
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
});

describe("session-only fired cue history", () => {
  beforeEach(() => {
    useWorkspaceStore.setState({ playedCueIds: new Set() });
  });

  it("records fired cue IDs once and clears the history without persistence", () => {
    const { markCuePlayed, clearPlayedCueHistory } = useWorkspaceStore.getState();
    markCuePlayed("cue-a");
    markCuePlayed("cue-a");
    markCuePlayed("cue-b");
    expect([...useWorkspaceStore.getState().playedCueIds]).toEqual(["cue-a", "cue-b"]);

    clearPlayedCueHistory();
    expect(useWorkspaceStore.getState().playedCueIds.size).toBe(0);
  });
});
