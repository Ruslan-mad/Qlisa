import { useCallback, useEffect, useState } from "react";
import type { AudioCueData, CameraCueData, OutputPatch, VideoCueData } from "../../lib/types";
import { getNormalizeDb, getOutputPatchTable, setLiveLevel } from "../../lib/commands";
import { Field, inputStyle } from "./Field";
import { Select } from "../common/Select";
import { LevelMatrixGrid } from "./LevelMatrixGrid";
import { DragNumber } from "../common/DragNumber";
import { useLocale } from "../../i18n";

export function LevelsTab({
  cue,
  isAudio,
  onSave,
}: {
  cue: AudioCueData | VideoCueData | CameraCueData;
  isAudio: boolean;
  onSave: (p: Partial<AudioCueData | VideoCueData | CameraCueData>) => void;
}) {
  const { t } = useLocale();
  const [volumeDb, setVolumeDb] = useState(cue.volume_db);
  const [pan, setPan] = useState(isAudio ? (cue as AudioCueData).pan : 0);
  const [normalizing, setNormalizing] = useState(false);
  const [normalizeError, setNormalizeError] = useState<string | null>(null);
  const [patches, setPatches] = useState<OutputPatch[]>([]);
  const canNormalize = cue.cue_type === "audio";

  useEffect(() => {
    getOutputPatchTable()
      .then((t) => { setPatches(t.patches); })
      .catch(console.error);
  }, []);

  // Missing assignment always means Main. Preview/headphones is never part of
  // this list because it is an operator-only route.
  const activePatch = patches.find((p) => p.id === cue.output_patch_id) ?? patches.find((p) => p.kind === "main");
  const selectedBusValue = activePatch?.kind === "main" ? "" : (cue.output_patch_id ?? "");

  // Sync when the selected cue changes or after an external save
  useEffect(() => {
    setVolumeDb(cue.volume_db);
    if (isAudio) setPan((cue as AudioCueData).pan);
    setNormalizeError(null);
  }, [cue.id, cue.volume_db, isAudio, (cue as AudioCueData).pan]);

  // While dragging, the value goes straight to the engine so a playing cue
  // follows the slider. Persisting through onSave on every step would
  // re-serialise the cue and push an undo snapshot per pixel moved.
  const previewLevels = useCallback(
    (v: number, p: number) => { void setLiveLevel(cue.id, v, p).catch(console.error); },
    [cue.id]
  );
  const commitVolume = useCallback(
    (v: number) => onSave({ volume_db: v }),
    [onSave]
  );
  const commitPan = useCallback(
    (v: number) => onSave({ pan: v } as Partial<AudioCueData>),
    [onSave]
  );

  const handleNormalize = useCallback(async () => {
    setNormalizing(true);
    setNormalizeError(null);
    try {
      const db = await getNormalizeDb(cue.id);
      const rounded = Math.round(db * 10) / 10;
      setVolumeDb(rounded);
      commitVolume(rounded);
    } catch (e) {
      setNormalizeError(String(e));
    } finally {
      setNormalizing(false);
    }
  }, [cue.id, commitVolume]);

  return (
    <>
      <Field label={t("audioBusUi.bus")}>
        <Select
          style={{ ...inputStyle, cursor: "pointer" }}
          value={selectedBusValue}
          onChange={(e) => onSave({ output_patch_id: e.target.value || null })}
        >
          <option value="">{t("audioBusUi.main")}</option>
          {patches.filter((p) => p.kind === "aux" && p.enabled).map((p) => (
            <option key={p.id} value={p.id}>{p.name}</option>
          ))}
          {cue.output_patch_id && !patches.some((p) => p.id === cue.output_patch_id) && (
            <option value={cue.output_patch_id}>{t("errors.outputPatchNotFound")}</option>
          )}
        </Select>
      </Field>

      <Field label="Volume (dB)">
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <input
            style={{ ...inputStyle, flex: 1, padding: "2px 4px" }}
            type="range"
            min="-60"
            max="12"
            step="0.5"
            value={volumeDb}
            onChange={(e) => {
              const v = parseFloat(e.target.value);
              setVolumeDb(v);
              previewLevels(v, pan);
            }}
            onMouseUp={() => commitVolume(volumeDb)}
          />
          <DragNumber
            style={{ ...inputStyle, width: 60 }}
            step="0.5"
            min="-60"
            max="12"
            value={volumeDb.toFixed(1)}
            onChange={(e) => setVolumeDb(parseFloat(e.target.value))}
            onBlur={() => commitVolume(volumeDb)}
          />
        </div>
      </Field>

      {canNormalize && (
        <Field label="">
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <button
              onClick={() => void handleNormalize()}
              disabled={normalizing}
              style={{
                background: normalizing ? "var(--wc-bg-surface)" : "var(--wc-bg-app)",
                border: "1px solid var(--wc-border-strong)",
                borderRadius: 4,
                color: normalizing ? "var(--wc-text-faint)" : "var(--wc-text-secondary)",
                cursor: normalizing ? "default" : "pointer",
                fontSize: 12,
                padding: "4px 10px",
                textAlign: "center",
              }}
              onMouseEnter={(e) => {
                if (!normalizing)
                  (e.currentTarget as HTMLButtonElement).style.color = "var(--wc-text)";
              }}
              onMouseLeave={(e) => {
                if (!normalizing)
                  (e.currentTarget as HTMLButtonElement).style.color = "var(--wc-text-secondary)";
              }}
            >
              {normalizing ? t("uiFixes.analyzing") : t("sweepUi.normalize")}
            </button>
            {normalizeError && (
              <span style={{ fontSize: 11, color: "#f87171" }}>{normalizeError}</span>
            )}
          </div>
        </Field>
      )}
      {isAudio && (
        <Field label="Pan">
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <span style={{ color: "var(--wc-text-secondary)", fontSize: 11, flexShrink: 0 }}>L</span>
            <input
              style={{ ...inputStyle, flex: 1, padding: "2px 4px" }}
              type="range"
              min="-1"
              max="1"
              step="0.05"
              value={pan}
              onChange={(e) => {
                const p = parseFloat(e.target.value);
                setPan(p);
                previewLevels(volumeDb, p);
              }}
              onMouseUp={() => commitPan(pan)}
            />
            <span style={{ color: "var(--wc-text-secondary)", fontSize: 11, flexShrink: 0 }}>R</span>
            <DragNumber
              style={{ ...inputStyle, width: 60 }}
              step="0.05"
              min="-1"
              max="1"
              value={pan.toFixed(2)}
              onChange={(e) => setPan(parseFloat(e.target.value))}
              onBlur={() => commitPan(pan)}
            />
          </div>
        </Field>
      )}

      <LevelMatrixGrid
        cueId={cue.id}
        matrix={cue.level_matrix ?? null}
        patchName={activePatch?.name ?? t("audioBusUi.main")}
        deviceChannels={activePatch?.channels ?? []}
        onSave={(m) => onSave({ level_matrix: m } as Partial<AudioCueData>)}
      />
    </>
  );
}
