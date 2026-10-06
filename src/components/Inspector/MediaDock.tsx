import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AUDIO_EXTENSIONS, IMAGE_EXTENSIONS, MIDI_EXTENSIONS, VIDEO_EXTENSIONS } from "../../lib/mediaTypes";
import { getCue, setAudioFile, setImageFile, setMidiFile, setVideoFile } from "../../lib/commands";
import type { AudioCueData, CueSummary, ImageCueData, MidiFileCueData, NumberCueData, VideoCueData } from "../../lib/types";
import { useLocale } from "../../i18n";
import { NumberPreview } from "./NumberPreview";
import { MediaPreview } from "./MediaThumbnail";
import { resolveMediaPreviewKind } from "./mediaPreviewModel";
import { formatInspectorSaveError, isCurrentCueRequest } from "./singleCueSave";
import { CueTypeIcon } from "../common/CueTypeIcon";
import { EditorErrorBoundary } from "../common/EditorErrorBoundary";
import { CLIP_EDITOR_DOCK_HEADER_HEIGHT, CLIP_EDITOR_DOCK_HEIGHT } from "../Editor/clipEditorLayout";
import { inputStyle } from "./Field";
import { normalizeNumberCueData } from "./numberModel";

type MediaCue = AudioCueData | VideoCueData | ImageCueData | MidiFileCueData | NumberCueData;
type MediaFileKind = "audio" | "video" | "image" | "midi";
const MEDIA_TYPES = new Set<CueSummary["cue_type"]>(["audio", "video", "image", "midi_file", "number"]);

export function MediaDock({
  cue, selectedCueCount, reloadToken, onRefresh, onCueSaved,
}: {
  cue: CueSummary | null;
  selectedCueCount: number;
  reloadToken: number;
  onRefresh: () => void;
  onCueSaved: () => void;
}) {
  const { t } = useLocale();
  const [cueData, setCueData] = useState<MediaCue | null>(null);
  const [error, setError] = useState<string | null>(null);
  const loadGeneration = useRef(0);
  const mediaGeneration = useRef(0);
  const cueId = selectedCueCount === 1 && cue && MEDIA_TYPES.has(cue.cue_type) ? cue.id : null;
  const selectedCueIdRef = useRef(cueId);
  selectedCueIdRef.current = cueId;

  useLayoutEffect(() => {
    ++mediaGeneration.current;
    setError(null);
    return () => { ++mediaGeneration.current; };
  }, [cueId, reloadToken]);

  useEffect(() => {
    const generation = ++loadGeneration.current;
    if (!cueId || !cue) {
      setCueData(null);
      setError(null);
      return;
    }
    getCue(cueId).then((data) => {
      if (generation !== loadGeneration.current || data.id !== cueId) return;
      const normalized = { ...data, cue_type: cue.cue_type } as MediaCue;
      setCueData(cue.cue_type === "number"
        ? normalizeNumberCueData(normalized as NumberCueData)
        : normalized);
      setError(null);
    }).catch((reason) => {
      if (generation === loadGeneration.current) {
        console.error("Failed to load media dock cue:", reason);
        setCueData(null);
      }
    });
    return () => { ++loadGeneration.current; };
  }, [cueId, cue?.cue_type, cue?.file_path, cue?.media_source_revision, reloadToken]);

  const browse = async (kind: MediaFileKind) => {
    if (!cueData) return;
    const targetId = cueData.id;
    const ticket = { cueId: targetId, generation: ++mediaGeneration.current };
    const current = () => isCurrentCueRequest(ticket, mediaGeneration.current, selectedCueIdRef.current);
    const options = {
      audio: { name: "Audio Files", extensions: [...AUDIO_EXTENSIONS] },
      video: { name: "Video Files", extensions: [...VIDEO_EXTENSIONS] },
      image: { name: "Image Files", extensions: [...IMAGE_EXTENSIONS] },
      midi: { name: "MIDI Files", extensions: [...MIDI_EXTENSIONS] },
    }[kind];
    const setFile = { audio: setAudioFile, video: setVideoFile, image: setImageFile, midi: setMidiFile }[kind];
    let picked: string | string[] | null;
    try {
      picked = await open({ multiple: false, filters: [options] });
    } catch (reason) {
      if (current()) setError(formatInspectorSaveError(reason, false, t));
      return;
    }
    if (typeof picked !== "string" || !current()) return;
    try {
      await setFile(targetId, picked);
      if (!current()) { onRefresh(); return; }
      const refreshed = await getCue(targetId);
      if (!current() || refreshed.id !== targetId) { onRefresh(); return; }
      const normalized = { ...refreshed, cue_type: cueData.cue_type } as MediaCue;
      setCueData(cueData.cue_type === "number"
        ? normalizeNumberCueData(normalized as NumberCueData)
        : normalized);
      setError(null);
      onRefresh();
      onCueSaved();
    } catch (reason) {
      if (current()) setError(formatInspectorSaveError(reason, false, t));
    }
  };

  const currentCueData = cueData?.id === cueId ? cueData : null;
  const mediaType = currentCueData?.cue_type;
  const fileKind: MediaFileKind | null = mediaType === "audio" ? "audio" : mediaType === "video" ? "video" : mediaType === "image" ? "image" : mediaType === "midi_file" ? "midi" : null;
  const filePath = fileKind ? currentCueData?.file_path ?? null : null;
  const mediaDuration = currentCueData?.cached_duration_ms ?? currentCueData?.file_duration_ms ?? currentCueData?.duration_ms ?? 0;
  const visualCue = currentCueData as ((VideoCueData | ImageCueData) & { start_time_ms?: number | null; end_time_ms?: number | null }) | null;
  const mediaEnd = visualCue?.end_time_ms ?? mediaDuration;
  const title = currentCueData?.name || cue?.name || t("common.select");

  return <section aria-label={t("inspector.media")} style={{ ...dockShell, flex: `0 0 ${CLIP_EDITOR_DOCK_HEIGHT}px` }}>
    <header style={dockHeader}>
      <span style={{ color: "var(--wc-text-bright)", fontWeight: 650, fontSize: 12 }}>{t("inspector.media")}</span>
      {currentCueData && <>
        <CueTypeIcon type={currentCueData.cue_type} size={15} tone="neutral" />
        <span title={title} style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", color: "var(--wc-text-secondary)", fontSize: 11 }}>{currentCueData.number ? `#${currentCueData.number} · ` : ""}{title}</span>
      </>}
    </header>
    <div style={dockBody}>
      {selectedCueCount > 1 ? <div style={emptyStyle}>{t("common.select")} {t("cueList.cue").toLowerCase()}.</div>
        : !cue ? <div style={emptyStyle}>{t("common.select")} {t("cueList.cue").toLowerCase()}.</div>
          : !MEDIA_TYPES.has(cue.cue_type) ? <div style={emptyStyle}>{t("inspector.noMediaForCue")}</div>
            : !currentCueData ? <div style={emptyStyle}>{t("status.loading")}</div>
              : <>
                {mediaType === "number" ? <EditorErrorBoundary label="Предпросмотр номера временно недоступен"><NumberPreview cue={currentCueData as NumberCueData} compact /></EditorErrorBoundary>
                  : (mediaType === "video" || mediaType === "image") && filePath ? <MediaPreview
                    key={`${currentCueData.id}\u0000${filePath}\u0000${currentCueData.media_source_revision ?? ""}`}
                    cueId={currentCueData.id}
                    path={filePath}
                    sourceRevision={currentCueData.media_source_revision}
                    kind={resolveMediaPreviewKind(mediaType === "video")}
                    startMs={visualCue?.start_time_ms ?? 0}
                    durationMs={Math.max(0, mediaEnd)}
                    maxHeight={150}
                  /> : null}
                {fileKind && <div style={fileRow}>
                  <input style={{ ...inputStyle, flex: 1, minWidth: 0, fontSize: 11 }} readOnly value={filePath ? filePath.split(/[\\/]/).pop() ?? filePath : t("common.none")} title={filePath ?? ""} />
                  <button type="button" onClick={() => void browse(fileKind)} style={browseButton}>{t("uiFixes.browse")}</button>
                </div>}
                {error && <div role="alert" style={errorStyle} title={error}>{error}</div>}
                {fileKind === "audio" && !filePath && <div style={emptyStyle}>{t("sweepUi.noFileAssigned")}</div>}
              </>}
    </div>
  </section>;
}

const dockShell: React.CSSProperties = { height: CLIP_EDITOR_DOCK_HEIGHT, minHeight: CLIP_EDITOR_DOCK_HEIGHT, maxHeight: CLIP_EDITOR_DOCK_HEIGHT, boxSizing: "border-box", overflow: "hidden", display: "flex", flexDirection: "column", borderTop: "1px solid var(--wc-border)", background: "var(--wc-bg-app)" };
const dockHeader: React.CSSProperties = { height: CLIP_EDITOR_DOCK_HEADER_HEIGHT, minHeight: CLIP_EDITOR_DOCK_HEADER_HEIGHT, flexShrink: 0, display: "flex", alignItems: "center", gap: 8, padding: "0 10px", boxSizing: "border-box", borderBottom: "1px solid var(--wc-border)", background: "var(--wc-bg-deepest)" };
const dockBody: React.CSSProperties = { minHeight: 0, flex: 1, overflow: "hidden", padding: "7px 9px", boxSizing: "border-box" };
const fileRow: React.CSSProperties = { display: "flex", gap: 5, alignItems: "center", minWidth: 0 };
const browseButton: React.CSSProperties = { padding: "4px 10px", background: "var(--wc-bg-hover)", border: "1px solid var(--wc-border-strong)", borderRadius: 4, color: "var(--wc-text)", cursor: "pointer", fontSize: 11, flexShrink: 0 };
const emptyStyle: React.CSSProperties = { height: "100%", display: "grid", placeItems: "center", color: "var(--wc-text-faint)", textAlign: "center", fontSize: 11, padding: 8, boxSizing: "border-box" };
const errorStyle: React.CSSProperties = { marginTop: 4, color: "#fca5a5", fontSize: 10, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" };
