import { describe, expect, it } from "vitest";
import type { CueSummary } from "./types";
import { hasActivePlayback } from "./closeGuard";

const cue = (state: CueSummary["state"], children?: CueSummary[]): CueSummary => ({
  id: `${state}-${children?.length ?? 0}`,
  number: "1",
  name: "Test cue",
  type: "memo",
  state,
  children,
});

describe("hasActivePlayback", () => {
  it("does not warn for idle or completed cues", () => {
    expect(hasActivePlayback([cue("standby"), cue("completed")])).toBe(false);
  });

  it("warns for running and paused cues", () => {
    expect(hasActivePlayback([cue("running")])).toBe(true);
    expect(hasActivePlayback([cue("paused")])).toBe(true);
  });

  it("finds playback nested inside groups", () => {
    expect(hasActivePlayback([cue("standby", [cue("standby", [cue("running")])])])).toBe(true);
  });
});
