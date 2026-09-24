import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { exit, relaunch } from "@tauri-apps/plugin-process";
import { useLocale } from "../i18n";
import { getMediaRuntimeStatus, prepareMediaRuntime } from "../lib/commands";
import type { MediaRuntimeProgress } from "../lib/types";

type ViewState = "checking" | "preparing" | "error" | "ready";

export function MediaRuntimeBootstrap({ children }: { children: React.ReactNode }) {
  const { t } = useLocale();
  const started = useRef(false);
  const relaunchRequired = useRef(false);
  const [view, setView] = useState<ViewState>("checking");
  const [progress, setProgress] = useState<MediaRuntimeProgress>({ phase: "checking", percent: 0 });
  const [error, setError] = useState("");

  const prepare = async (force = false) => {
    setView("preparing");
    setError("");
    try {
      if (!force && !relaunchRequired.current) {
        const current = await getMediaRuntimeStatus();
        if (current.ready) {
          setView("ready");
          return;
        }
      }
      const status = await prepareMediaRuntime(force);
      if (!status.ready) throw new Error(t("mediaRuntimeUi.incomplete"));
      // The backend may have had to initialize without libmpv. Relaunch so the
      // newly installed DLL is loaded by the normal application startup path.
      relaunchRequired.current = true;
      await relaunch();
    } catch (cause) {
      setError(String(cause));
      setView("error");
    }
  };

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<MediaRuntimeProgress>("media-runtime-progress", (event) => setProgress(event.payload))
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    if (!started.current) {
      started.current = true;
      void prepare(false);
    }
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  if (view === "ready") return <>{children}</>;

  if (view === "checking" || view === "preparing") {
    const percent = Math.max(0, Math.min(100, Math.round(progress.percent ?? 0)));
    const component = progress.message?.toLowerCase().includes("libmpv")
      ? t("mediaRuntimeUi.downloadingLibmpv")
      : progress.message?.toLowerCase().includes("ffmpeg")
        ? t("mediaRuntimeUi.downloadingFfmpeg")
        : "";
    const progressMessage = progress.phase === "complete"
      ? t("mediaRuntimeUi.complete")
      : progress.phase === "download" && component
        ? component
        : t("mediaRuntimeUi.downloading");
    return <main style={styles.page}>
      <section style={styles.card}>
        <div style={styles.mark}>Q</div>
        <h1 style={styles.title}>{t("mediaRuntimeUi.preparing")}</h1>
        <div style={styles.label}>{t("mediaRuntimeUi.title")}</div>
        <div style={styles.track}><div style={{ ...styles.bar, width: `${percent}%` }} /></div>
        <div style={styles.percent}>{percent}%</div>
        <p style={styles.message}>{progressMessage}</p>
      </section>
    </main>;
  }

  return <main style={styles.page}>
    <section style={styles.card}>
      <div style={{ ...styles.mark, background: "#7f1d1d" }}>!</div>
      <h1 style={styles.title}>{t("mediaRuntimeUi.failed")}</h1>
      <p style={styles.message}>{error || t("mediaRuntimeUi.unknownError")}</p>
      <div style={styles.actions}>
        <button style={styles.primary} onClick={() => void prepare(false)}>{t("common.retry")}</button>
        <button style={styles.secondary} onClick={() => void exit(0)}>{t("common.close")}</button>
      </div>
    </section>
  </main>;
}

const styles: Record<string, React.CSSProperties> = {
  page: { minHeight: "100vh", display: "grid", placeItems: "center", background: "var(--wc-bg-app, #111318)", color: "var(--wc-text, #f3f4f6)", fontFamily: "Segoe UI, sans-serif" },
  card: { width: "min(440px, calc(100vw - 48px))", textAlign: "center", padding: 32, background: "var(--wc-bg-surface, #1b1d24)", border: "1px solid var(--wc-border, #343741)", borderRadius: 12, boxSizing: "border-box" },
  mark: { width: 42, height: 42, margin: "0 auto 18px", display: "grid", placeItems: "center", borderRadius: 12, background: "var(--wc-accent, #6957d9)", fontSize: 22, fontWeight: 700 },
  title: { fontSize: 19, margin: "0 0 24px" },
  label: { fontSize: 13, textAlign: "left", marginBottom: 9, color: "var(--wc-text-secondary, #c5c7d0)" },
  track: { height: 7, overflow: "hidden", background: "var(--wc-bg-deepest, #101116)", borderRadius: 8 },
  bar: { height: "100%", background: "var(--wc-accent, #6957d9)", transition: "width 180ms ease" },
  percent: { textAlign: "right", fontSize: 12, marginTop: 6, color: "var(--wc-text-muted, #979aa6)" },
  message: { minHeight: 20, fontSize: 13, lineHeight: 1.5, color: "var(--wc-text-secondary, #c5c7d0)", overflowWrap: "anywhere" },
  actions: { display: "flex", justifyContent: "center", gap: 10, marginTop: 24 },
  primary: { padding: "8px 16px", border: 0, borderRadius: 6, color: "white", background: "var(--wc-accent, #6957d9)", cursor: "pointer" },
  secondary: { padding: "8px 16px", border: "1px solid var(--wc-border-strong, #454854)", borderRadius: 6, color: "var(--wc-text, #f3f4f6)", background: "transparent", cursor: "pointer" },
};
