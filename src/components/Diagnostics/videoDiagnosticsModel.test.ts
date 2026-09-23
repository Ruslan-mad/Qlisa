import { describe, expect, it } from "vitest";
import { videoCueModel, videoSummaryModel } from "./videoDiagnosticsModel";

describe("video diagnostics model", () => {
  it("reads the backend camelCase VideoCue fields without inventing missing values", () => {
    const cue = videoCueModel({
      cueId: "video-a",
      cueNumber: "42",
      cueName: "VideoCue A",
      filePath: "C:/media/a.mp4",
      width: 1920,
      height: 1080,
      fps: 59.94,
      outputs: ["main", "preview", "record"],
      outputCount: 3,
      mpvContexts: 3,
      mpvRenderContexts: 3,
      hardwareDecoding: true,
      hwdecBackend: "d3d11va",
      decoderFormat: "h264",
      droppedFrames: 2,
      delayedFrames: 1,
      timePosMs: 12500,
      maxOutputDesyncMs: 14,
      renderCalls: 100,
      averageRenderTimeUs: 1250,
      maximumRenderTimeUs: 4500,
      preload: false,
      playing: true,
      paused: false,
      eof: false,
    });

    expect(cue).toMatchObject({
      id: "video-a",
      cueNumber: "42",
      file: "C:/media/a.mp4",
      resolution: "1920×1080",
      fps: 59.94,
      outputs: 3,
      mpvContexts: 3,
      renderContexts: 3,
      hardwareDecode: true,
      hardwareBackend: "d3d11va",
      decoderFormat: "h264",
      droppedFrames: 2,
      delayedFrames: 1,
      timePosSeconds: 12.5,
      desyncMs: 14,
      renderCalls: 100,
      averageRenderMs: 1.25,
      maximumRenderMs: 4.5,
    });
  });

  it("derives global counts from VideoCue records when summary counters are absent", () => {
    const summary = videoSummaryModel({
      cues: [
        { cueId: "a", outputs: 3, mpvContexts: 3, droppedFrames: 2 },
        { cueId: "b", outputs: 1, mpvContexts: 1, droppedFrames: 0 },
      ],
    });

    expect(summary).toMatchObject({
      activeCues: 2,
      activeDecoders: 4,
      mpvContexts: 4,
      outputs: 4,
      droppedFrames: 2,
      multiDecodeCues: 1,
    });
  });

  it("keeps unavailable metrics unavailable instead of turning them into zero", () => {
    const summary = videoSummaryModel({ cues: [{ cueId: "a", cueName: "A" }] });
    expect(summary.cues[0].hardwareDecode).toBeNull();
    expect(summary.cues[0].desyncMs).toBeNull();
    expect(summary.droppedFrames).toBeNull();
    expect(summary.mpvContexts).toBeNull();
  });
});
