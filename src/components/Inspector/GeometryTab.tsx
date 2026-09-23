// Geometry tab for visual cues: fit mode, position, scale, rotation and crop.
// Edits apply live when the cue is on the output window. Compositing controls
// (layer order / opacity / blend) live in the Layer tab.

import type { CameraCueData, FitMode, ImageCueData, VideoCueData, VideoGeometry } from "../../lib/types";
import { DEFAULT_GEOMETRY } from "../../lib/types";
import { Grid2, MiniField, NumberInput, Section, Segmented, SliderRow } from "./Field";
import { useLocale } from "../../i18n";
import type { ValueState } from "./multiCueModel";

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
  onPatch,
  onReset,
}: {
  geometry: GeometryEditorState;
  disabled?: boolean;
  mixedLabel?: string;
  commitSlidersOnRelease?: boolean;
  resetDisabled?: boolean;
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
      <Section title="Fit" hint={t("help.outputSelection")}>
        <Segmented
          options={FIT_MODES.map((mode) => ({ ...mode, label: fitLabels[mode.value], hint: fitHints[mode.value] }))}
          value={fitMode}
          disabled={disabled}
          onChange={(value) => onPatch({ fit_mode: value })}
        />
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginTop: -4, marginBottom: 6 }}>
          {fitMode ? fitHints[fitMode] : mixedLabel}
        </div>
      </Section>

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
}: {
  cue: VideoCueData | ImageCueData | CameraCueData;
  onSave: (p: Partial<VideoCueData | ImageCueData | CameraCueData>) => void;
}) {
  const geometry: VideoGeometry = cue.geometry ?? DEFAULT_GEOMETRY;
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
