export const LOOP_INFINITE = 0xffff_ffff;

/** Media cues store extra repeats: 0 plays once, u32::MAX repeats forever. */
export function loopCountAfterToggle(enabled: boolean): number {
  return enabled ? LOOP_INFINITE : 0;
}

/** Quick control always toggles between infinite and off, even from finite loops. */
export function loopQuickToggleAfterCount(loopCount: number): number {
  return loopCount === LOOP_INFINITE ? 0 : LOOP_INFINITE;
}

/** The infinity control switches between infinite and a finite single repeat. */
export function loopCountAfterInfiniteToggle(loopCount: number): number {
  return loopCount === LOOP_INFINITE ? 1 : LOOP_INFINITE;
}
