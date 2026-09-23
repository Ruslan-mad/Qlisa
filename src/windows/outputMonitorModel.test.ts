import { describe, expect, it } from "vitest";
import { EMPTY_OUTPUT_MONITOR_FRAME, frameForOutputSource, mergeOutputMonitorFrame } from "./outputMonitorModel";

const response = (overrides: Partial<Parameters<typeof mergeOutputMonitorFrame>[1]> = {}) => ({
  source_id: "stage",
  status: "frame" as const,
  sequence: 4,
  width: 640,
  height: 360,
  data_url: "data:image/bmp;base64,frame",
  error: null,
  ...overrides,
});

describe("output monitor frame model", () => {
  it("installs a fresh frame", () => {
    expect(mergeOutputMonitorFrame(EMPTY_OUTPUT_MONITOR_FRAME, response())).toEqual({
      sourceId: "stage",
      sequence: 4,
      dataUrl: "data:image/bmp;base64,frame",
      status: "frame",
      error: null,
    });
  });

  it("keeps the last image when the backend reports no change", () => {
    const current = mergeOutputMonitorFrame(EMPTY_OUTPUT_MONITOR_FRAME, response());
    const next = mergeOutputMonitorFrame(current, response({ status: "unchanged", sequence: 5, data_url: null }));
    expect(next.dataUrl).toBe(current.dataUrl);
    expect(next.status).toBe("frame");
    expect(next.sequence).toBe(5);
  });

  it("turns an explicit black program frame into black instead of a stale image", () => {
    const current = mergeOutputMonitorFrame(EMPTY_OUTPUT_MONITOR_FRAME, response());
    const next = mergeOutputMonitorFrame(current, response({ status: "black", data_url: null }));
    expect(next.dataUrl).toBeNull();
    expect(next.status).toBe("black");
  });

  it.each(["no_frame", "unavailable", "error"] as const)("clears a stale image for %s", (status) => {
    const current = mergeOutputMonitorFrame(EMPTY_OUTPUT_MONITOR_FRAME, response());
    const next = mergeOutputMonitorFrame(current, response({ status, data_url: null }));
    expect(next.dataUrl).toBeNull();
    expect(next.status).toBe(status);
  });

  it("keeps an unchanged black state without turning it into waiting", () => {
    const black = mergeOutputMonitorFrame(EMPTY_OUTPUT_MONITOR_FRAME, response({ status: "black", data_url: null }));
    const unchanged = mergeOutputMonitorFrame(black, response({ status: "unchanged", sequence: 5, data_url: null }));
    expect(unchanged.status).toBe("black");
    expect(unchanged.dataUrl).toBeNull();
  });

  it("does not expose the previous tab's frame after a source switch", () => {
    const stage = mergeOutputMonitorFrame(EMPTY_OUTPUT_MONITOR_FRAME, response());
    expect(frameForOutputSource(stage, "lobby")).toBe(EMPTY_OUTPUT_MONITOR_FRAME);
    expect(frameForOutputSource(stage, "stage")).toBe(stage);
  });
});
