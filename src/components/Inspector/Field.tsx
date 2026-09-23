import { useEffect, useRef, useState } from "react";
import { DragNumber } from "../common/DragNumber";
import { useLocale } from "../../i18n";

/** Translate the shared Inspector vocabulary while keeping user data and protocol names intact. */
const INSPECTOR_LABEL_KEYS: Record<string, string> = {
  Inspector: "inspector.inspector", Basics: "inspector.basics", Camera: "inspector.camera",
  Command: "inspector.command", Devamp: "inspector.devamp", Fade: "inspector.fade",
  Geometry: "inspector.geometry", Group: "inspector.group", Layer: "inspector.layer",
  Levels: "inspector.levels", Light: "inspector.light", Memo: "inspector.memo", Mic: "inspector.mic",
  MIDI: "inspector.midi", OSC: "inspector.osc", Script: "inspector.script", Stop: "inspector.stop",
  Text: "inspector.text", Time: "inspector.time", Timecode: "inspector.timecode", Triggers: "inspector.triggers",
  "Cue Name": "inspector.cueName", "Cue Number": "inspector.cueNumber", Duration: "inspector.duration",
  "Cue #": "uiFixes.cueNumberShort", Port: "preferences.port", "Volume (dB)": "uiFixes.volumeDb", Pan: "uiFixes.pan",
  "Wait (s)": "uiFixes.waitSeconds", "Fade (s)": "uiFixes.fadeSeconds", "Pre-Wait (s)": "uiFixes.preWaitSeconds",
  "Post-Wait (s)": "uiFixes.postWaitSeconds", "Start Time (s)": "uiFixes.startTimeSeconds", "End Time (s)": "uiFixes.endTimeSeconds",
  "Rate (0.1 – 4×)": "uiFixes.rateRange",
  Target: "inspector.target", Opacity: "inspector.opacity", Volume: "inspector.volume",
  "Fade Curve": "inspector.fadeCurve", Linear: "inspector.linear", Exponential: "inspector.exponential",
  "S-Curve": "inspector.sCurve", Loop: "inspector.loop", Rate: "inspector.rate", "Start Time": "inspector.startTime",
  "End Time": "inspector.endTime", "Pre-Wait": "inspector.preWait", "Post-Wait": "inspector.postWait",
  "Continue Mode": "inspector.continueMode", "Auto-Continue": "inspector.autoContinue", "Auto-Follow": "inspector.autoFollow",
  "Text Content": "inspector.textContent", Font: "inspector.font", "Font Size": "inspector.fontSize", Color: "inspector.color",
  Alignment: "inspector.alignment", "Camera Device": "inspector.cameraDevice", "Video Source": "inspector.videoSource",
  "Image Source": "inspector.imageSource", Output: "inspector.output", Trigger: "inspector.trigger",
  "On GO": "inspector.onGo", "On Start": "inspector.onStart", "On Finish": "inspector.onFinish",
  Targets: "sweepUi.targets", Curve: "inspector.fadeCurve", Audio: "cueTypes.audio", Visual: "inspector.visual",
  "On Complete": "inspector.onComplete", Mode: "inspector.mode", Compositing: "inspector.compositing",
  Display: "sweepUi.display", Waits: "sweepUi.sectionWaits", Clip: "sweepUi.sectionClip", Destination: "sweepUi.sectionDestination",
  Playback: "sweepUi.sectionPlayback", "Stop Mode": "sweepUi.stopMode", "Input Patch": "sweepUi.inputPatch",
  "Output Patch": "sweepUi.outputPatchLabel", Fit: "inspector.fit", Crop: "output.crop", Rotation: "output.rotation", Scale: "output.scale",
  "Video Fade In": "inspector.videoFadeIn", "Video Fade Out": "inspector.videoFadeOut", "Audio Fade In": "inspector.audioFadeIn", "Audio Fade Out": "inspector.audioFadeOut",
  "Fade In": "inspector.fadeIn", "Fade Out": "inspector.fadeOut", "Duration (s)": "inspector.durationSeconds", "Time (s)": "inspector.timeSeconds",
  Identity: "sweepUi.sectionIdentity", Media: "sweepUi.sectionMedia", Flow: "sweepUi.sectionFlow", Continue: "sweepUi.continue",
  Name: "common.name", Notes: "cueList.notes", Size: "sweepUi.size", Position: "sweepUi.position", Source: "sweepUi.source", "Stream URL": "sweepUi.streamUrl",
  "After the current pass": "sweepUi.sectionAfterPass", "Position & Scale": "sweepUi.sectionPositionScale", File: "sweepUi.sectionFile",
  "Fade volume": "sweepUi.fadeVolume", "Fade pan": "sweepUi.fadePan", Brightness: "sweepUi.brightness", "To (dB)": "sweepUi.toDb",
  "Fade time (s)": "sweepUi.fadeTime", "Limit display duration": "sweepUi.limitDisplay", "Hold last frame at end (no cut to black)": "sweepUi.holdLastFrame",
};

function useInspectorText() {
  const { t } = useLocale();
  return (value?: string) => (value && INSPECTOR_LABEL_KEYS[value] ? t(INSPECTOR_LABEL_KEYS[value]) : value);
}
// Shared layout primitives and input styles used across all Inspector tabs.
//
// The inspector's visual language: content is grouped into `Section` cards,
// fields are label+control `Field` rows, related numerics sit side-by-side in
// a `Grid2`, continuous values use `SliderRow`, and short exclusive choices
// use `Segmented` buttons.

export const inputStyle: React.CSSProperties = {
  background: "var(--wc-bg-surface)",
  border: "1px solid var(--wc-border-strong)",
  borderRadius: 4,
  color: "var(--wc-text)",
  padding: "4px 8px",
  fontSize: 13,
  width: "100%",
  boxSizing: "border-box",
};

export function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  const text = useInspectorText();
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        marginBottom: 10,
        gap: 8,
      }}
    >
      <label style={{ width: 100, color: "var(--wc-text-secondary)", flexShrink: 0, fontSize: 12 }}>
        {text(label)}
      </label>
      <div style={{ flex: 1, minWidth: 0 }}>{children}</div>
    </div>
  );
}

/** Card grouping related fields under an uppercase micro-title. */
export function Section({
  title,
  children,
  hint,
}: {
  title: string;
  children: React.ReactNode;
  /** Optional caption below the title (e.g. live-apply note). */
  hint?: string;
}) {
  const text = useInspectorText();
  return (
    <div
      style={{
        background: "var(--wc-bg-deepest)",
        border: "1px solid var(--wc-border)",
        borderRadius: 6,
        padding: "10px 12px 6px",
        marginBottom: 10,
      }}
    >
      <div
        style={{
          fontSize: 10,
          fontWeight: 700,
          letterSpacing: "0.07em",
          textTransform: "uppercase",
          color: "var(--wc-text-muted)",
          marginBottom: hint ? 2 : 8,
        }}
      >
        {text(title)}
      </div>
      {hint && (
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginBottom: 8 }}>{text(hint)}</div>
      )}
      {children}
    </div>
  );
}

/** Two-column grid for compact side-by-side fields. */
export function Grid2({ children }: { children: React.ReactNode }) {
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "1fr 1fr",
        columnGap: 10,
        rowGap: 8,
        marginBottom: 10,
      }}
    >
      {children}
    </div>
  );
}

/** Small stacked label + control, for use inside `Grid2` cells. */
export function MiniField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  const text = useInspectorText();
  return (
    <div style={{ minWidth: 0 }}>
      <div style={{ fontSize: 11, color: "var(--wc-text-secondary)", marginBottom: 3 }}>
        {text(label)}
      </div>
      {children}
    </div>
  );
}

/** Numeric input that clamps and commits on blur (or Enter). */
export function NumberInput({
  value,
  step,
  min,
  max,
  width,
  placeholder,
  disabled = false,
  onCommit,
}: {
  value: number | null;
  step: number;
  min: number;
  max: number;
  width?: number;
  placeholder?: string;
  disabled?: boolean;
  onCommit: (v: number) => void;
}) {
  // Controlled draft: the field was uncontrolled (defaultValue + a remount
  // key), which cannot work with a drag wheel — dragging has to move the
  // displayed value on every pointer event.
  const [draft, setDraft] = useState<string>(value?.toString() ?? "");
  useEffect(() => { setDraft(value?.toString() ?? ""); }, [value]);

  const commit = () => {
    const parsed = parseFloat(draft);
    if (Number.isNaN(parsed)) return;
    onCommit(Math.min(max, Math.max(min, parsed)));
  };

  return (
    <DragNumber
      style={{ ...inputStyle, width: width ?? "100%" }}
      step={step}
      min={min}
      max={max}
      value={draft}
      placeholder={placeholder}
      disabled={disabled}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
    />
  );
}

/** Label + range slider + numeric value, committing on drag and on blur. */
export function SliderRow({
  label,
  value,
  min,
  max,
  step,
  format,
  disabled = false,
  mixedLabel,
  commitOnRelease = false,
  onChange,
}: {
  label: string;
  value: number | null;
  min: number;
  max: number;
  step: number;
  /** Renders the numeric readout (e.g. percents, degrees). */
  format: (v: number) => string;
  disabled?: boolean;
  mixedLabel?: string;
  /** Multi-edit can commit once at the end of a drag while keeping identical markup. */
  commitOnRelease?: boolean;
  onChange: (v: number) => void;
}) {
  const text = useInspectorText();
  const displayValue = value ?? (min + max) / 2;
  const [draft, setDraft] = useState(displayValue);
  useEffect(() => { setDraft(displayValue); }, [displayValue]);
  const readout = commitOnRelease && draft !== displayValue
    ? format(draft)
    : value == null ? mixedLabel ?? "—" : format(value);
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10 }}>
      <label style={{ width: 100, color: "var(--wc-text-secondary)", flexShrink: 0, fontSize: 12 }}>
        {text(label)}
      </label>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={draft}
        disabled={disabled}
        onChange={(e) => {
          const next = parseFloat(e.target.value);
          setDraft(next);
          if (!commitOnRelease) onChange(next);
        }}
        onPointerUp={() => { if (commitOnRelease) onChange(draft); }}
        onKeyUp={() => { if (commitOnRelease) onChange(draft); }}
        style={{ flex: 1, cursor: disabled ? "default" : "pointer", minWidth: 0, opacity: disabled ? 0.45 : 1 }}
      />
      <span
        style={{
          width: 52,
          textAlign: "right",
          fontSize: 12,
          fontVariantNumeric: "tabular-nums",
          color: "var(--wc-text-secondary)",
          flexShrink: 0,
        }}
      >
        {readout}
      </span>
    </div>
  );
}

/** Row of mutually-exclusive segment buttons (e.g. Fit / Fill / Stretch). */
export function Segmented<T extends string>({
  options,
  value,
  onChange,
  disabled = false,
}: {
  options: { value: T; label: string; hint?: string }[];
  /** `null` leaves every segment unselected (used for mixed multi-edit values). */
  value: T | null;
  onChange: (v: T) => void;
  disabled?: boolean;
}) {
  const text = useInspectorText();
  return (
    <div style={{ display: "flex", gap: 0, marginBottom: 10, borderRadius: 5, overflow: "hidden", border: "1px solid var(--wc-border-strong)" }}>
      {options.map((o, i) => {
        const active = value === o.value;
        return (
          <button
            key={o.value}
            title={text(o.hint)}
            disabled={disabled}
            onClick={() => onChange(o.value)}
            style={{
              flex: 1,
              padding: "5px 0",
              fontSize: 12,
              cursor: disabled ? "default" : "pointer",
              border: "none",
              borderLeft: i > 0 ? "1px solid var(--wc-border-strong)" : "none",
              background: active ? "var(--wc-accent)" : "var(--wc-bg-surface)",
              color: active ? "var(--wc-accent-fg)" : "var(--wc-text)",
              fontWeight: active ? 600 : 400,
              opacity: disabled ? 0.45 : 1,
            }}
          >
            {text(o.label)}
          </button>
        );
      })}
    </div>
  );
}

/** A row whose nested controls are gated by a leading checkbox. */
export function ToggleRow({
  label, checked, onToggle, children, disabled = false, mixedLabel,
}: {
  label: string; checked: boolean | null; onToggle: (v: boolean) => void; children?: React.ReactNode;
  disabled?: boolean; mixedLabel?: string;
}) {
  const text = useInspectorText();
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (inputRef.current) inputRef.current.indeterminate = checked === null;
  }, [checked]);
  return (
    <div style={{ marginBottom: 10 }}>
      <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: disabled ? "default" : "pointer", opacity: disabled ? 0.5 : 1 }}>
        <input ref={inputRef} type="checkbox" checked={checked === true} disabled={disabled} onChange={(e) => onToggle(e.target.checked)}
          style={{ width: 15, height: 15, cursor: disabled ? "default" : "pointer" }} />
        <span style={{ fontSize: 13, color: "var(--wc-text)" }}>{text(label)}</span>
        {checked === null && mixedLabel && <span style={{ fontSize: 11, color: "var(--wc-text-muted)" }}>{mixedLabel}</span>}
      </label>
      {checked !== false && children && <div style={{ marginTop: 8, paddingLeft: 23 }}>{children}</div>}
    </div>
  );
}
