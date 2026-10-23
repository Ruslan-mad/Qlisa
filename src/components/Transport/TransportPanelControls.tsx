import type { TransportPanelVisibility } from "../Workbench/uiLayoutModel";
import { useLocale } from "../../i18n";

export type TransportPanelName = keyof TransportPanelVisibility;

export function TransportPanelControls({
  visibility,
  onToggle,
  outputMonitorVisible,
  onToggleOutputMonitor,
}: {
  visibility: TransportPanelVisibility;
  onToggle: (panel: TransportPanelName) => void;
  outputMonitorVisible: boolean;
  onToggleOutputMonitor: () => void;
}) {
  const { t } = useLocale();
  const panels: { name: TransportPanelName; label: string; position: string }[] = [
    { name: "toolbar", label: "transport.panelToolbar", position: "top" },
    { name: "activeCues", label: "transport.panelActiveCues", position: "left" },
    { name: "inspector", label: "transport.panelInspector", position: "right" },
    { name: "clipEditorDock", label: "transport.panelClipEditor", position: "bottom" },
  ];

  return (
    <div className="transport-panel-map" role="group" aria-label={t("transport.panelMapLabel")}>
      {panels.map(({ name, label, position }) => (
        <button
          key={name}
          className={`transport-panel-map__button transport-panel-map__button--${position}`}
          data-active={visibility[name] ? "true" : "false"}
          type="button"
          title={t(label)}
          aria-label={t(label)}
          aria-pressed={visibility[name]}
          onClick={() => onToggle(name)}
        />
      ))}
      <button
        className="transport-panel-map__center"
        type="button"
        title={t("transport.panelOutputMonitor")}
        aria-label={t("transport.panelOutputMonitor")}
        aria-pressed={outputMonitorVisible}
        data-active={outputMonitorVisible ? "true" : "false"}
        onClick={onToggleOutputMonitor}
      >
        <i /><i /><i /><i />
      </button>
    </div>
  );
}
