import { open } from "@tauri-apps/plugin-dialog";
import { Pause, Play, Stop } from "@phosphor-icons/react";
import { getOutputPatches, goCue, pauseCue, resumeCue, stopCue } from "../../lib/commands";
import type { CueSummary, OutputPatch, TimerCueData } from "../../lib/types";
import { useLocale } from "../../i18n";
import { Field, Grid2, MiniField, NumberInput, Section, ToggleRow, inputStyle } from "./Field";
import { listSystemFonts } from "../../lib/commands";
import { useEffect, useState } from "react";
import { useRef } from "react";
import { useTimingStore } from "../../stores/timingStore";
import { applyTimerDurationDelta, createTimerDurationSaveQueue, enqueueTimerDurationSave } from "./timerDurationSteps";
import { formatTimerDurationInput, parseTimerDurationInput } from "./timerDurationInput";
import { timerPlayButtonModel } from "./timerTransportModel";

const PRESETS = ["notification_1"] as const;
const transportButton: React.CSSProperties = {
  width: 32, height: 30, display: "inline-flex", alignItems: "center", justifyContent: "center", padding: 0,
  background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
  borderRadius: 4, color: "var(--wc-text)", cursor: "pointer",
};

export function TimerTab({ cue, selectedCue, onSave, onRefresh }: {
  cue: TimerCueData; selectedCue: CueSummary | null;
  onSave: (patch: Partial<TimerCueData>) => Promise<void>; onRefresh: () => void;
}) {
  const { t } = useLocale();
  const timing = useTimingStore((store) => store.timings[cue.id]);
  const [fonts, setFonts] = useState<string[]>([]);
  const [outputPatches, setOutputPatches] = useState<OutputPatch[]>([]);
  const [busy, setBusy] = useState(false);
  const [commandError, setCommandError] = useState<string | null>(null);
  const [durationDraft, setDurationDraft] = useState(() => formatTimerDurationInput(cue.timer_duration_ms ?? 60000));
  const durationQueueRef = useRef(createTimerDurationSaveQueue(cue.id, cue.timer_duration_ms ?? 60000));
  if (durationQueueRef.current.cueId !== cue.id) durationQueueRef.current = createTimerDurationSaveQueue(cue.id, cue.timer_duration_ms ?? 60000);
  const state = selectedCue?.state ?? "standby";
  const playButton = timerPlayButtonModel(state, timing?.timer_zero_reached === true, timing?.wait_phase ?? null);
  useEffect(() => {
    listSystemFonts().then(setFonts).catch(() => {});
    getOutputPatches().then(setOutputPatches).catch(() => {});
  }, []);
  useEffect(() => {
    const queue = durationQueueRef.current;
    if (queue.pending === 0) {
      queue.value = cue.timer_duration_ms ?? 60000;
      setDurationDraft(formatTimerDurationInput(queue.value));
    }
  }, [cue.id, cue.timer_duration_ms]);
  const saveDuration = (durationMs: number) => {
    const queue = durationQueueRef.current;
    setDurationDraft(formatTimerDurationInput(durationMs));
    setCommandError(null);
    enqueueTimerDurationSave(queue, durationMs, (value) => onSave({ timer_duration_ms: value }));
    void queue.queue.catch((error) => setCommandError(String(error)));
  };
  const commitDurationDraft = () => {
    const value = parseTimerDurationInput(durationDraft);
    if (value == null) {
      setCommandError(t("timerUi.invalidDuration"));
      return;
    }
    saveDuration(value);
  };
  const command = async (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setCommandError(null);
    try { await durationQueueRef.current.queue; await action(); onRefresh(); }
    catch (error) { setCommandError(String(error)); }
    finally { setBusy(false); }
  };
  const startOrRestart = (action: () => Promise<void>) => {
    const durationMs = parseTimerDurationInput(durationDraft);
    if (durationMs == null) {
      setCommandError(t("timerUi.invalidDuration"));
      return;
    }
    // onBlur usually queues this first. Keep the guard here too, so Start never
    // fires with the previous duration if blur did not commit the current draft.
    if (durationMs !== durationQueueRef.current.value) saveDuration(durationMs);
    void command(action);
  };
  const pickSound = async (field: "sound_start_path" | "sound_end_path") => {
    const path = await open({ multiple: false, filters: [{ name: t("cueTypes.audio"), extensions: ["wav", "mp3", "flac", "ogg", "m4a"] }] });
    if (typeof path === "string") await onSave({ [field]: path } as Partial<TimerCueData>);
  };
  return <>
    <Section title={t("timerUi.controls")}>
      <div style={{ marginBottom: 10, minWidth: 0 }}>
        <div style={{ fontSize: 12, color: "var(--wc-text-secondary)", marginBottom: 5 }}>{t("timerUi.duration")}</div>
        <div style={{ display: "grid", gridTemplateColumns: "minmax(0,1fr) minmax(0,1fr) 72px minmax(0,1fr) minmax(0,1fr)", gap: 4, width: "100%", minWidth: 0, alignItems: "center" }}>
          {[-60_000, -10_000].map((delta) => <button key={delta} type="button" style={{ ...transportButton, boxSizing: "border-box", width: "100%", minWidth: 0, padding: "0 2px", fontSize: 11, whiteSpace: "nowrap" }}
            onClick={() => saveDuration(applyTimerDurationDelta(durationQueueRef.current.value, delta))}>
            {delta < 0 ? "−" : "+"}{Math.abs(delta) === 60_000 ? t("timerUi.oneMinute") : `${Math.abs(delta) / 1000} ${t("timerUi.secondsShort")}`}
          </button>)}
        <input type="text" inputMode="text" value={durationDraft} aria-label={t("timerUi.duration")}
          aria-invalid={commandError === t("timerUi.invalidDuration")}
          onChange={(e) => { setDurationDraft(e.target.value); setCommandError(null); }}
          onBlur={commitDurationDraft}
          onKeyDown={(e) => {
            if (e.key === "Enter") { e.preventDefault(); e.currentTarget.blur(); }
            if (e.key === "Escape") { setCommandError(null); setDurationDraft(formatTimerDurationInput(durationQueueRef.current.value)); }
          }}
          style={{ ...inputStyle, boxSizing: "border-box", width: "100%", minWidth: 0, height: 30, padding: "4px 3px", textAlign: "center", fontFamily: "monospace", fontVariantNumeric: "tabular-nums" }} />
          {[10_000, 60_000].map((delta) => <button key={delta} type="button" style={{ ...transportButton, boxSizing: "border-box", width: "100%", minWidth: 0, padding: "0 2px", fontSize: 11, whiteSpace: "nowrap" }}
            onClick={() => saveDuration(applyTimerDurationDelta(durationQueueRef.current.value, delta))}>
          {delta < 0 ? "−" : "+"}{Math.abs(delta) === 60_000 ? t("timerUi.oneMinute") : `${Math.abs(delta) / 1000} ${t("timerUi.secondsShort")}`}
        </button>)}
      </div>
      </div>
      <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
        <button type="button" title={t(`timerUi.${playButton.label}`)} aria-label={t(`timerUi.${playButton.label}`)}
          style={transportButton} disabled={busy}
          onClick={() => {
            if (playButton.action === "pause") void command(() => pauseCue(cue.id));
            else if (playButton.action === "resume") startOrRestart(() => resumeCue(cue.id));
            else startOrRestart(() => goCue(cue.id));
          }}>
          {playButton.icon === "pause" ? <Pause size={16} weight="fill" /> : <Play size={16} weight="fill" />}
        </button>
        <button type="button" title={t("timerUi.stop")} aria-label={t("timerUi.stop")}
          style={{ ...transportButton, color: "#ef4444" }} disabled={busy}
          onClick={() => void command(() => stopCue(cue.id))}>
          <Stop size={16} weight="fill" />
        </button>
      </div>
      {commandError && <div role="alert" style={{ color: "var(--wc-danger, #e56b6f)", fontSize: 11, marginTop: 6 }}>{commandError}</div>}
    </Section>

    <Section title={t("timerUi.behavior")}>
      <Field label={t("timerUi.mode")}>
        <select value={cue.mode} onChange={(e) => void onSave({ mode: e.target.value as TimerCueData["mode"] })} style={inputStyle}>
          <option value="countdown">{t("timerUi.countdown")}</option><option value="countup">{t("timerUi.countup")}</option>
        </select>
      </Field>
      <Field label={t("timerUi.format")}>
        <select value={cue.format} onChange={(e) => void onSave({ format: e.target.value as TimerCueData["format"] })} style={inputStyle}>
          <option value="minutes_seconds">MM:SS</option><option value="seconds">{t("timerUi.seconds")}</option>
        </select>
      </Field>
      <ToggleRow label={t("timerUi.finishAndHide")} checked={cue.finish_and_hide} onToggle={(finish_and_hide) => void onSave({ finish_and_hide })} />
    </Section>

    <Section title={t("timerUi.appearance")}>
      <Field label={t("inspector.font")}>
        <select value={cue.font} onChange={(e) => void onSave({ font: e.target.value })} style={inputStyle}>{(fonts.length ? fonts : [cue.font]).map((font) => <option key={font}>{font}</option>)}</select>
      </Field>
      <Grid2>
        <MiniField label={t("inspector.fontSize")}><NumberInput value={cue.font_size} step={1} min={8} max={300} onCommit={(v) => void onSave({ font_size: Math.round(v) })} /></MiniField>
        <MiniField label={t("inspector.color")}><input type="color" value={cue.text_color} onChange={(e) => void onSave({ text_color: e.target.value })} style={{ ...inputStyle, padding: 2, height: 29 }} /></MiniField>
      </Grid2>
    </Section>

    {(["start", "end"] as const).map((edge) => {
      const enabledKey = edge === "start" ? "sound_start" : "sound_end";
      const presetKey = edge === "start" ? "sound_start_preset" : "sound_end_preset";
      const pathKey = edge === "start" ? "sound_start_path" : "sound_end_path";
      return <Section key={edge} title={edge === "start" ? t("timerUi.startSound") : t("timerUi.endSound")}>
        <ToggleRow label={t("timerUi.enabled")} checked={cue[enabledKey]} onToggle={(value) => void onSave({ [enabledKey]: value } as Partial<TimerCueData>)} />
        <Field label={t("timerUi.sound")}>
          <select value={cue[presetKey] ?? "notification_1"} onChange={(e) => void onSave({ [presetKey]: e.target.value, [pathKey]: null } as Partial<TimerCueData>)} style={inputStyle}>
            {PRESETS.map((preset) => <option key={preset} value={preset}>{t(`timerUi.preset.${preset}`)}</option>)}
            <option value="custom">{t("timerUi.customFile")}</option>
          </select>
        </Field>
        {cue[presetKey] === "custom" && <Field label={t("timerUi.file")}><div style={{ display: "flex", gap: 6 }}><input readOnly value={cue[pathKey] ?? ""} style={inputStyle} /><button onClick={() => void pickSound(pathKey)}>{t("common.choose")}</button></div></Field>}
      </Section>;
    })}
    <Section title={t("timerUi.soundSettings")}>
      <Field label={t("inspector.volume")}><NumberInput value={cue.sound_volume * 100} step={1} min={0} max={100} onCommit={(v) => void onSave({ sound_volume: v / 100 })} /></Field>
      <Field label={t("timerUi.soundOutput")}>
        <select value={outputPatches.find((patch) => patch.id === cue.sound_patch_id)?.kind === "main" ? "" : (cue.sound_patch_id ?? "")} onChange={(e) => void onSave({ sound_patch_id: e.target.value || null })} style={inputStyle}>
          <option value="">— {t("audioBusUi.main")} —</option>
          {outputPatches.filter((patch) => patch.kind === "aux" && patch.enabled).map((patch) => <option key={patch.id} value={patch.id}>{patch.name}</option>)}
        </select>
      </Field>
    </Section>
  </>;
}
