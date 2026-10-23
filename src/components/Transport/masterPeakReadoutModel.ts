export interface MasterPeakReadout {
  left: number | null;
  right: number | null;
  overloadLeft: boolean;
  overloadRight: boolean;
}

export const EMPTY_MASTER_PEAK_READOUT: MasterPeakReadout = {
  left: null,
  right: null,
  overloadLeft: false,
  overloadRight: false,
};

export function observeMasterPeaks(
  current: MasterPeakReadout,
  peakL: number,
  peakR: number,
): MasterPeakReadout {
  return {
    left: peakL > 0 ? Math.max(current.left ?? 0, peakL) : current.left,
    right: peakR > 0 ? Math.max(current.right ?? 0, peakR) : current.right,
    overloadLeft: current.overloadLeft || peakL >= 1,
    overloadRight: current.overloadRight || peakR >= 1,
  };
}

export function resetMasterPeakChannel(
  current: MasterPeakReadout,
  channel: "left" | "right",
): MasterPeakReadout {
  return channel === "left"
    ? { ...current, left: null, overloadLeft: false }
    : { ...current, right: null, overloadRight: false };
}

export function formatMasterPeakDbfs(peak: number | null): string {
  if (peak === null || peak <= 0) return "−∞ dBFS";
  const db = 20 * Math.log10(peak);
  return `${db > 0 ? "+" : ""}${db.toFixed(1)} dBFS`;
}
