import { describe, expect, it } from "vitest";
import { fileDropPathAllowedForTarget, fileDropTargetForCue, presentationSiblingLocationForChild } from "./fileDragModel";

describe("file drag targets", () => {
  it.each(["group", "number", "presentation"])("treats %s as a child container", (cueType) => {
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

  it("accepts only image and video files inside a Presentation", () => {
    expect(fileDropPathAllowedForTarget("presentation", "image")).toBe(true);
    expect(fileDropPathAllowedForTarget("presentation", "video")).toBe(true);
    expect(fileDropPathAllowedForTarget("presentation", "audio")).toBe(false);
    expect(fileDropPathAllowedForTarget("presentation", "presentation")).toBe(false);
  });

  it("places a dropped document beside the whole Presentation when dropped on a page", () => {
    const cues = [
      { id: "before", cue_type: "audio" },
      { id: "deck", cue_type: "presentation", children: [{ id: "page-1", cue_type: "image" }] },
      { id: "after", cue_type: "audio" },
    ];
    expect(presentationSiblingLocationForChild(cues, "page-1")).toEqual({ parentId: null, position: 2 });
  });
});
