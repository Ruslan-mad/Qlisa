import { describe, expect, it } from "vitest";
import { conversionProgress, findCompletedMediaOutputJob, formatMediaDuration, isConvertibleMediaCue, mediaConversionBatchState, resolveMediaConversionEnqueueResults } from "./mediaConversionModel";

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

  it("tracks mixed successful and rejected enqueue requests", () => {
    const cues = [{ id: "cue-1", name: "Video" }, { id: "cue-2", name: "Image" }];
    const results = [
      { status: "fulfilled", value: { id: "job-1", cue_id: "cue-1", status: "queued" } },
      { status: "rejected", reason: new Error("Unsupported codec") },
    ] as const;
    const resolved = resolveMediaConversionEnqueueResults(cues, results);

    expect(resolved.jobs).toEqual([{ id: "job-1", cue_id: "cue-1", status: "queued" }]);
    expect(resolved.failures).toEqual([{ cue_id: "cue-2", cue_name: "Image", error: "Unsupported codec" }]);
    expect(mediaConversionBatchState(2, [{ status: "queued" }], resolved.failures)).toEqual({ busy: true, finished: false, completed: false });
    expect(mediaConversionBatchState(2, [{ status: "completed" }], resolved.failures)).toEqual({ busy: false, finished: true, completed: false });
  });

  it("finishes when every enqueue request is rejected", () => {
    const cues = [{ id: "cue-1", name: "Video" }, { id: "cue-2", name: "Image" }];
    const resolved = resolveMediaConversionEnqueueResults(cues, [
      { status: "rejected", reason: "File is missing" },
      { status: "rejected", reason: new Error("Permission denied") },
    ]);

    expect(resolved.jobs).toEqual([]);
    expect(resolved.failures.map(({ error }) => error)).toEqual(["File is missing", "Permission denied"]);
    expect(mediaConversionBatchState(2, resolved.jobs, resolved.failures)).toEqual({ busy: false, finished: true, completed: false });
  });

  it("treats failed and cancelled jobs as terminal without completing the batch", () => {
    expect(mediaConversionBatchState(2, [{ status: "failed" }, { status: "cancelled" }], [])).toEqual({ busy: false, finished: true, completed: false });
    expect(mediaConversionBatchState(2, [{ status: "queued" }], [])).toEqual({ busy: true, finished: false, completed: false });
  });

  it("opens a completed output when an earlier failed job also has an output path", () => {
    const jobs = [
      { id: "failed", status: "failed", output_path: "stale.mp4" },
      { id: "ready", status: "completed", output_path: "ready.mp4" },
    ] as const;
    expect(findCompletedMediaOutputJob(jobs)?.id).toBe("ready");
    expect(findCompletedMediaOutputJob([{ id: "pending", status: "running", output_path: "" }])).toBeUndefined();
  });
});
