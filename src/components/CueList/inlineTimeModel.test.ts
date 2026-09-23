import { describe, expect, it, vi } from "vitest";
import type { CueType } from "../../lib/types";
import {
  canEditCueDuration,
  emptyCueDurationValue,
  formatEditableMilliseconds,
  formatInlineTimeInput,
  isInlineTimeActivationKey,
  parseInlineTimeInput,
  resolveInlineTimeCommit,
  stopInlineTimeCellEvent,
} from "./inlineTimeModel";

describe("CueList inline time model", () => {
  it("allows duration editing only for finite/indefinite duration-capable types", () => {
    const editable: CueType[] = ["wait", "fade", "image", "text", "light"];
    const readOnly: CueType[] = [
      "audio", "video", "midi_file", "timecode", "camera", "mic", "group", "memo",
      "stop", "devamp", "osc", "midi", "script", "start", "pause", "resume", "load", "reset", "goto", "arm", "disarm",
    ];

    for (const type of editable) expect(canEditCueDuration(type), type).toBe(true);
    for (const type of readOnly) expect(canEditCueDuration(type), type).toBe(false);
  });

  it("uses null for blank Image/Text duration and zero for Wait/Fade/Light", () => {
    expect(emptyCueDurationValue("image")).toBeNull();
    expect(emptyCueDurationValue("text")).toBeNull();
    expect(emptyCueDurationValue("wait")).toBe(0);
    expect(emptyCueDurationValue("fade")).toBe(0);
    expect(emptyCueDurationValue("light")).toBe(0);
    expect(parseInlineTimeInput("  ", null)).toEqual({ valid: true, valueMs: null });
    expect(parseInlineTimeInput("  ", 0)).toEqual({ valid: true, valueMs: 0 });
  });

  it("parses decimal seconds and M:SS with milliseconds", () => {
    expect(parseInlineTimeInput("1.25", 0)).toEqual({ valid: true, valueMs: 1250 });
    expect(parseInlineTimeInput(".5", 0)).toEqual({ valid: true, valueMs: 500 });
    expect(parseInlineTimeInput("1:02.5", 0)).toEqual({ valid: true, valueMs: 62500 });
    expect(parseInlineTimeInput("2:7", 0)).toEqual({ valid: true, valueMs: 127000 });
    expect(formatInlineTimeInput(62500)).toBe("62.5");
    expect(formatInlineTimeInput(null)).toBe("");
  });

  it("round-trips editable millisecond precision, including the largest safe value", () => {
    const values = [0, 1, 9, 999, 1_000, 1_001, 1_250, 1_234_567, Number.MAX_SAFE_INTEGER];
    for (const milliseconds of values) {
      const formatted = formatEditableMilliseconds(milliseconds);
      expect(parseInlineTimeInput(formatted, 0), `${milliseconds} ms → ${formatted}`).toEqual({
        valid: true,
        valueMs: milliseconds,
      });
    }

    expect(formatEditableMilliseconds(0)).toBe("0");
    expect(formatEditableMilliseconds(1_250)).toBe("1.25");
    expect(formatEditableMilliseconds(1_001)).toBe("1.001");
    expect(formatEditableMilliseconds(1_000)).toBe("1");
    expect(formatEditableMilliseconds(null)).toBe("");
  });

  it("rejects negative, malformed, and out-of-range M:SS values", () => {
    for (const input of ["-1", "1:60", "abc", "2 seconds", "1,,5", "1:2:"]) {
      expect(parseInlineTimeInput(input, 0), input).toEqual({ valid: false });
    }
  });

  it("does not commit an unchanged numeric value, including equivalent formatting", () => {
    expect(resolveInlineTimeCommit("1.25", 0, 1_250)).toEqual({
      valid: true,
      valueMs: 1_250,
      shouldCommit: false,
    });
    expect(resolveInlineTimeCommit("", null, null)).toEqual({
      valid: true,
      valueMs: null,
      shouldCommit: false,
    });
    expect(resolveInlineTimeCommit("", 0, 0)).toEqual({
      valid: true,
      valueMs: 0,
      shouldCommit: false,
    });
  });

  it("commits semantic empty values and changed numbers", () => {
    expect(resolveInlineTimeCommit("", null, 1_250)).toEqual({
      valid: true,
      valueMs: null,
      shouldCommit: true,
    });
    expect(resolveInlineTimeCommit("", 0, 1_250)).toEqual({
      valid: true,
      valueMs: 0,
      shouldCommit: true,
    });
    expect(resolveInlineTimeCommit("2", 0, 1_250)).toEqual({
      valid: true,
      valueMs: 2_000,
      shouldCommit: true,
    });
    expect(resolveInlineTimeCommit("oops", 0, 1_250)).toEqual({ valid: false });
  });

  it("uses Enter/Space as native cell activation and stops bubbling", () => {
    expect(isInlineTimeActivationKey("Enter")).toBe(true);
    expect(isInlineTimeActivationKey(" ")).toBe(true);
    expect(isInlineTimeActivationKey("Escape")).toBe(false);
    const stopPropagation = vi.fn();
    stopInlineTimeCellEvent({ stopPropagation });
    expect(stopPropagation).toHaveBeenCalledOnce();
  });
});
