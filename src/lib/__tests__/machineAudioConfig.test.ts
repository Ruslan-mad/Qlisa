import { describe, expect, it } from "vitest";
import { DEFAULT_MACHINE_AUDIO_CONFIG, machineAudioConfigsEqual } from "../types";

describe("machine audio Apply diff", () => {
  it("treats unchanged settings as a strict no-op", () => {
    expect(machineAudioConfigsEqual(DEFAULT_MACHINE_AUDIO_CONFIG, { ...DEFAULT_MACHINE_AUDIO_CONFIG })).toBe(true);
  });

  it("detects every hardware setting that requires a restart", () => {
    for (const patch of [
      { backend: "wasapi_exclusive" as const },
      { device_id: "device" },
      { device_name: "Speakers" },
      { preview_device_id: "headphones" },
      { preview_device_name: "Headphones" },
      { preview_asio_pair: 2 },
      { input_device_id: "mic" },
      { buffer_size: 512 },
      { asio_out_pair: 1 },
    ]) {
      expect(machineAudioConfigsEqual(DEFAULT_MACHINE_AUDIO_CONFIG, { ...DEFAULT_MACHINE_AUDIO_CONFIG, ...patch })).toBe(false);
    }
  });
});
