import { useEffect, useRef, useState } from "react";
import type { CueListSummary, CueListTcConfig, TcOnStop, TcRate } from "../../lib/types";
import { getCuelistTcConfig, setCuelistTcConfig } from "../../lib/commands";
import { useLocale } from "../../i18n";
import { Select } from "../common/Select";
import { DragNumber } from "../common/DragNumber";

const DEFAULT_CONFIG: CueListTcConfig = {
  enabled: false, rate: "30", freewheel_ms: 500, on_stop: "continue",
};
const TC_RATES: TcRate[] = ["24", "25", "29.97", "29.97df", "30"];
const TC_RATE_LABELS: Record<TcRate, string> = {
  "24": "24 fps", "25": "25 fps (PAL)", "29.97": "29.97 fps",
  "29.97df": "29.97df (NTSC DF)", "30": "30 fps",
};
const ON_STOP_LABELS: Record<TcOnStop, string> = {
  continue: "keepRunning", pause: "pauseRunning", stop: "stopRunning",
};
const fieldLabel: React.CSSProperties = { fontSize: 10, color: "var(--wc-text-muted)", marginBottom: 3 };
const inputStyle: React.CSSProperties = {
  background: "var(--wc-bg-app)", border: "1px solid var(--wc-border-strong)",
  borderRadius: 4, color: "var(--wc-text)", fontSize: 12, padding: "4px 6px",
  width: "100%", boxSizing: "border-box",
};

export function CueListTcPreferences({
  lists,
  selectedListId,
  onSelectedListIdChange,
}: {
  lists: CueListSummary[];
  selectedListId: string;
  onSelectedListIdChange: (id: string) => void;
}) {
  const { t, locale } = useLocale();
  const [config, setConfig] = useState<CueListTcConfig | null>(null);
  const [loadedListId, setLoadedListId] = useState<string | null>(null);
  const generation = useRef(0);

  useEffect(() => {
    const request = ++generation.current;
    setConfig(null);
    setLoadedListId(null);
    getCuelistTcConfig(selectedListId)
      .then((value) => {
        if (generation.current === request) {
          setConfig(value ?? DEFAULT_CONFIG);
          setLoadedListId(selectedListId);
        }
      })
      .catch((error) => {
        if (generation.current === request) console.error(error);
      });
    return () => { if (generation.current === request) generation.current += 1; };
  }, [selectedListId]);

  const apply = (patch: Partial<CueListTcConfig>) => {
    if (!config || loadedListId !== selectedListId) return;
    const listId = loadedListId;
    const request = generation.current;
    const next = { ...config, ...patch };
    setConfig(next);
    setCuelistTcConfig(next, listId).catch((error) => {
      if (generation.current === request) console.error(error);
    });
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
      <div>
        <div style={fieldLabel}>{t("cueList.cueList")}</div>
        <Select
          style={{ ...inputStyle, cursor: "pointer" }}
          value={selectedListId}
          onChange={(event) => onSelectedListIdChange(event.target.value)}
        >
          {lists.map((list) => <option key={list.id} value={list.id}>{list.name}</option>)}
        </Select>
      </div>
      {config && loadedListId === selectedListId ? (
        <>
          <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12 }}>
            <input
              type="checkbox"
              checked={config.enabled}
              onChange={(event) => apply({ enabled: event.target.checked })}
              style={{ accentColor: "var(--wc-accent)", width: 14, height: 14 }}
            />
            {locale === "ru" ? "Синхронизировать cue с входящим таймкодом" : "Sync cues from incoming timecode"}
          </label>
          <div>
            <div style={fieldLabel}>{t("components.expectedRate")}</div>
            <Select style={{ ...inputStyle, cursor: "pointer" }} value={config.rate}
              onChange={(event) => apply({ rate: event.target.value as TcRate })}>
              {TC_RATES.map((rate) => <option key={rate} value={rate}>{TC_RATE_LABELS[rate]}</option>)}
            </Select>
          </div>
          <div>
            <div style={fieldLabel}>{t("components.freewheel")}</div>
            <DragNumber min={0} max={2000} step={50} value={config.freewheel_ms}
              onChange={(event) => apply({ freewheel_ms: Math.max(0, Math.min(2000, Number(event.target.value) || 0)) })}
              style={{ ...inputStyle, fontFamily: "monospace" }} />
          </div>
          <div>
            <div style={fieldLabel}>{t("components.onStop")}</div>
            <Select style={{ ...inputStyle, cursor: "pointer" }} value={config.on_stop}
              onChange={(event) => apply({ on_stop: event.target.value as TcOnStop })}>
              {(["continue", "pause", "stop"] as TcOnStop[]).map((stop) => (
                <option key={stop} value={stop}>{t(`sweepUi.${ON_STOP_LABELS[stop]}`)}</option>
              ))}
            </Select>
          </div>
          <div style={{ fontSize: 10, color: "var(--wc-text-faint)", lineHeight: 1.4 }}>
            {locale === "ru"
              ? "Включите приём TC выше, затем задайте время срабатывания cue в Инспекторе → Триггеры."
              : "Enable TC receive above, then set cue trigger times in Inspector → Triggers."}
          </div>
        </>
      ) : <div style={{ fontSize: 11, color: "var(--wc-text-muted)" }}>{t("common.loading")}</div>}
    </div>
  );
}
