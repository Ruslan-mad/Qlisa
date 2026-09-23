import type { CueType } from "../../lib/types";

const DURATION_EDITABLE_TYPES = new Set<CueType>(["wait", "fade", "image", "text", "light"]);

export type InlineTimeParseResult =
  | { valid: true; valueMs: number | null }
  | { valid: false };

export type InlineTimeCommitDecision =
  | { valid: true; shouldCommit: boolean; valueMs: number | null }
  | { valid: false };

export function canEditCueDuration(cueType: CueType): boolean {
  return DURATION_EDITABLE_TYPES.has(cueType);
}

export function emptyCueDurationValue(cueType: CueType): number | null {
  return cueType === "image" || cueType === "text" ? null : 0;
}

const MAX_SAFE_MILLISECONDS = BigInt(Number.MAX_SAFE_INTEGER);

function roundedFractionMilliseconds(fraction: string | undefined): bigint {
  if (!fraction) return 0n;
  const milliseconds = BigInt(fraction.slice(0, 3).padEnd(3, "0"));
  return milliseconds + (fraction.length > 3 && fraction[3] >= "5" ? 1n : 0n);
}

function parsedMilliseconds(value: bigint): InlineTimeParseResult {
  return value <= MAX_SAFE_MILLISECONDS
    ? { valid: true, valueMs: Number(value) }
    : { valid: false };
}

export function parseInlineTimeInput(raw: string, emptyValueMs: number | null): InlineTimeParseResult {
  const input = raw.trim();
  if (input === "") return { valid: true, valueMs: emptyValueMs };

  const colon = /^(\d+):([0-5]?\d)(?:\.(\d+))?$/.exec(input);
  if (colon) {
    const valueMs = BigInt(colon[1]) * 60_000n
      + BigInt(colon[2]) * 1_000n
      + roundedFractionMilliseconds(colon[3]);
    return parsedMilliseconds(valueMs);
  }

  const seconds = /^(\d*)(?:\.(\d*))?$/.exec(input);
  if (!seconds || (seconds[1] === "" && (seconds[2] ?? "") === "")) return { valid: false };
  const valueMs = BigInt(seconds[1] || "0") * 1_000n + roundedFractionMilliseconds(seconds[2]);
  return parsedMilliseconds(valueMs);
}

/** Parse an inline draft and avoid saving when its semantic value is unchanged. */
export function resolveInlineTimeCommit(
  raw: string,
  emptyValueMs: number | null,
  currentValueMs: number | null,
): InlineTimeCommitDecision {
  const parsed = parseInlineTimeInput(raw, emptyValueMs);
  if (!parsed.valid) return parsed;
  return {
    valid: true,
    valueMs: parsed.valueMs,
    shouldCommit: parsed.valueMs !== currentValueMs,
  };
}

export function formatEditableMilliseconds(valueMs: number | null): string {
  if (valueMs == null) return "";
  const wholeSeconds = Math.floor(valueMs / 1_000);
  const milliseconds = valueMs % 1_000;
  if (milliseconds === 0) return String(wholeSeconds);
  return `${wholeSeconds}.${String(milliseconds).padStart(3, "0").replace(/0+$/, "")}`;
}

export const formatInlineTimeInput = formatEditableMilliseconds;

export function isInlineTimeActivationKey(key: string): boolean {
  return key === "Enter" || key === " ";
}

export function stopInlineTimeCellEvent(event: { stopPropagation: () => void }): void {
  event.stopPropagation();
}
