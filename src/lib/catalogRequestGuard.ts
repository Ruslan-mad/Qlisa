/** Keeps overlapping read-only catalog snapshots from applying out of order. */
export function createCatalogRequestGuard() {
  let generation = 0;
  return {
    begin: () => ++generation,
    invalidate: () => { generation += 1; },
    isCurrent: (requestGeneration: number) => requestGeneration === generation,
  };
}

export function commitIfCurrent<T>(guard: ReturnType<typeof createCatalogRequestGuard>, requestGeneration: number, value: T, commit: (value: T) => void): boolean {
  if (!guard.isCurrent(requestGeneration)) return false;
  commit(value);
  return true;
}
