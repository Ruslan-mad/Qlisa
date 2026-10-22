import { useTimingStore } from "../stores/timingStore";
import { useWorkspaceStore } from "../stores/workspaceStore";

/** Reconcile a successful replace command when its event may have been missed. */
export async function reconcileWorkspaceAfterReplacement(startingRevision: number): Promise<void> {
  const store = useWorkspaceStore.getState();
  if (store.workspaceReplacementRevision === startingRevision) {
    store.resetWorkspaceView();
    useTimingStore.getState().resetWorkspaceTiming();
  }
  // Refresh the authoritative snapshot first. The store may advance its
  // context generation while applying it, so dependent reads must start after.
  await store.refreshCues();
  await Promise.all([store.refreshWorkspaceInfo(), store.refreshValidation()]);
}
