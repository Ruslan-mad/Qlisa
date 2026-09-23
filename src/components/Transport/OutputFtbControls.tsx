import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getOutputControlStatuses, listOutputDestinations, toggleOutputFtb } from "../../lib/commands";
import type { OutputControlStatus } from "../../lib/types";
import { useLocale } from "../../i18n";
import { orderOutputStatuses, outputFtbLedColor, outputFtbVisualState } from "./outputFtbModel";

/** Compact per-destination operator FTB controls. Audio is not involved. */
export function OutputFtbControls() {
  const { t } = useLocale();
  const [outputs, setOutputs] = useState<OutputControlStatus[]>([]);

  const refresh = () => {
    void Promise.all([getOutputControlStatuses(), listOutputDestinations()])
      .then(([statuses, destinations]) => setOutputs(orderOutputStatuses(statuses, destinations)))
      .catch(console.error);
  };

  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, 1000);
    let unlisten: (() => void) | undefined;
    void listen("output-control-status-changed", refresh).then((dispose) => {
      unlisten = dispose;
    });
    return () => {
      window.clearInterval(timer);
      unlisten?.();
    };
  }, []);

  if (outputs.length === 0) return null;

  return (
    <div
      aria-label={t("components.outputFtb.panelLabel")}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 5,
        maxWidth: 390,
        overflowX: "auto",
        flexShrink: 1,
        marginLeft: "auto",
      }}
    >
      {outputs.map((output) => {
        const usable = output.available && output.healthy;
        const color = outputFtbLedColor(output);
        const visualState = outputFtbVisualState(output);
        const stateText = visualState === "ok"
          ? t("components.outputFtb.statusOk")
          : visualState === "ftb"
            ? t("components.outputFtb.statusFtb")
            : t("components.outputFtb.statusUnavailable");
        const title = usable
          ? t(output.ftb ? "components.outputFtb.outputEnabled" : "components.outputFtb.outputActive", { name: output.name })
          : t("components.outputFtb.outputUnavailable", { name: output.name, detail: output.detail ?? t("status.outputOffline") });
        return (
          <button
            key={output.output_id}
            type="button"
            disabled={!usable}
            title={title}
            aria-label={`${output.name} ${stateText}`}
            onClick={() => {
              void toggleOutputFtb(output.output_id)
                .then(() => refresh())
                .catch(console.error);
            }}
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: 5,
              minWidth: 0,
              maxWidth: 150,
              padding: "4px 7px",
              borderRadius: 4,
              border: `1px solid ${output.ftb ? "#ef4444" : "var(--wc-border-strong)"}`,
              background: output.ftb ? "#3b1114" : "var(--wc-bg-surface)",
              color: usable ? "var(--wc-text-secondary)" : "var(--wc-text-faint)",
              cursor: usable ? "pointer" : "not-allowed",
              opacity: usable ? 1 : 0.75,
              whiteSpace: "nowrap",
              overflow: "hidden",
            }}
          >
            <span
              aria-hidden="true"
              style={{
                width: 8,
                height: 8,
                borderRadius: "50%",
                flexShrink: 0,
                background: color,
                boxShadow: `0 0 5px ${color}`,
              }}
            />
            <span style={{ overflow: "hidden", textOverflow: "ellipsis", fontSize: 10, fontWeight: 700 }}>
              {output.name}
            </span>
            <span style={{ fontSize: 9, fontWeight: 800, color, flexShrink: 0 }}>{stateText}</span>
          </button>
        );
      })}
    </div>
  );
}
