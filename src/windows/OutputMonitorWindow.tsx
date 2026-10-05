import { useCallback, useEffect, useRef, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { MediaPreview } from "../components/Inspector/MediaThumbnail";
import { useLocale } from "../i18n";
import {
  getOutputMonitorFrame,
  getCue,
  listOutputMonitorSources,
  reportOutputMonitorFrontendDiagnostics,
  setOutputMonitorSource,
  type OutputMonitorSource,
} from "../lib/commands";
import {
  clearOutputMonitorCanvas,
  OutputMonitorMetricsWindow,
  outputMonitorGenerationIsCurrent,
  outputMonitorNextDelay,
  outputMonitorPacketIsCurrent,
  paintOutputMonitorFrame,
  parseOutputMonitorPacket,
} from "./outputMonitorModel";

const PREVIEW_TAB = "__preview__";
const ALWAYS_ON_TOP_KEY = "qlisa_output_monitor_always_on_top";
const POLL_MS = 1000 / 30;

interface MonitorPreviewEvent {
  cueId: string;
}

interface MonitorPreviewSelection extends MonitorPreviewEvent {
  name: string;
  path: string;
  kind: "video" | "image";
}

function readAlwaysOnTop(): boolean {
  try { return localStorage.getItem(ALWAYS_ON_TOP_KEY) === "true"; }
  catch { return false; }
}

function statusKey(status: "waiting" | "black" | "unavailable" | "error"): string {
  switch (status) {
    case "black": return "outputMonitorUi.black";
    case "unavailable": return "outputMonitorUi.unavailable";
    case "error": return "outputMonitorUi.error";
    default: return "outputMonitorUi.waiting";
  }
}

export function OutputMonitorWindow() {
  const { t } = useLocale();
  const [active, setActive] = useState(false);
  const [sources, setSources] = useState<OutputMonitorSource[]>([]);
  const [selectedTab, setSelectedTab] = useState("");
  const [sourceRevision, setSourceRevision] = useState(0);
  const [monitorStatus, setMonitorStatus] = useState<"waiting" | "black" | "unavailable" | "error">("waiting");
  const [aspectRatio, setAspectRatio] = useState<number | null>(null);
  const [preview, setPreview] = useState<MonitorPreviewSelection | null>(null);
  const [alwaysOnTop, setAlwaysOnTop] = useState(readAlwaysOnTop);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const imageDataRef = useRef<{ imageData: ImageData | null }>({ imageData: null });
  const generation = useRef(0);
  const selectionToken = useRef(0);
  const previewRequest = useRef(0);
  const sourceReloadRequest = useRef(0);

  useEffect(() => {
    const theme = localStorage.getItem("wc_theme") ?? "dark";
    document.documentElement.setAttribute("data-theme", theme);
  }, []);

  const reloadSources = useCallback(async () => {
    const request = ++sourceReloadRequest.current;
    try {
      const next = await listOutputMonitorSources();
      if (sourceReloadRequest.current !== request) return;
      setSources(next);
      setSourceRevision((revision) => revision + 1);
      setSelectedTab((current) => {
        if (current === PREVIEW_TAB || next.some((source) => source.id === current)) return current;
        return next[0]?.id ?? PREVIEW_TAB;
      });
    } catch (error) {
      if (sourceReloadRequest.current !== request) return;
      console.error("Failed to list output monitor sources", error);
      setSources([]);
      setSelectedTab(PREVIEW_TAB);
      setSourceRevision((revision) => revision + 1);
    }
  }, []);

  const hide = useCallback(() => {
    setActive(false);
    generation.current += 1;
    const token = Math.max(Date.now() * 1000, selectionToken.current + 1);
    selectionToken.current = token;
    void setOutputMonitorSource(null, token).catch(() => undefined);
    void emit("output-monitor-visible", false);
    void getCurrentWindow().hide();
  }, []);

  useEffect(() => {
    const opened = listen("output-monitor-opened", () => {
      setActive(true);
      void reloadSources();
    });
    const selection = listen<MonitorPreviewEvent | null>("output-monitor-preview-selection", (event) => {
      const request = ++previewRequest.current;
      const cueId = event.payload?.cueId;
      if (!cueId) {
        setPreview(null);
        return;
      }
      void getCue(cueId)
        .then((cue) => {
          if (previewRequest.current !== request) return;
          if ((cue.cue_type === "video" || cue.cue_type === "image") && cue.file_path) {
            setPreview({
              cueId: cue.id,
              name: cue.name,
              path: cue.file_path,
              kind: cue.cue_type,
            });
          } else {
            setPreview(null);
          }
        })
        .catch(() => {
          if (previewRequest.current === request) setPreview(null);
        });
    });
    const hideRequested = listen("output-monitor-request-hide", () => hide());
    const workspace = listen("workspace-modified", () => void reloadSources());
    const preferences = listen("preferences-updated", () => void reloadSources());
    const onVisibility = () => {
      if (document.visibilityState === "hidden") {
        setActive(false);
        const token = Math.max(Date.now() * 1000, selectionToken.current + 1);
        selectionToken.current = token;
        void setOutputMonitorSource(null, token).catch(() => undefined);
      } else {
        setActive(true);
        void reloadSources();
      }
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      generation.current += 1;
      document.removeEventListener("visibilitychange", onVisibility);
      void opened.then((unlisten) => unlisten());
      void selection.then((unlisten) => unlisten());
      void hideRequested.then((unlisten) => unlisten());
      void workspace.then((unlisten) => unlisten());
      void preferences.then((unlisten) => unlisten());
      const token = Math.max(Date.now() * 1000, selectionToken.current + 1);
      selectionToken.current = token;
      void setOutputMonitorSource(null, token).catch(() => undefined);
    };
  }, [hide, reloadSources]);

  useEffect(() => {
    void getCurrentWindow().setAlwaysOnTop(alwaysOnTop).catch(console.error);
    try { localStorage.setItem(ALWAYS_ON_TOP_KEY, String(alwaysOnTop)); } catch { /* ignore */ }
  }, [alwaysOnTop]);

  useEffect(() => {
    const currentGeneration = ++generation.current;
    const metrics = new OutputMonitorMetricsWindow();
    const reportInactive = (token: number) => {
      const now = performance.now();
      metrics.take(now, null, token, false);
      window.setTimeout(() => {
        const report = metrics.take(performance.now(), null, token, false);
        if (report) void reportOutputMonitorFrontendDiagnostics(report).catch(() => undefined);
      }, 1000);
    };
    setMonitorStatus("waiting");
    setAspectRatio(null);
    const canvas = canvasRef.current;
    if (canvas) clearOutputMonitorCanvas(canvas);
    const token = Math.max(Date.now() * 1000, selectionToken.current + 1);
    selectionToken.current = token;

    if (!active || !selectedTab || selectedTab === PREVIEW_TAB) {
      void setOutputMonitorSource(null, token).then(() => {
        reportInactive(token);
      }).catch(console.error);
      return;
    }

    let stopped = false;
    let timer = 0;
    let sequence: bigint | null = null;

    const poll = async () => {
      if (!outputMonitorGenerationIsCurrent(stopped, currentGeneration, generation.current)) return;
      const startedAt = performance.now();
      try {
        const buffer = await getOutputMonitorFrame(selectedTab, sequence === null ? null : Number(sequence), token);
        if (!outputMonitorGenerationIsCurrent(stopped, currentGeneration, generation.current)) return;
        const endedAt = performance.now();
        metrics.recordRequest(endedAt - startedAt);
        if (buffer) {
          const packet = parseOutputMonitorPacket(buffer);
          if (outputMonitorPacketIsCurrent(packet, token, sequence)) {
            sequence = packet.sequence;
            if (packet.status === "frame" || packet.status === "black") {
              const ageMs = Math.max(0, Date.now() - Number(packet.capturedAtUnixUs / 1000n));
              metrics.recordReceived(ageMs);
              setAspectRatio(packet.width / packet.height);
              if (packet.status === "black") setMonitorStatus("black");
              else setMonitorStatus("waiting");
              const canvas = canvasRef.current;
              if (canvas) {
                const conversionStart = performance.now();
                paintOutputMonitorFrame(canvas, packet, imageDataRef.current);
                metrics.recordDisplayed(performance.now() - conversionStart);
              }
            } else if (packet.status === "no_frame") {
              if (canvasRef.current) clearOutputMonitorCanvas(canvasRef.current);
              setAspectRatio(null);
              setMonitorStatus("unavailable");
            }
            // unchanged deliberately retains the last painted canvas.
          }
        }
      } catch (error) {
        if (outputMonitorGenerationIsCurrent(stopped, currentGeneration, generation.current)) {
          console.error("Output Monitor frame request failed", error);
          setMonitorStatus("error");
        }
      }
      if (!outputMonitorGenerationIsCurrent(stopped, currentGeneration, generation.current)) return;
      const report = metrics.take(performance.now(), selectedTab, token, true);
      if (report) void reportOutputMonitorFrontendDiagnostics(report).catch(() => undefined);
      timer = window.setTimeout(poll, outputMonitorNextDelay({ requestDurationMs: performance.now() - startedAt }, POLL_MS));
    };

    void setOutputMonitorSource(selectedTab, token)
      .then(() => {
        if (outputMonitorGenerationIsCurrent(stopped, currentGeneration, generation.current)) void poll();
      })
      .catch(() => {
        if (outputMonitorGenerationIsCurrent(stopped, currentGeneration, generation.current)) {
          setMonitorStatus("error");
        }
      });

    return () => {
      stopped = true;
      window.clearTimeout(timer);
      const cleanupToken = Math.max(Date.now() * 1000, selectionToken.current + 1);
      selectionToken.current = cleanupToken;
      void setOutputMonitorSource(null, cleanupToken).then(() => reportInactive(cleanupToken)).catch(() => undefined);
    };
  }, [active, selectedTab, sourceRevision]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") hide();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [hide]);

  useEffect(() => {
    const closeRequested = getCurrentWindow().onCloseRequested((event) => {
      event.preventDefault();
      hide();
    });
    return () => { void closeRequested.then((unlisten) => unlisten()); };
  }, [hide]);

  const statusText = t(statusKey(monitorStatus) as never);

  return (
    <div style={{
      position: "fixed", inset: 0, display: "flex", flexDirection: "column",
      background: "var(--wc-bg-app)", color: "var(--wc-text)", userSelect: "none",
      border: "1px solid var(--wc-border-strong)", overflow: "hidden",
    }}>
      <div
        onMouseDown={(event) => {
          if ((event.target as HTMLElement).closest("button, input, label")) return;
          void getCurrentWindow().startDragging();
        }}
        style={{
          height: 34, padding: "0 10px", display: "flex", alignItems: "center", gap: 10,
          background: "var(--wc-bg-deepest)", borderBottom: "1px solid var(--wc-border)",
          flexShrink: 0, cursor: "grab",
        }}
      >
        <span style={{ fontSize: 11, fontWeight: 700, letterSpacing: 0.6, color: "var(--wc-text-bright)" }}>
          {t("outputMonitorUi.title")}
        </span>
        <label style={{ marginLeft: "auto", display: "flex", gap: 6, alignItems: "center", fontSize: 11, cursor: "pointer" }}>
          <input
            type="checkbox"
            checked={alwaysOnTop}
            onChange={(event) => setAlwaysOnTop(event.target.checked)}
          />
          {t("outputMonitorUi.alwaysOnTop")}
        </label>
        <button
          onClick={hide}
          title={t("common.close")}
          aria-label={t("common.close")}
          style={{ border: 0, background: "transparent", color: "var(--wc-text-muted)", cursor: "pointer", fontSize: 14, padding: 4 }}
        >✕</button>
      </div>

      <div style={{ display: "flex", overflowX: "auto", flexShrink: 0, background: "var(--wc-bg-deepest)", borderBottom: "1px solid var(--wc-border)" }}>
        {sources.map((source) => (
          <button
            key={source.id}
            onClick={() => setSelectedTab(source.id)}
            style={{
              padding: "8px 14px", border: 0, borderRight: "1px solid var(--wc-border)",
              borderBottom: selectedTab === source.id ? "2px solid var(--wc-accent)" : "2px solid transparent",
              background: selectedTab === source.id ? "var(--wc-bg-surface)" : "transparent",
              color: selectedTab === source.id ? "var(--wc-text-bright)" : "var(--wc-text-muted)",
              cursor: "pointer", whiteSpace: "nowrap", fontSize: 12,
            }}
          >{source.name}</button>
        ))}
        <button
          onClick={() => setSelectedTab(PREVIEW_TAB)}
          style={{
            padding: "8px 14px", border: 0, borderRight: "1px solid var(--wc-border)",
            borderBottom: selectedTab === PREVIEW_TAB ? "2px solid var(--wc-accent)" : "2px solid transparent",
            background: selectedTab === PREVIEW_TAB ? "var(--wc-bg-surface)" : "transparent",
            color: selectedTab === PREVIEW_TAB ? "var(--wc-text-bright)" : "var(--wc-text-muted)",
            cursor: "pointer", whiteSpace: "nowrap", fontSize: 12,
          }}
        >{t("outputMonitorUi.preview")}</button>
      </div>

      <div style={{ flex: 1, minHeight: 0, padding: 12, display: "flex", alignItems: "center", justifyContent: "center" }}>
        {selectedTab === PREVIEW_TAB ? (
          active && preview ? (
            <div style={{ width: "100%", maxWidth: 640 }}>
              <MediaPreview cueId={preview.cueId} path={preview.path} kind={preview.kind} />
              <div style={{ textAlign: "center", marginTop: 4, fontSize: 11, color: "var(--wc-text-muted)", overflow: "hidden", textOverflow: "ellipsis" }}>
                {preview.name}
              </div>
            </div>
          ) : (
            <span style={{ fontSize: 12, color: "var(--wc-text-muted)" }}>{t("outputMonitorUi.selectMedia")}</span>
          )
        ) : (
          <div style={{
            width: "100%", maxWidth: 640, aspectRatio: aspectRatio ? String(aspectRatio) : "16 / 9", position: "relative",
            display: "flex", alignItems: "center", justifyContent: "center", overflow: "hidden",
            background: "#000", border: "1px solid var(--wc-border-strong)", borderRadius: 5,
          }}>
            <canvas ref={canvasRef} style={{ width: "100%", height: "100%", objectFit: "contain" }} />
            {(monitorStatus !== "waiting" || !aspectRatio) && (
              <span style={{ position: "absolute", padding: "6px 10px", borderRadius: 4, background: "rgba(0,0,0,.65)", color: "#d1d5db", fontSize: 11 }}>
                {statusText}
              </span>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
