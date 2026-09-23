// Main cue list table — resizable / hideable / reorderable columns,
// playhead indicator, file drag-drop, and cue context menu.
//
// Layout: the header strip and the rows area share the same gridTemplateColumns
// string (all pixel widths, no fr). Horizontal overflow is handled by a
// scroll-sync pair: the rows container has overflow:auto and scrolls freely;
// the header container has its scrollbar hidden and its scrollLeft is kept in
// sync via an onScroll handler. This way the header always aligns with the
// rows regardless of window width.

import { useEffect, useRef, useState, useMemo, useCallback, Fragment } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { CueRow } from "./CueRow";
import { fileDropPathAllowedForTarget, fileDropTargetForCue } from "./fileDragModel";
import {
  DEFAULT_COLUMNS,
  buildGridCols,
  getVisibleDefs,
  loadColumnConfig,
  saveColumnConfig,
  type ColumnConfig,
  type ColumnDef,
  type ColumnId,
} from "./columns";
import type { ContinueMode, CueSummary, CueType, MediaConversionJob, NumberCueData } from "../../lib/types";
import { useLocale } from "../../i18n";
import { getOutputControlStatuses, listOutputDestinations } from "../../lib/commands";
import type { OutputControlStatus } from "../../lib/types";
import { orderOutputStatuses } from "../Transport/outputFtbModel";
import { cueRangeFromVisibleOrder, cueSelectionFromSweep, flattenVisibleCueTree, isMultiSelectModifier, toggleCueSelection } from "../../lib/cueSelection";
import { AUDIO_EXTS, VIDEO_EXTS, IMAGE_EXTS, MIDI_EXTS, extensionOf } from "../../lib/mediaTypes";
import { buildContinueModeUpdates, resolveContinueModeTargets } from "./continueModeMenu";
import { canTargetCueType, resolveContextCueInsert } from "./contextCueInsert";
import { canAddCuesToNumber, numberTargets } from "./numberContextMenu";
import { numberChildTimelineStartMs } from "../Editor/numberTimelineModel";
import { consumeCueListStopShortcut, isCueListShortcutBlocked, moveCueSelectionByArrow, stopSelectedCueSet } from "./cueListKeyboard";
import { CueTypeIcon } from "../common/CueTypeIcon";
import { MediaConverterDialog } from "../MediaConversion/MediaConverterDialog";
import { useMediaConversionStore } from "../../stores/mediaConversionStore";
import {
  addCue,
  addTargetedCue,
  removeCue,
  duplicateCue,
  groupCues,
  moveCue,
  moveCues,
  ungroup,
  removeCueFromGroup,
  addCueToGroup,
  moveToTopLevel,
  setAudioFile,
  setVideoFile,
  setImageFile,
  setMidiFile,
  setPlayhead,
  stopCue,
  bulkUpdateCues,
} from "../../lib/commands";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------


// Every cue type that can be created, in toolbar order. Adding a new cue type
// needs one entry here; its icon remains exhaustive in CueTypeIcon.
const CUE_TYPES: { type: CueType; label: string }[] = [
    { type: "audio",    label: "Audio" },
    { type: "video",    label: "Video" },
    { type: "image",    label: "Image" },
    { type: "stop",     label: "Stop" },
    { type: "fade",     label: "Fade" },
    { type: "wait",     label: "Wait" },
    { type: "group",    label: "Group" },
    { type: "number",   label: "Number" },
    { type: "midi",     label: "MIDI" },
    { type: "midi_file", label: "MIDI File" },
    { type: "osc",      label: "OSC" },
    { type: "light",    label: "Light" },
    { type: "mic",      label: "Mic" },
    { type: "timecode", label: "Timecode" },
    { type: "text",     label: "Text" },
    { type: "memo",     label: "Memo" },
    { type: "camera",   label: "Camera" },
    { type: "browser",  label: "Browser" },
    { type: "devamp",   label: "Devamp" },
    { type: "script",   label: "Script" },
    // Command cues last: the toolbar groups them behind one button, but the
    // right-click "Add Cue" list is where you look when you want a specific
    // one, so they are spelled out here.
    { type: "start",    label: "Start" },
    { type: "pause",    label: "Pause" },
    { type: "resume",   label: "Resume" },
    { type: "load",     label: "Load" },
    { type: "reset",    label: "Reset" },
    { type: "goto",     label: "Goto" },
    { type: "arm",      label: "Arm" },
    { type: "disarm",   label: "Disarm" },
];

// Cue types that hold a media file, with the open-dialog filter for each.
const FILE_FILTERS: Partial<Record<CueType, { name: string; extensions: string[] }>> = {
  audio: { name: "Audio Files", extensions: [...AUDIO_EXTS] },
  video: { name: "Video Files", extensions: [...VIDEO_EXTS] },
  image: { name: "Image Files", extensions: [...IMAGE_EXTS] },
  midi_file: { name: "MIDI Files", extensions: [...MIDI_EXTS] },
};

/** Cue types that own a file, as far as drop and "Assign … File…" go. */
type MediaCueType = "audio" | "video" | "image" | "midi_file";

function isAudioPath(p: string) {
  return AUDIO_EXTS.has(extensionOf(p));
}
function isVideoPath(p: string) {
  return VIDEO_EXTS.has(extensionOf(p));
}
function isImagePath(p: string) {
  return IMAGE_EXTS.has(extensionOf(p));
}
function isMidiPath(p: string) {
  return MIDI_EXTS.has(extensionOf(p));
}
function isMediaPath(p: string) {
  return isAudioPath(p) || isVideoPath(p) || isImagePath(p) || isMidiPath(p);
}
function cueTypeForPath(p: string): MediaCueType {
  if (isVideoPath(p)) return "video";
  if (isImagePath(p)) return "image";
  if (isMidiPath(p)) return "midi_file";
  return "audio";
}
async function setFileForCue(cueType: MediaCueType, cueId: string, path: string) {
  if (cueType === "video") await setVideoFile(cueId, path);
  else if (cueType === "image") await setImageFile(cueId, path);
  else if (cueType === "midi_file") await setMidiFile(cueId, path);
  else await setAudioFile(cueId, path);
}
// ---------------------------------------------------------------------------
// Cue context-menu item
// ---------------------------------------------------------------------------

/** Compute the set of child cue IDs that should show the inner playhead indicator.
 *
 * Rules per group mode:
 * - Sequential (at outer playhead, Standby): show first child (previews what fires on GO).
 * - Sequential (running): show `active_child_id` (the next child that fires on GO).
 * - Simultaneous (at outer playhead OR running): show all direct children.
 *
 * `outerPlayheadId` is the ID of the cue currently at the outer playhead. */
function computeInnerPlayheadIds(cues: CueSummary[], outerPlayheadId: string | null): Set<string> {
  const result = new Set<string>();
  for (const cue of cues) {
    if (cue.cue_type === "group" || cue.cue_type === "number") {
      const isAtPlayhead = cue.id === outerPlayheadId;
      const isRunning = cue.state === "running";

      if (cue.group_mode === "sequential" || cue.group_mode === "playlist") {
        // active_child_id is the child a GO fires next, in every state. Show it
        // whenever the group is running or parked at the outer playhead, so a
        // child the user parked the Playhead on is highlighted — not just the first.
        // Playlist behaves like Sequential here; Start Random has no armed child.
        if ((isRunning || isAtPlayhead) && cue.active_child_id) {
          result.add(cue.active_child_id);
        }
      } else if (cue.group_mode === "simultaneous") {
        if (isAtPlayhead || isRunning) {
          // All children will fire (or are firing) — show playhead on all.
          for (const child of cue.children ?? []) {
            result.add(child.id);
          }
        }
      }
    }
    if (cue.children?.length) {
      computeInnerPlayheadIds(cue.children, outerPlayheadId).forEach((id) => result.add(id));
    }
  }
  return result;
}

function CtxItem({ label, danger, disabled, icon, onClick }: { label: string; danger?: boolean; disabled?: boolean; icon?: React.ReactNode; onClick: () => void }) {
  const [hov, setHov] = useState(false);

  return (
    <button
      style={{
        display: "flex", alignItems: "center", gap: 8, width: "100%", padding: "6px 16px",
        background: hov && !disabled ? "var(--wc-bg-hover)" : "transparent", border: "none",
        textAlign: "left", color: danger ? "#ef4444" : "var(--wc-text)",
        fontSize: 13, cursor: disabled ? "not-allowed" : "pointer", whiteSpace: "nowrap",
        opacity: disabled ? 0.4 : 1,
      }}
      onMouseEnter={() => setHov(true)}
      onMouseLeave={() => setHov(false)}
      disabled={disabled}
      onClick={onClick}
    >
      {icon ? (
        <span style={{ width: 16, height: 16, display: "inline-flex", alignItems: "center", justifyContent: "center", flexShrink: 0 }}>
          {icon}
        </span>
      ) : null}
      {label}
    </button>
  );
}

// A context-menu row that reveals a flyout of child items on hover. The flyout
// opens to the right by default, or to the left when the menu sits near the
// right edge of the window (`openLeft`).
function CtxSubmenu({ label, openLeft, onMainClick, children }: { label: string; openLeft: boolean; onMainClick?: () => void; children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <div style={{ position: "relative" }} onMouseEnter={() => setOpen(true)} onMouseLeave={() => setOpen(false)}>
      <button
        style={{
          display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12,
          width: "100%", padding: "6px 16px",
          background: open ? "var(--wc-bg-hover)" : "transparent", border: "none",
          textAlign: "left", color: "var(--wc-text)", fontSize: 13, cursor: "default", whiteSpace: "nowrap",
        }}
      >
        <span
          style={{ flex: 1, cursor: onMainClick ? "pointer" : "default" }}
          onClick={onMainClick}
        >
          {label}
        </span>
        <span
          style={{ color: "var(--wc-text-muted)", paddingLeft: 12, cursor: "default" }}
          onClick={(event) => {
            event.stopPropagation();
            setOpen(true);
          }}
        >
          {openLeft ? "‹" : "›"}
        </span>
      </button>
      {open && (
        <div
          style={{
            position: "absolute", top: -5,
            ...(openLeft ? { right: "100%" } : { left: "100%" }),
            background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
            borderRadius: 6, padding: "4px 0", minWidth: 160, maxHeight: 380, overflowY: "auto",
            boxShadow: "0 4px 16px rgba(0,0,0,0.6)",
          }}
        >
          {children}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Column visibility menu (shown on header right-click)
// ---------------------------------------------------------------------------

function ColumnMenu({
  config,
  pos,
  onToggle,
  onClose,
}: {
  config: ColumnConfig;
  pos: { x: number; y: number };
  onToggle: (id: ColumnId, visible: boolean) => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  const items = config.order
    .map((id) => DEFAULT_COLUMNS.find((d) => d.id === id)!)
    .filter((d) => d && !d.fixed);

  const menuW = 200;
  const left = Math.min(pos.x, window.innerWidth - menuW - 8);
  const top  = Math.min(pos.y, window.innerHeight - items.length * 30 - 50);

  return (
    <>
      <div style={{ position: "fixed", inset: 0, zIndex: 9998 }} onClick={onClose} />
      <div
        style={{
          position: "fixed", left, top,
          background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
          borderRadius: 6, padding: "6px 0", zIndex: 9999,
          minWidth: menuW, boxShadow: "0 4px 16px rgba(0,0,0,0.6)",
        }}
      >
        <div style={{ padding: "2px 12px 6px", fontSize: 10, color: "var(--wc-text-muted)", textTransform: "uppercase", letterSpacing: "0.05em" }}>
          {t("cueList.columns")}
        </div>
        {items.map((d) => (
          <label
            key={d.id}
            style={{
              display: "flex", alignItems: "center", gap: 8,
              padding: "5px 12px", cursor: "pointer",
              fontSize: 12, color: "var(--wc-text)", userSelect: "none",
            }}
          >
            <input
              type="checkbox"
              checked={!config.hidden[d.id]}
              onChange={(e) => onToggle(d.id, e.target.checked)}
              style={{ accentColor: "var(--wc-accent)" }}
            />
            {d.id === "number" ? t("cueList.number") : d.id === "name" ? t("common.name") : d.id === "notes" ? t("cueList.notes") : d.id === "file" ? t("cueList.file") : d.id === "target" ? t("inspector.target") : d.id === "file_size" ? t("cueList.fileSize") : d.id === "resolution" ? t("cueList.resolution") : d.id === "output" ? t("output.output") : d.id === "outputs" ? t("output.outputs") : d.id === "type" ? t("cueList.type") : d.id === "pre_wait" ? t("transport.preWait") : d.id === "duration" ? t("cueList.duration") : d.id === "post_wait" ? t("transport.postWait") : d.id === "continue" ? t("inspector.continueMode") : (d.label || d.id.replace(/_/g, " "))}
          </label>
        ))}
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// RowGhost — floating row card that follows the cursor during cue reorder
// ---------------------------------------------------------------------------

const ROW_GHOST_COLORS: Record<string, string> = {
  none: "transparent", red: "#ef4444", orange: "#f97316", yellow: "#eab308",
  green: "#22c55e", cyan: "#06b6d4", blue: "#3b82f6",
  purple: "#a855f7", pink: "#ec4899", white: "#f1f5f9", black: "#334155",
};

function RowGhost({ cue, x, y, rotation }: { cue: CueSummary; x: number; y: number; rotation: number }) {
  const { t } = useLocale();
  const accent = ROW_GHOST_COLORS[cue.color] ?? "transparent";
  const hasColor = accent !== "transparent";
  return (
    <div
      className="wc-drag-ghost"
      style={{
        position: "fixed", left: x, top: y,
        width: 320, height: 36,
        transform: `translate(-40px, -50%) rotate(${rotation.toFixed(2)}deg) scale(1.04)`,
        pointerEvents: "none", zIndex: 99999,
        background: "var(--wc-bg-surface)",
        border: "1px solid var(--wc-border-strong)",
        borderRadius: 6,
        display: "flex", alignItems: "center",
        overflow: "hidden",
        boxShadow: "0 16px 40px rgba(0,0,0,0.6), 0 4px 12px rgba(0,0,0,0.4)",
      }}
    >
      {hasColor && (
        <div style={{ width: 4, alignSelf: "stretch", background: accent, flexShrink: 0 }} />
      )}
      <span style={{
        fontSize: 10, color: "var(--wc-text-faint)", fontFamily: "monospace",
        padding: "0 8px", flexShrink: 0, minWidth: 36, textAlign: "right",
      }}>
        {cue.number ?? "–"}
      </span>
      <span style={{ display: "flex", alignItems: "center", flexShrink: 0, marginRight: 6 }}>
        <CueTypeIcon type={cue.cue_type} size={14} tone="neutral" />
      </span>
      <span style={{
        fontSize: 13, fontWeight: 600, color: "var(--wc-text)",
        overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1,
      }}>
        {cue.name || t("uiFixes.untitled")}
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main component
// ---------------------------------------------------------------------------

interface Props {
  onCueDoubleClick: (cue: CueSummary) => void;
  onOpenInspector: () => void;
  onRefresh: () => void;
}

export function CueListView({ onCueDoubleClick, onOpenInspector, onRefresh }: Props) {
  const { t, locale } = useLocale();
  const {
    cues, selectedCueId, selectedCueIds, playheadCueId,
    setCueSelection, setPlayheadCueId, generalPrefs, displayPrefs,
  } = useWorkspaceStore();
  const [outputStatuses, setOutputStatuses] = useState<OutputControlStatus[]>([]);

  useEffect(() => {
    let active = true;
    void Promise.all([getOutputControlStatuses(), listOutputDestinations()])
      .then(([statuses, destinations]) => {
        if (active) setOutputStatuses(orderOutputStatuses(statuses, destinations));
      })
      .catch(console.error);
    return () => { active = false; };
  }, [displayPrefs.output_destinations]);

  const rowHeight = generalPrefs.cue_row_height === "compact" ? 22
    : generalPrefs.cue_row_height === "tall" ? 32
    : 26;

  // Auto-scroll to playhead when it moves
  useEffect(() => {
    if (!generalPrefs.auto_scroll_to_playhead || !playheadCueId) return;
    const el = rowsScrollRef.current?.querySelector(`[data-cue-id="${playheadCueId}"]`);
    if (el) el.scrollIntoView({ block: "nearest" });
  }, [playheadCueId, generalPrefs.auto_scroll_to_playhead]);

  // ---------- Column config ----------
  const [colConfig, setColConfig] = useState<ColumnConfig>(loadColumnConfig);
  const [colMenuPos, setColMenuPos] = useState<{ x: number; y: number } | null>(null);
  const [draggingColId, setDraggingColId] = useState<ColumnId | null>(null);
  const [hoveredResizeId, setHoveredResizeId] = useState<ColumnId | null>(null);

  // ---------- Group expand/collapse ----------
  const [expandedGroupIds, setExpandedGroupIds] = useState<Set<string>>(new Set());

  function toggleGroupExpand(groupId: string) {
    setExpandedGroupIds(prev => {
      const next = new Set(prev);
      if (next.has(groupId)) next.delete(groupId);
      else next.add(groupId);
      return next;
    });
  }

  // Flatten nested cue tree into a display list respecting expansion state.
  const flatItems = useMemo(
    () => flattenVisibleCueTree(cues, expandedGroupIds),
    [cues, expandedGroupIds],
  );

  // Number children have no independent pre-wait clock. Show their authored
  // position on the Number timeline in that column instead of the child cue's
  // (normally zero) persisted pre_wait_ms.
  const numberTimelineStartByChildId = useMemo(() => {
    const byId = new Map(flatItems.map((item) => [item.cue.id, item.cue]));
    const starts = new Map<string, number>();
    for (const item of flatItems) {
      if (!item.parentGroupId) continue;
      const parent = byId.get(item.parentGroupId);
      if (parent?.cue_type !== "number" || !parent.children) continue;
      const start = numberChildTimelineStartMs(parent as NumberCueData, item.cue.id);
      if (start != null) starts.set(item.cue.id, start);
    }
    return starts;
  }, [flatItems]);

  // Inner playhead IDs — children shown with the playhead indicator because
  // their parent group is running and they are the active child (sequential)
  // or all children (simultaneous).
  const innerPlayheadIds = useMemo(
    () => computeInnerPlayheadIds(cues, playheadCueId ?? null),
    [cues, playheadCueId],
  );

  // ---------- Multi-selection anchors ----------
  // anchorCueIdRef: the fixed end of a range selection (set by plain click / plain arrow).
  // selectionEndRef: the moving end (updated by shift+click / shift+arrow).
  const anchorCueIdRef = useRef<string | null>(null);
  const selectionEndRef = useRef<string | null>(null);
  const selectedCueSet = useMemo(() => new Set(selectedCueIds), [selectedCueIds]);

  useEffect(() => { saveColumnConfig(colConfig); }, [colConfig]);

  const visibleDefs = useMemo(() => getVisibleDefs(colConfig), [colConfig]);
  const gridCols    = useMemo(() => buildGridCols(visibleDefs, colConfig), [visibleDefs, colConfig]);

  const setColConfigRef = useRef(setColConfig);
  setColConfigRef.current = setColConfig;

  // ---------- Cue drag-and-drop state ----------
  const [draggingCueId,     setDraggingCueId]     = useState<string | null>(null);
  const [dropInsertIndex,   setDropInsertIndex]    = useState<number | null>(null);
  const [dropTargetGroupId, setDropTargetGroupId] = useState<string | null>(null);
  const [ghostState, setGhostState] = useState<{ x: number; y: number; rotation: number } | null>(null);
  const prevMouseXRef  = useRef<number | null>(null);
  const smoothedVelRef = useRef(0);

  // ---------- New-cue drag state (toolbar buttons dragged into list) ----------
  // Driven by a CustomEvent "inkue:cue-drag-start" dispatched by external buttons.
  const [newCueDragType,     setNewCueDragType]     = useState<import("../../lib/types").CueType | null>(null);
  const [newCueDragInsertIdx, setNewCueDragInsertIdx] = useState<number | null>(null);

  const newCueDragRef = useRef<{
    cueType: import("../../lib/types").CueType;
    startX: number;
    startY: number;
    active: boolean;
    insertIdx: number | null;
  } | null>(null);

  // Keep a fresh ref to onRefresh so the stale useEffect closure can call it.
  const onRefreshRef = useRef(onRefresh);
  useEffect(() => { onRefreshRef.current = onRefresh; }, [onRefresh]);

  // Tracks an in-progress cue drag entirely in a ref (no re-render on every
  // pixel moved). dropIdx is mirrored here so the mouseup closure can read it.
  // ids: all cues being dragged (single or multi-selection drag).
  const cueDragRef = useRef<{
    id: string;
    ids: string[];
    fromIndex: number;
    parentGroupId: string | null;
    startX: number;
    startY: number;
    active: boolean;
    dropIdx: number | null;
    dropGroupId: string | null;
    /** true = drop at end of group; false = insert at specific position within group */
    dropGroupAtEnd: boolean;
  } | null>(null);

  // A selection sweep can start in a row's left gutter or in the empty list
  // background. Row content keeps its existing reorder gesture.
  const selectionDragRef = useRef<{
    anchorId: string | null;
    existingIds: string[];
    additive: boolean;
    startX: number;
    startY: number;
    active: boolean;
  } | null>(null);

  const [selectionSweepPreview, setSelectionSweepPreview] = useState<{
    startY: number;
    currentY: number;
  } | null>(null);

  // Set to true for one event loop tick after a drag completes so the row's
  // onClick handler can ignore the spurious click that follows mouseup.
  const justDroppedRef = useRef(false);

  // ---------- Scroll-sync refs ----------
  // The header scrollbar is hidden (class="no-scrollbar"); its scrollLeft is
  // driven programmatically by the rows container's onScroll handler.
  const headerScrollRef = useRef<HTMLDivElement>(null);
  const rowsScrollRef   = useRef<HTMLDivElement>(null);

  // ---------- Resize drag state ----------
  const resizingRef = useRef<{ id: ColumnId; startX: number; startW: number } | null>(null);

  // ---------- Column reorder drag state ----------
  const colDragRef = useRef<{
    id: ColumnId;
    startX: number;
    active: boolean;
    originalOrder: ColumnId[];
    lastTargetId: ColumnId | null;
  } | null>(null);

  // Combined document-level pointer tracker for resize + reorder.
  useEffect(() => {
    // Compute insert index from cursor Y, identical to the previous logic.
    function calcInsertIdxFromY(clientY: number): number {
      const rowEls = rowsScrollRef.current
        ? (Array.from(rowsScrollRef.current.querySelectorAll("[data-cue-id]")) as HTMLElement[])
        : [];
      for (const el of rowEls) {
        const rect = el.getBoundingClientRect();
        if (clientY < rect.top + rect.height / 2) {
          return Number(el.dataset.cueIndex ?? 0);
        }
      }
      return flatItemsRef.current.length;
    }

    // Return the child-list position for inserting into `groupId` such that
    // the new child lands just before flatIndex `dropIdx`.
    function resolveChildPositionInGroup(groupId: string, dropIdx: number): number {
      const items = flatItemsRef.current;
      let pos = 0;
      for (let i = 0; i < dropIdx; i++) {
        if (items[i].parentGroupId === groupId) pos++;
      }
      return pos;
    }

    // Return the ID of the first top-level cue at or after flatIndex `idx`.
    // Used when a child cue is dragged to a between-rows drop position.
    function resolveTopLevelBeforeId(idx: number): string | null {
      const items = flatItemsRef.current;
      for (let i = idx; i < items.length; i++) {
        if (items[i].parentGroupId === null) return items[i].cue.id;
      }
      return null; // append at end
    }

    // Determine where to drop a dragged cue.
    //
    // Two-pass approach:
    //   Pass 1 — check if cursor is in the MIDDLE zone of a group header row
    //             (the only case where we drop at end without a positional insert line).
    //   Pass 2 — standard midpoint logic gives a stable insertIdx, then derive
    //             the target group purely from flatItems data.
    //             This is flicker-free: groupId only changes when insertIdx crosses
    //             a real group boundary, not on pixel-level gaps between rows.
    function calcDropTarget(clientY: number): { insertIdx: number; groupId: string | null; atEnd: boolean } {
      const rowEls = rowsScrollRef.current
        ? (Array.from(rowsScrollRef.current.querySelectorAll("[data-cue-id]")) as HTMLElement[])
        : [];

      // Pass 1: group-header middle zone → drop at end of that group.
      for (const el of rowEls) {
        if (el.dataset.isGroup !== "true") continue;
        const rect = el.getBoundingClientRect();
        if (clientY < rect.top || clientY > rect.bottom) continue;
        const relY = clientY - rect.top;
        const DEAD = rect.height * 0.28;
        if (relY >= DEAD && relY <= rect.height - DEAD) {
          const idx = Number(el.dataset.cueIndex ?? 0);
          return { insertIdx: idx, groupId: el.dataset.cueId ?? null, atEnd: true };
        }
      }

      // Pass 2: midpoint insert position.
      let insertIdx = flatItemsRef.current.length;
      for (const el of rowEls) {
        const rect = el.getBoundingClientRect();
        if (clientY < rect.top + rect.height / 2) {
          insertIdx = Number(el.dataset.cueIndex ?? 0);
          break;
        }
      }

      // Derive group from flat-items data: if the item we're inserting BEFORE
      // is a child, we're within its parent group. This is stable — groupId only
      // changes when insertIdx crosses into a different parentGroupId territory.
      const items = flatItemsRef.current;
      const itemAt = insertIdx < items.length ? items[insertIdx] : null;
      const groupId = itemAt?.parentGroupId ?? null;

      return { insertIdx, groupId, atEnd: false };
    }

    const onMove = (e: MouseEvent) => {
      // ── Column resize ─────────────────────────────────────────────────────
      if (resizingRef.current) {
        const { id, startX, startW } = resizingRef.current;
        const def = DEFAULT_COLUMNS.find((d) => d.id === id)!;
        const newW = Math.max(def.minWidth, startW + (e.clientX - startX));
        setColConfigRef.current((prev) => ({
          ...prev,
          widths: { ...prev.widths, [id]: newW },
        }));
        return;
      }

      // ── Column reorder ────────────────────────────────────────────────────
      if (colDragRef.current) {
        const drag = colDragRef.current;
        if (!drag.active) {
          if (Math.abs(e.clientX - drag.startX) < 6) return;
          drag.active = true;
          document.body.style.cursor = "grabbing";
          setDraggingColId(drag.id);
        }
        const colEl = document.elementFromPoint(e.clientX, e.clientY)
          ?.closest("[data-col-id]") as HTMLElement | null;
        const targetId = (colEl?.dataset.colId ?? null) as ColumnId | null;
        if (
          !targetId || targetId === drag.id || targetId === drag.lastTargetId ||
          DEFAULT_COLUMNS.find((d) => d.id === targetId)?.fixed
        ) return;
        drag.lastTargetId = targetId;
        setColConfigRef.current((prev) => {
          const order = [...prev.order];
          const from = order.indexOf(drag.id);
          const to   = order.indexOf(targetId);
          if (from < 0 || to < 0) return prev;
          order.splice(from, 1);
          order.splice(to, 0, drag.id);
          return { ...prev, order };
        });
        return;
      }

      // ── New-cue drag (toolbar button → insert position in list) ──────────
      if (newCueDragRef.current) {
        const drag = newCueDragRef.current;
        if (!drag.active) {
          if (Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) < 5) return;
          drag.active = true;
          document.body.style.cursor = "copy";
          setNewCueDragType(drag.cueType);
        }
        const newDrop = calcInsertIdxFromY(e.clientY);
        drag.insertIdx = newDrop;
        setNewCueDragInsertIdx(newDrop);
        return;
      }

      // ── Cue range selection (row gutter or empty-background sweep) ──────
      if (selectionDragRef.current) {
        const drag = selectionDragRef.current;
        if (!drag.active) {
          if (Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) < 5) return;
          drag.active = true;
        }
        setSelectionSweepPreview({ startY: drag.startY, currentY: e.clientY });
        const row = document.elementFromPoint(e.clientX, e.clientY)
          ?.closest("[data-cue-id]") as HTMLElement | null;
        const endId = row?.dataset.cueId;
        if (!endId) return;
        if (drag.anchorId === null) drag.anchorId = endId;
        if (drag.anchorId === null) return;
        const visibleIds = flatItemsRef.current.map((item) => item.cue.id);
        const next = cueSelectionFromSweep(
          drag.anchorId,
          endId,
          visibleIds,
          drag.existingIds,
          drag.additive,
        );
        setCueSelection(next.selectedCueId, next.selectedCueIds);
        anchorCueIdRef.current = drag.anchorId;
        selectionEndRef.current = endId;
        return;
      }

      // ── Cue reorder ───────────────────────────────────────────────────────
      if (cueDragRef.current) {
        const drag = cueDragRef.current;
        if (!drag.active) {
          if (Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) < 5) return;
          drag.active = true;
          document.body.style.cursor = "none";
          prevMouseXRef.current = e.clientX;
          smoothedVelRef.current = 0;
          setDraggingCueId(drag.id);
        }
        const { insertIdx, groupId, atEnd } = calcDropTarget(e.clientY);
        // Don't allow dropping a cue onto itself as a group target.
        const targetCue = groupId
          ? flatItemsRef.current.find((item) => item.cue.id === groupId)?.cue
          : null;
        const draggedTypes = drag.ids.map((id) =>
          flatItemsRef.current.find((item) => item.cue.id === id)?.cue.cue_type,
        );
        const sourceIncludesNumber = draggedTypes.some((type) => type === "number");
        const targetIsContainer = targetCue?.cue_type === "group" || targetCue?.cue_type === "number";
        const targetAcceptsDrag = !targetIsContainer || (
          !sourceIncludesNumber && (
            targetCue?.cue_type !== "number" ||
            draggedTypes.every((type) => type === "audio" || type === "video" || type === "image" || type === "group")
          )
        );
        const resolvedGroupId = drag.ids.includes(groupId ?? "") || !targetAcceptsDrag ? null : groupId;
        drag.dropIdx = insertIdx;
        drag.dropGroupId = resolvedGroupId;
        drag.dropGroupAtEnd = atEnd;
        if (resolvedGroupId && atEnd) {
          setDropTargetGroupId(resolvedGroupId);
          setDropInsertIndex(null);
        } else if (resolvedGroupId && !atEnd) {
          setDropTargetGroupId(resolvedGroupId);
          setDropInsertIndex(insertIdx);
        } else {
          setDropTargetGroupId(null);
          setDropInsertIndex(insertIdx);
        }
        // Inertia tilt for the floating ghost
        const dx = e.clientX - (prevMouseXRef.current ?? e.clientX);
        prevMouseXRef.current = e.clientX;
        smoothedVelRef.current = smoothedVelRef.current * 0.78 + dx * 0.22;
        const rotation = Math.max(-6, Math.min(6, smoothedVelRef.current * 0.45));
        setGhostState({ x: e.clientX, y: e.clientY, rotation });
        return;
      }
    };

    const onUp = (e: MouseEvent) => {
      if (resizingRef.current) {
        resizingRef.current = null;
        document.body.style.cursor = "";
        return;
      }
      if (newCueDragRef.current) {
        const drag = newCueDragRef.current;
        if (drag.active && drag.insertIdx !== null) {
          const rowsEl = rowsScrollRef.current;
          if (rowsEl) {
            const rect = rowsEl.getBoundingClientRect();
            const inBounds =
              e.clientX >= rect.left && e.clientX <= rect.right &&
              e.clientY >= rect.top  && e.clientY <= rect.bottom;
            if (inBounds) {
              addCue(drag.cueType, drag.insertIdx)
                .then(() => onRefreshRef.current())
                .catch(console.error);
            }
          }
        }
        newCueDragRef.current = null;
        document.body.style.cursor = "";
        setNewCueDragType(null);
        setNewCueDragInsertIdx(null);
        return;
      }
      if (selectionDragRef.current) {
        const drag = selectionDragRef.current;
        if (drag.active && drag.anchorId !== null) {
          // A sweep can cause a synthetic click in some WebView versions;
          // suppress it so the completed range is not collapsed to one row.
          justDroppedRef.current = true;
          setTimeout(() => { justDroppedRef.current = false; }, 0);
        }
        selectionDragRef.current = null;
        setSelectionSweepPreview(null);
        return;
      }
      if (colDragRef.current) {
        colDragRef.current = null;
        document.body.style.cursor = "";
        setDraggingColId(null);
        return;
      }
      if (cueDragRef.current) {
        const drag = cueDragRef.current;
        if (drag.active) {
          if (drag.dropGroupId) {
            // Drop onto / insert into a group.
            const pos = drag.dropGroupAtEnd
              ? -1  // append at end
              : resolveChildPositionInGroup(drag.dropGroupId, drag.dropIdx ?? 0);
            const promises = drag.ids.map((id) =>
              addCueToGroup(id, drag.dropGroupId!, pos).catch(console.error)
            );
            Promise.all(promises).then(onRefresh).catch(console.error);
          } else if (drag.dropIdx !== null) {
            if (drag.parentGroupId) {
              // Cue(s) sourced from inside a group.
              const dropItem = flatItemsRef.current[drag.dropIdx];
              const prevItem = drag.dropIdx > 0 ? flatItemsRef.current[drag.dropIdx - 1] : null;
              const sameGroup =
                dropItem?.parentGroupId === drag.parentGroupId ||
                prevItem?.parentGroupId === drag.parentGroupId;
              if (sameGroup) {
                // Reorder within the same group.
                const childPos = resolveChildPositionInGroup(drag.parentGroupId, drag.dropIdx);
                const promises = drag.ids.map((id) =>
                  addCueToGroup(id, drag.parentGroupId!, childPos).catch(console.error)
                );
                Promise.all(promises).then(onRefresh).catch(console.error);
              } else {
                // Extract to top level.
                const beforeId = resolveTopLevelBeforeId(drag.dropIdx);
                const promises = drag.ids.map((id) =>
                  moveToTopLevel(id, beforeId).catch(console.error)
                );
                Promise.all(promises).then(onRefresh).catch(console.error);
              }
            } else if (drag.ids.length > 1) {
              const beforeId = flatItemsRef.current[drag.dropIdx]?.cue.id ?? null;
              const draggingSet = new Set(drag.ids);
              if (!draggingSet.has(beforeId ?? "")) {
                moveCues(drag.ids, beforeId).then(onRefresh).catch(console.error);
              }
            } else {
              const from   = cuesRef.current.findIndex((c) => c.id === drag.id);
              const newPos = from < drag.dropIdx ? drag.dropIdx - 1 : drag.dropIdx;
              if (from >= 0 && newPos !== from) {
                moveCue(drag.id, newPos).then(onRefresh).catch(console.error);
              }
            }
          }
          // Suppress the spurious onClick that fires after mouseup.
          justDroppedRef.current = true;
          setTimeout(() => { justDroppedRef.current = false; }, 0);
          maybeShowRenumberHint();
        }
        cueDragRef.current = null;
        document.body.style.cursor = "";
        smoothedVelRef.current = 0;
        prevMouseXRef.current = null;
        setDraggingCueId(null);
        setDropInsertIndex(null);
        setDropTargetGroupId(null);
        setGhostState(null);
        return;
      }
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (selectionDragRef.current) selectionDragRef.current = null;
        setSelectionSweepPreview(null);
        if (colDragRef.current?.active) {
          const orig = colDragRef.current.originalOrder;
          setColConfigRef.current((prev) => ({ ...prev, order: orig }));
          colDragRef.current = null;
          document.body.style.cursor = "";
          setDraggingColId(null);
        }
        if (cueDragRef.current?.active) {
          cueDragRef.current = null;
          document.body.style.cursor = "";
          smoothedVelRef.current = 0;
          prevMouseXRef.current = null;
          setDraggingCueId(null);
          setDropInsertIndex(null);
          setDropTargetGroupId(null);
          setGhostState(null);
        }
        if (newCueDragRef.current?.active) {
          newCueDragRef.current = null;
          document.body.style.cursor = "";
          setNewCueDragType(null);
          setNewCueDragInsertIdx(null);
        }
      }
    };

    const onNewCueDragStart = (e: Event) => {
      const { cueType, startX, startY } = (e as CustomEvent).detail as {
        cueType: import("../../lib/types").CueType;
        startX: number;
        startY: number;
      };
      newCueDragRef.current = { cueType, startX, startY, active: false, insertIdx: null };
    };

    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup",   onUp);
    document.addEventListener("keydown",   onKeyDown);
    document.addEventListener("inkue:cue-drag-start", onNewCueDragStart);
    return () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup",   onUp);
      document.removeEventListener("keydown",   onKeyDown);
      document.removeEventListener("inkue:cue-drag-start", onNewCueDragStart);
    };
  }, []);

  // ---------- Column header handlers ----------

  function startResize(e: React.MouseEvent, def: ColumnDef) {
    e.preventDefault();
    e.stopPropagation();
    document.body.style.cursor = "col-resize";
    const startW = colConfig.widths[def.id] ?? def.defaultWidth;
    resizingRef.current = { id: def.id, startX: e.clientX, startW };
  }

  function startColDrag(e: React.MouseEvent, def: ColumnDef) {
    if (e.button !== 0 || def.fixed) return;
    e.preventDefault();
    colDragRef.current = {
      id: def.id,
      startX: e.clientX,
      active: false,
      originalOrder: [...colConfig.order],
      lastTargetId: null,
    };
  }

  // ---------- Cue drag start ----------
  function startCueDrag(e: React.MouseEvent, cueId: string, index: number) {
    if (e.button !== 0) return;
    // Don't steal the event if a column resize handle was clicked.
    if ((e.target as HTMLElement).closest("[data-resize]")) return;

    const flatItem = flatItemsRef.current[index];
    const parentGroupId = flatItem?.parentGroupId ?? null;

    // Allow multi-drag for top-level cues and for children within the same group.
    const dragIds = selectedCueSet.has(cueId) && selectedCueIds.length > 1
      ? [...selectedCueIds]
      : [cueId];

    cueDragRef.current = {
      id: cueId,
      ids: dragIds,
      fromIndex: index,
      parentGroupId,
      startX: e.clientX,
      startY: e.clientY,
      active: false,
      dropIdx: null,
      dropGroupId: null,
      dropGroupAtEnd: true,
    };
  }

  function startSelectionDrag(e: React.MouseEvent, cueId: string | null) {
    if (e.button !== 0) return;
    selectionDragRef.current = {
      anchorId: cueId,
      existingIds: [...selectedCueIdsRef.current],
      additive: isMultiSelectModifier(e.ctrlKey, e.metaKey),
      startX: e.clientX,
      startY: e.clientY,
      active: false,
    };
    setSelectionSweepPreview({ startY: e.clientY, currentY: e.clientY });
  }

  function startBackgroundSelection(e: React.MouseEvent) {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (
      target.closest("[data-cue-id]") ||
      target.closest("button, input, textarea, select, [contenteditable], [data-resize], [data-selection-gutter]")
    ) return;
    e.preventDefault();
    startSelectionDrag(e, null);
  }

  // ---------- File drag-drop state ----------
  const [dragOverCueId,    setDragOverCueId]    = useState<string | null>(null);
  const [dragOverGroupId,  setDragOverGroupId]  = useState<string | null>(null);
  const [isDragging,       setIsDragging]       = useState(false);
  // When a file is dragged in insert-between mode (cursor near row edge),
  // this holds the insertion index; dragOverCueId is null in that case.
  const [fileDragInsertIdx, setFileDragInsertIdx] = useState<number | null>(null);
  const [contextMenu,   setContextMenu]   = useState<{ x: number; y: number; cueId: string | null; parentGroupId?: string | null } | null>(null);
  const [continueContextMenu, setContinueContextMenu] = useState<{
    x: number;
    y: number;
    targetIds: string[];
  } | null>(null);
  const conversionJobs = useMediaConversionStore((state) => state.jobs);
  const [mediaDialog, setMediaDialog] = useState<{ cueIds: string[]; infoOnly: boolean } | null>(null);
  const [mediaDialogJob, setMediaDialogJob] = useState<MediaConversionJob | null>(null);
  useEffect(() => {
    const open = (event: Event) => {
      const job = (event as CustomEvent<MediaConversionJob>).detail;
      if (job?.cue_id) { setMediaDialog({ cueIds: [job.cue_id], infoOnly: false }); setMediaDialogJob(job); }
    };
    window.addEventListener("media-conversion-open", open);
    return () => window.removeEventListener("media-conversion-open", open);
  }, []);

  const cuesRef = useRef(cues);
  useEffect(() => { cuesRef.current = cues; }, [cues]);

  const flatItemsRef = useRef(flatItems);
  useEffect(() => { flatItemsRef.current = flatItems; }, [flatItems]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    (async () => {
      // Cursor in the top/bottom 8 logical px of a row → insert line.
      // Cursor in the middle of a non-group row → assign/replace that cue.
      // Cursor in the middle of a top-level group row → drop into the group.
      const EDGE_PX = 8;
      function resolveFileDragMode(_physX: number, physY: number): {
        insertIdx: number | null;
        assignId: string | null;
        groupId: string | null;
      } {
        const rowsEl = rowsScrollRef.current;
        if (!rowsEl) return { insertIdx: flatItemsRef.current.length, assignId: null, groupId: null };

        // Get the container's bounding rect to convert screen coords to container-relative
        const containerRect = rowsEl.getBoundingClientRect();
        // Tauri gives screen-relative coords; convert to container-relative by subtracting container's top
        const containerRelativeY = physY - containerRect.top + rowsEl.scrollTop;

        const rowEls = Array.from(rowsEl.querySelectorAll("[data-cue-id]")) as HTMLElement[];
        for (const el of rowEls) {
          const rect = el.getBoundingClientRect();
          // Convert row's screen-relative rect to container-relative by subtracting container's top
          const rowTopRelative = rect.top - containerRect.top + rowsEl.scrollTop;
          const rowBottomRelative = rect.bottom - containerRect.top + rowsEl.scrollTop;

          if (containerRelativeY < rowTopRelative) {
            return { insertIdx: Number(el.dataset.cueIndex ?? 0), assignId: null, groupId: null };
          }
          if (containerRelativeY < rowBottomRelative) {
            const idx = Number(el.dataset.cueIndex ?? -1);
            const id  = el.dataset.cueId ?? null;
            if (!id) return { insertIdx: idx >= 0 ? idx : 0, assignId: null, groupId: null };
            if (containerRelativeY - rowTopRelative < EDGE_PX) {
              return { insertIdx: idx >= 0 ? idx : 0, assignId: null, groupId: null };
            }
            if (rowBottomRelative - containerRelativeY < EDGE_PX) {
              return { insertIdx: idx >= 0 ? idx + 1 : flatItemsRef.current.length, assignId: null, groupId: null };
            }
            // Middle of row — group vs normal cue.
            const rowCue = flatItemsRef.current.find((item) => item.cue.id === id)?.cue;
            return {
              insertIdx: null,
              ...(el.dataset.isGroup === "true"
                ? fileDropTargetForCue(rowCue?.cue_type ?? "group", id)
                : fileDropTargetForCue(rowCue?.cue_type, id)),
            };
          }
        }
        return { insertIdx: flatItemsRef.current.length, assignId: null, groupId: null };
      }

      const fn_ = await getCurrentWindow().onDragDropEvent(async (event) => {
        const { type } = event.payload;
        if (type === "enter" || type === "over") {
          setIsDragging(true);
          const pos = event.payload.position;
          if (pos) {
            const { insertIdx, assignId, groupId } = resolveFileDragMode(pos.x, pos.y);
            setFileDragInsertIdx(insertIdx);
            setDragOverCueId(assignId);
            setDragOverGroupId(groupId);
          }
        } else if (type === "leave") {
          setIsDragging(false);
          setDragOverCueId(null);
          setDragOverGroupId(null);
          setFileDragInsertIdx(null);
        } else if (type === "drop") {
          setIsDragging(false);
          setDragOverCueId(null);
          setDragOverGroupId(null);
          setFileDragInsertIdx(null);
          const paths: string[] = (event.payload as { paths?: string[] }).paths ?? [];
          const pos = event.payload.position;
          const { insertIdx, assignId, groupId } = pos
            ? resolveFileDragMode(pos.x, pos.y)
            : { insertIdx: null, assignId: null, groupId: null };
          const candidatePaths = paths.filter(isMediaPath);
          const targetCueType = groupId
            ? flatItemsRef.current.find((item) => item.cue.id === groupId)?.cue.cue_type
            : undefined;
          const mediaPaths = candidatePaths.filter((path) =>
            fileDropPathAllowedForTarget(targetCueType, cueTypeForPath(path)),
          );
          if (mediaPaths.length === 0) return;

          if (groupId) {
            // Drop into group: create cue(s) then move into the group.
            for (const p of mediaPaths) {
              const cueType = cueTypeForPath(p);
              const newId = await addCue(cueType, -1).catch(() => null);
              if (newId) {
                await setFileForCue(cueType, newId, p).catch(console.error);
                await addCueToGroup(newId, groupId, -1).catch(console.error);
                setCueSelection(newId, [newId]);
              }
            }
            await onRefresh();
          } else if (insertIdx !== null) {
            // Insert mode: create new cue(s) at the target position.
            let at = insertIdx;
            for (const p of mediaPaths) {
              const cueType = cueTypeForPath(p);
              const newId = await addCue(cueType, at).catch(() => null);
              if (newId) {
                await setFileForCue(cueType, newId, p).catch(console.error);
                setCueSelection(newId, [newId]);
                at++;
              }
            }
            await onRefresh();
          } else if (mediaPaths.length === 1) {
            await handleFileDrop(mediaPaths[0], assignId);
          } else {
            for (const p of mediaPaths) await handleFileDrop(p, null);
          }
        }
      });
      if (cancelled) fn_(); else unlisten = fn_;
    })().catch(console.error);

    return () => { cancelled = true; unlisten?.(); };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  async function handleFileDrop(filePath: string, targetCueId: string | null) {
    const cueType = cueTypeForPath(filePath);
    if (targetCueId) {
      // Assign to an existing cue only if the file type matches the cue type.
      const targetCue = cuesRef.current.find((c) => c.id === targetCueId);
      if (targetCue?.cue_type === cueType) {
        await setFileForCue(cueType, targetCueId, filePath).catch(console.error);
      } else {
        // Type mismatch — insert a new cue at the end instead.
        const newId = await addCue(cueType, -1).catch(() => null);
        if (newId) {
          await setFileForCue(cueType, newId, filePath).catch(console.error);
          setCueSelection(newId, [newId]);
        }
      }
    } else {
      const newId = await addCue(cueType, -1).catch(() => null);
      if (newId) {
        await setFileForCue(cueType, newId, filePath).catch(console.error);
        setCueSelection(newId, [newId]);
      }
    }
    await onRefresh();
  }

  // ---------- Cue context menu ----------
  const closeCtx = () => setContextMenu(null);
  const closeContinueCtx = () => setContinueContextMenu(null);

  const setContinueMode = async (mode: ContinueMode) => {
    const targetIds = continueContextMenu?.targetIds;
    closeContinueCtx();
    if (!targetIds?.length) return;
    await bulkUpdateCues(buildContinueModeUpdates(targetIds, mode)).catch(console.error);
    // bulk_update_cues emits workspace-modified; the global Tauri event hook
    // refreshes cue summaries, so an explicit refresh here would duplicate it.
  };

  const ctxAddType   = async (type: CueType) => { closeCtx(); await addCue(type, -1).catch(console.error); await onRefresh(); };
  const ctxAddTypeAt = async (type: CueType, offset: 0 | 1) => {
    const clickedCueId = contextMenu?.cueId;
    closeCtx();
    if (!clickedCueId) return;
    const insertion = resolveContextCueInsert(
      type,
      clickedCueId,
      cuesRef.current.map((cue) => cue.id),
      offset,
    );
    if (!insertion) return;

    const newId = await (insertion.targetCueId
      ? addTargetedCue(type, insertion.targetCueId, insertion.position)
      : addCue(type, insertion.position)
    ).catch((error) => {
      console.error(error);
      return null;
    });
    if (!newId) return;

    await onRefresh();
    setCueSelection(newId, [newId]);
    anchorCueIdRef.current = newId;
    selectionEndRef.current = newId;
    onOpenInspector();
  };
  const ctxDuplicate = async () => {
    closeCtx();
    if (!contextMenu?.cueId) return;
    await duplicateCue(contextMenu.cueId).catch(console.error);
    await onRefresh();
  };
  const ctxDelete    = async () => {
    closeCtx();
    if (!contextMenu?.cueId) return;
    await removeCue(contextMenu.cueId).catch(console.error);
    await onRefresh();
  };
  const ctxAddToNumber = async (numberId: string, cueIds: string[]) => {
    closeCtx();
    if (!canAddCuesToNumber(cuesRef.current, cueIds)) return;
    for (const cueId of cueIds) {
      await addCueToGroup(cueId, numberId, -1).catch(console.error);
    }
    await onRefresh();
    setCueSelection(numberId, [numberId]);
  };
  const ctxCreateNumber = async (cueIds: string[]) => {
    closeCtx();
    if (!canAddCuesToNumber(cuesRef.current, cueIds)) return;
    const numberId = await addCue("number", -1).catch(() => null);
    if (!numberId) return;
    for (const cueId of cueIds) {
      await addCueToGroup(cueId, numberId, -1).catch(console.error);
    }
    await onRefresh();
    setCueSelection(numberId, [numberId]);
  };
  const ctxAssignFile = async (cueType: MediaCueType) => {
    const cueId = contextMenu?.cueId;
    closeCtx();
    if (!cueId) return;
    const filter = FILE_FILTERS[cueType];
    if (!filter) return;
    const result = await open({ multiple: false, filters: [{ ...filter, name: `${t(`cueTypes.${cueType === "midi_file" ? "midiFile" : cueType}`)} ${locale === "ru" ? "файлы" : "Files"}` }] });
    if (typeof result === "string") {
      await setFileForCue(cueType, cueId, result).catch(console.error);
      await onRefresh();
    }
  };

  // ---------- Keyboard navigation ----------
  const selectedCueIdsRef = useRef(selectedCueIds); selectedCueIdsRef.current = selectedCueIds;
  const selectedCueIdRef = useRef(selectedCueId); selectedCueIdRef.current = selectedCueId;
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return;
      const modalOpen = Boolean(document.querySelector(
        '[role="dialog"], [aria-modal="true"], [style*="position: fixed"][style*="inset: 0"]',
      ));
      if (isCueListShortcutBlocked(event.target, modalOpen)) return;

      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        if (event.altKey || event.ctrlKey || event.metaKey) return;
        const next = moveCueSelectionByArrow(
          flatItemsRef.current.map((item) => item.cue.id),
          selectedCueIdRef.current,
          selectionEndRef.current,
          anchorCueIdRef.current,
          event.key === "ArrowDown" ? "down" : "up",
          event.shiftKey,
          cuesRef.current,
        );
        if (!next) return;
        event.preventDefault();
        setCueSelection(next.selectedCueId, next.selectedCueIds);
        anchorCueIdRef.current = next.anchorCueId;
        selectionEndRef.current = next.endCueId;
        return;
      }

      if (!consumeCueListStopShortcut(event, selectedCueIdsRef.current.length > 0)) return;
      // Prevent the older window-level S handler from also stopping only the
      // primary cue after this multi-selection stop has already been queued.
      // Repeated keydowns are consumed too, but the helper returns false for
      // them so a held key cannot queue duplicate stop commands.
      // Dispatch together so selected cues (and selected nested groups) stop in
      // the same transport action rather than one after another.
      void stopSelectedCueSet(selectedCueIdsRef.current, cuesRef.current, (cueId) => stopCue(cueId).catch(console.error));
    };

    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [setCueSelection]);

  // ---------------------------------------------------------------------------
  // Render
  // ---------------------------------------------------------------------------

  // Common grid style used by both header and rows.
  // Memoized so its reference is stable across renders — a fresh object here
  // would defeat React.memo on every CueRow (they all take gridStyle).
  const gridStyle: React.CSSProperties = useMemo(() => ({
    display: "grid",
    gridTemplateColumns: gridCols,
    gap: "0 8px",
    padding: "0 8px",
    // min-width: max-content ensures pixel-based columns never get squeezed when
    // the container is narrower than the total column width. The scroll containers
    // handle the overflow.
    minWidth: "max-content",
  }), [gridCols]);

  // --- Stable per-row callbacks (so React.memo(CueRow) can skip unchanged rows) ---
  // The row handlers need values that change every render (flatItems, selection).
  // Reading them through refs lets the handlers stay referentially stable while
  // never going stale — the key to memoizing a large cue list.
  // flatItemsRef is already defined above (kept in sync via useEffect).
  const startCueDragRef = useRef(startCueDrag);  startCueDragRef.current = startCueDrag;
  const startSelectionDragRef = useRef(startSelectionDrag); startSelectionDragRef.current = startSelectionDrag;
  const toggleGroupExpandRef = useRef(toggleGroupExpand); toggleGroupExpandRef.current = toggleGroupExpand;
  const onCueDoubleClickRef = useRef(onCueDoubleClick); onCueDoubleClickRef.current = onCueDoubleClick;

  const handleRowToggleExpand = useCallback((id: string) => toggleGroupExpandRef.current(id), []);
  const handleRowDragStart = useCallback(
    (id: string, index: number, e: React.MouseEvent) => startCueDragRef.current(e, id, index),
    [],
  );
  const handleRowSelectionDragStart = useCallback(
    (id: string, _index: number, e: React.MouseEvent) => startSelectionDragRef.current(e, id),
    [],
  );
  const handleRowDoubleClick = useCallback((cue: CueSummary) => onCueDoubleClickRef.current(cue), []);
  const handleRowRefresh = useCallback(() => onRefreshRef.current(), []);
  const handleRowSaved = useCallback((_cue: CueSummary) => onRefreshRef.current(), []);
  const handleRowContextMenu = useCallback(
    (id: string, parentGroupId: string | null, e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      setContinueContextMenu(null);
      setContextMenu({ x: e.clientX, y: e.clientY, cueId: id, parentGroupId });
    },
    [],
  );
  const handleContinueContextMenu = useCallback(
    (id: string, _parentGroupId: string | null, e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();

      const currentSelection = selectedCueIdsRef.current;
      const clickedWithinSelection = currentSelection.includes(id);
      const targetIds = resolveContinueModeTargets(id, currentSelection);

      if (!clickedWithinSelection) {
        setCueSelection(id, [id]);
        anchorCueIdRef.current = id;
        selectionEndRef.current = id;
      }

      setContextMenu(null);
      setColMenuPos(null);
      setContinueContextMenu({ x: e.clientX, y: e.clientY, targetIds });
    },
    [setCueSelection],
  );
  const handleRowClick = useCallback(
    (id: string, _index: number, parentGroupId: string | null, e: React.MouseEvent) => {
      if (justDroppedRef.current) return;
      // Cue rows are the playlist's keyboard focus target.  In particular,
      // this explicitly blurs an inline editor in a different row so its
      // pending time edit commits when the operator simply clicks elsewhere.
      // (The row drag mousedown intentionally no longer prevents the browser
      // focus transition.)
      (e.currentTarget as HTMLElement).focus();
      setContextMenu(null);
      const items = flatItemsRef.current;
      const visibleIds = items.map((item) => item.cue.id);
      if (e.shiftKey && anchorCueIdRef.current) {
        setCueSelection(id, cueRangeFromVisibleOrder(anchorCueIdRef.current, id, visibleIds));
        selectionEndRef.current = id;
      } else if (isMultiSelectModifier(e.ctrlKey, e.metaKey)) {
        const selIds = selectedCueIdsRef.current;
        const next = toggleCueSelection(selIds, selectedCueIdRef.current, id, visibleIds);
        setCueSelection(next.selectedCueId, next.selectedCueIds);
        anchorCueIdRef.current = next.selectedCueId ?? id;
        selectionEndRef.current = next.selectedCueId ?? id;
      } else {
        setCueSelection(id, [id]);
        // Park the Playhead on this exact cue (backend routes a child of a
        // Sequential group to its ancestor group). Optimistically reflect it.
        setPlayheadCueId(parentGroupId ?? id);
        setPlayhead(id).catch(console.error);
        anchorCueIdRef.current = id;
        selectionEndRef.current = id;
      }
    },
    [setCueSelection, setPlayheadCueId],
  );

  // One-time, non-blocking hint the first time the operator reorders cues while
  // auto-renumber is off — so the new stable-numbers behaviour is discoverable
  // without a modal interrupting the drag.
  const [showRenumberHint, setShowRenumberHint] = useState(false);
  const maybeShowRenumberHint = () => {
    // Read the pref fresh from the store — this runs from a document event
    // handler whose closure could otherwise hold a stale value.
    if (useWorkspaceStore.getState().generalPrefs.auto_renumber_on_reorder) return;
    if (localStorage.getItem("inkue.renumberHintSeen")) return;
    localStorage.setItem("inkue.renumberHintSeen", "1");
    setShowRenumberHint(true);
  };

  const mediaDialogCues = mediaDialog
    ? mediaDialog.cueIds.map((id) => flatItems.find((item) => item.cue.id === id)?.cue ?? null).filter((item): item is CueSummary => !!item && !!item.file_path && (item.cue_type === "audio" || item.cue_type === "video" || item.cue_type === "image"))
    : [];
  const mediaDialogCue = mediaDialogCues[0] ?? null;
  // A completed conversion that was never applied is not an active dialog
  // session. Do not preload it for a new context-menu conversion, or the
  // dialog would hide the Start button and make re-conversion impossible.
  const mediaDialogInitialJobs = mediaDialog && !mediaDialog.infoOnly && !mediaDialogJob
    ? mediaDialog.cueIds.map((id) => {
        const matches = conversionJobs.filter((item) => item.cue_id === id);
        return matches[matches.length - 1];
      }).filter((item): item is MediaConversionJob => !!item)
    : [];
  const hasCompleteDialogJobSet = mediaDialogCues.length === mediaDialog?.cueIds.length
    && mediaDialogInitialJobs.length === mediaDialogCues.length
    && mediaDialogInitialJobs.every((item) => item.status === "queued" || item.status === "running" || item.applied_to_cue);
  const mediaDialogIsBatch = mediaDialogCues.length > 1;

  return (
    <div
      style={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0, outline: "none", position: "relative" }}
      tabIndex={0}
      onContextMenu={(e) => {
        e.preventDefault();
        setContinueContextMenu(null);
        setContextMenu({ x: e.clientX, y: e.clientY, cueId: null });
      }}
      onDragOver={(e) => e.preventDefault()}
      onDrop={(e) => e.preventDefault()}
    >
      {/* ── Column header (scrollbar hidden, scroll driven by rows) ───────── */}
      <div
        ref={headerScrollRef}
        className="no-scrollbar"
        style={{
          overflowX: "scroll",
          overflowY: "hidden",
          flexShrink: 0,
          background: "var(--wc-bg-app)",
          borderBottom: "2px solid var(--wc-border-strong)",
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          setContextMenu(null);
          setContinueContextMenu(null);
          setColMenuPos({ x: e.clientX, y: e.clientY });
        }}
      >
        <div
          style={{
            ...gridStyle,
            height: 28,
            alignItems: "center",
            fontSize: 11,
            color: "var(--wc-text-muted)",
            textTransform: "uppercase",
            letterSpacing: "0.05em",
            userSelect: "none",
          }}
        >
          {visibleDefs.map((def, i) => (
            <div
              key={def.id}
              data-col-id={def.id}
              title={def.fixed ? undefined : t("sweepUi.dragReorder")}
              style={{
                position: def.stickyRight ? "sticky" : "relative",
                right: def.stickyRight ? 0 : undefined,
                zIndex: def.stickyRight ? 3 : undefined,
                background: def.stickyRight ? "var(--wc-bg-app)" : undefined,
                boxShadow: def.stickyRight ? "-4px 0 8px rgba(0,0,0,0.18)" : undefined,
                display: "flex",
                alignItems: "center",
                height: "100%",
                minWidth: 0,
                opacity: draggingColId === def.id ? 0.4 : 1,
                cursor: def.fixed ? "default" : "grab",
                transition: "opacity 0.1s",
                borderRight: i < visibleDefs.length - 1
                  ? `1px solid ${hoveredResizeId === def.id ? "var(--wc-text-faint)" : "var(--wc-border)"}`
                  : undefined,
              }}
              onMouseDown={(e) => startColDrag(e, def)}
            >
              <span
                style={{
                  flex: 1,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                  paddingLeft: 5,
                  paddingRight: 5,
                  pointerEvents: "none",
                }}
              >
                {def.id === "name" ? t("common.name") : def.id === "notes" ? t("cueList.notes") : def.id === "file" ? t("cueList.file") : def.id === "target" ? t("inspector.target") : def.id === "file_size" ? t("cueList.fileSize") : def.id === "resolution" ? t("cueList.resolution") : def.id === "output" ? t("output.output") : def.id === "outputs" ? t("output.outputs") : def.id === "pre_wait" ? t("transport.preWait") : def.id === "post_wait" ? t("transport.postWait") : def.label}
              </span>

              {/* Resize handle — 8 px wide, centred on the right border.
                  Invisible zone (cursor-only feedback); border colour shifts on hover. */}
              {def.resizable && i < visibleDefs.length - 1 && (
                <div
                  data-resize="true"
                  style={{
                    position: "absolute",
                    right: -4,
                    top: 0,
                    bottom: 0,
                    width: 8,
                    cursor: "col-resize",
                    zIndex: 10,
                  }}
                  onMouseDown={(e) => startResize(e, def)}
                  onMouseEnter={() => setHoveredResizeId(def.id)}
                  onMouseLeave={() => setHoveredResizeId(null)}
                />
              )}
            </div>
          ))}
        </div>
      </div>

      {/* ── Cue rows (scrolls both axes; drives header horizontal sync) ────── */}
      <div
        ref={rowsScrollRef}
        style={{ flex: 1, overflow: "auto", position: "relative" }}
        onMouseDown={startBackgroundSelection}
        onClick={(e) => {
          if (justDroppedRef.current) return;
          setContextMenu(null);
          setContinueContextMenu(null);
          setColMenuPos(null);
          // Clear selection when clicking on empty space (not on a cue row).
          if (!(e.target as HTMLElement).closest("[data-cue-id]")) {
            setCueSelection(null, []);
            anchorCueIdRef.current = null;
            selectionEndRef.current = null;
          }
        }}
        onScroll={(e) => {
          if (headerScrollRef.current) {
            headerScrollRef.current.scrollLeft = e.currentTarget.scrollLeft;
          }
        }}
      >
        {selectionSweepPreview && (() => {
          const rect = rowsScrollRef.current?.getBoundingClientRect();
          if (!rect) return null;
          const top = Math.min(selectionSweepPreview.startY, selectionSweepPreview.currentY)
            - rect.top + rowsScrollRef.current!.scrollTop;
          const height = Math.max(
            2,
            Math.abs(selectionSweepPreview.currentY - selectionSweepPreview.startY),
          );
          return (
            <div
              aria-hidden="true"
              style={{
                position: "absolute",
                left: 0,
                right: 0,
                top,
                height,
                background: "color-mix(in srgb, var(--wc-accent) 12%, transparent)",
                borderTop: "1px solid color-mix(in srgb, var(--wc-accent) 65%, transparent)",
                borderBottom: "1px solid color-mix(in srgb, var(--wc-accent) 65%, transparent)",
                pointerEvents: "none",
                zIndex: 20,
              }}
            />
          );
        })()}
        {cues.length === 0 && (
          <div style={{ padding: 32, textAlign: "center", color: "var(--wc-text-faint)", fontSize: 14 }}>
            {t("uiFixes.emptyCueList")}
          </div>
        )}

        {flatItems.map(({ cue, depth, parentGroupId }, flatIndex) => (
          <Fragment key={cue.id}>
            {/* Drop-target indicator ABOVE (file insert, cue reorder, new-cue drag) */}
            {isDragging && fileDragInsertIdx === flatIndex && (
              <div style={{ height: 2, background: "var(--wc-accent)", margin: `0 ${8 + depth * 20}px`, borderRadius: 1, pointerEvents: "none" }} />
            )}
            {draggingCueId !== null && dropInsertIndex === flatIndex && (
              <div style={{
                height: 2, background: "var(--wc-accent)",
                margin: `0 ${8 + depth * 20}px`,
                borderRadius: 1, pointerEvents: "none",
              }} />
            )}
            {newCueDragType !== null && newCueDragInsertIdx === flatIndex && (
              <div style={{ height: 2, background: "#ef4444", margin: "0 8px", borderRadius: 1, pointerEvents: "none" }} />
            )}
            <CueRow
              cue={cue}
              cueIndex={flatIndex}
              gridStyle={gridStyle}
              visibleDefs={visibleDefs}
              rowHeight={rowHeight}
              cueColorStyle={displayPrefs.cue_color_style}
              depth={depth}
              isGroup={cue.cue_type === "group" || cue.cue_type === "number"}
              isGroupExpanded={expandedGroupIds.has(cue.id)}
              onToggleExpand={handleRowToggleExpand}
              isSelected={selectedCueSet.has(cue.id)}
              isAtPlayhead={playheadCueId === cue.id || innerPlayheadIds.has(cue.id)}
              isDragOver={dragOverCueId === cue.id}
              isGroupDropTarget={
              (cue.cue_type === "group" || cue.cue_type === "number") &&
                (dropTargetGroupId === cue.id || dragOverGroupId === cue.id)
              }
              parentGroupId={parentGroupId}
              isDragSource={
                draggingCueId !== null &&
                (cueDragRef.current?.ids.includes(cue.id) ?? false)
              }
              onCueDragStart={handleRowDragStart}
              onSelectionDragStart={handleRowSelectionDragStart}
              onClick={handleRowClick}
              onDoubleClick={handleRowDoubleClick}
              onContextMenu={handleRowContextMenu}
              onContinueContextMenu={handleContinueContextMenu}
              onRefresh={handleRowRefresh}
              onSaved={handleRowSaved}
              outputStatuses={outputStatuses}
              defaultOutputId={displayPrefs.default_output_id}
              numberTimelineStartMs={numberTimelineStartByChildId.get(cue.id)}
            />
          </Fragment>
        ))}

        {/* Drop-target indicators AFTER the last row */}
        {isDragging && fileDragInsertIdx === flatItems.length && (
          <div style={{ height: 2, background: "var(--wc-accent)", margin: "0 8px", borderRadius: 1, pointerEvents: "none" }} />
        )}
        {draggingCueId !== null && dropInsertIndex === flatItems.length && (
          <div style={{ height: 2, background: "var(--wc-accent)", margin: "0 8px", borderRadius: 1, pointerEvents: "none" }} />
        )}
        {/* Drop-target indicator line AFTER the last row (new-cue drag from toolbar) */}
        {newCueDragType !== null && newCueDragInsertIdx === cues.length && (
          <div
            style={{
              height: 2,
              background: "#ef4444",
              margin: "0 8px",
              borderRadius: 1,
              pointerEvents: "none",
            }}
          />
        )}

        {isDragging && !dragOverCueId && fileDragInsertIdx === null && (
          <div
            style={{
              margin: "8px 16px",
              border: "2px dashed var(--wc-accent)",
              borderRadius: 6,
              padding: 16,
              textAlign: "center",
              color: "var(--wc-accent)",
              fontSize: 13,
              pointerEvents: "none",
            }}
          >
            {locale === "ru" ? "Отпустите, чтобы создать новое аудио cue" : "Drop to create new Audio Cue"}
          </div>
        )}
      </div>

      {/* ── First-reorder hint (numbers stay fixed) ──────────────────────── */}
      {showRenumberHint && (
        <div
          style={{
            position: "absolute", bottom: 12, left: "50%", transform: "translateX(-50%)",
            zIndex: 2000, maxWidth: 520,
            display: "flex", alignItems: "center", gap: 12,
            background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
            borderRadius: 8, boxShadow: "0 8px 24px rgba(0,0,0,0.5)", padding: "10px 14px",
          }}
        >
          <span style={{ fontSize: 12, color: "var(--wc-text)", lineHeight: 1.5 }}>
            {t("cueList.renumberFrom")} <strong>{t("cueList.actions")} → {t("cueList.renumberTitle")}</strong>.
            — {t("preferencesExtra.autoRenumberHint")}
          </span>
          <button
            onClick={() => setShowRenumberHint(false)}
            style={{
              flexShrink: 0, background: "var(--wc-accent)", border: "none", borderRadius: 4,
              color: "var(--wc-accent-fg)", fontSize: 12, cursor: "pointer", padding: "4px 12px",
            }}
          >
            {t("common.done")}
          </button>
        </div>
      )}

      {/* ── Row drag ghost ───────────────────────────────────────────────── */}
      {draggingCueId !== null && ghostState !== null && (() => {
        const cue = cues.find(c => c.id === draggingCueId)
          ?? flatItems.find(fi => fi.cue.id === draggingCueId)?.cue;
        return cue
          ? <RowGhost cue={cue} x={ghostState.x} y={ghostState.y} rotation={ghostState.rotation} />
          : null;
      })()}

      {/* ── Column visibility menu ────────────────────────────────────────── */}
      {colMenuPos && (
        <ColumnMenu
          config={colConfig}
          pos={colMenuPos}
          onToggle={(id, visible) =>
            setColConfig((prev) => ({ ...prev, hidden: { ...prev.hidden, [id]: !visible } }))
          }
          onClose={() => setColMenuPos(null)}
        />
      )}

      {/* ── Continue-mode context menu ────────────────────────────────────── */}
      {continueContextMenu && (
        <>
          <div
            style={{ position: "fixed", inset: 0, zIndex: 9998 }}
            onClick={closeContinueCtx}
            onContextMenu={(e) => { e.preventDefault(); e.stopPropagation(); closeContinueCtx(); }}
          />
          <div
            style={{
              position: "fixed",
              left: continueContextMenu.x,
              top: continueContextMenu.y,
              background: "var(--wc-bg-surface)",
              border: "1px solid var(--wc-border-strong)",
              borderRadius: 6,
              padding: "4px 0",
              zIndex: 9999,
              minWidth: 180,
              boxShadow: "0 4px 16px rgba(0,0,0,0.6)",
              fontSize: 13,
            }}
          >
            <CtxItem label={t("inspector.autoContinue")} onClick={() => void setContinueMode("auto_continue")} />
            <CtxItem label={t("inspector.autoFollow")} onClick={() => void setContinueMode("auto_follow")} />
            <CtxItem label={t("actions.reset")} onClick={() => void setContinueMode("do_not_continue")} />
          </div>
        </>
      )}

      {/* ── Cue context menu ──────────────────────────────────────────────── */}
      {contextMenu && (
        <>
          <div
            style={{ position: "fixed", inset: 0, zIndex: 9998 }}
            onClick={closeCtx}
            onContextMenu={(e) => { e.preventDefault(); closeCtx(); }}
          />
          <div
            style={{
              position: "fixed",
              left: contextMenu.x,
              top: contextMenu.y,
              background: "var(--wc-bg-surface)",
              border: "1px solid var(--wc-border-strong)",
              borderRadius: 6,
              padding: "4px 0",
              zIndex: 9999,
              minWidth: 200,
              boxShadow: "0 4px 16px rgba(0,0,0,0.6)",
              fontSize: 13,
            }}
          >
            {(() => {
              const openLeft = contextMenu.x > window.innerWidth - 380;
              if (!contextMenu.cueId) {
                return (
                  <CtxSubmenu label={`${t("common.add")} ${t("cueList.cue")}`} openLeft={openLeft}>
                    {CUE_TYPES.map((ct) => (
                      <CtxItem key={ct.type} icon={<CueTypeIcon type={ct.type} size={16} tone="type" />} label={t(`cueTypes.${ct.type === "midi_file" ? "midiFile" : ct.type}`)} onClick={() => ctxAddType(ct.type)} />
                    ))}
                  </CtxSubmenu>
                );
              }
              const ctxType = flatItems.find((fi) => fi.cue.id === contextMenu.cueId)?.cue.cue_type ?? null;
              const contextCue = flatItems.find((fi) => fi.cue.id === contextMenu.cueId)?.cue ?? null;
              const assignType: MediaCueType | null =
                ctxType === "audio" || ctxType === "video" || ctxType === "image" || ctxType === "midi_file"
                  ? ctxType
                  : null;
              const conversionTargetIds = selectedCueIds.length > 1 && selectedCueIds.includes(contextMenu.cueId)
                ? selectedCueIds
                : [contextMenu.cueId];
              const conversionTargets = conversionTargetIds
                .map((id) => flatItems.find((item) => item.cue.id === id)?.cue)
                .filter((item): item is CueSummary => !!item && !!item.file_path && (item.cue_type === "audio" || item.cue_type === "video" || item.cue_type === "image"));
              return (
              <>
                {!contextMenu.parentGroupId && (
                  <>
                    <CtxSubmenu label={`${t("common.add")} ${t("cueList.cue")} ↑`} openLeft={openLeft}>
                      {CUE_TYPES.map((ct) => (
                        <CtxItem key={ct.type} disabled={ctxType != null && !canTargetCueType(ct.type, ctxType)} icon={<CueTypeIcon type={ct.type} size={16} tone="type" />} label={t(`cueTypes.${ct.type === "midi_file" ? "midiFile" : ct.type}`)} onClick={() => ctxAddTypeAt(ct.type, 0)} />
                      ))}
                    </CtxSubmenu>
                    <CtxSubmenu label={`${t("common.add")} ${t("cueList.cue")} ↓`} openLeft={openLeft}>
                      {CUE_TYPES.map((ct) => (
                        <CtxItem key={ct.type} disabled={ctxType != null && !canTargetCueType(ct.type, ctxType)} icon={<CueTypeIcon type={ct.type} size={16} tone="type" />} label={t(`cueTypes.${ct.type === "midi_file" ? "midiFile" : ct.type}`)} onClick={() => ctxAddTypeAt(ct.type, 1)} />
                      ))}
                    </CtxSubmenu>
                    <div style={{ height: 1, background: "var(--wc-border-strong)", margin: "4px 0" }} />
                  </>
                )}
                <CtxItem label={t("common.copy")} onClick={ctxDuplicate} />
                <CtxItem label={t("common.delete")} danger onClick={ctxDelete} />
                {/* Group / ungroup */}
                {!contextMenu.parentGroupId && (() => {
                  const ids = selectedCueIds.length > 1 && contextMenu.cueId && selectedCueIds.includes(contextMenu.cueId)
                    ? selectedCueIds
                    : contextMenu.cueId ? [contextMenu.cueId] : [];
                   const label = ids.length > 1 ? t("sweepUi.groupCues", { count: ids.length }) : t("sweepUi.groupCue");
                  return ids.length > 0 ? (
                    <>
                      <div style={{ height: 1, background: "var(--wc-border-strong)", margin: "4px 0" }} />
                      <CtxItem
                        label={label}
                        onClick={async () => {
                          closeCtx();
                          const newGroupId = await groupCues(ids).catch(() => null);
                          if (newGroupId) {
                            setCueSelection(newGroupId, [newGroupId]);
                            await onRefresh();
                          }
                        }}
                      />
                      {canAddCuesToNumber(
                        flatItems.filter((item) => !item.parentGroupId).map((item) => item.cue),
                        ids,
                      ) && (() => {
                        const targets = numberTargets(cues);
                        const label = locale === "ru" ? "Добавить в номер" : "Add to Number";
                        return targets.length > 0 ? (
                          <CtxSubmenu label={label} openLeft={openLeft} onMainClick={() => void ctxCreateNumber(ids)}>
                            {targets.map((number) => (
                              <CtxItem
                                key={number.id}
                                label={[number.number, number.name].filter(Boolean).join(" · ") || t("cueTypes.number")}
                                onClick={() => void ctxAddToNumber(number.id, ids)}
                              />
                            ))}
                          </CtxSubmenu>
                        ) : (
                          <CtxItem label={label} onClick={() => void ctxCreateNumber(ids)} />
                        );
                      })()}
                    </>
                  ) : null;
                })()}
                {/* Group-specific actions */}
                {(() => {
                  const cueItem = flatItems.find(fi => fi.cue.id === contextMenu.cueId);
                  const isPlainGroup = cueItem?.cue.cue_type === "group";
                  const inGroup = !!contextMenu.parentGroupId;
                  return (
                    <>
                      {isPlainGroup && (
                        <>
                          <div style={{ height: 1, background: "var(--wc-border-strong)", margin: "4px 0" }} />
                          <CtxItem
                            label={t("common.remove")}
                            onClick={async () => {
                              closeCtx();
                              if (!contextMenu.cueId) return;
                              await ungroup(contextMenu.cueId).catch(console.error);
                              await onRefresh();
                            }}
                          />
                        </>
                      )}
                      {inGroup && (
                        <>
                          <div style={{ height: 1, background: "var(--wc-border-strong)", margin: "4px 0" }} />
                          <CtxItem
                            label={t("help.moveToPlaylist")}
                            onClick={async () => {
                              closeCtx();
                              if (!contextMenu.cueId || !contextMenu.parentGroupId) return;
                              await removeCueFromGroup(contextMenu.parentGroupId, contextMenu.cueId).catch(console.error);
                              await onRefresh();
                            }}
                          />
                          <CtxItem
                            label={t("common.remove")}
                            onClick={async () => {
                              closeCtx();
                              if (!contextMenu.cueId || !contextMenu.parentGroupId) return;
                              // Remove all selected cues that share this parent group,
                              // or just the right-clicked cue if it's not in the selection.
                              const groupId = contextMenu.parentGroupId;
                              const targets =
                                selectedCueIds.includes(contextMenu.cueId)
                                  ? selectedCueIds.filter((id) => {
                                      const fi = flatItems.find((f) => f.cue.id === id);
                                      return fi?.parentGroupId === groupId;
                                    })
                                  : [contextMenu.cueId];
                              await Promise.all(
                                targets.map((id) => removeCueFromGroup(groupId, id).catch(console.error))
                              );
                              await onRefresh();
                            }}
                          />
                        </>
                      )}
                    </>
                  );
                })()}
                {!contextMenu.parentGroupId && assignType && (
                  <>
                    <div style={{ height: 1, background: "var(--wc-border-strong)", margin: "4px 0" }} />
                    <CtxItem
                      label={assignType === "midi_file"
                        ? t("sweepUi.assignFile", { type: "MIDI" })
                        : t("sweepUi.assignFile", { type: t(`cueTypes.${assignType}`) })}
                      onClick={() => ctxAssignFile(assignType)}
                    />
                  </>
                )}
                {(assignType === "audio" || assignType === "video" || assignType === "image") && contextCue?.file_path && (
                  <>
                    <div style={{ height: 1, background: "var(--wc-border-strong)", margin: "4px 0" }} />
                    <CtxItem
                      label={t("mediaConversion.convert")}
                      onClick={() => {
                        closeCtx();
                        if (conversionTargets.length) {
                          setMediaDialog({ cueIds: conversionTargets.map((item) => item.id), infoOnly: false });
                          setMediaDialogJob(null);
                        }
                      }}
                    />
                    <CtxItem
                      label={t("mediaConversion.info")}
                      onClick={() => {
                        const id = contextMenu.cueId;
                        closeCtx();
                        if (id) setMediaDialog({ cueIds: [id], infoOnly: true });
                      }}
                    />
                  </>
                )}
              </>
              );
            })()}
          </div>
        </>
      )}

      {mediaDialogCue && (
        <MediaConverterDialog
          cue={mediaDialogCue}
          cues={mediaDialogCues}
          infoOnly={mediaDialog?.infoOnly ?? false}
          initialJob={mediaDialogJob ?? (!mediaDialogIsBatch ? mediaDialogInitialJobs[0] ?? null : null)}
          initialJobs={hasCompleteDialogJobSet ? mediaDialogInitialJobs : []}
          onClose={() => { setMediaDialog(null); setMediaDialogJob(null); }}
          onRefresh={() => void onRefresh()}
        />
      )}
    </div>
  );
}
