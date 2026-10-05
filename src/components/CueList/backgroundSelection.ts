export function prepareBackgroundSelection(
  target: HTMLElement,
  activeElement: HTMLElement | null,
  preventDefault: () => void,
): boolean {
  if (
    target.closest("[data-cue-id]") ||
    target.closest("button, input, textarea, select, [contenteditable], [data-resize], [data-selection-gutter]")
  ) return false;

  // Commit the current Inspector edit before selection changes and unmounts it.
  activeElement?.blur();
  preventDefault();
  return true;
}
