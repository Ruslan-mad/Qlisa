import { useEffect, useState } from "react";
import { PLAY_COUNT_INFINITE, type CueSummary, type NumberCueData, type NumberStartStopMode } from "../../lib/types";
import { setNumberActionOffset, setNumberMaster, updateCue } from "../../lib/commands";
import { CueTypeIcon } from "../common/CueTypeIcon";
import { inputStyle, Section } from "./Field";
import { useLocale } from "../../i18n";
import { isNumberActionSupported, isNumberMasterCandidate, numberActionConfigured } from "./numberModel";

const badgeStyle: React.CSSProperties = { borderRadius: 10, padding: "2px 7px", fontSize: 10, whiteSpace: "nowrap" };

function ActionIcon({
  label,
  onClick,
  disabled,
  children,
  tone = "default",
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
  tone?: "default" | "danger" | "success";
}) {
  const color = tone === "danger" ? "#f87171" : tone === "success" ? "#86efac" : "var(--wc-text-bright)";
  return <button
    type="button"
    aria-label={label}
    title={label}
    disabled={disabled}
    onClick={onClick}
    style={{
      width: 28,
      height: 26,
      padding: 4,
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      border: "1px solid var(--wc-border)",
      borderRadius: 4,
      background: disabled ? "transparent" : "var(--wc-control-bg)",
      // Keep the muted state legible even though the action is intentionally
      // disabled: the red crossed speaker is the persisted-state feedback.
      color: tone === "danger" ? color : disabled ? "var(--wc-text-faint)" : color,
      cursor: disabled ? "default" : "pointer",
      flexShrink: 0,
    }}
  >{children}</button>;
}

function OpenIcon() {
  return <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M14 4h6v6" /><path d="M20 4 11 13" /><path d="M18 13v6a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h6" /></svg>;
}

function ResetIcon({ done }: { done: boolean }) {
  return done
    ? <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m5 12 4 4L19 6" /></svg>
    : <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M4 12a8 8 0 1 0 2.3-5.7" /><path d="M4 4v6h6" /><path d="M12 8v4l3 2" /></svg>;
}

function AudioIcon({ muted }: { muted: boolean }) {
  return <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M4 10h3l4-4v12l-4-4H4z" /><path d="M15 9a4 4 0 0 1 0 6" />{muted && <path d="m4 4 16 16" stroke="#f87171" />}</svg>;
}

function LoopIcon({ active }: { active: boolean }) {
  return <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M17 2l4 4-4 4" /><path d="M3 11V9a3 3 0 0 1 3-3h15" />
    <path d="M7 22l-4-4 4-4" /><path d="M21 13v2a3 3 0 0 1-3 3H3" />
    {active && <path d="M8 12h8" stroke="#4ade80" />}
  </svg>;
}

function StatusBadge({ child, label }: { child: CueSummary; label: (key: string) => string }) {
  const configured = numberActionConfigured(child);
  const supported = isNumberActionSupported(child);
  return <span style={{
    ...badgeStyle,
    color: !supported ? "#fca5a5" : configured ? "#86efac" : "#fbbf24",
    background: !supported ? "#7f1d1d66" : configured ? "#14532d66" : "#713f1266",
  }}>{!supported ? label("numberUi.unsupported") : configured ? label("numberUi.configured") : label("numberUi.needsSetup")}</span>;
}

export function NumberTab({
  cue,
  onRefresh,
  onSelectCue,
  onSaved,
  allCues,
}: {
  cue: NumberCueData;
  onRefresh: () => void;
  onSelectCue?: (cueId: string) => void;
  /** Notify the open clip editor so its Number timeline reloads immediately. */
  onSaved?: () => void;
  allCues?: CueSummary[];
}) {
  const { t } = useLocale();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [resetDone, setResetDone] = useState<string | null>(null);
  const [localCue, setLocalCue] = useState(cue);
  useEffect(() => setLocalCue(cue), [cue]);
  const master = localCue.children.find((child) => child.id === localCue.number_master_id) ?? null;
  const masterCandidates = localCue.children.filter(isNumberMasterCandidate);
  const flattenCues = (nodes: CueSummary[]): CueSummary[] =>
    nodes.flatMap((node) => [node, ...(node.children ? flattenCues(node.children) : [])]);
  // Flow actions operate on other cues. Excluding the entire Number tree
  // prevents stopping its own master/action or starting itself after finish.
  const ownTreeIds = new Set(flattenCues([localCue]).map((candidate) => candidate.id));
  const flowCandidates = flattenCues(allCues ?? []).filter((candidate) => !ownTreeIds.has(candidate.id));

  const stopMode = localCue.number_start_stop_mode ?? "none";
  const startStopIds = localCue.number_start_stop_ids ?? [];
  const finishStartIds = localCue.number_finish_start_ids ?? [];
  const flowLabel = (mode: NumberStartStopMode) => ({
    none: t("numberUi.stopNone"), all: t("numberUi.stopAll"), audio: t("numberUi.stopAudio"), video: t("numberUi.stopVideo"), selected: t("numberUi.stopSelected"),
  }[mode]);

  const saveFlow = (patch: Partial<NumberCueData>) => {
    void run(
      () => updateCue(localCue.id, patch),
      (current) => ({ ...current, ...patch }),
    );
  };

  const run = async (operation: () => Promise<void>, optimistic?: (current: NumberCueData) => NumberCueData) => {
    const before = localCue;
    if (optimistic) setLocalCue(optimistic(before));
    setBusy(true);
    setError(null);
    try {
      await operation();
      onRefresh();
      onSaved?.();
    } catch (caught) {
      setLocalCue(before);
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const chooseMaster = (childId: string) => {
    if (!childId) return;
    void run(
      async () => { await setNumberMaster(localCue.id, childId); },
      (current) => ({ ...current, number_master_id: childId }),
    );
  };

  const resetChildTimeline = (childId: string) => {
    void run(async () => {
      if (childId !== master?.id) await setNumberActionOffset(localCue.id, childId, 0);
      const child = localCue.children.find((candidate) => candidate.id === childId);
      if (child?.cue_type === "audio" || child?.cue_type === "video") {
        await updateCue(childId, { start_time_ms: null, end_time_ms: null });
      }
      setResetDone(childId);
      window.setTimeout(() => setResetDone((current) => current === childId ? null : current), 1200);
    }, (current) => ({
      ...current,
      number_action_offsets_ms: { ...current.number_action_offsets_ms, ...(childId === master?.id ? {} : { [childId]: 0 }) },
      children: current.children.map((child) => child.id === childId && (child.cue_type === "audio" || child.cue_type === "video")
        ? { ...child, start_time_ms: null, end_time_ms: null }
        : child),
    }));
  };

  const muteChildAudio = (childId: string) => {
    void run(async () => {
      // Use the cue's normal volume field. Number has no parallel mute state
      // and must never guess the previous gain when unmuting.
      await updateCue(childId, { volume_db: -60 });
    }, (current) => ({
      ...current,
      children: current.children.map((child) => child.id === childId ? { ...child, volume_db: -60 } : child),
    }));
  };

  const toggleChildLoop = (child: CueSummary) => {
    if (child.cue_type !== "audio" && child.cue_type !== "video") return;
    void run(async () => {
      await updateCue(child.id, { loop_count: (child.loop_count ?? 0) > 0 ? 0 : PLAY_COUNT_INFINITE });
    }, (current) => ({
      ...current,
      children: current.children.map((candidate) => candidate.id === child.id
        ? { ...candidate, loop_count: (child.loop_count ?? 0) > 0 ? 0 : PLAY_COUNT_INFINITE }
        : candidate),
    }));
  };

  return <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
    {error && <div role="alert" style={{ color: "#fecaca", background: "#7f1d1d66", border: "1px solid #ef444466", borderRadius: 5, padding: "7px 9px", fontSize: 11 }}>{error}</div>}

    <Section title={`1 · ${t("numberUi.master")}`}>
      <div style={{ color: "var(--wc-text-muted)", fontSize: 11, marginBottom: 8 }}>{t("numberUi.masterHint")}</div>
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <select
          value={localCue.number_master_id ?? ""}
          disabled={busy || masterCandidates.length === 0}
          aria-label={t("numberUi.master")}
          style={{ ...inputStyle, flex: 1 }}
          onChange={(event) => chooseMaster(event.target.value)}
        >
          <option value="">{t("numberUi.selectMaster")}</option>
          {masterCandidates.map((child) => <option key={child.id} value={child.id}>{child.name}</option>)}
        </select>
        {master && <StatusBadge child={master} label={t} />}
        {master && <button type="button" disabled={busy} style={{ ...inputStyle, width: "auto", cursor: "pointer" }} onClick={() => onSelectCue?.(master.id)}>{t("numberUi.openChild")}</button>}
      </div>
      {!master && <div style={{ color: "#fbbf24", fontSize: 11, marginTop: 7 }}>{t("numberUi.required")}</div>}
      {masterCandidates.length === 0 && <div style={{ color: "var(--wc-text-faint)", fontSize: 11, marginTop: 7 }}>{t("numberUi.noChildren")}</div>}
    </Section>

    <Section title={`2 · ${t("numberUi.quickActions")}`}>
      <div style={{ color: "var(--wc-text-muted)", fontSize: 11, marginBottom: 8 }}>{t("numberUi.quickActionsHint")}</div>
      {localCue.children.length === 0 && <div style={{ color: "var(--wc-text-faint)", fontSize: 11 }}>{t("numberUi.noChildren")}</div>}
      {localCue.children.map((child) => {
        const supported = isNumberActionSupported(child);
        const audioCapable = child.cue_type === "audio" || child.cue_type === "video";
        const muted = audioCapable && (child.volume_db ?? 0) <= -59;
        const childName = child.name || `#${child.number ?? child.id.slice(0, 8)}`;
        const looped = audioCapable && (child.loop_count ?? 0) > 0;
        return <div key={child.id} style={{ display: "grid", gridTemplateColumns: "18px minmax(0, 1fr) auto", alignItems: "center", gap: 6, marginBottom: 6, minWidth: 0 }}>
          <CueTypeIcon type={child.cue_type} size={15} tone="type" />
          <span title={childName} style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 11 }}>{childName}</span>
          <div style={{ display: "flex", gap: 4 }}>
            <ActionIcon label={t("numberUi.openChild")} disabled={busy} onClick={() => onSelectCue?.(child.id)}><OpenIcon /></ActionIcon>
            <ActionIcon label={looped ? t("numberUi.loopOn") : t("numberUi.loopOff")} disabled={busy || !audioCapable} tone={looped ? "success" : "default"} onClick={() => toggleChildLoop(child)}><LoopIcon active={looped} /></ActionIcon>
            <ActionIcon label={t("numberUi.resetPositionCrop")} disabled={busy || !supported} tone={resetDone === child.id ? "success" : "default"} onClick={() => resetChildTimeline(child.id)}><ResetIcon done={resetDone === child.id} /></ActionIcon>
            <ActionIcon label={muted ? t("numberUi.audioMuted") : t("numberUi.muteAudio")} disabled={busy || !audioCapable || muted} tone={muted ? "danger" : "default"} onClick={() => muteChildAudio(child.id)}><AudioIcon muted={muted} /></ActionIcon>
          </div>
        </div>;
      })}
      <div style={{ color: "var(--wc-text-muted)", fontSize: 11, marginTop: 8 }}>{t("numberUi.childrenHint")}</div>
    </Section>

    <Section title={`3 · ${t("numberUi.flowSettings")}`}>
      <div style={{ color: "var(--wc-text-muted)", fontSize: 11, marginBottom: 8 }}>{t("numberUi.flowSettingsHint")}</div>
      <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12 }}>
        <span style={{ minWidth: 150 }}>{t("numberUi.stopOnStart")}</span>
        <select
          value={stopMode}
          disabled={busy}
          style={{ ...inputStyle, flex: 1 }}
          onChange={(event) => saveFlow({ number_start_stop_mode: event.target.value as NumberStartStopMode })}
        >
          {(["none", "all", "audio", "video", "selected"] as NumberStartStopMode[]).map((mode) => <option key={mode} value={mode}>{flowLabel(mode)}</option>)}
        </select>
      </label>
      {stopMode === "selected" && <div style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 8, maxHeight: 150, overflow: "auto" }}>
        {flowCandidates.map((candidate) => {
          const checked = startStopIds.includes(candidate.id);
          return <label key={`stop-${candidate.id}`} style={{ display: "flex", alignItems: "center", gap: 7, fontSize: 11 }}>
            <input type="checkbox" checked={checked} disabled={busy} onChange={(event) => saveFlow({ number_start_stop_ids: event.target.checked ? [...startStopIds, candidate.id] : startStopIds.filter((id) => id !== candidate.id) })} />
            <CueTypeIcon type={candidate.cue_type} size={14} tone="type" />
            <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{candidate.name}</span>
          </label>;
        })}
      </div>}
      <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12, marginTop: 10 }}>
        <span style={{ minWidth: 150 }}>{t("numberUi.startAfterFinish")}</span>
      </label>
      <div style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 6, maxHeight: 150, overflow: "auto" }}>
        {flowCandidates.map((candidate) => {
          const checked = finishStartIds.includes(candidate.id);
          return <label key={`finish-${candidate.id}`} style={{ display: "flex", alignItems: "center", gap: 7, fontSize: 11 }}>
            <input type="checkbox" checked={checked} disabled={busy} onChange={(event) => saveFlow({ number_finish_start_ids: event.target.checked ? [...finishStartIds, candidate.id] : finishStartIds.filter((id) => id !== candidate.id) })} />
            <CueTypeIcon type={candidate.cue_type} size={14} tone="type" />
            <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{candidate.name}</span>
          </label>;
        })}
      </div>
    </Section>
  </div>;
}
