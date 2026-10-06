// Basics tab: cue identity only (number, name, color, notes, media file,
// flow). Type-specific behaviour lives in each cue type's own tab.

import { Field, Grid2, MiniField, Section, ToggleRow, inputStyle } from "./Field";
import { ColorPicker } from "./ColorPicker";
import { useLocale } from "../../i18n";
import { cueNotesText, cueNotesUpdate } from "../../lib/cueNotes";

export function BasicsTab({
  cue,
  onSave,
}: {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  cue: any;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  onSave: (p: Partial<any>) => void;
}) {
  const { t } = useLocale();
  return (
    <>
      <Section title={t("inspector.identity")}>
        <Grid2>
          <MiniField label="Cue #">
            <input
              style={inputStyle}
              defaultValue={cue.number ?? ""}
              onBlur={(e) => onSave({ number: e.target.value || null })}
            />
          </MiniField>
          <MiniField label="Color">
            <div style={{ paddingTop: 3 }}>
              <ColorPicker value={cue.color} onChange={(c) => onSave({ color: c })} />
            </div>
          </MiniField>
        </Grid2>
        <Field label="Name">
          <input
            style={inputStyle}
            defaultValue={cue.name}
            onBlur={(e) => onSave({ name: e.target.value })}
          />
        </Field>
        <Field label="Notes">
          <textarea
            style={{ ...inputStyle, resize: "vertical", minHeight: 56 }}
            defaultValue={cueNotesText(cue)}
            onBlur={(e) => onSave(cueNotesUpdate(cue, e.target.value))}
          />
        </Field>
      </Section>

      <Section title={t("inspector.flow")}>
        <ToggleRow
          label={t("sweepUi.disableCue")}
          checked={cue.is_disabled ?? false}
          onToggle={(v) => onSave({ is_disabled: v })}
        />
      </Section>
    </>
  );
}
