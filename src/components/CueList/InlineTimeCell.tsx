import { useId, useLayoutEffect, useRef, useState } from "react";
import type { CueSummary } from "../../lib/types";
import { useLocale } from "../../i18n";
import {
  formatInlineTimeInput,
  isInlineTimeActivationKey,
  resolveInlineTimeCommit,
  stopInlineTimeCellEvent,
} from "./inlineTimeModel";

type CommitResult = CueSummary | void;

interface Props {
  cueId: string;
  fieldKey: string;
  label: string;
  isEditing: boolean;
  valueMs: number | null;
  emptyValueMs: number | null;
  displayValue: string;
  transparentDisplayBackground?: boolean;
  onEditingChange: (editing: boolean) => void;
  onCommit: (cueId: string, valueMs: number | null) => Promise<CommitResult>;
  onSaved?: (cue: CueSummary) => void;
  onFailure?: () => void;
}

const INPUT_STYLE: React.CSSProperties = {
  width: "100%",
  height: "100%",
  minWidth: 0,
  boxSizing: "border-box",
  padding: "0 6px",
  border: "1px solid var(--wc-accent)",
  borderRadius: 3,
  background: "var(--wc-bg-app)",
  color: "var(--wc-text)",
  fontSize: 12,
  textAlign: "right",
  outline: "none",
};

const SR_ONLY_STYLE: React.CSSProperties = {
  position: "absolute",
  width: 1,
  height: 1,
  padding: 0,
  margin: -1,
  overflow: "hidden",
  clip: "rect(0, 0, 0, 0)",
  whiteSpace: "nowrap",
  border: 0,
};

export function InlineTimeCell({
  cueId,
  fieldKey,
  label,
  isEditing,
  valueMs,
  emptyValueMs,
  displayValue,
  transparentDisplayBackground = false,
  onEditingChange,
  onCommit,
  onSaved,
  onFailure,
}: Props) {
  const { t } = useLocale();
  const errorId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const cueIdRef = useRef(cueId);
  const previousCueIdRef = useRef(cueId);
  const requestGenerationRef = useRef(0);
  const mountedRef = useRef(false);
  const pendingRef = useRef(false);
  const skipBlurCommitRef = useRef(false);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);

  cueIdRef.current = cueId;

  useLayoutEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      ++requestGenerationRef.current;
    };
  }, []);

  useLayoutEffect(() => {
    if (previousCueIdRef.current === cueId) return;
    previousCueIdRef.current = cueId;
    ++requestGenerationRef.current;
    pendingRef.current = false;
    setSaving(false);
    setError(null);
    setDraft("");
    onEditingChange(false);
  }, [cueId]);

  useLayoutEffect(() => {
    if (!isEditing) {
      setHovered(false);
      setFocused(false);
      return;
    }
    skipBlurCommitRef.current = false;
    setDraft(formatInlineTimeInput(valueMs));
    setError(null);
  }, [isEditing]);

  const isCurrent = (cueIdAtSubmit: string, generation: number) =>
    mountedRef.current && cueIdRef.current === cueIdAtSubmit && requestGenerationRef.current === generation;

  const openEditor = () => {
    if (saving) return;
    ++requestGenerationRef.current;
    setHovered(false);
    setFocused(false);
    setError(null);
    onEditingChange(true);
  };

  const commit = async () => {
    if (pendingRef.current) return;
    const decision = resolveInlineTimeCommit(draft, emptyValueMs, valueMs);
    if (!decision.valid) {
      setError(t("cueList.invalidTime"));
      return;
    }
    if (!decision.shouldCommit) {
      ++requestGenerationRef.current;
      pendingRef.current = false;
      setSaving(false);
      setError(null);
      setHovered(false);
      setFocused(false);
      onEditingChange(false);
      return;
    }

    const submittedCueId = cueId;
    const generation = ++requestGenerationRef.current;
    pendingRef.current = true;
    setSaving(true);
    setError(null);
    try {
      const result = await onCommit(submittedCueId, decision.valueMs);
      if (!isCurrent(submittedCueId, generation)) return;
      setError(null);
      if (result && typeof result === "object" && "id" in result) onSaved?.(result);
      setHovered(false);
      setFocused(false);
      onEditingChange(false);
    } catch (reason) {
      if (!isCurrent(submittedCueId, generation)) return;
      const message = t("cueList.timeSaveFailed", { error: String(reason) });
      setError(message);
      onFailure?.();
    } finally {
      if (isCurrent(submittedCueId, generation)) {
        pendingRef.current = false;
        setSaving(false);
      }
    }
  };

  const handleKeyboardActivation = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (isInlineTimeActivationKey(event.key)) stopInlineTimeCellEvent(event);
  };

  return (
    <div data-time-cell={fieldKey} style={{ position: "relative", width: "100%", height: "100%", minWidth: 0 }}>
      {isEditing ? (
        <input
          ref={inputRef}
          autoFocus
          type="text"
          inputMode="decimal"
          value={draft}
          aria-label={label}
          aria-invalid={error != null}
          aria-describedby={error ? errorId : undefined}
          title={error ?? label}
          disabled={saving}
          onChange={(event) => { setDraft(event.target.value); setError(null); }}
          onFocus={(event) => event.currentTarget.select()}
          onBlur={() => {
            if (skipBlurCommitRef.current) {
              skipBlurCommitRef.current = false;
              return;
            }
            if (!pendingRef.current) void commit();
          }}
          onKeyDown={(event) => {
            event.stopPropagation();
            if (event.key === "Enter") {
              event.preventDefault();
              void commit();
            } else if (event.key === "Escape") {
              event.preventDefault();
              skipBlurCommitRef.current = true;
              ++requestGenerationRef.current;
              pendingRef.current = false;
              setSaving(false);
              setError(null);
              setHovered(false);
              setFocused(false);
              onEditingChange(false);
            } else if (event.key === " ") {
              event.preventDefault();
            }
          }}
          onKeyUp={(event) => event.stopPropagation()}
          onClick={stopInlineTimeCellEvent}
          onMouseDown={stopInlineTimeCellEvent}
          style={{ ...INPUT_STYLE, borderColor: error ? "#ef4444" : "var(--wc-accent)" }}
        />
      ) : (
        <button
          type="button"
          aria-label={label}
          aria-describedby={error ? errorId : undefined}
          title={error ?? label}
          disabled={saving}
          onMouseEnter={() => setHovered(true)}
          onMouseLeave={() => setHovered(false)}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          onMouseDown={stopInlineTimeCellEvent}
          onKeyDown={handleKeyboardActivation}
          onKeyUp={handleKeyboardActivation}
          onClick={(event) => {
            event.stopPropagation();
            openEditor();
          }}
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "flex-end",
            gap: 4,
            width: "100%",
            height: "100%",
            boxSizing: "border-box",
            minWidth: 0,
            padding: "0 8px 0 4px",
            border: `1px solid ${error ? "rgba(239,68,68,0.65)" : "transparent"}`,
            borderRadius: 3,
            background: transparentDisplayBackground
              ? "transparent"
              : focused ? "var(--wc-accent-dim)" : hovered ? "var(--wc-bg-hover)" : "transparent",
            color: error ? "#fca5a5" : "var(--wc-text)",
            font: "inherit",
            fontSize: 12,
            textAlign: "right",
            cursor: saving ? "default" : "text",
            outline: focused ? "1px solid var(--wc-accent)" : "none",
            outlineOffset: -1,
          }}
        >
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{displayValue}</span>
          {(hovered || focused) && <span aria-hidden="true" style={{ fontSize: 10, opacity: 0.65 }}>✎</span>}
          {error && <span aria-hidden="true" style={{ color: "#ef4444", fontWeight: 700 }}>!</span>}
        </button>
      )}
      {error && <span id={errorId} role="alert" style={SR_ONLY_STYLE}>{error}</span>}
    </div>
  );
}
