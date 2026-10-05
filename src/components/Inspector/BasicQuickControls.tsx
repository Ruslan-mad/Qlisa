import type { CSSProperties } from "react";
import { Repeat, Volume2, VolumeX } from "lucide-react";
import type { ContinueMode, FitMode, OutputDestination, VideoGeometry } from "../../lib/types";
import { DEFAULT_GEOMETRY } from "../../lib/types";
import { useLocale } from "../../i18n";
import { Section } from "./Field";
import { FitByOutputRows } from "./GeometryTab";
import { useGeometryOutputOverrides } from "./geometryOutputModel";
import { LOOP_INFINITE, loopQuickToggleAfterCount } from "./loopModel";

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
  const { locale } = useLocale();
  const isRu = locale === "ru";
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
  const groupStyle: CSSProperties = { display: "flex", alignItems: "center", gap: 3 };
  const buttonStyle = (active: boolean): CSSProperties => ({
    minWidth: 29, height: 27, display: "inline-flex", alignItems: "center", justifyContent: "center",
    padding: "3px 7px", border: `1px solid ${active ? "var(--wc-accent)" : "var(--wc-border)"}`,
    borderRadius: 4, background: active ? "var(--wc-accent-soft)" : "var(--wc-bg-surface)",
    color: active ? "var(--wc-text)" : "var(--wc-text-muted)", cursor: disabled ? "default" : "pointer",
    opacity: disabled ? 0.55 : 1,
  });
  const modeLabel = (mode: ContinueMode) => mode === "do_not_continue" ? (isRu ? "Не продолжать" : "Do not continue")
    : mode === "auto_continue" ? (isRu ? "Автопродолжение" : "Auto-Continue")
    : (isRu ? "Автопереход" : "Auto-Follow");

  return <Section title={isRu ? "Быстрые настройки" : "Quick controls"}>
    <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: "7px 10px" }}>
      {(canLoop || canMute) && <div style={groupStyle} aria-label={isRu ? "Звук и зацикливание" : "Sound and loop"}>
        {canLoop && <button type="button" disabled={disabled} aria-pressed={infiniteLoop} aria-label={infiniteLoop ? (isRu ? "Выключить бесконечное зацикливание" : "Turn infinite looping off") : (isRu ? "Зациклить бесконечно" : "Loop infinitely")} title={infiniteLoop ? (isRu ? "Выключить бесконечное зацикливание" : "Turn infinite looping off") : (isRu ? "Зациклить бесконечно" : "Loop infinitely")} style={buttonStyle(infiniteLoop)} onClick={() => onSave({ loop_count: loopQuickToggleAfterCount(cue.loop_count ?? 0) })}><Repeat size={15} /><span style={{ fontSize: 12, marginLeft: 2 }}>∞</span></button>}
        {canMute && <button type="button" disabled={disabled} aria-pressed={cue.muted ?? false} aria-label={(cue.muted ?? false) ? (isRu ? "Включить звук" : "Enable sound") : (isRu ? "Выключить звук" : "Mute sound")} title={(cue.muted ?? false) ? (isRu ? "Включить звук" : "Enable sound") : (isRu ? "Выключить звук" : "Mute sound")} style={buttonStyle(cue.muted ?? false)} onClick={() => onSave({ muted: !(cue.muted ?? false) })}>{(cue.muted ?? false) ? <VolumeX size={16} /> : <Volume2 size={16} />}</button>}
      </div>}
      <div role="group" aria-label={isRu ? "Продолжение" : "Continue mode"} style={groupStyle}>
        {MODES.map((mode) => <button key={mode} type="button" disabled={disabled} aria-pressed={cue.continue_mode === mode} aria-label={modeLabel(mode)} title={modeLabel(mode)} style={buttonStyle(cue.continue_mode === mode)} onClick={() => onSave({ continue_mode: mode })}>
          {mode === "do_not_continue" ? <span style={{ fontSize: 15, lineHeight: 1 }}>—</span> : <svg aria-hidden="true" width="16" height="16" viewBox="0 0 17 17" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round"><path d={mode === "auto_continue" ? "M8.5 2v13" : "M2 8.5h13"} /><path d={mode === "auto_continue" ? "M3.5 10 8.5 15 13.5 10" : "M10 3.5 15 8.5 10 13.5"} /></svg>}
        </button>)}
      </div>
    </div>
    {canFit && selected.length > 0 && <div style={{ marginTop: 8 }}>
      <div style={{ fontSize: 10, color: "var(--wc-text-muted)", marginBottom: 5 }}>{isRu ? "Вписывание по выходам" : "Fit by output"}</div>
      <FitByOutputRows outputs={selected} geometryByOutput={currentGeometryByOutput} fallback={geometry} disabled={disabled} onSelect={(id: string, fitMode: FitMode) => saveOutput(id, { fit_mode: fitMode })} />
    </div>}
  </Section>;
}
