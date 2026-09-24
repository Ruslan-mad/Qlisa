// Preferences — draggable floating modal overlaid on the workspace.
// Opened via File → Preferences or Ctrl+,

import { useEffect, useRef, useState, useCallback } from "react";
import { createPortal } from "react-dom";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { emit, listen } from "@tauri-apps/api/event";
import { machineAudioConfigsEqual } from "../../lib/types";
import type { AppPreferences, AudioPreferences, AudioRuntimeStatus, CueColor, CueColorStyle, CueType, DeviceInfo, DisplayPreferences, GeneralPreferences, MachineAudioConfig, NetworkIoStatus, NetworkOutputRuntimeStatus, OscReceiveConfig, ScreenInfo, TimerPosition, OutputDestination } from "../../lib/types";
import { cloneNetworkOutputSettings, cloneOutputTransform, createOutputId, DEFAULT_DISPLAY_PREFS, DEFAULT_MACHINE_AUDIO_CONFIG, findDuplicateDisplayMonitor, normalizeOutputDestinations, validateOutputDestinations } from "../../lib/types";
import { CurveSelect } from "../common/CurveSelect";
import { Select } from "../common/Select";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import {
  getAsioOutputPairs,
  getAudioRuntimeStatus,
  getAvailableBackends,
  getMachineAudioConfig,
  getOscConfig,
  getPreferences,
  getNetworkIoStatus,
  getNetworkOutputStatuses,
  identifyOutputScreen,
  listAudioDevices,
  listPreviewAudioDevices,
  listSystemFonts,
  listVideoScreens,
  openExternalUrl,
  previewOutputTimer,
  setOscConfig,
  setOutputScreen,
  testAudioDevice,
  testPreviewAudio,
  updateAudioPreferences,
  updateDisplayPreferences,
  updateOutputDestinations,
  updateGeneralPreferences,
  updateMachineAudioConfig,
} from "../../lib/commands";
import { OscPatchesPanel } from "../OscPatches/OscPatchesPanel";
import { InputPatchesPanel } from "../InputPatches/InputPatchesPanel";
import { OutputPatchesPanel } from "../OutputPatches/OutputPatchesPanel";
import { TcPreferences } from "../Timecode/TcPreferences";
import { MidiTriggerPreferences } from "./MidiTriggerPreferences";
import { NetworkInterfaceSection } from "./NetworkInterfaceSection";
import { ProjectorToolsSection } from "./ProjectorToolsSection";
import { listInputDevices } from "../../lib/commands";
import { DragNumber } from "../common/DragNumber";
import { useLocale } from "../../i18n";
import { ColorPicker } from "../Inspector/ColorPicker";
import { mergeOutputMonitorAssignments } from "./preferencesModel";
import { MediaRuntimeSection } from "./MediaRuntimeSection";

// ---------------------------------------------------------------------------
// Sidebar categories
// ---------------------------------------------------------------------------

type Category = "audio" | "general" | "network" | "networkOutput" | "display" | "personalization" | "mediaRuntime";

const CATEGORIES: { id: Category; icon: string }[] = [
  { id: "audio",           icon: "🔊" },
  { id: "general",         icon: "⚙️" },
  { id: "network",         icon: "🌐" },
  { id: "networkOutput",   icon: "📡" },
  { id: "display",         icon: "🖥" },
  { id: "personalization", icon: "🎨" },
  { id: "mediaRuntime", icon: "🎞" },
];

// ---------------------------------------------------------------------------
// Small reusable atoms
// ---------------------------------------------------------------------------

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div style={{ marginBottom: 24 }}>
      <div style={{
        fontSize: 11, fontWeight: 600, color: "var(--wc-text-muted)",
        textTransform: "uppercase", letterSpacing: "0.07em",
        marginBottom: 10, paddingBottom: 5,
        borderBottom: "1px solid var(--wc-border)",
      }}>
        {title}
      </div>
      {children}
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 8, minHeight: 28 }}>
      <label style={{ width: 170, fontSize: 12, color: "var(--wc-text-secondary)", flexShrink: 0, textAlign: "right" }}>
        {label}
      </label>
      <div style={{ flex: 1, display: "flex", alignItems: "center", gap: 8 }}>
        {children}
      </div>
    </div>
  );
}

const inputStyle: React.CSSProperties = {
  background: "var(--wc-bg-app)", border: "1px solid var(--wc-border-strong)", borderRadius: 4,
  color: "var(--wc-text)", fontSize: 12, padding: "3px 7px", width: "100%",
};
const selectStyle: React.CSSProperties = { ...inputStyle, cursor: "pointer" };
const btnStyle: React.CSSProperties = {
  padding: "3px 10px", background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
  borderRadius: 4, color: "var(--wc-text)", fontSize: 11, cursor: "pointer",
};

const BACKEND_LABELS: Record<string, string> = {
  wasapi_shared:    "WASAPI Shared",
  wasapi_exclusive: "WASAPI Exclusive",
  asio:             "ASIO",
  system_default:   "System Default (CoreAudio / ALSA)",
};

function localizedOutputValidationError(
  destinations: readonly OutputDestination[],
  translate: (key: string, vars?: Record<string, string | number | boolean | null | undefined>) => string,
): string {
  if (destinations.length === 0) return translate("preferencesUi.emptyOutputs");
  if (!destinations.some((destination) => destination.enabled)) return translate("preferencesUi.noEnabledOutputs");
  if (!destinations.some((destination) => destination.enabled && destination.sink_kind === "display")) return translate("networkOutputUi.displayRequired");
  const ids = new Set<string>();
  const names = new Set<string>();
  for (let index = 0; index < destinations.length; index += 1) {
    const destination = destinations[index];
    const id = destination.id.trim();
    const name = destination.name.trim();
    if (!id || !name) return translate("preferencesUi.outputRequired", { index: index + 1 });
    if (id !== destination.id || name !== destination.name) {
      return translate("preferencesUi.outputWhitespace", { name: name || id });
    }
    if (ids.has(id)) return translate("preferencesUi.duplicateOutputId", { id });
    if (names.has(name.toLocaleLowerCase())) return translate("preferencesUi.duplicateOutputName", { name });
    ids.add(id);
    names.add(name.toLocaleLowerCase());
  }
  const duplicateMonitor = findDuplicateDisplayMonitor(destinations);
  if (duplicateMonitor) {
    return `${translate("fullscreenUi.monitor", { count: duplicateMonitor.monitor + 1 })}: ${duplicateMonitor.first.name}; ${duplicateMonitor.second.name}`;
  }
  return "";
}

// Reject with `message` if `promise` has not settled within `ms`.  The backend
// already bounds device enumeration, but this is a belt-and-suspenders guard so
// a wedged IPC round-trip can never leave the panel spinning on "Loading…".
function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  return Promise.race([
    promise,
    new Promise<T>((_, reject) => setTimeout(() => reject(new Error(message)), ms)),
  ]);
}

// ---------------------------------------------------------------------------
// Audio content
// ---------------------------------------------------------------------------

function AudioContent({
  machineConfig,
  audioPrefs,
  onMachineConfigChange,
  onAudioPrefsChange,
  availableBackends,
  onImmediateApplyMachine,
}: {
  machineConfig: MachineAudioConfig;
  audioPrefs: AudioPreferences;
  onMachineConfigChange: (c: MachineAudioConfig) => void;
  onAudioPrefsChange: (p: AudioPreferences) => void;
  availableBackends: string[];
  onImmediateApplyMachine?: (c: MachineAudioConfig) => Promise<void>;
}) {
  const { t, locale } = useLocale();
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [previewDevices, setPreviewDevices] = useState<DeviceInfo[]>([]);
  const [inputDevices, setInputDevices] = useState<DeviceInfo[]>([]);
  const [devicesError, setDevicesError] = useState<string | null>(null);
  const [previewDevicesError, setPreviewDevicesError] = useState<string | null>(null);
  const [devicesLoading, setDevicesLoading] = useState(false);
  const [previewDevicesLoading, setPreviewDevicesLoading] = useState(false);
  const [asioPairs, setAsioPairs] = useState<number>(1);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [runtimeStatus, setRuntimeStatus] = useState<AudioRuntimeStatus | null>(null);
  const [mainTestState, setMainTestState] = useState<"idle" | "working" | "error">("idle");
  const [previewTestState, setPreviewTestState] = useState<"idle" | "working" | "error">("idle");
  const [applyingAsioDevice, setApplyingAsioDevice] = useState(false);
  const previewVerifiedRef = useRef(false);
  const previewRouteDirtyRef = useRef(false);
  const [previewRouteDirty, setPreviewRouteDirty] = useState(false);
  const asioAvailable = availableBackends.includes("asio");
  const isAsio = machineConfig.backend === "asio";
  const isShared = machineConfig.backend === "wasapi_shared";

  const loadDevices = useCallback(async (backend: MachineAudioConfig["backend"]) => {
    setDevicesLoading(true);
    setDevicesError(null);
    try {
      const list = await withTimeout(
        listAudioDevices(backend),
        8000,
        t("preferencesExtra.timeout"),
      );
      setDevices(list);
    } catch (e) {
      setDevicesError(e instanceof Error ? e.message : String(e));
      setDevices([]);
    } finally {
      setDevicesLoading(false);
    }
  }, [t]);

  const loadPreviewDevices = useCallback(async () => {
    setPreviewDevicesLoading(true);
    setPreviewDevicesError(null);
    try {
      const list = await withTimeout(
        listPreviewAudioDevices(),
        8000,
        t("preferencesExtra.timeout"),
      );
      setPreviewDevices(list);
    } catch (e) {
      setPreviewDevicesError(e instanceof Error ? e.message : String(e));
      setPreviewDevices([]);
    } finally {
      setPreviewDevicesLoading(false);
    }
  }, [t]);

  useEffect(() => {
    void loadDevices(machineConfig.backend);
    if (machineConfig.backend === "asio") {
      getAsioOutputPairs().then(setAsioPairs).catch(() => setAsioPairs(1));
    }
  }, [machineConfig.backend, loadDevices]);

  useEffect(() => {
    if (!isAsio) void loadPreviewDevices();
  }, [isAsio, loadPreviewDevices]);

  useEffect(() => {
    listInputDevices().then(setInputDevices).catch(console.error);
  }, []);

  useEffect(() => {
    let cancelled = false;
    const refresh = () => getAudioRuntimeStatus().then((status) => {
      if (!cancelled) {
        setRuntimeStatus(previewVerifiedRef.current
          ? { ...status, preview_state: "working" }
          : previewRouteDirtyRef.current
            ? { ...status, preview_state: "not_tested" }
            : status);
      }
    }).catch(() => undefined);
    refresh();
    const timer = window.setInterval(refresh, 2000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen("preferences-updated", () => {
      previewVerifiedRef.current = false;
      previewRouteDirtyRef.current = false;
      if (!cancelled) {
        setPreviewRouteDirty(false);
        getAudioRuntimeStatus().then(setRuntimeStatus).catch(() => undefined);
      }
    }).then((remove) => {
      if (cancelled) remove();
      else unlisten = remove;
    }).catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const currentDevice = devices.find((d) => d.id === machineConfig.device_id) ?? devices[0] ?? null;
  const latencyMs = !isShared && currentDevice && machineConfig.buffer_size
    ? ((machineConfig.buffer_size / currentDevice.sample_rate) * 1000).toFixed(1)
    : "—";

  const runtimeLabel = (state: AudioRuntimeStatus["main_state"]): string => {
    return state === "working" ? t("audioBusUi.runtimeWorking") : state === "fallback" ? t("audioBusUi.runtimeFallback") : state === "error" ? t("audioBusUi.runtimeError") : state === "not_tested" ? t("audioBusUi.runtimeNotTested") : t("audioBusUi.runtimeNotConfigured");
  };

  const previewConfigured = isAsio
    ? machineConfig.preview_asio_pair !== null
    : machineConfig.preview_device_id !== null;
  const displayedPreviewState: AudioRuntimeStatus["preview_state"] = !previewConfigured
    ? "not_configured"
    : previewRouteDirty
      ? "not_tested"
    : previewTestState === "working"
      ? "working"
      : (runtimeStatus?.preview_state ?? "not_tested");

  const updateMachineConfig = (next: MachineAudioConfig) => {
    const backend = next.backend;
    const normalized = backend === "asio"
      ? { ...next, preview_device_id: null, preview_device_name: null }
      : { ...next, preview_asio_pair: null };
    const routeChanged = machineConfig.backend !== normalized.backend
      || machineConfig.device_id !== normalized.device_id
      || machineConfig.preview_device_id !== normalized.preview_device_id
      || machineConfig.preview_asio_pair !== normalized.preview_asio_pair;
    if (routeChanged) {
      previewVerifiedRef.current = false;
      previewRouteDirtyRef.current = true;
      setPreviewRouteDirty(true);
      setPreviewTestState("idle");
      setRuntimeStatus((current) => current ? { ...current, preview_state: normalized.preview_asio_pair !== null || normalized.preview_device_id !== null ? "not_tested" : "not_configured" } : current);
    }
    onMachineConfigChange(normalized);
  };

  const runMainTest = async () => {
    setMainTestState("working");
    try {
      await testAudioDevice(machineConfig.device_id ?? "", machineConfig.backend);
      setMainTestState("idle");
    } catch {
      setMainTestState("error");
    }
  };

  const runPreviewTest = async () => {
    setPreviewTestState("working");
    try {
      await testPreviewAudio(
        isAsio ? null : machineConfig.preview_device_id,
        machineConfig.backend,
        isAsio ? machineConfig.preview_asio_pair : null,
      );
      previewVerifiedRef.current = true;
      previewRouteDirtyRef.current = false;
      setPreviewRouteDirty(false);
      setRuntimeStatus((current) => ({
        main_state: current?.main_state ?? "not_tested",
        main_device_name: current?.main_device_name ?? null,
        preview_state: "working",
        preview_device_name: isAsio ? (currentDevice?.name ?? machineConfig.device_name ?? null) : machineConfig.preview_device_name,
      }));
      setPreviewTestState("idle");
    } catch {
      setPreviewTestState("error");
      getAudioRuntimeStatus().then(setRuntimeStatus).catch(() => undefined);
    }
  };

  return (
    <>
      <Section title={t("preferencesUi.audioEngine")}>
        <Row label={t("audioBusUi.mainOutput")}>
          {devicesLoading ? (
            <span style={{ fontSize: 12, color: "var(--wc-text-muted)" }}>{t("preferencesUi.loading")}</span>
          ) : devicesError ? (
            <>
              <span style={{ fontSize: 12, color: "#ef4444", flex: 1 }}>{devicesError}</span>
              <button
                style={btnStyle}
                onClick={() => void loadDevices(machineConfig.backend)}
                title={t("common.retry")}
              >
                ↺ {t("common.retry")}
              </button>
            </>
          ) : (
            <>
              <Select
                style={selectStyle}
                value={machineConfig.device_id ?? ""}
                onChange={(e) => {
                  const id = e.target.value || null;
                  const name = id ? (devices.find((d) => d.id === id)?.name ?? null) : null;
                  const next = { ...machineConfig, device_id: id, device_name: name };
                  updateMachineConfig(next);
                  if (isAsio && onImmediateApplyMachine) {
                    setApplyingAsioDevice(true);
                    void onImmediateApplyMachine(next).finally(() => setApplyingAsioDevice(false));
                  }
                }}
              >
                <option value="">— {t("preferencesUi.systemDefault")} —</option>
                {devices.map((d) => (
                  <option key={d.id} value={d.id}>{d.name}</option>
                ))}
              </Select>
              <button
                style={btnStyle}
                onClick={() => void loadDevices(machineConfig.backend)}
                title={t("common.refresh")}
              >
                ↺
              </button>
              {!isAsio && <span style={{ fontSize: 12, color: "var(--wc-text-secondary)", whiteSpace: "nowrap" }}>1–2</span>}
              {isAsio && (
                <Select
                  style={{ ...selectStyle, maxWidth: 130 }}
                  value={machineConfig.asio_out_pair}
                  onChange={async (e) => {
                    const next = { ...machineConfig, asio_out_pair: Number(e.target.value) };
                    updateMachineConfig(next);
                    if (onImmediateApplyMachine) await onImmediateApplyMachine(next);
                  }}
                  aria-label={t("preferencesUi.outputPair")}
                >
                  {Array.from({ length: Math.max(asioPairs, 1) }, (_, i) => <option key={i} value={i}>{t("preferencesUi.outputPairLabel", { first: i * 2 + 1, last: i * 2 + 2 })}</option>)}
                </Select>
              )}
              <span style={{ fontSize: 11, color: mainTestState === "error" ? "#ef4444" : "var(--wc-text-muted)", whiteSpace: "nowrap" }}>
                {mainTestState === "working" ? t("audioBusUi.testing") : mainTestState === "error" ? t("audioBusUi.testFailed") : runtimeLabel(runtimeStatus?.main_state ?? "not_tested")}
              </span>
              <button style={btnStyle} disabled={applyingAsioDevice} onClick={() => void runMainTest()} title={t("preferencesExtra.testTone")}>{t("preferencesExtra.test")}</button>
            </>
          )}
        </Row>

        <Row label={t("audioBusUi.previewOutput")}>
          {!isAsio && previewDevicesLoading ? (
            <span style={{ fontSize: 12, color: "var(--wc-text-muted)" }}>{t("preferencesUi.loading")}</span>
          ) : (!isAsio && previewDevicesError) ? (
            <>
              <span style={{ fontSize: 12, color: "#ef4444", flex: 1 }}>{previewDevicesError}</span>
              <button
                style={btnStyle}
                onClick={() => void loadPreviewDevices()}
                title={t("common.retry")}
              >
                ↺ {t("common.retry")}
              </button>
            </>
          ) : (
            <>
              {isAsio ? (
                <>
                  <span style={{ fontSize: 12, color: "var(--wc-text-secondary)", minWidth: 130 }}>{currentDevice?.name ?? machineConfig.device_name ?? "—"}</span>
                  <Select
                    style={{ ...selectStyle, maxWidth: 130 }}
                    value={machineConfig.preview_asio_pair ?? ""}
                    onChange={(e) => updateMachineConfig({ ...machineConfig, preview_asio_pair: e.target.value === "" ? null : Number(e.target.value), preview_device_id: null, preview_device_name: null })}
                    aria-label={t("preferencesUi.outputPair")}
                  >
                    <option value="">— {t("common.disabledShort")} —</option>
                    {Array.from({ length: Math.max(asioPairs, 1) }, (_, i) => <option key={i} value={i} disabled={i === machineConfig.asio_out_pair}>{t("preferencesUi.outputPairLabel", { first: i * 2 + 1, last: i * 2 + 2 })}</option>)}
                  </Select>
                </>
              ) : (
                <>
                  <Select
                    style={selectStyle}
                    value={machineConfig.preview_device_id ?? ""}
                    onChange={(e) => {
                      const id = e.target.value || null;
                      const name = id ? (previewDevices.find((d) => d.id === id)?.name ?? null) : null;
                      updateMachineConfig({ ...machineConfig, preview_device_id: id, preview_device_name: name, preview_asio_pair: null });
                    }}
                  >
                    <option value="">— {t("common.disabledShort")} —</option>
                    {previewDevices.map((d) => <option key={d.id} value={d.id}>{d.name}</option>)}
                  </Select>
                  <span style={{ fontSize: 12, color: "var(--wc-text-secondary)", whiteSpace: "nowrap" }}>1–2</span>
                </>
              )}
              <span style={{ fontSize: 11, color: previewTestState === "error" ? "#ef4444" : "var(--wc-text-muted)", whiteSpace: "nowrap" }}>
                {previewTestState === "working" ? t("audioBusUi.testing") : previewTestState === "error" ? t("audioBusUi.testFailed") : runtimeLabel(displayedPreviewState)}
              </span>
              <button
                style={btnStyle}
                disabled={!previewConfigured || previewTestState === "working"}
                onClick={() => void runPreviewTest()}
                title={t("preferencesExtra.testTone")}
              >
                {t("preferencesExtra.test")}
              </button>
            </>
          )}
        </Row>

        <OutputPatchesPanel backend={machineConfig.backend} />

        <label style={{ display: "flex", alignItems: "center", gap: 8, margin: "12px 0", fontSize: 12, color: "var(--wc-text-secondary)", cursor: "pointer" }}>
          <input type="checkbox" checked={showAdvanced} onChange={(e) => setShowAdvanced(e.target.checked)} />
          {t("audioBusUi.advancedSettings")}
        </label>

        {showAdvanced && <>
        <Row label={t("preferencesUi.backend")}>
          <Select style={selectStyle} value={machineConfig.backend} onChange={(e) => updateMachineConfig({ ...machineConfig, backend: e.target.value as MachineAudioConfig["backend"], device_id: null, device_name: null })}>
            {availableBackends.map((id) => <option key={id} value={id} disabled={id === "asio" && !asioAvailable}>{id === "system_default" ? t("preferencesUi.systemDefault") : id === "wasapi_shared" ? t("audioBusUi.wasapiShared") : id === "wasapi_exclusive" ? t("audioBusUi.wasapiExclusive") : BACKEND_LABELS[id] ?? id}{id === "asio" && !asioAvailable ? ` (${t("preferencesUi.asioUnavailable")})` : ""}</option>)}
          </Select>
        </Row>
        <Row label={t("preferences.bufferSize")}>
          <Select
            style={{ ...selectStyle, opacity: isShared ? 0.4 : 1 }}
            value={machineConfig.buffer_size}
            disabled={isShared}
            onChange={(e) => onMachineConfigChange({ ...machineConfig, buffer_size: Number(e.target.value) })}
          >
              {[64, 128, 256, 512, 1024, 2048].map((s) => (
              <option key={s} value={s}>{t("preferencesUi.sampleCount", { count: s })}</option>
            ))}
          </Select>
          {isShared && (
            <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>{t("preferencesUi.managedByWindows")}</span>
          )}
          {isAsio && (
            <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>{t("preferencesUi.asioControlPanel")}</span>
          )}
        </Row>

        <Row label={t("preferences.sampleRate")}>
          <span style={{ fontSize: 12, color: "var(--wc-text-secondary)" }}>
            {currentDevice?.sample_rate ?? "—"} Hz
            <span style={{ fontSize: 11, color: "var(--wc-text-faint)", marginLeft: 8 }}>({t("preferencesUi.setByDevice")})</span>
          </span>
        </Row>

        {!isShared && (
          <Row label={t("preferences.estimatedLatency")}>
            <span style={{ fontSize: 12, color: "#22c55e", fontFamily: "monospace" }}>
              {latencyMs} ms
            </span>
          </Row>
        )}
        <Row label={t("audioBusUi.refreshDevices")}>
          <button style={btnStyle} onClick={() => void loadDevices(machineConfig.backend)}>↺</button>
          {!isAsio && <button style={btnStyle} onClick={() => void loadPreviewDevices()}>↺ {locale === "ru" ? "предпрослушка" : "preview"}</button>}
        </Row>
        </>}
      </Section>

      {showAdvanced && <Section title={t("preferences.input")}>
          <Row label={t("preferencesUi.inputDevice")}>
            <Select
              style={selectStyle}
              value={machineConfig.input_device_id ?? ""}
              onChange={(e) =>
                onMachineConfigChange({ ...machineConfig, input_device_id: e.target.value || null })
              }
            >
              <option value="">— {t("preferencesUi.systemDefault")} —</option>
              {inputDevices.map((d) => (
                <option key={d.id} value={d.id}>{d.name}</option>
              ))}
            </Select>
          </Row>
          <div style={{ marginTop: 12 }}>
            <InputPatchesPanel />
          </div>
        </Section>}

      <Section title={t("preferences.defaults")}>
        <Row label={t("preferencesUi.defaultVolume")}>
          <input
            type="range" min={-60} max={0} step={0.5}
            value={audioPrefs.default_volume_db} style={{ flex: 1 }}
            onChange={(e) => onAudioPrefsChange({ ...audioPrefs, default_volume_db: Number(e.target.value) })}
          />
          <span style={{ width: 52, textAlign: "right", fontFamily: "monospace", fontSize: 12, color: "var(--wc-text-secondary)" }}>
            {audioPrefs.default_volume_db.toFixed(1)} dB
          </span>
        </Row>
        <Row label={t("preferencesUi.fadeOutStop")}>
          <DragNumber min={0} max={5000} step={50}
            style={{ ...inputStyle, width: 90 }}
            value={audioPrefs.default_fade_out_ms}
            onChange={(e) => onAudioPrefsChange({ ...audioPrefs, default_fade_out_ms: Number(e.target.value) })}
          />
        </Row>
        <Row label={t("preferencesUi.defaultFadeCurve")}>
          <CurveSelect
            value={audioPrefs.default_fade_curve}
            onChange={(v) => onAudioPrefsChange({ ...audioPrefs, default_fade_curve: v })}
            baseStyle={selectStyle}
          />
        </Row>
      </Section>
    </>
  );
}

// ---------------------------------------------------------------------------
// General content
// ---------------------------------------------------------------------------

function GeneralContent({ prefs, onChange }: {
  prefs: GeneralPreferences;
  onChange: (p: GeneralPreferences) => void;
}) {
  const { t } = useLocale();
  const cueTypes: { type: CueType; label: string }[] = [
    { type: "audio", label: "cueTypes.audio" },
    { type: "video", label: "cueTypes.video" },
    { type: "image", label: "cueTypes.image" },
    { type: "stop", label: "cueTypes.stop" },
    { type: "fade", label: "cueTypes.fade" },
    { type: "wait", label: "cueTypes.wait" },
    { type: "group", label: "cueTypes.group" },
    { type: "number", label: "cueTypes.number" },
    { type: "midi", label: "cueTypes.midi" },
    { type: "midi_file", label: "cueTypes.midiFile" },
    { type: "osc", label: "cueTypes.osc" },
    { type: "light", label: "cueTypes.light" },
    { type: "mic", label: "cueTypes.mic" },
    { type: "timecode", label: "cueTypes.timecode" },
    { type: "text", label: "cueTypes.text" },
    { type: "camera", label: "cueTypes.camera" },
    { type: "memo", label: "cueTypes.memo" },
    { type: "devamp", label: "cueTypes.devamp" },
    { type: "script", label: "cueTypes.script" },
    { type: "start", label: "cueTypes.start" },
    { type: "pause", label: "cueTypes.pause" },
    { type: "resume", label: "cueTypes.resume" },
    { type: "load", label: "cueTypes.load" },
    { type: "reset", label: "cueTypes.reset" },
    { type: "goto", label: "cueTypes.goto" },
    { type: "arm", label: "cueTypes.arm" },
    { type: "disarm", label: "cueTypes.disarm" },
  ];

  const setDefaultCueColor = (cueType: CueType, color: CueColor) => {
    const defaultCueColors = { ...(prefs.default_cue_colors ?? {}) };
    if (color === "none") delete defaultCueColors[cueType];
    else defaultCueColors[cueType] = color;
    onChange({ ...prefs, default_cue_colors: defaultCueColors });
  };

  return (
    <>
      <Section title={t("preferences.transport")}>
        <Row label={t("preferencesUi.doubleGoProtection")}>
          <DragNumber min={0} max={5000} step={50}
            style={{ ...inputStyle, width: 90 }}
            value={prefs.double_go_protection_ms}
            onChange={(e) => onChange({ ...prefs, double_go_protection_ms: Number(e.target.value) })}
          />
          <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>{t("preferencesUi.millisecondsDisabled")}</span>
        </Row>
      </Section>
      <Section title={t("preferences.cueList")}>
        <Row label={t("preferencesUi.confirmBeforeDelete")}>
          <input
            type="checkbox"
            checked={prefs.confirm_before_delete}
            onChange={(e) => onChange({ ...prefs, confirm_before_delete: e.target.checked })}
            style={{ accentColor: "var(--wc-accent)", width: 14, height: 14 }}
          />
        </Row>
        <Row label={t("preferencesUi.autoScrollPlayhead")}>
          <input
            type="checkbox"
            checked={prefs.auto_scroll_to_playhead}
            onChange={(e) => onChange({ ...prefs, auto_scroll_to_playhead: e.target.checked })}
            style={{ accentColor: "var(--wc-accent)", width: 14, height: 14 }}
          />
        </Row>
        <Row label={t("preferencesUi.autoRenumber")}>
          <input
            type="checkbox"
            checked={prefs.auto_renumber_on_reorder}
            onChange={(e) => onChange({ ...prefs, auto_renumber_on_reorder: e.target.checked })}
            style={{ accentColor: "var(--wc-accent)", width: 14, height: 14 }}
          />
          <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>
              {t("preferencesExtra.autoRenumberHint")}
          </span>
        </Row>
        <Row label={t("preferencesUi.rowHeight")}>
          <Select
            style={selectStyle}
            value={prefs.cue_row_height}
            onChange={(e) => onChange({ ...prefs, cue_row_height: e.target.value as GeneralPreferences["cue_row_height"] })}
          >
            <option value="compact">{t("preferencesUi.compact")}</option>
            <option value="normal">{t("preferencesUi.normal")}</option>
            <option value="tall">{t("preferencesUi.tall")}</option>
          </Select>
        </Row>
      </Section>
      <Section title={t("cueColorDefaultsUi.title")}>
        <p style={{ margin: "-2px 0 12px", fontSize: 11, color: "var(--wc-text-faint)", lineHeight: 1.5 }}>
          {t("cueColorDefaultsUi.hint")}
        </p>
        {cueTypes.map(({ type, label }) => (
          <Row key={type} label={t(label)}>
            <ColorPicker
              value={prefs.default_cue_colors?.[type] ?? "none"}
              onChange={(color) => setDefaultCueColor(type, color)}
            />
          </Row>
        ))}
      </Section>
    </>
  );
}

// ---------------------------------------------------------------------------
// Display content
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Timer position picker — 3×3 grid with only the 5 valid positions active
// ---------------------------------------------------------------------------

const TIMER_POSITIONS: { pos: TimerPosition; gridArea: string; label: string }[] = [
  { pos: "top_left",     gridArea: "1 / 1", label: "↖" },
  { pos: "top_right",    gridArea: "1 / 3", label: "↗" },
  { pos: "center",       gridArea: "2 / 2", label: "⊙" },
  { pos: "bottom_left",  gridArea: "3 / 1", label: "↙" },
  { pos: "bottom_right", gridArea: "3 / 3", label: "↘" },
];

const TIMER_POSITION_LABEL_KEYS: Record<TimerPosition, string> = {
  top_left: "preferencesExtra.positionTopLeft",
  top_right: "preferencesExtra.positionTopRight",
  center: "preferencesExtra.positionCenter",
  bottom_left: "preferencesExtra.positionBottomLeft",
  bottom_right: "preferencesExtra.positionBottomRight",
};

function TimerPositionPicker({ value, onChange }: { value: TimerPosition; onChange: (v: TimerPosition) => void }) {
  const { t } = useLocale();
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(3, 32px)",
        gridTemplateRows: "repeat(3, 32px)",
        gap: 3,
        background: "var(--wc-bg-app)",
        border: "1px solid var(--wc-border-strong)",
        borderRadius: 5,
        padding: 4,
      }}
    >
      {TIMER_POSITIONS.map(({ pos, gridArea, label }) => (
        <button
          key={pos}
          title={`${t("preferences.position")}: ${t(TIMER_POSITION_LABEL_KEYS[pos])}`}
          onClick={() => onChange(pos)}
          style={{
            gridArea,
            width: 32, height: 32,
            border: value === pos ? "2px solid var(--wc-accent)" : "1px solid var(--wc-border-strong)",
            borderRadius: 4,
            background: value === pos ? "var(--wc-accent)" : "var(--wc-bg-surface)",
            color: value === pos ? "var(--wc-accent-fg)" : "var(--wc-text-secondary)",
            fontSize: 16,
            cursor: "pointer",
            display: "flex", alignItems: "center", justifyContent: "center",
            lineHeight: 1,
          }}
        >
          {label}
        </button>
      ))}
    </div>
  );
}


function DisplayContent({
  showOutputTimer, onTimerChange,
  timerFloating, onTimerFloatingChange,
  timerCountDown, onTimerModeChange,
  timerFont, onTimerFontChange,
  timerFontSize, onTimerFontSizeChange,
  timerPosition, onTimerPositionChange,
  timerShowMs, onTimerShowMsChange,
  timerMargin, onTimerMarginChange,
  timerPreview, onTimerPreviewChange,
  committedTimerStyle,
  outputs, onOutputsChange,
  defaultOutputId, onDefaultOutputChange,
}: {
  showOutputTimer: boolean;
  onTimerChange: (v: boolean) => void;
  timerFloating: boolean;
  onTimerFloatingChange: (v: boolean) => void;
  timerCountDown: boolean;
  onTimerModeChange: (v: boolean) => void;
  timerFont: string;
  onTimerFontChange: (v: string) => void;
  timerFontSize: number;
  onTimerFontSizeChange: (v: number) => void;
  timerPosition: TimerPosition;
  onTimerPositionChange: (v: TimerPosition) => void;
  timerShowMs: boolean;
  onTimerShowMsChange: (v: boolean) => void;
  timerMargin: number;
  onTimerMarginChange: (v: number) => void;
  timerPreview: boolean;
  onTimerPreviewChange: (v: boolean) => void;
  /** Committed (applied) style, used to restore mpv state on cancel. */
  committedTimerStyle: { font: string; fontSize: number; position: TimerPosition; margin: number };
  outputs: OutputDestination[];
  onOutputsChange: (outputs: OutputDestination[]) => void;
  defaultOutputId: string;
  onDefaultOutputChange: (id: string) => void;
}) {
  const { t } = useLocale();
  const [screens, setScreens] = useState<ScreenInfo[]>([]);
  const [systemFonts, setSystemFonts] = useState<string[]>([]);
  const [selectedOutputId, setSelectedOutputId] = useState(outputs[0]?.id ?? "default");

  useEffect(() => {
    if (!outputs.some((output) => output.id === selectedOutputId)) setSelectedOutputId(outputs[0]?.id ?? "default");
  }, [outputs, selectedOutputId]);

  useEffect(() => {
    listVideoScreens().then(setScreens).catch(console.error);
    listSystemFonts().then(setSystemFonts).catch(console.error);
  }, []);

  // Derive preview text from current draft show_ms setting.
  const previewText = timerShowMs ? "00:00.000" : "00:00";

  // Whenever draft style settings change while preview is on, push them live.
  useEffect(() => {
    if (!timerPreview) return;
    void previewOutputTimer(timerFont, timerFontSize, timerPosition, timerMargin, previewText);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [timerPreview, timerFont, timerFontSize, timerPosition, timerMargin, previewText]);

  const handlePreviewToggle = (on: boolean) => {
    onTimerPreviewChange(on);
    if (on) {
      void previewOutputTimer(timerFont, timerFontSize, timerPosition, timerMargin, previewText);
    } else {
      void previewOutputTimer(
        committedTimerStyle.font, committedTimerStyle.fontSize,
        committedTimerStyle.position, committedTimerStyle.margin,
        null,
      );
    }
  };

  return (
    <>
      <Section title={t("preferences.outputSurface")}>
        <OutputManager
          outputs={outputs}
          onChange={onOutputsChange}
          screens={screens}
          selectedOutputId={selectedOutputId}
          onSelectedOutputChange={setSelectedOutputId}
          defaultOutputId={defaultOutputId}
          onDefaultOutputChange={onDefaultOutputChange}
        />
        <Row label={t("preferences.outputTimer")}>
          <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
            <input
              type="checkbox"
              checked={showOutputTimer}
              onChange={(e) => onTimerChange(e.target.checked)}
              style={{ width: 14, height: 14, cursor: "pointer" }}
            />
            <span style={{ fontSize: 13, color: "var(--wc-text)" }}>
              {t("preferencesExtra.showCueTimer")}
            </span>
          </label>
        </Row>
        {showOutputTimer && (
          <>
            <Row label={t("preferencesExtra.displayMode")}>
              <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
                <input
                  type="checkbox"
                  checked={timerFloating}
                  onChange={(e) => onTimerFloatingChange(e.target.checked)}
                  style={{ width: 14, height: 14, cursor: "pointer" }}
                />
                <span style={{ fontSize: 13, color: "var(--wc-text)" }}>
                  {t("preferencesExtra.floatingTimer")}
                </span>
              </label>
            </Row>
            <Row label={t("preferencesExtra.timerMode")}>
              <div style={{ display: "flex", gap: 16 }}>
                <label style={{ display: "flex", alignItems: "center", gap: 6, cursor: "pointer" }}>
                  <input
                    type="radio"
                    checked={!timerCountDown}
                    onChange={() => onTimerModeChange(false)}
                    style={{ cursor: "pointer" }}
                  />
                  <span style={{ fontSize: 13, color: "var(--wc-text)" }}>{t("preferencesExtra.elapsed")}</span>
                </label>
                <label style={{ display: "flex", alignItems: "center", gap: 6, cursor: "pointer" }}>
                  <input
                    type="radio"
                    checked={timerCountDown}
                    onChange={() => onTimerModeChange(true)}
                    style={{ cursor: "pointer" }}
                  />
                  <span style={{ fontSize: 13, color: "var(--wc-text)" }}>{t("preferencesExtra.remaining")}</span>
                </label>
              </div>
            </Row>
            <Row label={t("preferencesExtra.milliseconds")}>
              <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
                <input
                  type="checkbox"
                  checked={timerShowMs}
                  onChange={(e) => onTimerShowMsChange(e.target.checked)}
                  style={{ width: 14, height: 14, cursor: "pointer" }}
                />
                <span style={{ fontSize: 13, color: "var(--wc-text)" }}>{t("preferencesExtra.showMilliseconds")}</span>
              </label>
            </Row>
            <Row label={t("preferences.position")}>
              <div style={{ opacity: timerFloating ? 0.35 : 1, pointerEvents: timerFloating ? "none" : undefined }}>
                <TimerPositionPicker value={timerPosition} onChange={onTimerPositionChange} />
              </div>
              {timerFloating && (
                <span style={{ fontSize: 11, color: "var(--wc-text-faint)", marginLeft: 8 }}>
                  {t("preferencesExtra.notApplicablePosition")}
                </span>
              )}
            </Row>
            {timerPosition !== "center" && !timerFloating && (
              <Row label={t("preferencesExtra.cornerMargin")}>
                <input
                  type="range" min={0} max={300} step={5}
                  value={timerMargin} style={{ flex: 1 }}
                  onChange={(e) => onTimerMarginChange(Number(e.target.value))}
                />
                <span style={{ width: 40, textAlign: "right", fontFamily: "monospace", fontSize: 12, color: "var(--wc-text-secondary)" }}>
                  {timerMargin}px
                </span>
              </Row>
            )}
            <Row label={t("preferencesExtra.preview")}>
              <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
                <input
                  type="checkbox"
                  checked={timerPreview}
                  onChange={(e) => handlePreviewToggle(e.target.checked)}
                  style={{ width: 14, height: 14, cursor: "pointer" }}
                />
                <span style={{ fontSize: 13, color: "var(--wc-text)" }}>
                  {t("preferencesExtra.showPreview")}
                </span>
              </label>
            </Row>
            <Row label={t("inspector.font")}>
              <>
                <input
                  type="text"
                  list="timer-font-list"
                  value={timerFont}
                  onChange={(e) => onTimerFontChange(e.target.value)}
                  placeholder={t("preferencesExtra.fontPlaceholder")}
                  style={{ ...inputStyle, flex: 1, fontFamily: timerFont }}
                />
                <datalist id="timer-font-list">
                  {systemFonts.map((f) => <option key={f} value={f} />)}
                </datalist>
              </>
            </Row>
            <Row label={t("inspector.fontSize")}>
              <DragNumber
                min={20}
                max={400}
                step={4}
                value={timerFontSize}
                onChange={(e) => onTimerFontSizeChange(Number(e.target.value))}
                style={{ ...inputStyle, width: 80 }}
              />
              <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>
                {t("preferencesExtra.fontSizeHint")}
              </span>
            </Row>
          </>
        )}
      </Section>

      <ProjectorToolsSection
        output={outputs.find((output) => output.id === selectedOutputId)}
        defaultOutputName={outputs.find((output) => output.id === selectedOutputId)?.name}
        onTransformChange={(transform) => onOutputsChange(outputs.map((output) => output.id === selectedOutputId ? { ...output, transform } : output))}
      />
    </>
  );
}

function OutputManager({ outputs, onChange, screens, selectedOutputId, onSelectedOutputChange, defaultOutputId, onDefaultOutputChange }: { outputs: OutputDestination[]; onChange: (v: OutputDestination[]) => void; screens: ScreenInfo[]; selectedOutputId: string; onSelectedOutputChange: (id: string) => void; defaultOutputId: string; onDefaultOutputChange: (id: string) => void }) {
  const { t } = useLocale();
  // Display tabs deliberately do not include network destinations: a network
  // stream has no monitor, native window, or projector calibration surface.
  const displayOutputs = outputs.filter((output) => output.sink_kind === "display");
  const current = displayOutputs.find((o) => o.id === selectedOutputId) ?? displayOutputs[0];
  const monitorConflict = (output: OutputDestination) => output.enabled && output.monitor !== null
    ? displayOutputs.find((other) => other.id !== output.id && other.enabled && other.monitor === output.monitor)
    : undefined;
  const currentMonitorOwner = current ? monitorConflict(current) : undefined;
  const patch = (p: Partial<OutputDestination>) => current && onChange(outputs.map((o) => o.id === current.id ? { ...o, ...p, fullscreen_locked: true } : o));
  const add = () => {
    const id = createOutputId();
    const usedNames = new Set(outputs.map((output) => output.name));
    let index = displayOutputs.length + 1;
    while (usedNames.has(t("outputScreenUi.newName", { count: index }))) index += 1;
    const next: OutputDestination = {
      id, name: t("outputScreenUi.newName", { count: index }), sink_kind: "display",
      monitor: null, floating_window: null, enabled: true, transform: cloneOutputTransform(),
      fullscreen_locked: true, always_on_top: false, hide_cursor: false,
    };
    onChange([...outputs, next]);
    onSelectedOutputChange(id);
  };
  const remove = (id: string) => {
    if (outputs.length <= 1) return;
    const removedIndex = outputs.findIndex((output) => output.id === id);
    if (removedIndex < 0) return;
    const removed = outputs[removedIndex];
    if (displayOutputs.length <= 1) return;
    const rest = outputs.filter((o) => o.id !== id);
    onChange(rest);
    if (removed.id === defaultOutputId) onDefaultOutputChange(rest.find((output) => output.sink_kind === "display" && output.enabled)?.id ?? rest.find((output) => output.sink_kind === "display")?.id ?? "");
    // Keep the selection near the removed tab; when removing the last tab,
    // select the preceding one. This also works when the removed tab is not active.
    if (selectedOutputId === id) {
      const nextSelection = rest.filter((output) => output.sink_kind === "display")[Math.min(removedIndex, displayOutputs.length - 2)] ?? rest.find((output) => output.sink_kind === "display");
      onSelectedOutputChange(nextSelection.id);
    }
  };
  if (!current) return null;
  return <>
    <Row label={t("outputScreenUi.screensLabel")}>
      <div role="tablist" aria-label={t("preferences.namedOutputs")} style={{ display: "flex", flexWrap: "wrap", gap: 5, width: "100%" }}>
        {displayOutputs.map((output) => {
          const selected = output.id === current.id;
          const outputMonitorOwner = monitorConflict(output);
          return (
            <div
              key={output.id}
              role="tab"
              aria-selected={selected}
              tabIndex={selected ? 0 : -1}
              onClick={() => onSelectedOutputChange(output.id)}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  onSelectedOutputChange(output.id);
                }
              }}
              style={{
                display: "inline-flex", alignItems: "center", gap: 5, minWidth: 0,
                padding: "3px 5px 3px 9px", borderRadius: 5,
                border: `1px solid ${outputMonitorOwner ? "#ef4444" : selected ? "var(--wc-accent)" : "var(--wc-border-strong)"}`,
                background: outputMonitorOwner ? "rgba(239,68,68,0.14)" : selected ? "color-mix(in srgb, var(--wc-accent) 16%, var(--wc-bg-surface))" : "var(--wc-bg-surface)",
                color: outputMonitorOwner ? "#fca5a5" : selected ? "var(--wc-text)" : "var(--wc-text-secondary)",
                cursor: "pointer", maxWidth: "100%",
              }}
            >
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", maxWidth: 150 }}>{output.name || t("preferencesUi.outputName")}</span>
              {!output.enabled && <span style={{ fontSize: 10, color: "var(--wc-text-faint)" }}>{t("common.disabledShort")}</span>}
              {output.id === defaultOutputId && <span style={{ fontSize: 10, lineHeight: 1.4, padding: "1px 4px", borderRadius: 3, background: "var(--wc-accent)", color: "var(--wc-accent-fg)", whiteSpace: "nowrap" }}>{t("outputScreenUi.defaultBadge")}</span>}
              {displayOutputs.length > 1 && (
                <button
                  type="button"
                  aria-label={t("outputScreenUi.removeScreen", { name: output.name })}
                  title={t("outputScreenUi.removeScreen", { name: output.name })}
                  onClick={(event) => { event.stopPropagation(); remove(output.id); }}
                  style={{ border: 0, background: "transparent", color: "var(--wc-text-muted)", padding: "0 2px", fontSize: 15, lineHeight: 1, cursor: "pointer" }}
                >×</button>
              )}
            </div>
          );
        })}
        <button
          type="button"
          aria-label={t("outputScreenUi.addScreen")}
          title={t("outputScreenUi.addScreen")}
          onClick={add}
          style={{ ...btnStyle, minWidth: 28, padding: "2px 8px", fontSize: 16, lineHeight: 1.2 }}
        >＋</button>
      </div>
    </Row>
    <Row label={t("preferences.defaultOutput")}>
      {current.id === defaultOutputId ? (
        <span style={{ fontSize: 12, fontWeight: 600, color: "var(--wc-accent)" }}>{t("outputScreenUi.defaultStatus")}</span>
      ) : (
        <button type="button" onClick={() => current.enabled && onDefaultOutputChange(current.id)} disabled={!current.enabled} style={btnStyle}>
          {t("outputScreenUi.makeDefault")}
        </button>
      )}
    </Row>
    <Row label=""><span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>{t("outputScreenUi.defaultRoutingHint")}</span></Row>
    <Row label={t("common.name")}><input style={inputStyle} value={current.name} onChange={(e) => patch({ name: e.target.value })} /></Row>
    <Row label={t("common.enabled")}><input type="checkbox" checked={current.enabled} onChange={(e) => {
      const enabled = e.target.checked;
      patch({ enabled });
      if (!enabled && current.id === defaultOutputId) onDefaultOutputChange(outputs.find((output) => output.id !== current.id && output.enabled)?.id ?? "");
    }} /></Row>
    <Row label={t("preferences.monitor")}>
      <Select style={{ ...selectStyle, ...(currentMonitorOwner ? { borderColor: "#ef4444", color: "#fca5a5", background: "rgba(239,68,68,0.14)" } : {}) }} value={current.monitor ?? "floating"} onChange={(e) => patch({ monitor: e.target.value === "floating" ? null : Number(e.target.value) })}>
        <option value="floating">{t("preferencesExtra.floatingWindow")}</option>
        {current.monitor !== null && !screens.some((screen) => screen.index === current.monitor) && <option value={current.monitor}>{t("preferencesExtra.screen", { count: current.monitor + 1 })} ({t("preferences.noMonitor")})</option>}
        {screens.map((s) => {
          const monitorOwner = displayOutputs.find((output) => output.id !== current.id && output.enabled && output.monitor === s.index);
          return <option key={s.index} value={s.index} disabled={Boolean(monitorOwner) && current.monitor !== s.index}>
            {t("preferencesExtra.screen", { count: s.index + 1 })}{s.is_primary ? ` (${t("output.primary")})` : ""}{monitorOwner && current.monitor !== s.index ? ` — ${t("fullscreenUi.monitorUsedBy", { name: monitorOwner.name })}` : ""}
          </option>;
        })}
      </Select>
      <button onClick={() => void identifyOutputScreen(current.monitor)}>{t("preferencesExtra.identify")}</button>
    </Row>
    {currentMonitorOwner && (
      <Row label="">
        <span style={{ fontSize: 11, color: "#fbbf24" }}>
            {t("fullscreenUi.monitor", { count: current.monitor! + 1 })}: {current.name}; {currentMonitorOwner.name}
        </span>
      </Row>
    )}
    <Row label={t("outputScreenUi.windowBehavior")}>
      <div style={{ display: "grid", gap: 9 }}>
        <label style={{ display: "flex", alignItems: "flex-start", gap: 8, cursor: "pointer" }}>
          <input
            type="checkbox"
            checked={current.always_on_top}
            onChange={(event) => patch({ always_on_top: event.target.checked })}
            style={{ width: 14, height: 14, marginTop: 2, cursor: "pointer" }}
          />
          <span style={{ display: "grid", gap: 2 }}>
            <span style={{ fontSize: 13, color: "var(--wc-text)" }}>{t("outputScreenUi.alwaysOnTop")}</span>
            <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>{t("outputScreenUi.alwaysOnTopHint")}</span>
          </span>
        </label>
        <label style={{ display: "flex", alignItems: "flex-start", gap: 8, cursor: "pointer" }}>
          <input
            type="checkbox"
            checked={current.hide_cursor}
            onChange={(event) => patch({ hide_cursor: event.target.checked })}
            style={{ width: 14, height: 14, marginTop: 2, cursor: "pointer" }}
          />
          <span style={{ display: "grid", gap: 2 }}>
            <span style={{ fontSize: 13, color: "var(--wc-text)" }}>{t("outputScreenUi.hideCursor")}</span>
            <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>{t("outputScreenUi.hideCursorHint")}</span>
          </span>
        </label>
      </div>
    </Row>
    <Row label="">
      <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>
        {t("preferencesExtra.outputSafetyHint")}
      </span>
    </Row>
  </>;
}

// ---------------------------------------------------------------------------
// Network output destinations (NDI / SRT)
// ---------------------------------------------------------------------------

function NetworkOutputContent({ outputs, onOutputsChange }: { outputs: OutputDestination[]; onOutputsChange: (outputs: OutputDestination[]) => void }) {
  const { t } = useLocale();
  const networkOutputs = outputs.filter((output) => output.sink_kind === "ndi" || output.sink_kind === "srt");
  const [selectedId, setSelectedId] = useState<string>(networkOutputs[0]?.id ?? "");
  const [status, setStatus] = useState<NetworkIoStatus | null>(null);
  const [runtimeStatuses, setRuntimeStatuses] = useState<NetworkOutputRuntimeStatus[]>([]);
  const [statusError, setStatusError] = useState<string | null>(null);
  const current = networkOutputs.find((output) => output.id === selectedId) ?? networkOutputs[0];

  useEffect(() => {
    if (!networkOutputs.some((output) => output.id === selectedId)) setSelectedId(networkOutputs[0]?.id ?? "");
  }, [networkOutputs, selectedId]);
  const refreshStatus = useCallback(() => {
    setStatusError(null);
    void Promise.all([getNetworkIoStatus(), getNetworkOutputStatuses()])
      .then(([providerStatus, outputStatuses]) => { setStatus(providerStatus); setRuntimeStatuses(outputStatuses); })
      .catch((error) => setStatusError(String(error)));
  }, []);
  useEffect(() => {
    refreshStatus();
    const timer = window.setInterval(refreshStatus, 1000);
    return () => window.clearInterval(timer);
  }, [refreshStatus]);

  const patch = (partial: Partial<OutputDestination>) => {
    if (!current) return;
    onOutputsChange(outputs.map((output) => output.id === current.id ? { ...output, ...partial } : output));
  };
  const patchNetwork = (partial: ReturnType<typeof cloneNetworkOutputSettings>) => patch({ network: partial });
  const add = (sink: "ndi" | "srt") => {
    const id = createOutputId();
    const usedNames = new Set(outputs.map((output) => output.name));
    const key = sink === "ndi" ? "networkOutputUi.newNdiName" : "networkOutputUi.newSrtName";
    let count = networkOutputs.filter((output) => output.sink_kind === sink).length + 1;
    while (usedNames.has(t(key, { count }))) count += 1;
    const network = cloneNetworkOutputSettings();
    if (sink === "ndi") network.ndi.enabled = true;
    else network.srt.enabled = true;
    const next: OutputDestination = {
      id, name: t(key, { count }), sink_kind: sink, network, monitor: null,
      floating_window: null, enabled: true, transform: cloneOutputTransform(), fullscreen_locked: true,
      always_on_top: false, hide_cursor: false,
    };
    onOutputsChange([...outputs, next]);
    setSelectedId(id);
  };
  const remove = () => {
    if (!current) return;
    const rest = outputs.filter((output) => output.id !== current.id);
    onOutputsChange(rest);
    setSelectedId(rest.find((output) => output.sink_kind === "ndi" || output.sink_kind === "srt")?.id ?? "");
  };
  const provider = current?.sink_kind === "ndi" ? status?.ndi : current?.sink_kind === "srt" ? status?.srt : undefined;
  const runtime = current ? runtimeStatuses.find((item) => item.output_id === current.id) : undefined;
  const network = cloneNetworkOutputSettings(current?.network);
  const availabilityColour = provider?.availability === "ready" ? "#4ade80" : provider?.availability === "unavailable" ? "#ef4444" : "#facc15";

  return <>
    <Section title={t("networkOutputUi.title")}>
      <Row label={t("networkOutputUi.outputs")}>
        <div role="tablist" aria-label={t("networkOutputUi.outputs")} style={{ display: "flex", flexWrap: "wrap", gap: 5, width: "100%" }}>
          {networkOutputs.map((output) => (
            <button key={output.id} role="tab" aria-selected={output.id === current?.id} onClick={() => setSelectedId(output.id)} style={{ ...btnStyle, borderColor: output.id === current?.id ? "var(--wc-accent)" : undefined, background: output.id === current?.id ? "color-mix(in srgb, var(--wc-accent) 16%, var(--wc-bg-surface))" : undefined }}>
              {output.sink_kind.toUpperCase()} · {output.name}{!output.enabled ? ` (${t("common.disabledShort")})` : ""}
            </button>
          ))}
          <button type="button" onClick={() => add("ndi")} style={btnStyle}>＋ NDI</button>
          <button type="button" onClick={() => add("srt")} style={btnStyle}>＋ SRT</button>
        </div>
      </Row>
      {!current ? <div style={{ fontSize: 12, color: "var(--wc-text-faint)" }}>{t("networkOutputUi.empty")}</div> : <>
        <Row label={t("common.name")}><input style={inputStyle} value={current.name} onChange={(event) => patch({ name: event.target.value })} /></Row>
        <Row label={t("common.enabled")}><input type="checkbox" checked={current.enabled} onChange={(event) => {
          const enabled = event.target.checked;
          const next = cloneNetworkOutputSettings(current.network);
          if (current.sink_kind === "ndi") next.ndi.enabled = enabled;
          else next.srt.enabled = enabled;
          patch({ enabled, network: next });
        }} /></Row>
        <Row label=""><button type="button" onClick={remove} style={btnStyle}>{t("networkOutputUi.remove")}</button></Row>
      </>}
    </Section>
    {current && <>
      <Section title={current.sink_kind === "ndi" ? t("networkOutputUi.ndiSettings") : t("networkOutputUi.srtSettings")}>
        {current.sink_kind === "ndi" ? <>
          <Row label={t("networkOutputUi.streamName")}><input style={inputStyle} value={network.ndi.stream_name} onChange={(event) => patchNetwork({ ...network, ndi: { ...network.ndi, stream_name: event.target.value } })} /></Row>
          <Row label={t("networkOutputUi.quality")}><Select style={selectStyle} value={network.ndi.quality} onChange={(event) => patchNetwork({ ...network, ndi: { ...network.ndi, quality: event.target.value as typeof network.ndi.quality } })}><option value="highest">{t("networkOutputUi.qualityHighest")}</option><option value="low_bandwidth">{t("networkOutputUi.qualityLow")}</option></Select></Row>
          <Row label={t("networkOutputUi.ndiGroup")}><input style={inputStyle} value={network.ndi.group} onChange={(event) => patchNetwork({ ...network, ndi: { ...network.ndi, group: event.target.value } })} /></Row>
          <Row label=""><div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
            <button type="button" onClick={() => void openExternalUrl("https://ndi.link/NDIRedistV6").catch(console.error)} style={btnStyle}>{t("networkInputUi.ndiRuntimeLink")}</button>
            <button type="button" onClick={() => void openExternalUrl("https://ndi.video/").catch(console.error)} style={btnStyle}>{t("networkInputUi.ndiOfficialLink")}</button>
          </div></Row>
        </> : <>
          <Row label={t("networkOutputUi.mode")}><Select style={selectStyle} value={network.srt.mode} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, mode: event.target.value as typeof network.srt.mode } })}><option value="listener">{t("networkOutputUi.listener")}</option><option value="caller">{t("networkOutputUi.caller")}</option><option value="rendezvous">{t("networkOutputUi.rendezvous")}</option></Select></Row>
          <Row label={network.srt.mode === "listener" ? t("networkOutputUi.bindAddress") : t("networkOutputUi.remoteAddress")}><input style={inputStyle} placeholder={network.srt.mode === "listener" ? "0.0.0.0" : "192.168.1.100"} value={network.srt.host} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, host: event.target.value } })} /></Row>
          <Row label={t("preferencesUi.port")}><DragNumber min={1} max={65535} value={network.srt.port} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, port: Number(event.target.value) } })} style={{ ...inputStyle, width: 110 }} /></Row>
          <Row label={t("networkOutputUi.latency")}><DragNumber min={20} max={10000} value={network.srt.latency_ms} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, latency_ms: Number(event.target.value) } })} style={{ ...inputStyle, width: 110 }} /><span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>ms</span></Row>
          <details style={{ marginLeft: 182, marginBottom: 8 }}><summary style={{ cursor: "pointer", fontSize: 12 }}>{t("networkOutputUi.advanced")}</summary><div style={{ paddingTop: 10 }}>
            <Row label={t("networkOutputUi.streamId")}><input style={inputStyle} value={network.srt.stream_id ?? ""} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, stream_id: event.target.value || null } })} /></Row>
            <Row label={t("networkOutputUi.passphrase")}><input type="password" autoComplete="new-password" style={inputStyle} value={network.srt.passphrase ?? ""} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, passphrase: event.target.value || null } })} /></Row>
            <Row label={t("networkOutputUi.payload")}><input type="number" min={1} max={1456} style={{ ...inputStyle, width: 110 }} value={network.srt.payload_size ?? ""} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, payload_size: event.target.value === "" ? null : Number(event.target.value) } })} /></Row>
            <Row label={t("networkOutputUi.bitrate")}><input type="number" min={100} max={200000} style={{ ...inputStyle, width: 110 }} value={network.srt.bitrate_kbps ?? ""} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, bitrate_kbps: event.target.value === "" ? null : Number(event.target.value) } })} /><span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>kbps</span></Row>
            <Row label={t("networkOutputUi.resolution")}><input type="number" min={16} max={16384} style={{ ...inputStyle, width: 90 }} value={network.srt.width ?? ""} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, width: event.target.value === "" ? null : Number(event.target.value) } })} /><span>×</span><input type="number" min={16} max={16384} style={{ ...inputStyle, width: 90 }} value={network.srt.height ?? ""} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, height: event.target.value === "" ? null : Number(event.target.value) } })} /></Row>
            <Row label={t("networkOutputUi.fps")}><input type="number" min={1} max={240} style={{ ...inputStyle, width: 90 }} value={network.srt.fps ?? ""} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, fps: event.target.value === "" ? null : Number(event.target.value) } })} /></Row>
            <Row label={t("networkOutputUi.codec")}><Select style={selectStyle} value={network.srt.codec ?? ""} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, codec: event.target.value || null } })}><option value="">{t("networkOutputUi.auto")}</option><option value="h264">H.264</option><option value="hevc">HEVC / H.265</option></Select></Row>
            <Row label={t("networkOutputUi.lateDrop")}><input type="checkbox" checked={network.srt.too_late_packet_drop} onChange={(event) => patchNetwork({ ...network, srt: { ...network.srt, too_late_packet_drop: event.target.checked } })} /></Row>
          </div></details>
        </>}
      </Section>
      <Section title={t("networkOutputUi.status")}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 5 }}><span style={{ color: availabilityColour, fontWeight: 600 }}>{provider ? t(`networkOutputUi.${provider.availability}`) : t("networkOutputUi.checking")}</span><button type="button" style={btnStyle} onClick={refreshStatus}>{t("common.refresh")}</button></div>
        {provider && <div style={{ fontSize: 11, lineHeight: 1.45, color: "var(--wc-text-secondary)" }}>{provider.detail}{provider.library_path ? <><br /><code>{provider.library_path}</code></> : null}</div>}
        {runtime && <div style={{ marginTop: 8, padding: "7px 9px", border: "1px solid var(--wc-border)", borderRadius: 4, fontSize: 11, lineHeight: 1.55 }}>
          <div><strong>{t("networkOutputUi.runtimeState")}</strong>: {t(`networkOutputUi.state_${runtime.state}`)}</div>
          <div>{t("networkOutputUi.submittedFrames")}: {runtime.submitted_frames.toLocaleString()} · {t("networkOutputUi.supersededFrames")}: {runtime.superseded_frames.toLocaleString()}</div>
          {runtime.last_error && <div style={{ color: "#ef4444", overflowWrap: "anywhere" }}>{t("networkOutputUi.lastError")}: {runtime.last_error}</div>}
        </div>}
        {statusError && <div style={{ fontSize: 11, color: "#ef4444" }}>{statusError}</div>}
        <div style={{ marginTop: 8, fontSize: 11, color: "var(--wc-text-faint)" }}>{t("networkOutputUi.transportLimit")}</div>
      </Section>
    </>}
  </>;
}

// ---------------------------------------------------------------------------
// Personalization content
// ---------------------------------------------------------------------------

const CUE_COLOR_STYLES: { value: CueColorStyle }[] = [
  { value: "stripe" },
  { value: "full_row" },
];

function PersonalizationContent({
  theme, onThemeChange,
}: {
  theme: Pick<DisplayPreferences, "theme" | "cue_color_style">;
  onThemeChange: (t: Pick<DisplayPreferences, "theme" | "cue_color_style">) => void;
}) {
  const { locale, setLocale, t } = useLocale();
  return (
    <>
      <Section title={t("preferencesLabels.cueAppearance")}>
        <Row label={t("preferencesLabels.cueColorStyle")}>
          <Select
            style={selectStyle}
            value={theme.cue_color_style}
            onChange={(e) => onThemeChange({ ...theme, cue_color_style: e.target.value as CueColorStyle })}
          >
            {CUE_COLOR_STYLES.map(({ value }) => (
              <option key={value} value={value}>{t(value === "stripe" ? "personalizationUi.stripe" : "personalizationUi.fullRow")}</option>
            ))}
          </Select>
        </Row>
      </Section>

      <Section title={t("preferences.appearance")}>
        <Row label={t("preferences.theme")}>
          <Select
            style={selectStyle}
            value={theme.theme}
            onChange={(e) => onThemeChange({ ...theme, theme: e.target.value as typeof theme.theme })}
          >
            <option value="dark">{t("preferences.dark")}</option>
            <option value="navy">{t("preferencesLabels.navy")}</option>
            <option value="light">{t("preferences.light")} ({t("preferencesLabels.warmCream")})</option>
            <option value="system">{t("preferences.system")} ({t("preferencesLabels.followOs")})</option>
          </Select>
        </Row>
      </Section>
      <Section title={t("preferences.language")}>
        <Row label={t("preferences.language")}>
          <Select style={selectStyle} value={locale} onChange={(e) => setLocale(e.target.value as "ru" | "en")}>
            <option value="ru">{t("preferencesUi.languageRu")}</option>
            <option value="en">{t("preferencesUi.languageEn")}</option>
          </Select>
        </Row>
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>{t("preferencesLabels.languageHint")}</div>
      </Section>
    </>
  );
}

// ---------------------------------------------------------------------------
// OSC / Network content
// ---------------------------------------------------------------------------

function OscContent({
  config,
  onChange,
}: {
  config: OscReceiveConfig;
  onChange: (c: OscReceiveConfig) => void;
}) {
  const { t } = useLocale();
  return (
    <>
      <Section title={t("networkUi.oscReceive")}>
        <Row label={t("networkUi.enable")}>
          <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
            <input
              type="checkbox"
              checked={config.enabled}
              onChange={(e) => onChange({ ...config, enabled: e.target.checked })}
              style={{ width: 14, height: 14, cursor: "pointer" }}
            />
            <span style={{ fontSize: 13, color: "var(--wc-text)" }}>
              {t("networkUi.listenOsc")}
            </span>
          </label>
        </Row>
        <Row label={t("preferencesUi.port")}>
          <DragNumber
            min={1024}
            max={65535}
            value={config.port}
            onChange={(e) => onChange({ ...config, port: Number(e.target.value) })}
            style={{ ...inputStyle, width: 100 }}
          />
          <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>{t("networkUi.defaultPort")}</span>
        </Row>
        <Row label={t("preferencesUi.ipAllowlist")}>
          <div style={{ flex: 1 }}>
            <textarea
              rows={3}
              placeholder={t("networkUi.allowlistPlaceholder")}
              value={config.allowed_ips.join("\n")}
              onChange={(e) => {
                const ips = e.target.value.split("\n").map(s => s.trim()).filter(Boolean);
                onChange({ ...config, allowed_ips: ips });
              }}
              style={{ ...inputStyle, width: "100%", resize: "vertical", fontFamily: "monospace" }}
            />
            <span style={{ fontSize: 11, color: "var(--wc-text-faint)", display: "block", marginTop: 2 }}>
              {t("networkUi.allowlistHint")}
            </span>
          </div>
        </Row>
        <Row label={t("networkUi.addressReference")}>
          <div style={{ flex: 1, fontSize: 11, color: "var(--wc-text-muted)", lineHeight: 1.6 }}>
            <code style={{ fontFamily: "monospace" }}>/inkue/go</code> · <code>/inkue/stop</code> · <code>/inkue/hardstop</code><br />
            <code>/inkue/pause</code> · <code>/inkue/resume</code><br />
            <code>/inkue/cue/&#123;n&#125;/go</code> · <code>/inkue/cue/&#123;n&#125;/select</code> · <code>/inkue/cue/&#123;n&#125;/stop</code><br />
            <code>/inkue/cue/&#123;n&#125;/seek &lt;s&gt;</code> · <code>…/seek/relative &lt;±s&gt;</code> · <code>…/seek/percent &lt;0..1&gt;</code>
          </div>
        </Row>
      </Section>

      <OscPatchesPanel />

      <Section title={t("networkUi.oscFeedback")}>
        <Row label={t("networkUi.enable")}>
          <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
            <input
              type="checkbox"
              checked={config.feedback_enabled}
              onChange={(e) => onChange({ ...config, feedback_enabled: e.target.checked })}
              style={{ width: 14, height: 14, cursor: "pointer" }}
            />
            <span style={{ fontSize: 13, color: "var(--wc-text)" }}>
              {t("networkUi.broadcastCueInfo")}
            </span>
          </label>
        </Row>
        {config.feedback_enabled && (
          <>
            <Row label={t("preferences.destination")}>
              <input
                type="text"
                value={config.feedback_host}
                onChange={(e) => onChange({ ...config, feedback_host: e.target.value })}
                placeholder="127.0.0.1"
                style={{ ...inputStyle, flex: 1 }}
              />
              <span style={{ fontSize: 12, color: "var(--wc-text-muted)" }}>:</span>
              <DragNumber
                min={1024}
                max={65535}
                value={config.feedback_port}
                onChange={(e) => onChange({ ...config, feedback_port: Number(e.target.value) })}
                style={{ ...inputStyle, width: 80 }}
              />
            </Row>
            <Row label={t("networkUi.progressRate")}>
              <DragNumber
                min={0}
                max={30}
                value={config.feedback_progress_hz}
                onChange={(e) => onChange({ ...config, feedback_progress_hz: Math.max(0, Math.min(30, Number(e.target.value))) })}
                style={{ ...inputStyle, width: 80 }}
              />
              <span style={{ fontSize: 12, color: "var(--wc-text-muted)" }}>
                {t("networkUi.progressHint")}
              </span>
            </Row>
            <Row label={t("networkUi.messagesSent")}>
              <div style={{ flex: 1, fontSize: 11, color: "var(--wc-text-muted)", lineHeight: 1.7 }}>
                <code style={{ fontFamily: "monospace" }}>/inkue/cue/number</code> — {t("uiFixes.oscCueNumber")}<br />
                <code style={{ fontFamily: "monospace" }}>/inkue/cue/name</code> &nbsp;&nbsp;&nbsp;— {t("uiFixes.oscCueName")}<br />
                <code style={{ fontFamily: "monospace" }}>/inkue/cue/active</code> &nbsp;— {t("uiFixes.oscActive")}<br />
                <code style={{ fontFamily: "monospace" }}>{"/inkue/cue/{i}/progress"}</code> — {t("uiFixes.oscProgress")}<br />
                <code style={{ fontFamily: "monospace" }}>{"/inkue/cue/{i}/elapsed"}</code> &nbsp;— {t("uiFixes.oscSeconds")}<br />
                <code style={{ fontFamily: "monospace" }}>{"/inkue/cue/{i}/remaining"}</code> — {t("uiFixes.oscSecondsUnknown")}<br />
                <code style={{ fontFamily: "monospace" }}>{"/inkue/cue/{i}/duration"}</code> &nbsp;— {t("uiFixes.oscSecondsUnknown")}<br />
                <span style={{ color: "var(--wc-text-faint)" }}>
                  {t("networkUi.messagesHint")}
                </span>
              </div>
            </Row>
          </>
        )}
      </Section>
    </>
  );
}

// ---------------------------------------------------------------------------
// Draggable floating modal
// ---------------------------------------------------------------------------

interface Props {
  onClose: () => void;
  standalone?: boolean;
}

const MODAL_W = 740;
const MODAL_H = 520;

export function PreferencesModal({ onClose, standalone = false }: Props) {
  const { t } = useLocale();
  const { setGeneralPrefs, setDisplayPrefs } = useWorkspaceStore();
  const [category, setCategory] = useState<Category>("audio");
  const [prefs, setPrefs] = useState<AppPreferences | null>(null);
  const [draft, setDraft] = useState<AppPreferences | null>(null);
  // Machine audio config is stored separately from the workspace prefs.
  const [machineConfig, setMachineConfig] = useState<MachineAudioConfig>(DEFAULT_MACHINE_AUDIO_CONFIG);
  const [draftMachineConfig, setDraftMachineConfig] = useState<MachineAudioConfig>(DEFAULT_MACHINE_AUDIO_CONFIG);
  const [outputScreen, setOutputScreen_] = useState<number | null>(null);
  const [draftOutputScreen, setDraftOutputScreen] = useState<number | null>(null);
  const [draftOutputs, setDraftOutputs] = useState<OutputDestination[]>([]);
  const [draftDefaultOutputId, setDraftDefaultOutputId] = useState("default");
  const [showOutputTimer, setShowOutputTimer] = useState(false);
  const [draftShowOutputTimer, setDraftShowOutputTimer] = useState(false);
  const [timerCountDown, setTimerCountDown] = useState(false);
  const [draftTimerCountDown, setDraftTimerCountDown] = useState(false);
  const [timerFont, setTimerFont] = useState("DSEG7 Classic");
  const [draftTimerFont, setDraftTimerFont] = useState("DSEG7 Classic");
  const [timerFontSize, setTimerFontSize] = useState(120);
  const [draftTimerFontSize, setDraftTimerFontSize] = useState(120);
  const [timerPosition, setTimerPosition] = useState<TimerPosition>("center");
  const [draftTimerPosition, setDraftTimerPosition] = useState<TimerPosition>("center");
  const [timerShowMs, setTimerShowMs] = useState(false);
  const [draftTimerShowMs, setDraftTimerShowMs] = useState(false);
  const [timerMargin, setTimerMargin] = useState(50);
  const [draftTimerMargin, setDraftTimerMargin] = useState(50);
  const [timerPreview, setTimerPreview] = useState(false);
  const [timerFloating, setTimerFloating] = useState(false);
  const [draftTimerFloating, setDraftTimerFloating] = useState(false);
  const [theme, setTheme] = useState({ ...DEFAULT_DISPLAY_PREFS });
  const [draftTheme, setDraftTheme] = useState({ ...DEFAULT_DISPLAY_PREFS });
  const [availableBackends, setAvailableBackends] = useState<string[]>(["wasapi_shared", "wasapi_exclusive"]);
  const [oscConfig, setOscConfig_] = useState<OscReceiveConfig>({ enabled: false, port: 53001, allowed_ips: [], feedback_enabled: false, feedback_host: "127.0.0.1", feedback_port: 53000, feedback_progress_hz: 10 });
  const [applyError, setApplyError] = useState<string | null>(null);
  const [justApplied, setJustApplied] = useState(false);
  const [initFailed, setInitFailed] = useState(false);
  const [preferencesLoading, setPreferencesLoading] = useState(true);

  // Drag state
  const posRef = useRef({ x: Math.round((window.innerWidth - MODAL_W) / 2), y: Math.round((window.innerHeight - MODAL_H) / 2) });
  const [pos, setPos] = useState(posRef.current);
  const dragRef = useRef<{ startMouseX: number; startMouseY: number; startPosX: number; startPosY: number } | null>(null);
  // Monotonically increasing request id prevents an older load/retry from
  // overwriting a newer workspace-replacement reload.
  const loadRequestRef = useRef(0);

  const applyLoadedPreferences = useCallback((p: AppPreferences) => {
    setPrefs(p);
    setDraft(p);
    const normalizedOutputs = normalizeOutputDestinations(p.display.output_destinations, p.display.default_output_id, p.display.output_screen);
    setDraftOutputs(normalizedOutputs.destinations);
    setDraftDefaultOutputId(normalizedOutputs.defaultOutputId);
    const normalizedDefault = normalizedOutputs.destinations.find((output) => output.id === normalizedOutputs.defaultOutputId);
    setOutputScreen_(normalizedDefault?.monitor ?? null);
    setDraftOutputScreen(normalizedDefault?.monitor ?? null);
    const nextTheme = { ...DEFAULT_DISPLAY_PREFS, ...p.display };
    setTheme(nextTheme);
    setDraftTheme(nextTheme);
    const timer = p.display.show_output_timer ?? false;
    setShowOutputTimer(timer);
    setDraftShowOutputTimer(timer);
    const countDown = p.display.timer_count_down ?? false;
    setTimerCountDown(countDown);
    setDraftTimerCountDown(countDown);
    const font = p.display.timer_font ?? "DSEG7 Classic";
    setTimerFont(font);
    setDraftTimerFont(font);
    const fontSize = p.display.timer_font_size ?? 120;
    setTimerFontSize(fontSize);
    setDraftTimerFontSize(fontSize);
    const pos = p.display.timer_position ?? "center";
    setTimerPosition(pos);
    setDraftTimerPosition(pos);
    const showMs = p.display.timer_show_ms ?? false;
    setTimerShowMs(showMs);
    setDraftTimerShowMs(showMs);
    const margin = p.display.timer_margin ?? 50;
    setTimerMargin(margin);
    setDraftTimerMargin(margin);
    const floating = p.display.timer_floating ?? false;
    setTimerFloating(floating);
    setDraftTimerFloating(floating);
  }, []);

  const loadPreferences = useCallback(() => {
    const request = ++loadRequestRef.current;
    // A replacement invalidates every editable field. Keep Apply disabled
    // until this request installs a complete, current draft.
    setDraft(null);
    setApplyError(null);
    setInitFailed(false);
    setPreferencesLoading(true);
    let attempts = 0;

    // The preferences window is pre-created at startup (visible:false), so
    // this fetch can race slow backend startup. Retries also make an event
    // reload resilient without allowing an older request to overwrite it.
    const attempt = () => {
      withTimeout(getPreferences(), 12000, t("preferencesUi.timeoutLoad")).then((p) => {
        if (request !== loadRequestRef.current) return;
        applyLoadedPreferences(p);
        setPreferencesLoading(false);
      }).catch((e) => {
        if (request !== loadRequestRef.current) return;
        console.error(e);
        if (attempts < 5) {
          attempts += 1;
          window.setTimeout(attempt, 1500 * attempts);
        } else {
          setInitFailed(true);
          setPreferencesLoading(false);
        }
      });
    };
    attempt();
  }, [applyLoadedPreferences, t]);

  useEffect(() => {
    loadPreferences();
    getMachineAudioConfig().then((c) => { setMachineConfig(c); setDraftMachineConfig(c); }).catch(console.error);
    getAvailableBackends().then(setAvailableBackends).catch(console.error);
    getOscConfig().then(setOscConfig_).catch(console.error);
  }, [loadPreferences]);

  // FullscreenControl can change a physical monitor while this modal remains
  // open. Refresh only monitor fields so unsaved edits in every other field
  // stay intact and Apply cannot write an old monitor back to the backend.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen("preferences-updated", () => {
      void getPreferences().then((latest) => {
        if (cancelled) return;
        const normalizedLatest = normalizeOutputDestinations(
          latest.display.output_destinations,
          latest.display.default_output_id,
          latest.display.output_screen,
        );
        setPrefs((current) => current ? {
          ...current,
          display: {
            ...current.display,
            output_screen: latest.display.output_screen,
            output_destinations: mergeOutputMonitorAssignments(current.display.output_destinations ?? [], normalizedLatest.destinations),
          },
        } : current);
        setDraft((current) => current ? {
          ...current,
          display: {
            ...current.display,
            output_destinations: mergeOutputMonitorAssignments(current.display.output_destinations ?? [], normalizedLatest.destinations),
          },
        } : current);
        setDraftOutputs((current) => mergeOutputMonitorAssignments(current, normalizedLatest.destinations));
        const latestDefault = normalizedLatest.destinations.find((output) => output.id === draftDefaultOutputId);
        if (latestDefault) {
          setOutputScreen_(latestDefault.monitor);
          setDraftOutputScreen(latestDefault.monitor);
        }
      }).catch((error) => console.error("Failed to refresh output monitors", error));
    }).then((remove) => {
      if (cancelled) remove();
      else unlisten = remove;
    }).catch(console.error);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [draftDefaultOutputId]);

  // Workspace replacement is the only event that invalidates this draft. Do
  // not reload it for ordinary workspace-modified mutations while the user is
  // editing Preferences.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen("workspace-replaced", () => {
      if (!cancelled) loadPreferences();
    }).then((remove) => {
      if (cancelled) remove();
      else unlisten = remove;
    }).catch(console.error);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [loadPreferences]);

  // In standalone mode: apply theme preview to this window's document as the
  // user changes the theme selector so the preferences panel itself repaints.
  useEffect(() => {
    if (!standalone) return;
    const root = document.documentElement;
    const t = draftTheme.theme ?? "system";
    if (t === "system") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      const apply = (dark: boolean) => root.setAttribute("data-theme", dark ? "dark" : "light");
      apply(mq.matches);
      const h = (e: MediaQueryListEvent) => apply(e.matches);
      mq.addEventListener("change", h);
      return () => mq.removeEventListener("change", h);
    } else {
      root.setAttribute("data-theme", t);
    }
  }, [standalone, draftTheme.theme]);

  // Escape closes without applying
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") { e.stopImmediatePropagation(); onClose(); }
    };
    window.addEventListener("keydown", handler, { capture: true });
    return () => window.removeEventListener("keydown", handler, { capture: true });
  }, [onClose]);

  // Drag handlers registered on document to track mouse outside the element
  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!dragRef.current) return;
      const dx = e.clientX - dragRef.current.startMouseX;
      const dy = e.clientY - dragRef.current.startMouseY;
      const newPos = {
        x: dragRef.current.startPosX + dx,
        y: dragRef.current.startPosY + dy,
      };
      posRef.current = newPos;
      setPos({ ...newPos });
    };
    const onUp = () => { dragRef.current = null; document.body.style.cursor = ""; };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
    return () => { document.removeEventListener("mousemove", onMove); document.removeEventListener("mouseup", onUp); };
  }, []);

  const startDrag = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    e.preventDefault();
    if (standalone) {
      void getCurrentWindow().startDragging();
      return;
    }
    dragRef.current = {
      startMouseX: e.clientX, startMouseY: e.clientY,
      startPosX: posRef.current.x, startPosY: posRef.current.y,
    };
    document.body.style.cursor = "grabbing";
  };

  const handleApply = async () => {
    if (!draft) return;
    setApplyError(null);
    const outputValidationError = validateOutputDestinations(draftOutputs);
    if (outputValidationError) {
      setApplyError(localizedOutputValidationError(draftOutputs, t) || outputValidationError);
      return;
    }
    const normalizedOutputs = normalizeOutputDestinations(draftOutputs, draftDefaultOutputId, draftOutputScreen);
    const committedDefault = normalizedOutputs.destinations.find((output) => output.id === normalizedOutputs.defaultOutputId)!;
    const committedDisplay: DisplayPreferences = {
      ...draftTheme,
      output_screen: committedDefault.monitor,
      output_transform: cloneOutputTransform(committedDefault.transform),
      output_destinations: normalizedOutputs.destinations,
      default_output_id: normalizedOutputs.defaultOutputId,
      show_output_timer: draftShowOutputTimer,
      timer_floating: draftTimerFloating,
      timer_count_down: draftTimerCountDown,
      timer_font: draftTimerFont,
      timer_font_size: draftTimerFontSize,
      timer_position: draftTimerPosition,
      timer_show_ms: draftTimerShowMs,
      timer_margin: draftTimerMargin,
      // Clip Editor visibility is global UI state, so preserve the current
      // global values while applying the rest of this complete Preferences
      // draft; it is not project content and must not affect dirty state.
      show_live_panel: draft.display.show_live_panel ?? true,
      show_slice_panel: draft.display.show_slice_panel ?? true,
      clip_editor_active_tab: draft.display.clip_editor_active_tab ?? "Live",
    };
    const displayPayload: DisplayPreferences = {
      ...committedDisplay,
    };
    try {
      const machineAudioUpdate = machineAudioConfigsEqual(machineConfig, draftMachineConfig)
        ? Promise.resolve()
        : updateMachineAudioConfig(draftMachineConfig);
      const outputScreenUpdate = outputScreen === committedDefault.monitor
        ? Promise.resolve()
        : setOutputScreen(committedDefault.monitor);
      await Promise.all([
        machineAudioUpdate,
        updateAudioPreferences(draft.audio),
        updateGeneralPreferences(draft.general),
        outputScreenUpdate,
        updateDisplayPreferences(displayPayload),
        updateOutputDestinations(normalizedOutputs.destinations, normalizedOutputs.defaultOutputId),
      ]);
      if (standalone) {
        await emit("preferences-applied");
      } else {
        setGeneralPrefs(draft.general);
        setDisplayPrefs(committedDisplay);
      }
      setPrefs({ ...draft, display: committedDisplay });
      setDraft({ ...draft, display: committedDisplay });
      setDraftOutputs(normalizedOutputs.destinations);
      setDraftDefaultOutputId(normalizedOutputs.defaultOutputId);
      setMachineConfig(draftMachineConfig);
      setOutputScreen_(committedDefault.monitor);
      setShowOutputTimer(draftShowOutputTimer);
      setTimerCountDown(draftTimerCountDown);
      setTimerFont(draftTimerFont);
      setTimerFontSize(draftTimerFontSize);
      setTimerPosition(draftTimerPosition);
      setTimerShowMs(draftTimerShowMs);
      setTimerMargin(draftTimerMargin);
      setTimerFloating(draftTimerFloating);
      setTheme(draftTheme);
      setDraftOutputScreen(committedDefault.monitor);
      // Stay open: the operator often iterates (backend switch → re-patch →
      // test).  Show a brief confirmation instead of closing.
      setJustApplied(true);
      window.setTimeout(() => setJustApplied(false), 1600);
    } catch (e) {
      setApplyError(String(e));
    }
  };

  // Apply machine config immediately (without closing the modal).
  // Used for ASIO output pair which must restart the engine to take effect.
  const handleImmediateApplyMachine = useCallback(async (config: MachineAudioConfig) => {
    setApplyError(null);
    try {
      await updateMachineAudioConfig(config);
      setMachineConfig(config);
      setDraftMachineConfig(config);
    } catch (e) {
      setApplyError(String(e));
    }
  }, []);

  const handleDraftOutputsChange = (outputs: OutputDestination[]) => {
    const normalized = normalizeOutputDestinations(outputs, draftDefaultOutputId, draftOutputScreen);
    setDraftOutputs(normalized.destinations);
    setDraftDefaultOutputId(normalized.defaultOutputId);
    const defaultOutput = normalized.destinations.find((output) => output.id === normalized.defaultOutputId);
    setDraftOutputScreen(defaultOutput?.monitor ?? null);
  };

  const handleDraftDefaultOutputChange = (id: string) => {
    const normalized = normalizeOutputDestinations(draftOutputs, id, draftOutputScreen);
    setDraftOutputs(normalized.destinations);
    setDraftDefaultOutputId(normalized.defaultOutputId);
    const defaultOutput = normalized.destinations.find((output) => output.id === normalized.defaultOutputId);
    setDraftOutputScreen(defaultOutput?.monitor ?? null);
  };

  const handleCancel = () => {
    if (!prefs) return;
    // Restore committed style and clear preview.
    void previewOutputTimer(timerFont, timerFontSize, timerPosition, timerMargin, null);
    setTimerPreview(false);
    setDraft(prefs);
    setDraftMachineConfig(machineConfig);
    setDraftOutputScreen(outputScreen);
    const normalizedCommitted = normalizeOutputDestinations(
      prefs.display.output_destinations,
      prefs.display.default_output_id,
      prefs.display.output_screen,
    );
    setDraftOutputs(normalizedCommitted.destinations);
    setDraftDefaultOutputId(normalizedCommitted.defaultOutputId);
    setDraftShowOutputTimer(showOutputTimer);
    setDraftTimerCountDown(timerCountDown);
    setDraftTimerFont(timerFont);
    setDraftTimerFontSize(timerFontSize);
    setDraftTimerPosition(timerPosition);
    setDraftTimerShowMs(timerShowMs);
    setDraftTimerMargin(timerMargin);
    setDraftTimerFloating(timerFloating);
    setDraftTheme(theme);
    onClose();
  };

  const modalStyle: React.CSSProperties = standalone
    ? { position: "fixed", inset: 0, background: "var(--wc-bg-app)", display: "flex", flexDirection: "column", overflow: "hidden" }
    : {
        position: "fixed",
        left: pos.x, top: pos.y,
        width: MODAL_W, height: MODAL_H,
        zIndex: 50000,
        background: "var(--wc-bg-app)",
        border: "1px solid var(--wc-border-strong)",
        borderRadius: 8,
        boxShadow: "0 24px 64px rgba(0,0,0,0.8)",
        display: "flex", flexDirection: "column",
        overflow: "hidden",
      };

  const inner = (
    <>
      {!standalone && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 49999, background: "rgba(0,0,0,0.45)" }}
          onClick={handleCancel}
        />
      )}

      {/* Floating window */}
      <div
        style={modalStyle}
        onClick={standalone ? undefined : (e) => e.stopPropagation()}
      >
        {/* Draggable title bar */}
        <div
          onMouseDown={startDrag}
          style={{
            height: 40, display: "flex", alignItems: "center",
            padding: "0 14px", flexShrink: 0,
            background: "var(--wc-bg-app)", borderBottom: "1px solid var(--wc-border)",
            cursor: "grab", userSelect: "none",
          }}
        >
          <span style={{ fontSize: 13, fontWeight: 600, color: "var(--wc-text-bright)" }}>
            {t("preferences.title")}
          </span>
          <button
            onClick={handleCancel}
            style={{
              marginLeft: "auto", background: "transparent", border: "none",
              color: "var(--wc-text-muted)", cursor: "pointer", fontSize: 16, lineHeight: 1, padding: 4,
            }}
          >
            ✕
          </button>
        </div>
        <div style={{ padding: "7px 14px", fontSize: 11, color: "var(--wc-text-faint)", borderBottom: "1px solid var(--wc-border)" }}>
          {t("preferencesUi.globalScopeHint")}
        </div>

        {/* Body */}
        <div style={{ display: "flex", flex: 1, overflow: "hidden" }}>
          {/* Sidebar */}
          <div style={{ width: 150, background: "var(--wc-bg-deepest)", borderRight: "1px solid var(--wc-border)", padding: "8px 0", flexShrink: 0 }}>
            {CATEGORIES.map((cat) => (
              <button
                key={cat.id}
                onClick={() => setCategory(cat.id)}
                style={{
                  display: "flex", alignItems: "center", gap: 8,
                  width: "100%", padding: "7px 14px",
                  background: category === cat.id ? "var(--wc-accent)" : "transparent",
                  border: "none",
                  color: category === cat.id ? "var(--wc-accent-fg)" : "var(--wc-text-secondary)",
                  fontSize: 12, cursor: "pointer", textAlign: "left",
                }}
              >
                <span style={{ fontSize: 14 }}>{cat.icon}</span>
                {cat.id === "audio" ? t("preferences.audio") : cat.id === "general" ? t("preferences.defaults") : cat.id === "network" ? t("preferencesUi.network") : cat.id === "networkOutput" ? t("networkOutputUi.title") : cat.id === "display" ? t("preferencesUi.display") : cat.id === "personalization" ? t("preferencesUi.personalization") : t("mediaRuntimeUi.settings")}
              </button>
            ))}
          </div>

          {/* Content */}
          <div style={{ flex: 1, overflowY: "auto", padding: "18px 22px" }}>
            {draft === null ? (
              <div style={{ display: "flex", flexDirection: "column", gap: 10, alignItems: "center", justifyContent: "center", height: "100%", color: "var(--wc-text-muted)", fontSize: 13 }}>
                {initFailed ? (
                  <>
                    <span style={{ color: "#ef4444" }}>{t("preferencesUi.failedLoad")}</span>
                    <button style={btnStyle} onClick={() => loadPreferences()}>{t("common.retry")}</button>
                  </>
                ) : (
                  t("preferencesUi.loading")
                )}
              </div>
            ) : (
              <>
                {category === "audio" && (
                  <AudioContent
                    machineConfig={draftMachineConfig}
                    audioPrefs={draft.audio}
                    onMachineConfigChange={setDraftMachineConfig}
                    onAudioPrefsChange={(audio) => setDraft({ ...draft, audio })}
                    availableBackends={availableBackends}
                    onImmediateApplyMachine={handleImmediateApplyMachine}
                  />
                )}
                {category === "general" && (
                  <GeneralContent
                    prefs={draft.general}
                    onChange={(general) => setDraft({ ...draft, general })}
                  />
                )}
                {category === "display" && (
                  <DisplayContent
                    showOutputTimer={draftShowOutputTimer}
                    onTimerChange={setDraftShowOutputTimer}
                    timerFloating={draftTimerFloating}
                    onTimerFloatingChange={setDraftTimerFloating}
                    timerCountDown={draftTimerCountDown}
                    onTimerModeChange={setDraftTimerCountDown}
                    timerFont={draftTimerFont}
                    onTimerFontChange={setDraftTimerFont}
                    timerFontSize={draftTimerFontSize}
                    onTimerFontSizeChange={setDraftTimerFontSize}
                    timerPosition={draftTimerPosition}
                    onTimerPositionChange={setDraftTimerPosition}
                    timerShowMs={draftTimerShowMs}
                    onTimerShowMsChange={setDraftTimerShowMs}
                    timerMargin={draftTimerMargin}
                    onTimerMarginChange={setDraftTimerMargin}
                    timerPreview={timerPreview}
                    onTimerPreviewChange={setTimerPreview}
                    committedTimerStyle={{ font: timerFont, fontSize: timerFontSize, position: timerPosition, margin: timerMargin }}
                    outputs={draftOutputs}
                    onOutputsChange={handleDraftOutputsChange}
                    defaultOutputId={draftDefaultOutputId}
                    onDefaultOutputChange={handleDraftDefaultOutputChange}
                  />
                )}
                {category === "personalization" && (
                  <PersonalizationContent
                    theme={draftTheme}
                    onThemeChange={setDraftTheme}
                  />
                )}
                {category === "network" && (
                  <>
                    <NetworkInterfaceSection />
                    <OscContent
                      config={oscConfig}
                      onChange={async (c) => {
                        setOscConfig_(c);
                        try { await setOscConfig(c); } catch (e) { console.error(e); }
                      }}
                    />
                    <TcPreferences />
                    <MidiTriggerPreferences />
                  </>
                )}
                {category === "networkOutput" && (
                  <NetworkOutputContent outputs={draftOutputs} onOutputsChange={handleDraftOutputsChange} />
                )}
                {category === "mediaRuntime" && <MediaRuntimeSection />}
              </>
            )}
          </div>
        </div>

        {/* Footer */}
        <div style={{
          height: applyError ? "auto" : 46, display: "flex", flexDirection: "column",
          justifyContent: "center",
          borderTop: "1px solid var(--wc-border)", flexShrink: 0,
          background: "var(--wc-bg-app)",
        }}>
          {applyError && (
            <div style={{ padding: "6px 16px 0", fontSize: 11, color: "#ef4444" }}>
              {applyError}
            </div>
          )}
          <div style={{ display: "flex", alignItems: "center", justifyContent: "flex-end", gap: 8, padding: "8px 16px" }}>
            {justApplied && (
              <span style={{ fontSize: 11, color: "#4ade80", marginRight: 4 }}>
                ✓ {t("preferencesUi.applied")}
              </span>
            )}
            <button
              onClick={() => void handleApply()}
              disabled={draft === null || preferencesLoading}
              style={{ ...btnStyle, padding: "5px 16px", fontSize: 12, background: "var(--wc-accent)", border: "1px solid var(--wc-accent-hover)", color: "var(--wc-accent-fg)", opacity: draft === null || preferencesLoading ? 0.5 : 1 }}
            >
              {t("preferencesUi.apply")}
            </button>
            <button onClick={handleCancel} style={{ ...btnStyle, padding: "5px 16px", fontSize: 12 }}>
              {t("preferencesUi.close")}
            </button>
          </div>
        </div>
      </div>
    </>
  );

  return standalone ? inner : createPortal(inner, document.body);
}
