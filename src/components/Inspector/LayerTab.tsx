// Layer tab for visual cues (Video / Image / Camera): how this cue composites
// with the other layers on the output — stacking order, opacity, blend mode.

import type { BlendMode, CameraCueData, ImageCueData, LayerStyle, VideoCueData } from "../../lib/types";
import { DEFAULT_LAYER_STYLE } from "../../lib/types";
import { NumberInput, Section, SliderRow, ToggleRow, inputStyle } from "./Field";
import { Select } from "../common/Select";
import { useLocale } from "../../i18n";
import type { ValueState } from "./multiCueModel";

const BLEND_MODES: { value: BlendMode; label: string }[] = [
  { value: "normal", label: "Normal" },
  { value: "add", label: "Add" },
  { value: "multiply", label: "Multiply" },
  { value: "screen", label: "Screen" },
  { value: "overlay", label: "Overlay" },
  { value: "soft_light", label: "Soft Light" },
  { value: "hard_light", label: "Hard Light" },
  { value: "darken", label: "Darken" },
  { value: "lighten", label: "Lighten" },
  { value: "color_dodge", label: "Color Dodge" },
  { value: "color_burn", label: "Color Burn" },
  { value: "difference", label: "Difference" },
  { value: "exclusion", label: "Exclusion" },
  { value: "subtract", label: "Subtract" },
];

const MIXED_BLEND = "__mixed_blend__";

function uniform<T>(value: T): ValueState<T> {
  return { kind: "uniform", value };
}

export function LayerEditor({
  automatic,
  layer,
  opacity,
  blendMode,
  disabled = false,
  mixedLabel,
  commitSlidersOnRelease = false,
  onPatch,
}: {
  automatic: ValueState<boolean>;
  layer: ValueState<number | null>;
  opacity: ValueState<number>;
  blendMode: ValueState<BlendMode>;
  disabled?: boolean;
  mixedLabel?: string;
  commitSlidersOnRelease?: boolean;
  onPatch: (partial: Partial<LayerStyle>) => void;
}) {
  const { t, locale } = useLocale();
  const blendLabels: Record<BlendMode, string> = locale === "ru"
    ? { normal: "Обычный", add: "Сложение", multiply: "Умножение", screen: "Экран", overlay: "Перекрытие", soft_light: "Мягкий свет", hard_light: "Жёсткий свет", darken: "Затемнение", lighten: "Осветление", color_dodge: "Осветление с усилением", color_burn: "Затемнение с усилением", difference: "Разница", exclusion: "Исключение", subtract: "Вычитание" }
    : Object.fromEntries(BLEND_MODES.map((mode) => [mode.value, mode.label])) as Record<BlendMode, string>;
  const automaticValue = automatic.kind === "uniform" ? automatic.value : null;
  const layerValue = layer.kind === "uniform" ? layer.value : null;
  const opacityValue = opacity.kind === "uniform" ? opacity.value : null;
  const blendValue = blendMode.kind === "uniform" ? blendMode.value : MIXED_BLEND;

  return (
    <Section
      title={t("inspector.compositing")}
      hint={locale === "ru" ? "Визуальные cue располагаются слоями на выходе; изменения применяются сразу." : "Visual cues stack as layers on the output; changes apply live."}
    >
      <ToggleRow
        label={t("components.automaticLayer")}
        checked={automaticValue}
        mixedLabel={mixedLabel}
        disabled={disabled}
        onToggle={(value) => onPatch({ layer: value ? null : 500 })}
      />
      {automaticValue !== true && (
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10, paddingLeft: 23 }}>
          <span style={{ fontSize: 12, color: "var(--wc-text-secondary)" }}>{t("inspector.layerHint")}</span>
          <NumberInput
            value={layerValue}
            placeholder={layer.kind === "mixed" ? mixedLabel : undefined}
            disabled={disabled}
            step={1}
            min={1}
            max={1000}
            width={80}
            onCommit={(value) => onPatch({ layer: Math.round(value) })}
          />
        </div>
      )}
      <SliderRow
        label="Opacity"
        value={opacityValue}
        mixedLabel={mixedLabel}
        disabled={disabled}
        commitOnRelease={commitSlidersOnRelease}
        min={0}
        max={1}
        step={0.01}
        format={(value) => `${Math.round(value * 100)}%`}
        onChange={(value) => onPatch({ opacity: value })}
      />
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10 }}>
        <label style={{ width: 100, color: "var(--wc-text-secondary)", flexShrink: 0, fontSize: 12 }}>
          {t("components.blendMode")}
        </label>
        <Select
          style={{ ...inputStyle, cursor: disabled ? "default" : "pointer" }}
          value={blendValue}
          disabled={disabled}
          onChange={(event) => { if (event.target.value !== MIXED_BLEND) onPatch({ blend_mode: event.target.value as BlendMode }); }}
        >
          {blendMode.kind === "mixed" && <option value={MIXED_BLEND} disabled>{mixedLabel}</option>}
          {BLEND_MODES.map((mode) => (
            <option key={mode.value} value={mode.value}>{blendLabels[mode.value]}</option>
          ))}
        </Select>
      </div>
    </Section>
  );
}

export function LayerTab({
  cue,
  onSave,
}: {
  cue: VideoCueData | ImageCueData | CameraCueData;
  onSave: (p: Partial<VideoCueData | ImageCueData | CameraCueData>) => void;
}) {
  const layerStyle: LayerStyle = cue.layer_style ?? DEFAULT_LAYER_STYLE;
  const patch = (partial: Partial<LayerStyle>) =>
    onSave({ layer_style: { ...layerStyle, ...partial } });

  return <LayerEditor
    automatic={uniform(layerStyle.layer === null)}
    layer={uniform(layerStyle.layer)}
    opacity={uniform(layerStyle.opacity)}
    blendMode={uniform(layerStyle.blend_mode)}
    onPatch={patch}
  />;
}
