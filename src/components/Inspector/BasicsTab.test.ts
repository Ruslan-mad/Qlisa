import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { cueNotesUpdate } from "../../lib/cueNotes";
import { BasicsTab } from "./BasicsTab";

function renderBasics(cue: Record<string, unknown>, isAudio = false): string {
  return renderToStaticMarkup(createElement(BasicsTab, {
    cue,
    isAudio,
    onSave: vi.fn(),
    onBrowse: vi.fn(),
  }));
}

describe("Basics tab notes", () => {
  it("renders Memo text from memo_text even when BaseCue notes differs", () => {
    const html = renderBasics({
      id: "memo-1",
      cue_type: "memo",
      number: null,
      name: "Memo",
      color: "none",
      memo_text: "Scene change",
      notes: "old note",
      is_disabled: false,
    });

    expect(html).toMatch(/<textarea[^>]*>Scene change<\/textarea>/);
    expect(html).not.toContain("old note");
    expect(cueNotesUpdate({ cue_type: "memo" }, "Edited scene"))
      .toEqual({ memo_text: "Edited scene" });
  });

  it("renders ordinary cue notes and saves them through notes", () => {
    const audio = { cue_type: "audio" as const, notes: "Keep quiet" };
    const html = renderBasics({
      id: "audio-1",
      cue_type: "audio",
      number: null,
      name: "Audio",
      color: "none",
      notes: audio.notes,
      is_disabled: false,
      file_path: null,
    }, true);

    expect(html).toMatch(/<textarea[^>]*>Keep quiet<\/textarea>/);
    expect(cueNotesUpdate(audio, "Edited note")).toEqual({ notes: "Edited note" });
  });
});
