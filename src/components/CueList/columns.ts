// Column definitions and config helpers for the Cue List table.

export type ColumnId =
  | "playhead"
  | "led"
  | "number"
  | "name"
  | "notes"
  | "file"
  | "target"
  | "file_size"
  | "resolution"
  | "output"
  | "outputs"
  | "type"
  | "pre_wait"
  | "duration"
  | "post_wait"
  | "continue";

export interface ColumnDef {
  id: ColumnId;
  label: string;
  /** Default pixel width. */
  defaultWidth: number;
  /** Minimum drag width in px. */
  minWidth: number;
  /** Cannot be hidden or reordered. */
  fixed: boolean;
  /** Shows a resize drag handle on the right edge. */
  resizable: boolean;
  /** Sticks to the right edge of the scroll container — always visible. */
  stickyRight?: boolean;
}

export const DEFAULT_COLUMNS: ColumnDef[] = [
  { id: "playhead",  label: "▶",      defaultWidth: 28,  minWidth: 24, fixed: true,  resizable: false },
  { id: "led",       label: "",        defaultWidth: 20,  minWidth: 16, fixed: true,  resizable: false },
  { id: "type",      label: "T",       defaultWidth: 28,  minWidth: 28, fixed: false, resizable: true  },
  { id: "number",    label: "#",       defaultWidth: 36,  minWidth: 36, fixed: false, resizable: true  },
  { id: "name",      label: "Name",    defaultWidth: 97,  minWidth: 80, fixed: true,  resizable: true  },
  { id: "file",      label: "File",    defaultWidth: 126, minWidth: 80, fixed: false, resizable: true  },
  { id: "notes",     label: "Notes",   defaultWidth: 118, minWidth: 60, fixed: false, resizable: true  },
  { id: "pre_wait",  label: "Pre-W",   defaultWidth: 64,  minWidth: 48, fixed: false, resizable: true  },
  { id: "duration",  label: "Dur",     defaultWidth: 59,  minWidth: 48, fixed: false, resizable: true  },
  { id: "post_wait", label: "Post-W",  defaultWidth: 49,  minWidth: 48, fixed: false, resizable: true  },
  { id: "continue",  label: "C",       defaultWidth: 40,  minWidth: 28, fixed: false, resizable: true  },
  { id: "output",    label: "Output",  defaultWidth: 102, minWidth: 50, fixed: false, resizable: true  },
  { id: "outputs",   label: "Outputs", defaultWidth: 82,  minWidth: 48, fixed: false, resizable: true  },
  { id: "file_size", label: "Size",    defaultWidth: 82,  minWidth: 64, fixed: false, resizable: true  },
  { id: "resolution",label: "Res",     defaultWidth: 100, minWidth: 78, fixed: false, resizable: true  },
  { id: "target",    label: "Target",  defaultWidth: 180, minWidth: 80, fixed: false, resizable: true  },
];

const DEFAULT_ORDER: ColumnId[] = DEFAULT_COLUMNS.map((d) => d.id);

// ---------------------------------------------------------------------------
// Persisted config shape
// ---------------------------------------------------------------------------

export interface ColumnConfig {
  /** Per-column pixel width overrides (absent = use defaultWidth). */
  widths: Partial<Record<ColumnId, number>>;
  /** Columns explicitly hidden by the user. */
  hidden: Partial<Record<ColumnId, boolean>>;
  /** User-defined display order (all column IDs, including hidden). */
  order: ColumnId[];
}

export const DEFAULT_COLUMN_CONFIG: ColumnConfig = {
  widths: {},
  hidden: { file: true, output: true },
  order: DEFAULT_ORDER,
};

const LS_KEY = "inkue_column_config_v2";

export function loadColumnConfig(): ColumnConfig {
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (!raw) return DEFAULT_COLUMN_CONFIG;
    const parsed = JSON.parse(raw) as Partial<ColumnConfig>;
    // Older layouts used `target` to show media filenames. Keep that saved
    // slot, width and visibility as `file`, then add the newly separated true
    // Target column after the media detail columns.
    const rawOrder = (parsed.order ?? []) as string[];
    const legacyTargetColumn = !rawOrder.includes("file");
    const migratedWidths = { ...(parsed.widths ?? {}) };
    const migratedHidden = { ...(parsed.hidden ?? {}) };
    if (legacyTargetColumn) {
      if (migratedWidths.file == null && migratedWidths.target != null) {
        migratedWidths.file = migratedWidths.target;
      }
      delete migratedWidths.target;
      if (migratedHidden.file == null && migratedHidden.target != null) {
        migratedHidden.file = migratedHidden.target;
      }
      delete migratedHidden.target;
    }

    // Keep only IDs that still exist; append any new columns at the end.
    const savedOrder = rawOrder
      .map((id) => legacyTargetColumn && id === "target" ? "file" : id)
      .filter((id) =>
      DEFAULT_ORDER.includes(id as ColumnId),
      ) as ColumnId[];
    const missing = DEFAULT_ORDER.filter((id) => !savedOrder.includes(id));
    const order: ColumnId[] = [...savedOrder, ...missing];
    const missingTargetColumn = !savedOrder.includes("target");

    // Put only newly introduced media-information columns beside File while
    // preserving every column the operator has explicitly reordered.
    const newMediaColumns = (["file_size", "resolution"] as const)
      .filter((id) => !savedOrder.includes(id));
    if (newMediaColumns.length > 0) {
      for (const id of newMediaColumns) {
        const index = order.indexOf(id);
        if (index >= 0) order.splice(index, 1);
      }
      const filePos = order.indexOf("file");
      order.splice(filePos >= 0 ? filePos + 1 : order.length, 0, ...newMediaColumns);
    }

    // Keep the output-assignment lamps beside the existing Output column when
    // upgrading a saved layout, instead of appending them to the far right.
    if (!savedOrder.includes("outputs")) {
      const outputsPos = order.indexOf("outputs");
      if (outputsPos >= 0) order.splice(outputsPos, 1);
      const outputPos = order.indexOf("output");
      order.splice(outputPos >= 0 ? outputPos + 1 : order.length, 0, "outputs");
    }

    // `target` in old layouts was File. Once migrated, the new true Target
    // column is positioned after Resolution (or after File if no detail
    // columns exist), while an explicitly reordered Target stays put.
    if (missingTargetColumn) {
      const appendedTargetPos = order.indexOf("target");
      if (appendedTargetPos >= 0) order.splice(appendedTargetPos, 1);
      const resolutionPos = order.indexOf("resolution");
      const filePos = order.indexOf("file");
      const insertAfter = resolutionPos >= 0 ? resolutionPos : filePos;
      order.splice(insertAfter >= 0 ? insertAfter + 1 : order.length, 0, "target");
    }

    // Ensure "led" always sits right after "playhead" (migration for older configs).
    const phPos = order.indexOf("playhead");
    const ldPos = order.indexOf("led");
    if (phPos >= 0 && ldPos >= 0 && ldPos !== phPos + 1) {
      order.splice(ldPos, 1);
      order.splice(phPos + 1, 0, "led");
    }

    return {
      widths: migratedWidths,
      hidden: migratedHidden,
      order,
    };
  } catch {
    return DEFAULT_COLUMN_CONFIG;
  }
}

export function saveColumnConfig(c: ColumnConfig): void {
  try {
    localStorage.setItem(LS_KEY, JSON.stringify(c));
  } catch {
    // ignore (private / storage-full)
  }
}

// ---------------------------------------------------------------------------
// Derived helpers
// ---------------------------------------------------------------------------

/** All column defs in user-defined order (including hidden). */
export function getOrderedDefs(config: ColumnConfig): ColumnDef[] {
  return config.order
    .map((id) => DEFAULT_COLUMNS.find((d) => d.id === id))
    .filter((d): d is ColumnDef => d != null);
}

/** Visible column defs in user-defined order. */
export function getVisibleDefs(config: ColumnConfig): ColumnDef[] {
  return getOrderedDefs(config).filter((d) => !config.hidden[d.id]);
}

/** CSS grid-template-columns string — all pixel values, no fr units. */
export function buildGridCols(visibleDefs: ColumnDef[], config: ColumnConfig): string {
  return visibleDefs
    .map((d) => {
      const px = config.widths[d.id] ?? d.defaultWidth;
      return `${px}px`;
    })
    .join(" ");
}
