import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn() }));

// cueOperations also contains Tauri-backed edit actions; mock the bridge so
// selection ordering and paste-selection behavior stay frontend-only tests.
import { copySelection, cueIdsInPlaylist, pasteAfterSelection } from "../cueOperations";
import { useWorkspaceStore } from "../../stores/workspaceStore";

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  useWorkspaceStore.setState({
    cues: [],
    selectedCueId: null,
    selectedCueIds: [],
  });
});

describe("playlist edit selection", () => {
  it("selects every cue in display/list order, including nested group children", () => {
    expect(cueIdsInPlaylist([
      { id: "a" },
      { id: "group", children: [
        { id: "b" },
        { id: "nested", children: [{ id: "c" }] },
      ] },
      { id: "d" },
    ])).toEqual(["a", "group", "b", "nested", "c", "d"]);
  });

  it("does not mutate the source playlist while collecting IDs", () => {
    const cues = [{ id: "a", children: [{ id: "b" }] }];
    expect(cueIdsInPlaylist(cues)).toEqual(["a", "b"]);
    expect(cues).toEqual([{ id: "a", children: [{ id: "b" }] }]);
  });

  it("copies multi-selection in playlist order rather than Ctrl-click order", async () => {
    useWorkspaceStore.setState({
      cues: [
        { id: "a" },
        { id: "group", children: [{ id: "b" }] },
        { id: "c" },
      ] as never,
      selectedCueId: "c",
      selectedCueIds: ["c", "a", "b"],
    });

    await copySelection();

    expect(invokeMock).toHaveBeenCalledWith("copy_cues", {
      cueIds: ["a", "b", "c"],
    });
  });

  it("selects every newly pasted cue when the clipboard contains a set", async () => {
    invokeMock.mockResolvedValue(["new-a", "new-b"]);
    useWorkspaceStore.setState({ selectedCueId: "anchor", selectedCueIds: ["anchor"] });

    await pasteAfterSelection(() => {});

    expect(invokeMock).toHaveBeenCalledWith("paste_cue", { afterCueId: "anchor" });
    expect(useWorkspaceStore.getState().selectedCueIds).toEqual(["new-a", "new-b"]);
    expect(useWorkspaceStore.getState().selectedCueId).toBe("new-b");
  });
});
