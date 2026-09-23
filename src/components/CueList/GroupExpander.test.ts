import { createElement } from "react";
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import {
  GROUP_EXPANDER_TARGET_HEIGHT,
  GROUP_EXPANDER_TARGET_WIDTH,
  GroupExpander,
  handleGroupExpanderClick,
  stopGroupExpanderEvent,
  stopGroupExpanderKeyboardEvent,
} from "./GroupExpander";

describe("GroupExpander", () => {
  it("renders an accessible button with a 28px+ target and expansion state", () => {
    const html = renderToStaticMarkup(createElement(GroupExpander, {
      expanded: true,
      playhead: true,
      label: "Collapse group Intro",
      onToggle: () => {},
    }));

    expect(GROUP_EXPANDER_TARGET_WIDTH).toBeGreaterThanOrEqual(28);
    expect(GROUP_EXPANDER_TARGET_HEIGHT).toBeGreaterThanOrEqual(28);
    expect(html).toContain('type="button"');
    expect(html).toContain('aria-label="Collapse group Intro"');
    expect(html).toContain('aria-expanded="true"');
    expect(html).toContain('title="Collapse group Intro"');
    expect(html).toContain("width:30px");
    expect(html).toContain("height:28px");
  });

  it("stops row propagation for pointer, click, and Enter/Space while preserving native activation", () => {
    const stopPropagation = vi.fn();
    const onToggle = vi.fn();

    stopGroupExpanderEvent({ stopPropagation });
    handleGroupExpanderClick({ stopPropagation }, onToggle);
    stopGroupExpanderKeyboardEvent({ key: "Enter", stopPropagation });
    stopGroupExpanderKeyboardEvent({ key: " ", stopPropagation });

    expect(stopPropagation).toHaveBeenCalledTimes(4);
    expect(onToggle).toHaveBeenCalledOnce();
  });

  it("does not intercept unrelated keyboard input", () => {
    const stopPropagation = vi.fn();
    stopGroupExpanderKeyboardEvent({ key: "ArrowRight", stopPropagation });
    expect(stopPropagation).not.toHaveBeenCalled();
  });
});
