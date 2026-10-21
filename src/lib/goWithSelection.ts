import { go } from "./commands";
import { useWorkspaceStore } from "../stores/workspaceStore";

/** Run GO and follow its resulting Playhead unless the operator changed selection meanwhile. */
export async function goAndFollowSelection() {
  const expectedRevision = useWorkspaceStore.getState().cueSelectionRevision;
  const result = await go();
  if (result?.advanced) {
    useWorkspaceStore.getState().applyGoSelection(
      result.cue_list_id,
      result.cue_id,
      expectedRevision,
    );
  }
  return result;
}
