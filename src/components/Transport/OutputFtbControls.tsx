import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getOutputControlStatuses, listOutputDestinations, toggleOutputFtb } from "../../lib/commands";
import type { OutputControlStatus } from "../../lib/types";
import { useLocale } from "../../i18n";
import { orderOutputStatuses, outputControlsNeedTwoRows, outputFtbLedColor, outputFtbVisualState } from "./outputFtbModel";
import { animateButtonPress } from "../common/buttonPressAnimation";

/** Compact per-destination operator FTB controls. Audio is not involved. */
export function OutputFtbControls() {
  const { t } = useLocale();
  const [outputs, setOutputs] = useState<OutputControlStatus[]>([]);
  const controlsRef = useRef<HTMLDivElement>(null);
  const [twoRows, setTwoRows] = useState(false);

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

  useLayoutEffect(() => {
    const container = controlsRef.current;
    if (!container) return;
    const measure = () => {
      const chips = Array.from(container.querySelectorAll<HTMLElement>(".stage-output-chip"));
      const gap = Number.parseFloat(getComputedStyle(container).columnGap) || 0;
      setTwoRows(outputControlsNeedTwoRows(chips.map((chip) => chip.offsetWidth), container.clientWidth, gap));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(container);
    container.querySelectorAll(".stage-output-chip").forEach((chip) => observer.observe(chip));
    return () => observer.disconnect();
  }, [outputs.length]);

  if (outputs.length === 0) return null;

  return (
    <div
      className="stage-output-controls"
      data-two-rows={twoRows ? "true" : "false"}
      ref={controlsRef}
      aria-label={t("components.outputFtb.panelLabel")}
      style={{
        display: "grid",
        alignItems: "center",
        gap: 5,
        maxWidth: "100%",
        minWidth: 0,
        overflowX: "auto",
        flexShrink: 0,
        marginLeft: 0,
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
            className="stage-output-chip"
            key={output.output_id}
            type="button"
            disabled={!usable}
            onClickCapture={(event) => animateButtonPress(event.currentTarget)}
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
              gap: 7,
              minWidth: 0,
              maxWidth: 180,
              padding: "7px 10px",
              borderRadius: 6,
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
                width: 9,
                height: 9,
                borderRadius: "50%",
                flexShrink: 0,
                background: color,
                boxShadow: `0 0 5px ${color}`,
              }}
            />
            <span style={{ overflow: "hidden", textOverflow: "ellipsis", fontSize: 11, fontWeight: 700 }}>
              {output.name}
            </span>
            <span style={{ fontSize: 10, fontWeight: 800, color, flexShrink: 0 }}>{stateText}</span>
          </button>
        );
      })}
    </div>
  );
}
