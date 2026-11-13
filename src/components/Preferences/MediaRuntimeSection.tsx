import { useCallback, useEffect, useState } from "react";
import { relaunch } from "@tauri-apps/plugin-process";
import { useLocale } from "../../i18n";
import { getMediaRuntimeStatus, scheduleMediaRuntimeReinstall } from "../../lib/commands";
import type { MediaRuntimeStatus } from "../../lib/types";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { hasActivePlayback } from "../../lib/closeGuard";

export function MediaRuntimeSection() {
  const { t } = useLocale();
  const [status, setStatus] = useState<MediaRuntimeStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const refresh = useCallback(async () => {
    setBusy(true);
    setError("");
    try { setStatus(await getMediaRuntimeStatus()); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);

  const reinstall = async () => {
    const workspace = useWorkspaceStore.getState();
    if ((workspace.workspaceInfo?.is_modified || hasActivePlayback(workspace.cues))
      && !window.confirm(t("mediaRuntimeUi.restartWarning"))) return;
    setBusy(true);
    setError("");
    try {
      await scheduleMediaRuntimeReinstall(status?.state === "repair-required" ? "repair" : "update");
      await relaunch();
    } catch (cause) {
      setError(String(cause));
      setBusy(false);
    }
  };
  const componentState = (value: boolean) => value ? t("mediaRuntimeUi.installed") : t("mediaRuntimeUi.missing");
  const actionLabel = status?.state === "repair-required"
    ? t("mediaRuntimeUi.repair")
    : status?.update_available ? t("mediaRuntimeUi.update") : t("mediaRuntimeUi.reinstall");
  return <section style={{ marginBottom: 24 }}>
    <div style={{ fontSize: 11, fontWeight: 600, color: "var(--wc-text-muted)", textTransform: "uppercase", letterSpacing: "0.07em", marginBottom: 10, paddingBottom: 5, borderBottom: "1px solid var(--wc-border)" }}>{t("mediaRuntimeUi.settings")}</div>
    {([ ["FFmpeg", status?.ffmpeg], ["ffprobe", status?.ffprobe], ["libmpv", status?.libmpv] ] as const).map(([name, installed]) =>
      <div key={name} style={{ display: "flex", justifyContent: "space-between", maxWidth: 340, padding: "5px 0", fontSize: 12 }}>
        <span style={{ color: "var(--wc-text-secondary)" }}>{name}</span>
        <span style={{ color: installed ? "var(--wc-text)" : "var(--wc-text-muted)" }}>{installed === undefined ? t("common.loading") : componentState(installed)}</span>
      </div>)}
    {status?.version && <div style={{ fontSize: 11, color: "var(--wc-text-muted)", margin: "4px 0 10px" }}>{status.version}</div>}
    {status && <div style={{ maxWidth: 340, fontSize: 11, color: "var(--wc-text-muted)", margin: "4px 0 10px", overflowWrap: "anywhere" }}>
      <div>{t("mediaRuntimeUi.installedKit")}: {status.installed_kit_id || t("mediaRuntimeUi.noKit")}</div>
      <div>{t("mediaRuntimeUi.recommendedKit")}: {status.recommended_kit_id}</div>
      {status.update_available && <div style={{ marginTop: 4 }}>{t("mediaRuntimeUi.updateAvailable")}</div>}
      {status.repair_reason && <div role="alert" style={{ color: "#ef4444", marginTop: 4 }}>{status.repair_reason}</div>}
    </div>}
    {error && <div role="alert" style={{ color: "#ef4444", fontSize: 12, margin: "8px 0" }}>{error}</div>}
    <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
      <button disabled={busy} onClick={() => void refresh()} style={buttonStyle}>{busy ? t("common.loading") : t("mediaRuntimeUi.checkIntegrity")}</button>
      <button disabled={busy} onClick={() => void reinstall()} style={buttonStyle}>{busy ? t("common.loading") : actionLabel}</button>
    </div>
  </section>;
}

const buttonStyle: React.CSSProperties = { padding: "5px 10px", background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", borderRadius: 4, color: "var(--wc-text)", fontSize: 11, cursor: "pointer" };
