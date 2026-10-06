// Searchable, cross-list target picker. Catalog tabs are local UI state and
// never change the workspace's active cue list.
import { useMemo, useState } from "react";
import type { CueType, WorkspaceCueCatalogList } from "../../lib/types";
import { flattenCueCatalog } from "./targetCueModel";
export { flattenCueCatalog } from "./targetCueModel";
import { inputStyle } from "./Field";
import { useLocale } from "../../i18n";

const listStyle: React.CSSProperties = { maxHeight: 180, overflowY: "auto", border: "1px solid var(--wc-border-strong)", borderRadius: 4, padding: "2px 0", background: "var(--wc-bg-surface)" };
const chipStyle: React.CSSProperties = { display: "inline-flex", alignItems: "center", gap: 4, padding: "1px 4px 1px 7px", borderRadius: 10, background: "var(--wc-accent)", color: "var(--wc-accent-fg)", fontSize: 11, maxWidth: "100%" };
type CatalogCue = ReturnType<typeof flattenCueCatalog>[number];

export function CueTargetPicker({
  lists, selfId, selectedIds, onChange, filterTypes,
}: {
  lists: WorkspaceCueCatalogList[];
  selfId: string;
  selectedIds: string[];
  onChange: (ids: string[]) => void;
  filterTypes?: CueType[];
}) {
  const { t } = useLocale();
  const [query, setQuery] = useState("");
  const [activeListId, setActiveListId] = useState<string | null>(null);
  const flat = useMemo(() => flattenCueCatalog(lists), [lists]);
  const candidates = useMemo(() => flat.filter((cue) => cue.id !== selfId && (!filterTypes || filterTypes.includes(cue.cue_type))), [flat, selfId, filterTypes]);
  const byId = useMemo(() => new Map(flat.map((cue) => [cue.id, cue])), [flat]);
  const ownerListId = byId.get(selfId)?.listId;
  const activeList = lists.find((list) => list.id === activeListId)
    ?? lists.find((list) => list.id === ownerListId)
    ?? lists[0];
  const selectedCount = (listId: string) => selectedIds.filter((id) => byId.get(id)?.listId === listId).length;
  const label = (cue: CatalogCue) => `${cue.listName} · ${cue.number ? `#${cue.number} — ` : ""}${cue.name || t("uiFixes.untitled")}`;
  const q = query.trim().toLowerCase();
  const visible = candidates.filter((cue) => cue.listId === activeList?.id && (!q || label(cue).toLowerCase().includes(q)));
  const selectedCues = selectedIds.map((id) => byId.get(id));
  const toggle = (id: string, on: boolean) => onChange(on ? [...selectedIds, id] : selectedIds.filter((x) => x !== id));

  return <div style={{ width: "100%" }}>
    {selectedIds.length > 0 && <div style={{ display: "flex", flexWrap: "wrap", gap: 4, marginBottom: 6 }}>
      {selectedIds.map((id, index) => {
        const cue = selectedCues[index];
        const text = cue ? label(cue) : `${t("components.missingTarget")} · ${id}`;
        return <span key={id} style={{ ...chipStyle, opacity: cue ? 1 : 0.72 }} title={text}>
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", maxWidth: 220 }}>{text}</span>
          <button type="button" onClick={() => toggle(id, false)} style={{ background: "none", border: "none", color: "inherit", cursor: "pointer", fontSize: 13, lineHeight: 1, padding: "0 2px" }} title={t("components.remove")}>×</button>
        </span>;
      })}
      <button type="button" onClick={() => onChange([])} style={{ background: "none", border: "none", color: "var(--wc-text-muted)", cursor: "pointer", fontSize: 11 }}>{t("components.clearAll")}</button>
    </div>}

    {lists.length > 1 && <div role="tablist" aria-label={t("components.targetCueLists")} style={{ display: "flex", gap: 4, overflowX: "auto", marginBottom: 5 }}>
      {lists.map((list) => {
        const active = list.id === activeList?.id;
        const count = selectedCount(list.id);
        return <button type="button" role="tab" aria-selected={active} key={list.id} onClick={() => setActiveListId(list.id)} style={{ flexShrink: 0, padding: "3px 8px", border: `1px solid ${active ? "var(--wc-accent)" : "var(--wc-border-strong)"}`, borderRadius: 4, background: active ? "var(--wc-bg-hover)" : "var(--wc-bg-surface)", color: active ? "var(--wc-text-bright)" : "var(--wc-text-secondary)", cursor: "pointer", fontSize: 11 }}>
          {list.name}{count > 0 && <span style={{ marginLeft: 5, opacity: 0.8 }}>{count}</span>}
        </button>;
      })}
    </div>}

    {candidates.some((cue) => cue.listId === activeList?.id) && <input type="text" value={query} onChange={(e) => setQuery(e.target.value)} placeholder={`${t("common.search")} ${candidates.filter((cue) => cue.listId === activeList?.id).length} ${t("cueList.cue").toLowerCase()}…`} style={{ ...inputStyle, marginBottom: 4, fontSize: 12 }} />}
    <div style={listStyle}>
      {visible.length === 0 ? <div style={{ padding: "4px 8px", fontSize: 12, color: "var(--wc-text-muted)" }}>{q ? t("components.noMatch") : t("components.noEligibleCues")}</div> : visible.map((cue) => {
        const checked = selectedIds.includes(cue.id);
        return <label key={cue.id} style={{ display: "flex", alignItems: "center", gap: 6, padding: "3px 8px", cursor: "pointer", background: checked ? "var(--wc-bg-hover)" : undefined }}>
          <input type="checkbox" checked={checked} onChange={(e) => toggle(cue.id, e.target.checked)} />
          <span style={{ fontSize: 12, color: "var(--wc-text)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{cue.number ? `#${cue.number} — ` : ""}{cue.name || t("uiFixes.untitled")}</span>
        </label>;
      })}
    </div>
    <div style={{ marginTop: 4, fontSize: 10, color: "var(--wc-text-muted)" }}>{t("components.selectedTargetCount", { count: selectedIds.length })}</div>
  </div>;
}
