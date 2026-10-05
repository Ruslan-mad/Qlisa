import { useEffect, useMemo, useRef, useState } from "react";
import { Pulse as Activity, CaretDown, SquaresFour } from "@phosphor-icons/react";
import { useLocale } from "../../i18n";
import { COMMAND_CUE_TYPES, CUE_TYPE_COLORS, type CueType } from "../../lib/types";
import { CueTypeIcon } from "../common/CueTypeIcon";
import { CUE_TOOLBAR_DESCRIPTORS, resolveCueToolbarLayout } from "./cueToolbarModel";

type Props = {
  activeCueCount: number;
  rightPanel: string;
  activeCuesOpen?: boolean;
  onToggleRightPanel: (panel: "active-cues" | "inspector") => void;
  onSettings: () => void;
  onAdd: (type: CueType) => void;
  onDragStart: (type: CueType, event: React.MouseEvent) => void;
};

const buttonStyle: React.CSSProperties = {
  padding: "3px 10px", background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
  borderRadius: 4, color: "var(--wc-text)", cursor: "pointer", fontSize: 12,
};

export function CueToolbar({ activeCueCount, rightPanel, activeCuesOpen, onToggleRightPanel, onAdd, onDragStart }: Props) {
  const { t, locale } = useLocale();
  const leftRef = useRef<HTMLDivElement>(null);
  const measureRefs = useRef(new Map<CueType, HTMLButtonElement>());
  const moreMeasureRef = useRef<HTMLButtonElement>(null);
  const [availableWidth, setAvailableWidth] = useState(0);
  const [dimensions, setDimensions] = useState<{ widths: Partial<Record<CueType, number>>; more: number }>({ widths: {}, more: 60 });
  const [open, setOpen] = useState(false);
  const [hoveredItem, setHoveredItem] = useState<string | null>(null);
  const [hoveredMain, setHoveredMain] = useState<CueType | null>(null);
  const [hoveredMore, setHoveredMore] = useState(false);
  const descriptors = CUE_TOOLBAR_DESCRIPTORS;
  const layout = useMemo(() => resolveCueToolbarLayout(availableWidth, dimensions.widths, dimensions.more), [availableWidth, dimensions]);

  useEffect(() => {
    const container = leftRef.current;
    if (!container) return;
    const update = () => setAvailableWidth(container.clientWidth);
    update();
    const observer = new ResizeObserver(update);
    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const widths: Partial<Record<CueType, number>> = {};
    for (const [type, element] of measureRefs.current) widths[type] = element.getBoundingClientRect().width;
    const more = moreMeasureRef.current?.getBoundingClientRect().width ?? 60;
    setDimensions((old) => {
      const same = old.more === more && descriptors.every(({ type }) => old.widths[type] === widths[type]);
      return same ? old : { widths, more };
    });
  // `t` is a stable function. Re-measure after locale changes because translated
  // button labels can have different widths even though the translator identity stays fixed.
  }, [descriptors, locale]);

  const label = (type: CueType) => t(descriptors.find((item) => item.type === type)?.labelKey ?? `cueTypes.${type}`);
  const hint = (type: CueType) => {
    const item = descriptors.find((descriptor) => descriptor.type === type);
    if (item?.hintKey) return t(item.hintKey);
    if (COMMAND_CUE_TYPES.some((command) => command.type === type)) return t(`toolbar.commandHint${type[0].toUpperCase()}${type.slice(1)}`);
    return t("toolbar.addCueAfterSelection", { cue: label(type) });
  };
  const mainButton = (type: CueType, measuring = false) => (
    <button
      key={type}
      ref={measuring ? (element) => { if (element) measureRefs.current.set(type, element); else measureRefs.current.delete(type); } : undefined}
      style={{
        ...buttonStyle, width: "max-content", minWidth: 72, minHeight: 62, padding: "6px 10px", borderRadius: 6, display: "inline-flex", flexDirection: "column", justifyContent: "center", alignItems: "center", gap: 3, flexShrink: 0, userSelect: "none", fontSize: 13, fontWeight: 600,
        color: hoveredMain === type ? CUE_TYPE_COLORS[type] : "var(--wc-text-secondary)",
        borderColor: hoveredMain === type ? CUE_TYPE_COLORS[type] : "var(--wc-border-strong)",
        background: hoveredMain === type ? "var(--wc-bg-hover)" : "var(--wc-bg-surface)",
        transition: "color 0.12s, border-color 0.12s, background 0.12s",
      }}
      onClick={measuring ? undefined : () => onAdd(type)}
      onMouseDown={measuring ? undefined : (event) => onDragStart(type, event)}
      onMouseEnter={measuring ? undefined : () => setHoveredMain(type)}
      onMouseLeave={measuring ? undefined : () => setHoveredMain(null)}
      title={hint(type)}
      tabIndex={measuring ? -1 : undefined}
      aria-hidden={measuring || undefined}
    >
      <CueTypeIcon type={type} size={24} tone="type" />{label(type)}
    </button>
  );

  const menuItems = layout.overflow.map((type) => {
    const item = descriptors.find((descriptor) => descriptor.type === type)!;
    return (
      <button key={type} role="menuitem" onMouseEnter={() => setHoveredItem(type)} onMouseLeave={() => setHoveredItem(null)}
        onClick={(event) => { event.stopPropagation(); setOpen(false); onAdd(type); }}
        onMouseDown={(event) => { if (event.button === 0) onDragStart(type, event); }}
        style={{ display: "flex", alignItems: "center", gap: 10, width: "100%", height: 26, padding: "0 10px", borderRadius: 4, border: "none", textAlign: "left", background: hoveredItem === type ? "var(--wc-bg-hover)" : "transparent", cursor: "pointer", whiteSpace: "nowrap" }}>
        <CueTypeIcon type={type} size={16} tone="type" />
        <span style={{ color: CUE_TYPE_COLORS[type], fontSize: 12, fontWeight: 600, width: 82, flexShrink: 0 }}>{t(item.labelKey)}</span>
        <span style={{ color: "var(--wc-text-muted)", fontSize: 11 }}>{hint(type)}</span>
      </button>
    );
  });

  return (
    <div className="stage-toolbar-row" style={{ display: "flex", flexWrap: "nowrap", gap: 8, padding: "0 12px 8px", alignItems: "center", minWidth: 0 }} onClick={(event) => {
      const button = (event.target as HTMLElement).closest("button");
      button?.animate([
        { transform: "scale(1)", filter: "brightness(1)" },
        { transform: "scale(0.88)", filter: "brightness(1.7)", offset: 0.35 },
        { transform: "scale(1)", filter: "brightness(1)" },
      ], { duration: 260, easing: "cubic-bezier(.2,.7,.3,1)" });
    }}>
      <div ref={leftRef} className="stage-cue-actions" style={{ display: "flex", flex: "1 1 0", minWidth: 0, flexWrap: "nowrap", alignItems: "center", gap: 8, overflow: "visible" }}>
        {layout.visible.map((type) => mainButton(type))}
        {layout.showMore && <div style={{ position: "relative", flexShrink: 0, zIndex: open ? 10002 : undefined }}>
          {open && <div style={{ position: "fixed", inset: 0, zIndex: 10000 }} onClick={() => setOpen(false)} />}
          <button aria-haspopup="menu" aria-expanded={open} style={{ ...buttonStyle, position: "relative", width: "max-content", minWidth: 72, minHeight: 62, padding: "6px 10px", borderRadius: 6, display: "inline-flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: 3, flexShrink: 0, userSelect: "none", fontSize: 13, fontWeight: 600, color: hoveredMore || open ? "var(--wc-text)" : "var(--wc-text-secondary)", borderColor: hoveredMore || open ? "var(--wc-text-secondary)" : "var(--wc-border-strong)", background: hoveredMore || open ? "var(--wc-bg-hover)" : "var(--wc-bg-surface)", transition: "color 0.12s, border-color 0.12s, background 0.12s" }}
            onClick={(event) => { event.stopPropagation(); setOpen((value) => !value); }} onMouseEnter={() => setHoveredMore(true)} onMouseLeave={() => setHoveredMore(false)} title={t("toolbar.otherTitle")}>
            <SquaresFour size={24} weight="bold" aria-hidden="true" /><span>{t("toolbar.other")}</span><CaretDown size={11} weight="bold" aria-hidden="true" style={{ position: "absolute", right: 5, top: 5 }} />
          </button>
          {open && <div role="menu" aria-label={t("toolbar.otherTitle")} style={{ position: "absolute", left: 0, top: "100%", marginTop: 4, zIndex: 10001, background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", borderRadius: 6, boxShadow: "0 8px 24px rgba(0,0,0,0.6)", padding: 4, minWidth: 262, maxHeight: "70vh", overflowY: "auto" }}>{menuItems}</div>}
        </div>}
        <div aria-hidden="true" style={{ position: "absolute", visibility: "hidden", pointerEvents: "none", whiteSpace: "nowrap", display: "flex", gap: 6, left: -10000, top: 0 }}>
          {descriptors.filter((item) => item.primary).map(({ type }) => mainButton(type, true))}
          <button ref={moreMeasureRef} style={{ ...buttonStyle, width: "max-content", minWidth: 72, minHeight: 62, padding: "6px 10px", display: "inline-flex", flexDirection: "column", justifyContent: "center", alignItems: "center", gap: 3, fontSize: 13 }}><SquaresFour size={24} />{t("toolbar.other")}</button>
        </div>
      </div>
      <div className="stage-toolbar-tools" style={{ display: "flex", alignItems: "center", gap: 8, flex: "0 0 auto", marginLeft: "auto", whiteSpace: "nowrap" }}>
        <button className="stage-action-button" style={{ ...buttonStyle, minHeight: 60, padding: "7px 12px", borderRadius: 6, flexShrink: 0, fontSize: 12, fontWeight: 600 }} onClick={() => onToggleRightPanel("active-cues")} title={t("toolbar.activeCuesToggle")} aria-pressed={activeCuesOpen ?? rightPanel === "active-cues"}>
          <Activity size={17} weight="bold" style={{ verticalAlign: "-3px", marginRight: 7 }} />{t("menus.activeCues")}
          {activeCueCount > 0 && <span style={{ marginLeft: 6, padding: "1px 5px", borderRadius: 8, background: "var(--wc-bg-hover)", color: "var(--wc-text-secondary)", fontSize: 10 }}>{activeCueCount}</span>}
        </button>
        <button className="stage-action-button" style={{ ...buttonStyle, minHeight: 60, padding: "7px 12px", borderRadius: 6, flexShrink: 0, fontSize: 12, fontWeight: 600 }} onClick={() => onToggleRightPanel("inspector")} title={t("toolbar.inspectorToggle")} aria-pressed={rightPanel === "inspector"}>{t("menus.inspector")}</button>
      </div>
    </div>
  );
}
