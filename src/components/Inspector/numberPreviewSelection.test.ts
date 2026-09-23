import { describe, expect, it } from "vitest";
import type { NumberCueData } from "../../lib/types";
import { resolveMonitorPreviewSelection, resolveNumberPreviewCue } from "./numberPreviewSelection";

const child = (id: string, cue_type: "audio" | "video" | "image" | "group", duration_ms: number, file_path = `${id}.media`) => ({
  id, cue_type, name: id, number: null, notes: "", state: "standby" as const,
  continue_mode: "do_not_continue" as const, color: "none" as const, pre_wait_ms: 0, post_wait_ms: 0,
  duration_ms, file_path, file_size_bytes: null, media_width: null, media_height: null,
  media_file_missing: false, is_loading: false, is_disabled: false, is_broken: false,
  is_warning: false,
});

const numberCue = (children: NumberCueData["children"], masterId: string): NumberCueData => ({
  ...child("number", "group", 4000, ""),
  cue_type: "number",
  children,
  number_master_id: masterId,
  number_action_offsets_ms: { image: 1000 },
  file_path: null,
});

describe("Number output-monitor preview selection", () => {
  it("selects the master visual at the initial Number cursor", () => {
    const master = child("master", "video", 4000);
    const cue = numberCue([master, child("image", "image", 1000)], "master");
    expect(resolveNumberPreviewCue(cue, 0)?.id).toBe("master");
  });

  it("selects the active visual child at the shared cursor", () => {
    const cue = numberCue([child("master", "video", 4000), child("image", "image", 1000)], "master");
    expect(resolveNumberPreviewCue(cue, 1200)?.id).toBe("image");
  });

  it("preserves an existing monitor selection during an audio-only Number gap", () => {
    const audioMaster = child("master", "audio", 4000);
    const cue = numberCue([audioMaster], "master");
    expect(resolveMonitorPreviewSelection(cue, cue, 0)).toBeUndefined();
  });

  it("keeps ordinary video/image and clear semantics unchanged", () => {
    const video = child("video", "video", 1000);
    expect(resolveMonitorPreviewSelection(video, null, 0)).toEqual({ cueId: "video" });
    expect(resolveMonitorPreviewSelection(null, null, 0)).toBeNull();
  });
});
