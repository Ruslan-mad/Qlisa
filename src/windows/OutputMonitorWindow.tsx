import { useCallback, useEffect, useRef, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { MediaPreview } from "../components/Inspector/MediaThumbnail";
import { useLocale } from "../i18n";
import {
  getOutputMonitorFrame,
  getCue,
  listOutputMonitorSources,
  setOutputMonitorSource,
  type OutputMonitorSource,
} from "../lib/commands";
import {
  EMPTY_OUTPUT_MONITOR_FRAME,
  frameForOutputSource,
  mergeOutputMonitorFrame,
  type OutputMonitorFrameState,
} from "./outputMonitorModel";

const PREVIEW_TAB = "__preview__";
const ALWAYS_ON_TOP_KEY = "qlisa_output_monitor_always_on_top";
const POLL_MS = 250;

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

function statusKey(status: OutputMonitorFrameState["status"]): string {
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
  const [frame, setFrame] = useState<OutputMonitorFrameState>(EMPTY_OUTPUT_MONITOR_FRAME);
  const [preview, setPreview] = useState<MonitorPreviewSelection | null>(null);
  const [alwaysOnTop, setAlwaysOnTop] = useState(readAlwaysOnTop);
  const generation = useRef(0);
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
    void setOutputMonitorSource(null).catch(() => undefined);
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
        void setOutputMonitorSource(null).catch(() => undefined);
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
      void setOutputMonitorSource(null).catch(() => undefined);
    };
  }, [hide, reloadSources]);

  useEffect(() => {
    void getCurrentWindow().setAlwaysOnTop(alwaysOnTop).catch(console.error);
    try { localStorage.setItem(ALWAYS_ON_TOP_KEY, String(alwaysOnTop)); } catch { /* ignore */ }
  }, [alwaysOnTop]);

  useEffect(() => {
    const currentGeneration = ++generation.current;
    setFrame(EMPTY_OUTPUT_MONITOR_FRAME);

    if (!active || !selectedTab || selectedTab === PREVIEW_TAB) {
      void setOutputMonitorSource(null).catch(console.error);
      return;
    }

    let stopped = false;
    let timer = 0;
    let sequence: number | null = null;

    const poll = async () => {
      try {
        const next = await getOutputMonitorFrame(selectedTab, sequence);
        if (stopped || generation.current !== currentGeneration) return;
        sequence = next.sequence;
        setFrame((previous) => mergeOutputMonitorFrame(previous, next));
      } catch (error) {
        if (!stopped && generation.current === currentGeneration) {
          setFrame({
            ...EMPTY_OUTPUT_MONITOR_FRAME,
            sourceId: selectedTab,
            status: "error",
            error: String(error),
          });
        }
      }
      if (!stopped) timer = window.setTimeout(poll, POLL_MS);
    };

    void setOutputMonitorSource(selectedTab)
      .then(poll)
      .catch((error) => {
        if (!stopped) {
          setFrame({ ...EMPTY_OUTPUT_MONITOR_FRAME, sourceId: selectedTab, status: "error", error: String(error) });
        }
      });

    return () => {
      stopped = true;
      window.clearTimeout(timer);
      void setOutputMonitorSource(null).catch(() => undefined);
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

  const displayedFrame = frameForOutputSource(frame, selectedTab);
  const statusText = displayedFrame.error
    ? `${t(statusKey(displayedFrame.status) as never)}: ${displayedFrame.error}`
    : t(statusKey(displayedFrame.status) as never);

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
            width: "100%", maxWidth: 640, aspectRatio: "16 / 9", position: "relative",
            display: "flex", alignItems: "center", justifyContent: "center", overflow: "hidden",
            background: "#000", border: "1px solid var(--wc-border-strong)", borderRadius: 5,
          }}>
            {displayedFrame.dataUrl && <img src={displayedFrame.dataUrl} alt="" style={{ width: "100%", height: "100%", objectFit: "contain" }} />}
            {(!displayedFrame.dataUrl || displayedFrame.status === "error" || displayedFrame.status === "unavailable") && (
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
