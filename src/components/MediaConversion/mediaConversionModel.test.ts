import { describe, expect, it } from "vitest";
import { conversionProgress, formatMediaDuration, isConvertibleMediaCue } from "./mediaConversionModel";

const cue = (cue_type: "audio" | "video" | "image" | "group", file_path: string | null) => ({ cue_type, file_path } as never);

describe("media conversion model", () => {
  it("only enables conversion for file-backed audio/video cues", () => {
    expect(isConvertibleMediaCue(cue("audio", "a.wav"))).toBe(true);
    expect(isConvertibleMediaCue(cue("video", "v.mov"))).toBe(true);
    expect(isConvertibleMediaCue(cue("image", "poster.png"))).toBe(true);
    expect(isConvertibleMediaCue(cue("group", "g"))).toBe(false);
    expect(isConvertibleMediaCue(cue("audio", null))).toBe(false);
  });

  it("clamps backend progress for the progress bar", () => {
    expect(conversionProgress({ progress: -1 })).toBe(0);
    expect(conversionProgress({ progress: 0.4 })).toBe(0.4);
    expect(conversionProgress({ progress: 2 })).toBe(1);
  });

  it("formats metadata duration", () => {
    expect(formatMediaDuration(125000)).toBe("2:05");
    expect(formatMediaDuration(null)).toBe("—");
  });
});
