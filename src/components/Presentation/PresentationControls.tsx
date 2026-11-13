import { useState } from "react";
import { CaretLeft, CaretRight } from "@phosphor-icons/react";
import { stepPresentation } from "../../lib/commands";
import { useLocale } from "../../i18n";
import { presentationChildren, presentationTarget, type PresentationNode } from "./presentationNavigation";

export function PresentationControls({ owner, mode, currentChildId, onStep, onChanged }: {
  owner: PresentationNode;
  mode: "inspector" | "preview" | "live";
  currentChildId?: string | null;
  onStep?: (direction: "previous" | "next") => void;
  onChanged?: () => void;
}) {
  const { t } = useLocale();
  const [error, setError] = useState<{ ownerId: string; message: string } | null>(null);
  const preview = mode === "preview";
  const children = preview ? presentationChildren(owner, true, true) : (owner.children ?? []);
  const activeId = preview ? currentChildId : owner.presentation_current_child_id;
  const index = children.findIndex((child) => child.id === activeId);
  const previousTarget = preview ? children[index - 1] ?? null : presentationTarget(owner, "previous");
  const nextTarget = preview ? children[index + 1] ?? null : presentationTarget(owner, "next");
  const started = owner.state === "running" || owner.state === "paused";
  const title = mode === "preview" ? t("presentationUi.previewContext") : t("presentationUi.programContext");
  const step = async (direction: "previous" | "next") => {
    setError(null);
    const ownerId = owner.id;
    try {
      if (preview) onStep?.(direction);
      else {
        await stepPresentation(owner.id, direction);
        onChanged?.();
      }
    } catch (cause) {
      const message = String(cause);
      setError({ ownerId, message });
      window.dispatchEvent(new CustomEvent("inkue:transport-error", { detail: message }));
    }
  };

  if (mode !== "preview" && owner.cue_type !== "presentation") return null;
  return (
    <div style={{ padding: "8px 10px", borderBottom: "1px solid var(--wc-border)", background: "var(--wc-bg-surface)", display: "flex", alignItems: "center", gap: 7, flexWrap: "wrap" }}>
      <span style={{ color: "var(--wc-text-muted)", fontSize: 11, fontWeight: 600, marginRight: 2 }}>{title}</span>
      <button className="presentation-nav-button" type="button" aria-label={t("presentationUi.previous")} title={t("presentationUi.previous")} disabled={!previousTarget || (!preview && !started)} onClick={() => void step("previous")} style={buttonStyle(!previousTarget || (!preview && !started))}>
        <CaretLeft size={15} />{t("presentationUi.previous")}
      </button>
      <button className="presentation-nav-button" type="button" aria-label={t("presentationUi.next")} title={t("presentationUi.next")} disabled={!nextTarget || (!preview && !started)} onClick={() => void step("next")} style={buttonStyle(!nextTarget || (!preview && !started))}>
        <CaretRight size={15} />{t("presentationUi.next")}
      </button>
      <span style={{ color: "var(--wc-text-muted)", fontSize: 11, minWidth: 46 }}>{index >= 0 ? `${index + 1} / ${children.length}` : `— / ${children.length}`}</span>
      {index >= 0 && <span style={{ color: "var(--wc-text)", fontSize: 11, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0 }}>{children[index].name}</span>}
      {!preview && !started && <span style={{ color: "var(--wc-text-muted)", fontSize: 11 }}>{t("presentationUi.goToStart")}</span>}
      {error?.ownerId === owner.id && <span role="alert" style={{ width: "100%", color: "#fca5a5", fontSize: 11 }}>{error.message}</span>}
    </div>
  );
}

function buttonStyle(disabled: boolean): React.CSSProperties {
  return { display: "inline-flex", alignItems: "center", gap: 4, minHeight: 28, padding: "4px 8px", border: "1px solid var(--wc-border-strong)", borderRadius: 5, color: disabled ? "var(--wc-text-faint)" : "var(--wc-text)", background: "var(--wc-bg)", fontSize: 11, cursor: disabled ? "default" : "pointer", opacity: disabled ? 0.65 : 1 };
}
