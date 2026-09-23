import { describe, expect, it } from "vitest";
import type { CueType } from "../../lib/types";
import { canTargetCueType, isTargetedCueType, resolveContextCueInsert } from "./contextCueInsert";

describe("context cue insertion", () => {
  it.each<CueType>([
    "stop", "fade", "devamp",
    "start", "pause", "resume", "load", "reset", "goto", "arm", "disarm",
  ])("recognises %s as target-based", (cueType) => {
    expect(isTargetedCueType(cueType)).toBe(true);
  });

  it.each<CueType>(["audio", "video", "image", "wait", "group", "memo", "text"])(
    "keeps ordinary %s insertion untargeted",
    (cueType) => {
      expect(isTargetedCueType(cueType)).toBe(false);
    },
  );

  it("targets the right-clicked cue above and below", () => {
    const cueIds = ["cue-a", "cue-b", "cue-c"];
    expect(resolveContextCueInsert("stop", "cue-b", cueIds, 0)).toEqual({
      position: 1,
      targetCueId: "cue-b",
    });
    expect(resolveContextCueInsert("fade", "cue-b", cueIds, 1)).toEqual({
      position: 2,
      targetCueId: "cue-b",
    });
  });

  it("does not assign a target to an ordinary cue", () => {
    expect(resolveContextCueInsert("audio", "cue-b", ["cue-a", "cue-b"], 1)).toEqual({
      position: 2,
      targetCueId: null,
    });
  });

  it("limits Fade targets to media, camera, and group cues", () => {
    for (const targetType of ["audio", "video", "image", "camera", "group"] as CueType[]) {
      expect(canTargetCueType("fade", targetType)).toBe(true);
    }
    for (const targetType of ["wait", "memo", "stop", "midi"] as CueType[]) {
      expect(canTargetCueType("fade", targetType)).toBe(false);
    }
  });

  it("limits Devamp targets to playable media and groups", () => {
    for (const targetType of ["audio", "video", "group"] as CueType[]) {
      expect(canTargetCueType("devamp", targetType)).toBe(true);
    }
    for (const targetType of ["image", "camera", "wait", "stop"] as CueType[]) {
      expect(canTargetCueType("devamp", targetType)).toBe(false);
    }
  });

  it("allows Stop and command cues to target any cue type", () => {
    for (const actionType of ["stop", "start", "pause", "resume", "load", "reset", "goto", "arm", "disarm"] as CueType[]) {
      expect(canTargetCueType(actionType, "memo")).toBe(true);
    }
  });

  it("rejects a clicked cue that is not in the top-level list", () => {
    expect(resolveContextCueInsert("stop", "nested", ["cue-a", "cue-b"], 1)).toBeNull();
  });
});
