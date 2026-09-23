// Camera tab: local capture, generic mpv URLs, plus managed NDI/SRT inputs.
import { useEffect, useState, type ReactNode } from "react";
import type { CameraCueData, CameraDeviceInfo, CameraSource, NdiQuality, NdiSourceInfo, SrtSettings } from "../../lib/types";
import { listCameraDevices, listNdiSources, openExternalUrl } from "../../lib/commands";
import { Field, inputStyle } from "./Field";
import { Select } from "../common/Select";
import { useLocale } from "../../i18n";

const freshSrt = (): SrtSettings => ({ enabled: true, mode: "listener", host: "", port: 9000, latency_ms: 120, passphrase: null, stream_id: null, payload_size: null, too_late_packet_drop: true, bitrate_kbps: null, width: null, height: null, fps: null, codec: null });
const refreshButtonStyle = { padding: "3px 10px", fontSize: 13, borderRadius: 4, border: "1px solid var(--wc-border-strong)", background: "var(--wc-bg-surface)", color: "var(--wc-text)", cursor: "pointer" };
function Hint({ children }: { children: ReactNode }) { return <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginBottom: 10 }}>{children}</div>; }

export function CameraTab({ cue, onSave }: { cue: CameraCueData; onSave: (p: Partial<CameraCueData>) => void }) {
  const { t, locale } = useLocale();
  const [devices, setDevices] = useState<CameraDeviceInfo[]>([]);
  const [ndiSources, setNdiSources] = useState<NdiSourceInfo[]>([]);
  const [loadingDevices, setLoadingDevices] = useState(false);
  const [loadingNdi, setLoadingNdi] = useState(false);
  const [ndiError, setNdiError] = useState<string | null>(null);
  const source: CameraSource = cue.source ?? { kind: "device", id: "", name: "" };
  const kind = source.kind === "network" ? source.input.protocol : source.kind;
  const srt = source.kind === "network" && source.input.protocol === "srt" ? source.input.settings : freshSrt();
  const ndiName = source.kind === "network" && source.input.protocol === "ndi" ? source.input.source_name : "";
  const ndiQuality: NdiQuality = cue.ndi_quality ?? "highest";
  const refreshDevices = () => { setLoadingDevices(true); listCameraDevices().then(setDevices).catch(console.error).finally(() => setLoadingDevices(false)); };
  const refreshNdi = () => { setLoadingNdi(true); setNdiError(null); listNdiSources().then(setNdiSources).catch((e) => setNdiError(String(e))).finally(() => setLoadingNdi(false)); };
  useEffect(refreshDevices, []);
  const setKind = (next: "device" | "url" | "ndi" | "srt") => {
    if (next === "device") onSave({ source: { kind: "device", id: "", name: "" } });
    if (next === "url") onSave({ source: { kind: "url", url: "" } });
    if (next === "ndi") { onSave({ source: { kind: "network", input: { protocol: "ndi", source_name: "" } } }); refreshNdi(); }
    if (next === "srt") onSave({ source: { kind: "network", input: { protocol: "srt", settings: freshSrt() } } });
  };
  const saveSrt = (patch: Partial<SrtSettings>) => onSave({ source: { kind: "network", input: { protocol: "srt", settings: { ...srt, ...patch } } } });
  const number = (value: string): number | null => value.trim() === "" ? null : Number(value);
  const radio = (value: "device" | "url" | "ndi" | "srt", label: string) => <label key={value} style={{ display: "flex", alignItems: "center", gap: 6, cursor: "pointer" }}><input type="radio" checked={kind === value} onChange={() => setKind(value)} style={{ cursor: "pointer" }} /><span style={{ fontSize: 13 }}>{label}</span></label>;
  const deviceId = source.kind === "device" ? source.id : "";
  const deviceMissing = deviceId !== "" && !devices.some((d) => d.id === deviceId);
  const ndiMissing = ndiName !== "" && !ndiSources.some((item) => item.name === ndiName);

  return <>
    <Field label={t("networkInputUi.sourceType")}><div style={{ display: "flex", gap: 14, flexWrap: "wrap" }}>{radio("device", t("networkInputUi.device"))}{radio("url", t("networkInputUi.url"))}{radio("ndi", "NDI")}{radio("srt", "SRT")}</div></Field>
    {kind === "device" && <><Field label={t("inspector.cameraDevice")}><div style={{ display: "flex", gap: 6, alignItems: "center" }}><Select style={{ ...inputStyle, cursor: "pointer" }} value={deviceId} onChange={(e) => { const d = devices.find((item) => item.id === e.target.value); onSave({ source: { kind: "device", id: e.target.value, name: d?.name ?? e.target.value } }); }}><option value="">— {t("common.select").toLowerCase()} —</option>{deviceMissing && <option value={deviceId}>{source.kind === "device" ? source.name : deviceId} ({t("networkInputUi.missing")})</option>}{devices.map((d) => <option key={d.id} value={d.id}>{d.name}</option>)}</Select><button title={t("common.refresh")} onClick={refreshDevices} disabled={loadingDevices} style={refreshButtonStyle}>↺</button></div></Field>{!loadingDevices && devices.length === 0 && <Hint>{t("networkInputUi.noCamera")}</Hint>}</>}
    {kind === "url" && <><Field label={t("networkInputUi.streamUrl")}><input style={inputStyle} type="text" key={`cam-url-${source.kind === "url" ? source.url : ""}`} defaultValue={source.kind === "url" ? source.url : ""} placeholder="rtsp://192.168.1.50:8554/live" onBlur={(e) => onSave({ source: { kind: "url", url: e.target.value.trim() } })} /></Field><Hint>{t("networkInputUi.urlHint")}</Hint></>}
    {kind === "ndi" && <>
      <Field label={t("networkInputUi.ndiSource")}>
        <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
          <Select style={{ ...inputStyle, flex: 1, minWidth: 100, cursor: "pointer" }} value={ndiName} onChange={(e) => onSave({ source: { kind: "network", input: { protocol: "ndi", source_name: e.target.value } } })}>
            <option value="">— {t("common.select").toLowerCase()} —</option>
            {ndiMissing && <option value={ndiName}>{ndiName} ({t("networkInputUi.missing")})</option>}
            {ndiSources.map((item) => <option key={item.name} value={item.name}>{item.name}</option>)}
          </Select>
          <button title={t("common.refresh")} onClick={refreshNdi} disabled={loadingNdi} style={refreshButtonStyle}>↺</button>
        </div>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginTop: 5 }}>
          <button type="button" onClick={() => void openExternalUrl("https://ndi.link/NDIRedistV6").catch(console.error)} style={refreshButtonStyle}>{t("networkInputUi.ndiRuntimeLink")}</button>
          <button type="button" onClick={() => void openExternalUrl("https://ndi.video/").catch(console.error)} style={refreshButtonStyle}>{t("networkInputUi.ndiOfficialLink")}</button>
        </div>
      </Field>
      <Field label={t("networkInputUi.ndiReceiverQuality")}>
        <Select style={inputStyle} value={ndiQuality} onChange={(e) => onSave({ ndi_quality: e.target.value as NdiQuality })}>
          <option value="highest">{t("networkOutputUi.qualityHighest")}</option>
          <option value="low_bandwidth">{t("networkOutputUi.qualityLow")}</option>
        </Select>
      </Field>
      {ndiError && <div style={{ fontSize: 11, color: "var(--wc-danger, #e66)", marginBottom: 10 }}>{ndiError}</div>}
      {!loadingNdi && !ndiError && ndiSources.length === 0 && <Hint>{t("networkInputUi.noNdi")}</Hint>}
      <Hint>{t("networkInputUi.ndiHint")}</Hint>
    </>}
    {kind === "srt" && <><Field label={t("networkInputUi.mode")}><Select style={inputStyle} value={srt.mode} onChange={(e) => saveSrt({ mode: e.target.value as SrtSettings["mode"] })}><option value="listener">{t("networkOutputUi.listener")}</option><option value="caller">{t("networkOutputUi.caller")}</option><option value="rendezvous">{t("networkOutputUi.rendezvous")}</option></Select></Field><Field label={srt.mode === "listener" ? t("networkOutputUi.bindAddress") : t("networkOutputUi.remoteAddress")}><input style={inputStyle} defaultValue={srt.host} key={`srt-host-${srt.host}`} placeholder={srt.mode === "listener" ? "0.0.0.0" : "192.168.1.50"} onBlur={(e) => saveSrt({ host: e.target.value.trim() })} /></Field><Field label={t("preferences.port")}><input style={inputStyle} type="number" min={1} max={65535} value={srt.port} onChange={(e) => saveSrt({ port: Number(e.target.value) || 0 })} /></Field><Field label={`${t("networkOutputUi.latency")} (ms)`}><input style={inputStyle} type="number" min={20} max={10000} value={srt.latency_ms} onChange={(e) => saveSrt({ latency_ms: Number(e.target.value) || 0 })} /></Field><details style={{ marginBottom: 10 }}><summary style={{ cursor: "pointer", fontSize: 13 }}>{t("networkOutputUi.advanced")}</summary><div style={{ marginTop: 8 }}><Field label={t("networkOutputUi.streamId")}><input style={inputStyle} type="text" defaultValue={srt.stream_id ?? ""} key={`srt-id-${srt.stream_id ?? ""}`} onBlur={(e) => saveSrt({ stream_id: e.target.value.trim() || null })} /></Field><Field label={t("networkOutputUi.passphrase")}><input style={inputStyle} type="password" autoComplete="new-password" defaultValue={srt.passphrase ?? ""} key={`srt-pass-${srt.passphrase ?? ""}`} onBlur={(e) => saveSrt({ passphrase: e.target.value || null })} /></Field><Field label={t("networkOutputUi.payload")}><input style={inputStyle} type="number" min={1} max={1456} defaultValue={srt.payload_size ?? ""} key={`srt-payload-${srt.payload_size ?? ""}`} onBlur={(e) => saveSrt({ payload_size: number(e.target.value) })} /></Field><label style={{ display: "flex", gap: 7, alignItems: "center", fontSize: 13 }}><input type="checkbox" checked={srt.too_late_packet_drop} onChange={(e) => saveSrt({ too_late_packet_drop: e.target.checked })} />{t("networkOutputUi.lateDrop")}</label></div></details><Hint>{t("networkInputUi.srtHint")}</Hint></>}
    <Hint>{locale === "ru" ? "Поток работает до остановки. Размещение и фейды задаются на соседних вкладках." : "The feed runs until stopped. Use the nearby Geometry and Fade tabs to place and fade it."}</Hint>
  </>;
}
