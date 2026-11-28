import { describe, expect, it } from "vitest";
import { drawVideoFilmstripWindows, filmstripFrameAtSourceTime, sourceMsToPassTimelineMs, videoFilmstripRequest, visibleFilmstripSourceWindows, visiblePassSourceWindow } from "./videoFilmstripModel";

describe("videoFilmstripModel", () => {
  it("requests denser, tighter frames as the visible interval gets smaller", () => {
    const wide = videoFilmstripRequest(2_110, 4_110, 20_000, 960)!;
    const close = videoFilmstripRequest(2_111, 2_211, 20_000, 960)!;

    expect(wide.tiles).toBe(14);
    expect(close.tiles).toBe(wide.tiles);
    expect(wide.startMs).toBeLessThanOrEqual(2_110);
    expect(wide.endMs).toBeGreaterThanOrEqual(4_110);
    expect(close.endMs - close.startMs).toBeLessThan(wide.endMs - wide.startMs);
  });

  it("clamps requests at file boundaries and caps decode work", () => {
    expect(videoFilmstripRequest(-300, 80, 1_000, 10_000)).toEqual({
      startMs: 0,
      endMs: 80,
      tiles: 24,
    });
    expect(videoFilmstripRequest(990, 1_100, 1_000, 960)).toEqual({
      startMs: 980,
      endMs: 1_000,
      tiles: 14,
    });
    expect(videoFilmstripRequest(1, 1, 1_000, 960)).toBeNull();
  });

  it("maps a visible portion of a rate-adjusted pass back to source time", () => {
    const window = visiblePassSourceWindow(1_500, 2_500, 1_000, 3_000, 5_000, 2)!;
    expect(window).toEqual({ startMs: 6_000, endMs: 8_000 });
    expect(sourceMsToPassTimelineMs(7_000, 1_000, 3_000, 5_000, 9_000)).toBe(2_000);
  });

  it("selects the frame from the most detailed window that contains the source time", () => {
    const base = { sourceStartMs: 0, sourceEndMs: 10_000, tiles: [{ id: "base" }, { id: "base-2" }] };
    const detail = { sourceStartMs: 2_000, sourceEndMs: 3_000, tiles: [{ id: "detail" }, { id: "detail-2" }] };
    expect(filmstripFrameAtSourceTime([base, detail] as any, 2_100)).toEqual({ id: "detail" });
    expect(filmstripFrameAtSourceTime([detail, base] as any, 2_100)).toEqual({ id: "detail" });
    expect(filmstripFrameAtSourceTime([base, detail] as any, 8_000)).toEqual({ id: "base-2" });
  });

  it("plans one or two visible loop windows and falls back to the source for many passes", () => {
    const loop = { id: "v", startMs: 0, endMs: 6_000, sourceStartMs: 2_000, sourceEndMs: 4_000, rate: 1, looped: true };
    expect(visibleFilmstripSourceWindows(loop, 1_500, 2_500)).toEqual([
      { sourceStartMs: 3_500, sourceEndMs: 4_000 },
      { sourceStartMs: 2_000, sourceEndMs: 2_500 },
    ]);
    expect(visibleFilmstripSourceWindows(loop, 0, 6_000)).toEqual([
      { sourceStartMs: 2_000, sourceEndMs: 4_000 },
    ]);
  });

  it("keeps sparse fallback frames at a stable width and overlays detail only on its source interval", () => {
    const drawn: Array<{ image: unknown; x: number; width: number }> = [];
    const ctx = {
      save() {}, beginPath() {}, rect() {}, clip() {}, restore() {}, fillRect() {},
      drawImage(image: unknown, x: number, _y: number, width: number) { drawn.push({ image, x, width }); },
      set fillStyle(_value: string) {},
    } as unknown as CanvasRenderingContext2D;
    const base = { width: 160, height: 90 } as HTMLImageElement;
    const detail = { width: 160, height: 90 } as HTMLImageElement;
    drawVideoFilmstripWindows(ctx, [
      { sourceStartMs: 0, sourceEndMs: 10_000, tiles: [base] },
      { sourceStartMs: 1_000, sourceEndMs: 2_000, tiles: [detail] },
    ], {
      timelineStartMs: 0, timelineEndMs: 10_000, sourceStartMs: 0, sourceEndMs: 10_000,
      viewStartMs: 0, viewEndMs: 10_000, width: 1_000, top: 0, height: 50,
    });

    expect(drawn.length).toBeGreaterThan(2);
    expect(drawn.every((call) => call.width <= 160)).toBe(true);
    expect(drawn.at(-1)?.image).toBe(detail);
    expect(drawn.at(-1)?.x).toBeGreaterThanOrEqual(100);
    expect(drawn.at(-1)?.x).toBeLessThan(200);
  });
});
