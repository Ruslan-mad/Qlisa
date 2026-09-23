import type { KeyboardEvent, MouseEvent } from "react";
import { PlayheadIndicator } from "./PlayheadIndicator";

export const GROUP_EXPANDER_TARGET_WIDTH = 30;
export const GROUP_EXPANDER_TARGET_HEIGHT = 28;

type PropagationEvent = { stopPropagation: () => void };

export function stopGroupExpanderEvent(event: PropagationEvent): void {
  event.stopPropagation();
}

export function stopGroupExpanderKeyboardEvent(event: Pick<KeyboardEvent<HTMLButtonElement>, "key" | "stopPropagation">): void {
  if (event.key === "Enter" || event.key === " ") event.stopPropagation();
}

export function handleGroupExpanderClick(event: PropagationEvent, onToggle: () => void): void {
  event.stopPropagation();
  onToggle();
}

export function GroupExpander({
  expanded,
  playhead,
  label,
  onToggle,
}: {
  expanded: boolean;
  playhead: boolean;
  label: string;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      draggable={false}
      aria-label={label}
      aria-expanded={expanded}
      title={label}
      onMouseDown={stopGroupExpanderEvent}
      onKeyDown={stopGroupExpanderKeyboardEvent}
      onKeyUp={stopGroupExpanderKeyboardEvent}
      onClick={(event: MouseEvent<HTMLButtonElement>) => handleGroupExpanderClick(event, onToggle)}
      onDoubleClick={stopGroupExpanderEvent}
      onDragStart={(event) => {
        event.preventDefault();
        event.stopPropagation();
      }}
      style={{
        position: "absolute",
        left: 0,
        top: "50%",
        transform: "translateY(-50%)",
        width: GROUP_EXPANDER_TARGET_WIDTH,
        height: GROUP_EXPANDER_TARGET_HEIGHT,
        padding: 0,
        border: "none",
        borderRadius: 4,
        background: "transparent",
        color: "var(--wc-text-muted)",
        cursor: "pointer",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: 1,
        flexShrink: 0,
        zIndex: 4,
      }}
    >
      <PlayheadIndicator visible={playhead} />
      <span aria-hidden="true" style={{ fontSize: 13, lineHeight: 1, width: 13, textAlign: "center" }}>
        {expanded ? "▼" : "▶"}
      </span>
    </button>
  );
}
