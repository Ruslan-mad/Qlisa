import { beforeEach, describe, expect, it, vi } from "vitest";

const { goMock } = vi.hoisted(() => ({ goMock: vi.fn() }));
vi.mock("../commands", () => ({ go: goMock }));

import { goAndFollowSelection } from "../goWithSelection";
import { useWorkspaceStore } from "../../stores/workspaceStore";

beforeEach(() => {
  goMock.mockReset();
  useWorkspaceStore.setState({
    activeCueListId: "list-a",
    selectedCueId: "current",
    selectedCueIds: ["current", "also-selected"],
    cueSelectionRevision: 0,
  });
});

describe("goAndFollowSelection", () => {
  it.each([
    ["ignored GO", { cue_list_id: "list-a", cue_id: "next", advanced: false }],
    ["cancelled GO", undefined],
  ])("does not change selection after %s", async (_label, result) => {
    goMock.mockResolvedValue(result);

    await goAndFollowSelection();

    expect(useWorkspaceStore.getState().selectedCueIds).toEqual(["current", "also-selected"]);
  });

  it("follows the resulting Playhead after a successful GO", async () => {
    goMock.mockResolvedValue({ cue_list_id: "list-a", cue_id: "next", advanced: true });

    await goAndFollowSelection();

    expect(useWorkspaceStore.getState().selectedCueId).toBe("next");
    expect(useWorkspaceStore.getState().selectedCueIds).toEqual(["next"]);
  });

  it("preserves a manual selection made while GO is pending", async () => {
    let finish!: (value: { cue_list_id: string; cue_id: string; advanced: boolean }) => void;
    goMock.mockReturnValue(new Promise((resolve) => { finish = resolve; }));

    const pending = goAndFollowSelection();
    useWorkspaceStore.getState().setCueSelection("manual", ["manual", "multi"]);
    finish({ cue_list_id: "list-a", cue_id: "next", advanced: true });
    await pending;

    expect(useWorkspaceStore.getState().selectedCueId).toBe("manual");
    expect(useWorkspaceStore.getState().selectedCueIds).toEqual(["manual", "multi"]);
  });

  it("leaves selection unchanged when GO rejects", async () => {
    const failure = new Error("GO failed");
    goMock.mockRejectedValue(failure);

    await expect(goAndFollowSelection()).rejects.toBe(failure);

    expect(useWorkspaceStore.getState().selectedCueIds).toEqual(["current", "also-selected"]);
  });
});
