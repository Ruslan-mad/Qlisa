import { CaretDown, CaretUp, Check, Monitor } from "@phosphor-icons/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getOutputControlStatuses,
  listVideoScreens,
  setDisplayOutputMonitor,
  toggleOutputWindow,
} from "../../lib/commands";
import type { OutputControlStatus, ScreenInfo } from "../../lib/types";
import { useLocale } from "../../i18n";
import { hasMonitorConflict, monitorOwner, physicalDisplayOutputs, physicalOutputsVisible, screenLabel } from "./fullscreenModel";

const buttonStyle = {
  display: "inline-flex",
  alignItems: "center",
  gap: 5,
  minWidth: 0,
  padding: "4px 7px",
  border: "1px solid var(--wc-border-strong)",
  background: "var(--wc-bg-surface)",
  color: "var(--wc-text-secondary)",
  fontSize: 10,
  fontWeight: 700,
  letterSpacing: 0.5,
  cursor: "pointer",
  userSelect: "none" as const,
};

function monitorText(screen: ScreenInfo, t: (key: string, vars?: Record<string, string | number | boolean | null | undefined>) => string): string {
  const label = screenLabel(screen, t("fullscreenUi.primary"), t("fullscreenUi.monitor", { count: screen.index + 1 }));
  return `${label} · ${screen.width}×${screen.height}`;
}

/** Split control for showing all physical outputs and assigning a monitor. */
export function FullscreenControl() {
  const { t } = useLocale();
  const rootRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [visible, setVisible] = useState(false);
  const [outputs, setOutputs] = useState<OutputControlStatus[]>([]);
  const [screens, setScreens] = useState<ScreenInfo[]>([]);
  const [busy, setBusy] = useState(false);

  const displayOutputs = useMemo(() => physicalDisplayOutputs(outputs), [outputs]);

  const refreshStatuses = useCallback(async () => {
    try {
      const nextOutputs = await getOutputControlStatuses();
      setOutputs(nextOutputs);
      setVisible(physicalOutputsVisible(nextOutputs));
    } catch (error) {
      console.error("Failed to refresh fullscreen output statuses", error);
    }
  }, []);

  const refreshScreens = useCallback(async () => {
    try {
      setScreens(await listVideoScreens());
    } catch (error) {
      console.error("Failed to refresh fullscreen monitors", error);
    }
  }, []);

  const refreshTargets = useCallback(() => {
    void refreshStatuses();
    void refreshScreens();
  }, [refreshScreens, refreshStatuses]);

  useEffect(() => {
    refreshTargets();
    const retryTimer = window.setTimeout(() => {
      refreshTargets();
    }, 500);

    let unlistenVisibility: (() => void) | undefined;
    let unlistenStatuses: (() => void) | undefined;
    let unlistenPreferences: (() => void) | undefined;
    const visibility = listen<boolean>("output-window-visible", (event) => setVisible(event.payload));
    const statuses = listen("output-control-status-changed", () => void refreshStatuses());
    const preferences = listen("preferences-updated", () => void refreshTargets());
    void visibility.then((dispose) => { unlistenVisibility = dispose; });
    void statuses.then((dispose) => { unlistenStatuses = dispose; });
    void preferences.then((dispose) => { unlistenPreferences = dispose; });
    return () => {
      window.clearTimeout(retryTimer);
      unlistenVisibility?.();
      unlistenStatuses?.();
      unlistenPreferences?.();
    };
  }, [refreshStatuses, refreshTargets]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const toggleVisibility = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await toggleOutputWindow();
    } catch (error) {
      console.error("Failed to toggle physical output windows", error);
    } finally {
      setBusy(false);
    }
  };

  const chooseMonitor = async (output: OutputControlStatus, monitor: number) => {
    if (busy) return;
    setBusy(true);
    try {
      await setDisplayOutputMonitor(output.output_id, monitor);
      await refreshStatuses();
    } catch (error) {
      console.error("Failed to set display output monitor", error);
      await refreshStatuses();
    } finally {
      setBusy(false);
    }
  };

  const label = visible ? t("fullscreenUi.hide") : t("fullscreenUi.show");

  return (
    <div ref={rootRef} style={{ position: "relative", display: "inline-flex", flexShrink: 0 }}>
      <button
        type="button"
        aria-label={label}
        aria-pressed={visible}
        disabled={busy}
        title={label}
        onClick={() => void toggleVisibility()}
        style={{
          ...buttonStyle,
          borderRadius: "4px 0 0 4px",
          opacity: busy ? 0.65 : 1,
          cursor: busy ? "default" : "pointer",
        }}
      >
        <span aria-hidden="true" style={{ width: 8, height: 8, borderRadius: "50%", background: visible ? "#4ade80" : "var(--wc-text-faint)", boxShadow: visible ? "0 0 5px #4ade80" : "none", flexShrink: 0 }} />
        <span>{t("fullscreenUi.button")}</span>
      </button>
      <button
        type="button"
        aria-label={t("fullscreenUi.menuLabel")}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={busy}
        title={t("fullscreenUi.menuLabel")}
        onClick={() => setOpen((current) => !current)}
        style={{
          ...buttonStyle,
          borderLeft: "1px solid var(--wc-border)",
          borderRadius: "0 4px 4px 0",
          padding: "4px 5px",
          color: open ? "var(--wc-text)" : "var(--wc-text-muted)",
        }}
      >
        {open ? <CaretUp size={12} weight="bold" aria-hidden="true" /> : <CaretDown size={12} weight="bold" aria-hidden="true" />}
      </button>

      {open && (
        <div
          role="menu"
          aria-label={t("fullscreenUi.menuLabel")}
          style={{
            position: "absolute",
            right: 0,
            top: "calc(100% + 8px)",
            width: 320,
            maxHeight: 440,
            overflowY: "auto",
            zIndex: 9999,
            padding: "7px 0",
            background: "var(--wc-bg-surface)",
            border: "1px solid var(--wc-border-strong)",
            borderRadius: 6,
            boxShadow: "0 8px 24px rgba(0,0,0,0.7)",
          }}
        >
          <div style={{ padding: "2px 10px 5px", color: "var(--wc-text-muted)", fontSize: 10, fontWeight: 700, textTransform: "uppercase", letterSpacing: 0.6 }}>
            {t("fullscreenUi.outputs")}
          </div>
          {displayOutputs.length === 0 ? (
            <div style={{ padding: "6px 10px", color: "var(--wc-text-muted)", fontSize: 12 }}>{t("fullscreenUi.noOutputs")}</div>
          ) : (
            displayOutputs.map((output) => {
              return (
                <div key={output.output_id}>
                  <div style={{ display: "flex", alignItems: "center", gap: 7, width: "100%", padding: "6px 10px 4px", color: "var(--wc-text)", fontSize: 12, fontWeight: 600 }}>
                    <Monitor size={14} weight="regular" aria-hidden="true" style={{ color: "var(--wc-text-muted)", flexShrink: 0 }} />
                    <span style={{ minWidth: 0, flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{output.name}</span>
                    <span style={{ color: "var(--wc-text-muted)", fontSize: 10, whiteSpace: "nowrap" }}>{output.monitor == null ? t("fullscreenUi.unassigned") : t("fullscreenUi.monitor", { count: output.monitor + 1 })}</span>
                  </div>
                  <div role="group" aria-label={t("fullscreenUi.selectMonitor", { name: output.name })} style={{ padding: "0 8px 7px 30px" }}>
                      <div style={{ padding: "3px 2px", color: "var(--wc-text-muted)", fontSize: 10, fontWeight: 700, textTransform: "uppercase", letterSpacing: 0.6 }}>{t("fullscreenUi.monitors")}</div>
                      {screens.length === 0 ? (
                        <div style={{ padding: "4px 2px", color: "var(--wc-text-muted)", fontSize: 11 }}>{t("fullscreenUi.noMonitors")}</div>
                      ) : screens.map((screen) => {
                        const monitorSelected = output.monitor === screen.index;
                        const owner = monitorOwner(displayOutputs, screen.index, output.output_id);
                        const conflict = monitorSelected && hasMonitorConflict(displayOutputs, output);
                        const monitorDisabled = busy;
                        return (
                          <button
                            key={screen.index}
                            type="button"
                            role="menuitemradio"
                            aria-checked={monitorSelected}
                            aria-disabled={monitorDisabled}
                            disabled={monitorDisabled}
                            title={owner ? t("fullscreenUi.monitorUsedBy", { name: owner.name }) : undefined}
                            onClick={() => void chooseMonitor(output, screen.index)}
                            style={{ display: "flex", alignItems: "center", gap: 6, width: "100%", padding: "5px 4px", border: conflict ? "1px solid #ef4444" : "1px solid transparent", borderRadius: 3, background: conflict ? "rgba(239,68,68,0.14)" : monitorSelected ? "var(--wc-bg-hover)" : "transparent", color: conflict ? "#fca5a5" : "var(--wc-text-secondary)", cursor: monitorDisabled ? "not-allowed" : "pointer", textAlign: "left", fontSize: 11 }}
                          >
                            <Check size={12} weight="bold" aria-hidden="true" style={{ visibility: monitorSelected ? "visible" : "hidden", flexShrink: 0 }} />
                            <span>{monitorText(screen, t)}{owner ? ` — ${t("fullscreenUi.monitorUsedBy", { name: owner.name })}` : ""}</span>
                          </button>
                        );
                      })}
                    </div>
                </div>
              );
            })
          )}
        </div>
      )}
    </div>
  );
}
