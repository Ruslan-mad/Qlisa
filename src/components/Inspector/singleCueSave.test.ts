import { describe, expect, it, vi } from "vitest";
import { formatInspectorSaveError, isCurrentCueRequest, isFadeEdit, persistCurrentThenCommit, persistThenCommit } from "./singleCueSave";

describe("single-cue Inspector save", () => {
  it("does not commit attempted values when backend persistence rejects", async () => {
    let cueData = { id: "cue-1", name: "Actual name" };
    const onFailure = vi.fn();
    const backendError = new Error("Cue is Paused; stop it before changing fades");

    const saved = await persistThenCommit(
      () => Promise.reject(backendError),
      () => { cueData = { ...cueData, name: "Attempted name" }; },
      onFailure,
    );

    expect(saved).toBe(false);
    expect(cueData.name).toBe("Actual name");
    expect(onFailure).toHaveBeenCalledWith(backendError);
  });

  it("commits local values only after backend persistence succeeds", async () => {
    let cueData = { id: "cue-1", name: "Actual name" };
    const commit = vi.fn(() => { cueData = { ...cueData, name: "Saved name" }; });
    const onFailure = vi.fn();

    const saved = await persistThenCommit(
      async () => undefined,
      commit,
      onFailure,
    );

    expect(saved).toBe(true);
    expect(commit).toHaveBeenCalledOnce();
    expect(cueData.name).toBe("Saved name");
    expect(onFailure).not.toHaveBeenCalled();
  });

  it("shows a friendly fade-safety message and preserves other backend details", () => {
    const t = (key: string, vars?: Record<string, string | number | boolean | null | undefined>) => {
      if (key === "inspector.stopCueBeforeFadeEdit") return "Остановите cue перед изменением фейда.";
      return `Ошибка сохранения: ${vars?.error ?? ""}`;
    };

    expect(isFadeEdit({ video_fade_in_ms: 500 })).toBe(true);
    expect(isFadeEdit({ volume_db: -3 })).toBe(false);
    expect(formatInspectorSaveError("Cue is preloaded; stop it before changing fades", true, t))
      .toBe("Остановите cue перед изменением фейда.");
    expect(formatInspectorSaveError("Cue not found", false, t)).toBe("Ошибка сохранения: Cue not found");
  });
});

describe("cue request generation guard", () => {
  it("rejects stale A→B→A responses even when the selected cue ID matches again", () => {
    expect(isCurrentCueRequest({ cueId: "A", generation: 1 }, 3, "A")).toBe(false);
    expect(isCurrentCueRequest({ cueId: "A", generation: 3 }, 3, "A")).toBe(true);
  });

  it("rejects a response when selection moved to another cue", () => {
    expect(isCurrentCueRequest({ cueId: "A", generation: 2 }, 2, "B")).toBe(false);
  });
});

describe("selection-scoped Inspector save", () => {
  it("does not commit a successful stale response or run its side effects", async () => {
    const commit = vi.fn();
    const onFailure = vi.fn();
    const ticket = { cueId: "A", generation: 1 };
    let currentGeneration = 1;

    const saved = await persistCurrentThenCommit(
      async () => { currentGeneration = 3; },
      () => isCurrentCueRequest(ticket, currentGeneration, "A"),
      commit,
      onFailure,
    );

    expect(saved).toBe(false);
    expect(commit).not.toHaveBeenCalled();
    expect(onFailure).not.toHaveBeenCalled();
  });

  it("does not report a stale rejection to the newly selected cue", async () => {
    const commit = vi.fn();
    const onFailure = vi.fn();
    const ticket = { cueId: "A", generation: 1 };
    let currentGeneration = 1;

    const saved = await persistCurrentThenCommit(
      async () => {
        currentGeneration = 3;
        throw new Error("old cue rejected");
      },
      () => isCurrentCueRequest(ticket, currentGeneration, "A"),
      commit,
      onFailure,
    );

    expect(saved).toBe(false);
    expect(commit).not.toHaveBeenCalled();
    expect(onFailure).not.toHaveBeenCalled();
  });

  it("commits a successful current request", async () => {
    const commit = vi.fn();
    const onFailure = vi.fn();

    const saved = await persistCurrentThenCommit(async () => undefined, () => true, commit, onFailure);

    expect(saved).toBe(true);
    expect(commit).toHaveBeenCalledOnce();
    expect(onFailure).not.toHaveBeenCalled();
  });

  it("ignores a same-cue success after an authoritative reload", async () => {
    const ticket = { cueId: "A", generation: 1 };
    let currentGeneration = 1;
    let currentDraft = "new draft after reload";
    const commit = vi.fn(() => { currentDraft = "stale saved value"; });
    const onFailure = vi.fn();

    const saved = await persistCurrentThenCommit(
      async () => { currentGeneration = 2; }, // reloadToken changed; cue ID stayed A
      () => isCurrentCueRequest(ticket, currentGeneration, "A"),
      commit,
      onFailure,
    );

    expect(saved).toBe(false);
    expect(currentDraft).toBe("new draft after reload");
    expect(commit).not.toHaveBeenCalled();
    expect(onFailure).not.toHaveBeenCalled();
  });

  it("does not surface a same-cue rejection after an authoritative reload", async () => {
    const ticket = { cueId: "A", generation: 1 };
    let currentGeneration = 1;
    let currentDraft = "new draft after reload";
    const commit = vi.fn();
    const onFailure = vi.fn(() => { currentDraft = "old draft restored"; });

    const saved = await persistCurrentThenCommit(
      async () => {
        currentGeneration = 2; // reloadToken changed; cue ID stayed A
        throw new Error("old request rejected");
      },
      () => isCurrentCueRequest(ticket, currentGeneration, "A"),
      commit,
      onFailure,
    );

    expect(saved).toBe(false);
    expect(currentDraft).toBe("new draft after reload");
    expect(commit).not.toHaveBeenCalled();
    expect(onFailure).not.toHaveBeenCalled();
  });
});
