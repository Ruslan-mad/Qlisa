/** Old payloads without an owner belong to the active list for compatibility. */
export function shouldApplyPlayheadMovedEvent(eventListId: string | undefined, activeListId: string | null): boolean {
  return eventListId == null || eventListId === activeListId;
}
