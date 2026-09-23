import { describe, expect, it } from "vitest";
import { networkState, networkSummaryModel } from "./networkDiagnosticsModel";

describe("network diagnostics model", () => {
  it("translates transport states to Russian labels", () => {
    expect(networkState("Connected")).toEqual({ label: "Подключено", tone: "good" });
    expect(networkState("reconnecting")).toEqual({ label: "Переподключение", tone: "warn" });
    expect(networkState("failed")).toEqual({ label: "Ошибка", tone: "bad" });
  });

  it("does not invent summary counters when backend fields are absent", () => {
    const model = networkSummaryModel({
      connections: [
        { id: "in-1", name: "Камера", protocol: "srt", direction: "incoming", state: "connected" },
        { id: "out-1", name: "Эфир", protocol: "ndi", direction: "outgoing", state: "reconnecting", errors: 2 },
      ],
    });
    expect(model.active).toBeNull();
    expect(model.incoming).toBeNull();
    expect(model.outgoing).toBeNull();
    expect(model.connected).toBeNull();
    expect(model.reconnecting).toBeNull();
    expect(model.connections[1].problem).toBe(true);
  });

  it("does not invent aggregate bitrate or rows", () => {
    const model = networkSummaryModel({});
    expect(model.incomingBitrate).toBeNull();
    expect(model.outgoingBitrate).toBeNull();
    expect(model.connections).toEqual([]);
    expect(model.active).toBeNull();
  });

  it("keeps a receiving stream connected when FFmpeg only reports a warning", () => {
    const model = networkSummaryModel({
      connections: [{
        id: "srt-1",
        name: "SRT",
        protocol: "srt",
        direction: "input",
        state: "connected",
        ffmpegRunning: true,
        receivedFrames: 12,
        lastWarning: "non-existing PPS referenced",
      }],
    });
    expect(model.connections[0].state).toEqual({ label: "Подключено", tone: "warn" });
    expect(model.connections[0].problem).toBe(false);
    expect(model.errors).toBeNull();
  });
});
