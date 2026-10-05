import { describe, expect, it, vi } from "vitest";
import { editorPreviewCursorPosition, headphonePreviewStartPosition, mirrorHeadphonePlayback, PreviewSessionController, seekHeadphonePreview, stepPreviewAndSyncHeadphones } from "./headphonePreviewTransport";
import { useVideoPreviewTransport } from "../Inspector/mediaPreviewTransport";
import { useNumberPreviewStore } from "../../stores/numberPreviewStore";

describe("headphone preview cursor", () => {
  it("serializes startup and coalesces rapid seeks to the latest playing cursor", async () => {
    let resolveStart!: () => void;
    let resolveFirstSeek!: () => void;
    const startGate = new Promise<void>((resolve) => { resolveStart = resolve; });
    const seekGate = new Promise<void>((resolve) => { resolveFirstSeek = resolve; });
    const control = vi.fn(async (action: string, position?: number) => {
      if (action === "seek" && position === 10) await seekGate;
      return { voice_id: `voice-${position ?? action}`, generation: position ?? 1 };
    });
    const controller = new PreviewSessionController({
      start: vi.fn(() => startGate), control,
      onSession: vi.fn(), onError: vi.fn(),
    });
    controller.setIntent(true, true, 0);
    for (let position = 1; position <= 10; position++) controller.seek(position);
    expect(control).not.toHaveBeenCalled();
    resolveStart();
    await vi.waitFor(() => expect(control).toHaveBeenCalledWith("seek", 10, true));
    for (let position = 11; position <= 20; position++) controller.seek(position);
    resolveFirstSeek();
    await vi.waitFor(() => expect(control).toHaveBeenCalledWith("seek", 20, true));
    await vi.waitFor(() => expect(control).toHaveBeenCalledWith("resume", undefined, true));
    expect(control.mock.calls.filter(([action]) => action === "seek").map(([, position]) => position)).toEqual([10, 20]);
  });

  it("keeps the newest pause, mute, and cursor intent while commands are pending", async () => {
    const deferred = <T,>() => {
      let resolve!: (value: T) => void;
      const promise = new Promise<T>((done) => { resolve = done; });
      return { promise, resolve };
    };
    const muteGate = deferred<unknown>();
    const control = vi.fn(async (action: string, position?: number) => {
      if (action === "mute") await muteGate.promise;
      return { voice_id: `${action}-${position ?? 0}`, generation: position ?? 2 };
    });
    const controller = new PreviewSessionController({ start: async () => ({}), control, onSession: vi.fn(), onError: vi.fn() });
    controller.setIntent(true, true, 5);
    await vi.waitFor(() => expect(control).toHaveBeenCalledWith("resume", undefined, true));
    controller.setIntent(true, false, 5000);
    await vi.waitFor(() => expect(control).toHaveBeenCalledWith("mute", undefined, false));
    controller.setIntent(false, true, 5);
    controller.seek(500);
    muteGate.resolve({});
    await vi.waitFor(() => expect(control).toHaveBeenCalledWith("seek_paused", 500, true));
    await vi.waitFor(() => expect(control).toHaveBeenCalledWith("unmute", undefined, true));
    await vi.waitFor(() => expect(control).toHaveBeenCalledWith("pause", undefined, true));
    expect(control.mock.calls.filter(([action]) => action === "seek_paused")).toHaveLength(1);
    expect(control.mock.calls.some(([action, position]) => action === "seek" && position === 5000)).toBe(false);
  });

  it("ignores a control response after the session was ended", async () => {
    let finishControl!: (value: unknown) => void;
    const pendingControl = new Promise<unknown>((resolve) => { finishControl = resolve; });
    const onSession = vi.fn();
    const controller = new PreviewSessionController({
      start: async () => ({ voice_id: "voice-1", generation: 1 }),
      control: vi.fn(() => pendingControl), onSession, onError: vi.fn(),
    });
    controller.setIntent(true, true, 0);
    await vi.waitFor(() => expect(onSession).toHaveBeenCalledWith(true, { voice_id: "voice-1", generation: 1 }));
    controller.end();
    await vi.waitFor(() => expect(onSession).toHaveBeenCalledWith(false));
    const countAfterEnd = onSession.mock.calls.length;
    finishControl({ voice_id: "voice-old", generation: 2 });
    await Promise.resolve();
    await Promise.resolve();
    expect(onSession).toHaveBeenCalledTimes(countAfterEnd);
  });

  it("ignores a late control rejection after session teardown", async () => {
    let rejectControl!: (error: Error) => void;
    const pendingControl = new Promise<unknown>((_resolve, reject) => { rejectControl = reject; });
    const onSession = vi.fn();
    const onError = vi.fn();
    const controller = new PreviewSessionController({
      start: async () => ({ voice_id: "voice-1", generation: 1 }),
      control: vi.fn(() => pendingControl), onSession, onError,
    });
    controller.setIntent(true, true, 0);
    await vi.waitFor(() => expect(onSession).toHaveBeenCalledWith(true, { voice_id: "voice-1", generation: 1 }));
    controller.setIntent(false, true, 0);
    controller.end();
    const countAfterEnd = onSession.mock.calls.length;
    rejectControl(new Error("late command failure"));
    await Promise.resolve();
    await Promise.resolve();
    expect(onSession).toHaveBeenCalledTimes(countAfterEnd);
    expect(onError).not.toHaveBeenCalled();
  });

  it("does not retry a failed start until a new user intent arrives", async () => {
    const start = vi.fn().mockRejectedValue(new Error("no output"));
    const onError = vi.fn();
    const controller = new PreviewSessionController({ start, control: vi.fn(), onSession: vi.fn(), onError });
    controller.setIntent(true, true, 0);
    await vi.waitFor(() => expect(onError).toHaveBeenCalledOnce());
    await Promise.resolve();
    expect(start).toHaveBeenCalledOnce();
    controller.setIntent(false, true, 0);
    controller.setIntent(true, true, 0);
    await vi.waitFor(() => expect(start).toHaveBeenCalledTimes(2));
  });

  it("starts a fresh session at the selected cursor after the previous session ends", async () => {
    const start = vi.fn(async (positionMs: number) => ({ voice_id: `voice-${positionMs}`, generation: positionMs }));
    const controller = new PreviewSessionController({ start, control: vi.fn(async () => ({})), onSession: vi.fn(), onError: vi.fn() });
    controller.setIntent(true, true, 250);
    await vi.waitFor(() => expect(start).toHaveBeenCalledOnce());
    controller.end();
    controller.setIntent(true, true, 1250);
    await vi.waitFor(() => expect(start).toHaveBeenCalledTimes(2));
    expect(start).toHaveBeenLastCalledWith(1250, true);
  });

  it("survives StrictMode activate-dispose-activate and cancels the old start", async () => {
    const deferred = <T,>() => {
      let resolve!: (value: T) => void;
      const promise = new Promise<T>((done) => { resolve = done; });
      return { promise, resolve };
    };
    const first = deferred<unknown>();
    const second = deferred<unknown>();
    const start = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const onSession = vi.fn();
    const cancelStart = vi.fn();
    const controller = new PreviewSessionController({ start, control: vi.fn(async () => ({})), onSession, onError: vi.fn(), cancelStart });
    controller.activate();
    controller.setIntent(true, true, 100);
    controller.dispose();
    controller.activate();
    controller.setIntent(true, true, 200);
    first.resolve({ voice_id: "old-voice", generation: 1 });
    await vi.waitFor(() => expect(start).toHaveBeenCalledTimes(2));
    expect(cancelStart).toHaveBeenCalledWith({ voice_id: "old-voice", generation: 1 });
    expect(onSession).not.toHaveBeenCalledWith(true, { voice_id: "old-voice", generation: 1 });
    second.resolve({ voice_id: "new-voice", generation: 2 });
    await vi.waitFor(() => expect(onSession).toHaveBeenCalledWith(true, { voice_id: "new-voice", generation: 2 }));
  });

  it("starts ordinary media from its editor preview cursor before runtime timing", () => {
    expect(headphonePreviewStartPosition(false, 0, 4200, 9100, 1200)).toBe(4200);
  });

  it("starts a Number mix from the Number clock cursor", () => {
    expect(headphonePreviewStartPosition(true, 2500, null, 8000, 0)).toBe(2500);
  });

  it("starts Video from its current identity-matched visual cursor", () => {
    const videoCursor = editorPreviewCursorPosition(true, 5200, 2100);
    expect(headphonePreviewStartPosition(false, 0, videoCursor, 9000, 1200)).toBe(5200);
    expect(editorPreviewCursorPosition(true, null, 2100)).toBe(2100);
  });

  it("uses live timing, then the effective trim start when no editor cursor exists", () => {
    expect(headphonePreviewStartPosition(false, 0, null, 3100, 1200)).toBe(3100);
    expect(headphonePreviewStartPosition(false, 0, null, null, 1200)).toBe(1200);
  });

  it("sends visual play and pause edges to an active headphone session", () => {
    const send = vi.fn();
    expect(mirrorHeadphonePlayback(false, true, send)).toBe(true);
    expect(mirrorHeadphonePlayback(true, true, send)).toBe(true);
    expect(send.mock.calls).toEqual([["resume"], ["pause"]]);
  });

  it("keeps inactive visual playback local and sends only explicit seek edges", () => {
    const send = vi.fn();
    expect(mirrorHeadphonePlayback(false, false, send)).toBe(false);
    expect(seekHeadphonePreview(false, 1200, send)).toBe(false);
    expect(seekHeadphonePreview(true, 1200.4, send)).toBe(true);
    expect(send.mock.calls).toEqual([[1200, false]]);
  });

  it("wires a video frame-step state change to one paused headphone seek", () => {
    const store = useVideoPreviewTransport.getState();
    store.activate("video-a", 0, 4000);
    store.setReady("video-a", true);
    store.setDocumentVisible("video-a", true);
    store.toggle("video-a");
    const sync = vi.fn();
    const position = stepPreviewAndSyncHeadphones(() => {
      useVideoPreviewTransport.getState().stepFrame("video-a", 1, 25);
      return useVideoPreviewTransport.getState().positionMs;
    }, sync);
    expect(useVideoPreviewTransport.getState().playing).toBe(false);
    expect(sync).toHaveBeenCalledWith(position, true);
    useVideoPreviewTransport.getState().deactivate("video-a");
  });

  it("wires a Number frame-step state change to one paused headphone seek", () => {
    const store = useNumberPreviewStore.getState();
    store.select("number-a", 4000);
    store.setPlaying("number-a", true);
    const sync = vi.fn();
    const position = stepPreviewAndSyncHeadphones(() => {
      useNumberPreviewStore.getState().stepFrame("number-a", -1, 25, 4000);
      return useNumberPreviewStore.getState().positionMs;
    }, sync);
    expect(useNumberPreviewStore.getState().playing).toBe(false);
    expect(sync).toHaveBeenCalledWith(position, true);
    useNumberPreviewStore.getState().clear("number-a");
  });

  it("marks a frame-step seek as pause-before-seek", () => {
    const send = vi.fn();
    seekHeadphonePreview(true, 2444, send, true);
    expect(send).toHaveBeenCalledWith(2444, true);
  });
});
