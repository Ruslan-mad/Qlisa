import type { MediaConversionJob } from "../../lib/types";

type EventPayload = { payload: unknown };
type ListenForEvents = (handler: (event: EventPayload) => void) => Promise<() => void>;
type EventKind = "worker" | "applied-state" | "state";
type MediaConversionEvent = MediaConversionJob & { kind?: EventKind };

/** Apply a live event to the store, and auto-replace only on worker completion. */
export function handleMediaConversionJobUpdate(
  event: MediaConversionEvent,
  upsert: (job: MediaConversionJob) => void,
  replaceCueMedia: (jobId: string) => Promise<void>,
  handledCompletionIds: Set<string>,
  onError: (error: unknown) => void = (error) => console.warn("automatic media replacement failed", error),
): void {
  const { kind: eventKind, ...job } = event;
  upsert(job);

  if (eventKind !== "worker"
    || job.status !== "completed"
    || !job.cue_id
    || job.applied_to_cue
    || handledCompletionIds.has(job.id)) return;

  // Keep this guard for the lifetime of the subscriber. A delayed status poll
  // may move the store backwards, but it cannot make the worker event replay.
  handledCompletionIds.add(job.id);
  void replaceCueMedia(job.id).catch(onError);
}

export function subscribeToMediaConversionEvents(
  listenForEvents: ListenForEvents,
  onJob: (event: MediaConversionEvent) => void,
  onError: (error: unknown) => void = (error) => console.warn("media conversion event subscription failed", error),
): () => void {
  const subscription = listenForEvents((event) => onJob(event.payload as MediaConversionEvent))
    .catch((error) => {
      onError(error);
      return () => undefined;
    });

  return () => { void subscription.then((unlisten) => unlisten()).catch(onError); };
}
