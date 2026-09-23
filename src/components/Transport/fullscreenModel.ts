import type { OutputControlStatus, ScreenInfo } from "../../lib/types";

/** The fullscreen control is intentionally limited to native display outputs. */
export function physicalDisplayOutputs(outputs: readonly OutputControlStatus[]): OutputControlStatus[] {
  return outputs.filter((output) => output.active && !output.network);
}

/** Return the enabled physical output that already owns a monitor. */
export function monitorOwner(
  outputs: readonly OutputControlStatus[],
  monitor: number,
  exceptOutputId?: string,
): OutputControlStatus | undefined {
  return outputs.find((output) => output.output_id !== exceptOutputId && output.monitor === monitor);
}

/** True only when an output's current assignment is already duplicated. */
export function hasMonitorConflict(outputs: readonly OutputControlStatus[], output: OutputControlStatus): boolean {
  return !output.network && output.monitor !== null
    ? Boolean(monitorOwner(outputs, output.monitor, output.output_id))
    : false;
}

/** Aggregate LED state for the physical output group. */
export function physicalOutputsVisible(outputs: readonly OutputControlStatus[]): boolean {
  const physical = physicalDisplayOutputs(outputs);
  return physical.some((output) => output.visible);
}

export function screenLabel(screen: ScreenInfo, primaryLabel: string, monitorLabel = `Screen ${screen.index + 1}`): string {
  return `${monitorLabel}${screen.is_primary ? ` (${primaryLabel})` : ""}`;
}
