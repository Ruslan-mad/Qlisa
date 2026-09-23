// Global keyboard shortcut handler, mirroring QLab's key bindings.

import { useEffect, useRef } from "react";

const isMac = typeof navigator !== "undefined" && /mac/i.test(navigator.platform);
const cmdOrCtrl = (e: KeyboardEvent) => isMac ? e.metaKey : e.ctrlKey;

type KeyboardTarget = {
  tagName?: string;
  type?: string;
  isContentEditable?: boolean;
};

/**
 * Keyboard shortcuts are bound to the physical key, not the character
 * produced by the active layout.  `KeyboardEvent.code` therefore keeps Ctrl+A
 * (and the other command keys) working when the user is typing with the
 * Russian layout.  The key-character map remains as a fallback for synthetic
 * events and older WebViews which omit `code`.
 */
const RUSSIAN_LAYOUT_KEYS: Record<string, string> = {
  й: "q", ц: "w", у: "e", к: "r", е: "t", н: "y", г: "u", ш: "i", щ: "o", з: "p",
  ф: "a", ы: "s", в: "d", а: "f", п: "g", р: "h", о: "j", л: "k", д: "l",
  я: "z", ч: "x", с: "c", м: "v", и: "b", т: "n", ь: "m",
};

const PHYSICAL_SHORTCUT_KEYS: Record<string, string> = {
  BracketLeft: "[",
  BracketRight: "]",
  Comma: ",",
};

export function getKeyboardShortcutKey(
  e: Pick<KeyboardEvent, "key"> & Partial<Pick<KeyboardEvent, "code">>,
): string {
  if (e.code && e.code in PHYSICAL_SHORTCUT_KEYS) {
    return PHYSICAL_SHORTCUT_KEYS[e.code];
  }
  if (e.code?.startsWith("Key") && e.code.length === 4) {
    return e.code.slice(3).toLowerCase();
  }
  const key = e.key.toLowerCase();
  return RUSSIAN_LAYOUT_KEYS[key] ?? (/^[a-z]$/.test(key) ? key : e.key);
}

const TEXT_INPUT_TYPES = new Set(["text", "search", "url", "email", "tel", "password", "number"]);

/** Return true only for controls where a Space should remain text/editing input. */
export function isEditableKeyboardTarget(target: EventTarget | null): boolean {
  if (!target || typeof target !== "object") return false;

  const element = target as KeyboardTarget;
  if (element.isContentEditable) return true;

  const tagName = element.tagName?.toUpperCase();
  if (tagName === "TEXTAREA") return true;
  if (tagName !== "INPUT") return false;

  // An omitted input type is the browser's text input default.
  return TEXT_INPUT_TYPES.has((element.type || "text").toLowerCase());
}

/** Preserve the legacy guard for every non-Space shortcut. */
export function isShortcutGuardedTarget(target: EventTarget | null): boolean {
  if (!target || typeof target !== "object") return false;

  const element = target as KeyboardTarget;
  const tagName = element.tagName?.toUpperCase();
  return tagName === "INPUT" || tagName === "TEXTAREA" || Boolean(element.isContentEditable);
}

/** Prefer the physical Space key while retaining a fallback for synthetic/older events. */
export function isSpaceShortcutEvent(e: Pick<KeyboardEvent, "code" | "key">): boolean {
  return e.code === "Space" || (!e.code && e.key === " ");
}

/** Space is global GO everywhere except a real text-editing target. */
export function shouldHandleGlobalSpaceShortcut(
  target: EventTarget | null,
  e: Pick<KeyboardEvent, "code" | "key">,
): boolean {
  return isSpaceShortcutEvent(e) && !isEditableKeyboardTarget(target);
}

/** The unmodified S shortcut (or Ы on the Russian layout) stops only the currently selected cue. */
export function isSelectedCueStopShortcut(
  e: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey"> & Partial<Pick<KeyboardEvent, "code">>,
): boolean {
  return getKeyboardShortcutKey(e) === "s" && !e.ctrlKey && !e.metaKey && !e.altKey;
}
import {
  go,
  hardStopAll,
  stopAll,
  stopCue,
  pauseCue,
  resumeCue,
  addCue,
  setPlayhead,
} from "../lib/commands";
// Edit operations are shared with the Edit / Action menus so a shortcut and
// its menu entry can never behave differently.
import {
  copySelection,
  deleteSelection,
  duplicateSelection,
  groupSelection,
  pasteAfterSelection,
  redoAction,
  selectAllCues,
  undoAction,
} from "../lib/cueOperations";
import { useWorkspaceStore } from "../stores/workspaceStore";

export function useKeyboardShortcuts(
  onRefresh: () => void,
  onOpenPreferences?: () => void,
  onSave?: () => void,
  onOpen?: () => void,
  onToggleInspector?: () => void,
  onGoto?: () => void,
  onToggleOutputWindow?: () => void,
  onToggleShowMode?: () => void,
  onToggleSearch?: () => void,
) {
  const lastEscapeRef = useRef<number>(0);
  const lastGoRef = useRef<number>(0);
  const { selectedCueId, generalPrefs } = useWorkspaceStore();

  useEffect(() => {
    const handler = async (e: KeyboardEvent) => {
      // Ctrl+F toggles the in-app search bar and overrides the WebView's native
      // find bar. Handled before the input guard so it works even while a text
      // field (including the search box itself) is focused.
      if (getKeyboardShortcutKey(e) === "f" && e.ctrlKey) {
        e.preventDefault();
        onToggleSearch?.();
        return;
      }

      // Space is global GO on buttons, checkboxes, selects, and other
      // non-editable controls. Text editing targets retain native spaces.
      if (shouldHandleGlobalSpaceShortcut(e.target, e)) {
        // Space → GO (with double-GO protection)
        e.preventDefault();
        const now = Date.now();
        const protection = generalPrefs.double_go_protection_ms;
        if (protection > 0 && now - lastGoRef.current < protection) return;
        lastGoRef.current = now;
        await go().then(() => {
          window.dispatchEvent(new CustomEvent("inkue:transport-error", { detail: null }));
        }).catch((error) => {
          console.error(error);
          window.dispatchEvent(new CustomEvent("inkue:transport-error", { detail: String(error) }));
        });
        onRefresh();
        return;
      }

      // Preserve the original guard for every non-Space shortcut: clicking a
      // checkbox/radio must not make Delete/S stop or mutate the selected cue.
      if (isShortcutGuardedTarget(e.target)) return;

      switch (getKeyboardShortcutKey(e)) {
        case "Escape": {
          // Single Escape → Stop All; double Escape → Hard Stop All
          const now = Date.now();
          if (now - lastEscapeRef.current < 500) {
            await hardStopAll().catch(console.error);
          } else {
            await stopAll().catch(console.error);
          }
          lastEscapeRef.current = now;
          onRefresh();
          break;
        }
        case "s":
        {
          if (cmdOrCtrl(e)) {
            e.preventDefault();
            onSave?.();
          } else if (isSelectedCueStopShortcut(e) && selectedCueId) {
            e.preventDefault();
            await stopCue(selectedCueId).catch(console.error);
            onRefresh();
          }
          break;
        }
        case "o":
        {
          if (cmdOrCtrl(e)) {
            e.preventDefault();
            onOpen?.();
          }
          break;
        }
        case "i":
        {
          if (cmdOrCtrl(e)) {
            e.preventDefault();
            onToggleInspector?.();
          }
          break;
        }
        case "p":
        case "[": {
          if (!cmdOrCtrl(e) && selectedCueId) {
            await pauseCue(selectedCueId).catch(console.error);
            onRefresh();
          }
          break;
        }
        case "]": {
          if (selectedCueId) {
            await resumeCue(selectedCueId).catch(console.error);
            onRefresh();
          }
          break;
        }
        case ",": {
          if (cmdOrCtrl(e)) {
            e.preventDefault();
            onOpenPreferences?.();
          }
          break;
        }
        case "ArrowUp": {
          if (cmdOrCtrl(e)) {
            e.preventDefault();
            const { cues, playheadCueId } = useWorkspaceStore.getState();
            const idx = cues.findIndex((c) => c.id === playheadCueId);
            const prevCue = idx > 0 ? cues[idx - 1] : cues[0];
            if (prevCue) {
              await setPlayhead(prevCue.id).catch(console.error);
              onRefresh();
            }
          }
          break;
        }
        case "ArrowDown": {
          if (cmdOrCtrl(e)) {
            e.preventDefault();
            const { cues, playheadCueId } = useWorkspaceStore.getState();
            const idx = cues.findIndex((c) => c.id === playheadCueId);
            const nextCue = idx < cues.length - 1 ? cues[idx + 1] : cues[cues.length - 1];
            if (nextCue) {
              await setPlayhead(nextCue.id).catch(console.error);
              onRefresh();
            }
          }
          break;
        }
        case "a":
        {
          if (cmdOrCtrl(e)) {
            e.preventDefault();
            selectAllCues();
          }
          break;
        }
        case "g":
        {
          if (cmdOrCtrl(e)) {
            e.preventDefault();
            await groupSelection(onRefresh);
          } else if (!cmdOrCtrl(e) && !e.shiftKey && !e.altKey) {
            e.preventDefault();
            onGoto?.();
          }
          break;
        }
        case "n":
        {
          if (cmdOrCtrl(e)) {
            e.preventDefault();
            await addCue("audio").catch(console.error);
            onRefresh();
          }
          break;
        }
        case "d":
        {
          if (cmdOrCtrl(e) && selectedCueId) {
            e.preventDefault();
            await duplicateSelection(onRefresh);
          }
          break;
        }
        case "z":
        {
          if (cmdOrCtrl(e) && e.shiftKey) {
            // Ctrl+Shift+Z → Redo (alternative to Ctrl+Y)
            e.preventDefault();
            await redoAction(onRefresh);
          } else if (cmdOrCtrl(e)) {
            // Ctrl+Z → Undo
            e.preventDefault();
            await undoAction(onRefresh);
          }
          break;
        }
        case "y":
        {
          if (cmdOrCtrl(e)) {
            e.preventDefault();
            await redoAction(onRefresh);
          }
          break;
        }
        case "c":
        {
          if (cmdOrCtrl(e) && selectedCueId) {
            e.preventDefault();
            await copySelection();
          }
          break;
        }
        case "v":
        {
          if (cmdOrCtrl(e)) {
            e.preventDefault();
            await pasteAfterSelection(onRefresh);
          }
          break;
        }
        case "Delete":
        case "Backspace": {
          if (selectedCueId && cmdOrCtrl(e) === false) {
            await deleteSelection(onRefresh, generalPrefs.confirm_before_delete);
          }
          break;
        }
        case "F5": {
          e.preventDefault();
          onToggleShowMode?.();
          break;
        }
        case "F9": {
          e.preventDefault();
          onToggleOutputWindow?.();
          break;
        }
        default:
          break;
      }
    };

    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [selectedCueId, generalPrefs, onRefresh, onOpenPreferences, onSave, onOpen, onToggleInspector, onGoto, onToggleOutputWindow, onToggleShowMode, onToggleSearch]);
}
