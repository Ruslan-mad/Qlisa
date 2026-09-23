import { afterEach, describe, expect, it, vi } from "vitest";
import {
  DEFAULT_COLUMN_CONFIG,
  buildGridCols,
  getVisibleDefs,
  loadColumnConfig,
} from "./columns";

const LEGACY_ORDER = [
  "playhead", "led", "number", "name", "notes", "target", "output",
  "type", "pre_wait", "duration", "post_wait", "continue",
];

function useStoredConfig(value: unknown) {
  vi.stubGlobal("localStorage", {
    getItem: vi.fn(() => JSON.stringify(value)),
    setItem: vi.fn(),
  });
}

afterEach(() => vi.unstubAllGlobals());

describe("cue-list column configuration", () => {
  it("uses the current operator layout as the built-in default", () => {
    const visible = getVisibleDefs(DEFAULT_COLUMN_CONFIG);
    expect(visible.map((column) => column.id)).toEqual([
      "playhead", "led", "type", "number", "name", "notes", "pre_wait",
      "duration", "post_wait", "continue", "outputs", "file_size", "resolution", "target",
    ]);
    expect(buildGridCols(visible, DEFAULT_COLUMN_CONFIG)).toBe(
      "28px 20px 28px 36px 97px 118px 64px 59px 49px 40px 82px 82px 100px 180px",
    );
    expect(DEFAULT_COLUMN_CONFIG.hidden).toEqual({ file: true, output: true });
  });

  it("normalizes the current saved legacy layout to the same default without rewriting it", () => {
    const setItem = vi.fn();
    vi.stubGlobal("localStorage", {
      getItem: vi.fn(() => JSON.stringify({
        widths: {
          type: 28, target: 126, notes: 118, number: 36, output: 102,
          continue: 40, post_wait: 49, duration: 59, name: 97,
        },
        hidden: { number: false, target: true, output: true },
        order: [
          "playhead", "led", "type", "number", "name", "target", "notes",
          "pre_wait", "duration", "post_wait", "continue", "output",
          "file_size", "resolution",
        ],
      })),
      setItem,
    });

    const migrated = loadColumnConfig();
    expect(migrated.order).toEqual(DEFAULT_COLUMN_CONFIG.order);
    expect(migrated.widths).toMatchObject({ file: 126, type: 28, number: 36, name: 97, notes: 118 });
    expect(migrated.widths.target).toBeUndefined();
    expect(migrated.hidden).toMatchObject({ file: true, output: true });
    expect(getVisibleDefs(migrated).map((column) => column.id)).toEqual(
      getVisibleDefs(DEFAULT_COLUMN_CONFIG).map((column) => column.id),
    );
    expect(setItem).not.toHaveBeenCalled();
  });

  it("migrates the legacy Target file column without resetting its layout", () => {
    const customLegacyOrder = [
      "playhead", "led", "name", "target", "number", "notes", "output",
      "type", "pre_wait", "duration", "post_wait", "continue",
    ];
    useStoredConfig({
      order: customLegacyOrder,
      widths: { target: 333 },
      hidden: { target: true, notes: true },
    });

    const config = loadColumnConfig();
    expect(config.order.slice(0, 7)).toEqual([
      "playhead", "led", "name", "file", "file_size", "resolution", "target",
    ]);
    const nonNewColumns = config.order.filter((id) =>
      id !== "file_size" && id !== "resolution",
    );
    expect(nonNewColumns).toEqual([
      "playhead", "led", "name", "file", "target", "number", "notes",
      "output", "outputs", "type", "pre_wait", "duration", "post_wait", "continue",
    ]);
    expect(config.widths.file).toBe(333);
    expect(config.widths.target).toBeUndefined();
    expect(config.hidden.notes).toBe(true);
    expect(config.hidden.file).toBe(true);
    expect(config.hidden.target).toBeUndefined();
    expect(config.hidden.file_size).not.toBe(true);
    expect(config.hidden.resolution).not.toBe(true);
  });

  it("keeps separate File and Target layout settings after migration", () => {
    useStoredConfig({
      order: [...LEGACY_ORDER.map((id) => id === "target" ? "file" : id), "target", "file_size", "resolution"],
      widths: { file: 310, target: 145 },
      hidden: { file: true },
    });

    const config = loadColumnConfig();
    expect(config.widths).toMatchObject({ file: 310, target: 145 });
    expect(config.hidden.file).toBe(true);
    expect(config.hidden.target).toBeUndefined();
    expect(config.order.slice(-2)).toEqual(["file_size", "resolution"]);
  });

  it("keeps explicit widths for the new columns", () => {
    useStoredConfig({
      order: [...LEGACY_ORDER, "file_size", "resolution"],
      widths: { file_size: 123, resolution: 145 },
      hidden: {},
    });

    const config = loadColumnConfig();
    const mediaDefs = getVisibleDefs(config)
      .filter((column) => column.id === "file_size" || column.id === "resolution");
    expect(buildGridCols(mediaDefs, config)).toBe("123px 145px");
  });
});
