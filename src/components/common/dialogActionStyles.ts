import type { CSSProperties } from "react";

/** Shared sizing for actions in app-owned modal dialogs. */
export const dialogActionButtonStyle: CSSProperties = {
  boxSizing: "border-box",
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  flexShrink: 0,
  minHeight: 32,
  padding: "0 12px",
  lineHeight: 1.2,
  whiteSpace: "nowrap",
};

export const dialogActionFooterStyle: CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  alignItems: "center",
  justifyContent: "flex-end",
  gap: 8,
};
