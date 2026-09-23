import { describe, expect, it, vi } from "vitest";
import {
  resolveMediaPreviewKind,
  hasBlockingPreviewOverlay,
  measurePreviewFrameRate,
  shouldPlayMediaPreview,
  shouldStepPreviewFrame,
  toMediaAssetUrl,
} from "./mediaPreviewModel";

describe("media preview model", () => {
  it("selects a moving preview only for video cues", () => {
    expect(resolveMediaPreviewKind(true)).toBe("video");
    expect(resolveMediaPreviewKind(false)).toBe("image");
  });

  it("delegates local paths to the Tauri asset URL converter unchanged", () => {
    const converter = vi.fn((path: string) => `asset://${encodeURIComponent(path)}`);
    const path = "C:\\Show Media\\сцена 1\\clip #2.MOV";

    expect(toMediaAssetUrl(path, converter)).toBe(
      `asset://${encodeURIComponent(path)}`,
    );
    expect(converter).toHaveBeenCalledOnce();
    expect(converter).toHaveBeenCalledWith(path);
  });

  it("re-converts a changed source instead of reusing the previous URL", () => {
    const converter = vi.fn((path: string) => `asset://${path}`);

    expect(toMediaAssetUrl("D:/show/first.mov", converter)).toBe(
      "asset://D:/show/first.mov",
    );
    expect(toMediaAssetUrl("D:/show/second.mp4", converter)).toBe(
      "asset://D:/show/second.mp4",
    );
    expect(converter.mock.calls).toEqual([
      ["D:/show/first.mov"],
      ["D:/show/second.mp4"],
    ]);
  });

  it("falls back when asset URL conversion throws", () => {
    const error = new Error("asset protocol unavailable");
    const converter = vi.fn(() => {
      throw error;
    });

    expect(toMediaAssetUrl("C:/show/broken.mov", converter)).toBeNull();
  });

  it("falls back when the asset URL converter returns an empty URL", () => {
    expect(toMediaAssetUrl("C:/show/broken.mov", () => "  ")).toBeNull();
  });

  it.each([
    [{ documentVisible: true, intersecting: true, failed: false }, true],
    [{ documentVisible: false, intersecting: true, failed: false }, false],
    [{ documentVisible: true, intersecting: false, failed: false }, false],
    [{ documentVisible: true, intersecting: true, failed: true }, false],
    [{ documentVisible: false, intersecting: false, failed: true }, false],
  ])("gates playback for visibility, viewport and decode errors", (state, expected) => {
    expect(shouldPlayMediaPreview(state)).toBe(expected);
  });

  it("measures source frame rate from adjacent presented video frames", () => {
    expect(measurePreviewFrameRate(
      { mediaTime: 10, presentedFrames: 300 },
      { mediaTime: 10 + 1001 / 24000, presentedFrames: 301 },
    )).toBeCloseTo(23.976, 2);
    expect(measurePreviewFrameRate(
      { mediaTime: 2, presentedFrames: 60 },
      { mediaTime: 2, presentedFrames: 61 },
    )).toBeNull();
    expect(measurePreviewFrameRate(
      { mediaTime: 2, presentedFrames: 60 },
      { mediaTime: 2.001, presentedFrames: 61 },
    )).toBeNull();
  });

  it("only frame-steps on unmodified arrows outside text entry and modal UI", () => {
    const outside = { tagName: "DIV", closest: () => null };
    expect(shouldStepPreviewFrame("ArrowLeft", outside, {})).toBe(true);
    expect(shouldStepPreviewFrame("ArrowRight", { tagName: "INPUT" }, {})).toBe(false);
    expect(shouldStepPreviewFrame("ArrowLeft", { tagName: "DIV", isContentEditable: true }, {})).toBe(false);
    expect(shouldStepPreviewFrame("ArrowRight", {
      tagName: "BUTTON",
      closest: (selector) => selector.includes('[role="dialog"]') ? {} : null,
    }, {})).toBe(false);
    expect(shouldStepPreviewFrame("ArrowRight", outside, { ctrlKey: true })).toBe(false);
    expect(shouldStepPreviewFrame("ArrowLeft", outside, { shiftKey: true })).toBe(false);
    expect(shouldStepPreviewFrame("ArrowUp", outside, {})).toBe(false);
  });

  it("blocks frame stepping whenever the document contains a modal or full-screen overlay", () => {
    const querySelector = vi.fn((selector: string) => selector.includes("[role=\"dialog\"]") ? {} : null);
    expect(hasBlockingPreviewOverlay(querySelector)).toBe(true);
    expect(querySelector).toHaveBeenCalledOnce();
    expect(hasBlockingPreviewOverlay(() => null)).toBe(false);
  });
});
