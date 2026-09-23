// Devamp Cue main tab: which vamping cues to release, and whether the target
// stops at the end of its current slice or continues into the next one.

import type { DevampCueData } from "../../lib/types";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { Section, Segmented } from "./Field";
import { CueTargetPicker } from "./CueTargetPicker";
import { useLocale } from "../../i18n";

export function DevampTab({
  cue,
  onSave,
}: {
  cue: DevampCueData;
  onSave: (p: Partial<DevampCueData>) => void;
}) {
  const { t, locale } = useLocale();
  const allCues = useWorkspaceStore((s) => s.cues);
  const targetIds: string[] = cue.target_cue_ids ?? [];

  return (
    <>
      <Section
        title={t("inspectorCueUi.devampTargets")}
        hint={locale === "ru" ? "Выберите cue, которым будет управлять эта команда." : t("help.go")}
      >
        <CueTargetPicker
          allCues={allCues}
          selfId={cue.id}
          selectedIds={targetIds}
          filterTypes={["audio", "video", "group"]}
          onChange={(ids) => {
            const nums = ids
              .map((id) => allCues.find((c) => c.id === id)?.number)
              .filter((n): n is string => n != null);
            onSave({ target_cue_ids: ids, target_cue_numbers: nums });
          }}
        />
        <div style={{ height: 8 }} />
      </Section>

      <Section title={locale === "ru" ? "После текущего повтора" : "After the current pass"}>
        <Segmented
          options={[
            { value: "continue", label: locale === "ru" ? "Продолжить дальше" : t("actions.resume"), hint: locale === "ru" ? "Продолжить воспроизведение после текущего повтора." : t("actions.resume") },
            { value: "stop", label: locale === "ru" ? "Остановиться" : t("common.stop"), hint: locale === "ru" ? "Остановиться на границе Slice после текущего повтора." : t("common.stop") },
          ]}
          value={cue.stop_at_end ? "stop" : "continue"}
          onChange={(v) => onSave({ stop_at_end: v === "stop" })}
        />
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginTop: -4, marginBottom: 6 }}>
          {locale === "ru"
            ? cue.stop_at_end
              ? "Выбранный cue завершит текущий повтор и остановится на границе Slice."
              : "Выбранный cue закончит текущий повтор и продолжит воспроизведение дальше."
            : cue.stop_at_end
              ? "The target finishes its current pass, then stops at the slice boundary."
              : "The target finishes its current pass, then continues into the next slice."}
        </div>
      </Section>
    </>
  );
}
