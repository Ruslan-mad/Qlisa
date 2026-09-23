// MIDI trigger section shown in Preferences → Network.
//
// Machine config, not workspace config: the port name belongs to the hardware
// in front of the operator, so a show carried to another rig keeps its per-cue
// triggers and picks up that rig's input here.

import { useEffect, useState } from "react";
import type { MidiTriggerConfig } from "../../lib/types";
import { getMidiTriggerConfig, setMidiTriggerConfig, listMidiInputPorts } from "../../lib/commands";
import { Select } from "../common/Select";
import { useLocale } from "../../i18n";

const inputStyle: React.CSSProperties = {
  background: "var(--wc-bg-app)",
  border: "1px solid var(--wc-border-strong)",
  borderRadius: 4,
  color: "var(--wc-text)",
  fontSize: 12,
  padding: "4px 8px",
  width: "100%",
};

const labelStyle: React.CSSProperties = {
  fontSize: 10, fontWeight: 600, color: "var(--wc-text-muted)",
  textTransform: "uppercase", letterSpacing: "0.07em",
  marginBottom: 10, paddingBottom: 5,
  borderBottom: "1px solid var(--wc-border)",
};

export function MidiTriggerPreferences() {
  const { t } = useLocale();
  const [config, setConfig] = useState<MidiTriggerConfig>({ enabled: false, port: null });
  const [ports, setPorts] = useState<string[]>([]);

  useEffect(() => {
    getMidiTriggerConfig().then(setConfig).catch(console.error);
    listMidiInputPorts().then(setPorts).catch(console.error);
  }, []);

  const apply = async (next: MidiTriggerConfig) => {
    setConfig(next);
    try {
      await setMidiTriggerConfig(next);
    } catch (e) {
      console.error(e);
    }
  };

  return (
    <div style={{ marginBottom: 24 }}>
      <div style={labelStyle}>{t("preferencesUi.midiTriggers")}</div>

      <div style={{ marginBottom: 12 }}>
        <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12 }}>
          <input
            type="checkbox"
            checked={config.enabled}
            onChange={(e) => apply({ ...config, enabled: e.target.checked })}
            style={{ accentColor: "var(--wc-accent)", width: 14, height: 14 }}
          />
          {t("preferencesUi.fireIncomingMidi")}
        </label>
      </div>

      {config.enabled && (
        <div style={{ marginBottom: 12 }}>
          <div style={{ fontSize: 11, color: "var(--wc-text-secondary)", marginBottom: 4 }}>
            {t("preferencesUi.midiInputPort")}
          </div>
          {ports.length > 0 ? (
            <Select
              style={{ ...inputStyle, cursor: "pointer" }}
              value={config.port ?? ""}
              onChange={(e) => apply({ ...config, port: e.target.value || null })}
            >
              <option value="">{t("preferencesUi.firstAvailable")}</option>
              {ports.map((p) => (
                <option key={p} value={p}>{p}</option>
              ))}
              {config.port && !ports.includes(config.port) && (
                <option value={config.port}>{config.port} ({t("preferencesUi.notFound")})</option>
              )}
            </Select>
          ) : (
            <div style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>
              {t("preferencesUi.noMidiPorts")}
            </div>
          )}
        </div>
      )}

      <div style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>
        {t("preferencesUi.midiTriggerHint")}
      </div>
    </div>
  );
}
