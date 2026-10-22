import type { TransportPanelVisibility } from "../Workbench/uiLayoutModel";
import { useLocale } from "../../i18n";

export type TransportPanelName = keyof TransportPanelVisibility;

export function TransportPanelControls({
  visibility,
  onToggle,
}: {
  visibility: TransportPanelVisibility;
  onToggle: (panel: TransportPanelName) => void;
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
      <span className="transport-panel-map__center" aria-hidden="true">
        <i /><i /><i /><i />
      </span>
    </div>
  );
}
