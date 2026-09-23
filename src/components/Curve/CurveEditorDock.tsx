// Curve editor dock: the full-size fade curve editor under the cue list,
// opened by the ⤢ button in the inspector's Curve section.
//
// Same idea as the clip editor dock — the inspector column is too narrow to
// place control points precisely, so the real editing happens down here where
// there is room for both curves side by side.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { FadeCueData, FadeShapes } from "../../lib/types";
import { getCue, updateCue } from "../../lib/commands";
import { CurveEditor } from "./CurveEditor";
import { useLocale } from "../../i18n";
import { formatInspectorSaveError, isCurrentCueRequest, persistThenCommit } from "../Inspector/singleCueSave";

const DEFAULT_SHAPES: FadeShapes = {
  up: { kind: "s_curve", intensity: 0, points: [], bends: [] },
  down: { kind: "s_curve", intensity: 0, points: [], bends: [] },
  mirrored: true,
};

export function CurveEditorDock({
  cueId,
  onClose,
  onSaved,
  reloadToken,
}: {
  cueId: string;
  onClose: () => void;
  /** Called after every save so the cue list + inspector refresh. */
  onSaved: () => void;
  /** Bump to re-fetch the cue (the inspector edited it). */
  reloadToken?: number;
}) {
  const { t } = useLocale();
  const [cue, setCue] = useState<FadeCueData | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const cueLoadGeneration = useRef(0);
  const saveGeneration = useRef(0);
  const currentCueId = useRef(cueId);

  useLayoutEffect(() => {
    currentCueId.current = cueId;
    ++saveGeneration.current;
  }, [cueId, reloadToken]);

  useEffect(() => {
    const generation = ++cueLoadGeneration.current;
    ++saveGeneration.current;
    setCue(null);
    setSaveError(null);
    getCue(cueId)
      .then((data) => {
        if (generation === cueLoadGeneration.current && data.id === cueId) setCue(data as unknown as FadeCueData);
      })
      .catch((error) => {
        if (generation === cueLoadGeneration.current) console.error(error);
      });
    return () => {
      ++cueLoadGeneration.current;
      ++saveGeneration.current;
    };
  }, [cueId, reloadToken]);

  const save = async (fade_shapes: FadeShapes) => {
    const ticket = { cueId, generation: ++saveGeneration.current };
    const isCurrent = () => isCurrentCueRequest(ticket, saveGeneration.current, currentCueId.current);
    await persistThenCommit(
      () => updateCue(ticket.cueId, { fade_shapes }),
      () => {
        if (!isCurrent()) return;
        setCue((prev) => (prev?.id === ticket.cueId ? { ...prev, fade_shapes } : prev));
        setSaveError(null);
        onSaved();
      },
      (error) => {
        if (!isCurrent()) return;
        setSaveError(formatInspectorSaveError(error, true, t));
        // The editor is controlled by cue state, so re-fetch restores its
        // actual persisted shape without committing the rejected draft.
        void getCue(ticket.cueId).then((data) => {
          if (isCurrent() && data.id === ticket.cueId) setCue(data as unknown as FadeCueData);
        }).catch(() => {});
      },
    );
  };

  return (
    <div
      style={{
        borderTop: "1px solid var(--wc-border-strong)",
        background: "var(--wc-bg-deepest)",
        padding: "10px 14px 14px",
        flexShrink: 0,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
        <span style={{ fontSize: 12, fontWeight: 600, color: "var(--wc-text)" }}>
          {t("editorUi.fadeCurve")}
        </span>
        <span
          style={{
            fontSize: 12, color: "var(--wc-text-muted)", overflow: "hidden",
            textOverflow: "ellipsis", whiteSpace: "nowrap",
          }}
        >
          {cue?.number ? `${cue.number} · ` : ""}{cue?.name ?? ""}
        </span>
        <button
          onClick={onClose}
          style={{
            marginLeft: "auto", padding: "2px 10px",
            background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
            borderRadius: 4, color: "var(--wc-text-secondary)", fontSize: 11, cursor: "pointer",
          }}
        >
          {t("editorUi.close")}
        </button>
      </div>

      {saveError && (
        <div role="alert" style={{ padding: 8, marginBottom: 8, borderRadius: 4, background: "rgba(248,113,113,0.12)", color: "#fca5a5", fontSize: 12 }}>
          {saveError}
        </div>
      )}

      {cue ? (
        <CurveEditor shapes={cue.fade_shapes ?? DEFAULT_SHAPES} onChange={(s) => void save(s)} />
      ) : (
        <div style={{ fontSize: 12, color: "var(--wc-text-faint)" }}>{t("editorUi.loading")}</div>
      )}
    </div>
  );
}
