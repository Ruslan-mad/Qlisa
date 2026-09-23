import { describe, expect, it } from "vitest";
import type { CueSummary } from "../../lib/types";
import {
  cueFileName,
  cueNotesProperty,
  cueNotesText,
  formatTargetCues,
} from "./cueRowContent";

function summary(overrides: Partial<CueSummary>): CueSummary {
  return {
    id: "cue",
    cue_type: "audio",
    name: "Audio Intro",
    number: "1",
    notes: "Operator note",
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
    ...overrides,
  };
}

describe("cue row file, notes, and target content", () => {
  it("shows only the assigned file basename, including its extension", () => {
    expect(cueFileName(summary({ file_path: "C:\\Show\\audio\\Intro theme.wav" })))
      .toBe("Intro theme.wav");
    expect(cueFileName(summary({ file_path: null }))).toBe("");
  });

  it("uses Memo text in Notes and saves it through memo_text", () => {
    const memo = summary({ cue_type: "memo", notes: "old note", memo_text: "Scene change" });
    expect(cueNotesText(memo)).toBe("Scene change");
    expect(cueNotesProperty(memo)).toBe("memo_text");

    const audio = summary({ notes: "Keep the room quiet" });
    expect(cueNotesText(audio)).toBe("Keep the room quiet");
    expect(cueNotesProperty(audio)).toBe("notes");
  });

  it("shows one target with its number and name and exposes the full text as a title", () => {
    expect(formatTargetCues(
      [{ number: "12", name: "House lights" }],
      false,
      "All Cues",
    )).toEqual({ text: "12 House lights", title: "12 House lights" });
  });

  it("compacts many targets but retains their complete title, and labels Stop All", () => {
    const targets = [
      { number: "1", name: "Music" },
      { number: "2", name: "Video" },
      { number: "3", name: "Lights" },
    ];
    expect(formatTargetCues(targets, false, "All Cues")).toEqual({
      text: "1 Music, 2 Video, +1",
      title: "1 Music, 2 Video, 3 Lights",
    });
    expect(formatTargetCues([], true, "All Cues")).toEqual({
      text: "All Cues",
      title: "All Cues",
    });
  });
});
