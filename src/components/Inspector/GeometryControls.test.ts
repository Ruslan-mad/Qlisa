import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { DEFAULT_GEOMETRY } from "../../lib/types";
import { GeometryControlGrid, GeometryEditor, snapRotationToCardinal, type GeometryEditorState } from "./GeometryTab";
import { advanceGeometryCenterDetent, geometryEffectivePointerX, geometrySliderPositionFromPointer, geometryValueAtDetentPosition, idleGeometryCenterDetent, releaseGeometryCenterDetent } from "./geometrySnapModel";

describe("GeometryControlGrid", () => {
  it("renders scale, rotation and X/Y in a compact two-column grid", () => {
    const markup = renderToStaticMarkup(createElement(GeometryControlGrid, {
      geometry: DEFAULT_GEOMETRY,
      contextKey: "cue:main:linked",
      disabled: false,
      overlay: false,
      gestureHandleRef: { current: null },
      onPreview: () => undefined,
      onCommit: () => undefined,
    }));

    expect(markup.match(/type="range"/g)).toHaveLength(4);
    expect(markup).toContain("Масштаб");
    expect(markup).toContain("Поворот");
    expect(markup).toContain('aria-label="X (%)"');
    expect(markup).toContain('aria-label="Y (%)"');
    expect(markup).toContain("grid-template-columns:minmax(0,1fr) minmax(0,1fr)");
    expect(markup).toContain('max="360"');
    expect(markup).not.toContain('width:6px');
  });

  it("keeps the 2x2 control order and disables unsupported rotation for Text and Timer", () => {
    const markup = renderToStaticMarkup(createElement(GeometryControlGrid, {
      geometry: DEFAULT_GEOMETRY,
      contextKey: "timer:main:linked",
      disabled: false,
      overlay: true,
      gestureHandleRef: { current: null },
      onPreview: () => undefined,
      onCommit: () => undefined,
    }));
    expect(markup.match(/type="range"/g)).toHaveLength(4);
    expect(markup).toContain("Недоступно для текста и таймера");
    expect(markup.match(/disabled=""/g)).toHaveLength(1);
  });

  it("shows signed percentage pan and explains output-relative units", () => {
    const markup = renderToStaticMarkup(createElement(GeometryControlGrid, {
      geometry: { ...DEFAULT_GEOMETRY, pan_x: 0.205, pan_y: -0.1 },
      contextKey: "cue:screen:output",
      disabled: false,
      overlay: false,
      panMode: "output",
      gestureHandleRef: { current: null },
      onPreview: () => undefined,
      onCommit: () => undefined,
    }));
    expect(markup).toContain('title="Смещение относительно размера экрана; шаг 1%."');
    expect(markup).toContain("+20.5%");
    expect(markup).toContain("-10%");
  });

  it("renders the legacy GeometryEditor rotation range through 360 degrees", () => {
    const geometry = Object.fromEntries(Object.keys(DEFAULT_GEOMETRY).map((key) => [
      key,
      { kind: "uniform", value: DEFAULT_GEOMETRY[key as keyof typeof DEFAULT_GEOMETRY] },
    ])) as GeometryEditorState;
    const markup = renderToStaticMarkup(createElement(GeometryEditor, {
      geometry,
      disabled: false,
      onPatch: () => undefined,
      onReset: () => undefined,
    }));

    expect(markup).toContain('max="360"');
    expect(markup).toContain("0 – 100");
    expect(markup).toContain("Слева");
  });
});

describe("geometry pointer magnets", () => {
  const move = (state: ReturnType<typeof idleGeometryCenterDetent>, from: number, to: number) =>
    advanceGeometryCenterDetent(state, from, to, 100, 100);

  it("holds pan zero and scale one through jitter, then releases smoothly on either side", () => {
    for (const direction of [-1, 1]) {
      let state = move(idleGeometryCenterDetent(), 100, 100 + direction * 7);
      expect(state.latched).toBe(true);
      expect(geometryValueAtDetentPosition(0.53, -1, 1, 0, state)).toBe(0);
      state = move(state, 100 + direction * 7, 100 - direction * 4);
      state = move(state, 100 - direction * 4, 100 + direction * 3);
      expect(state.latched).toBe(true);
      state = move(state, 100 + direction * 3, 100 + direction * 15);
      expect(state.latched).toBe(false);
      const anchor = state.releasePosition!;
      expect(geometryValueAtDetentPosition(anchor, -1, 1, 0, state)).toBe(0);
      expect(geometryValueAtDetentPosition(anchor, 0.05, 8, 1, state)).toBe(1);
      const endpoint = direction > 0 ? 1 : 0;
      expect(geometryValueAtDetentPosition(endpoint, -1, 1, 0, state)).toBe(direction > 0 ? 1 : -1);
      expect(geometryValueAtDetentPosition(endpoint, 0.05, 8, 1, state)).toBe(direction > 0 ? 8 : 0.05);
    }
  });

  it("releases a fast center-crossing jump at pointerup instead of trapping at neutral", () => {
    for (const [start, end, expected] of [[0, 200, 1], [200, 0, -1]] as const) {
      const crossed = move(idleGeometryCenterDetent(), start, end);
      expect(crossed).toMatchObject({ latched: true, holdNextMove: true });
      const released = releaseGeometryCenterDetent(crossed, end, 100, 100);
      expect(geometryValueAtDetentPosition(end === 200 ? 1 : 0, -1, 1, 0, released)).toBe(expected);
      expect(geometryValueAtDetentPosition(end === 200 ? 1 : 0, 0.05, 8, 1, released)).toBe(end === 200 ? 8 : 0.05);
    }
  });

  it("starts the next gesture with no stale detent", () => {
    const first = move(idleGeometryCenterDetent(), 100, 107);
    expect(first.latched).toBe(true);
    expect(move(idleGeometryCenterDetent(), 100, 93).latched).toBe(true);
  });

  it("uses one thumb-grab offset through center crossing and release", () => {
    const left = 0;
    const width = 116;
    const trackWidth = 100;
    const center = 58;
    for (const grabOffset of [-6, 6]) {
      const raw = [30, 52, 63, 76].map((x) => x + grabOffset);
      let state = idleGeometryCenterDetent();
      for (let index = 1; index < raw.length - 1; index += 1) {
        state = advanceGeometryCenterDetent(
          state,
          geometryEffectivePointerX(raw[index - 1], grabOffset),
          geometryEffectivePointerX(raw[index], grabOffset),
          center,
          trackWidth,
        );
      }
      expect(state.latched).toBe(true);
      state = advanceGeometryCenterDetent(state, geometryEffectivePointerX(raw[2], grabOffset), geometryEffectivePointerX(raw[3], grabOffset), center, trackWidth);
      state = releaseGeometryCenterDetent(state, geometryEffectivePointerX(raw[3], grabOffset), center, trackWidth);
      const position = geometrySliderPositionFromPointer(geometryEffectivePointerX(raw[3], grabOffset), left, width);
      expect(position).toBeCloseTo(0.68);
      expect(geometryValueAtDetentPosition(position, -1, 1, 0, state)).toBeGreaterThan(0);
    }
  });

  it("snaps pointer rotation to all cardinals and preserves authored 360", () => {
    for (const angle of [0, 90, 180, 270, 360]) expect(snapRotationToCardinal(angle)).toBe(angle);
    expect(snapRotationToCardinal(3)).toBe(0);
    expect(snapRotationToCardinal(89)).toBe(90);
    expect(snapRotationToCardinal(178)).toBe(180);
    expect(snapRotationToCardinal(274)).toBe(270);
    expect(snapRotationToCardinal(357)).toBe(360);
    expect(snapRotationToCardinal(85)).toBe(85);
  });
});
