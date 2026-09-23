// A single row in the cue list table.

import { memo, useState } from "react";
import { PlayheadIndicator } from "./PlayheadIndicator";
import { RunningLed } from "../common/RunningLed";
import type { ColumnDef } from "./columns";
import type { ContinueMode, CueColorStyle, CueSummary, OutputControlStatus } from "../../lib/types";
import { useTimingStore } from "../../stores/timingStore";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { setCueDuration, updateCue } from "../../lib/commands";
import { useLocale } from "../../i18n";
import { GroupExpander } from "./GroupExpander";
import { InlineTimeCell } from "./InlineTimeCell";
import { canEditCueDuration, emptyCueDurationValue } from "./inlineTimeModel";
import { CueTypeIcon } from "../common/CueTypeIcon";
import { durationProgressPercent } from "./durationProgress";
import { formatDurationMs } from "./formatDuration";
import { cueFileName, cueNotesProperty, cueNotesText, formatTargetCues } from "./cueRowContent";
import {
  assessFileSize,
  assessResolution,
  formatFileSize,
  formatResolution,
} from "./mediaInfo";
import { cueOutputAssignments } from "../Transport/outputFtbModel";

const INLINE_INPUT_STYLE: React.CSSProperties = {
  width: "100%",
  background: "var(--wc-bg-app)",
  border: "1px solid var(--wc-accent)",
  borderRadius: 3,
  color: "var(--wc-text)",
  fontSize: 12,
  textAlign: "right",
  padding: "0 4px",
  outline: "none",
  boxSizing: "border-box",
  height: "100%",
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

function ContinueModeIcon({ mode }: { mode: ContinueMode }) {
  if (mode === "do_not_continue") {
    return <span style={{ color: "var(--wc-text-bright)", fontWeight: 600 }}>—</span>;
  }

  const arrowHead = mode === "auto_continue"
    ? "M3.5 10 8.5 15 13.5 10"
    : "M10 3.5 15 8.5 10 13.5";
  const arrowShaft = mode === "auto_continue"
    ? "M8.5 2v13"
    : "M2 8.5h13";

  return (
    <svg
      aria-hidden="true"
      width="17"
      height="17"
      viewBox="0 0 17 17"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ display: "block", color: "var(--wc-text-bright)" }}
    >
      <path d={arrowShaft} />
      <path d={arrowHead} />
    </svg>
  );
}

const COLOR_SWATCHES: Record<string, string> = {
  none:   "transparent",
  red:    "#ef4444",
  orange: "#f97316",
  yellow: "#eab308",
  green:  "#22c55e",
  cyan:   "#06b6d4",
  blue:   "#3b82f6",
  purple: "#a855f7",
  pink:   "#ec4899",
  white:  "#f1f5f9",
  black:  "#334155",
};

const MEDIA_ISSUE_TRANSLATION_KEYS: Record<string, string> = {
  missing_file: "cueList.missingMediaFile",
  empty_file: "cueList.zeroByteFile",
  large_image: "cueList.largeImageFile",
  large_audio: "cueList.largeAudioFile",
  large_video: "cueList.largeVideoFile",
  large_midi_file: "cueList.largeMidiFile",
  high_video_bitrate: "cueList.highAverageBitrate",
  high_audio_bitrate: "cueList.highAverageBitrate",
  four_k_video: "cueList.video4k",
  huge_image: "cueList.oversizedImageResolution",
};

const MEDIA_COMPATIBILITY_REASON_TEXT: Record<number, [string, string]> = {
  1: ["Аудиокодек может не поддерживаться — Оптимизировать", "Audio codec may not be supported — Optimize"],
  2: ["Контейнер видео не MP4/MOV — Оптимизировать", "Video container is not MP4/MOV — Optimize"],
  3: ["Видеокодек может быть тяжёлым — Оптимизировать", "Video codec may be difficult to decode — Optimize"],
  4: ["10-битный HEVC может работать нестабильно — Оптимизировать", "10-bit HEVC may be unstable — Optimize"],
  5: ["Формат пикселей видео может работать нестабильно — Оптимизировать", "Video pixel format may be unstable — Optimize"],
  6: ["Видео выше 4K UHD — Оптимизировать", "Video exceeds 4K UHD — Optimize"],
  7: ["Видео выше 60 кадров/с — Оптимизировать", "Video exceeds 60 fps — Optimize"],
  8: ["Аудиокодек видео не AAC — Оптимизировать", "Video audio codec is not AAC — Optimize"],
  9: ["Частота аудио не 48 кГц — Оптимизировать", "Audio sample rate is not 48 kHz — Optimize"],
  10: ["Аудио не стерео — Оптимизировать", "Audio is not stereo — Optimize"],
  11: ["Размеры видео не чётные — Оптимизировать", "Video dimensions are not even — Optimize"],
  12: ["Совместимость медиа требует внимания — Оптимизировать", "Media compatibility needs attention — Optimize"],
  13: ["HEVC может создавать повышенную нагрузку — Оптимизировать", "HEVC may put extra load on playback — Optimize"],
};

/** `#rrggbb` -> `rgba(r, g, b, alpha)`, used to tint the whole row without
 *  drowning out the text in "full row" cue colour style. */
function hexToRgba(hex: string, alpha: number): string {
  const m = /^#([0-9a-f]{6})$/i.exec(hex);
  if (!m) return hex;
  const n = parseInt(m[1], 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
}

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

interface Props {
  cue: CueSummary;
  /** Zero-based position of this row in the visible cue list. */
  cueIndex: number;
  /** Pre-built grid style (display, gridTemplateColumns, gap, padding, minWidth). */
  gridStyle: React.CSSProperties;
  visibleDefs: ColumnDef[];
  isSelected: boolean;
  isAtPlayhead: boolean;
  rowHeight?: number;
  isDragOver?: boolean;
  /** True while this cue is being dragged (dims the row). */
  isDragSource?: boolean;
  /** Nesting depth — 0 = top-level, 1 = inside a group, etc. */
  depth?: number;
  /** True if this is a Group cue. */
  isGroup?: boolean;
  /** Whether the group is currently expanded. */
  isGroupExpanded?: boolean;
  /** Toggle the group's expand/collapse state. */
  onToggleExpand?: (id: string) => void;
  /** True when a cue is being dragged over the middle of this group row (drop-into-group). */
  isGroupDropTarget?: boolean;
  /** ID of the parent group, if this cue is a child. Used for within-group insert detection. */
  parentGroupId?: string | null;
  /** Called on mousedown to start a cue drag operation. */
  onCueDragStart: (id: string, index: number, e: React.MouseEvent) => void;
  /** Called from the narrow left gutter to sweep-select rows without starting a reorder. */
  onSelectionDragStart: (id: string, index: number, e: React.MouseEvent) => void;
  onClick: (id: string, index: number, parentGroupId: string | null, e: React.MouseEvent) => void;
  onDoubleClick: (cue: CueSummary) => void;
  onContextMenu: (id: string, parentGroupId: string | null, e: React.MouseEvent) => void;
  onContinueContextMenu: (id: string, parentGroupId: string | null, e: React.MouseEvent) => void;
  onRefresh?: () => void;
  onSaved?: (cue: CueSummary) => void;
  /** How the cue's colour tag is rendered — left-edge stripe, or the whole row tinted. */
  cueColorStyle?: CueColorStyle;
  outputStatuses?: OutputControlStatus[];
  defaultOutputId?: string | null;
  /** Number children show their timeline offset in the Pre-Wait column. */
  numberTimelineStartMs?: number;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

function CueRowImpl({
  cue,
  cueIndex,
  gridStyle,
  visibleDefs,
  rowHeight = 26,
  isSelected,
  isAtPlayhead,
  isDragOver,
  isDragSource,
  depth = 0,
  isGroup = false,
  isGroupExpanded = false,
  onToggleExpand,
  isGroupDropTarget = false,
  parentGroupId = null,
  onCueDragStart,
  onSelectionDragStart,
  onClick,
  onDoubleClick,
  onContextMenu,
  onContinueContextMenu,
  onRefresh,
  onSaved,
  cueColorStyle = "stripe",
  outputStatuses = [],
  defaultOutputId,
  numberTimelineStartMs,
}: Props) {
  const { locale, t } = useLocale();
  const timing = useTimingStore((s) => s.timings[cue.id]);
  const played = useWorkspaceStore((s) => s.playedCueIds.has(cue.id));
  const outputAssignments = cueOutputAssignments(cue, outputStatuses, defaultOutputId);

  const [editingCell, setEditingCell] = useState<"pre_wait_ms" | "post_wait_ms" | "duration_ms" | "notes" | null>(null);
  const [editingValue, setEditingValue] = useState("");
  const [savedDuration, setSavedDuration] = useState<{ cue: CueSummary; sourceDurationMs: number | null } | null>(null);

  async function commitInlineEdit() {
    if (!editingCell) return;
    await updateCue(cue.id, { [cueNotesProperty(cue)]: editingValue }).catch(console.error);
    onRefresh?.();
    setEditingCell(null);
  }

  function startEditNotes() {
    setEditingCell("notes");
    setEditingValue(cueNotesText(cue));
  }

  function inlineInput(align: "left" | "right" = "right") {
    return (
      <input
        autoFocus
        value={editingValue}
        onChange={(e) => setEditingValue(e.target.value)}
        onFocus={(e) => e.target.select()}
        onBlur={() => void commitInlineEdit()}
        onKeyDown={(e) => {
          if (e.key === "Enter") { e.preventDefault(); void commitInlineEdit(); }
          if (e.key === "Escape") { e.preventDefault(); setEditingCell(null); }
          e.stopPropagation();
        }}
        onClick={(e) => e.stopPropagation()}
        onMouseDown={(e) => e.stopPropagation()}
        style={{ ...INLINE_INPUT_STYLE, textAlign: align }}
      />
    );
  }

  const isRunning  = cue.state === "running";
  const isPaused   = cue.state === "paused";
  const isDisabled = cue.is_disabled ?? false;
  const isBroken   = cue.is_broken ?? false;
  // Keep the existing backend warning (missing/unassigned media, etc.) and
  // surface the same yellow badge for media that is known to be unusually
  // large or high-bitrate.  This uses metadata already present in the row;
  // it does not probe every Cue on every render, and ordinary MP3s remain
  // quiet.
  const mediaWarningIssues = !isBroken
    ? [
      ...((cue.cue_type === "audio" || cue.cue_type === "video") ? assessFileSize(cue).issues : []),
      ...((cue.cue_type === "video" || cue.cue_type === "image") ? assessResolution(cue).issues : []),
    ]
    : [];
  const mediaWarning = mediaWarningIssues.length > 0;
  const compatibilityStatus = cue.media_compatibility_status ?? 0;
  const compatibilityWarning = !isBroken && compatibilityStatus > 0;
  const compatibilityText = compatibilityWarning
    ? (MEDIA_COMPATIBILITY_REASON_TEXT[cue.media_compatibility_reason ?? 12]?.[locale === "ru" ? 0 : 1]
      ?? (compatibilityStatus === 2
        ? t("mediaConversion.compatibilityUnsupported")
        : t("mediaConversion.compatibilityRecommended")))
    : null;
  const isWarning  = (cue.is_warning ?? false) || mediaWarning || compatibilityWarning;
  const mediaWarningText = mediaWarningIssues.length > 0
    ? mediaWarningIssues.map((issue) => {
      const vars = { ...issue.vars };
      if (typeof vars.bitrateMbps === "number") {
        vars.bitrateMbps = new Intl.NumberFormat(locale, {
          minimumFractionDigits: 0,
          maximumFractionDigits: 1,
        }).format(vars.bitrateMbps);
      }
      return t(MEDIA_ISSUE_TRANSLATION_KEYS[issue.code] ?? "mediaConversion.rowWarning", vars);
    }).join(" ")
    : compatibilityText;
  const warningSeverity = compatibilityStatus === 2 ? "#ef4444" : "#eab308";

  const progressPct = durationProgressPercent({
    state: cue.state,
    elapsedMs: timing?.action_elapsed_ms,
    durationMs: cue.duration_ms,
    fileDurationMs: cue.file_duration_ms,
    isEditing: editingCell === "duration_ms",
  });

  const colorAccent = COLOR_SWATCHES[cue.color] ?? "transparent";
  const fullRowTint = cueColorStyle === "full_row" && colorAccent !== "transparent"
    ? hexToRgba(colorAccent, 0.28)
    : null;

  let bg = fullRowTint ?? "transparent";
  if (isDragOver)      bg = "var(--wc-bg-drag-over)";
  // Keep transport state visible underneath selection.  Selection is a
  // separate interaction state (the outline below), so a running/paused cue
  // does not become indistinguishable from an ordinary selected row.
  else if (isRunning)  bg = "var(--wc-bg-running)";
  else if (isPaused)   bg = "var(--wc-bg-paused)";
  else if (isSelected) bg = "var(--wc-accent-dim)";

  // Solid background for sticky-right cells (must be opaque to cover scrolled content).
  const stickyBg = isRunning  ? "var(--wc-bg-running)"
    : isPaused ? "var(--wc-bg-paused)"
    : isSelected ? "var(--wc-accent-dim)"
    : isGroup  ? "var(--wc-bg-group)"
    : "var(--wc-bg-app)";

  // The playhead/running state and the operator's selection are deliberately
  // rendered independently.  The left inset marks the active playhead while
  // the full inset outline marks every selected row (including a multi-select).
  const stateShadow = isAtPlayhead ? "inset 3px 0 0 var(--wc-text-bright)" : null;
  const selectionShadow = isSelected ? "inset 0 0 0 1px var(--wc-accent)" : null;

  const rowStyle: React.CSSProperties = {
    ...gridStyle,
    position: "relative",
    alignItems: "center",
    paddingTop: 2,
    paddingBottom: 2,
    cursor: isDragSource ? "none" : "pointer",
    userSelect: "none",
    background: isDragSource ? "transparent"
      : isGroup && !isSelected ? (bg === "transparent" ? "var(--wc-bg-group)" : bg) : bg,
    borderBottom: isDragSource ? "1px dashed var(--wc-border)"
      : isDragOver ? "1px solid var(--wc-accent)" : "1px solid var(--wc-border)",
    boxShadow: isGroupDropTarget
      ? "inset 0 0 0 2px #22d3ee"
      : isDragOver
      ? "inset 0 0 0 1px var(--wc-accent)"
      : [stateShadow, selectionShadow].filter(Boolean).join(", ") || "none",
    fontSize: 13,
    color: isDisabled ? "var(--wc-text-faint)" : "var(--wc-text)",
    minHeight: rowHeight,
    opacity: isDragSource ? 0.15 : isDisabled ? 0.55 : 1,
    transition: "opacity 0.15s",
  };

  const filename = cueFileName(cue);
  const targetDisplay = formatTargetCues(
    cue.target_cues,
    cue.targets_all,
    t("components.allCues"),
  );

  // Keep the table grid aligned while making the hierarchy visible in the
  // leading identity cells.  The name already reserves this inset in its
  // own content; the type icon and cue number need the same visual offset.
  // Keep the identity cells inside their own grid tracks.  The name column
  // has enough room for the full hierarchy inset; the narrow icon/number
  // cells only need a small inset to communicate nesting.
  const nestedIdentityInset = depth > 0 ? Math.min(8, 4 + (depth - 1) * 2) : 0;

  function mediaInfoCell(kind: "file_size" | "resolution") {
    const assessment = kind === "file_size" ? assessFileSize(cue) : assessResolution(cue);
    const value = kind === "file_size"
      ? formatFileSize(cue.file_size_bytes, locale)
      : formatResolution(cue.media_width, cue.media_height);
    const label = t(kind === "file_size" ? "cueList.fileSize" : "cueList.resolution");
    const reasons = assessment.issues.map((issue) => {
      const vars = { ...issue.vars };
      if (typeof vars.bitrateMbps === "number") {
        vars.bitrateMbps = new Intl.NumberFormat(locale, {
          minimumFractionDigits: 0,
          maximumFractionDigits: 1,
        }).format(vars.bitrateMbps);
      }
      return t(MEDIA_ISSUE_TRANSLATION_KEYS[issue.code] ?? issue.code, vars);
    });
    const description = reasons.length > 0
      ? `${label}: ${value}. ${reasons.join(" ")}`
      : `${label}: ${value}`;

    return (
      <div
        title={description}
        aria-label={description}
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "flex-end",
          width: "100%",
          height: "100%",
          boxSizing: "border-box",
          padding: "0 6px",
          overflow: "hidden",
          whiteSpace: "nowrap",
          textOverflow: "ellipsis",
          fontSize: 12,
          fontVariantNumeric: "tabular-nums",
          color: assessment.problem ? "#fecaca" : "var(--wc-text)",
          background: assessment.problem ? "rgba(239, 68, 68, 0.18)" : "transparent",
          boxShadow: assessment.problem ? "inset 0 0 0 1px rgba(239, 68, 68, 0.65)" : "none",
        }}
      >
        {value}
      </div>
    );
  }

  function durationCell(content: React.ReactNode) {
    return (
      <div style={{ position: "relative", width: "100%", height: "100%", minWidth: 0, overflow: "hidden" }}>
        {progressPct !== null && (
          <div
            aria-hidden="true"
            style={{
              position: "absolute",
              left: 3,
              top: 4,
              bottom: 4,
              width: `calc((100% - 6px) * ${progressPct / 100})`,
              borderRadius: 2,
              background: "rgba(74, 222, 128, 0.28)",
              pointerEvents: "none",
              transition: "width 80ms linear",
            }}
          />
        )}
        <div style={{ position: "relative", zIndex: 1, width: "100%", height: "100%" }}>{content}</div>
      </div>
    );
  }

  const renderCell = (id: string) => {
    switch (id) {
      case "playhead":
        if (isGroup) {
          return (
            <div style={{ position: "relative", width: "100%", height: "100%" }}>
              <GroupExpander
                expanded={isGroupExpanded}
                playhead={isAtPlayhead}
                label={t(isGroupExpanded ? "cueList.collapseGroup" : "cueList.expandGroup", {
                  name: cue.name || t("uiFixes.untitled"),
                })}
                onToggle={() => onToggleExpand?.(cue.id)}
              />
            </div>
          );
        }
        return (
          <div style={{ display: "flex", justifyContent: "flex-start", alignItems: "center", width: "100%", height: "100%", paddingLeft: 6 }}>
            <PlayheadIndicator visible={isAtPlayhead} />
          </div>
        );

      case "led":
        return (
          <div style={{ display: "flex", alignItems: "center", justifyContent: "center", height: "100%" }}>
            {isRunning
              ? <RunningLed />
              : played && (
                <span
                  title={t("cueList.played")}
                  aria-label={t("cueList.played")}
                  style={{
                    width: 14,
                    height: 14,
                    display: "inline-flex",
                    alignItems: "center",
                    justifyContent: "center",
                    border: "1px solid #4ade80",
                    borderRadius: "50%",
                    color: "#4ade80",
                    fontSize: 9,
                    lineHeight: 1,
                    fontWeight: 700,
                    boxSizing: "border-box",
                  }}
                >
                  ✓
                </span>
              )}
          </div>
        );

      case "number":
        return (
          <span style={{
            display: "block",
            fontFamily: "monospace",
            color: "var(--wc-text)",
            paddingLeft: nestedIdentityInset,
            boxSizing: "border-box",
            overflow: "hidden",
            whiteSpace: "nowrap",
          }}>
            {cue.number ?? ""}
          </span>
        );

      case "name":
        return (
          <div style={{ position: "relative", overflow: "hidden", minWidth: 0 }}>
            <span
              style={{
                position: "relative",
                zIndex: 1,
                display: "flex",
                alignItems: "center",
                gap: 5,
                overflow: "hidden",
                paddingLeft: 5 + depth * 20,
              }}
            >
              {isBroken && (
                <span title={cue.error_message ?? t("errors.notFound", { item: t("cueList.cue") })} style={{ color: "#ef4444", flexShrink: 0, fontSize: 11, fontWeight: 700 }}>!</span>
              )}
              {!isBroken && cue.media_metadata_pending && (
                <span
                  title="Анализ медиа…"
                  aria-label="Анализ медиа…"
                  style={{ color: "var(--wc-text-faint)", flexShrink: 0, fontSize: 11 }}
                >◌</span>
              )}
              {isWarning && !isBroken && (
                <span
                  title={cue.warning_message ?? mediaWarningText ?? t("sweepUi.warning")}
                  aria-label={cue.warning_message ?? mediaWarningText ?? t("sweepUi.warning")}
                  style={{ color: warningSeverity, flexShrink: 0, fontSize: 11, fontWeight: 700 }}
                >⚠</span>
              )}
              {isDisabled && (
                <span title={t("common.disabled")} style={{ color: "var(--wc-text-faint)", flexShrink: 0, fontSize: 10 }}>{t("common.disabledShort")}</span>
              )}
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", textDecoration: isDisabled ? "line-through" : "none" }}>
                {cue.name}
                {cue.cue_type === "number" && (cue.number_master_name || cue.number_master_type) && (
                  <span style={{ color: "var(--wc-text-muted)", fontSize: 11, marginLeft: 4 }}>
                    · {cue.number_master_type ? t(`cueTypes.${cue.number_master_type}`) : ""} {cue.number_master_name ?? ""}
                  </span>
                )}
              </span>
            </span>
          </div>
        );

      case "file":
        return (
          <span
            style={{
              display: "block",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              color: "var(--wc-text)",
              fontSize: 12,
              paddingLeft: 5,
            }}
            title={cue.file_path ?? undefined}
          >
            {filename}
          </span>
        );

      case "target":
        return (
          <span
            style={{
              display: "block",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              color: "var(--wc-text)",
              fontSize: 12,
              paddingLeft: 5,
            }}
            title={targetDisplay.title || undefined}
          >
            {targetDisplay.text}
          </span>
        );

      case "file_size":
        return mediaInfoCell("file_size");

      case "resolution":
        return mediaInfoCell("resolution");

      case "output":
        return (
          <span
            style={{
              display: "block",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              color: "var(--wc-text)",
              fontSize: 12,
              paddingLeft: 5,
            }}
          >
            {cue.output_patch_name ?? ""}
          </span>
        );

      case "outputs":
        return (
          <div
            aria-label={t("output.outputs")}
            style={{ display: "flex", alignItems: "center", justifyContent: "center", gap: 5, height: "100%", minWidth: 0 }}
          >
            {outputStatuses.map((output, index) => {
              const lit = outputAssignments[index];
              const color = lit ? "#4ade80" : "var(--wc-text-faint)";
              return (
                <span
                  key={output.output_id}
                  title={output.name}
                  aria-label={`${output.name}: ${lit ? "on" : "off"}`}
                  style={{
                    width: 8,
                    height: 8,
                    borderRadius: "50%",
                    flexShrink: 0,
                    background: color,
                    opacity: lit ? 1 : 0.55,
                    boxShadow: lit ? `0 0 5px ${color}` : "none",
                  }}
                />
              );
            })}
          </div>
        );

      case "type":
        return (
          <span style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            height: "100%",
            paddingLeft: nestedIdentityInset,
            boxSizing: "border-box",
          }}>
            <CueTypeIcon
              type={cue.cue_type}
              size={18}
              tone="neutral"
              label={t(`cueTypes.${cue.cue_type === "midi_file" ? "midiFile" : cue.cue_type}`)}
            />
          </span>
        );

      case "pre_wait":
        if (numberTimelineStartMs != null) {
          return (
            <span
              title="Number timeline position"
              style={{ display: "block", textAlign: "right", color: "var(--wc-text-muted)", fontVariantNumeric: "tabular-nums" }}
            >
              {numberTimelineStartMs ? `${(numberTimelineStartMs / 1000).toFixed(1)}s` : "—"}
            </span>
          );
        }
        return (
          <InlineTimeCell
            cueId={cue.id}
            fieldKey="pre_wait_ms"
            label={t("cueList.editPreWait")}
            isEditing={editingCell === "pre_wait_ms"}
            valueMs={cue.pre_wait_ms}
            emptyValueMs={0}
            displayValue={cue.pre_wait_ms ? `${(cue.pre_wait_ms / 1000).toFixed(1)}s` : "—"}
            onEditingChange={(editing) => setEditingCell((current) =>
              editing ? "pre_wait_ms" : current === "pre_wait_ms" ? null : current)}
            onCommit={(cueId, valueMs) => updateCue(cueId, { pre_wait_ms: valueMs ?? 0 })}
            onFailure={onRefresh}
          />
        );

      case "duration": {
        const canEdit = canEditCueDuration(cue.cue_type);
        const shownDurationMs = savedDuration?.cue.id === cue.id && cue.duration_ms === savedDuration.sourceDurationMs
          ? savedDuration.cue.duration_ms
          : cue.duration_ms;
        const displayDuration = cue.is_loading
          ? t("sweepUi.warningLoading")
          : shownDurationMs == null
            ? cue.cue_type === "image" || cue.cue_type === "text"
              ? t("cueList.indefiniteDuration")
              : ""
            : formatDurationMs(shownDurationMs);
        if (canEdit) {
          return (
            durationCell(<InlineTimeCell
              cueId={cue.id}
              fieldKey="duration_ms"
              label={t("cueList.editDuration")}
              isEditing={editingCell === "duration_ms"}
              transparentDisplayBackground={progressPct !== null}
              valueMs={shownDurationMs}
              emptyValueMs={emptyCueDurationValue(cue.cue_type)}
              displayValue={displayDuration}
              onEditingChange={(editing) => setEditingCell((current) =>
                editing ? "duration_ms" : current === "duration_ms" ? null : current)}
              onCommit={(cueId, valueMs) => setCueDuration(cueId, valueMs)}
              onSaved={(savedCue) => {
                setSavedDuration({ cue: savedCue, sourceDurationMs: cue.duration_ms });
                onSaved?.(savedCue);
              }}
              onFailure={onRefresh}
            />)
          );
        }
        return durationCell(
          <div
            style={{ display: "flex", alignItems: "center", justifyContent: "flex-end", height: "100%", paddingRight: 8, cursor: "default", color: cue.is_loading ? "#f59e0b" : "var(--wc-text)", fontSize: 12 }}
          >
            {cue.is_loading
              ? t("sweepUi.warningLoading")
              : formatDurationMs(cue.duration_ms)}
          </div>
        );
      }

      case "post_wait":
        return (
          <InlineTimeCell
            cueId={cue.id}
            fieldKey="post_wait_ms"
            label={t("cueList.editPostWait")}
            isEditing={editingCell === "post_wait_ms"}
            valueMs={cue.post_wait_ms}
            emptyValueMs={0}
            displayValue={cue.post_wait_ms ? `${(cue.post_wait_ms / 1000).toFixed(1)}s` : "—"}
            onEditingChange={(editing) => setEditingCell((current) =>
              editing ? "post_wait_ms" : current === "post_wait_ms" ? null : current)}
            onCommit={(cueId, valueMs) => updateCue(cueId, { post_wait_ms: valueMs ?? 0 })}
            onFailure={onRefresh}
          />
        );

      case "continue": {
        const label = t(cue.continue_mode === "auto_continue"
          ? "inspector.autoContinue"
          : cue.continue_mode === "auto_follow"
            ? "inspector.autoFollow"
            : "inspector.doNotContinue");
        return (
          <div
            role="img"
            aria-label={label}
            title={label}
            onContextMenu={(e) => {
              e.preventDefault();
              e.stopPropagation();
              onContinueContextMenu(cue.id, parentGroupId ?? null, e);
            }}
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              width: "100%",
              height: "100%",
            }}
          >
            <ContinueModeIcon mode={cue.continue_mode} />
          </div>
        );
      }

      case "notes": {
        if (editingCell === "notes") return inlineInput("left");
        const notesText = cueNotesText(cue);
        return (
          <div
            title={notesText || undefined}
            onDoubleClick={(e) => { e.stopPropagation(); startEditNotes(); }}
            style={{
              display: "flex", alignItems: "center",
              height: "100%", paddingLeft: 5,
              overflow: "hidden", cursor: "text",
              color: "var(--wc-text)", fontSize: 12,
            }}
          >
            <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {notesText}
            </span>
          </div>
        );
      }

      default:
        return null;
    }
  };

  return (
    <div
      style={rowStyle}
      tabIndex={-1}
      role="row"
      aria-selected={isSelected}
      data-cue-id={cue.id}
      data-cue-index={cueIndex}
      data-is-group={isGroup ? "true" : undefined}
      data-cue-depth={depth}
      data-parent-group-id={parentGroupId ?? undefined}
      onMouseDown={(e) => onCueDragStart(cue.id, cueIndex, e)}
      onClick={(e) => onClick(cue.id, cueIndex, parentGroupId ?? null, e)}
      onDoubleClick={() => onDoubleClick(cue)}
      onContextMenu={(e) => onContextMenu(cue.id, parentGroupId ?? null, e)}
    >
      {/* The left gutter is an unambiguous mouse-selection affordance.  Row
          dragging remains available from the rest of the row, so a sweep can
          never accidentally reorder cues. */}
      <div
        aria-label={locale === "ru" ? "Потяните для выделения диапазона cue" : "Drag to select a cue range"}
        title={locale === "ru" ? "Потяните для выделения диапазона" : "Drag to select a range"}
        data-selection-gutter="true"
        onMouseDown={(e) => {
          e.stopPropagation();
          onSelectionDragStart(cue.id, cueIndex, e);
        }}
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          bottom: 0,
          width: 8,
          cursor: "crosshair",
          zIndex: 5,
        }}
      >
        {/* A quiet three-dot handle keeps the gesture discoverable without
            covering the playhead cell or the cue-colour stripe. */}
        <span
          aria-hidden="true"
          style={{
            position: "absolute",
            left: 5,
            top: "50%",
            width: 2,
            height: 2,
            borderRadius: "50%",
            background: "var(--wc-text-faint)",
            boxShadow: "0 -4px 0 var(--wc-text-faint), 0 4px 0 var(--wc-text-faint)",
            opacity: 0.8,
            transform: "translateY(-50%)",
          }}
        />
      </div>
      {/* Color indicator strip — shifts right with nesting depth (4 px per level).
          z-index 0 keeps it below column content (playhead indicator, etc.).
          Always drawn — "full row" adds a background tint on top of this, it
          doesn't replace it. */}
      <div
        style={{
          position: "absolute",
          left: depth * 4,
          top: 0,
          bottom: 0,
          width: 4,
          background: colorAccent,
          pointerEvents: "none",
          zIndex: 0,
        }}
      />
      {visibleDefs.map((col) => (
        <div
          key={col.id}
          style={col.stickyRight ? {
            position: "sticky",
            right: 0,
            zIndex: 2,
            background: stickyBg,
            boxShadow: "-4px 0 8px rgba(0,0,0,0.18)",
            alignSelf: "stretch",
          } : {
            minWidth: 0,
            overflow: col.id === "playhead" && isGroup ? "visible" : "hidden",
            position: "relative",
            zIndex: col.id === "playhead" && isGroup ? 3 : 1,
            alignSelf: "stretch",
          }}
        >
          {renderCell(col.id)}
        </div>
      ))}
    </div>
  );
}

// Memoized: with stable props from CueListView (all callbacks + gridStyle are
// stable), a large cue list only re-renders the rows whose data actually
// changed, instead of all of them on every state/selection change.
export const CueRow = memo(CueRowImpl);
