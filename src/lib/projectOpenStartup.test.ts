import { describe, expect, it, vi } from "vitest";
import { createProjectOpenStartupGate } from "./projectOpenStartup";

describe("startup project-open event barrier", () => {
  it("holds a cold-start open until the workspace event listeners finish registering", () => {
    const drain = vi.fn();
    const gate = createProjectOpenStartupGate(drain);

    // Tauri has delivered the startup request while the other listeners are
    // still awaiting registration.
    gate.requestDrain();
    expect(drain).not.toHaveBeenCalled();

    gate.markListenersReady();
    expect(drain).toHaveBeenCalledTimes(1);
  });

  it("drains later requests immediately and ignores work after disposal", () => {
    const drain = vi.fn();
    const gate = createProjectOpenStartupGate(drain);
    gate.markListenersReady();
    gate.requestDrain();
    expect(drain).toHaveBeenCalledTimes(1);
    gate.dispose();
    gate.requestDrain();
    expect(drain).toHaveBeenCalledTimes(1);
  });
});
