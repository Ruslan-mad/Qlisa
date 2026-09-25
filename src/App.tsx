// Root application layout — mirrors QLab's three-zone layout.

import { useEffect, useState, useCallback, useRef } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { emitTo, listen } from "@tauri-apps/api/event";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { CueListView } from "./components/CueList/CueListView";
import { ActionMenu } from "./components/CueList/ActionMenu";
import { EditMenu } from "./components/MenuBar/EditMenu";
import { CartView } from "./components/CueList/CartView";
import { ShowModeView } from "./components/ShowMode/ShowModeView";
import { ActiveCuesView } from "./components/ActiveCues/ActiveCuesView";
import { migrateRightPanelMode, toggleRightPanel, type RightPanelMode, flattenActiveCues } from "./components/ActiveCues/activeCueModel";
import { CueListTabs } from "./components/CueList/CueListTabs";
import { InspectorPanel } from "./components/Inspector/InspectorPanel";
import { ClipEditorDock } from "./components/Editor/ClipEditorDock";
import { CurveEditorDock } from "./components/Curve/CurveEditorDock";
import { TransportBar } from "./components/Transport/TransportBar";
import { FullscreenControl } from "./components/Transport/FullscreenControl";
import { useTauriEvents } from "./hooks/useTauriEvents";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { useWorkspaceStore } from "./stores/workspaceStore";
import { addCue, collectAndSave, importQlabWorkspace, saveWorkspace, loadWorkspace, newWorkspace, drainPendingProjectOpens, getWorkspaceInfo, setPlayhead, toggleOutputWindow, getOutputWindowVisible, openPreferencesWindow, openDiagnosticsWindow, openOutputMonitorWindow, getCue, getCueLists, checkRecovery, restoreRecovery, discardRecovery, updateDisplayPreferences } from "./lib/commands";
import { AboutDialog } from "./components/About/AboutDialog";
import { UpdateDialog } from "./components/Update/UpdateDialog";
import { useUpdateStore } from "./stores/updateStore";
import { InkueMark } from "./components/common/InkueMark";
import { PreflightModal } from "./components/Preflight/PreflightModal";
import { LogViewerModal } from "./components/Logs/LogViewerModal";
import { HealthBanner } from "./components/Health/HealthBanner";
import type { CollectReport, ImportReport, RecoveryInfo, CueType, NumberCueData } from "./lib/types";
import { QlabImportDialog } from "./components/Import/QlabImportDialog";
import type { CueSummary } from "./lib/types";
import { useLocale } from "./i18n";
import { CLIP_EDITOR_TAB_LABELS, clipEditorDockVisible, normalizeClipEditorVisibility, resolveClipEditorTargetCueId, toggleClipEditorPanel } from "./lib/clipEditorPrefs";
import { hasActivePlayback } from "./lib/closeGuard";
import { normalizeNumberCueData } from "./components/Inspector/numberModel";
import { resolveMonitorPreviewSelection } from "./components/Inspector/numberPreviewSelection";
import { useNumberPreviewStore } from "./stores/numberPreviewStore";
import { CueToolbar } from "./components/CueToolbar/CueToolbar";
import { inspectWorkspaceGuard, isProjectFilePath, resolveWorkspaceGuard, withDefaultProjectExtension } from "./lib/projectFile";

// ---------------------------------------------------------------------------
// Recent files
// ---------------------------------------------------------------------------

const RECENT_FILES_KEY = "inkue_recent_files";
const MAX_RECENT = 8;

function loadRecentFiles(): string[] {
  try { return JSON.parse(localStorage.getItem(RECENT_FILES_KEY) ?? "[]") as string[]; }
  catch { return []; }
}

function pushRecentFile(path: string): string[] {
  const list = loadRecentFiles().filter((p) => p !== path);
  list.unshift(path);
  const trimmed = list.slice(0, MAX_RECENT);
  try { localStorage.setItem(RECENT_FILES_KEY, JSON.stringify(trimmed)); } catch { /* ignore */ }
  return trimmed;
}

// ---------------------------------------------------------------------------
// Window control buttons (minimize / maximize / close)
// ---------------------------------------------------------------------------

function WindowControls() {
  const { t } = useLocale();
  const [hovered, setHovered] = useState<"min" | "max" | "close" | null>(null);

  const handleMin   = () => void getCurrentWindow().minimize();
  const handleMax   = () => void getCurrentWindow().toggleMaximize();
  // Close goes through the normal close path so onCloseRequested fires.
  const handleClose = () => void getCurrentWindow().close();

  const btn = (
    key: "min" | "max" | "close",
    label: string,
    color: string,
    hoverColor: string,
    onClick: () => void,
  ) => (
    <button
      key={key}
      title={label}
      onClick={onClick}
      onMouseEnter={() => setHovered(key)}
      onMouseLeave={() => setHovered(null)}
      style={{
        width: 13, height: 13, borderRadius: "50%", border: "none",
        background: hovered === key ? hoverColor : color,
        cursor: "pointer", display: "flex", alignItems: "center",
        justifyContent: "center", padding: 0, flexShrink: 0,
        fontSize: 8,
        color: hovered === key ? "rgba(0,0,0,0.6)" : "transparent",
        transition: "background 0.1s",
      }}
    >
      {hovered === key ? (key === "close" ? "✕" : key === "min" ? "–" : "▢") : ""}
    </button>
  );

  return (
    <div style={{ display: "flex", gap: 7, alignItems: "center", flexShrink: 0 }}>
      {btn("close", t("window.close"), "#ef4444", "#dc2626", handleClose)}
      {btn("min",   t("window.minimize"), "#f59e0b", "#d97706", handleMin)}
      {btn("max",   t("window.maximize"), "#22c55e", "#16a34a", handleMax)}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Unsaved-changes close confirmation dialog
// ---------------------------------------------------------------------------

function CloseConfirmDialog({
  titleKey = "dialogs.unsavedChanges",
  messageKey = "dialogs.closeMessage",
  onSave,
  onDiscard,
  onCancel,
}: {
  titleKey?: "dialogs.unsavedChanges" | "dialogs.workspaceChangeTitle";
  messageKey?: "dialogs.closeMessage" | "dialogs.workspaceChangeMessage";
  onSave: () => void;
  onDiscard: () => void;
  onCancel: () => void;
}) {
  const { t } = useLocale();
  return (
    <div
      style={{
        position: "fixed", inset: 0, zIndex: 99999,
        background: "rgba(0,0,0,0.6)",
        display: "flex", alignItems: "center", justifyContent: "center",
      }}
    >
      <div
        style={{
          background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
          borderRadius: 10, padding: "28px 32px", width: 360,
          boxShadow: "0 16px 48px rgba(0,0,0,0.8)",
        }}
      >
        <div style={{ fontSize: 15, fontWeight: 600, color: "var(--wc-text-bright)", marginBottom: 8 }}>
          {t(titleKey)}
        </div>
        <div style={{ fontSize: 13, color: "var(--wc-text-secondary)", marginBottom: 24 }}>
          {t(messageKey)}
        </div>
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
          <DialogBtn label={t("common.cancel")} onClick={onCancel} />
          <DialogBtn label={t("common.discard")} onClick={onDiscard} danger />
          <DialogBtn label={t("common.save")} onClick={onSave} primary />
        </div>
      </div>
    </div>
  );
}

/** Closing while a cue is live is destructive even when the workspace is
 * otherwise saved. Keep this separate from the unsaved-work prompt so that
 * continuing can still honour the existing Save/Discard flow. */
function ActivePlaybackCloseDialog({
  onContinue,
  onCancel,
}: {
  onContinue: () => void;
  onCancel: () => void;
}) {
  const { t } = useLocale();
  return (
    <div
      style={{
        position: "fixed", inset: 0, zIndex: 100000,
        background: "rgba(0,0,0,0.6)",
        display: "flex", alignItems: "center", justifyContent: "center",
      }}
    >
      <div
        role="alertdialog"
        aria-modal="true"
        style={{
          background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
          borderRadius: 10, padding: "28px 32px", width: 420,
          boxShadow: "0 16px 48px rgba(0,0,0,0.8)",
        }}
      >
        <div style={{ fontSize: 15, fontWeight: 600, color: "var(--wc-text-bright)", marginBottom: 8 }}>
          {t("dialogs.activePlaybackTitle")}
        </div>
        <div style={{ fontSize: 13, color: "var(--wc-text-secondary)", marginBottom: 24 }}>
          {t("dialogs.activePlaybackMessage")}
        </div>
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
          <DialogBtn label={t("common.cancel")} onClick={onCancel} />
          <DialogBtn label={t("common.close")} onClick={onContinue} danger />
        </div>
      </div>
    </div>
  );
}

function DialogBtn({
  label, onClick, primary, danger,
}: {
  label: string; onClick: () => void; primary?: boolean; danger?: boolean;
}) {
  const [hov, setHov] = useState(false);
  const bg = primary
    ? hov ? "var(--wc-accent-hover)" : "var(--wc-accent)"
    : danger
      ? hov ? "#dc2626" : "#b91c1c"
      : hov ? "var(--wc-bg-hover)" : "var(--wc-bg-surface)";
  const color = primary ? "var(--wc-accent-fg)" : danger ? "#fff" : "var(--wc-text)";
  return (
    <button
      onClick={onClick}
      onMouseEnter={() => setHov(true)}
      onMouseLeave={() => setHov(false)}
      style={{
        padding: "6px 16px", border: "1px solid var(--wc-border-strong)", borderRadius: 6,
        background: bg, color, fontSize: 13, cursor: "pointer",
      }}
    >
      {label}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Crash-recovery dialog — shown on startup when a recovery snapshot exists
// ---------------------------------------------------------------------------

function RecoveryDialog({
  info,
  onRecover,
  onDiscard,
}: {
  info: RecoveryInfo;
  onRecover: () => void;
  onDiscard: () => void;
}) {
  const { t, locale } = useLocale();
  const label = info.name || t("app.untitled");
  const when = info.modified_at
    ? new Date(info.modified_at).toLocaleString(locale === "ru" ? "ru-RU" : "en-US")
    : t("common.unknown").toLowerCase();

  return (
    <div
      style={{
        position: "fixed", inset: 0, zIndex: 99999,
        background: "rgba(0,0,0,0.6)",
        display: "flex", alignItems: "center", justifyContent: "center",
      }}
    >
      <div
        style={{
          background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
          borderRadius: 10, padding: "28px 32px", width: 380,
          boxShadow: "0 16px 48px rgba(0,0,0,0.8)",
        }}
      >
        <div style={{ fontSize: 15, fontWeight: 600, color: "var(--wc-text-bright)", marginBottom: 8 }}>
          {t("dialogs.recoverTitle")}
        </div>
        <div style={{ fontSize: 13, color: "var(--wc-text-secondary)", marginBottom: 6 }}>
          {t("dialogs.recoverMessage", { time: when })}
        </div>
        <div style={{ fontSize: 13, color: "var(--wc-text)", marginBottom: 24 }}>
          {t("dialogs.recoveryPrompt", { name: label, time: when })}
        </div>
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
          <DialogBtn label={t("common.discard")} onClick={onDiscard} danger />
          <DialogBtn label={t("common.confirm")} onClick={onRecover} primary />
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Goto cue dialog (G key) — type a cue number to move the playhead
// ---------------------------------------------------------------------------

function GotoDialog({
  onClose,
  onRefresh,
}: {
  onClose: () => void;
  onRefresh: () => void;
}) {
  const { t } = useLocale();
  const [value, setValue] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const commit = async () => {
    const q = value.trim();
    if (!q) { onClose(); return; }
    const { cues } = useWorkspaceStore.getState();
    const match = cues.find((c) => c.number != null && c.number === q);
    if (match) {
      await setPlayhead(match.id).catch(console.error);
      onRefresh();
    }
    onClose();
  };

  const handleKey = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") { e.preventDefault(); void commit(); }
    if (e.key === "Escape") { e.preventDefault(); onClose(); }
  };

  return (
    <div
      style={{
        position: "fixed", inset: 0, zIndex: 99998,
        display: "flex", alignItems: "center", justifyContent: "center",
      }}
      onClick={onClose}
    >
      <div
        style={{
          background: "var(--wc-bg-surface)", border: "1px solid var(--wc-text-faint)",
          borderRadius: 8, padding: "16px 20px", width: 280,
          boxShadow: "0 12px 40px rgba(0,0,0,0.8)",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <div style={{ fontSize: 12, color: "var(--wc-text-secondary)", marginBottom: 8 }}>
          {t("dialogs.gotoTitle")}
        </div>
        <input
          ref={inputRef}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={handleKey}
          placeholder={t("dialogs.gotoPrompt")}
          style={{
            width: "100%", boxSizing: "border-box",
            background: "var(--wc-bg-app)", border: "1px solid var(--wc-border-strong)",
            borderRadius: 5, color: "var(--wc-text-bright)", fontSize: 14,
            padding: "7px 10px", outline: "none",
          }}
        />
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginTop: 8 }}>
          {t("common.confirm")} · {t("common.cancel")}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// File menu
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Collect & Save result dialog
// ---------------------------------------------------------------------------

function CollectResultDialog({
  report,
  onClose,
}: {
  report: CollectReport;
  onClose: () => void;
}) {
  const { t } = useLocale();
  const hasMissing = report.files_missing.length > 0;
  return (
    <div
      style={{
        position: "fixed", inset: 0, zIndex: 99999,
        background: "rgba(0,0,0,0.6)", display: "flex",
        alignItems: "center", justifyContent: "center",
      }}
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
          borderRadius: 10, padding: "24px 28px", maxWidth: 520, width: "90%",
          boxShadow: "0 16px 48px rgba(0,0,0,0.8)",
        }}
      >
        <h3 style={{ margin: "0 0 16px", fontSize: 15, color: "var(--wc-text)" }}>
          {t("dialogs.collectTitle")} — {t("common.done")}
        </h3>

        <div style={{ fontSize: 12, color: "var(--wc-text-muted)", marginBottom: 14, fontFamily: "monospace", wordBreak: "break-all" }}>
          {report.workspace_path}
        </div>

        <div style={{ display: "flex", flexDirection: "column", gap: 6, fontSize: 13, marginBottom: 16 }}>
          <div style={{ color: "var(--wc-text)" }}>
            <span style={{ color: "#4ade80" }}>✓</span>{" "}
            {t("dialogs.filesCopied", { count: report.files_copied })}
          </div>
          {report.files_skipped > 0 && (
            <div style={{ color: "var(--wc-text-muted)" }}>
              — {t("dialogs.filesSkipped", { count: report.files_skipped })}
            </div>
          )}
          {hasMissing && (
            <div style={{ color: "#f87171" }}>
              ⚠ {t("dialogs.filesMissing", { count: report.files_missing.length })}
            </div>
          )}
        </div>

        {hasMissing && (
          <div style={{
            background: "rgba(239,68,68,0.08)", border: "1px solid rgba(239,68,68,0.3)",
            borderRadius: 6, padding: "8px 12px", marginBottom: 16,
            maxHeight: 140, overflowY: "auto",
          }}>
            {report.files_missing.map((p) => (
              <div key={p} style={{ fontSize: 11, color: "#fca5a5", fontFamily: "monospace", wordBreak: "break-all", marginBottom: 4 }}>
                {p}
              </div>
            ))}
          </div>
        )}

        <div style={{ display: "flex", justifyContent: "flex-end" }}>
          <button
            onClick={onClose}
            style={{
              background: "var(--wc-bg-hover)", border: "1px solid var(--wc-border-strong)",
              borderRadius: 6, color: "var(--wc-text)", cursor: "pointer",
              fontSize: 13, padding: "6px 18px",
            }}
          >
            {t("common.ok")}
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// File menu
// ---------------------------------------------------------------------------

function FileMenu({
  onSave,
  onSaveAs,
  onOpen,
  onNew,
  onImportQlab,
  onCollect,
  onCheck,
  onLogs,
  onDiagnostics,
  onPreferences,
  onAbout,
  recentFiles,
  onOpenRecent,
}: {
  onSave: () => void;
  onSaveAs: () => void;
  onOpen: () => void;
  onNew: () => void;
  onImportQlab: () => void;
  onCollect: () => void;
  onCheck: () => void;
  onLogs: () => void;
  onDiagnostics: () => void;
  onPreferences: () => void;
  onAbout: () => void;
  recentFiles: string[];
  onOpenRecent: (path: string) => void;
}) {
  const { t } = useLocale();
  const [open, setOpen] = useState(false);
  const [hovered, setHovered] = useState<string | null>(null);

  const close = () => setOpen(false);

  const act = (fn: () => void) => () => { close(); fn(); };

  const handleQuit = () => {
    close();
    void getCurrentWindow().close();
  };

  const menuItems: Array<
    | { type: "item"; label: string; shortcut?: string; action: () => void; muted?: boolean }
    | { type: "separator" }
  > = [
    { type: "item", label: t("actions.newWorkspace"), shortcut: "Ctrl+N", action: act(onNew) },
    { type: "item", label: t("actions.openWorkspace"), shortcut: "Ctrl+O", action: act(onOpen) },
    ...(recentFiles.length > 0
      ? [
          { type: "separator" as const },
          ...recentFiles.map((p) => ({
            type: "item" as const,
            label: p.split(/[\\/]/).pop() ?? p,
            action: act(() => onOpenRecent(p)),
            muted: true,
          })),
        ]
      : []),
    { type: "separator" },
    { type: "item", label: t("actions.saveWorkspace"), shortcut: "Ctrl+S", action: act(onSave) },
    { type: "item", label: t("actions.saveWorkspaceAs"), shortcut: "Ctrl+Shift+S", action: act(onSaveAs) },
    { type: "item", label: t("actions.collectAndSave"), action: act(onCollect) },
    { type: "separator" },
    { type: "item", label: t("actions.importQlab"), action: act(onImportQlab) },
    { type: "separator" },
    { type: "item", label: t("actions.checkWorkspace"), action: act(onCheck) },
    { type: "item", label: t("actions.logs"), action: act(onLogs) },
    { type: "item", label: "Диагностика", action: act(onDiagnostics) },
    { type: "separator" },
    { type: "item", label: t("app.preferences"), shortcut: "Ctrl+,", action: act(onPreferences) },
    { type: "separator" },
    { type: "item", label: t("app.about"), action: act(onAbout) },
    { type: "separator" },
    { type: "item", label: t("actions.quit"), action: handleQuit },
  ];

  return (
    <div style={{ position: "relative", flexShrink: 0 }}>
      {open && (
        <div style={{ position: "fixed", inset: 0, zIndex: 9990 }} onClick={close} />
      )}
      <button
        onClick={(e) => { e.stopPropagation(); setOpen((v) => !v); }}
        style={{
          background: open ? "var(--wc-bg-surface)" : "transparent",
          border: "none", color: "var(--wc-text)", cursor: "pointer",
          fontSize: 12, padding: "3px 8px", borderRadius: 4, userSelect: "none",
        }}
      >
        {t("menus.file")}
      </button>
      {open && (
        <div
          style={{
            position: "absolute", left: 0, top: "100%", marginTop: 2,
            background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", borderRadius: 6,
            padding: "4px 0", minWidth: 220,
            boxShadow: "0 8px 24px rgba(0,0,0,0.7)", zIndex: 9999,
          }}
        >
          {menuItems.map((item, i) =>
            item.type === "separator" ? (
              <div key={i} style={{ height: 1, background: "var(--wc-border-strong)", margin: "4px 0" }} />
            ) : (
              <button
                key={item.label}
                onMouseEnter={() => setHovered(item.label)}
                onMouseLeave={() => setHovered(null)}
                onClick={(e) => { e.stopPropagation(); item.action(); }}
                style={{
                  display: "flex", alignItems: "center", justifyContent: "space-between",
                  width: "100%", padding: "6px 14px",
                  background: hovered === item.label ? "var(--wc-bg-hover)" : "transparent",
                  border: "none",
                  color: item.muted ? "var(--wc-text-secondary)" : "var(--wc-text)",
                  fontSize: 13, cursor: "pointer", textAlign: "left", gap: 24,
                }}
              >
                <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", maxWidth: 180 }}>
                  {item.label}
                </span>
                {item.shortcut && (
                  <span style={{ color: "var(--wc-text-muted)", fontSize: 11, flexShrink: 0 }}>{item.shortcut}</span>
                )}
              </button>
            )
          )}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// View menu
// ---------------------------------------------------------------------------

interface ViewMenuItem {
  label: string;
  checked: boolean;
  onClick: () => void;
  shortcut?: string;
}

function ViewMenu({ items }: { items: ViewMenuItem[] }) {
  const { t } = useLocale();
  const [open, setOpen] = useState(false);
  const [hovered, setHovered] = useState<string | null>(null);

  const close = () => setOpen(false);

  return (
    <div style={{ position: "relative", flexShrink: 0 }}>
      {open && (
        <div style={{ position: "fixed", inset: 0, zIndex: 9990 }} onClick={close} />
      )}
      <button
        onClick={(e) => { e.stopPropagation(); setOpen((v) => !v); }}
        style={{
          background: open ? "var(--wc-bg-surface)" : "transparent",
          border: "none", color: "var(--wc-text)", cursor: "pointer",
          fontSize: 12, padding: "3px 8px", borderRadius: 4, userSelect: "none",
        }}
      >
        {t("menus.view")}
      </button>
      {open && (
        <div
          style={{
            position: "absolute", left: 0, top: "100%", marginTop: 2,
            background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", borderRadius: 6,
            padding: "4px 0", minWidth: 200,
            boxShadow: "0 8px 24px rgba(0,0,0,0.7)", zIndex: 9999,
          }}
        >
          {items.map((item) => (
            <button
              key={item.label}
              onMouseEnter={() => setHovered(item.label)}
              onMouseLeave={() => setHovered(null)}
              onClick={(e) => { e.stopPropagation(); close(); item.onClick(); }}
              style={{
                display: "flex", alignItems: "center", gap: 8,
                width: "100%", padding: "6px 14px",
                background: hovered === item.label ? "var(--wc-bg-hover)" : "transparent",
                border: "none", color: "var(--wc-text)", fontSize: 13,
                cursor: "pointer", textAlign: "left",
              }}
            >
              <span style={{ width: 14, textAlign: "center", color: "var(--wc-text-secondary)" }}>
                {item.checked ? "✓" : ""}
              </span>
              <span style={{ flex: 1 }}>{item.label}</span>
              {item.shortcut && (
                <span style={{ color: "var(--wc-text-muted)", fontSize: 11 }}>{item.shortcut}</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function HelpMenu({ onCheck, onAbout }: { onCheck: () => void; onAbout: () => void }) {
  const { t } = useLocale();
  const [open, setOpen] = useState(false);
  return (
    <div style={{ position: "relative", flexShrink: 0 }}>
      <button onClick={() => setOpen((value) => !value)} style={{ background: open ? "var(--wc-bg-surface)" : "transparent", border: "none", color: "var(--wc-text)", cursor: "pointer", fontSize: 12, padding: "3px 8px", borderRadius: 4 }}>{t("menus.help")}</button>
      {open && <>
        <div style={{ position: "fixed", inset: 0, zIndex: 9990 }} onClick={() => setOpen(false)} />
        <div style={{ position: "absolute", left: 0, top: "100%", marginTop: 2, background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", borderRadius: 6, padding: 4, minWidth: 190, boxShadow: "0 8px 24px rgba(0,0,0,0.7)", zIndex: 9999 }}>
          <button onClick={() => { setOpen(false); onCheck(); }} style={{ display: "block", width: "100%", background: "transparent", border: 0, color: "var(--wc-text)", cursor: "pointer", textAlign: "left", padding: "7px 10px", fontSize: 13 }}>{t("systemUi.checkUpdates")}</button>
          <button onClick={() => { setOpen(false); onAbout(); }} style={{ display: "block", width: "100%", background: "transparent", border: 0, color: "var(--wc-text)", cursor: "pointer", textAlign: "left", padding: "7px 10px", fontSize: 13 }}>{t("app.about")}</button>
        </div>
      </>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Persisted UI layout (panel visibility) — mirrors the column-config pattern.
// ---------------------------------------------------------------------------

const LS_LAYOUT_KEY = "inkue_ui_layout";

interface UiLayout {
  showCueListTabs: boolean;
  rightPanel: RightPanelMode;
  showSearchBar: boolean;
  inspectorWidth: number;
  showLivePanel: boolean;
  showSlicePanel: boolean;
  activeClipTab: "Live" | "Slice";
}

const INSPECTOR_MIN_WIDTH = 320;
const INSPECTOR_MAX_WIDTH = 560;
const INSPECTOR_DEFAULT_WIDTH = 360;

const clampInspectorWidth = (w: number) =>
  Math.min(INSPECTOR_MAX_WIDTH, Math.max(INSPECTOR_MIN_WIDTH, w));

const DEFAULT_UI_LAYOUT: UiLayout = {
  showCueListTabs: true,
  rightPanel: "inspector",
  showSearchBar: true,
  inspectorWidth: INSPECTOR_DEFAULT_WIDTH,
  showLivePanel: true,
  showSlicePanel: true,
  activeClipTab: "Live",
};

function loadUiLayout(): UiLayout {
  try {
    const raw = localStorage.getItem(LS_LAYOUT_KEY);
    if (!raw) return DEFAULT_UI_LAYOUT;
    const parsed = JSON.parse(raw) as Partial<UiLayout>;
    return {
      showCueListTabs: parsed.showCueListTabs ?? true,
      rightPanel: migrateRightPanelMode((parsed as Partial<UiLayout>).rightPanel, (parsed as { inspectorOpen?: boolean }).inspectorOpen),
      showSearchBar: parsed.showSearchBar ?? true,
      inspectorWidth: clampInspectorWidth(parsed.inspectorWidth ?? INSPECTOR_DEFAULT_WIDTH),
      showLivePanel: parsed.showLivePanel ?? true,
      showSlicePanel: parsed.showSlicePanel ?? true,
      activeClipTab: parsed.activeClipTab === "Slice" ? "Slice" : "Live",
    };
  } catch {
    return DEFAULT_UI_LAYOUT;
  }
}

function saveUiLayout(layout: UiLayout): void {
  try {
    localStorage.setItem(LS_LAYOUT_KEY, JSON.stringify(layout));
  } catch {
    // ignore (private / storage-full)
  }
}

// ---------------------------------------------------------------------------
// Search results overlay
// ---------------------------------------------------------------------------

function flattenCues(cues: CueSummary[]): CueSummary[] {
  return cues.flatMap((c) => [c, ...(c.children ? flattenCues(c.children) : [])]);
}

function SearchResults({
  query,
  allCues,
  onSelect,
}: {
  query: string;
  allCues: CueSummary[];
  onSelect: (id: string) => void;
}) {
  const { t } = useLocale();
  const q = query.toLowerCase();
  const matches = flattenCues(allCues).filter(
    (c) =>
      c.name?.toLowerCase().includes(q) ||
      (c.number ?? "").toLowerCase().includes(q),
  );

  if (matches.length === 0) {
    return (
      <div style={{ padding: "16px", fontSize: 12, color: "var(--wc-text-faint)", textAlign: "center" }}>
        {t("cueList.noMatches")} ({query})
      </div>
    );
  }

  return (
    <div style={{ flex: 1, overflowY: "auto" }}>
      {matches.map((cue) => (
        <button
          key={cue.id}
          onClick={() => onSelect(cue.id)}
          style={{
            display: "flex", alignItems: "center", gap: 10,
            width: "100%", padding: "7px 12px",
            background: "transparent", border: "none",
            borderBottom: "1px solid var(--wc-border)",
            color: "var(--wc-text)", cursor: "pointer", textAlign: "left",
          }}
          onMouseEnter={(e) => { (e.currentTarget as HTMLElement).style.background = "var(--wc-bg-hover)"; }}
          onMouseLeave={(e) => { (e.currentTarget as HTMLElement).style.background = "transparent"; }}
        >
          {cue.color && (
            <div style={{ width: 3, height: 22, borderRadius: 2, background: cue.color, flexShrink: 0 }} />
          )}
          <span style={{ fontSize: 11, color: "var(--wc-text-muted)", width: 36, flexShrink: 0, fontFamily: "monospace" }}>
            {cue.number ?? ""}
          </span>
          <span style={{ fontSize: 13, flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {cue.name || <span style={{ color: "var(--wc-text-faint)" }}>{t("app.unnamed")}</span>}
          </span>
          <span style={{ fontSize: 11, color: "var(--wc-text-faint)", flexShrink: 0 }}>
            {t(`cueTypes.${cue.cue_type === "midi_file" ? "midiFile" : cue.cue_type}`)}
          </span>
        </button>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Root component
// ---------------------------------------------------------------------------

function findCueRecursive(cues: CueSummary[], id: string | null): CueSummary | undefined {
  if (!id) return undefined;
  for (const cue of cues) {
    if (cue.id === id) return cue;
    if (cue.children) {
      const found = findCueRecursive(cue.children, id);
      if (found) return found;
    }
  }
  return undefined;
}

function findNumberOwner(cues: CueSummary[], targetId: string | null): string | null {
  if (!targetId) return null;
  for (const cue of cues) {
    if (cue.cue_type === "number" && cue.children?.some((child) => child.id === targetId)) return cue.id;
    const nested = findNumberOwner(cue.children ?? [], targetId);
    if (nested) return nested;
  }
  return null;
}

export default function App() {
  const { t } = useLocale();
  const { refreshCues, refreshWorkspaceInfo, refreshValidation, refreshHealth, brokenCueIds, loadGeneralPrefs, loadDisplayPrefs, displayPrefs, workspaceInfo, selectedCueId, selectedCueIds, cues, cueLists, activeCueListId } =
    useWorkspaceStore();

  const [rightPanel, setRightPanel]               = useState<RightPanelMode>(() => loadUiLayout().rightPanel);
  const [showCueListTabs, setShowCueListTabs]     = useState(() => loadUiLayout().showCueListTabs);
  const [showSearchBar, setShowSearchBar]         = useState(() => loadUiLayout().showSearchBar);
  const [showLivePanel, setShowLivePanel]         = useState(() => loadUiLayout().showLivePanel);
  const [showSlicePanel, setShowSlicePanel]       = useState(() => loadUiLayout().showSlicePanel);
  const [activeClipTab, setActiveClipTab]         = useState<"Live" | "Slice">(() => loadUiLayout().activeClipTab);
  const [inspectorWidth, setInspectorWidth]       = useState(() => loadUiLayout().inspectorWidth);
  const [editorCueId, setEditorCueId]             = useState<string | null>(null);
  const [numberTimelineCueId, setNumberTimelineCueId] = useState<string | null>(null);
  const [curveCueId, setCurveCueId]               = useState<string | null>(null);
  const [inspectorReload, setInspectorReload]     = useState(0);
  const [editorReload, setEditorReload]           = useState(0);
  const [showMode, setShowMode]                   = useState(false);
  const [closeDialogOpen, setCloseDialogOpen]     = useState(false);
  const [activePlaybackCloseOpen, setActivePlaybackCloseOpen] = useState(false);
  const [gotoOpen, setGotoOpen]                   = useState(false);
  const [outputSurfaceVisible, setOutputSurfaceVisible] = useState(false);
  const [outputMonitorVisible, setOutputMonitorVisible] = useState(false);
  const [loadError, setLoadError]                 = useState<string | null>(null);
  const [workspaceGuardOpen, setWorkspaceGuardOpen] = useState(false);
  const [workspaceError, setWorkspaceError]         = useState<string | null>(null);
  const [collectReport, setCollectReport]         = useState<CollectReport | null>(null);
  const [importReport, setImportReport]           = useState<ImportReport | null>(null);
  const [recentFiles, setRecentFiles]             = useState<string[]>(loadRecentFiles);
  const [showAbout, setShowAbout]                 = useState(false);
  const [preflightOpen, setPreflightOpen]         = useState(false);
  const [logsOpen, setLogsOpen]                   = useState(false);
  const [searchQuery, setSearchQuery]             = useState("");
  const [recoveryInfo, setRecoveryInfo]           = useState<RecoveryInfo | null>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const closeDialogOpenRef = useRef(false);
  const activePlaybackCloseOpenRef = useRef(false);
  const pendingWorkspaceActionRef = useRef<(() => Promise<void>) | null>(null);
  const pendingProjectOpensRef = useRef<string[]>([]);
  const processingProjectOpensRef = useRef(false);
  const workspaceActionBusyRef = useRef(false);

  // Persist panel visibility + inspector width across launches.
  useEffect(() => {
    saveUiLayout({ showCueListTabs, rightPanel, showSearchBar, inspectorWidth, showLivePanel, showSlicePanel, activeClipTab });
  }, [showCueListTabs, rightPanel, showSearchBar, inspectorWidth, showLivePanel, showSlicePanel, activeClipTab]);

  useEffect(() => {
    useUpdateStore.getState().setInstallGuard(() => {
      const workspace = useWorkspaceStore.getState();
      if (flattenActiveCues(workspace.cues).length > 0) return "activeCuesRunning";
      if (workspace.workspaceInfo?.is_modified) return "unsavedWorkspace";
      return null;
    });
    const timer = window.setTimeout(() => void useUpdateStore.getState().checkForUpdates({ silent: true }), 5000);
    return () => window.clearTimeout(timer);
  }, []);

  // Clip Editor visibility is machine-global UI state. The local layout
  // remains a startup fallback for old sessions; the global preference mirror
  // loaded below is authoritative and is never project dirty state.
  useEffect(() => {
    const migrated = normalizeClipEditorVisibility(displayPrefs);
    setShowLivePanel(migrated.show_live_panel);
    setShowSlicePanel(migrated.show_slice_panel);
    setActiveClipTab(migrated.clip_editor_active_tab);
  }, [displayPrefs.show_live_panel, displayPrefs.show_slice_panel, displayPrefs.clip_editor_active_tab]);

  const persistClipEditorPrefs = useCallback((patch: Partial<Pick<import("./lib/types").DisplayPreferences, "show_live_panel" | "show_slice_panel" | "clip_editor_active_tab">>) => {
    const store = useWorkspaceStore.getState();
    const next = { ...store.displayPrefs, ...patch };
    store.setDisplayPrefs(next);
    void updateDisplayPreferences(next).catch((error) => console.error("Failed to save Clip Editor preferences", error));
  }, []);

  // Drag-resize the inspector from its left edge (pointer capture keeps the
  // drag alive even when the cursor leaves the 5 px handle).
  const handleInspectorResizeStart = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = inspectorWidth;
    const onMove = (ev: PointerEvent) =>
      setInspectorWidth(clampInspectorWidth(startWidth + (startX - ev.clientX)));
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  // Apply data-theme whenever display prefs change
  useEffect(() => {
    const root = document.documentElement;
    const theme = displayPrefs.theme ?? "system";

    if (theme === "system") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      const apply = (dark: boolean) => {
        const effective = dark ? "dark" : "light";
        root.setAttribute("data-theme", effective);
        try { localStorage.setItem("wc_theme", effective); } catch { /* ignore */ }
      };
      apply(mq.matches);
      const handler = (e: MediaQueryListEvent) => apply(e.matches);
      mq.addEventListener("change", handler);
      return () => mq.removeEventListener("change", handler);
    } else {
      root.setAttribute("data-theme", theme);
      try { localStorage.setItem("wc_theme", theme); } catch { /* ignore */ }
    }
  }, [displayPrefs.theme]);

  // Bootstrap
  useEffect(() => {
    refreshCues();
    // Use getState() inside the .then() so we read the store at resolution time,
    // not at call time. This prevents a stale response from overwriting a
    // cue-lists-changed event that fired while the IPC was in flight.
    void getCueLists().then((lists) => {
      const store = useWorkspaceStore.getState();
      if (store.cueLists.length === 0 && lists.length > 0) {
        store.setCueLists(lists, lists[0].id);
      }
    }).catch(console.error);
    refreshWorkspaceInfo();
    void refreshValidation();
    void refreshHealth();
    loadGeneralPrefs();
    loadDisplayPrefs();
    void getOutputWindowVisible().then(setOutputSurfaceVisible);

    const unlistenVisible = listen<boolean>("output-window-visible", (e) => {
      setOutputSurfaceVisible(e.payload);
    });
    const unlistenMonitorVisible = listen<boolean>("output-monitor-visible", (e) => {
      setOutputMonitorVisible(e.payload);
    });
    return () => {
      void unlistenVisible.then((u) => u()).catch(console.error);
      void unlistenMonitorVisible.then((u) => u()).catch(console.error);
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // One-time crash-recovery prompt: a snapshot left by a previous session means
  // it ended abnormally (crash / power loss) with unsaved work. Offer to restore.
  const recoveryPrompted = useRef(false);
  useEffect(() => {
    if (recoveryPrompted.current) return;
    recoveryPrompted.current = true;
    void checkRecovery()
      .then((info) => { if (info) setRecoveryInfo(info); })
      .catch((err) => console.error("recovery check failed", err));
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const handleRecover = useCallback(async () => {
    setRecoveryInfo(null);
    try {
      await restoreRecovery();
      await refreshCues();
      await refreshWorkspaceInfo();
    } catch (err) {
      console.error("restore recovery failed", err);
      setLoadError(t("sweepUi.recoveryFailed", { error: String(err) }));
    }
  }, [refreshCues, refreshWorkspaceInfo, t]);

  const handleDiscardRecovery = useCallback(async () => {
    setRecoveryInfo(null);
    await discardRecovery().catch((err) => console.error("discard recovery failed", err));
  }, []);

  // -------------------------------------------------------------------------
  // Shared save helpers (used by FileMenu AND CloseConfirmDialog)
  // -------------------------------------------------------------------------

  /** Save to the existing path, or open Save As if no path is set yet.
   *  Returns true if the save completed, false if the user cancelled. */
  const handleSaveAs = useCallback(async (): Promise<boolean> => {
    const path = await saveDialog({
      filters: [{ name: "Qlisa Project", extensions: ["qlisa", "inkue"] }],
      defaultPath: (workspaceInfo?.name ?? t("app.untitled")) + ".qlisa",
    });
    if (typeof path !== "string") return false;
    const filePath = withDefaultProjectExtension(path);
    try {
      await saveWorkspace(filePath);
      await refreshWorkspaceInfo();
      setRecentFiles(pushRecentFile(filePath));
      return true;
    } catch (error) {
      console.error("Failed to save Qlisa project", error);
      setWorkspaceError(String(error));
      return false;
    }
  }, [workspaceInfo, refreshWorkspaceInfo]);

  const handleSave = useCallback(async (): Promise<boolean> => {
    let path: string | null;
    try {
      path = (await getWorkspaceInfo()).file_path;
    } catch (error) {
      console.error("Failed to read current Qlisa project before saving", error);
      setWorkspaceError(String(error));
      return false;
    }
    if (path) {
      try {
        await saveWorkspace(path);
        await refreshWorkspaceInfo();
        setRecentFiles(pushRecentFile(path));
        return true;
      } catch (error) {
        console.error("Failed to save Qlisa project", error);
        setWorkspaceError(String(error));
        return false;
      }
    }
    return handleSaveAs();
  }, [refreshWorkspaceInfo, handleSaveAs]);

  const performOpenWorkspacePath = useCallback(async (path: string): Promise<boolean> => {
    try {
      await loadWorkspace(path);
      setRecentFiles(pushRecentFile(path));
      setSearchQuery("");
      return true;
    } catch (error) {
      console.error("Failed to open Qlisa project", error);
      setWorkspaceError(String(error));
      return false;
    }
  }, []);

  const runWorkspaceAction = useCallback(async (action: () => Promise<void>) => {
    if (pendingWorkspaceActionRef.current || workspaceActionBusyRef.current) return;
    workspaceActionBusyRef.current = true;
    try {
      const inspection = await inspectWorkspaceGuard(async () => (await getWorkspaceInfo()).is_modified);
      if (inspection.result === "error") {
        console.error("Failed to check Qlisa project dirty state", inspection.error);
        setWorkspaceError(String(inspection.error));
        return;
      }
      if (inspection.result === "prompt") {
        pendingWorkspaceActionRef.current = action;
        setWorkspaceGuardOpen(true);
        return;
      }
      await action();
    } catch (error) {
      console.error("Workspace action failed", error);
      setWorkspaceError(String(error));
    } finally {
      workspaceActionBusyRef.current = false;
    }
  }, []);

  const openWorkspacePath = useCallback(async (path: string) => {
    await runWorkspaceAction(async () => { await performOpenWorkspacePath(path); });
  }, [runWorkspaceAction, performOpenWorkspacePath]);

  const drainProjectOpenQueue = useCallback(async () => {
    if (processingProjectOpensRef.current || pendingWorkspaceActionRef.current || workspaceActionBusyRef.current) return;
    processingProjectOpensRef.current = true;
    workspaceActionBusyRef.current = true;
    try {
      while (pendingProjectOpensRef.current.length > 0) {
        if (pendingWorkspaceActionRef.current) return;
        const path = pendingProjectOpensRef.current[0];
        const inspection = await inspectWorkspaceGuard(async () => (await getWorkspaceInfo()).is_modified);
        if (inspection.result === "error") {
          console.error("Failed to check Qlisa project dirty state", inspection.error);
          pendingProjectOpensRef.current = [];
          setWorkspaceError(String(inspection.error));
          return;
        }
        const action = async () => {
          pendingProjectOpensRef.current.shift();
          await performOpenWorkspacePath(path);
          void drainProjectOpenQueue();
        };
        if (inspection.result === "prompt") {
          pendingWorkspaceActionRef.current = action;
          processingProjectOpensRef.current = false;
          setWorkspaceGuardOpen(true);
          return;
        }
        pendingProjectOpensRef.current.shift();
        await performOpenWorkspacePath(path);
      }
    } finally {
      processingProjectOpensRef.current = false;
      workspaceActionBusyRef.current = false;
    }
  }, [performOpenWorkspacePath]);

  const enqueueProjectOpens = useCallback((paths: string[]) => {
    pendingProjectOpensRef.current.push(...paths.filter(isProjectFilePath));
    void drainProjectOpenQueue();
  }, [drainProjectOpenQueue]);

  const resolveWorkspaceGuardAction = useCallback(async (choice: "save" | "discard" | "cancel") => {
    const action = pendingWorkspaceActionRef.current;
    if (!action || workspaceActionBusyRef.current) return;
    workspaceActionBusyRef.current = true;
    try {
      const saved = choice === "save" ? await handleSave() : false;
      // This dialog is only opened for a dirty workspace. Keep that original
      // condition authoritative: a cancelled/failed Save must never allow replace.
      const resolution = resolveWorkspaceGuard(true, choice, saved);
      if (resolution !== "execute") {
        if (choice === "cancel") {
          pendingWorkspaceActionRef.current = null;
          pendingProjectOpensRef.current = [];
          setWorkspaceGuardOpen(false);
        }
        return;
      }
      pendingWorkspaceActionRef.current = null;
      setWorkspaceGuardOpen(false);
      await action();
    } catch (error) {
      console.error("Workspace action failed", error);
      setWorkspaceError(String(error));
    } finally {
      workspaceActionBusyRef.current = false;
      void drainProjectOpenQueue();
    }
  }, [handleSave, drainProjectOpenQueue]);

  const handleOpen = useCallback(async () => {
    const path = await openDialog({
      multiple: false,
      filters: [{ name: "Qlisa Project", extensions: ["qlisa", "inkue", "wincue"] }],
    });
    if (typeof path === "string") {
      await openWorkspacePath(path);
    }
  }, [openWorkspacePath]);

  const handleNew = useCallback(async () => {
    runWorkspaceAction(async () => {
      try { await newWorkspace(); }
      catch (error) { console.error("Failed to create Qlisa project", error); setWorkspaceError(String(error)); }
    });
  }, [runWorkspaceAction]);

  const handleImportQlab = useCallback(async () => {
    const path = await openDialog({
      multiple: false,
      filters: [{ name: "QLab Workspace", extensions: ["qlab5", "qlab4"] }],
    });
    if (typeof path !== "string") return;
    runWorkspaceAction(async () => {
      try {
        // The import replaces the current show and is deliberately left unsaved:
        // media resolves against the QLab bundle folder, so nothing is written
        // into the user's QLab project. Save As is the operator's next step.
        setImportReport(await importQlabWorkspace(path));
        setSearchQuery("");
      } catch (err) {
        setWorkspaceError(String(err));
      }
    });
  }, [runWorkspaceAction]);

  const handleCollectAndSave = useCallback(async () => {
    const dir = await openDialog({ directory: true });
    if (typeof dir !== "string") return;
    try {
      const report = await collectAndSave(dir);
      setCollectReport(report);
    } catch (err) {
      setLoadError(String(err));
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const drain = () => {
      void drainPendingProjectOpens()
        .then(enqueueProjectOpens)
        .catch((error) => console.error("Failed to receive project-open request", error));
    };
    // Register first. The queue then covers both startup arguments and later
    // single-instance/open-with requests without a startup race.
    void listen("project-open-requested", drain).then((stop) => {
      if (disposed) stop();
      else {
        unlisten = stop;
        drain();
      }
    }).catch((error) => console.error("Failed to listen for project-open requests", error));
    return () => { disposed = true; unlisten?.(); };
  }, [enqueueProjectOpens]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWindow().onDragDropEvent((event) => {
      if (event.payload.type === "drop") enqueueProjectOpens(event.payload.paths);
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch((error) => console.error("Failed to listen for project file drops", error));
    return () => { disposed = true; unlisten?.(); };
  }, [enqueueProjectOpens]);

  // -------------------------------------------------------------------------
  // Close-request interception
  // -------------------------------------------------------------------------

  // On Linux, GNOME/Mutter may restore a previous maximised *or* fullscreen state
  // from the session — overriding tauri.conf.json ("maximized": false,
  // "fullscreen": false) — and it applies that state *after* the window is mapped.
  // Normalise the window on mount and once more on the next tick so we reliably win
  // that race and always start at the configured 1280×800 size.
  useEffect(() => {
    const normalize = () => {
      const win = getCurrentWindow();
      void win.setFullscreen(false).catch(() => {});
      void win.unmaximize().catch(() => {});
    };
    normalize();
    const id = setTimeout(normalize, 150);
    return () => clearTimeout(id);
  }, []);

  useEffect(() => {
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;

    win.onCloseRequested((event) => {
      // A Tauri close request can arrive more than once while a modal is
      // being painted (for example from the title-bar button and Alt+F4).
      // Always consume duplicates while a close flow is already visible.
      if (activePlaybackCloseOpenRef.current || closeDialogOpenRef.current) {
        event.preventDefault();
        return;
      }

      const state = useWorkspaceStore.getState();
      if (hasActivePlayback(state.cues)) {
        event.preventDefault();
        activePlaybackCloseOpenRef.current = true;
        setActivePlaybackCloseOpen(true);
        return;
      }

      const isModified = state.workspaceInfo?.is_modified;
      if (isModified) {
        event.preventDefault();
        closeDialogOpenRef.current = true;
        setCloseDialogOpen(true);
      }
      // Not modified → close proceeds normally.
    }).then((u) => { unlisten = u; });

    return () => unlisten?.();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // destroy() force-closes without re-triggering onCloseRequested.
  const continueAfterActivePlaybackClose = () => {
    activePlaybackCloseOpenRef.current = false;
    setActivePlaybackCloseOpen(false);
    if (useWorkspaceStore.getState().workspaceInfo?.is_modified) {
      closeDialogOpenRef.current = true;
      setCloseDialogOpen(true);
    } else {
      void getCurrentWindow().destroy();
    }
  };

  const cancelActivePlaybackClose = () => {
    activePlaybackCloseOpenRef.current = false;
    setActivePlaybackCloseOpen(false);
  };

  const confirmSaveAndClose = async () => {
    closeDialogOpenRef.current = false;
    setCloseDialogOpen(false);
    const saved = await handleSave();
    if (saved) {
      void getCurrentWindow().destroy();
    }
    // If user cancelled the Save As dialog, keep the app open.
  };

  const confirmDiscardAndClose = () => {
    closeDialogOpenRef.current = false;
    setCloseDialogOpen(false);
    void getCurrentWindow().destroy();
  };

  const cancelClose = () => {
    closeDialogOpenRef.current = false;
    setCloseDialogOpen(false);
  };

  // -------------------------------------------------------------------------
  // Misc
  // -------------------------------------------------------------------------

  const handleLoadError = useCallback((_cueId: string, error: string) => {
    setLoadError(error);
  }, []);

  useTauriEvents({ onLoadError: handleLoadError });

  const handleRefresh = useCallback(async () => {
    await refreshCues();
  }, [refreshCues]);

  useKeyboardShortcuts(
    handleRefresh,
    () => void openPreferencesWindow(),
    () => void handleSave(),
    () => void handleOpen(),
    () => setRightPanel((v) => toggleRightPanel(v, "inspector")),
    () => setGotoOpen(true),
    () => void handleToggleSurface(),
    () => setShowMode((v) => !v),
    () => handleToggleSearch(),
  );

  const selectedCue = findCueRecursive(cues, selectedCueId) ?? null;
  const activeCueCount = flattenActiveCues(cues).length;
  const selectedNumberOwner = selectedCue?.cue_type === "number" ? selectedCue.id : findNumberOwner(cues, selectedCueId);
  const numberPreviewPositionMs = useNumberPreviewStore((state) =>
    selectedCue?.cue_type === "number" && state.numberId === selectedCue.id ? state.positionMs : 0,
  );
  const [selectedNumberData, setSelectedNumberData] = useState<NumberCueData | null>(null);
  const numberPreviewLoadGeneration = useRef(0);

  useEffect(() => {
    const generation = ++numberPreviewLoadGeneration.current;
    if (!selectedCue || selectedCue.cue_type !== "number") {
      setSelectedNumberData(null);
      return;
    }
    setSelectedNumberData(null);
    getCue(selectedCue.id)
      .then((data) => {
        if (generation !== numberPreviewLoadGeneration.current || data.cue_type !== "number") return;
        setSelectedNumberData(normalizeNumberCueData(data as NumberCueData & { master_child_id?: string | null; action_offsets_ms?: Record<string, number> }));
      })
      .catch(() => {
        if (generation === numberPreviewLoadGeneration.current) setSelectedNumberData(null);
      });
  }, [selectedCue?.id, selectedCue?.cue_type, inspectorReload, editorReload]);

  useEffect(() => {
    if (selectedNumberOwner) setNumberTimelineCueId(selectedNumberOwner);
    else setNumberTimelineCueId(null);
  }, [selectedNumberOwner]);

  const monitorPreviewPayload = resolveMonitorPreviewSelection(selectedCue, selectedNumberData, numberPreviewPositionMs);
  const monitorPreviewPayloadKey = monitorPreviewPayload === undefined
    ? "preserve"
    : monitorPreviewPayload?.cueId ?? "clear";
  const sendMonitorPreviewSelection = useCallback(async (payload: { cueId: string } | null | undefined) => {
    // A Number can legitimately be audio-only at the current cursor. Keep an
    // already-open Output Monitor on its previous visual instead of clearing
    // it and flashing a black frame.
    if (payload === undefined) return;
    await emitTo("output-monitor", "output-monitor-preview-selection", payload);
  }, []);

  useEffect(() => {
    void sendMonitorPreviewSelection(monitorPreviewPayload).catch(() => undefined);
  }, [selectedCue?.id, selectedCue?.cue_type, selectedCue?.file_path, selectedCue?.name, monitorPreviewPayloadKey, sendMonitorPreviewSelection]);

  const handleToggleOutputMonitor = async () => {
    if (outputMonitorVisible) {
      await emitTo("output-monitor", "output-monitor-request-hide");
      return;
    }
    await openOutputMonitorWindow();
    await sendMonitorPreviewSelection(monitorPreviewPayload);
  };

  const handleToggleSurface = async () => {
    await toggleOutputWindow().catch(console.error);
    // outputSurfaceVisible is driven by the "output-window-visible" event from Rust
  };

  // Show/hide the search bar. Showing it focuses the input; hiding clears the
  // query so the cue list reappears without a lingering filter.
  const handleToggleSearch = () => {
    if (showSearchBar) {
      setSearchQuery("");
      setShowSearchBar(false);
    } else {
      setShowSearchBar(true);
      requestAnimationFrame(() => searchInputRef.current?.focus());
    }
  };

  const handleClipTabChange = (tab: "Live" | "Slice") => {
    setActiveClipTab(tab);
    persistClipEditorPrefs({ clip_editor_active_tab: tab });
  };

  const toggleClipPanel = (panel: "Live" | "Slice") => {
    const next = toggleClipEditorPanel({ show_live_panel: showLivePanel, show_slice_panel: showSlicePanel, clip_editor_active_tab: activeClipTab }, panel);
    setShowLivePanel(next.show_live_panel);
    setShowSlicePanel(next.show_slice_panel);
    setActiveClipTab(next.clip_editor_active_tab);
    persistClipEditorPrefs(next);
  };

  const handleAddAudio = async () => {
    const { selectedCueId, cues } = useWorkspaceStore.getState();
    const idx = cues.findIndex((c) => c.id === selectedCueId);
    await addCue("audio", idx >= 0 ? idx + 1 : -1).catch(console.error);
    await refreshCues();
  };

  const handleAddStop = async () => {
    const { selectedCueId, cues } = useWorkspaceStore.getState();
    const idx = cues.findIndex((c) => c.id === selectedCueId);
    await addCue("stop", idx >= 0 ? idx + 1 : -1).catch(console.error);
    await refreshCues();
  };

  const handleAddVideo = async () => {
    const { selectedCueId, cues } = useWorkspaceStore.getState();
    const idx = cues.findIndex((c) => c.id === selectedCueId);
    await addCue("video", idx >= 0 ? idx + 1 : -1).catch(console.error);
    await refreshCues();
  };

  const handleAddImage = async () => {
    const { selectedCueId, cues } = useWorkspaceStore.getState();
    const idx = cues.findIndex((c) => c.id === selectedCueId);
    await addCue("image", idx >= 0 ? idx + 1 : -1).catch(console.error);
    await refreshCues();
  };

  const handleAddWait = async () => {
    const { selectedCueId, cues } = useWorkspaceStore.getState();
    const idx = cues.findIndex((c) => c.id === selectedCueId);
    await addCue("wait", idx >= 0 ? idx + 1 : -1).catch(console.error);
    await refreshCues();
  };

  const handleAddGroup = async () => {
    const { selectedCueId, cues } = useWorkspaceStore.getState();
    const idx = cues.findIndex((c) => c.id === selectedCueId);
    await addCue("group", idx >= 0 ? idx + 1 : -1).catch(console.error);
    await refreshCues();
  };

  const handleAddNumber = async () => {
    const { selectedCueId, cues } = useWorkspaceStore.getState();
    const idx = cues.findIndex((c) => c.id === selectedCueId);
    await addCue("number", idx >= 0 ? idx + 1 : -1).catch(console.error);
    await refreshCues();
  };

  const handleAddFade = async () => {
    const { selectedCueId, cues } = useWorkspaceStore.getState();
    const idx = cues.findIndex((c) => c.id === selectedCueId);
    await addCue("fade", idx >= 0 ? idx + 1 : -1).catch(console.error);
    await refreshCues();
  };

  const handleAddMemo = async () => {
    const { selectedCueId, cues } = useWorkspaceStore.getState();
    const idx = cues.findIndex((c) => c.id === selectedCueId);
    await addCue("memo", idx >= 0 ? idx + 1 : -1).catch(console.error);
    await refreshCues();
  };

  const handleAddText = async () => {
    const { selectedCueId, cues } = useWorkspaceStore.getState();
    const idx = cues.findIndex((c) => c.id === selectedCueId);
    await addCue("text", idx >= 0 ? idx + 1 : -1).catch(console.error);
    await refreshCues();
  };

  const handleAddCommand = async (cueType: CueType) => {
    const { selectedCueId, cues } = useWorkspaceStore.getState();
    const idx = cues.findIndex((c) => c.id === selectedCueId);
    await addCue(cueType, idx >= 0 ? idx + 1 : -1).catch(console.error);
    await refreshCues();
  };

  const dispatchCueDrag = (cueType: CueType, e: React.MouseEvent) => {
    if (e.button !== 0) return;
    document.dispatchEvent(
      new CustomEvent("inkue:cue-drag-start", {
        detail: { cueType, startX: e.clientX, startY: e.clientY },
      }),
    );
  };

  const titleBarName = workspaceInfo
    ? `${workspaceInfo.name}${workspaceInfo.is_modified ? " •" : ""}`
    : t("app.name");

  useEffect(() => {
    const title = workspaceInfo?.name
      ? t("app.workspaceTitle", { name: workspaceInfo.name })
      : t("app.name");
    document.title = title;
    void getCurrentWindow().setTitle(title).catch(() => {});
  }, [workspaceInfo?.name, t]);

  return (
    <div
      style={{
        display: "flex", flexDirection: "column", height: "100vh",
        background: "var(--wc-bg-app)", color: "var(--wc-text)",
        fontFamily: "'Segoe UI', system-ui, -apple-system, BlinkMacSystemFont, sans-serif",
        overflow: "hidden",
      }}
      onContextMenu={(e) => e.preventDefault()}
    >
      {/* Audio file load error toast */}
      {loadError && (
        <div
          style={{
            position: "fixed", bottom: 20, left: "50%", transform: "translateX(-50%)",
            zIndex: 99999, background: "#7f1d1d", border: "1px solid #ef4444",
            borderRadius: 8, padding: "10px 16px", maxWidth: 520,
            display: "flex", alignItems: "flex-start", gap: 12,
            boxShadow: "0 8px 24px rgba(0,0,0,0.8)",
          }}
        >
          <span style={{ color: "#fca5a5", fontSize: 13, flex: 1 }}>
            <strong style={{ color: "#fecaca" }}>{t("errors.failedToLoad", { item: t("cueTypes.audio").toLowerCase() })}</strong>
            <br />
            <span style={{ opacity: 0.85, fontFamily: "monospace", fontSize: 11 }}>{loadError}</span>
          </span>
          <button
            onClick={() => setLoadError(null)}
            style={{
              background: "transparent", border: "none", color: "#fca5a5",
              cursor: "pointer", fontSize: 16, padding: 0, lineHeight: 1, flexShrink: 0,
            }}
          >
            ✕
          </button>
        </div>
      )}

      {workspaceError && (
        <div role="alert" style={{ position: "fixed", bottom: 20, left: "50%", transform: "translateX(-50%)", zIndex: 100000, background: "#7f1d1d", border: "1px solid #ef4444", borderRadius: 8, padding: "10px 16px", maxWidth: 620, display: "flex", alignItems: "flex-start", gap: 12, boxShadow: "0 8px 24px rgba(0,0,0,0.8)" }}>
          <span style={{ color: "#fecaca", fontSize: 13, flex: 1, overflowWrap: "anywhere" }}>{workspaceError}</span>
          <button onClick={() => setWorkspaceError(null)} style={{ background: "transparent", border: "none", color: "#fca5a5", cursor: "pointer", fontSize: 16, padding: 0, lineHeight: 1, flexShrink: 0 }}>✕</button>
        </div>
      )}

      {/* Goto cue dialog */}
      {gotoOpen && (
        <GotoDialog onClose={() => setGotoOpen(false)} onRefresh={handleRefresh} />
      )}

      {/* Unsaved-changes dialog */}
      {closeDialogOpen && (
        <CloseConfirmDialog
          onSave={confirmSaveAndClose}
          onDiscard={confirmDiscardAndClose}
          onCancel={cancelClose}
        />
      )}
      {workspaceGuardOpen && !closeDialogOpen && !activePlaybackCloseOpen && (
        <CloseConfirmDialog
          titleKey="dialogs.workspaceChangeTitle"
          messageKey="dialogs.workspaceChangeMessage"
          onSave={() => void resolveWorkspaceGuardAction("save")}
          onDiscard={() => void resolveWorkspaceGuardAction("discard")}
          onCancel={() => void resolveWorkspaceGuardAction("cancel")}
        />
      )}
      {activePlaybackCloseOpen && (
        <ActivePlaybackCloseDialog
          onContinue={continueAfterActivePlaybackClose}
          onCancel={cancelActivePlaybackClose}
        />
      )}

      {/* Collect & Save result dialog */}
      {importReport && (
        <QlabImportDialog report={importReport} onClose={() => setImportReport(null)} />
      )}
      {collectReport && (
        <CollectResultDialog
          report={collectReport}
          onClose={() => setCollectReport(null)}
        />
      )}

      {/* Crash-recovery dialog */}
      {recoveryInfo && (
        <RecoveryDialog
          info={recoveryInfo}
          onRecover={() => void handleRecover()}
          onDiscard={() => void handleDiscardRecovery()}
        />
      )}

      {/* About dialog */}
      {showAbout && <AboutDialog onClose={() => setShowAbout(false)} />}
      <UpdateDialog />
      {preflightOpen && <PreflightModal onClose={() => setPreflightOpen(false)} />}
      {logsOpen && <LogViewerModal onClose={() => setLogsOpen(false)} />}

      {/* Custom title bar — two rows: Row 1 holds the window controls, menus and a
          full-width drag area; Row 2 holds the cue toolbar.  Splitting them means
          the toolbar can never squeeze the drag area down to an ungrabbable sliver
          when the window is narrow (the previous single-row layout collapsed it to
          ~40px, so only the "Inkue" label was draggable).  No drag-region on the
          row containers so menus/buttons keep working on Linux/WebKitGTK. */}
      <div
        style={{
          display: "flex", flexDirection: "column",
          background: "var(--wc-bg-surface)", borderBottom: "1px solid var(--wc-border)",
          flexShrink: 0, userSelect: "none", WebkitUserSelect: "none",
          // Keep title-bar popovers above the following workspace/section
          // stacking context while allowing them to extend below the bar.
          // Keep title-bar popovers above CueList's sticky cells and its
          // fixed context-menu/backdrop layers (currently below 10000).
          position: "relative", zIndex: 10000,
        }}
      >
        {/* Row 1 — window controls, File/View menus, draggable workspace title */}
        <div style={{ display: "flex", alignItems: "center", height: 36, padding: "0 12px", gap: 12, position: "relative" }}>
        <WindowControls />
        <FileMenu
          onSave={() => void handleSave()}
          onSaveAs={() => void handleSaveAs()}
          onOpen={() => void handleOpen()}
          onNew={() => void handleNew()}
          onImportQlab={() => void handleImportQlab()}
          onCollect={() => void handleCollectAndSave()}
          onCheck={() => setPreflightOpen(true)}
          onLogs={() => setLogsOpen(true)}
          onDiagnostics={() => void openDiagnosticsWindow()}
          onPreferences={() => void openPreferencesWindow()}
          onAbout={() => setShowAbout(true)}
          recentFiles={recentFiles}
          onOpenRecent={(p) => void openWorkspacePath(p)}
        />
        <EditMenu onRefresh={handleRefresh} />
        <HelpMenu onCheck={() => void useUpdateStore.getState().checkForUpdates()} onAbout={() => setShowAbout(true)} />
        <ViewMenu
          items={[
            { label: t("menus.showMode"), checked: showMode, onClick: () => setShowMode((v) => !v), shortcut: "F5" },
            { label: t("menus.cueListTabs"), checked: showCueListTabs, onClick: () => setShowCueListTabs((v) => !v) },
            { label: CLIP_EDITOR_TAB_LABELS.timeline, checked: showLivePanel, onClick: () => toggleClipPanel("Live") },
            { label: CLIP_EDITOR_TAB_LABELS.slice, checked: showSlicePanel, onClick: () => toggleClipPanel("Slice") },
            { label: t("menus.searchBar"), checked: showSearchBar, onClick: handleToggleSearch, shortcut: "Ctrl+F" },
            { label: t("menus.activeCues"), checked: rightPanel === "active-cues", onClick: () => setRightPanel((v) => toggleRightPanel(v, "active-cues")) },
            { label: t("menus.inspector"), checked: rightPanel === "inspector", onClick: () => setRightPanel((v) => toggleRightPanel(v, "inspector")) },
            { label: t("menus.outputSurface"), checked: outputSurfaceVisible, onClick: () => void handleToggleSurface() },
            { label: t("menus.outputMonitor"), checked: outputMonitorVisible, onClick: () => void handleToggleOutputMonitor() },
          ]}
        />
        <ActionMenu onDone={handleRefresh} />

        {/* Drag region: app name + workspace name */}
        <div
          data-tauri-drag-region
          style={{ flex: 1, minWidth: 40, position: "relative", display: "flex", alignItems: "center", gap: 8, overflow: "hidden" }}
        >
          <div data-tauri-drag-region style={{ flexShrink: 0, pointerEvents: "none", display: "flex" }}>
            <InkueMark size={18} />
          </div>
          <span
            data-tauri-drag-region
            style={{ fontWeight: 700, fontSize: 13, color: "var(--wc-text-bright)", flexShrink: 0, letterSpacing: "-0.01em" }}
          >
            Qlisa
          </span>
          <span
            data-tauri-drag-region
            style={{ fontSize: 12, color: "var(--wc-text-muted)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
          >
            {titleBarName}
          </span>
          {brokenCueIds.size > 0 && (
            <button
              onClick={() => setPreflightOpen(true)}
              title={t("toolbar.problems")}
              style={{
                flexShrink: 0, display: "flex", alignItems: "center", gap: 4,
                background: "rgba(239,68,68,0.15)", border: "1px solid rgba(239,68,68,0.5)",
                borderRadius: 5, color: "#ef4444", cursor: "pointer",
                fontSize: 11, padding: "2px 8px",
              }}
            >
              ⚠ {brokenCueIds.size}
            </button>
          )}
        </div>
        <div
          onPointerDown={(event) => event.stopPropagation()}
          style={{ position: "absolute", left: "50%", top: "50%", transform: "translate(-50%, -50%)", display: "flex", alignItems: "center" }}
        >
          <FullscreenControl />
        </div>

        </div>{/* end Row 1 */}

        {/* Row 2 — one-line cue toolbar. The fixed right controls keep their
            width while the left cue actions move into More as space shrinks. */}
        {showMode ? null : <CueToolbar
          activeCueCount={activeCueCount}
          rightPanel={rightPanel}
          onToggleRightPanel={(panel) => setRightPanel((value) => toggleRightPanel(value, panel))}
          onSettings={() => void openPreferencesWindow()}
          onAdd={(type) => {
            const direct: Partial<Record<CueType, () => void>> = {
              audio: handleAddAudio, video: handleAddVideo, image: handleAddImage,
              stop: handleAddStop, wait: handleAddWait, group: handleAddGroup,
              number: handleAddNumber, fade: handleAddFade, memo: handleAddMemo, text: handleAddText,
            };
            (direct[type] ?? (() => void handleAddCommand(type)))();
          }}
          onDragStart={(type, event) => dispatchCueDrag(type, event)}
        />}
      </div>

      {/* Runtime health banner (device/network faults) */}
      <HealthBanner />

      {/* Main area */}
      <div style={{ display: "flex", flex: 1, overflow: "hidden" }}>
        {showMode ? (
          <ShowModeView />
        ) : (
          <>
            <div style={{ flex: 1, minWidth: 0, minHeight: 0, overflow: "hidden", display: "flex", flexDirection: "column" }}>
              {showCueListTabs && <CueListTabs onRefresh={handleRefresh} />}
              {searchQuery.trim() ? (
                <SearchResults
                  query={searchQuery.trim()}
                  allCues={cues}
                  onSelect={(id) => {
                    useWorkspaceStore.getState().setSelectedCueId(id);
                    setRightPanel("inspector");
                    setSearchQuery("");
                  }}
                />
              ) : (
                (() => {
                  const activeList = cueLists.find((l) => l.id === activeCueListId);
                  if (activeList?.mode === "cart") {
                    return <CartView onRefresh={handleRefresh} />;
                  }
                  return (
                    <CueListView
                      onCueDoubleClick={(cue: CueSummary) => {
                        useWorkspaceStore.getState().setSelectedCueId(cue.id);
                        setRightPanel("inspector");
                      }}
                      onOpenInspector={() => setRightPanel("inspector")}
                      onRefresh={handleRefresh}
                    />
                  );
                })()
              )}

              {/* Curve editor dock — fade shapes for the opened Fade Cue */}
              {curveCueId && (
                <CurveEditorDock
                  cueId={curveCueId}
                  onClose={() => setCurveCueId(null)}
                  onSaved={() => {
                    void handleRefresh();
                    setInspectorReload((n) => n + 1);
                    // Re-fetch the active ClipEditorDock target as well. A
                    // child can be trimmed from its own dock while a Number
                    // parent remains the timeline owner; without this bump
                    // the parent keeps its old nested child timings/assets.
                    setEditorReload((n) => n + 1);
                  }}
                  reloadToken={editorReload}
                />
              )}

              {/* Clip editor dock — trim + slices for the opened cue */}
              {clipEditorDockVisible(showLivePanel, showSlicePanel) && (
                <ClipEditorDock
                  // A Number owns the multi-track Live timeline. Its child
                  // cues open their own editor for Slice, trim, and crop.
                  cueId={resolveClipEditorTargetCueId(
                    selectedCueId,
                    editorCueId,
                    selectedCue?.cue_type,
                    numberTimelineCueId,
                  )}
                  showLivePanel={showLivePanel}
                  showSlicePanel={showSlicePanel}
                  activeTab={activeClipTab}
                  onActiveTabChange={handleClipTabChange}
                  onClose={() => setEditorCueId(null)}
                  onSaved={() => {
                    void handleRefresh();
                    setInspectorReload((n) => n + 1);
                    // Re-fetch the active ClipEditorDock target as well. A
                    // child can be trimmed from its own dock while a Number
                    // parent remains the timeline owner; without this bump
                    // the parent keeps its old nested child timings/assets.
                    setEditorReload((n) => n + 1);
                  }}
                  reloadToken={editorReload}
                />
              )}

              {/* Search bar — anchored at the bottom of the cue list */}
              {showSearchBar && (
                <div style={{ padding: "4px 8px", borderTop: "1px solid var(--wc-border)", flexShrink: 0 }}>
                  <input
                    ref={searchInputRef}
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    onKeyDown={(e) => { if (e.key === "Escape") setSearchQuery(""); }}
                    placeholder={t("cueList.searchPlaceholder")}
                    style={{
                      width: "100%", boxSizing: "border-box",
                      background: searchQuery ? "var(--wc-bg-surface)" : "transparent",
                      border: searchQuery ? "1px solid var(--wc-accent)" : "1px solid transparent",
                      borderRadius: 4, color: "var(--wc-text)", fontSize: 12,
                      padding: "4px 8px", outline: "none",
                      transition: "border-color 0.15s, background 0.15s",
                    }}
                  />
                </div>
              )}
            </div>
            {rightPanel !== "closed" && (
              <div
                style={{
                  width: inspectorWidth, position: "relative",
                  borderLeft: "1px solid var(--wc-border)",
                  overflow: "hidden", display: "flex", flexDirection: "column", flexShrink: 0,
                }}
              >
                <div
                  onPointerDown={handleInspectorResizeStart}
                  title={t("toolbar.resize")}
                  style={{
                    position: "absolute", left: 0, top: 0, bottom: 0, width: 5,
                    cursor: "ew-resize", zIndex: 2,
                  }}
                />
                {rightPanel === "active-cues" ? <ActiveCuesView /> : <InspectorPanel
                  selectedCue={selectedCue}
                  selectedCueIds={selectedCueIds}
                  allCues={cues}
                  onRefresh={handleRefresh}
                  onOpenEditor={(id) => {
                    setEditorCueId(id);
                    // Inspector's expand affordance is specifically the Slice
                    // editor. Reveal it even if View previously hid Slice.
                    setShowSlicePanel(true);
                    setActiveClipTab("Slice");
                    persistClipEditorPrefs({ show_slice_panel: true, clip_editor_active_tab: "Slice" });
                  }}
                  onOpenCurveEditor={(id) => setCurveCueId(id)}
                  reloadToken={inspectorReload}
                  onCueSaved={() => setEditorReload((n) => n + 1)}
                  onSelectCue={(id) => useWorkspaceStore.getState().setSelectedCueId(id)}
                />}
              </div>
            )}
          </>
        )}
      </div>

      <TransportBar onRefresh={handleRefresh} />
    </div>
  );
}
