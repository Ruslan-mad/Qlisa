import type { CSSProperties } from "react";
import { Repeat, Volume2, VolumeX } from "lucide-react";
import type { ContinueMode, FitMode, OutputDestination, VideoGeometry } from "../../lib/types";
import { DEFAULT_GEOMETRY } from "../../lib/types";
import { useLocale } from "../../i18n";
import { Section } from "./Field";
import { FitByOutputRows } from "./GeometryTab";
import { useGeometryOutputOverrides } from "./geometryOutputModel";
import { LOOP_INFINITE, loopQuickToggleAfterCount } from "./loopModel";
import type { MultiCueRecord, ValueState } from "./multiCueModel";

type CueQuickState = {
  id: string;
  cue_type: string;
  continue_mode?: ContinueMode;
  loop_count?: number;
  muted?: boolean;
  geometry?: VideoGeometry;
  geometry_by_output?: Record<string, VideoGeometry>;
};

const MODES: ContinueMode[] = ["do_not_continue", "auto_continue", "auto_follow"];

function QuickControlsSection({
  loop, mute, continueMode, outputs, geometryByOutput, fallback, mixedOutputIds = [], disabled = false,
  loopDisabled = false, mixedLabel = "", onLoop, onMute, onContinue, onFit,
}: {
  loop?: ValueState<boolean>;
  mute?: ValueState<boolean>;
  continueMode: ValueState<ContinueMode | undefined>;
  outputs: Array<{ id: string; name: string }>;
  geometryByOutput: Record<string, VideoGeometry>;
  fallback: VideoGeometry;
  mixedOutputIds?: readonly string[];
  disabled?: boolean;
  loopDisabled?: boolean;
  mixedLabel?: string;
  onLoop?: () => void;
  onMute?: () => void;
  onContinue: (mode: ContinueMode) => void;
  onFit: (id: string, mode: FitMode) => void;
}) {
  const { locale } = useLocale();
  const isRu = locale === "ru";
  const style = (active: boolean, mixed = false, blocked = false): CSSProperties => ({
    minWidth: 29, height: 27, display: "inline-flex", alignItems: "center", justifyContent: "center", padding: "3px 7px",
    border: `1px ${mixed ? "dashed" : "solid"} ${active || mixed ? "var(--wc-accent)" : "var(--wc-border)"}`, borderRadius: 4,
    background: active ? "var(--wc-accent-soft)" : "var(--wc-bg-surface)", color: active ? "var(--wc-text)" : "var(--wc-text-muted)",
    cursor: disabled || blocked ? "default" : "pointer", opacity: disabled || blocked ? 0.55 : 1,
  });
  const modeLabel = (mode: ContinueMode) => mode === "do_not_continue" ? (isRu ? "Не продолжать" : "Do not continue") : mode === "auto_continue" ? (isRu ? "Автопродолжение" : "Auto-Continue") : (isRu ? "Автопереход" : "Auto-Follow");
  const loopOn = loop?.kind === "uniform" && loop.value;
  const muteOn = mute?.kind === "uniform" && mute.value;
  const showLoop = loop != null && loop.kind !== "empty";
  const showMute = mute != null && mute.kind !== "empty";
  const continueMixed = continueMode.kind === "mixed";
  return <Section title={isRu ? "Быстрые настройки" : "Quick controls"}>
    <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: "7px 10px" }}>
      {(showLoop || showMute) && <div style={{ display: "flex", alignItems: "center", gap: 3 }}>
        {showLoop && <button type="button" disabled={disabled || loopDisabled} aria-pressed={loop.kind === "mixed" ? "mixed" : !!loopOn} aria-label={isRu ? (loopOn ? "Выключить бесконечное зацикливание" : "Зациклить бесконечно") : (loopOn ? "Turn infinite looping off" : "Loop infinitely")} title={isRu ? "Бесконечное зацикливание" : "Infinite looping"} style={style(!!loopOn, loop.kind === "mixed", loopDisabled)} onClick={onLoop}><Repeat size={15} /><span style={{ fontSize: 12, marginLeft: 2 }}>∞</span></button>}
        {showMute && <button type="button" disabled={disabled} aria-pressed={mute.kind === "mixed" ? "mixed" : !!muteOn} aria-label={isRu ? (muteOn ? "Включить звук" : "Выключить звук") : (muteOn ? "Enable sound" : "Mute sound")} title={isRu ? "Звук" : "Sound"} style={style(!!muteOn, mute.kind === "mixed")} onClick={onMute}>{muteOn ? <VolumeX size={16} /> : <Volume2 size={16} />}</button>}
        {(loop?.kind === "mixed" || mute?.kind === "mixed") && <span style={{ fontSize: 10, color: "var(--wc-text-muted)" }}>{mixedLabel}</span>}
      </div>}
      <div role="group" aria-label={isRu ? "Продолжение" : "Continue mode"} style={{ display: "flex", alignItems: "center", gap: 3 }}>
        {MODES.map((mode) => {
          const active = continueMode.kind === "uniform" && continueMode.value === mode;
          return <button key={mode} type="button" disabled={disabled} aria-pressed={continueMixed ? "mixed" : active} aria-label={modeLabel(mode)} title={modeLabel(mode)} style={style(active, continueMixed)} onClick={() => onContinue(mode)}>
            {mode === "do_not_continue" ? <span style={{ fontSize: 15, lineHeight: 1 }}>—</span> : <svg aria-hidden="true" width="16" height="16" viewBox="0 0 17 17" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round"><path d={mode === "auto_continue" ? "M8.5 2v13" : "M2 8.5h13"} /><path d={mode === "auto_continue" ? "M3.5 10 8.5 15 13.5 10" : "M10 3.5 15 8.5 10 13.5"} /></svg>}
          </button>;
        })}
        {continueMixed && <span style={{ fontSize: 10, color: "var(--wc-text-muted)" }}>{mixedLabel}</span>}
      </div>
    </div>
    {outputs.length > 0 && <div style={{ marginTop: 8 }}>
      <div style={{ fontSize: 10, color: "var(--wc-text-muted)", marginBottom: 5 }}>{isRu ? "Вписывание по выходам" : "Fit by output"}</div>
      <div style={{ fontSize: 10, color: "var(--wc-text-faint)", marginBottom: 5 }}>{isRu ? "Каждая строка меняет только cue, назначенные на этот выход." : "Each row changes only cues assigned to that output."}</div>
      <FitByOutputRows outputs={outputs} geometryByOutput={geometryByOutput} fallback={fallback} disabled={disabled} mixedOutputIds={mixedOutputIds} mixedLabel={mixedLabel || undefined} onSelect={onFit} />
    </div>}
    {loopDisabled && showLoop && <div style={{ fontSize: 11, color: "var(--wc-warning, #fbbf24)", marginTop: 7 }}>{isRu ? "Остановите или сбросьте выбранные cue, чтобы изменить зацикливание." : "Stop or reset selected cues to change looping."}</div>}
  </Section>;
}

export function BasicQuickControls({
  cue,
  outputs,
  outputIds,
  disabled = false,
  onSave,
}: {
  cue: CueQuickState;
  outputs: OutputDestination[];
  outputIds: string[];
  disabled?: boolean;
  onSave: (patch: Record<string, unknown>) => void;
}) {
  const isAudio = cue.cue_type === "audio";
  const isVideo = cue.cue_type === "video";
  const isCamera = cue.cue_type === "camera";
  const isImage = cue.cue_type === "image";
  const canLoop = isAudio || isVideo;
  const canMute = isAudio || isVideo || isCamera;
  const canFit = isVideo || isImage || isCamera;
  const infiniteLoop = (cue.loop_count ?? 0) === LOOP_INFINITE;
  const geometry = cue.geometry ?? DEFAULT_GEOMETRY;
  const { values, saveOutput } = useGeometryOutputOverrides(
    cue.id,
    cue.geometry_by_output,
    geometry,
    (overrides) => onSave({ geometry_by_output: overrides }),
  );
  const selected = outputs.filter((output) => outputIds.includes(output.id)).map(({ id, name }) => ({ id, name }));
  const currentGeometryByOutput = Object.fromEntries(selected.map(({ id }) => [id, values[id] ?? geometry]));
  return <QuickControlsSection
    loop={canLoop ? { kind: "uniform", value: infiniteLoop } : undefined}
    mute={canMute ? { kind: "uniform", value: cue.muted ?? false } : undefined}
    continueMode={cue.continue_mode ? { kind: "uniform", value: cue.continue_mode } : { kind: "empty" }}
    outputs={canFit ? selected : []}
    geometryByOutput={currentGeometryByOutput}
    fallback={geometry}
    disabled={disabled}
    onLoop={() => onSave({ loop_count: loopQuickToggleAfterCount(cue.loop_count ?? 0) })}
    onMute={() => onSave({ muted: !(cue.muted ?? false) })}
    onContinue={(mode) => onSave({ continue_mode: mode })}
    onFit={(id, mode) => saveOutput(id, { fit_mode: mode })}
  />;
}

export function MultiBasicQuickControls({
  cues, outputs, outputIdsByCue, loopState, muteState, continueState, disabled = false,
  loopDisabled = false, fitEnabled = true, mixedLabel, onLoop, onMute, onContinue, onFit,
}: {
  cues: readonly MultiCueRecord[];
  outputs: OutputDestination[];
  outputIdsByCue: ReadonlyMap<string, readonly string[]>;
  loopState: ValueState<boolean>;
  muteState: ValueState<boolean>;
  continueState: ValueState<ContinueMode | undefined>;
  disabled?: boolean;
  loopDisabled?: boolean;
  fitEnabled?: boolean;
  mixedLabel: string;
  onLoop: (enabled: boolean) => void;
  onMute: (muted: boolean) => void;
  onContinue: (mode: ContinueMode) => void;
  onFit: (outputId: string, mode: FitMode) => void;
}) {
  const selectedIds = fitEnabled ? [...new Set([...outputIdsByCue.values()].flat())] : [];
  const selected = outputs.filter((output) => selectedIds.includes(output.id)).map(({ id, name }) => ({ id, name }));
  const fits = selected.map((output) => {
    const values = cues.flatMap((cue) => {
      if (!outputIdsByCue.get(cue.id)?.includes(output.id)) return [];
      const overrides = cue.geometry_by_output as Record<string, VideoGeometry> | undefined;
      return [{ ...DEFAULT_GEOMETRY, ...(cue.geometry as VideoGeometry | undefined), ...(overrides?.[output.id] ?? {}) }];
    });
    return { output, values };
  });
  const mixedOutputIds = fits.filter(({ values }) => new Set(values.map((value) => value.fit_mode)).size > 1).map(({ output }) => output.id);
  const geometryByOutput = Object.fromEntries(fits.map(({ output, values }) => [output.id, values[0] ?? DEFAULT_GEOMETRY]));
  return <QuickControlsSection
    loop={loopState}
    mute={muteState}
    continueMode={continueState}
    outputs={selected}
    geometryByOutput={geometryByOutput}
    fallback={DEFAULT_GEOMETRY}
    mixedOutputIds={mixedOutputIds}
    disabled={disabled}
    loopDisabled={loopDisabled}
    mixedLabel={mixedLabel}
    onLoop={() => onLoop(!(loopState.kind === "uniform" && loopState.value))}
    onMute={() => onMute(!(muteState.kind === "uniform" && muteState.value))}
    onContinue={onContinue}
    onFit={onFit}
  />;
}
