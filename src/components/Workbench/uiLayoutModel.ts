import { migrateRightPanelMode, type RightPanelMode } from "../ActiveCues/activeCueModel";

export interface UiLayout {
  showCueListTabs: boolean;
  rightPanel: RightPanelMode;
  activeCuesOpen: boolean;
  showSearchBar: boolean;
  inspectorWidth: number;
  showLivePanel: boolean;
  showSlicePanel: boolean;
  activeClipTab: "Live" | "Slice";
}

export const INSPECTOR_MIN_WIDTH = 320;
export const INSPECTOR_MAX_WIDTH = 560;
export const INSPECTOR_DEFAULT_WIDTH = 360;
export const DEFAULT_UI_LAYOUT: UiLayout = {
  showCueListTabs: true,
  rightPanel: "inspector",
  activeCuesOpen: true,
  showSearchBar: true,
  inspectorWidth: INSPECTOR_DEFAULT_WIDTH,
  showLivePanel: true,
  showSlicePanel: true,
  activeClipTab: "Live",
};

export const clampInspectorWidth = (width: number) =>
  Math.min(INSPECTOR_MAX_WIDTH, Math.max(INSPECTOR_MIN_WIDTH, width));

export function normalizeUiLayout(value: unknown): UiLayout {
  const parsed = value && typeof value === "object" ? value as Record<string, unknown> : {};
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
    showSearchBar: typeof parsed.showSearchBar === "boolean" ? parsed.showSearchBar : true,
    inspectorWidth: clampInspectorWidth(typeof parsed.inspectorWidth === "number" ? parsed.inspectorWidth : INSPECTOR_DEFAULT_WIDTH),
    showLivePanel: typeof parsed.showLivePanel === "boolean" ? parsed.showLivePanel : true,
    showSlicePanel: typeof parsed.showSlicePanel === "boolean" ? parsed.showSlicePanel : true,
    activeClipTab: parsed.activeClipTab === "Slice" ? "Slice" : "Live",
  };
}
