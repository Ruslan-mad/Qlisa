// MIDI File Cue main tab: where the file is sent and how fast it plays,
// plus a read-only summary of what the backend actually parsed.
//
// The summary is not decoration: a MIDI file gives no other feedback until it
// is played, so showing its length, track count and channels is how you
// confirm you picked the right file and patched the right instrument.

import { useEffect, useState } from "react";
import type { MidiFileCueData } from "../../lib/types";
import { listMidiOutputPorts } from "../../lib/commands";
import { Field, Section, inputStyle } from "./Field";
import { Select } from "../common/Select";
import { NumberInput } from "./Field";
import { useLocale } from "../../i18n";

/** mm:ss.t — long enough for a show cue, short enough to scan. */
function formatDuration(ms: number): string {
  const totalSeconds = ms / 1000;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds - minutes * 60;
  return `${minutes}:${seconds.toFixed(1).padStart(4, "0")}`;
}

function russianTrackWord(count: number): string {
  const mod10 = count % 10;
  const mod100 = count % 100;
  if (mod10 === 1 && mod100 !== 11) return "трек";
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) return "трека";
  return "треков";
}

export function MidiFileTab({
  cue,
  onSave,
}: {
  cue: MidiFileCueData;
  onSave: (p: Partial<MidiFileCueData>) => void;
}) {
  const { t, locale } = useLocale();
  const [ports, setPorts] = useState<string[]>([]);

  useEffect(() => {
    listMidiOutputPorts().then(setPorts).catch(console.error);
  }, []);

  const rate = cue.playback_rate ?? 1;
  const writtenMs = cue.sequence_duration_ms;
  const parsed = writtenMs != null;

  return (
    <>
      <Section title="Destination">
        <Field label="Port">
          {ports.length > 0 ? (
            <Select
              style={inputStyle}
              value={cue.port_name ?? ""}
              onChange={(e) => onSave({ port_name: e.target.value })}
            >
              <option value="">{t("sweepUi.noneOption")}</option>
              {ports.map((p) => (
                <option key={p} value={p}>{p}</option>
              ))}
              {cue.port_name && !ports.includes(cue.port_name) && (
                <option value={cue.port_name}>{cue.port_name} ({locale === "ru" ? "не найден" : "not found"})</option>
              )}
            </Select>
          ) : (
            <input
              style={inputStyle}
              placeholder={t("preferences.outputNamePlaceholder")}
              defaultValue={cue.port_name ?? ""}
              key={`port-${cue.id}`}
              onBlur={(e) => onSave({ port_name: e.target.value })}
            />
          )}
        </Field>
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginTop: -4 }}>
          {locale === "ru" ? "Весь файл отправляется в один порт; каналы внутри файла выполняют маршрутизацию." : "The whole file goes to one port; the channels inside it do the routing."}
        </div>
      </Section>

      <Section title="Playback">
        <Field label="Rate">
          <NumberInput
            value={rate}
            step={0.05}
            min={0.05}
            max={20}
            width={90}
            onCommit={(v) => onSave({ playback_rate: v })}
          />
        </Field>
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginTop: -4 }}>
          {locale === "ru" ? "Множитель для всех темпов файла — 0,5 означает половинную скорость. Изменения темпа, записанные в файл, соблюдаются в любом случае." : "A multiplier on every tempo in the file — 0.5 is half speed. Tempo changes written into the file are followed either way."}
        </div>
      </Section>

      <Section title="File">
        {cue.parse_error ? (
          <div style={{ fontSize: 12, color: "#f87171" }}>{cue.parse_error}</div>
        ) : !cue.file_path ? (
          <div style={{ fontSize: 12, color: "var(--wc-text-faint)" }}>
            {locale === "ru" ? "Файл не назначен — выберите его на вкладке «Основные»." : "No file assigned — pick one in the Basics tab."}
          </div>
        ) : !parsed ? (
          <div style={{ fontSize: 12, color: "#f87171" }}>{t("components.fileNotFound")}</div>
        ) : (
          <div style={{ fontSize: 12, color: "var(--wc-text-secondary)", lineHeight: 1.7 }}>
            <div>
              {locale === "ru" ? "Длительность" : "Length"}{" "}
              <strong style={{ color: "var(--wc-text)", fontVariantNumeric: "tabular-nums" }}>
                {formatDuration(writtenMs)}
              </strong>
              {rate !== 1 && (
                <>
                  {" → "}
                  <strong style={{ color: "var(--wc-text)", fontVariantNumeric: "tabular-nums" }}>
                    {formatDuration(writtenMs / rate)}
                  </strong>
                  {locale === "ru" ? ` при скорости ${rate}×` : ` at ${rate}×`}
                </>
              )}
            </div>
            <div>
              {locale === "ru"
                ? String(cue.track_count ?? 0) + " " + russianTrackWord(cue.track_count ?? 0) + (cue.channels && cue.channels.length > 0 ? " · каналы " + cue.channels.join(", ") : " · нет каналов")
                : <>{cue.track_count ?? 0} track{(cue.track_count ?? 0) === 1 ? "" : "s"}{" · channels "}{cue.channels && cue.channels.length > 0 ? cue.channels.join(", ") : "none"}</>}
            </div>
          </div>
        )}
      </Section>
    </>
  );
}
