import { describe, expect, it, vi } from "vitest";
import { editorPreviewCursorPosition, headphonePreviewStartPosition, mirrorHeadphonePlayback, seekHeadphonePreview, stepPreviewAndSyncHeadphones } from "./headphonePreviewTransport";
import { useVideoPreviewTransport } from "../Inspector/mediaPreviewTransport";
import { useNumberPreviewStore } from "../../stores/numberPreviewStore";

describe("headphone preview cursor", () => {
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
    store.setVisible("video-a", true);
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
