import { Activity, Pause, Play, Square } from "lucide-react";
import { pauseCue, resumeCue, stopCue } from "../../lib/commands";
import type { CueSummary } from "../../lib/types";
import { useLocale } from "../../i18n";
import { useTimingStore } from "../../stores/timingStore";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { formatDurationMs } from "../CueList/formatDuration";
import { CueTypeIcon } from "../common/CueTypeIcon";
import { cueProgressPercent, flattenActiveCues } from "./activeCueModel";

const COLOR_SWATCHES: Record<string, string> = {
  none: "transparent", red: "#ef4444", orange: "#f97316", yellow: "#eab308",
  green: "#22c55e", cyan: "#06b6d4", blue: "#3b82f6", purple: "#a855f7",
  pink: "#ec4899", white: "#f1f5f9", black: "#334155",
};

export function ActiveCuesView() {
  const { t } = useLocale();
  const cues = useWorkspaceStore((s) => s.cues);
  const activeCues = flattenActiveCues(cues);

  return (
    <div style={{ width: "100%", height: "100%", minWidth: 0, minHeight: 0, display: "flex", flexDirection: "column", overflow: "hidden", background: "var(--wc-bg-surface)" }}>
      <div style={{ height: 38, flexShrink: 0, display: "flex", alignItems: "center", gap: 8, padding: "0 14px", borderBottom: "1px solid var(--wc-border)", color: "var(--wc-text-bright)", fontWeight: 600, fontSize: 12 }}>
        <Activity size={15} aria-hidden="true" />
        <span>{t("activeCues.title")}</span>
        <span style={{ marginLeft: "auto", color: "var(--wc-text-muted)", fontVariantNumeric: "tabular-nums" }}>{activeCues.length}</span>
      </div>
      {activeCues.length === 0 ? (
        <div style={{ flex: 1, display: "grid", placeItems: "center", color: "var(--wc-text-muted)", fontSize: 12 }}>
          {t("activeCues.empty")}
        </div>
      ) : (
        <div style={{ flex: 1, minHeight: 0, overflowY: "auto", overflowX: "hidden", padding: 8, display: "flex", flexDirection: "column", gap: 6 }}>
          {activeCues.map((cue) => <ActiveCueRow key={cue.id} cue={cue} />)}
        </div>
      )}
    </div>
  );
}

function ActiveCueRow({ cue }: { cue: CueSummary }) {
  const { t } = useLocale();
  const timing = useTimingStore((s) => s.timings[cue.id]);
  const paused = cue.state === "paused";
  const elapsed = timing?.action_elapsed_ms ?? 0;
  const remaining = cue.duration_ms == null ? null : timing?.remaining_ms ?? Math.max(0, cue.duration_ms - elapsed);
  const progress = cueProgressPercent(elapsed, cue.duration_ms);
  const cueColor = COLOR_SWATCHES[cue.color] ?? "transparent";
  const stateColor = paused ? "#fb923c" : "#4ade80";
  const remainingLabel = remaining == null ? "—" : `−${formatDurationMs(remaining)}`;
  const title = cue.name || t("app.unnamed");

  const run = (action: () => Promise<void>) => { void action().catch((error) => console.error("Active cue control failed", error)); };

  return (
    <div
      title={title}
      data-state={cue.state}
      style={{
        position: "relative", isolation: "isolate", overflow: "hidden", flex: "0 0 58px", minWidth: 0,
        border: "1px solid var(--wc-border)", borderRadius: 6,
        background: "var(--wc-bg-app)", opacity: paused ? 0.82 : 1,
      }}
    >
      {progress !== null && <div aria-hidden="true" style={{ position: "absolute", inset: 0, zIndex: 0, pointerEvents: "none", background: `linear-gradient(90deg, ${paused ? "rgba(251,146,60,.24)" : "rgba(34,197,94,.20)"} ${progress}%, transparent ${progress}%)` }} />}
      {cueColor !== "transparent" && <div aria-hidden="true" style={{ position: "absolute", left: 0, top: 0, bottom: 0, width: 3, zIndex: 1, background: cueColor }} />}
      <div style={{ position: "relative", zIndex: 1, height: 32, minWidth: 0, display: "flex", alignItems: "center", gap: 7, padding: "0 6px 0 8px" }}>
        {canPauseCue(cue) ? <button type="button" onClick={() => run(() => paused ? resumeCue(cue.id) : pauseCue(cue.id))} title={t(paused ? "activeCues.resume" : "activeCues.pause")} aria-label={t(paused ? "activeCues.resume" : "activeCues.pause")} style={iconButtonStyle}>
          {paused ? <Play size={14} /> : <Pause size={14} />}
        </button> : <button type="button" disabled title={t("activeCues.pauseUnavailable")} aria-label={t("activeCues.pauseUnavailable")} style={{ ...iconButtonStyle, opacity: 0.4, cursor: "not-allowed" }}><Pause size={14} /></button>}
        <CueTypeIcon type={cue.cue_type} size={15} tone="neutral" />
        {cue.number && <span style={{ flexShrink: 0, color: "var(--wc-text-secondary)", fontFamily: "monospace", fontSize: 11 }}>{cue.number}</span>}
        <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", color: "var(--wc-text)", fontSize: 12 }}>{title}</span>
        <button type="button" onClick={() => run(() => stopCue(cue.id))} title={t("activeCues.stop")} aria-label={t("activeCues.stop")} style={{ ...iconButtonStyle, color: "#ef4444" }}>
          <Square size={13} fill="currentColor" />
        </button>
      </div>
      <div style={{ position: "relative", zIndex: 1, display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, padding: "0 9px 0 33px", height: 20, color: "var(--wc-text-secondary)", fontSize: 10, fontFamily: "monospace", fontVariantNumeric: "tabular-nums" }}>
        <span>{formatDurationMs(elapsed)}</span>
        <span style={{ color: remaining == null ? "var(--wc-text-muted)" : stateColor }}>{remainingLabel}</span>
      </div>
    </div>
  );
}

function canPauseCue(cue: CueSummary): boolean {
  return ["audio", "video", "image", "fade", "wait", "text", "group", "number", "mic", "midi_file"].includes(cue.cue_type);
}

const iconButtonStyle: React.CSSProperties = {
  width: 22, height: 22, flexShrink: 0, display: "grid", placeItems: "center", padding: 0,
  border: "1px solid var(--wc-border)", borderRadius: 4, background: "rgba(15,23,42,.45)",
  color: "var(--wc-text-secondary)", cursor: "pointer",
};
