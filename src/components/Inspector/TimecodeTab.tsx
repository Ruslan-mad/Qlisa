// Inspector tab for Timecode cues.

import { useEffect, useState } from "react";
import type { TimecodeCueData, TcRate } from "../../lib/types";
import { listMidiOutputPorts, getOutputPatches } from "../../lib/commands";
import type { OutputPatch } from "../../lib/types";
import { Select } from "../common/Select";
import { useLocale } from "../../i18n";

interface Props {
  cue: TimecodeCueData;
  onSave: (partial: Partial<TimecodeCueData>) => void;
}

const inputStyle: React.CSSProperties = {
  background: "var(--wc-bg-app)",
  border: "1px solid var(--wc-border-strong)",
  borderRadius: 4,
  color: "var(--wc-text)",
  fontSize: 12,
  padding: "3px 6px",
};

const selectStyle: React.CSSProperties = { ...inputStyle, cursor: "pointer", width: "100%" };
const labelStyle: React.CSSProperties = { fontSize: 10, color: "var(--wc-text-muted)", marginBottom: 2 };
const fieldStyle: React.CSSProperties = { marginBottom: 12 };

const TC_RATES: TcRate[] = ["24", "25", "29.97", "29.97df", "30"];
const TC_RATE_LABELS: Record<TcRate, string> = {
  "24": "24 fps",
  "25": "25 fps (PAL)",
  "29.97": "29.97 fps",
  "29.97df": "29.97df (NTSC DF)",
  "30": "30 fps",
};

export function TimecodeTab({ cue, onSave }: Props) {
  const { t, locale } = useLocale();
  const [midiPorts, setMidiPorts] = useState<string[]>([]);
  const [outputPatches, setOutputPatches] = useState<OutputPatch[]>([]);

  useEffect(() => {
    listMidiOutputPorts().then(setMidiPorts).catch(console.error);
    getOutputPatches().then(setOutputPatches).catch(console.error);
  }, []);

  const isMtc = cue.tc_type === "mtc";

  const formatPos = (p: typeof cue.start_frame | null | undefined) =>
    p ? `${String(p.h).padStart(2,"0")}:${String(p.m).padStart(2,"0")}:${String(p.s).padStart(2,"0")}${cue.rate.endsWith("df") ? ";" : ":"}${String(p.f).padStart(2,"0")}` : "";

  return (
    <div>
      {/* TC Output Type */}
      <div style={fieldStyle}>
        <div style={labelStyle}>{t("output.output")}</div>
        <Select
          style={selectStyle}
          value={cue.tc_type}
          onChange={(e) => onSave({ tc_type: e.target.value as "mtc" | "ltc" })}
        >
          <option value="mtc">{t("inspector.timecodeMtc")}</option>
          <option value="ltc">{t("inspector.timecodeLtc")}</option>
        </Select>
      </div>

      {/* Frame Rate */}
      <div style={fieldStyle}>
        <div style={labelStyle}>{t("inspector.rate")}</div>
        <Select
          style={selectStyle}
          value={cue.rate}
          onChange={(e) => onSave({ rate: e.target.value as TcRate })}
        >
          {TC_RATES.map((r) => (
            <option key={r} value={r}>{TC_RATE_LABELS[r]}</option>
          ))}
        </Select>
      </div>

      {/* MIDI Port (MTC) */}
      {isMtc && (
        <div style={fieldStyle}>
        <div style={labelStyle}>{t("preferencesUi.midiInputPort")}</div>
          <Select
            style={selectStyle}
            value={cue.midi_port ?? ""}
            onChange={(e) => onSave({ midi_port: e.target.value || null })}
          >
            <option value="">{t("inspector.firstAvailable")}</option>
            {midiPorts.map((p) => (
              <option key={p} value={p}>{p}</option>
            ))}
            {cue.midi_port && !midiPorts.includes(cue.midi_port) && (
              <option value={cue.midi_port}>{cue.midi_port} ({t("preferencesUi.notFound")})</option>
            )}
          </Select>
          {midiPorts.length === 0 && (
            <div style={{ marginTop: 4, fontSize: 11, color: "var(--wc-text-muted)" }}>
              {t("inspector.noMidiPorts")}
            </div>
          )}
        </div>
      )}

      {/* LTC Output Patch */}
      {!isMtc && (
        <div style={fieldStyle}>
        <div style={labelStyle}>{t("output.output")}</div>
          <Select
            style={selectStyle}
            value={outputPatches.find((p) => p.id === cue.output_patch_id)?.kind === "main" ? "" : (cue.output_patch_id ?? "")}
            onChange={(e) => onSave({ output_patch_id: e.target.value || null })}
          >
            <option value="">— {t("audioBusUi.main")} —</option>
            {outputPatches.filter((p) => p.kind === "aux" && p.enabled).map((p) => (
              <option key={p.id} value={p.id}>{p.name}</option>
            ))}
          </Select>
          <div style={{ marginTop: 4, fontSize: 11, color: "var(--wc-text-muted)" }}>
            {t("inspector.ltcHint")}
          </div>
        </div>
      )}

      {/* Start Frame */}
      <div style={{ display: "flex", gap: 12, ...fieldStyle }}>
        <div style={{ flex: 1 }}>
        <div style={labelStyle}>{t("inspector.startTime")}</div>
          <input
            style={{ ...inputStyle, width: "100%", fontFamily: "monospace" }}
            value={formatPos(cue.start_frame)}
            placeholder="00:00:00:00"
            onChange={(e) => {
              // Parse HH:MM:SS:FF — simple heuristic
              const [hh,mm,ss,ff] = e.target.value.replace(";", ":").split(":").map(Number);
              if ([hh,mm,ss,ff].some(isNaN)) return;
              onSave({ start_frame: { h: hh, m: mm, s: ss, f: ff, rate: cue.rate } });
            }}
          />
        </div>
        <div style={{ flex: 1 }}>
        <div style={labelStyle}>{t("inspector.endTime")}</div>
          <input
            style={{ ...inputStyle, width: "100%", fontFamily: "monospace" }}
            value={cue.end_frame ? formatPos(cue.end_frame) : ""}
            placeholder={t("inspector.endUntilStopped")}
            onChange={(e) => {
              const v = e.target.value.trim();
              if (!v) { onSave({ end_frame: null }); return; }
              const [hh,mm,ss,ff] = v.replace(";", ":").split(":").map(Number);
              if ([hh,mm,ss,ff].some(isNaN)) return;
              onSave({ end_frame: { h: hh, m: mm, s: ss, f: ff, rate: cue.rate } });
            }}
          />
        </div>
      </div>

      <div style={{ marginTop: 10, fontSize: 11, color: "var(--wc-text-muted)" }}>
        {locale === "ru"
          ? "Создаётся непрерывный поток таймкода. Несколько cue таймкода могут работать одновременно. Остановите cue, чтобы завершить поток."
          : "Generates a continuous timecode stream. Multiple Timecode Cues can run simultaneously. Stop the cue to end the stream."}
      </div>
    </div>
  );
}
