import { describe, expect, it } from "vitest";
import type { MediaRuntimeStatus } from "../types";
import { resolveMediaRuntimeFailure, resolveMediaRuntimeStartup } from "../mediaRuntimeBootstrapModel";

const status = (overrides: Partial<MediaRuntimeStatus> = {}): MediaRuntimeStatus => ({
  ffmpeg: true,
  ffprobe: true,
  libmpv: true,
  ready: true,
  version: "1",
  state: "ready",
  installed_kit_id: "kit-1",
  recommended_kit_id: "kit-1",
  update_available: false,
  repair_reason: null,
  operation_pending: false,
  ...overrides,
});

describe("media runtime bootstrap policy", () => {
  it("prepares an empty runtime", () => {
    expect(resolveMediaRuntimeStartup(status({ state: "missing", ready: false, operation_pending: false }))).toBe("prepare");
  });

  it("runs with a ready installed kit", () => {
    expect(resolveMediaRuntimeStartup(status())).toBe("run");
  });

  it("runs an older compatible kit without applying its available update", () => {
    expect(resolveMediaRuntimeStartup(status({ state: "update-available", update_available: true, installed_kit_id: "kit-old" }))).toBe("run");
  });

  it("requires explicit repair for a damaged kit", () => {
    const damaged = status({ state: "repair-required", ready: false });
    expect(resolveMediaRuntimeStartup(damaged)).toBe("repair");
    expect(resolveMediaRuntimeFailure(damaged)).toEqual({ view: "repair", primary: "schedule-repair" });
  });

  it("executes an explicitly queued operation even when the old kit is ready", () => {
    expect(resolveMediaRuntimeStartup(status({ state: "update-available", update_available: true, operation_pending: true }))).toBe("prepare");
  });

  it("offers update retry and installed-kit launch after an update fails but old kit remains ready", () => {
    const failedUpdateStatus = status({ state: "ready", operation_pending: false });
    expect(resolveMediaRuntimeFailure(failedUpdateStatus)).toEqual({
      view: "fallback",
      primary: "schedule-update",
      secondary: "run-installed",
    });
  });

  it("returns to manual repair after a failed operation leaves no compatible kit", () => {
    expect(resolveMediaRuntimeFailure(status({ state: "repair-required", ready: false, operation_pending: false }))).toEqual({
      view: "repair",
      primary: "schedule-repair",
    });
  });

  it("does not offer launch with installed components while a failed relaunch left an operation pending", () => {
    expect(resolveMediaRuntimeFailure(status({ state: "ready", operation_pending: true }))).toEqual({
      view: "error",
      primary: "retry-missing",
    });
  });
});
