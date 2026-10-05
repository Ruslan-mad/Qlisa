export type OutputMonitorDiagnosticRecord = Record<string, unknown>;

const record = (value: unknown): OutputMonitorDiagnosticRecord | null =>
  value && typeof value === "object" && !Array.isArray(value) ? value as OutputMonitorDiagnosticRecord : null;

/** Normalize the backend's nested { capture, binary, frontend } diagnostics DTO. */
export function outputMonitorDiagnosticsModel(value: unknown) {
  const raw = record(value);
  if (!raw) return null;
  const frontend = record(raw.frontend);
  return {
    capture: record(raw.capture),
    binary: record(raw.binary),
    frontend,
    sourceId: typeof frontend?.sourceId === "string" ? frontend.sourceId : null,
    active: frontend?.active === true,
  };
}
