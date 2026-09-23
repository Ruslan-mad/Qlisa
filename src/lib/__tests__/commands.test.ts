// Zone (c) — command wrapper argument-shape contract.
//
// Mocks the Tauri invoke bridge and asserts each wrapper forwards the exact
// command name and argument object the Rust side expects — including the
// non-obvious normalisation logic (default positions, null coalescing, rounding)
// that could silently drift and send a malformed payload.

import { describe, it, expect, beforeEach, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

import * as cmd from "../commands";

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe("command wrappers forward the correct name + args", () => {
  it("Number commands preserve master and offset payloads", async () => {
    await cmd.addNumberAction("number-1", "audio", 250);
    expect(invokeMock).toHaveBeenCalledWith("add_number_action", {
      numberId: "number-1", cueType: "audio", offsetMs: 250,
    });
    await cmd.setNumberMaster("number-1", "master-1");
    expect(invokeMock).toHaveBeenCalledWith("set_number_master", {
      numberId: "number-1", childId: "master-1",
    });
    await cmd.setNumberActionOffset("number-1", "action-1", 900);
    expect(invokeMock).toHaveBeenCalledWith("set_number_action_offset", {
      numberId: "number-1", actionId: "action-1", offsetMs: 900,
    });
    await cmd.removeNumberAction("number-1", "action-1");
    expect(invokeMock).toHaveBeenCalledWith("remove_number_action", {
      numberId: "number-1", actionId: "action-1",
    });
  });
  it("testPreviewAudio keeps the preview route separate from the program route", async () => {
    await cmd.testPreviewAudio("headphones", "wasapi_shared", null);
    expect(invokeMock).toHaveBeenCalledWith("test_preview_audio", {
      previewDeviceId: "headphones",
      backend: "wasapi_shared",
      previewAsioPair: null,
    });
  });

  it("goCue → go_cue { cueId }", async () => {
    await cmd.goCue("cue-1");
    expect(invokeMock).toHaveBeenCalledWith("go_cue", { cueId: "cue-1" });
  });

  it("seekCue → seek_cue { cueId, positionMs }", async () => {
    await cmd.seekCue("cue-1", 1234.6);
    expect(invokeMock).toHaveBeenCalledWith("seek_cue", { cueId: "cue-1", positionMs: 1235 });
  });

  it("seekCueMedia rounds pointer positions before sending a Rust u64", async () => {
    await cmd.seekCueMedia("cue-1", -0.4);
    expect(invokeMock).toHaveBeenCalledWith("seek_cue_media", { cueId: "cue-1", filePositionMs: 0 });
  });

  it("addCue defaults position to -1 (append)", async () => {
    await cmd.addCue("audio");
    expect(invokeMock).toHaveBeenCalledWith("add_cue", { cueType: "audio", position: -1 });
  });

  it("addTargetedCue forwards the clicked target and insertion position", async () => {
    await cmd.addTargetedCue("stop", "cue-10", 4);
    expect(invokeMock).toHaveBeenCalledWith("add_targeted_cue", {
      cueType: "stop",
      targetCueId: "cue-10",
      position: 4,
    });
  });

  it("moveCues forwards a null beforeId verbatim", async () => {
    await cmd.moveCues(["a", "b"], null);
    expect(invokeMock).toHaveBeenCalledWith("move_cues", { ids: ["a", "b"], beforeId: null });
  });

  it("pasteCue with no arg coalesces to afterCueId: null", async () => {
    await cmd.pasteCue();
    expect(invokeMock).toHaveBeenCalledWith("paste_cue", { afterCueId: null });
  });

  it("copyCues forwards an ordered playlist selection", async () => {
    await cmd.copyCues(["cue-a", "cue-group", "cue-b"]);
    expect(invokeMock).toHaveBeenCalledWith("copy_cues", {
      cueIds: ["cue-a", "cue-group", "cue-b"],
    });
  });

  it("updateCue nests the partial properties object", async () => {
    await cmd.updateCue("cue-1", { volume_db: -6, pan: 0.5 } as never);
    expect(invokeMock).toHaveBeenCalledWith("update_cue", {
      cueId: "cue-1",
      properties: { volume_db: -6, pan: 0.5 },
    });
  });

  it("setCueDuration forwards null for an indefinite Image/Text duration", async () => {
    await cmd.setCueDuration("cue-1", null);
    expect(invokeMock).toHaveBeenCalledWith("set_cue_duration", {
      cueId: "cue-1",
      durationMs: null,
    });
  });

  it("getCues keeps the requested cue ID order", async () => {
    await cmd.getCues(["cue-b", "cue-a"]);
    expect(invokeMock).toHaveBeenCalledWith("get_cues", { cueIds: ["cue-b", "cue-a"] });
  });

  it("bulkUpdateCues sends one atomic per-cue patch batch", async () => {
    const updates = [
      { cueId: "cue-a", properties: { color: "blue", notes: "ready" } },
      { cueId: "cue-b", properties: { color: "blue", notes: "ready" } },
    ];
    await cmd.bulkUpdateCues(updates);
    expect(invokeMock).toHaveBeenCalledWith("bulk_update_cues", { updates });
  });

  it("previewCue rounds fractional millisecond markers to integers", async () => {
    await cmd.previewCue("cue-1", 12.7, 40.2);
    expect(invokeMock).toHaveBeenCalledWith("preview_cue", {
      cueId: "cue-1",
      startMs: 13,
      endMs: 40,
    });
  });

  it("previewCue passes null markers when omitted", async () => {
    await cmd.previewCue("cue-1");
    expect(invokeMock).toHaveBeenCalledWith("preview_cue", {
      cueId: "cue-1",
      startMs: null,
      endMs: null,
    });
  });

  it("prepareVideoPreview authorizes by cue ID instead of accepting an arbitrary path", async () => {
    invokeMock.mockResolvedValue("C:\\show\\clip.mov");

    await expect(cmd.prepareVideoPreview("video-cue-1")).resolves.toBe("C:\\show\\clip.mov");
    expect(invokeMock).toHaveBeenCalledWith("prepare_video_preview", {
      cueId: "video-cue-1",
    });
  });

  it("previewCueOnHeadphones uses a file-position argument", async () => {
    await cmd.previewCueOnHeadphones("cue-1", 12.7, 40.2);
    expect(invokeMock).toHaveBeenCalledWith("preview_cue", {
      cueId: "cue-1",
      startMs: 13,
      endMs: 40,
    });
  });

  it("preview start returns the authoritative event generation", async () => {
    invokeMock.mockResolvedValue({ voice_id: "voice-1", generation: 17 });
    await expect(cmd.previewCueOnHeadphones("cue-1", 500)).resolves.toEqual({
      voice_id: "voice-1",
      generation: 17,
    });
  });

  it("toggleCuePreview and stopCuePreview use their session commands", async () => {
    await cmd.toggleCuePreview("cue-1", 99.9);
    expect(invokeMock).toHaveBeenCalledWith("toggle_cue_preview", {
      cueId: "cue-1",
      positionMs: 100,
      endMs: null,
    });
    await cmd.stopCuePreview();
    expect(invokeMock).toHaveBeenCalledWith("stop_cue_preview");
  });

  it("listAudioDevices coalesces an omitted backend to null", async () => {
    invokeMock.mockResolvedValue([]);
    await cmd.listAudioDevices();
    expect(invokeMock).toHaveBeenCalledWith("list_audio_devices", { backend: null });
  });

  it("listPreviewAudioDevices uses its backend-independent command", async () => {
    invokeMock.mockResolvedValue([]);
    await cmd.listPreviewAudioDevices();
    expect(invokeMock).toHaveBeenCalledWith("list_preview_audio_devices");
  });

  it("setAudioFile → set_audio_file { cueId, filePath }", async () => {
    await cmd.setAudioFile("cue-1", "audio/track.wav");
    expect(invokeMock).toHaveBeenCalledWith("set_audio_file", {
      cueId: "cue-1",
      filePath: "audio/track.wav",
    });
  });

  it("setOutputPatch forwards the full patch definition", async () => {
    invokeMock.mockResolvedValue("patch-id");
    await cmd.setOutputPatch(null, "Main", "dev-1", [0, 1]);
    expect(invokeMock).toHaveBeenCalledWith("set_output_patch", {
      patchId: null,
      name: "Main",
      deviceId: "dev-1",
      channels: [0, 1],
    });
  });

  it("setDisplayOutputMonitor targets one named physical output", async () => {
    await cmd.setDisplayOutputMonitor("display-1", 2);
    expect(invokeMock).toHaveBeenCalledWith("set_display_output_monitor", {
      outputId: "display-1",
      monitor: 2,
    });
  });
});
