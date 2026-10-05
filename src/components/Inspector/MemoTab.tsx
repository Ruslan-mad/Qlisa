// Memo Cue main tab: the note itself.
//
// A Memo performs no action — the text *is* the cue. It shows in the cue
// list's Notes column, so a Memo reads as a stage direction in the middle of
// the stack. Imported "[Unconverted QLab …]" placeholders also use this field.

import type { MemoCueData } from "../../lib/types";
import { Section, inputStyle } from "./Field";
import { useLocale } from "../../i18n";

export function MemoTab({
  cue,
  onSave,
}: {
  cue: MemoCueData;
  onSave: (p: Partial<MemoCueData>) => void;
}) {
  const { t, locale } = useLocale();
  return (
    <Section title="Memo">
      <textarea
        style={{ ...inputStyle, minHeight: 120, resize: "vertical" }}
        defaultValue={cue.memo_text ?? ""}
        key={`memo-${cue.id}`}
        placeholder={t("cueList.notes")}
        onBlur={(e) => onSave({ memo_text: e.target.value })}
      />
      <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginTop: 4 }}>
        {locale === "ru"
          ? "Отображается в столбце «Заметки» списка cue. Memo ничего не делает при GO и завершается мгновенно, поэтому Auto-Continue и Auto-Follow продолжают цепочку."
          : "Shown in the cue list's Notes column. A Memo does nothing on GO — it completes instantly, so Auto-Continue and Auto-Follow still chain through it."}
      </div>
    </Section>
  );
}
