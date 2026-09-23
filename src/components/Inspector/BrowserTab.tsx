import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { BrowserCueData, BrowserSurfaceState } from "../../lib/types";
import { Field, inputStyle, Section } from "./Field";
import { DragNumber } from "../common/DragNumber";
import { useLocale } from "../../i18n";
import { getBrowserSurfaceState } from "../../lib/commands";
import { createBrowserPreviewModel } from "./browserPreviewModel";

export function BrowserTab({
  cue,
  onSave,
}: {
  cue: BrowserCueData;
  onSave: (properties: Partial<BrowserCueData>) => Promise<void>;
}) {
  const { t } = useLocale();
  const [surface, setSurface] = useState<BrowserSurfaceState | null>(null);
  const [previewReloadNonce, setPreviewReloadNonce] = useState(0);
  const preview = createBrowserPreviewModel(cue.url, previewReloadNonce);
  const [previewLoaded, setPreviewLoaded] = useState(false);
  const [previewError, setPreviewError] = useState(false);
  const [previewFit, setPreviewFit] = useState(true);
  const previewViewportRef = useRef<HTMLDivElement | null>(null);
  const [previewScale, setPreviewScale] = useState(1);
  useEffect(() => {
    let active = true;
    void getBrowserSurfaceState()
      .then((snapshot) => { if (active) setSurface(snapshot); })
      .catch(() => { /* Browser status is unavailable outside the Tauri host. */ });
    const subscription = listen<BrowserSurfaceState>("browser-surface-state", (event) => {
      if (active) setSurface(event.payload);
    });
    return () => {
      active = false;
      void subscription.then((dispose) => dispose());
    };
  }, []);
  useEffect(() => {
    setPreviewLoaded(false);
    setPreviewError(false);
  }, [cue.id, preview.frameKey]);
  useEffect(() => {
    const viewport = previewViewportRef.current;
    if (!viewport) return;
    const updateScale = () => {
      // Keep the page's real viewport at 1280×720. Scaling the iframe itself
      // prevents narrow Inspector layout from reflowing or cropping dashboards
      // designed for the physical Browser output.
      setPreviewScale(previewFit ? viewport.clientWidth / 1280 : 1);
    };
    updateScale();
    const observer = new ResizeObserver(updateScale);
    observer.observe(viewport);
    return () => observer.disconnect();
  }, [previewFit]);
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
      <Section title={t("inspector.browser") }>
        <Field label={t("browserCue.url")}>
          <input
            type="url"
            defaultValue={cue.url}
            key={`${cue.id}-browser-url`}
            placeholder="http://localhost:3000"
            style={inputStyle}
            onBlur={(event) => void onSave({ url: event.target.value.trim() })}
          />
        </Field>
        <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12, color: "var(--wc-text-secondary)" }}>
          <input
            type="checkbox"
            checked={cue.reload_on_go}
            onChange={(event) => void onSave({ reload_on_go: event.target.checked })}
          />
          {t("browserCue.reloadOnGo")}
        </label>
        <Field label={t("browserCue.zoom")}>
          <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
            <DragNumber
              min={0.25}
              max={3}
              step={0.05}
              value={cue.zoom}
              onChange={(event) => void onSave({ zoom: Math.min(3, Math.max(0.25, Number(event.target.value) || 1)) })}
              style={{ ...inputStyle, width: 90 }}
            />
            <span style={{ color: "var(--wc-text-faint)", fontSize: 12 }}>×</span>
          </div>
        </Field>
      </Section>
      <div style={{ padding: "8px 10px", borderRadius: 5, background: "rgba(96,165,250,0.10)", color: "var(--wc-text-secondary)", fontSize: 11, lineHeight: 1.45 }}>
        {t("browserCue.mvpHint")}
      </div>
      {surface && (
        <div role="status" style={{ fontSize: 11, color: surface.error ? "#fca5a5" : "var(--wc-text-muted)" }}>
          {surface.error ?? `${surface.status}${surface.active ? " · active" : ""}`}
        </div>
      )}
      <section
        aria-label={t("browserCue.preview")}
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 6,
          padding: 10,
          borderRadius: 6,
          border: "1px solid var(--wc-border)",
          background: "var(--wc-surface-raised)",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
          <strong style={{ fontSize: 12 }}>{t("browserCue.preview")}</strong>
          <div style={{ display: "flex", gap: 6 }}>
            <button
              type="button"
              disabled={!preview.src}
              onClick={() => setPreviewFit((value) => !value)}
              style={{ ...inputStyle, width: "auto", padding: "3px 8px", cursor: preview.src ? "pointer" : "not-allowed" }}
            >
              {previewFit ? t("browserCue.previewFit") : t("browserCue.previewActualSize")}
            </button>
            <button
              type="button"
              disabled={!preview.src}
              onClick={() => setPreviewReloadNonce((value) => value + 1)}
              style={{ ...inputStyle, width: "auto", padding: "3px 8px", cursor: preview.src ? "pointer" : "not-allowed" }}
            >
              {t("browserCue.reloadPreview")}
            </button>
          </div>
        </div>
        <div
          role="status"
          aria-live="polite"
          aria-busy={Boolean(preview.src && !previewLoaded && !previewError)}
          style={{ fontSize: 11, color: preview.src ? "var(--wc-text-muted)" : "#fca5a5" }}
        >
          {!preview.src
            ? t("browserCue.previewInvalidUrl")
            : previewError
              ? t("browserCue.previewBlocked")
            : previewLoaded
              ? t("browserCue.previewReady")
              : t("browserCue.previewLoading")}
        </div>
        {preview.src && (
          <div
            ref={previewViewportRef}
            style={{
              position: "relative",
              width: "100%",
              aspectRatio: "16 / 9",
              minHeight: 180,
              overflow: "hidden",
              borderRadius: 4,
              background: "#111827",
            }}
          >
            <iframe
              key={`${cue.id}-${preview.frameKey}`}
              title={t("browserCue.previewFrame")}
              src={preview.src}
              loading="lazy"
              referrerPolicy="no-referrer"
              sandbox="allow-scripts allow-forms allow-same-origin"
              onLoad={() => { setPreviewError(false); setPreviewLoaded(true); }}
              onError={() => { setPreviewLoaded(false); setPreviewError(true); }}
              style={{
                position: "absolute",
                left: 0,
                top: 0,
                width: 1280,
                height: 720,
                border: "0",
                transform: `scale(${previewScale})`,
                transformOrigin: "top left",
                background: "#111827",
              }}
            />
          </div>
        )}
        <div style={{ fontSize: 10, lineHeight: 1.4, color: "var(--wc-text-faint)" }}>
          {t("browserCue.previewFrameHint")}
        </div>
      </section>
    </div>
  );
}
