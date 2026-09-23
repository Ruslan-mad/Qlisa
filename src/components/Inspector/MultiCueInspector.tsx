import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type {
  CueColor,
  CueType,
  FadeCurve,
  OutputPatch,
} from "../../lib/types";
import {
  DEFAULT_GEOMETRY,
  DEFAULT_LAYER_STYLE,
} from "../../lib/types";
import { bulkUpdateCues, getCues, getOutputPatchTable } from "../../lib/commands";
import { useLocale } from "../../i18n";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { useTimingStore } from "../../stores/timingStore";
import { Select } from "../common/Select";
import { CurveSelect } from "../common/CurveSelect";
import { ColorPicker } from "./ColorPicker";
import { Field, Grid2, MiniField, Section, inputStyle } from "./Field";
import { LevelMatrixGrid } from "./LevelMatrixGrid";
import { OutputSelector } from "./OutputSelector";
import { LayerEditor } from "./LayerTab";
import { GeometryEditor, type GeometryEditorState } from "./GeometryTab";
import {
  inspectorContentStyle,
  inspectorRootStyle,
  inspectorTabBarStyle,
  inspectorTabStyle,
  inspectorTitleStyle,
} from "./InspectorChrome";
import {
  findCueSummaryById,
  getMultiCueCapabilities,
  isCurrentApplyRequest,
  nextGeneration,
  shouldRefreshFadeCapabilities,
  type MultiCueRecord,
  type ValueState,
} from "./multiCueModel";

type Tab = "basics" | "type" | "time" | "levels" | "fade" | "layer" | "geometry" | "triggers";
type PatchBuilder = Record<string, unknown> | ((cue: MultiCueRecord) => Record<string, unknown>);

const MIXED = "__multi_cue_mixed__";
const LOOP_INFINITE = 4294967295;

function deepState<T>(values: readonly T[]): ValueState<T> {
  if (values.length === 0) return { kind: "empty" };
  const first = JSON.stringify(values[0]);
  return values.every((value) => JSON.stringify(value) === first)
    ? { kind: "uniform", value: values[0] }
    : { kind: "mixed" };
}

function fieldState<T = unknown>(cues: readonly MultiCueRecord[], key: string): ValueState<T> {
  if (cues.length === 0 || cues.some((cue) => !(key in cue))) return { kind: "empty" };
  return deepState(cues.map((cue) => cue[key] as T));
}

function nestedState<T>(
  cues: readonly MultiCueRecord[],
  key: string,
  fallback: object,
  child: string,
): ValueState<T> {
  if (cues.length === 0 || cues.some((cue) => !(key in cue))) return { kind: "empty" };
  return deepState(cues.map((cue) => {
    const object = (cue[key] as object | null | undefined) ?? fallback;
    return (object as Record<string, unknown>)[child] as T;
  }));
}

function uniformValue<T>(state: ValueState<T>): T | null {
  return state.kind === "uniform" ? state.value : null;
}

function MixedTextInput({
  state,
  mixedLabel,
  disabled,
  multiline = false,
  onCommit,
}: {
  state: ValueState<string | null>;
  mixedLabel: string;
  disabled?: boolean;
  multiline?: boolean;
  onCommit: (value: string) => void;
}) {
  const source = state.kind === "uniform" ? state.value ?? "" : "";
  const sourceKey = `${state.kind}:${source}`;
  const [value, setValue] = useState(source);
  const [touched, setTouched] = useState(false);
  useEffect(() => { setValue(source); setTouched(false); }, [sourceKey]);
  const common = {
    value,
    placeholder: state.kind === "mixed" ? mixedLabel : undefined,
    disabled,
    onChange: (event: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => {
      setValue(event.target.value);
      setTouched(true);
    },
    onBlur: () => { if (touched) onCommit(value); },
    style: { ...inputStyle, ...(multiline ? { resize: "vertical" as const, minHeight: 56 } : {}) },
  };
  return multiline ? <textarea {...common} rows={3} /> : <input {...common} />;
}

function MixedNumberInput({
  state,
  mixedLabel,
  disabled,
  scale = 1,
  step,
  min,
  max,
  allowNull = false,
  onCommit,
}: {
  state: ValueState<number | null>;
  mixedLabel: string;
  disabled?: boolean;
  scale?: number;
  step: number;
  min: number;
  max: number;
  allowNull?: boolean;
  onCommit: (value: number | null) => void;
}) {
  const raw = state.kind === "uniform" && state.value != null ? String(state.value / scale) : "";
  const sourceKey = `${state.kind}:${raw}`;
  const [value, setValue] = useState(raw);
  const [touched, setTouched] = useState(false);
  useEffect(() => { setValue(raw); setTouched(false); }, [sourceKey]);
  return (
    <input
      type="number"
      style={inputStyle}
      value={value}
      placeholder={state.kind === "mixed" ? mixedLabel : undefined}
      disabled={disabled}
      step={step}
      min={min}
      max={max}
      onChange={(event) => { setValue(event.target.value); setTouched(true); }}
      onBlur={() => {
        if (!touched) return;
        if (value.trim() === "") {
          if (allowNull) onCommit(null);
          return;
        }
        const parsed = Number(value);
        if (Number.isFinite(parsed)) onCommit(Math.min(max, Math.max(min, parsed)) * scale);
      }}
    />
  );
}

function MixedCheckbox({
  state,
  label,
  mixedLabel,
  disabled,
  onChange,
}: {
  state: ValueState<boolean>;
  label: string;
  mixedLabel: string;
  disabled?: boolean;
  onChange: (value: boolean) => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = state.kind === "mixed";
  }, [state.kind]);
  return (
    <label style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10, cursor: disabled ? "default" : "pointer", opacity: disabled ? 0.5 : 1 }}>
      <input
        ref={ref}
        type="checkbox"
        checked={state.kind === "uniform" && state.value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
        style={{ width: 15, height: 15, cursor: disabled ? "default" : "pointer" }}
      />
      <span style={{ fontSize: 13, color: "var(--wc-text)" }}>{label}</span>
      {state.kind === "mixed" && <span style={{ fontSize: 11, color: "var(--wc-text-muted)" }}>{mixedLabel}</span>}
    </label>
  );
}

function MixedSliderNumber({
  state,
  mixedLabel,
  disabled,
  min,
  max,
  step,
  onCommit,
}: {
  state: ValueState<number>;
  mixedLabel: string;
  disabled?: boolean;
  min: number;
  max: number;
  step: number;
  onCommit: (value: number) => void;
}) {
  const source = state.kind === "uniform" ? state.value : 0;
  const [draft, setDraft] = useState(source);
  useEffect(() => { setDraft(source); }, [source, state.kind]);
  return (
    <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={draft}
        disabled={disabled || state.kind !== "uniform"}
        onChange={(event) => setDraft(Number(event.target.value))}
        onMouseUp={() => onCommit(draft)}
        onKeyUp={() => onCommit(draft)}
        style={{ flex: 1, minWidth: 0 }}
      />
      <div style={{ width: 72 }}>
        <MixedNumberInput
          state={state}
          mixedLabel={mixedLabel}
          disabled={disabled}
          step={step}
          min={min}
          max={max}
          onCommit={(value) => { if (value != null) onCommit(value); }}
        />
      </div>
    </div>
  );
}

function DisabledHint({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ padding: "9px 10px", marginBottom: 10, border: "1px solid var(--wc-border)", borderRadius: 5, color: "var(--wc-text-muted)", fontSize: 11 }}>
      {children}
    </div>
  );
}

function fadeKey(cue: MultiCueRecord, family: "audio" | "visual", direction: "in" | "out", part: "ms" | "curve"): string | null {
  if (family === "audio") {
    return cue.cue_type === "audio" || cue.cue_type === "video" ? `fade_${direction}_${part}` : null;
  }
  if (cue.cue_type === "video" || cue.cue_type === "camera") return `video_fade_${direction}_${part}`;
  if (cue.cue_type === "image") return `fade_${direction}_${part}`;
  return null;
}

function specialisedTab(type: CueType, t: (key: string) => string): string | null {
  const labels: Partial<Record<CueType, string>> = {
    fade: t("inspector.fade"), stop: t("inspector.stop"), devamp: t("inspector.devamp"),
    group: t("inspector.group"), camera: t("inspector.camera"), light: t("inspector.light"),
    mic: t("inspector.mic"), timecode: t("inspector.timecode"), text: t("inspector.text"),
    script: t("inspector.script"), memo: t("inspector.memo"), midi_file: t("sweepUi.cueTabsMidiFile"),
    midi: t("sweepUi.cueTabsMessages"), osc: t("sweepUi.cueTabsMessages"),
    start: t("inspector.command"), pause: t("inspector.command"), resume: t("inspector.command"),
    load: t("inspector.command"), reset: t("inspector.command"), goto: t("inspector.command"),
    arm: t("inspector.command"), disarm: t("inspector.command"),
  };
  return labels[type] ?? null;
}

export function MultiCueInspector({ cueIds }: { cueIds: string[] }) {
  const { t, locale } = useLocale();
  const selectionKey = cueIds.join("\u001f");
  const selectionRef = useRef(selectionKey);
  const loadGeneration = useRef(0);
  const saveGeneration = useRef(0);
  const capabilityRefreshGeneration = useRef(0);
  const capabilityRefreshTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const previousRuntimeSnapshot = useRef<{
    selectionKey: string;
    snapshots: { cueId: string; state: string; isLoading: boolean; hasTiming: boolean; summaryRevision: object }[];
  } | null>(null);
  const [cues, setCues] = useState<MultiCueRecord[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("basics");
  const [patches, setPatches] = useState<OutputPatch[]>([]);
  const [defaultPatchId, setDefaultPatchId] = useState<string | null>(null);
  const displayPrefs = useWorkspaceStore((state) => state.displayPrefs);
  const workspaceCues = useWorkspaceStore((state) => state.cues);
  const timingActiveKey = useTimingStore((state) => cueIds.filter((id) => state.timings[id] != null).join("\u001f"));

  useLayoutEffect(() => {
    selectionRef.current = selectionKey;
    saveGeneration.current = nextGeneration(saveGeneration.current);
    capabilityRefreshGeneration.current = nextGeneration(capabilityRefreshGeneration.current);
    if (capabilityRefreshTimer.current) clearTimeout(capabilityRefreshTimer.current);
    capabilityRefreshTimer.current = null;
    previousRuntimeSnapshot.current = null;
  }, [selectionKey]);

  const load = async (showLoading: boolean) => {
    const generation = ++loadGeneration.current;
    if (showLoading) setLoading(true);
    try {
      const records = await getCues(cueIds) as unknown as MultiCueRecord[];
      if (selectionRef.current !== selectionKey || generation !== loadGeneration.current) return;
      setCues(records);
      setError(null);
    } catch (reason) {
      if (selectionRef.current === selectionKey && generation === loadGeneration.current) setError(String(reason));
    } finally {
      if (selectionRef.current === selectionKey && generation === loadGeneration.current) setLoading(false);
    }
  };

  useEffect(() => {
    setCues(null);
    setTab("basics");
    setError(null);
    setSaving(false);
    void load(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectionKey]);

  useEffect(() => {
    getOutputPatchTable()
      .then((table) => { setPatches(table.patches); setDefaultPatchId(table.default_patch_id); })
      .catch(console.error);
  }, []);

  const timingCueIds = new Set(timingActiveKey ? timingActiveKey.split("\u001f") : []);
  const runtimeSnapshots = cueIds.flatMap((id) => {
    const summary = findCueSummaryById(workspaceCues, id);
    return summary ? [{
      cueId: id,
      state: String(summary.state ?? ""),
      isLoading: summary.is_loading === true,
      hasTiming: timingCueIds.has(id),
      summaryRevision: summary,
    }] : [];
  });
  const staleCapabilityIds = (cues ?? [])
    .filter((cue) => (cue._bulk_edit?.rebuild_editable ?? cue._bulk_edit?.fade_editable) === false)
    .map((cue) => cue.id);
  useEffect(() => {
    const previous = previousRuntimeSnapshot.current;
    previousRuntimeSnapshot.current = { selectionKey, snapshots: runtimeSnapshots };
    if (!cues || !previous || previous.selectionKey !== selectionKey) return;
    if (!shouldRefreshFadeCapabilities(previous.snapshots, runtimeSnapshots, staleCapabilityIds)) return;
    if (capabilityRefreshTimer.current) clearTimeout(capabilityRefreshTimer.current);
    const generation = nextGeneration(capabilityRefreshGeneration.current);
    capabilityRefreshGeneration.current = generation;
    capabilityRefreshTimer.current = setTimeout(() => {
      capabilityRefreshTimer.current = null;
      if (generation === capabilityRefreshGeneration.current && selectionRef.current === selectionKey) void load(false);
    }, 180);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectionKey, workspaceCues, timingActiveKey, cues, staleCapabilityIds.join("\u001f")]);

  useEffect(() => () => {
    capabilityRefreshGeneration.current = nextGeneration(capabilityRefreshGeneration.current);
    if (capabilityRefreshTimer.current) clearTimeout(capabilityRefreshTimer.current);
  }, []);

  const types = cues?.map((cue) => cue.cue_type) ?? [];
  const allSameType = types.length > 0 && types.every((type) => type === types[0]);
  const capabilities = getMultiCueCapabilities(types);
  const allLevels = capabilities.levels;
  const allVisual = capabilities.visual;
  const allOutput = capabilities.outputs;
  const allRebuildEditable = !!cues?.every((cue) => cue._bulk_edit?.rebuild_editable ?? cue._bulk_edit?.fade_editable ?? false);

  const tabs = useMemo(() => {
    const items: { id: Tab; label: string; enabled: boolean; reason?: string }[] = [
      { id: "basics", label: t("inspector.basics"), enabled: true },
    ];
    const special = allSameType && types[0] ? specialisedTab(types[0], t) : null;
    if (special) items.push({ id: "type", label: special, enabled: false, reason: locale === "ru" ? "Специальные настройки этого типа пока редактируются по одному cue." : "Type-specific settings are currently edited one cue at a time." });
    items.push(
      { id: "time", label: t("inspector.time"), enabled: true },
      { id: "levels", label: t("inspector.levels"), enabled: allLevels, reason: locale === "ru" ? "Уровни доступны при выборе только Audio/Video cue." : "Levels require an Audio/Video-only selection." },
      { id: "fade", label: t("inspector.fade"), enabled: capabilities.audioFade || capabilities.visualFade, reason: locale === "ru" ? "У выбранных cue нет общего типа fade." : "The selected cues do not share a fade family." },
      { id: "layer", label: t("inspector.layer"), enabled: allVisual, reason: locale === "ru" ? "Слои доступны при выборе только Video/Image/Camera cue." : "Layers require a Video/Image/Camera-only selection." },
      { id: "geometry", label: t("inspector.geometry"), enabled: allVisual, reason: locale === "ru" ? "Геометрия доступна при выборе только Video/Image/Camera cue." : "Geometry requires a Video/Image/Camera-only selection." },
      { id: "triggers", label: t("inspector.triggers"), enabled: false, reason: locale === "ru" ? "Триггеры уникальны для каждого cue и пока редактируются по одному." : "Triggers are unique per cue and are currently edited one at a time." },
    );
    return items;
  }, [allLevels, allSameType, allVisual, capabilities.audioFade, capabilities.visualFade, locale, t, types]);

  useEffect(() => {
    if (!tabs.find((item) => item.id === tab)?.enabled) setTab("basics");
  }, [tab, tabs]);

  const save = async (builder: PatchBuilder, requiresRebuild = false) => {
    if (!cues || saving || (requiresRebuild && !allRebuildEditable)) return;
    const submittedSelection = selectionKey;
    const submittedGeneration = nextGeneration(saveGeneration.current);
    saveGeneration.current = submittedGeneration;
    const updates = cues.flatMap((cue) => {
      const properties = typeof builder === "function" ? builder(cue) : builder;
      return Object.keys(properties).length > 0 ? [{ cueId: cue.id, properties }] : [];
    });
    if (updates.length === 0) return;
    setSaving(true);
    setError(null);
    try {
      await bulkUpdateCues(updates);
      if (!isCurrentApplyRequest(
        submittedGeneration,
        saveGeneration.current,
        submittedSelection,
        selectionRef.current,
      )) return;
      await load(false);
    } catch (reason) {
      if (isCurrentApplyRequest(
        submittedGeneration,
        saveGeneration.current,
        submittedSelection,
        selectionRef.current,
      )) setError(String(reason));
    } finally {
      if (isCurrentApplyRequest(
        submittedGeneration,
        saveGeneration.current,
        submittedSelection,
        selectionRef.current,
      )) setSaving(false);
    }
  };

  if (loading || !cues) {
    return (
      <div style={inspectorRootStyle}>
        <div style={inspectorTitleStyle}><span style={{ fontWeight: 600 }}>{t("multiCueInspector.title", { count: cueIds.length })}</span></div>
        <div style={{ ...inspectorContentStyle, color: "var(--wc-text-faint)", textAlign: "center" }}>
          {loading ? t("common.loading") : error ? (
            <div role="alert" style={{ padding: 9, borderRadius: 4, background: "rgba(248,113,113,0.12)", color: "#fca5a5", fontSize: 12, textAlign: "left" }}>{error}</div>
          ) : null}
        </div>
      </div>
    );
  }

  const mixedLabel = t("multiCueInspector.mixed");
  const rebuildHint = locale === "ru"
    ? "Остановите или сбросьте выбранные cue, чтобы изменить маршрутизацию, тайминг или fade. Live-параметры остаются доступны."
    : "Stop or reset the selected cues to edit routing, timing or fades. Live-safe controls remain available.";

  const patchNested = (key: "geometry" | "layer_style", partial: Record<string, unknown>) =>
    save((cue) => ({ [key]: { ...((cue[key] as Record<string, unknown> | undefined) ?? (key === "geometry" ? DEFAULT_GEOMETRY : DEFAULT_LAYER_STYLE)), ...partial } }));

  const renderOutputs = () => {
    if (!allOutput) return null;
    return (
      <>
        <OutputSelector
          cues={cues}
          outputs={displayPrefs.output_destinations ?? []}
          defaultOutputId={displayPrefs.default_output_id}
          disabled={saving || !allRebuildEditable}
          mixedLabel={mixedLabel}
          onChange={(updates) => {
            const byId = new Map(updates.map((update) => [update.cueId, update]));
            void save((cue) => {
              const update = byId.get(cue.id);
              return update ? { output_id: update.output_id, output_ids: update.output_ids } : {};
            }, true);
          }}
        />
        {!allRebuildEditable && <div style={{ fontSize: 11, color: "var(--wc-warning, #fbbf24)", marginBottom: 10 }}>{rebuildHint}</div>}
      </>
    );
  };

  const renderBasics = () => {
    const fileState = fieldState<string | null>(cues, "file_path");
    return (
      <>
        <Section title={t("inspector.identity")}>
          <Grid2>
            <MiniField label="Cue #"><MixedTextInput state={fieldState(cues, "number")} mixedLabel={mixedLabel} disabled={saving} onCommit={(number) => void save({ number: number || null })} /></MiniField>
            <MiniField label="Color"><div style={{ paddingTop: 3 }}><ColorPicker value={uniformValue(fieldState<CueColor>(cues, "color"))} disabled={saving} onChange={(color) => void save({ color })} /></div></MiniField>
          </Grid2>
          <Field label="Name"><MixedTextInput state={fieldState(cues, "name")} mixedLabel={mixedLabel} disabled={saving} onCommit={(name) => void save({ name })} /></Field>
          <Field label="Notes"><MixedTextInput state={fieldState(cues, "notes")} mixedLabel={mixedLabel} multiline disabled={saving} onCommit={(notes) => void save({ notes })} /></Field>
        </Section>
        {fileState.kind !== "empty" && (
          <Section title={t("inspector.media")}>
            <div style={{ display: "flex", gap: 6, marginBottom: 10 }}>
              <input style={{ ...inputStyle, flex: 1 }} readOnly value={fileState.kind === "uniform" ? (fileState.value?.split(/[\\/]/).pop() ?? t("common.none")) : ""} placeholder={fileState.kind === "mixed" ? mixedLabel : undefined} />
              <button disabled title={locale === "ru" ? "Медиафайл выбирается отдельно для каждого cue" : "Choose media separately for each cue"} style={{ padding: "4px 12px", background: "var(--wc-bg-hover)", border: "1px solid var(--wc-border-strong)", borderRadius: 4, color: "var(--wc-text-faint)", fontSize: 12 }}>{t("uiFixes.browse")}</button>
            </div>
          </Section>
        )}
        <Section title={t("inspector.flow")}>
          <Field label="Continue">
            <Select style={inputStyle} value={uniformValue(fieldState<string>(cues, "continue_mode")) ?? MIXED} disabled={saving} onChange={(event) => { if (event.target.value !== MIXED) void save({ continue_mode: event.target.value }); }}>
              <option value={MIXED} disabled>{mixedLabel}</option><option value="do_not_continue">{t("inspector.doNotContinue")}</option><option value="auto_continue">Auto-Continue</option><option value="auto_follow">Auto-Follow</option>
            </Select>
          </Field>
          <MixedCheckbox state={fieldState(cues, "is_disabled")} label={t("sweepUi.disableCue")} mixedLabel={mixedLabel} disabled={saving} onChange={(is_disabled) => void save({ is_disabled })} />
        </Section>
      </>
    );
  };

  const renderTime = () => {
    const allAv = types.every((type) => type === "audio" || type === "video");
    const onlyAudio = types.every((type) => type === "audio");
    const onlyVideo = types.every((type) => type === "video");
    const onlyImage = types.every((type) => type === "image");
    const onlyWait = types.every((type) => type === "wait");
    const onlyFade = types.every((type) => type === "fade");
    return (
      <>
        {!allRebuildEditable && <DisabledHint>{rebuildHint}</DisabledHint>}
        {onlyWait && <Section title="Duration"><Grid2><MiniField label="Wait (s)"><MixedNumberInput state={fieldState(cues, "wait_duration_ms")} mixedLabel={mixedLabel} disabled={saving || !allRebuildEditable} scale={1000} step={0.1} min={0} max={86400} onCommit={(value) => { if (value != null) void save({ wait_duration_ms: Math.round(value) }, true); }} /></MiniField></Grid2></Section>}
        {onlyFade && <Section title="Duration"><Grid2><MiniField label="Fade (s)"><MixedNumberInput state={fieldState(cues, "fade_duration_ms")} mixedLabel={mixedLabel} disabled={saving || !allRebuildEditable} scale={1000} step={0.1} min={0.1} max={3600} onCommit={(value) => { if (value != null) void save({ fade_duration_ms: Math.round(value) }, true); }} /></MiniField></Grid2></Section>}
        {onlyImage && (
          <Section title="Display">
            <MixedCheckbox state={deepState(cues.map((cue) => cue.display_duration_ms != null))} label="Limit display duration" mixedLabel={mixedLabel} disabled={saving || !allRebuildEditable} onChange={(enabled) => void save({ display_duration_ms: enabled ? 5000 : null }, true)} />
            <Grid2><MiniField label="Duration (s)"><MixedNumberInput state={fieldState(cues, "display_duration_ms")} mixedLabel={mixedLabel} disabled={saving || !allRebuildEditable} allowNull scale={1000} step={0.1} min={0.1} max={86400} onCommit={(value) => void save({ display_duration_ms: value == null ? null : Math.round(value) }, true)} /></MiniField></Grid2>
          </Section>
        )}
        <Section title="Waits"><Grid2>
          <MiniField label="Pre-Wait (s)"><MixedNumberInput state={fieldState(cues, "pre_wait_ms")} mixedLabel={mixedLabel} disabled={saving} scale={1000} step={0.1} min={0} max={86400} onCommit={(value) => { if (value != null) void save({ pre_wait_ms: Math.round(value) }); }} /></MiniField>
          <MiniField label="Post-Wait (s)"><MixedNumberInput state={fieldState(cues, "post_wait_ms")} mixedLabel={mixedLabel} disabled={saving} scale={1000} step={0.1} min={0} max={86400} onCommit={(value) => { if (value != null) void save({ post_wait_ms: Math.round(value) }); }} /></MiniField>
        </Grid2></Section>
        {allAv && (
          <Section title="Clip">
            <Grid2>
              <MiniField label="Start Time (s)"><MixedNumberInput state={fieldState(cues, "start_time_ms")} mixedLabel={mixedLabel} disabled={saving || !allRebuildEditable} allowNull scale={1000} step={0.001} min={0} max={86400} onCommit={(value) => void save({ start_time_ms: value == null ? null : Math.round(value) }, true)} /></MiniField>
              <MiniField label="End Time (s)"><MixedNumberInput state={fieldState(cues, "end_time_ms")} mixedLabel={mixedLabel} disabled={saving || !allRebuildEditable} allowNull scale={1000} step={0.001} min={0} max={86400} onCommit={(value) => void save({ end_time_ms: value == null ? null : Math.round(value) }, true)} /></MiniField>
            </Grid2>
            <MixedCheckbox state={deepState(cues.map((cue) => Number(cue.loop_count) > 0))} label="Loop" mixedLabel={mixedLabel} disabled={saving || !allRebuildEditable} onChange={(enabled) => void save({ loop_count: enabled ? 1 : 0 }, true)} />
            <Grid2>
              <MiniField label={locale === "ru" ? "Количество повторов" : "Loop count"}><MixedNumberInput state={fieldState(cues, "loop_count")} mixedLabel={mixedLabel} disabled={saving || !allRebuildEditable} step={1} min={0} max={LOOP_INFINITE} onCommit={(value) => { if (value != null) void save({ loop_count: Math.round(value) }, true); }} /></MiniField>
              <MiniField label="∞"><button disabled={saving || !allRebuildEditable} onClick={() => void save({ loop_count: LOOP_INFINITE }, true)} style={{ ...inputStyle, cursor: saving || !allRebuildEditable ? "default" : "pointer" }}>∞</button></MiniField>
            </Grid2>
            {onlyAudio && <Grid2><MiniField label="Rate (0.1 – 4×)"><MixedNumberInput state={fieldState(cues, "rate")} mixedLabel={mixedLabel} disabled={saving || !allRebuildEditable} step={0.1} min={0.1} max={4} onCommit={(value) => { if (value != null) void save({ rate: value }, true); }} /></MiniField></Grid2>}
            {onlyVideo && <MixedCheckbox state={fieldState(cues, "hold_last_frame")} label="Hold last frame at end (no cut to black)" mixedLabel={mixedLabel} disabled={saving || !allRebuildEditable} onChange={(hold_last_frame) => void save({ hold_last_frame }, true)} />}
          </Section>
        )}
      </>
    );
  };

  const renderLevels = () => {
    const volume = fieldState<number>(cues, "volume_db");
    const pan = fieldState<number>(cues, "pan");
    const patchState = fieldState<string | null>(cues, "output_patch_id");
    const matrixState = fieldState<number[][] | null>(cues, "level_matrix");
    const patchId = uniformValue(patchState) ?? defaultPatchId;
    const activePatch = patches.find((patch) => patch.id === patchId);
    const selectedBusValue = patchState.kind === "mixed"
      ? MIXED
      : (activePatch?.kind === "main" ? "" : uniformValue(patchState) ?? "");
    return (
      <>
        {!allRebuildEditable && <DisabledHint>{rebuildHint}</DisabledHint>}
        <Field label={t("audioBusUi.bus")}><Select style={inputStyle} value={selectedBusValue} disabled={saving || !allRebuildEditable} onChange={(event) => { if (event.target.value !== MIXED) void save({ output_patch_id: event.target.value || null }, true); }}><option value={MIXED} disabled>{mixedLabel}</option><option value="">{t("audioBusUi.main")}</option>{patches.filter((patch) => patch.kind === "aux" && patch.enabled).map((patch) => <option key={patch.id} value={patch.id}>{patch.name}</option>)}</Select></Field>
        <Field label="Volume (dB)"><MixedSliderNumber state={volume} mixedLabel={mixedLabel} disabled={saving} min={-60} max={12} step={0.5} onCommit={(volume_db) => void save({ volume_db })} /></Field>
        <Field label=""><button disabled title={locale === "ru" ? "Нормализация анализирует каждый файл отдельно" : "Normalization analyzes each file separately"} style={{ padding: "4px 10px", background: "var(--wc-bg-app)", border: "1px solid var(--wc-border-strong)", borderRadius: 4, color: "var(--wc-text-faint)", fontSize: 12 }}>{t("sweepUi.normalize")}</button></Field>
        <Field label="Pan"><MixedSliderNumber state={pan} mixedLabel={mixedLabel} disabled={saving || pan.kind === "empty"} min={-1} max={1} step={0.05} onCommit={(panValue) => void save({ pan: panValue })} /></Field>
        {matrixState.kind === "uniform" && patchState.kind !== "mixed" ? (
          <LevelMatrixGrid cueId={cues[0].id} cueIds={cueIds} matrix={matrixState.value} patchName={activePatch?.name ?? t("audioBusUi.main")} deviceChannels={activePatch?.channels ?? []} disabled={saving} onSave={(level_matrix) => void save({ level_matrix })} />
        ) : (
          <Section title={locale === "ru" ? "Матрица уровней" : "Level Matrix"} hint={mixedLabel}><div style={{ display: "flex", gap: 8, marginBottom: 8 }}><button disabled={saving} onClick={() => void save({ level_matrix: null })} style={{ ...inputStyle, cursor: "pointer" }}>{t("sweepUi.usePan")}</button><button disabled={saving || patchState.kind === "mixed"} onClick={() => { const columns = Math.max(activePatch?.channels.length ?? 0, 2); const matrix = Array.from({ length: 2 }, (_, row) => Array.from({ length: columns }, (_, column) => row === column ? 0 : -60)); void save({ level_matrix: matrix }); }} style={{ ...inputStyle, cursor: patchState.kind === "mixed" ? "default" : "pointer" }}>{t("sweepUi.useMatrix")}</button></div></Section>
        )}
      </>
    );
  };

  const renderFadeSection = (family: "audio" | "visual", direction: "in" | "out") => {
    const msKeys = cues.map((cue) => fadeKey(cue, family, direction, "ms"));
    const curveKeys = cues.map((cue) => fadeKey(cue, family, direction, "curve"));
    if (msKeys.some((key) => key == null) || curveKeys.some((key) => key == null)) return null;
    const durations = deepState(cues.map((cue, index) => cue[msKeys[index]!] as number | null));
    const curves = deepState(cues.map((cue, index) => cue[curveKeys[index]!] as FadeCurve | null));
    const prefix = family === "audio" ? (types.every((type) => type === "video") ? "Audio " : "") : "Video ";
    return (
      <Section title={`${prefix}${direction === "in" ? "Fade In" : "Fade Out"}`}><Grid2>
        <MiniField label="Duration (s)"><MixedNumberInput state={durations} mixedLabel={mixedLabel} disabled={saving || !allRebuildEditable} allowNull scale={1000} step={0.1} min={0} max={3600} onCommit={(value) => void save((cue) => { const key = fadeKey(cue, family, direction, "ms"); return key ? { [key]: value == null ? null : Math.round(value) } : {}; }, true)} /></MiniField>
        <MiniField label="Curve">{curves.kind === "uniform" && curves.value != null ? <CurveSelect value={curves.value} disabled={saving || !allRebuildEditable} onChange={(value) => void save((cue) => { const key = fadeKey(cue, family, direction, "curve"); return key ? { [key]: value } : {}; }, true)} /> : <Select style={inputStyle} value={MIXED} disabled={saving || !allRebuildEditable} onChange={(event) => { if (event.target.value !== MIXED) void save((cue) => { const key = fadeKey(cue, family, direction, "curve"); return key ? { [key]: event.target.value } : {}; }, true); }}><option value={MIXED} disabled>{mixedLabel}</option><option value="s_curve">{t("inspector.sCurve")}</option><option value="linear">{t("inspector.linear")}</option><option value="exponential">{t("inspector.exponential")}</option></Select>}</MiniField>
      </Grid2></Section>
    );
  };

  const renderFade = () => <>{!allRebuildEditable && <DisabledHint>{rebuildHint}</DisabledHint>}{capabilities.visualFade && renderFadeSection("visual", "in")}{capabilities.visualFade && renderFadeSection("visual", "out")}{capabilities.audioFade && renderFadeSection("audio", "in")}{capabilities.audioFade && renderFadeSection("audio", "out")}</>;

  const renderLayer = () => {
    const layer = nestedState<number | null>(cues, "layer_style", DEFAULT_LAYER_STYLE, "layer");
    return <LayerEditor
      automatic={deepState(cues.map((cue) => (((cue.layer_style as typeof DEFAULT_LAYER_STYLE | undefined) ?? DEFAULT_LAYER_STYLE).layer == null)))}
      layer={layer}
      opacity={nestedState(cues, "layer_style", DEFAULT_LAYER_STYLE, "opacity")}
      blendMode={nestedState(cues, "layer_style", DEFAULT_LAYER_STYLE, "blend_mode")}
      disabled={saving}
      mixedLabel={mixedLabel}
      commitSlidersOnRelease
      onPatch={(partial) => void patchNested("layer_style", partial)}
    />;
  };

  const renderGeometry = () => {
    const keys = Object.keys(DEFAULT_GEOMETRY) as (keyof typeof DEFAULT_GEOMETRY)[];
    const geometry = Object.fromEntries(keys.map((key) => [key, nestedState(cues, "geometry", DEFAULT_GEOMETRY, key)])) as GeometryEditorState;
    const resetDisabled = keys.every((key) => geometry[key].kind === "uniform" && Object.is(geometry[key].value, DEFAULT_GEOMETRY[key]));
    return <GeometryEditor
      geometry={geometry}
      disabled={saving}
      mixedLabel={mixedLabel}
      commitSlidersOnRelease
      resetDisabled={resetDisabled}
      onPatch={(partial) => void patchNested("geometry", partial)}
      onReset={() => void save({ geometry: { ...DEFAULT_GEOMETRY } })}
    />;
  };

  return (
    <div style={inspectorRootStyle}>
      <div style={inspectorTitleStyle}><span style={{ fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>◫ {t("multiCueInspector.title", { count: cueIds.length })}</span>{saving && <span style={{ fontSize: 11, color: "var(--wc-text-muted)" }}>{t("multiCueInspector.applying")}</span>}</div>
      <div role="tablist" style={inspectorTabBarStyle}>{tabs.map((item) => <button key={item.id} type="button" role="tab" aria-selected={tab === item.id} disabled={!item.enabled} title={item.enabled ? undefined : item.reason} style={inspectorTabStyle(tab === item.id, !item.enabled)} onClick={() => { if (item.enabled) setTab(item.id); }}>{item.label}</button>)}</div>
      <div style={inspectorContentStyle}>
        {error && <div role="alert" style={{ padding: 9, marginBottom: 10, borderRadius: 4, background: "rgba(248,113,113,0.12)", color: "#fca5a5", fontSize: 12 }}>{error}</div>}
        {renderOutputs()}
        {tab === "basics" && renderBasics()}
        {tab === "time" && renderTime()}
        {tab === "levels" && renderLevels()}
        {tab === "fade" && renderFade()}
        {tab === "layer" && renderLayer()}
        {tab === "geometry" && renderGeometry()}
      </div>
    </div>
  );
}
