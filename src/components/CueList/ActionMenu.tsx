// "Action" menu-bar dropdown — operations that act on the cue list as a whole
// rather than on the current selection (those live in the Edit menu).

import { useState } from "react";

import {
  clearAllCueNumbers,
  movePlayheadToSelection,
  renumberAll,
  renumberSelection,
} from "../../lib/cueOperations";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { MenuBarMenu, type MenuBarItem } from "../MenuBar/MenuBarMenu";
import { RenumberDialog } from "./RenumberDialog";
import { useLocale } from "../../i18n";

export function ActionMenu({ onDone }: { onDone: () => void }) {
  const [renumberOpen, setRenumberOpen] = useState(false);
  const selectionCount = useWorkspaceStore((s) => s.selectedCueIds.length);
  const hasSelection = useWorkspaceStore((s) => s.selectedCueId !== null);
  const playedCueCount = useWorkspaceStore((s) => s.playedCueIds.size);
  const clearPlayedCueHistory = useWorkspaceStore((s) => s.clearPlayedCueHistory);
  const { t } = useLocale();

  const items: MenuBarItem[] = [
    { type: "item", label: `${t("cueList.renumberTitle")} — ${t("cueList.cue")}`, onClick: () => void renumberAll(onDone) },
    { type: "item", label: `${t("cueList.renumberTitle")}…`, disabled: selectionCount === 0,
      onClick: () => setRenumberOpen(true) },
    { type: "item", label: `${t("common.remove")} ${t("cueList.number")}`, onClick: () => void clearAllCueNumbers(onDone) },
    { type: "separator" },
    { type: "item", label: t("cueList.clearPlayed"), disabled: playedCueCount === 0,
      onClick: clearPlayedCueHistory },
    { type: "separator" },
    { type: "item", label: t("transport.playhead"), disabled: !hasSelection,
      onClick: () => void movePlayheadToSelection(onDone) },
  ];

  return (
    <>
      <MenuBarMenu label={t("cueList.actions")} title={t("cueList.actions")} items={items} minWidth={220} />
      {renumberOpen && (
        <RenumberDialog
          cueCount={selectionCount}
          onCancel={() => setRenumberOpen(false)}
          onConfirm={(start, increment) => {
            setRenumberOpen(false);
            void renumberSelection(start, increment, onDone);
          }}
        />
      )}
    </>
  );
}
