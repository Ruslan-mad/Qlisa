import { describe, expect, it } from "vitest";
import { timerPlayButtonModel } from "./timerTransportModel";

describe("timer play button", () => {
  it("starts standby/completed cues and pauses a running cycle before zero", () => {
    expect(timerPlayButtonModel("standby", false, null)).toEqual({ icon: "play", action: "go", label: "start" });
    expect(timerPlayButtonModel("completed", false, null).action).toBe("go");
    expect(timerPlayButtonModel("running", false, null)).toEqual({ icon: "pause", action: "pause", label: "pause" });
  });

  it("resumes pause before zero and starts a new cycle after the runtime zero latch", () => {
    expect(timerPlayButtonModel("paused", false, null).action).toBe("resume");
    expect(timerPlayButtonModel("running", true, null)).toEqual({ icon: "play", action: "go", label: "start" });
    expect(timerPlayButtonModel("paused", true, null).action).toBe("go");
  });

  it("does not mistake pre-wait or missing timing for completion", () => {
    expect(timerPlayButtonModel("running", true, "pre_wait").action).toBe("pause");
    expect(timerPlayButtonModel("running", false, null).action).toBe("pause");
  });
});
