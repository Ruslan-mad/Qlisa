// Zone (c) — command wrapper argument-shape contract.
//
// Mocks the Tauri invoke bridge and asserts each wrapper forwards the exact
// command name and argument object the Rust side expects — including the
// non-obvious normalisation logic (default positions, null coalescing, rounding)
// that could silently drift and send a malformed payload.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { queueCueListPlayheadUpdate } from "../cueListGoQueue";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

import * as cmd from "../commands";

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe("command wrappers forward the correct name + args", () => {
  it("makes immediate GO wait for the latest row click", async () => {
    await queueCueListPlayheadUpdate(async () => {});
    const finish: Array<() => void> = [];
    invokeMock.mockImplementation((command: string) => {
      if (command === "set_playhead") {
        return new Promise<void>((resolve) => { finish.push(resolve); });
      }
      return Promise.resolve();
    });

    const first = queueCueListPlayheadUpdate(() => cmd.setPlayhead("cue-1"));
    const second = queueCueListPlayheadUpdate(() => cmd.setPlayhead("cue-2"));
    const starting = cmd.go();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(1));
    expect(invokeMock.mock.calls).toEqual([["set_playhead", { cueId: "cue-1" }]]);

    finish[0]();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(2));
    expect(invokeMock.mock.calls[1]).toEqual(["set_playhead", { cueId: "cue-2" }]);
    finish[1]();
    await Promise.all([first, second, starting]);
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["set_playhead", "set_playhead", "go"]);
  });

  it("keeps GO between the click before it and the click after it", async () => {
    await queueCueListPlayheadUpdate(async () => {});
    let finishA!: () => void;
    let finishGo!: () => void;
    let finishB!: () => void;
    invokeMock.mockImplementation((command: string, args?: { cueId?: string }) => {
      if (command === "set_playhead" && args?.cueId === "cue-a") {
        return new Promise<void>((resolve) => { finishA = resolve; });
      }
      if (command === "go") return new Promise<void>((resolve) => { finishGo = resolve; });
      if (command === "set_playhead" && args?.cueId === "cue-b") {
        return new Promise<void>((resolve) => { finishB = resolve; });
      }
      return Promise.resolve();
    });

    const clickA = queueCueListPlayheadUpdate(() => cmd.setPlayhead("cue-a"));
    const starting = cmd.go();
    const clickB = queueCueListPlayheadUpdate(() => cmd.setPlayhead("cue-b"));
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(1));
    expect(invokeMock.mock.calls).toEqual([["set_playhead", { cueId: "cue-a" }]]);

    finishA();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(2));
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["set_playhead", "go"]);
    finishGo();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(3));
    expect(invokeMock.mock.calls[2]).toEqual(["set_playhead", { cueId: "cue-b" }]);
    finishB();
    await Promise.all([clickA, starting, clickB]);
  });

  it("serializes rapid GO commands in their request order", async () => {
    await queueCueListPlayheadUpdate(async () => {});
    let finishFirst!: () => void;
    invokeMock.mockImplementationOnce(() => new Promise<void>((resolve) => { finishFirst = resolve; }));
    const first = cmd.go();
    const second = cmd.go();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(1));
    expect(invokeMock).toHaveBeenCalledWith("go");
    finishFirst();
    await Promise.all([first, second]);
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["go", "go"]);
  });

  it("does not GO after a failed row positioning until a later click succeeds", async () => {
    await queueCueListPlayheadUpdate(async () => {});
    const failure = new Error("position failed");
    invokeMock.mockRejectedValueOnce(failure);
    const positioning = queueCueListPlayheadUpdate(() => cmd.setPlayhead("cue-new"));
    const starting = cmd.go();
    await expect(positioning).rejects.toBe(failure);
    await expect(starting).rejects.toBe(failure);
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["set_playhead"]);

    invokeMock.mockResolvedValue(undefined);
    await queueCueListPlayheadUpdate(() => cmd.setPlayhead("cue-retried"));
    await cmd.go();
    expect(invokeMock.mock.calls.slice(-2).map(([command]) => command)).toEqual(["set_playhead", "go"]);
  });

  it.each([
    ["Stop All", () => cmd.stopAll(), "stop_all"],
    ["Hard Stop", () => cmd.hardStopAll(), "hard_stop_all"],
    ["Stop selected", () => cmd.stopCue("cue-selected"), "stop_cue"],
    ["Stop selected explicit", () => cmd.stopCueExplicit("cue-selected"), "stop_cue_explicit"],
  ])("cancels waiting GO when %s arrives, while preserving Playhead positioning", async (_label, stop, stopCommand) => {
    await queueCueListPlayheadUpdate(async () => {});
    let finish!: () => void;
    invokeMock.mockImplementation((command: string) => {
      if (command === "set_playhead") return new Promise<void>((resolve) => { finish = resolve; });
      return Promise.resolve();
    });
    const positioning = queueCueListPlayheadUpdate(() => cmd.setPlayhead("cue-selected"));
    const starting = cmd.go();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(1));
    const stopping = stop();
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["set_playhead", stopCommand]);
    finish();
    await Promise.all([positioning, starting, stopping]);
    expect(invokeMock.mock.calls.map(([command]) => command)).not.toContain("go");
  });

  it("blocks GO after a failed workspace transition instead of using the old show", async () => {
    await queueCueListPlayheadUpdate(async () => {});
    const failure = new Error("load failed");
    invokeMock.mockRejectedValueOnce(failure);
    await expect(cmd.loadWorkspace("new-show.inkue")).rejects.toBe(failure);
    await expect(cmd.go()).rejects.toBe(failure);
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["load_workspace"]);
  });

  it("Output Monitor sends monotonic selection tokens and uses binary frame responses", async () => {
    const first = cmd.setOutputMonitorSource("display-1", 100);
    const second = cmd.setOutputMonitorSource(null, 101);
    await Promise.all([first, second]);
    expect(invokeMock).toHaveBeenNthCalledWith(1, "set_output_monitor_source", { sourceId: "display-1", selectionToken: 100 });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "set_output_monitor_source", { sourceId: null, selectionToken: 101 });
    const binary = new ArrayBuffer(64);
    invokeMock.mockResolvedValueOnce(binary);
    await expect(cmd.getOutputMonitorFrame("display-1", 12, 102)).resolves.toBe(binary);
    expect(invokeMock).toHaveBeenLastCalledWith("get_output_monitor_frame", { sourceId: "display-1", afterSequence: 12, selectionToken: 102 });
  });
  it("drops concurrent frame polls across rapid source changes without queuing", async () => {
    let releaseFirst!: (value: ArrayBuffer) => void;
    invokeMock
      .mockImplementationOnce(() => new Promise<ArrayBuffer>((resolve) => { releaseFirst = resolve; }))
      .mockResolvedValueOnce(new ArrayBuffer(64));
    const first = cmd.getOutputMonitorFrame("display-1", null, 103);
    const skipped = await Promise.all(Array.from({ length: 99 }, (_, index) => cmd.getOutputMonitorFrame(`display-${index}`, null, 105 + index)));
    expect(skipped.every((result) => result === null)).toBe(true);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    releaseFirst(new ArrayBuffer(64));
    await first;
    const latest = await cmd.getOutputMonitorFrame("display-latest", null, 999);
    expect(latest).toBeInstanceOf(ArrayBuffer);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(invokeMock).toHaveBeenLastCalledWith("get_output_monitor_frame", { sourceId: "display-latest", afterSequence: null, selectionToken: 999 });
  });
  it("releases the frame semaphore when invoke rejects", async () => {
    invokeMock.mockRejectedValueOnce(new Error("test fault")).mockResolvedValueOnce(new ArrayBuffer(64));
    await expect(cmd.getOutputMonitorFrame("display-1", null, 1000)).rejects.toThrow("test fault");
    await expect(cmd.getOutputMonitorFrame("display-2", null, 1001)).resolves.toBeInstanceOf(ArrayBuffer);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });
  it("reports Output Monitor diagnostics through the existing snapshot API", async () => {
    const metrics = {
      source_id: "display-1", session: 110, active: true, received_fps: 30, displayed_fps: 30,
      received_frames: 30, displayed_frames: 30, frame_age_last_ms: 24, frame_age_average_ms: 26,
      frame_age_max_ms: 32, conversion_average_ms: 3, conversion_max_ms: 5,
      request_average_ms: 8, request_max_ms: 14, sampled_at_unix_ms: 1_800_000_000_000,
    };
    await cmd.reportOutputMonitorFrontendDiagnostics(metrics);
    expect(invokeMock).toHaveBeenCalledWith("report_output_monitor_frontend_diagnostics", { metrics });
    await cmd.getOutputMonitorDiagnostics("display-1");
    expect(invokeMock).toHaveBeenLastCalledWith("get_output_monitor_diagnostics", { sourceId: "display-1" });
  });

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
      startPaused: false,
    });
  });

  it("previewCue passes null markers when omitted", async () => {
    await cmd.previewCue("cue-1");
    expect(invokeMock).toHaveBeenCalledWith("preview_cue", {
      cueId: "cue-1",
      startMs: null,
      endMs: null,
      startPaused: false,
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
      startPaused: false,
      soundEnabled: true,
    });
  });

  it("preview start returns the authoritative event generation", async () => {
    invokeMock.mockResolvedValue({ voice_id: "voice-1", generation: 17 });
    await expect(cmd.previewCueOnHeadphones("cue-1", 500)).resolves.toEqual({
      voice_id: "voice-1",
      generation: 17,
    });
  });

  it("can start a headphone preview paused when the visual editor is paused", async () => {
    await cmd.previewCueOnHeadphones("cue-1", 500, 900, true);
    expect(invokeMock).toHaveBeenCalledWith("preview_cue", {
      cueId: "cue-1",
      startMs: 500,
      endMs: 900,
      startPaused: true,
      soundEnabled: true,
    });
  });

  it("sends headphone play, pause, and seek edges to the isolated session", async () => {
    await cmd.controlCuePreview("cue-1", "pause");
    await cmd.controlCuePreview("cue-1", "seek", 1500.6, 9000);
    await cmd.controlCuePreview("cue-1", "seek_paused", 1750);
    await cmd.controlCuePreview("cue-1", "unmute", undefined, undefined, true);
    expect(invokeMock).toHaveBeenNthCalledWith(1, "control_cue_preview", {
      cueId: "cue-1", action: "pause", positionMs: null, endMs: null, soundEnabled: null,
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "control_cue_preview", {
      cueId: "cue-1", action: "seek", positionMs: 1501, endMs: 9000, soundEnabled: null,
    });
    expect(invokeMock).toHaveBeenNthCalledWith(3, "control_cue_preview", {
      cueId: "cue-1", action: "seek_paused", positionMs: 1750, endMs: null, soundEnabled: null,
    });
    expect(invokeMock).toHaveBeenNthCalledWith(4, "control_cue_preview", {
      cueId: "cue-1", action: "unmute", positionMs: null, endMs: null, soundEnabled: true,
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
