import { useEffect, useRef } from "react";
import type { CueType, OutputDestination } from "../../lib/types";
import { getDefaultOutputDestination } from "../../lib/types";
import { useLocale } from "../../i18n";
import { Field } from "./Field";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { orderOutputDestinations } from "../Transport/outputFtbModel";

export interface OutputSelectableCue {
  id: string;
  cue_type: CueType;
  output_id?: string | null;
  output_ids?: string[];
  /** Legacy Video routing key. It is used only when it still names a configured destination. */
  output_surface_id?: string | null;
}

export interface OutputAssignmentUpdate {
  cueId: string;
  output_id: string | null;
  output_ids: string[];
}

export function selectedOutputIds(cue: OutputSelectableCue, namedOutputIds: ReadonlySet<string>): string[] {
  if (cue.output_ids?.length) return cue.output_ids;
  if (cue.output_id) return [cue.output_id];
  if (cue.cue_type === "video" && cue.output_surface_id && namedOutputIds.has(cue.output_surface_id)) {
    return [cue.output_surface_id];
  }
  return [];
}

/**
 * IDs shown as checked in the inspector.  An implicit cue is displayed on the
 * effective global default, while its persisted selection remains empty.
 */
export function displayedOutputIds(
  cue: OutputSelectableCue,
  namedOutputIds: ReadonlySet<string>,
  defaultOutputId?: string,
): string[] {
  const explicit = selectedOutputIds(cue, namedOutputIds);
  return explicit.length > 0 || !defaultOutputId ? explicit : [defaultOutputId];
}

/** Build the persisted route after toggling one displayed output. */
export function outputIdsAfterToggle(
  cue: OutputSelectableCue,
  outputId: string,
  checked: boolean,
  defaultOutputId: string | undefined,
  namedOutputIds: ReadonlySet<string>,
): string[] {
  const explicit = selectedOutputIds(cue, namedOutputIds);

  const next = new Set(explicit);
  if (checked) {
    // An empty route is displayed on the effective default.  Once another
    // output is selected, persist that displayed default so the route stays
    // on both outputs if the global default changes later.
    if (explicit.length === 0 && outputId !== defaultOutputId && defaultOutputId) {
      next.add(defaultOutputId);
    }
    next.add(outputId);
  } else {
    next.delete(outputId);
  }
  return [...next];
}

function CheckboxRow({
  checked,
  mixed,
  disabled,
  label,
  mixedLabel,
  onChange,
}: {
  checked: boolean;
  mixed: boolean;
  disabled: boolean;
  label: React.ReactNode;
  mixedLabel?: string;
  onChange: (checked: boolean) => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = mixed;
  }, [mixed]);
  return (
    <label style={{ display: "flex", alignItems: "center", gap: 7, fontSize: 12, cursor: disabled ? "default" : "pointer", opacity: disabled ? 0.5 : 1 }}>
      <input
        ref={ref}
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span>{label}</span>
      {mixed && mixedLabel && <span style={{ fontSize: 11, color: "var(--wc-text-muted)" }}>{mixedLabel}</span>}
    </label>
  );
}

/** The routing control shared by the single- and multi-cue inspectors. */
export function OutputSelector({
  cues,
  outputs,
  defaultOutputId,
  disabled = false,
  mixedLabel,
  onChange,
}: {
  cues: readonly OutputSelectableCue[];
  outputs: readonly OutputDestination[];
  defaultOutputId?: string | null;
  disabled?: boolean;
  mixedLabel?: string;
  onChange: (updates: OutputAssignmentUpdate[]) => void;
}) {
  const { t, locale } = useLocale();
  const displayPrefsLoaded = useWorkspaceStore((state) => state.displayPrefsLoaded);
  const loadDisplayPrefs = useWorkspaceStore((state) => state.loadDisplayPrefs);
  // The main window can render a newly selected visual cue before App's
  // asynchronous bootstrap fetch completes.  Fetch the authoritative global
  // destinations here as well, so routing checkboxes do not depend on the
  // user opening Preferences and pressing Apply first.
  useEffect(() => {
    if (!displayPrefsLoaded) void loadDisplayPrefs();
  }, [displayPrefsLoaded, loadDisplayPrefs]);
  const namedOutputIds = new Set(outputs.map((output) => output.id));
  const effectiveDefault = getDefaultOutputDestination([...outputs], defaultOutputId);
  const explicitSelections = cues.map((cue) => selectedOutputIds(cue, namedOutputIds));
  const selections = cues.map((cue) => displayedOutputIds(cue, namedOutputIds, effectiveDefault?.id));
  const selectedIds = new Set(selections.flat());
  const visibleOutputs = orderOutputDestinations<Pick<OutputDestination, "id" | "name" | "enabled" | "sink_kind">>([
    ...outputs.filter((output) => output.enabled || selectedIds.has(output.id)),
    ...[...selectedIds]
      .filter((id) => !namedOutputIds.has(id))
      .map((id) => ({ id, name: id, enabled: false, sink_kind: "display" as const })),
  ]);
  const stateFor = (predicate: (ids: readonly string[]) => boolean) => {
    const values = selections.map(predicate);
    return {
      checked: values.length > 0 && values.every(Boolean),
      mixed: values.some(Boolean) && !values.every(Boolean),
    };
  };
  const submit = (transform: (ids: readonly string[], index: number) => string[]) => onChange(cues.map((cue, index) => {
    const ids = transform(explicitSelections[index], index);
    return { cueId: cue.id, output_id: ids[0] ?? null, output_ids: ids };
  }));

  return (
    <Field label={locale === "ru" ? "Выходы" : "Outputs"}>
      <div style={{ display: "grid", gap: 5, padding: "4px 0" }}>
        {visibleOutputs.map((output) => {
          const state = stateFor((ids) => ids.includes(output.id));
          const suffix = !output.enabled
            ? ` — ${namedOutputIds.has(output.id) ? t("common.disabled") : t("status.disconnected")}`
            : "";
          return (
            <CheckboxRow
              key={output.id}
              {...state}
              disabled={disabled}
              mixedLabel={mixedLabel}
              label={`${output.name}${output.sink_kind === "display" ? "" : ` — ${output.sink_kind.toUpperCase()}`}${suffix}`}
              onChange={(checked) => submit((_ids, index) => outputIdsAfterToggle(
                cues[index], output.id, checked, effectiveDefault?.id, namedOutputIds,
              ))}
            />
          );
        })}
        {selections.length === 1 && selections[0].length > 1 && (
          <span style={{ fontSize: 11, color: "var(--wc-text-muted)" }}>
            {locale === "ru" ? "cue будет воспроизводиться параллельно на всех выбранных выходах." : "The cue will play in parallel on all selected outputs."}
          </span>
        )}
      </div>
    </Field>
  );
}
