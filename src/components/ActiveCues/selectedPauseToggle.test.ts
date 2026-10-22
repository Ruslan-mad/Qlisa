import { describe, expect, it, vi } from "vitest";
import type { WorkspaceCueCatalogEntry } from "../../lib/types";
import { getSelectedPauseTargets } from "./activeCueModel";
import { toggleSelectedPauseTargets } from "./selectedPauseToggle";

const cue = (id: string, state: "running" | "paused"): WorkspaceCueCatalogEntry => ({
  id,
  cue_type: "audio",
  name: id,
  number: id,
  state,
  color: "none",
  duration_ms: 10,
  is_disabled: false,
});

describe("selected pause toggle action", () => {
  it("pauses running selected targets", async () => {
    const pause = vi.fn(async () => undefined);
    const resume = vi.fn(async () => undefined);

    const list = { id: "active", name: "Active", cues: [cue("one", "running"), cue("parallel", "running")] };
    const targets = getSelectedPauseTargets(list, ["one"]);
    await toggleSelectedPauseTargets(targets, pause, resume);

    expect(pause.mock.calls).toEqual([["one"]]);
    expect(resume).not.toHaveBeenCalled();
  });

  it("resumes paused selected targets when none are running", async () => {
    const pause = vi.fn(async () => undefined);
    const resume = vi.fn(async () => undefined);

    const list = { id: "active", name: "Active", cues: [cue("one", "paused"), cue("parallel", "paused")] };
    const targets = getSelectedPauseTargets(list, ["one"]);
    await toggleSelectedPauseTargets(targets, pause, resume);

    expect(resume).toHaveBeenCalledExactlyOnceWith("one");
    expect(pause).not.toHaveBeenCalled();
  });

  it("does nothing with no eligible selected targets", async () => {
    const pause = vi.fn(async () => undefined);
    const resume = vi.fn(async () => undefined);

    await toggleSelectedPauseTargets([], pause, resume);

    expect(pause).not.toHaveBeenCalled();
    expect(resume).not.toHaveBeenCalled();
  });

  it("ignores repeated keydowns", async () => {
    const pause = vi.fn(async () => undefined);
    const resume = vi.fn(async () => undefined);

    await toggleSelectedPauseTargets([cue("one", "running")], pause, resume, true);

    expect(pause).not.toHaveBeenCalled();
    expect(resume).not.toHaveBeenCalled();
  });
});
