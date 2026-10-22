import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CueSummary } from "../../lib/types";
import { setLocale } from "../../i18n";
import { CueRow } from "./CueRow";
import type { ColumnDef } from "./columns";

const { timingState, workspaceState } = vi.hoisted(() => ({
  timingState: { current: null as unknown },
  workspaceState: { theme: "stage" },
}));
vi.mock("../../stores/timingStore", () => ({
  useTimingStore: (selector: (state: { timings: Record<string, unknown> }) => unknown) =>
    selector({ timings: { "cue-1": timingState.current } }),
}));
vi.mock("../../stores/workspaceStore", () => ({
  useWorkspaceStore: (selector: (state: { playedCueIds: Set<string>; displayPrefs: { theme: string } }) => unknown) =>
    selector({ playedCueIds: new Set(), displayPrefs: { theme: workspaceState.theme } }),
}));

const columns: ColumnDef[] = [
  { id: "playhead", label: "↳", defaultWidth: 28, minWidth: 24, fixed: true, resizable: false },
  { id: "led", label: "", defaultWidth: 20, minWidth: 16, fixed: true, resizable: false },
  { id: "continue", label: "C", defaultWidth: 40, minWidth: 28, fixed: false, resizable: true },
  { id: "pre_wait", label: "Pre-W", defaultWidth: 64, minWidth: 48, fixed: false, resizable: true },
  { id: "post_wait", label: "Post-W", defaultWidth: 49, minWidth: 48, fixed: false, resizable: true },
];

const cue = {
  id: "cue-1", cue_type: "audio", name: "Test", number: "1", notes: "",
  state: "standby", continue_mode: "auto_follow", color: "none",
  pre_wait_ms: 1000, post_wait_ms: 1000, duration_ms: 1000,
  file_path: null, file_size_bytes: null, media_width: null, media_height: null,
  file_duration_ms: null, media_file_missing: false, is_loading: false,
  is_disabled: false, is_broken: false, is_warning: false,
} as unknown as CueSummary;

type RowOptions = {
  theme?: "dark" | "navy" | "stage" | "light" | "system";
  color?: CueSummary["color"];
  cueType?: CueSummary["cue_type"];
  cueColorStyle?: "stripe" | "full_row";
  isGroup?: boolean;
  isSelected?: boolean;
  isDragOver?: boolean;
  isDragSource?: boolean;
  isDisabled?: boolean;
  stickyRight?: boolean;
  atPlayhead?: boolean;
  groupExpanded?: boolean;
};

function renderRow(state: CueSummary["state"] = "standby", options: RowOptions = {}) {
  workspaceState.theme = options.theme ?? "dark";
  return renderToStaticMarkup(createElement(CueRow, {
    cue: { ...cue, cue_type: options.cueType ?? cue.cue_type, state, color: options.color ?? cue.color, is_disabled: options.isDisabled ?? false }, cueIndex: 0, gridStyle: { display: "grid", gridTemplateColumns: "28px 20px 40px 64px 49px" },
    visibleDefs: options.stickyRight ? [...columns.slice(0, 2), { ...columns[2], stickyRight: true }] : columns,
    isSelected: options.isSelected ?? false, isAtPlayhead: options.atPlayhead ?? false,
    isDragOver: options.isDragOver, isDragSource: options.isDragSource,
    isGroup: options.isGroup,
    isGroupExpanded: options.groupExpanded,
    onToggleExpand: () => {},
    cueColorStyle: options.cueColorStyle,
    onCueDragStart: () => {}, onClick: () => {},
    onDoubleClick: () => {}, onContextMenu: () => {}, onContinueContextMenu: () => {},
  }));
}

afterEach(() => {
  timingState.current = null;
  setLocale("en");
  workspaceState.theme = "stage";
});

describe("CueRow transport markers", () => {
  beforeEach(() => setLocale("en"));

  it("shows the next arrow only on the playhead row with an accessible name", () => {
    const next = renderRow("standby", { atPlayhead: true });
    expect(next).toContain('aria-label="Next Cue"');
    expect(renderRow("standby")).not.toContain('aria-label="Next Cue"');
  });

  it("shows the running play marker for running cues and keeps it separate from next", () => {
    const running = renderRow("running");
    const runningNext = renderRow("running", { atPlayhead: true });
    expect(running).toContain('role="img" aria-label="Running"');
    expect(runningNext).toContain('aria-label="Next Cue"');
    expect(runningNext).toContain('role="img" aria-label="Running"');
    expect(runningNext.indexOf('aria-label="Running"')).toBeLessThan(runningNext.indexOf('aria-label="Next Cue"'));
    const runningNumber = renderRow("running", { cueType: "number" });
    expect(runningNumber).toContain('role="img" aria-label="Running"');
    for (const state of ["paused", "standby"] as const) {
      const html = renderRow(state);
      expect(html).not.toContain('role="img" aria-label="Running"');
    }
  });

  it("keeps the next marker in the fixed group expander target", () => {
    const html = renderRow("running", { atPlayhead: true, cueType: "group", isGroup: true, groupExpanded: false });
    expect(html).toContain('aria-label="Next Cue"');
    expect(html).toContain("width:30px");
    expect(html).toContain("height:28px");
  });
});

describe("CueRow transport wait display", () => {
  it("does not expose a mouse-drag range-selection gutter", () => {
    expect(renderRow()).not.toContain("data-selection-gutter");
  });

  it("fills only the active phase column and localizes Auto-Follow in Russian", () => {
    setLocale("ru");
    timingState.current = {
      elapsed_ms: 250, action_elapsed_ms: 0, remaining_ms: 1000,
      media_position_ms: null, wait_phase: "pre_wait", wait_elapsed_ms: 250,
      wait_duration_ms: 1000,
    };
    const html = renderRow("running");
    expect(html).toContain('aria-label="Автопереход"');
    expect(html.match(/width:calc\(\(100% - 6px\) \*/g)).toHaveLength(1);
    expect(html).toContain("width:calc((100% - 6px) * 0.25)");
  });

  it("renders a pending standby Auto-Follow post-wait only in the post-wait cell", () => {
    timingState.current = {
      elapsed_ms: 0, action_elapsed_ms: 0, remaining_ms: 0,
      media_position_ms: null, wait_phase: "post_wait", wait_elapsed_ms: 500,
      wait_duration_ms: 1000,
    };
    const html = renderRow();
    expect(html.match(/width:calc\(\(100% - 6px\) \*/g)).toHaveLength(1);
    expect(html).toContain("width:calc((100% - 6px) * 0.5)");
  });
});

describe("CueRow Stage authored colours", () => {
  it("keeps a running leaf tint and gives its sticky cell the matching opaque colour", () => {
    const html = renderRow("running", { theme: "stage", color: "green", cueColorStyle: "full_row", stickyRight: true });
    expect(html).toContain("background:color-mix(in srgb, #22c55e 28%, var(--wc-bg-app))");
    expect(html.split("color-mix(in srgb, #22c55e 28%, var(--wc-bg-app))")).toHaveLength(3);
  });

  it("brightens group and Number rows while mixing sticky backgrounds over their own base", () => {
    const group = renderRow("standby", { theme: "stage", color: "yellow", cueColorStyle: "full_row", cueType: "group", isGroup: true, stickyRight: true });
    expect(group).toContain("background:color-mix(in srgb, #eab308 45%, var(--wc-bg-group))");
    expect(group.split("color-mix(in srgb, #eab308 45%, var(--wc-bg-group))")).toHaveLength(3);

    const number = renderRow("standby", { theme: "stage", color: "blue", cueColorStyle: "full_row", cueType: "number", stickyRight: true });
    expect(number).toContain("background:color-mix(in srgb, #3b82f6 45%, var(--wc-bg-surface))");
    expect(number.split("color-mix(in srgb, #3b82f6 45%, var(--wc-bg-surface))")).toHaveLength(3);

    const white = renderRow("standby", { theme: "stage", color: "white", cueColorStyle: "full_row", isGroup: true, stickyRight: true });
    expect(white).toContain("background:color-mix(in srgb, #f1f5f9 20%, var(--wc-bg-group))");
    expect(white.split("color-mix(in srgb, #f1f5f9 20%, var(--wc-bg-group))")).toHaveLength(3);
  });

  it("keeps selection and drag states distinct from the authored tint", () => {
    const selected = renderRow("running", { theme: "stage", color: "green", cueColorStyle: "full_row", stickyRight: true, isSelected: true });
    expect(selected).toContain("background:var(--wc-bg-selected)");
    expect(selected).not.toContain("color-mix(in srgb");

    const dragged = renderRow("running", { theme: "stage", color: "green", cueColorStyle: "full_row", isDragOver: true, isDisabled: true });
    expect(dragged).toContain("background:var(--wc-bg-drag-over)");
    expect(dragged).toContain("opacity:0.55");
  });

  it("leaves legacy running backgrounds unchanged", () => {
    const html = renderRow("running", { theme: "dark", color: "green", cueColorStyle: "full_row", stickyRight: true });
    expect(html).toContain("background:var(--wc-bg-running)");
    expect(html).not.toContain("color-mix(in srgb");
  });
});
