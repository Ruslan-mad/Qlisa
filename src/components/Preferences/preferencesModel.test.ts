import { describe, expect, it } from "vitest";
import type { OutputDestination } from "../../lib/types";
import { mergeOutputMonitorAssignments } from "./preferencesModel";

const output = (id: string, monitor: number | null, name = id): OutputDestination => ({
  id, name, sink_kind: "display", monitor, enabled: true,
  transform: { pan_x: 0, pan_y: 0, scale: 1, rotation: 0, corners: [[0, 0], [0, 0], [0, 0], [0, 0]] },
  fullscreen_locked: true, always_on_top: false, hide_cursor: false,
});

describe("preferences model", () => {
  it("updates only monitors from fresh global preferences", () => {
    const draft = [output("main", 0, "Unsaved Main"), output("preview", null)];
    const latest = [output("main", 1, "Main"), output("preview", 0, "Preview")];
    const merged = mergeOutputMonitorAssignments(draft, latest);
    expect(merged).toEqual([
      { ...draft[0], monitor: 1 },
      { ...draft[1], monitor: 0 },
    ]);
    expect(merged[0].name).toBe("Unsaved Main");
  });

  it("keeps draft-only outputs untouched", () => {
    const draft = [output("new", null, "Draft output")];
    expect(mergeOutputMonitorAssignments(draft, [output("main", 1)])).toEqual(draft);
  });
});
