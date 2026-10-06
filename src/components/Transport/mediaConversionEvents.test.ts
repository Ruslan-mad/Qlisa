import { describe, expect, it, vi } from "vitest";
import type { MediaConversionJob } from "../../lib/types";
import {
  handleMediaConversionJobUpdate,
  subscribeToMediaConversionEvents,
} from "./mediaConversionEvents";

const job = (patch: Partial<MediaConversionJob> = {}): MediaConversionJob => ({
  id: "job-1",
  cue_id: "cue-1",
  input_path: "original.mov",
  output_path: "converted.mp4",
  mode: "size",
  status: "running",
  progress: 1,
  processed: 1,
  total: 1,
  speed: 1,
  source_size: 100,
  new_size: 40,
  error: null,
  error_category: null,
  applied_to_cue: false,
  ...patch,
});

describe("media conversion status events", () => {
  it("auto-applies once on worker completion, despite polls and restore events", async () => {
    const jobs = new Map<string, MediaConversionJob>([["job-1", job()]]);
    let onEvent: ((event: { payload: unknown }) => void) | undefined;
    const listenForEvents = vi.fn(async (handler: (event: { payload: unknown }) => void) => {
      onEvent = handler;
      return vi.fn();
    });
    const upsert = vi.fn((next: MediaConversionJob) => jobs.set(next.id, next));
    const handledCompletionIds = new Set<string>();
    const replaceCueMedia = vi.fn(async (_jobId: string): Promise<void> => undefined);
    const onWorkerEvent = (event: Parameters<typeof handleMediaConversionJobUpdate>[0]) =>
      handleMediaConversionJobUpdate(event, upsert, replaceCueMedia, handledCompletionIds);
    const subscription = subscribeToMediaConversionEvents(listenForEvents, onWorkerEvent);
    await Promise.resolve();

    const completed = job({ status: "completed" });
    // The 500/700 ms status poll can see completion before the Tauri event.
    upsert(completed);
    expect(replaceCueMedia).not.toHaveBeenCalled();

    // Only the explicit worker-completion event triggers auto-replace.
    onEvent?.({ payload: { ...completed, kind: "worker" } });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(replaceCueMedia).toHaveBeenCalledTimes(1);

    // A stale running poll may update display state but cannot consume or
    // replay the one-shot worker event.
    upsert(job({ status: "running", progress: 0.8 }));
    onEvent?.({ payload: { ...completed, applied_to_cue: true, kind: "applied-state" } });
    onEvent?.({ payload: { ...completed, applied_to_cue: false, kind: "applied-state" } });
    onEvent?.({ payload: { ...completed, kind: "worker" } });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(replaceCueMedia).toHaveBeenCalledTimes(1);
    expect(jobs.get("job-1")?.applied_to_cue).toBe(false);

    // The dialog's explicit Replace action remains a direct command call.
    await replaceCueMedia("job-1");
    expect(replaceCueMedia).toHaveBeenCalledTimes(2);
    subscription();
  });

  it("does not auto-apply historical completed jobs or non-worker updates", async () => {
    const completed = job({ status: "completed" });
    let onEvent: ((event: { payload: unknown }) => void) | undefined;
    const listenForEvents = vi.fn(async (handler: (event: { payload: unknown }) => void) => {
      onEvent = handler;
      return vi.fn();
    });
    const replaceCueMedia = vi.fn(async (_jobId: string): Promise<void> => undefined);
    const handledCompletionIds = new Set<string>();
    const onWorkerEvent = (event: Parameters<typeof handleMediaConversionJobUpdate>[0]) =>
      handleMediaConversionJobUpdate(event, vi.fn(), replaceCueMedia, handledCompletionIds);
    const subscription = subscribeToMediaConversionEvents(listenForEvents, onWorkerEvent);
    await Promise.resolve();

    onEvent?.({ payload: completed });
    onEvent?.({ payload: { ...completed, kind: "state" } });
    onEvent?.({ payload: { ...completed, kind: "applied-state" } });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(replaceCueMedia).not.toHaveBeenCalled();
    subscription();
  });
});
