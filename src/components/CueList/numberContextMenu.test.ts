import { describe, expect, it } from "vitest";
import type { CueSummary } from "../../lib/types";
import { canAddCuesToNumber, numberTargets } from "./numberContextMenu";

function cue(id: string, cue_type: CueSummary["cue_type"]): CueSummary {
  return { id, cue_type, name: id, number: null, notes: "", state: "idle", continue_mode: "do_not_continue", color: "none", pre_wait_ms: 0, post_wait_ms: 0, duration_ms: null, file_path: null, target_cues: undefined, file_size_bytes: null, media_width: null, media_height: null, media_file_missing: false, is_loading: false, is_disabled: false, is_broken: false, is_warning: false, file_duration_ms: null };
}

describe("Number context-menu targets", () => {
  it("allows Audio, Video, Image, and Group selections", () => {
    const cues = [cue("audio", "audio"), cue("video", "video"), cue("image", "image"), cue("group", "group"), cue("memo", "memo")];
    expect(canAddCuesToNumber(cues, ["audio", "video", "image"])).toBe(true);
    expect(canAddCuesToNumber(cues, ["group"])).toBe(true);
    expect(canAddCuesToNumber(cues, ["audio", "memo"])).toBe(false);
    expect(canAddCuesToNumber(cues, ["missing"])).toBe(false);
  });

  it("returns only Number targets", () => {
    const cues = [cue("number", "number"), cue("audio", "audio"), cue("group", "group")];
    expect(numberTargets(cues).map((target) => target.id)).toEqual(["number"]);
  });
});
