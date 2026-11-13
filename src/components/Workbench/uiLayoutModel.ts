import { migrateRightPanelMode, type RightPanelMode } from "../ActiveCues/activeCueModel";

export interface UiLayout {
  showCueListTabs: boolean;
  rightPanel: RightPanelMode;
  activeCuesOpen: boolean;
  showCueToolbar: boolean;
  showClipEditorDock: boolean;
  showSearchBar: boolean;
  inspectorWidth: number;
  showLivePanel: boolean;
  showSlicePanel: boolean;
  activeClipTab: "Preview" | "Live";
  clipEditorTabVersion: 2;
}

export const INSPECTOR_MIN_WIDTH = 320;
export const INSPECTOR_MAX_WIDTH = 560;
export const INSPECTOR_DEFAULT_WIDTH = 360;
export const DEFAULT_UI_LAYOUT: UiLayout = {
  showCueListTabs: true,
  rightPanel: "inspector",
  activeCuesOpen: true,
  showCueToolbar: true,
  showClipEditorDock: true,
  showSearchBar: true,
  inspectorWidth: INSPECTOR_DEFAULT_WIDTH,
  showLivePanel: true,
  showSlicePanel: false,
  activeClipTab: "Preview",
  clipEditorTabVersion: 2,
};

export const clampInspectorWidth = (width: number) =>
  Math.min(INSPECTOR_MAX_WIDTH, Math.max(INSPECTOR_MIN_WIDTH, width));

export function normalizeUiLayout(value: unknown): UiLayout {
  const parsed = value && typeof value === "object" ? value as Record<string, unknown> : {};
  const legacySliceVisible = typeof parsed.showSlicePanel === "boolean" ? parsed.showSlicePanel : true;
  const legacySliceTab = parsed.activeClipTab === "Slice";
  const showLivePanel = (typeof parsed.showLivePanel === "boolean" ? parsed.showLivePanel : true)
    || legacySliceVisible || legacySliceTab;
  const legacyRightPanel = migrateRightPanelMode(parsed.rightPanel, parsed.inspectorOpen as boolean | undefined);
  const inspectorOpen = typeof parsed.inspectorOpen === "boolean"
    ? parsed.inspectorOpen
    : legacyRightPanel === "inspector";
  const activeCuesOpen = typeof parsed.activeCuesOpen === "boolean"
    ? parsed.activeCuesOpen
    : legacyRightPanel !== "closed";
  return {
    showCueListTabs: typeof parsed.showCueListTabs === "boolean" ? parsed.showCueListTabs : true,
    rightPanel: inspectorOpen ? "inspector" : "closed",
    activeCuesOpen,
    showCueToolbar: typeof parsed.showCueToolbar === "boolean" ? parsed.showCueToolbar : true,
    showClipEditorDock: typeof parsed.showClipEditorDock === "boolean" ? parsed.showClipEditorDock : true,
    showSearchBar: typeof parsed.showSearchBar === "boolean" ? parsed.showSearchBar : true,
    inspectorWidth: clampInspectorWidth(typeof parsed.inspectorWidth === "number" ? parsed.inspectorWidth : INSPECTOR_DEFAULT_WIDTH),
    showLivePanel,
    // Kept in the serialized layout only so old layouts can be migrated.
    showSlicePanel: false,
    activeClipTab: parsed.clipEditorTabVersion === 2 && parsed.activeClipTab === "Live"
      ? "Live"
      : "Preview",
    clipEditorTabVersion: 2,
  };
}

export interface TransportPanelVisibility {
  toolbar: boolean;
  activeCues: boolean;
  inspector: boolean;
  clipEditorDock: boolean;
}

/** Resolve the panel map from actual workbench visibility, including Show Mode. */
export function resolveTransportPanelVisibility(layout: Pick<UiLayout,
  "showCueToolbar" | "activeCuesOpen" | "rightPanel" | "showClipEditorDock" | "showLivePanel" | "showSlicePanel"
> & { showMode: boolean }): TransportPanelVisibility {
  return {
    toolbar: !layout.showMode && layout.showCueToolbar,
    activeCues: !layout.showMode && layout.activeCuesOpen,
    inspector: !layout.showMode && layout.rightPanel === "inspector",
    clipEditorDock: !layout.showMode && layout.showClipEditorDock
      && layout.showLivePanel,
  };
}
