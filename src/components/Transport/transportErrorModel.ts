export function transportErrorFromEvent(event: Event): string | null {
  const detail = (event as CustomEvent<string | null>).detail;
  return typeof detail === "string" && detail.length > 0 ? detail : null;
}
