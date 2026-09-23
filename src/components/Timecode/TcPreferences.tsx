// Timecode section shown in Preferences → Network.
// Configures the TC receiver (enable, source, MIDI input port).

import { useEffect, useState } from "react";
import type { TcMachineConfig, DeviceInfo } from "../../lib/types";
import { getTcConfig, setTcConfig, listTcMidiInputPorts, listInputDevices } from "../../lib/commands";
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

export function TcPreferences() {
  const { t } = useLocale();
  const [config, setConfigState] = useState<TcMachineConfig>({
    enabled: false,
    receiver_config: { source: "mtc", midi_port: null, ltc_device_id: null },
  });
  const [ports, setPorts] = useState<string[]>([]);
  const [inputDevices, setInputDevices] = useState<DeviceInfo[]>([]);

  useEffect(() => {
    getTcConfig().then(setConfigState).catch(console.error);
    listTcMidiInputPorts().then(setPorts).catch(console.error);
    listInputDevices().then(setInputDevices).catch(console.error);
  }, []);

  const apply = async (next: TcMachineConfig) => {
    setConfigState(next);
    try { await setTcConfig(next); } catch (e) { console.error(e); }
  };

  return (
    <div style={{ marginBottom: 24 }}>
      <div style={labelStyle}>{t("preferencesUi.timecodeReceive")}</div>

      {/* Enable */}
      <div style={{ marginBottom: 12 }}>
        <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12 }}>
          <input
            type="checkbox"
            checked={config.enabled}
            onChange={(e) => apply({ ...config, enabled: e.target.checked })}
            style={{ accentColor: "var(--wc-accent)", width: 14, height: 14 }}
          />
          {t("preferencesUi.enableTc")}
        </label>
      </div>

      {config.enabled && (
        <>
          {/* Source */}
          <div style={{ marginBottom: 10 }}>
            <div style={{ fontSize: 10, color: "var(--wc-text-muted)", marginBottom: 2 }}>{t("preferencesUi.source")}</div>
            <Select
              style={{ ...inputStyle, cursor: "pointer" }}
              value={config.receiver_config.source}
              onChange={(e) => apply({
                ...config,
                receiver_config: { ...config.receiver_config, source: e.target.value as "mtc" | "ltc" },
              })}
            >
              <option value="mtc">{t("preferencesUi.mtcSource")}</option>
              <option value="ltc">{t("preferencesUi.ltcSource")}</option>
            </Select>
          </div>

          {/* MIDI port (MTC) */}
          {config.receiver_config.source === "mtc" && (
            <div style={{ marginBottom: 10 }}>
              <div style={{ fontSize: 10, color: "var(--wc-text-muted)", marginBottom: 2 }}>{t("preferencesUi.midiInputPort")}</div>
              <Select
                style={{ ...inputStyle, cursor: "pointer" }}
                value={config.receiver_config.midi_port ?? ""}
                onChange={(e) => apply({
                  ...config,
                  receiver_config: { ...config.receiver_config, midi_port: e.target.value || null },
                })}
              >
                <option value="">{t("preferencesUi.firstAvailableDash")}</option>
                {ports.map((p) => (
                  <option key={p} value={p}>{p}</option>
                ))}
                {config.receiver_config.midi_port && !ports.includes(config.receiver_config.midi_port) && (
                  <option value={config.receiver_config.midi_port}>
                    {config.receiver_config.midi_port} ({t("preferencesUi.notFound")})
                  </option>
                )}
              </Select>
              {ports.length === 0 && (
                <div style={{ marginTop: 4, fontSize: 11, color: "var(--wc-text-muted)" }}>
                  {t("preferencesUi.noMidiDevice")}
                </div>
              )}
            </div>
          )}

          {/* Audio input device (LTC) */}
          {config.receiver_config.source === "ltc" && (
            <div style={{ marginBottom: 10 }}>
              <div style={{ fontSize: 10, color: "var(--wc-text-muted)", marginBottom: 2 }}>{t("preferencesUi.audioInputDevice")}</div>
              <Select
                style={{ ...inputStyle, cursor: "pointer" }}
                value={config.receiver_config.ltc_device_id ?? ""}
                onChange={(e) => apply({
                  ...config,
                  receiver_config: { ...config.receiver_config, ltc_device_id: e.target.value || null },
                })}
              >
                <option value="">{t("preferencesUi.defaultInput")}</option>
                {inputDevices.map((d) => (
                  <option key={d.id} value={d.id}>{d.name}</option>
                ))}
                {config.receiver_config.ltc_device_id && !inputDevices.some((d) => d.id === config.receiver_config.ltc_device_id) && (
                  <option value={config.receiver_config.ltc_device_id}>
                    {config.receiver_config.ltc_device_id} ({t("preferencesUi.notFound")})
                  </option>
                )}
              </Select>
              {inputDevices.length === 0 && (
                <div style={{ marginTop: 4, fontSize: 11, color: "var(--wc-text-muted)" }}>
                  {t("preferencesUi.noAudioDevices")}
                </div>
              )}
            </div>
          )}

          <div style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>
            {t("preferencesUi.timecodeHint")}
          </div>
        </>
      )}
    </div>
  );
}
