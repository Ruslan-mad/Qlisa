import { describe, expect, it } from "vitest";
import { outputMonitorDiagnosticsModel } from "./outputMonitorDiagnosticsModel";

describe("Output Monitor diagnostics DTO", () => {
  it("reads the actual nested backend snapshot and frontend metrics", () => {
    const model = outputMonitorDiagnosticsModel({
      capture: { attempts: 31, captured: 30, skippedPbo: 1, droppedStale: 2, published: 28, totalCopyUs: 9000, maximumCopyUs: 700, previewFps: 29.8 },
      binary: { requests: 30, framePackets: 28, blackPackets: 0, unchangedPackets: 2, noFramePackets: 0, payloadBytes: 25_804_800, totalPrepareUs: 400, maximumPrepareUs: 40 },
      frontend: { sourceId: "display-1", session: 42, active: true, receivedFps: 28, displayedFps: 28, receivedFrames: 28, displayedFrames: 28, frameAgeLastMs: 40, frameAgeAverageMs: 33, frameAgeMaxMs: 51, conversionAverageMs: 4, conversionMaxMs: 6, requestAverageMs: 7, requestMaxMs: 18, sampledAtUnixMs: 1_800_000_000_000 },
    });
    expect(model?.capture?.published).toBe(28);
    expect(model?.binary?.framePackets).toBe(28);
    expect(model?.frontend?.displayedFps).toBe(28);
    expect(model).toMatchObject({ sourceId: "display-1", active: true });
  });
  it("does not invent values for a missing optional frontend snapshot", () => {
    expect(outputMonitorDiagnosticsModel({ capture: null, binary: { requests: 0 }, frontend: null })).toMatchObject({ sourceId: null, active: false, frontend: null });
    expect(outputMonitorDiagnosticsModel(null)).toBeNull();
  });
});
