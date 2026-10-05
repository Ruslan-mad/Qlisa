import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CueSummary } from "../../lib/types";
import { setLocale } from "../../i18n";
import { CueRow } from "./CueRow";
import type { ColumnDef } from "./columns";

const { timingState } = vi.hoisted(() => ({ timingState: { current: null as unknown } }));
vi.mock("../../stores/timingStore", () => ({
  useTimingStore: (selector: (state: { timings: Record<string, unknown> }) => unknown) =>
    selector({ timings: { "cue-1": timingState.current } }),
}));

const columns: ColumnDef[] = [
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

function renderRow(state: CueSummary["state"] = "standby") {
  return renderToStaticMarkup(createElement(CueRow, {
    cue: { ...cue, state }, cueIndex: 0, gridStyle: { display: "grid", gridTemplateColumns: "40px 64px 49px" },
    visibleDefs: columns, isSelected: false, isAtPlayhead: false,
    onCueDragStart: () => {}, onSelectionDragStart: () => {}, onClick: () => {},
    onDoubleClick: () => {}, onContextMenu: () => {}, onContinueContextMenu: () => {},
  }));
}

afterEach(() => {
  timingState.current = null;
  setLocale("en");
});

describe("CueRow transport wait display", () => {
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
