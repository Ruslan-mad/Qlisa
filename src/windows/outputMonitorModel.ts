import type { OutputMonitorFrontendMetrics } from "../lib/types";

export const OUTPUT_MONITOR_HEADER_BYTES = 64;
export const OUTPUT_MONITOR_MAX_WIDTH = 640;
export const OUTPUT_MONITOR_MAX_HEIGHT = 360;
export const OUTPUT_MONITOR_TARGET_INTERVAL_MS = 1000 / 30;

export interface OutputMonitorDisplaySize { width: number; height: number; }

/** Fit the preview inside its viewport without changing its aspect ratio. */
export function fitOutputMonitorDisplaySize(
  availableWidth: number,
  availableHeight: number,
  aspectRatio: number,
  maxWidth = OUTPUT_MONITOR_MAX_WIDTH,
): OutputMonitorDisplaySize {
  if (![availableWidth, availableHeight, aspectRatio, maxWidth].every(Number.isFinite)
    || availableWidth <= 0 || availableHeight <= 0 || aspectRatio <= 0 || maxWidth <= 0) {
    return { width: 0, height: 0 };
  }
  const width = Math.min(availableWidth, maxWidth, availableHeight * aspectRatio);
  return { width, height: width / aspectRatio };
}

export type OutputMonitorStatus = "no_frame" | "frame" | "unchanged" | "black";

export interface OutputMonitorPacket {
  status: OutputMonitorStatus;
  width: number;
  height: number;
  stride: number;
  sequence: bigint;
  session: bigint;
  capturedAtUnixUs: bigint;
  respondedAtUnixUs: bigint;
  prepareDurationUs: number;
  pixels: Uint8Array;
}

const STATUS: Record<number, OutputMonitorStatus> = {
  0: "no_frame", 1: "frame", 2: "unchanged", 3: "black",
};
const EMPTY_PIXELS = new Uint8Array(0);

/** Validate the complete wire packet before creating any pixel view or buffer. */
export function parseOutputMonitorPacket(buffer: ArrayBuffer): OutputMonitorPacket {
  if (buffer.byteLength < OUTPUT_MONITOR_HEADER_BYTES) throw new Error("Output Monitor packet is shorter than its header");
  const view = new DataView(buffer);
  if (view.getUint8(0) !== 0x51 || view.getUint8(1) !== 0x4c || view.getUint8(2) !== 0x4d || view.getUint8(3) !== 0x46) throw new Error("Invalid Output Monitor packet magic");
  if (view.getUint16(4, true) !== 1 || view.getUint16(6, true) !== OUTPUT_MONITOR_HEADER_BYTES) throw new Error("Unsupported Output Monitor packet version or header length");
  const status = STATUS[view.getUint8(8)];
  if (!status || view.getUint8(9) !== 1 || view.getUint16(10, true) !== 0) throw new Error("Invalid Output Monitor packet format fields");

  const width = view.getUint32(12, true);
  const height = view.getUint32(16, true);
  const stride = view.getUint32(20, true);
  const sequence = view.getBigUint64(24, true);
  const session = view.getBigUint64(32, true);
  const capturedAtUnixUs = view.getBigUint64(40, true);
  const respondedAtUnixUs = view.getBigUint64(48, true);
  const prepareDurationUs = view.getUint32(56, true);
  const payloadLength = view.getUint32(60, true);

  if (status === "frame" || status === "black") {
    if (width < 1 || height < 1 || width > OUTPUT_MONITOR_MAX_WIDTH || height > OUTPUT_MONITOR_MAX_HEIGHT) throw new Error("Output Monitor dimensions are outside the supported range");
    if (stride !== width * 4) throw new Error("Invalid Output Monitor pixel stride");
  } else if (width !== 0 || height !== 0 || stride !== 0) {
    throw new Error("Empty Output Monitor status has pixel dimensions");
  }
  const expectedPayloadLength = status === "frame" ? stride * height : 0;
  if (payloadLength !== expectedPayloadLength || buffer.byteLength !== OUTPUT_MONITOR_HEADER_BYTES + payloadLength) throw new Error("Invalid Output Monitor payload length");

  return {
    status, width, height, stride, sequence, session, capturedAtUnixUs, respondedAtUnixUs,
    prepareDurationUs,
    pixels: status === "frame" ? new Uint8Array(buffer, OUTPUT_MONITOR_HEADER_BYTES, payloadLength) : EMPTY_PIXELS,
  };
}

export interface MonitorCadenceState { requestDurationMs: number; }
/** Delay after completion so request start times stay close to the target interval. */
export function outputMonitorNextDelay(state: MonitorCadenceState, targetMs = OUTPUT_MONITOR_TARGET_INTERVAL_MS): number {
  return Math.max(0, targetMs - Math.max(0, state.requestDurationMs));
}

export function outputMonitorPacketIsCurrent(packet: OutputMonitorPacket, selectionToken: number, lastSequence: bigint | null): boolean {
  return packet.session === BigInt(selectionToken) && (lastSequence === null || packet.sequence >= lastSequence);
}

export function outputMonitorGenerationIsCurrent(stopped: boolean, generation: number, currentGeneration: number): boolean {
  return !stopped && generation === currentGeneration;
}

export function clearOutputMonitorCanvas(canvas: HTMLCanvasElement): void {
  const context = canvas.getContext("2d");
  if (context) context.clearRect(0, 0, canvas.width, canvas.height);
}

export class OutputMonitorMetricsWindow {
  private lastReportAt: number | null = null;
  private received = 0;
  private displayed = 0;
  private ageLast: number | null = null;
  private ageSum = 0;
  private ageMax: number | null = null;
  private conversionSum = 0;
  private conversionMax: number | null = null;
  private requestSum = 0;
  private requestMax: number | null = null;
  private requests = 0;

  recordReceived(ageMs: number): void {
    this.received += 1;
    const age = Math.max(0, ageMs);
    this.ageLast = age; this.ageSum += age; this.ageMax = this.ageMax === null ? age : Math.max(this.ageMax, age);
  }
  recordRequest(requestMs: number): void {
    this.requests += 1;
    const request = Math.max(0, requestMs);
    this.requestSum += request; this.requestMax = this.requestMax === null ? request : Math.max(this.requestMax, request);
  }
  recordDisplayed(conversionMs: number): void {
    this.displayed += 1;
    const conversion = Math.max(0, conversionMs);
    this.conversionSum += conversion; this.conversionMax = this.conversionMax === null ? conversion : Math.max(this.conversionMax, conversion);
  }
  /** Return bounded 1 Hz metrics and reset only the reporting window counters. */
  take(now: number, sourceId: string | null, session: number | null, active: boolean, sampledAtUnixMs = Date.now()): OutputMonitorFrontendMetrics | null {
    if (this.lastReportAt === null) { this.lastReportAt = now; return null; }
    if (now - this.lastReportAt < 1000) return null;
    const elapsed = (now - this.lastReportAt) / 1000;
    this.lastReportAt = now;
    const result: OutputMonitorFrontendMetrics = {
      source_id: sourceId, session,
      received_fps: this.received / elapsed, displayed_fps: this.displayed / elapsed,
      received_frames: this.received, displayed_frames: this.displayed,
      frame_age_last_ms: this.ageLast,
      frame_age_average_ms: this.received ? this.ageSum / this.received : null, frame_age_max_ms: this.ageMax,
      conversion_average_ms: this.displayed ? this.conversionSum / this.displayed : null, conversion_max_ms: this.conversionMax,
      request_average_ms: this.requests ? this.requestSum / this.requests : null, request_max_ms: this.requestMax, active,
      sampled_at_unix_ms: sampledAtUnixMs,
    };
    this.received = 0; this.displayed = 0; this.ageLast = null; this.ageSum = 0; this.ageMax = null;
    this.conversionSum = 0; this.conversionMax = null; this.requestSum = 0; this.requestMax = null; this.requests = 0;
    return result;
  }
}

/** Keep a single ImageData object per resolution and swizzle BGRA in place. */
export function paintOutputMonitorFrame(
  canvas: HTMLCanvasElement,
  packet: OutputMonitorPacket,
  cache: { imageData: ImageData | null },
): void {
  if (packet.status === "black") {
    const context = canvas.getContext("2d");
    if (!context) return;
    if (canvas.width !== packet.width || canvas.height !== packet.height) {
      canvas.width = packet.width; canvas.height = packet.height;
    }
    context.fillStyle = "#000";
    context.fillRect(0, 0, canvas.width, canvas.height);
    return;
  }
  if (packet.status !== "frame") return;
  const context = canvas.getContext("2d");
  if (!context) return;
  if (canvas.width !== packet.width || canvas.height !== packet.height) {
    canvas.width = packet.width;
    canvas.height = packet.height;
    cache.imageData = null;
  }
  if (!cache.imageData || cache.imageData.width !== packet.width || cache.imageData.height !== packet.height) {
    cache.imageData = context.createImageData(packet.width, packet.height);
  }
  const out = cache.imageData.data;
  const src = packet.pixels;
  for (let i = 0; i < src.length; i += 4) {
    out[i] = src[i + 2]; out[i + 1] = src[i + 1]; out[i + 2] = src[i]; out[i + 3] = 255;
  }
  context.putImageData(cache.imageData, 0, 0);
}
