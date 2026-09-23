// Optional machine-local Aux routes shown directly below Main and Preview in
// Preferences → Audio. Main and Preview are owned by the rows above; this
// component intentionally exposes only user-created Aux routes.

import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { DeviceInfo, OutputPatch } from "../../lib/types";
import { getOutputPatchTable, listAudioDevices, removeOutputPatch, setOutputPatch, testOutputPatch } from "../../lib/commands";
import { Select } from "../common/Select";
import { useLocale } from "../../i18n";

const inputStyle: React.CSSProperties = {
  background: "var(--wc-bg-app)",
  border: "1px solid var(--wc-border-strong)",
  borderRadius: 4,
  color: "var(--wc-text)",
  fontSize: 12,
  padding: "3px 7px",
  width: "100%",
  boxSizing: "border-box",
};
const buttonStyle: React.CSSProperties = {
  padding: "3px 10px",
  background: "var(--wc-bg-surface)",
  border: "1px solid var(--wc-border-strong)",
  borderRadius: 4,
  color: "var(--wc-text)",
  fontSize: 11,
  cursor: "pointer",
  whiteSpace: "nowrap",
};
const removeButtonStyle: React.CSSProperties = {
  ...buttonStyle,
  padding: "3px 8px",
  color: "var(--wc-text-muted)",
};
export function OutputPatchesPanel({ backend }: { backend?: string }) {
  const { t } = useLocale();
  const [patches, setPatches] = useState<OutputPatch[]>([]);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [testState, setTestState] = useState<Record<string, "testing" | "error"> >({});

  const reload = () => getOutputPatchTable().then((table) => setPatches(table.patches.filter((patch) => patch.kind === "aux" && patch.enabled))).catch(console.error);
  const refresh = () => { reload(); listAudioDevices(backend).then(setDevices).catch(console.error); };

  useEffect(() => {
    refresh();
    window.addEventListener("focus", refresh);
    const unlisten = listen("device-changed", refresh);
    return () => { window.removeEventListener("focus", refresh); void unlisten.then((unsubscribe) => unsubscribe()); };
  }, [backend]);

  const nextAuxName = () => {
    const used = new Set(patches.map((patch) => patch.name));
    let index = 1;
    while (used.has(t("audioBusUi.auxNumber", { index }))) index += 1;
    return t("audioBusUi.auxNumber", { index });
  };
  const handleAdd = async () => {
    try { await setOutputPatch(null, nextAuxName(), "", [0, 1], "aux", true); await reload(); }
    catch (error) { console.error(error); }
  };
  const setDevice = async (patch: OutputPatch, deviceId: string) => {
    try { await setOutputPatch(patch.id, patch.name, deviceId, patch.channels, "aux", true); setPatches((current) => current.map((item) => item.id === patch.id ? { ...item, device_id: deviceId } : item)); }
    catch (error) { console.error(error); }
  };
  const setPair = async (patch: OutputPatch, pair: number) => {
    const channels = [pair * 2, pair * 2 + 1];
    try {
      await setOutputPatch(patch.id, patch.name, patch.device_id, channels, "aux", true);
      setPatches((current) => current.map((item) => item.id === patch.id ? { ...item, channels } : item));
    } catch (error) {
      console.error(error);
    }
  };
  const runTest = async (patch: OutputPatch) => {
    if (!patch.device_id) return;
    setTestState((current) => ({ ...current, [patch.id]: "testing" }));
    try { await testOutputPatch(patch.device_id, patch.channels); setTestState((current) => { const next = { ...current }; delete next[patch.id]; return next; }); }
    catch { setTestState((current) => ({ ...current, [patch.id]: "error" })); }
  };
  const remove = async (id: string) => { try { await removeOutputPatch(id); await reload(); } catch (error) { console.error(error); } };

  return (
    <div style={{ marginTop: 4 }}>
      {patches.map((patch, index) => {
        const deviceMissing = Boolean(patch.device_id) && !devices.some((device) => device.id === patch.device_id);
        const device = devices.find((item) => item.id === patch.device_id);
        const channelsValid = patch.channels.length === 2 && patch.channels[1] === patch.channels[0] + 1 && (!device || patch.channels[1] < device.channels);
        const status = testState[patch.id] === "testing" ? t("audioBusUi.testing") : testState[patch.id] === "error" ? t("audioBusUi.testFailed") : deviceMissing || !patch.device_id || !channelsValid ? t("audioBusUi.unavailable") : t("audioBusUi.configured");
        return (
          <div key={patch.id} style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 8, minHeight: 28, flexWrap: "wrap", rowGap: 6 }}>
            <label style={{ width: 170, flexShrink: 0, textAlign: "right", fontSize: 12, color: "var(--wc-text-secondary)" }}>
              {t("audioBusUi.auxNumber", { index: index + 1 })}
            </label>
            <div style={{ flex: 1, minWidth: 0, display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap", rowGap: 6 }}>
              <div style={{ flex: "1 1 260px", minWidth: "min(220px, 100%)" }}>
                <Select style={{ ...inputStyle, ...(deviceMissing ? { borderColor: "#f59e0b", color: "#f59e0b" } : {}) }} value={patch.device_id} onChange={(event) => void setDevice(patch, event.target.value)} aria-label={t("preferencesUi.outputDevice")}>
                  {deviceMissing && <option value={patch.device_id}>⚠ {t("audioBusUi.unavailable")} — {patch.device_id}</option>}
                  <option value="">— {t("common.none")} —</option>
                  {devices.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}
                </Select>
              </div>
              <div style={{ flex: "0 1 128px", minWidth: 102 }}>
                <Select
                  style={inputStyle}
                  value={patch.channels.length >= 2 && patch.channels[1] === patch.channels[0] + 1 ? Math.floor(patch.channels[0] / 2) : ""}
                  disabled={!patch.device_id || deviceMissing}
                  onChange={(event) => {
                    const pair = Number(event.target.value);
                    if (Number.isInteger(pair) && pair >= 0) {
                      void setPair(patch, pair);
                    }
                  }}
                  aria-label={t("preferencesUi.outputPair")}
                >
                  <option value="">— {t("audioBusUi.channels")} —</option>
                  {Array.from({ length: Math.max(1, Math.floor((device?.channels ?? 2) / 2)) }, (_, pair) => (
                    <option key={pair} value={pair}>{t("preferencesUi.outputPairLabel", { first: pair * 2 + 1, last: pair * 2 + 2 })}</option>
                  ))}
                </Select>
              </div>
              <span style={{ minWidth: 70, fontSize: 11, color: status === t("audioBusUi.configured") ? "var(--wc-text-muted)" : "#f59e0b", whiteSpace: "nowrap" }}>{status}</span>
              <button style={buttonStyle} disabled={!patch.device_id || testState[patch.id] === "testing"} onClick={() => void runTest(patch)}>{t("preferencesExtra.test")}</button>
              <button style={removeButtonStyle} title={t("common.remove")} aria-label={`${t("common.remove")} ${patch.name || t("audioBusUi.auxNumber", { index: index + 1 })}`} onClick={() => void remove(patch.id)}>✕</button>
            </div>
          </div>
        );
      })}
      <div style={{ display: "flex", alignItems: "center", gap: 12, marginTop: patches.length > 0 ? 0 : 4 }}>
        <span style={{ width: 170, flexShrink: 0 }} aria-hidden="true" />
        <button style={buttonStyle} onClick={() => void handleAdd()}>
          + {t("audioBusUi.addAux")}
        </button>
      </div>
    </div>
  );
}
