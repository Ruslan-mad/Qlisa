import { describe, expect, it } from "vitest";
import type { OutputControlStatus } from "../../lib/types";
import { hasMonitorConflict, monitorOwner, physicalDisplayOutputs, physicalOutputsVisible } from "./fullscreenModel";

const output = (id: string, network = false, visible = false, detail: string | null = null, active = true): OutputControlStatus => ({
  output_id: id,
  name: id,
  monitor: null,
  network,
  visible,
  available: visible,
  healthy: visible,
  ftb: false,
  active,
  detail,
});

describe("fullscreen control model", () => {
  it("exposes only enabled physical display outputs", () => {
    expect(physicalDisplayOutputs([
      output("main", false),
      output("inactive", false, false, "Output is not active", false),
      output("ndi", true),
      output("srt", true),
    ]).map((item) => item.output_id)).toEqual(["main"]);
  });

  it("finds occupied monitors and marks only actual duplicate assignments", () => {
    const main = output("main");
    const preview = output("preview");
    main.monitor = 0;
    preview.monitor = 1;
    expect(monitorOwner([main, preview], 0, "preview")?.output_id).toBe("main");
    expect(hasMonitorConflict([main, preview], main)).toBe(false);
    preview.monitor = 0;
    expect(hasMonitorConflict([main, preview], main)).toBe(true);
    expect(hasMonitorConflict([main, preview], preview)).toBe(true);
  });

  it("matches backend aggregate visibility semantics", () => {
    const hidden = output("hidden", false, false);
    const visible = output("visible", false, true);
    expect(physicalOutputsVisible([hidden, visible])).toBe(true);
    expect(physicalOutputsVisible([hidden])).toBe(false);
  });
});
