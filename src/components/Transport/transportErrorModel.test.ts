import { describe, expect, it } from "vitest";
import { transportErrorFromEvent } from "./transportErrorModel";

describe("transport error event", () => {
  it("reads an error and clears on null", () => {
    expect(transportErrorFromEvent(new CustomEvent("inkue:transport-error", { detail: "Number is not ready" }))).toBe("Number is not ready");
    expect(transportErrorFromEvent(new CustomEvent("inkue:transport-error", { detail: null }))).toBeNull();
  });
});
