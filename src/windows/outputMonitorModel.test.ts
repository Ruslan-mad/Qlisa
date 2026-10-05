import { describe, expect, it } from "vitest";
import {
  OUTPUT_MONITOR_HEADER_BYTES,
  OutputMonitorMetricsWindow,
  clearOutputMonitorCanvas,
  outputMonitorGenerationIsCurrent,
  outputMonitorPacketIsCurrent,
  outputMonitorNextDelay,
  paintOutputMonitorFrame,
  parseOutputMonitorPacket,
} from "./outputMonitorModel";

function packet(status: number, payload: number[] = [], width = status === 1 || status === 3 ? 1 : 0, height = width): ArrayBuffer {
  const buffer = new ArrayBuffer(OUTPUT_MONITOR_HEADER_BYTES + payload.length);
  const view = new DataView(buffer);
  for (const [i, ch] of [..."QLMF"].entries()) view.setUint8(i, ch.charCodeAt(0));
  view.setUint16(4, 1, true); view.setUint16(6, OUTPUT_MONITOR_HEADER_BYTES, true);
  view.setUint8(8, status); view.setUint8(9, 1);
  view.setUint32(12, width, true); view.setUint32(16, height, true);
  view.setUint32(20, width * 4, true);
  view.setBigUint64(24, 123n, true); view.setBigUint64(32, 456n, true);
  view.setBigUint64(40, 1_700_000_000_000_000n, true); view.setBigUint64(48, 1_700_000_000_000_100n, true);
  view.setUint32(56, 17, true); view.setUint32(60, payload.length, true);
  new Uint8Array(buffer, OUTPUT_MONITOR_HEADER_BYTES).set(payload);
  return buffer;
}

describe("Output Monitor binary packet", () => {
  it("parses frame data and preserves sequence/session as u64", () => {
    const result = parseOutputMonitorPacket(packet(1, [3, 2, 1, 255]));
    expect(result.status).toBe("frame");
    expect(result.sequence).toBe(123n);
    expect(result.session).toBe(456n);
    expect([...result.pixels]).toEqual([3, 2, 1, 255]);
  });
  it.each([[0, "no_frame"], [2, "unchanged"], [3, "black"]] as const)("accepts empty status %s", (code, status) => {
    const result = parseOutputMonitorPacket(packet(code));
    expect(result.status).toBe(status);
    expect(result.pixels.byteLength).toBe(0);
  });
  it("rejects malformed headers, dimensions and payload lengths", () => {
    const badMagic = packet(0); new Uint8Array(badMagic)[0] = 0;
    expect(() => parseOutputMonitorPacket(badMagic)).toThrow(/magic/);
    expect(() => parseOutputMonitorPacket(new ArrayBuffer(63))).toThrow(/shorter/);
    const badVersion = packet(0); new DataView(badVersion).setUint16(4, 2, true);
    expect(() => parseOutputMonitorPacket(badVersion)).toThrow(/version/);
    const badHeaderLength = packet(0); new DataView(badHeaderLength).setUint16(6, 65, true);
    expect(() => parseOutputMonitorPacket(badHeaderLength)).toThrow(/header length/);
    const badPixelFormat = packet(0); new DataView(badPixelFormat).setUint8(9, 2);
    expect(() => parseOutputMonitorPacket(badPixelFormat)).toThrow(/format/);
    const badReserved = packet(0); new DataView(badReserved).setUint16(10, 1, true);
    expect(() => parseOutputMonitorPacket(badReserved)).toThrow(/format/);
    const badStatus = packet(0); new DataView(badStatus).setUint8(8, 9);
    expect(() => parseOutputMonitorPacket(badStatus)).toThrow(/format/);
    expect(() => parseOutputMonitorPacket(packet(1, [1, 2, 3, 4], 641, 1))).toThrow(/dimensions/);
    expect(() => parseOutputMonitorPacket(packet(1, [1, 2, 3], 1, 1))).toThrow(/payload/);
    expect(() => parseOutputMonitorPacket(packet(2, [1]))).toThrow(/payload/);
    const badStride = packet(1, [1, 2, 3, 4]); new DataView(badStride).setUint32(20, 8, true);
    expect(() => parseOutputMonitorPacket(badStride)).toThrow(/stride/);
    const trailingBytes = new Uint8Array(65); trailingBytes.set(new Uint8Array(packet(0)));
    expect(() => parseOutputMonitorPacket(trailingBytes.buffer)).toThrow(/payload/);
  });
});

describe("Output Monitor polling and metrics", () => {
  it("accounts for request duration and never schedules catch-up work", () => {
    expect(outputMonitorNextDelay({ requestDurationMs: 5 })).toBeCloseTo(1000 / 30 - 5);
    expect(outputMonitorNextDelay({ requestDurationMs: 40 })).toBe(0);
  });
  it("rejects stale session, sequence and lifecycle results", () => {
    const current = parseOutputMonitorPacket(packet(1, [1, 2, 3, 4]));
    expect(outputMonitorPacketIsCurrent(current, 456, null)).toBe(true);
    expect(outputMonitorPacketIsCurrent(current, 999, null)).toBe(false);
    expect(outputMonitorPacketIsCurrent(current, 456, 124n)).toBe(false);
    expect(outputMonitorGenerationIsCurrent(false, 7, 7)).toBe(true);
    expect(outputMonitorGenerationIsCurrent(false, 7, 8)).toBe(false);
    expect(outputMonitorGenerationIsCurrent(true, 7, 7)).toBe(false);
  });
  it("reports bounded 1 Hz received/displayed and timing metrics", () => {
    const metrics = new OutputMonitorMetricsWindow();
    metrics.recordReceived(20); metrics.recordReceived(40); metrics.recordRequest(10); metrics.recordRequest(20); metrics.recordDisplayed(2);
    expect(metrics.take(500, "display-1", 99, true)).toBeNull();
    const report = metrics.take(1500, "display-1", 99, true)!;
    expect(report).toMatchObject({ source_id: "display-1", session: 99, active: true, received_frames: 2, displayed_frames: 1, received_fps: 2, displayed_fps: 1, frame_age_average_ms: 30, frame_age_max_ms: 40, conversion_average_ms: 2, request_average_ms: 15 });
    expect(metrics.take(2499, null, null, false)).toBeNull();
  });
  it("reuses ImageData, paints BGRA as RGBA and keeps black output on the same canvas", () => {
    const state = { imageData: null as ImageData | null };
    const canvas = {
      width: 300, height: 150,
      getContext: () => context,
    } as unknown as HTMLCanvasElement;
    const imageData = { width: 1, height: 1, data: new Uint8ClampedArray(4) } as ImageData;
    const context = {
      createImageData: () => imageData,
      putImageData: () => undefined,
      fillStyle: "",
      fillRect: () => undefined,
    } as unknown as CanvasRenderingContext2D;
    const first = parseOutputMonitorPacket(packet(1, [9, 8, 7, 6]));
    paintOutputMonitorFrame(canvas, first, state);
    const retained = state.imageData;
    expect([...imageData.data]).toEqual([7, 8, 9, 255]);
    paintOutputMonitorFrame(canvas, parseOutputMonitorPacket(packet(3)), state);
    paintOutputMonitorFrame(canvas, first, state);
    expect(state.imageData).toBe(retained);
    expect(canvas.width).toBe(1);
    expect(canvas.height).toBe(1);
  });
  it("converts red, green, blue and white pixels and clears a no_frame canvas", () => {
    const state = { imageData: null as ImageData | null };
    const imageData = { width: 4, height: 1, data: new Uint8ClampedArray(16) } as ImageData;
    let clearCalls = 0;
    const context = {
      createImageData: () => imageData, putImageData: () => undefined,
      fillStyle: "", fillRect: () => undefined,
      clearRect: () => { clearCalls += 1; },
    } as unknown as CanvasRenderingContext2D;
    const canvas = { width: 4, height: 1, getContext: () => context } as unknown as HTMLCanvasElement;
    const colors = parseOutputMonitorPacket(packet(1, [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255], 4, 1));
    paintOutputMonitorFrame(canvas, colors, state);
    expect([...imageData.data]).toEqual([255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]);
    clearOutputMonitorCanvas(canvas);
    expect(clearCalls).toBe(1);
  });
});
