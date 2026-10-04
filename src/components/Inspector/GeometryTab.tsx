// Geometry tab for visual cues: fit mode, position, scale, rotation and crop.
// Edits apply live when the cue is on the output window. Compositing controls
// (layer order / opacity / blend) live in the Layer tab.

import { useEffect, useRef, useState, type CSSProperties } from "react";
import type { CameraCueData, FitMode, ImageCueData, OutputDestination, VideoCueData, VideoGeometry } from "../../lib/types";
import { DEFAULT_GEOMETRY } from "../../lib/types";
import { Grid2, MiniField, NumberInput, Section, Segmented, SliderRow } from "./Field";
import { useLocale } from "../../i18n";
import type { ValueState } from "./multiCueModel";
import { geometryOverridesForCue, mergeGeometryOutputOverride } from "./geometryOutputModel";

const FIT_MODES: { value: FitMode; label: string; hint: string }[] = [
  { value: "fit", label: "Fit", hint: "keep aspect, letterbox" },
  { value: "fill", label: "Fill", hint: "keep aspect, crop overflow" },
  { value: "stretch", label: "Stretch", hint: "ignore aspect ratio" },
];

export type GeometryEditorState = {
  [K in keyof VideoGeometry]: ValueState<VideoGeometry[K]>;
};
type GeometryNumberKey = Exclude<keyof VideoGeometry, "fit_mode">;

function uniform<T>(value: T): ValueState<T> {
  return { kind: "uniform", value };
}

function stateValue<T>(state: ValueState<T>): T | null {
  return state.kind === "uniform" ? state.value : null;
}

export function GeometryEditor({
  geometry,
  disabled = false,
  mixedLabel,
  commitSlidersOnRelease = false,
  resetDisabled = false,
  hideFit = false,
  onPatch,
  onReset,
}: {
  geometry: GeometryEditorState;
  disabled?: boolean;
  mixedLabel?: string;
  commitSlidersOnRelease?: boolean;
  resetDisabled?: boolean;
  hideFit?: boolean;
  onPatch: (partial: Partial<VideoGeometry>) => void;
  onReset: () => void;
}) {
  const { t, locale } = useLocale();
  const fitLabels: Record<FitMode, string> = locale === "ru" ? { fit: "Вписать", fill: "Вписать с обрезкой", stretch: "Растянуть" } : { fit: "Fit", fill: "Fill", stretch: "Stretch" };
  const fitHints: Record<FitMode, string> = locale === "ru" ? { fit: "сохранить пропорции, добавить поля", fill: "сохранить пропорции, обрезать выходящее за границы", stretch: "игнорировать соотношение сторон" } : { fit: "keep aspect, letterbox", fill: "keep aspect, crop overflow", stretch: "ignore aspect ratio" };
  const fitMode = stateValue(geometry.fit_mode);
  const numberField = (key: GeometryNumberKey, step: number, min: number, max: number) => (
    <NumberInput
      value={stateValue(geometry[key])}
      placeholder={geometry[key].kind === "mixed" ? mixedLabel : undefined}
      disabled={disabled}
      step={step}
      min={min}
      max={max}
      onCommit={(value) => onPatch({ [key]: key === "rotation" ? Math.round(value) : value })}
    />
  );

  return (
    <>
      {!hideFit && <Section title="Fit" hint={t("help.outputSelection")}>
        <Segmented
          options={FIT_MODES.map((mode) => ({ ...mode, label: fitLabels[mode.value], hint: fitHints[mode.value] }))}
          value={fitMode}
          disabled={disabled}
          onChange={(value) => onPatch({ fit_mode: value })}
        />
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginTop: -4, marginBottom: 6 }}>
          {fitMode ? fitHints[fitMode] : mixedLabel}
        </div>
      </Section>}

      <Section title="Position & Scale">
        <Grid2>
          <MiniField label={locale === "ru" ? "Положение X (± ширины)" : "Position X (± of width)"}>
            {numberField("pan_x", 0.01, -1, 1)}
          </MiniField>
          <MiniField label={locale === "ru" ? "Положение Y (± высоты)" : "Position Y (± of height)"}>
            {numberField("pan_y", 0.01, -1, 1)}
          </MiniField>
        </Grid2>
        <SliderRow
          label={t("output.scale")}
          value={stateValue(geometry.scale)}
          mixedLabel={mixedLabel}
          disabled={disabled}
          commitOnRelease={commitSlidersOnRelease}
          min={0.05}
          max={8}
          step={0.05}
          format={(value) => `${Math.round(value * 100)}%`}
          onChange={(value) => onPatch({ scale: value })}
        />
        <SliderRow
          label={t("output.rotation")}
          value={stateValue(geometry.rotation)}
          mixedLabel={mixedLabel}
          disabled={disabled}
          commitOnRelease={commitSlidersOnRelease}
          min={0}
          max={359}
          step={1}
          format={(value) => `${Math.round(value)}°`}
          onChange={(value) => onPatch({ rotation: Math.round(value) })}
        />
      </Section>

      <Section title={t("output.crop")} hint="0 – 0.45">
        <Grid2>
          <MiniField label={locale === "ru" ? "Слева" : "Left"}>{numberField("crop_left", 0.01, 0, 0.45)}</MiniField>
          <MiniField label={locale === "ru" ? "Справа" : "Right"}>{numberField("crop_right", 0.01, 0, 0.45)}</MiniField>
          <MiniField label={locale === "ru" ? "Сверху" : "Top"}>{numberField("crop_top", 0.01, 0, 0.45)}</MiniField>
          <MiniField label={locale === "ru" ? "Снизу" : "Bottom"}>{numberField("crop_bottom", 0.01, 0, 0.45)}</MiniField>
        </Grid2>
      </Section>

      <button
        disabled={disabled || resetDisabled}
        onClick={onReset}
        style={{
          padding: "5px 14px",
          fontSize: 12,
          borderRadius: 4,
          border: "1px solid var(--wc-border-strong)",
          background: "var(--wc-bg-surface)",
          color: disabled || resetDisabled ? "var(--wc-text-faint)" : "var(--wc-text)",
          cursor: disabled || resetDisabled ? "default" : "pointer",
        }}
      >
        {t("actions.reset")}
      </button>
    </>
  );
}

export function GeometryTab({
  cue,
  onSave,
  outputs = [],
  outputIds = [],
}: {
  cue: VideoCueData | ImageCueData | CameraCueData;
  onSave: (p: Partial<VideoCueData | ImageCueData | CameraCueData>) => void;
  outputs?: OutputDestination[];
  outputIds?: string[];
}) {
  const { locale } = useLocale();
  const geometry: VideoGeometry = cue.geometry ?? DEFAULT_GEOMETRY;
  const initialOverrides = { cueId: cue.id, values: cue.geometry_by_output ?? {} };
  const [localOverrides, setLocalOverrides] = useState(initialOverrides);
  const geometryOverrides = useRef(initialOverrides);
  useEffect(() => {
    const synced = { cueId: cue.id, values: cue.geometry_by_output ?? {} };
    geometryOverrides.current = synced;
    setLocalOverrides(synced);
  }, [cue.id, cue.geometry_by_output]);
  const activeOverrides = geometryOverridesForCue(localOverrides, cue.id, cue.geometry_by_output);
  const [activeOutput, setActiveOutput] = useState(outputIds[0] ?? "");
  useEffect(() => {
    if (outputIds.length > 0 && !outputIds.includes(activeOutput)) setActiveOutput(outputIds[0]);
  }, [outputIds.join("\u0000"), activeOutput]);
  const selectedOutputs = outputIds.map((id) => ({ id, name: outputs.find((output) => output.id === id)?.name ?? id }));
  const outputGeometry = (id: string) => activeOverrides[id] ?? geometry;
  const saveOutputGeometry = (id: string, partial: Partial<VideoGeometry>) => {
    const current = geometryOverridesForCue(geometryOverrides.current, cue.id, cue.geometry_by_output);
    const overrides = mergeGeometryOutputOverride(current, id, geometry, partial);
    const pending = { cueId: cue.id, values: overrides };
    geometryOverrides.current = pending;
    setLocalOverrides(pending);
    onSave({ geometry_by_output: overrides });
  };
  if (selectedOutputs.length > 0) {
    const fitButtonStyle = (selected: boolean): CSSProperties => ({
      flex: 1, minWidth: 0, padding: "5px 4px", fontSize: 11,
      border: `1px solid ${selected ? "var(--wc-accent)" : "var(--wc-border)"}`,
      borderRadius: 4, background: selected ? "var(--wc-accent-soft)" : "var(--wc-bg-surface)",
      color: selected ? "var(--wc-text)" : "var(--wc-text-muted)", cursor: "pointer",
    });
    const active = selectedOutputs.find((output) => output.id === activeOutput) ?? selectedOutputs[0];
    const stateFor = (id: string): GeometryEditorState => {
      const value = outputGeometry(id);
      return Object.fromEntries(Object.keys(DEFAULT_GEOMETRY).map((key) => [key, uniform(value[key as keyof VideoGeometry])])) as GeometryEditorState;
    };
    const resetActive = () => {
      const current = geometryOverridesForCue(geometryOverrides.current, cue.id, cue.geometry_by_output);
      const next = { ...current, [active.id]: { ...DEFAULT_GEOMETRY } };
      const pending = { cueId: cue.id, values: next };
      geometryOverrides.current = pending;
      setLocalOverrides(pending);
      onSave({ geometry_by_output: next });
    };
    return <>
      <Section title={localeLabel("fit-title", locale)}>
        <div style={{ display: "grid", gap: 7 }}>
          {selectedOutputs.map((output) => (
            <div key={output.id} style={{ display: "grid", gridTemplateColumns: "minmax(72px, 0.8fr) 2fr", gap: 7, alignItems: "center" }}>
              <span title={output.name} style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 11, color: "var(--wc-text-muted)" }}>{output.name}</span>
              <div style={{ display: "flex", gap: 4 }}>
                {FIT_MODES.map((mode) => <button key={mode.value} type="button" title={mode.hint} style={fitButtonStyle(outputGeometry(output.id).fit_mode === mode.value)} onClick={() => saveOutputGeometry(output.id, { fit_mode: mode.value })}>{localeLabel(mode.value, locale)}</button>)}
              </div>
            </div>
          ))}
        </div>
      </Section>
      <Section title={localeLabel("output", locale)}>
        <div role="tablist" aria-label={localeLabel("output", locale)} style={{ display: "flex", gap: 5, overflowX: "auto", marginBottom: 8 }}>
          {selectedOutputs.map((output) => <button key={output.id} type="button" role="tab" aria-selected={active.id === output.id} onClick={() => setActiveOutput(output.id)} style={{ ...fitButtonStyle(active.id === output.id), flex: "0 0 auto", maxWidth: 140 }}>{output.name}</button>)}
        </div>
        <GeometryEditor
          geometry={stateFor(active.id)}
          hideFit
          resetDisabled={JSON.stringify(outputGeometry(active.id)) === JSON.stringify(DEFAULT_GEOMETRY)}
          onPatch={(partial) => saveOutputGeometry(active.id, partial)}
          onReset={resetActive}
        />
      </Section>
    </>;
  }
  const patch = (partial: Partial<VideoGeometry>) =>
    onSave({ geometry: { ...geometry, ...partial } });

  const isDefault =
    geometry.fit_mode === "fit" &&
    geometry.pan_x === 0 &&
    geometry.pan_y === 0 &&
    geometry.scale === 1 &&
    geometry.rotation === 0 &&
    geometry.crop_left === 0 &&
    geometry.crop_right === 0 &&
    geometry.crop_top === 0 &&
    geometry.crop_bottom === 0;

  return <GeometryEditor
    geometry={{
      fit_mode: uniform(geometry.fit_mode),
      pan_x: uniform(geometry.pan_x),
      pan_y: uniform(geometry.pan_y),
      scale: uniform(geometry.scale),
      rotation: uniform(geometry.rotation),
      crop_left: uniform(geometry.crop_left),
      crop_right: uniform(geometry.crop_right),
      crop_top: uniform(geometry.crop_top),
      crop_bottom: uniform(geometry.crop_bottom),
    }}
    resetDisabled={isDefault}
    onPatch={patch}
    onReset={() => onSave({ geometry: { ...DEFAULT_GEOMETRY } })}
  />;
}

function localeLabel(key: string, locale: string): string {
  if (key === "fit-title") return locale === "ru" ? "Вписать по выходам" : "Fit by output";
  if (key === "output") return locale === "ru" ? "Выход" : "Output";
  if (locale !== "ru") return ({ fit: "Fit", fill: "Fill", stretch: "Stretch" } as Record<string, string>)[key] ?? key;
  if (key === "fit") return "Вписать";
  if (key === "fill") return "Обрезать";
  if (key === "stretch") return "Растянуть";
  return key;
}
