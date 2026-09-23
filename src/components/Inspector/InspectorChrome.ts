import type { CSSProperties } from "react";

/** Shared chrome for both single- and multi-cue inspection. Keeping these
 * primitives in one place prevents the multi editor from becoming a second,
 * visually unrelated inspector again. */
export const inspectorRootStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  height: "100%",
  background: "var(--wc-bg-app)",
  color: "var(--wc-text)",
  fontSize: 13,
};

export const inspectorTitleStyle: CSSProperties = {
  display: "flex",
  alignItems: "baseline",
  gap: 8,
  padding: "8px 12px",
  borderBottom: "1px solid var(--wc-border)",
  background: "var(--wc-bg-deepest)",
};

export const inspectorTabBarStyle: CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  borderBottom: "1px solid var(--wc-border)",
};

export const inspectorContentStyle: CSSProperties = {
  flex: 1,
  overflowY: "auto",
  padding: 12,
};

export function inspectorTabStyle(selected: boolean, disabled = false): CSSProperties {
  return {
    padding: "7px 11px",
    cursor: disabled ? "default" : "pointer",
    fontSize: 12,
    whiteSpace: "nowrap",
    background: selected ? "var(--wc-bg-surface)" : "transparent",
    color: disabled
      ? "var(--wc-text-faint)"
      : selected ? "var(--wc-text)" : "var(--wc-text-muted)",
    fontWeight: selected ? 600 : 400,
    border: "none",
    borderBottom: selected ? "2px solid var(--wc-accent)" : "2px solid transparent",
    outline: "none",
    opacity: disabled ? 0.55 : 1,
  };
}
