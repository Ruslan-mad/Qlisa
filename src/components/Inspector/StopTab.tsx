// Stop Cue main tab: targets (all cues or a subset) and stop mode.

import type { StopCueData } from "../../lib/types";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { Section, Segmented } from "./Field";
import { CueTargetPicker } from "./CueTargetPicker";
import { useLocale } from "../../i18n";

export function StopTab({
  cue,
  onSave,
}: {
  cue: StopCueData;
  onSave: (p: Partial<StopCueData>) => void;
}) {
  const { t, locale } = useLocale();
  const allCues = useWorkspaceStore((s) => s.cues);
  const targetIds: string[] = cue.target_cue_ids ?? [];

  return (
    <>
      <Section
        title={t("inspectorCueUi.stopTargets")}
        hint={locale === "ru" ? "Выберите cue, которым будет управлять эта команда. Если ничего не выбрано, команда остановит все cue." : undefined}
      >
        <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer", marginBottom: 8 }}>
          <input
            type="radio"
            checked={targetIds.length === 0}
            onChange={() => onSave({ target_cue_ids: [], target_cue_numbers: [] })}
          />
          <span style={{ fontSize: 13, color: "var(--wc-text)" }}>{t("components.allCues")}</span>
        </label>
        <CueTargetPicker
          allCues={allCues}
          selfId={cue.id}
          selectedIds={targetIds}
          onChange={(ids) => {
            const nums = ids
              .map((id) => allCues.find((c) => c.id === id)?.number)
              .filter((n): n is string => n != null);
            onSave({ target_cue_ids: ids, target_cue_numbers: nums });
          }}
        />
        <div style={{ height: 8 }} />
      </Section>

      <Section title="Stop Mode">
        <Segmented
          options={[
            { value: "soft", label: t("components.soft"), hint: t("inspector.fade") },
            { value: "hard", label: t("components.hard"), hint: t("common.stop") },
          ]}
          value={cue.hard_stop_mode ? "hard" : "soft"}
          onChange={(v) => onSave({ hard_stop_mode: v === "hard" })}
        />
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginBottom: 6 }}>
          {cue.hard_stop_mode ? t("sweepUi.immediateCut") : t("sweepUi.shortFadeStop")}
        </div>
      </Section>
    </>
  );
}
