import { describe, expect, it } from "vitest";
import { fileDropPathAllowedForTarget, fileDropTargetForCue } from "./fileDragModel";

describe("file drag targets", () => {
  it.each(["group", "number"])("treats %s as a child container", (cueType) => {
    expect(fileDropTargetForCue(cueType, "container")).toEqual({ assignId: null, groupId: "container" });
  });

  it("assigns media to ordinary cues", () => {
    expect(fileDropTargetForCue("audio", "cue")).toEqual({ assignId: "cue", groupId: null });
  });

  it("allows Number media children but rejects MIDI", () => {
    expect(fileDropPathAllowedForTarget("number", "audio")).toBe(true);
    expect(fileDropPathAllowedForTarget("number", "video")).toBe(true);
    expect(fileDropPathAllowedForTarget("number", "image")).toBe(true);
    expect(fileDropPathAllowedForTarget("number", "midi_file")).toBe(false);
    expect(fileDropPathAllowedForTarget("group", "image")).toBe(true);
  });
});
