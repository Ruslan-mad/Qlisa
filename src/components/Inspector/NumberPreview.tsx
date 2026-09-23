import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { NumberCueData, VideoGeometry } from "../../lib/types";
import { DEFAULT_GEOMETRY } from "../../lib/types";
import { getMediaThumbnail, prepareVideoPreview } from "../../lib/commands";
import { useNumberPreviewStore } from "../../stores/numberPreviewStore";
import { numberMasterDuration, numberPreviewItem, numberPreviewSourcePosition, numberPreviewSourceWindow, shouldRetainNumberPreviewAsset, type NumberVisualAction } from "../Editor/numberTimelineModel";
import { toMediaAssetUrl } from "./mediaPreviewModel";
import { useLocale } from "../../i18n";

type NumberPreviewProps = { cue: NumberCueData };

/** The Number transport uses the same header placement and button contract as
 * the ordinary video preview. The viewport itself remains in the Inspector. */
export function NumberPreviewControls({
  numberId,
  durationMs,
  available,
  buttonStyle,
}: {
  numberId: string;
  durationMs: number;
  available: boolean;
  buttonStyle: React.CSSProperties;
}) {
  const { t } = useLocale();
  const playing = useNumberPreviewStore((state) => state.numberId === numberId && state.playing);
  const canUse = available && durationMs > 0;
  const action = useNumberPreviewStore.getState();
  const disabledStyle: React.CSSProperties = { ...buttonStyle, opacity: canUse ? 1 : 0.5, cursor: canUse ? "pointer" : "default" };
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 3 }}>
      <button
        type="button"
        disabled={!canUse}
        aria-label={playing ? t("editorUi.videoPreviewPause") : t("editorUi.videoPreviewPlay")}
        aria-pressed={playing}
        title={canUse
          ? (playing ? t("editorUi.videoPreviewPause") : t("editorUi.videoPreviewPlay"))
          : t("editorUi.videoPreviewUnavailable")}
        onClick={() => action.toggle(numberId, durationMs)}
        style={disabledStyle}
      >{playing ? "❚❚" : "▶"}</button>
      <button
        type="button"
        disabled={!canUse}
        aria-label={t("editorUi.videoPreviewPrevFrame")}
        title={canUse ? t("editorUi.videoPreviewPrevFrame") : t("editorUi.videoPreviewUnavailable")}
        onClick={() => action.stepFrame(numberId, -1, 30, durationMs)}
        style={disabledStyle}
      >|◀</button>
      <button
        type="button"
        disabled={!canUse}
        aria-label={t("editorUi.videoPreviewNextFrame")}
        title={canUse ? t("editorUi.videoPreviewNextFrame") : t("editorUi.videoPreviewUnavailable")}
        onClick={() => action.stepFrame(numberId, 1, 30, durationMs)}
        style={disabledStyle}
      >▶|</button>
    </div>
  );
}

/** Inspector-side visual preview driven by the same cursor as Number's timeline. */
export function NumberPreview({ cue }: NumberPreviewProps) {
  const { t } = useLocale();
  const durationMs = numberMasterDuration(cue);
  const positionMs = useNumberPreviewStore((state) => state.numberId === cue.id ? state.positionMs : 0);
  const playing = useNumberPreviewStore((state) => state.numberId === cue.id && state.playing);
  const [assetUrl, setAssetUrl] = useState<string | null>(null);
  const [assetItem, setAssetItem] = useState<NumberVisualAction | null>(null);
  const [failed, setFailed] = useState(false);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const loadGenerationRef = useRef(0);
  const item = useMemo(() => numberPreviewItem(cue, positionMs), [cue, positionMs]);
  const itemKey = item ? `${item.id}\u0000${item.cue_type}\u0000${item.file_path ?? ""}` : null;
  const assetItemKey = assetItem ? `${assetItem.id}\u0000${assetItem.cue_type}\u0000${assetItem.file_path ?? ""}` : null;
  // Keep the last successfully decoded source visible while the next layer
  // is loading. This avoids a black flash when the Number cursor crosses a
  // layer boundary. A missing visual still clears the old source below.
  const retainPreviousAsset = shouldRetainNumberPreviewAsset(itemKey, assetItemKey, assetUrl);
  const renderItem = retainPreviousAsset || (itemKey && assetItemKey !== itemKey) ? assetItem : item;
  const renderAssetUrl = item ? assetUrl : null;
  const sourcePositionMs = renderItem ? numberPreviewSourcePosition(renderItem, positionMs) : 0;

  useEffect(() => {
    useNumberPreviewStore.getState().select(cue.id, durationMs);
  }, [cue.id, durationMs]);

  useEffect(() => {
    const generation = ++loadGenerationRef.current;
    let stale = false;
    setFailed(false);
    if (!item?.file_path) {
      setAssetUrl(null);
      setAssetItem(null);
      return () => { stale = true; };
    }
    const load = item.cue_type === "image"
      ? getMediaThumbnail(item.file_path, false)
      : prepareVideoPreview(item.id).then((path) => toMediaAssetUrl(path, convertFileSrc));
    load.then((url) => {
      if (!stale && generation === loadGenerationRef.current && url) {
        setAssetUrl(url);
        setAssetItem(item);
      }
    }).catch(() => {
      if (!stale && generation === loadGenerationRef.current) setFailed(true);
    });
    return () => { stale = true; };
  }, [item?.id, item?.cue_type, item?.file_path]);

  useEffect(() => {
    // Do not let the previous layer keep playing audibly/visually while the
    // replacement is being decoded. Its last frame remains visible.
    if (itemKey && assetItemKey !== itemKey && videoRef.current) videoRef.current.pause();
  }, [itemKey, assetItemKey]);

  useEffect(() => {
    // Images and audio-only gaps have no native clock. Keep the shared Number
    // cursor moving for those intervals; video is driven by HTMLMediaElement.
    // A ready non-looping video has its own media clock. During a layer
    // switch that clock is paused until the replacement is decoded, so keep
    // the shared Number cursor alive through the loading gap.
    if (!playing || (item?.cue_type === "video" && !item.looped && itemKey === assetItemKey)) return;
    const timer = window.setInterval(() => {
      const state = useNumberPreviewStore.getState();
      const next = state.positionMs + 33;
      if (next >= durationMs && durationMs > 0) {
        state.setPosition(cue.id, durationMs, durationMs);
        state.setPlaying(cue.id, false);
      } else {
        state.setPosition(cue.id, next, durationMs);
      }
    }, 33);
    return () => window.clearInterval(timer);
  }, [cue.id, durationMs, playing, item?.cue_type, item?.looped, itemKey, assetItemKey]);

  useEffect(() => {
    const video = videoRef.current;
    if (!video || !renderAssetUrl || renderItem?.cue_type !== "video" || !itemKey || assetItemKey !== itemKey) return;
    const targetSeconds = Math.max(0, sourcePositionMs) / 1000;
    const sync = () => {
      // Do not assign currentTime on every shared-cursor tick. That turns a
      // playing video into a sequence of seeks and produces a black/stalled
      // preview. Only correct a real external seek.
      if (!Number.isFinite(video.currentTime) || Math.abs(video.currentTime - targetSeconds) > 0.12) {
        try { video.currentTime = targetSeconds; } catch { /* metadata is not ready yet */ }
      }
      if (playing) {
        if (video.paused) void video.play().catch(() => { /* muted preview may still be blocked */ });
      } else if (!video.paused) video.pause();
    };
    if (video.readyState >= HTMLMediaElement.HAVE_METADATA) sync();
    else video.addEventListener("loadedmetadata", sync, { once: true });
    return () => video.removeEventListener("loadedmetadata", sync);
  }, [renderAssetUrl, renderItem?.id, renderItem?.cue_type, sourcePositionMs, playing, itemKey, assetItemKey]);

  const handleVideoTimeUpdate = (video: HTMLVideoElement) => {
    if (!renderItem || renderItem.cue_type !== "video") return;
    const { startMs: sourceStart, endMs: sourceEnd } = numberPreviewSourceWindow(renderItem);
    if (renderItem.looped && sourceEnd > sourceStart && video.currentTime * 1000 >= sourceEnd - 20) {
      try { video.currentTime = sourceStart / 1000; } catch { /* wait for the next media tick */ }
      return;
    }
    // Looping clips use the shared cursor timer so their timeline position can
    // continue beyond one source pass. Native timeupdate must not reset it.
    if (renderItem.looped) return;
    const timelineStart = Math.max(0, renderItem.timeline_start_ms ?? 0);
    const next = Math.min(durationMs, timelineStart + Math.max(0, video.currentTime * 1000 - sourceStart));
    const state = useNumberPreviewStore.getState();
    if (state.numberId === cue.id && state.playing) state.setPosition(cue.id, next, durationMs);
  };

  const geometry = ((renderItem as unknown as { geometry?: VideoGeometry } | null)?.geometry ?? DEFAULT_GEOMETRY);
  const cropLeft = Math.max(0, Math.min(0.45, geometry.crop_left ?? 0));
  const cropRight = Math.max(0, Math.min(0.45, geometry.crop_right ?? 0));
  const cropTop = Math.max(0, Math.min(0.45, geometry.crop_top ?? 0));
  const cropBottom = Math.max(0, Math.min(0.45, geometry.crop_bottom ?? 0));
  const visibleWidth = Math.max(0.1, 1 - cropLeft - cropRight);
  const visibleHeight = Math.max(0.1, 1 - cropTop - cropBottom);
  const mediaStyle: React.CSSProperties = {
    position: "absolute",
    width: `${100 / visibleWidth}%`,
    height: `${100 / visibleHeight}%`,
    left: `${(-cropLeft / visibleWidth) * 100}%`,
    top: `${(-cropTop / visibleHeight) * 100}%`,
    objectFit: geometry.fit_mode === "stretch" ? "fill" : geometry.fit_mode === "fill" ? "cover" : "contain",
    transform: `translate(${(geometry.pan_x ?? 0) * 100}%, ${(geometry.pan_y ?? 0) * 100}%) scale(${Math.max(0.01, geometry.scale ?? 1)}) rotate(${geometry.rotation ?? 0}deg)`,
    transformOrigin: "center",
  };

  return (
    <div style={previewShell} aria-label="Number preview">
      <div style={previewHeader}>
        <span style={previewTitle}>Preview</span>
        <span style={previewTime}>{(positionMs / 1000).toFixed(2)} / {(durationMs / 1000).toFixed(2)}s</span>
      </div>
      <div style={previewViewport}>
        {!item ? (
          <span style={previewHint}>{t("numberUi.previewAudioOnly")}</span>
        ) : !renderAssetUrl ? (
          <span style={previewHint}>{failed ? t("numberUi.previewFailed") : t("status.loading")}</span>
        ) : (
          <div style={previewFrame}>
            {renderItem?.cue_type === "image" ? (
              <img src={renderAssetUrl} alt={renderItem.name} style={mediaStyle} />
            ) : (
              <video
              key={`${renderItem?.id}\u0000${renderAssetUrl}`}
              ref={videoRef}
                src={renderAssetUrl}
                muted
                playsInline
                preload="metadata"
                loop={false}
                onTimeUpdate={(event) => handleVideoTimeUpdate(event.currentTarget)}
                onLoadedMetadata={(event) => {
                  try { event.currentTarget.currentTime = Math.max(0, sourcePositionMs) / 1000; } catch { /* wait */ }
                  if (playing && event.currentTarget.paused) void event.currentTarget.play().catch(() => { /* muted preview may still be blocked */ });
                }}
                onError={() => setFailed(true)}
                style={mediaStyle}
              />
            )}
          </div>
        )}
      </div>
    </div>
  );
}

const previewShell: React.CSSProperties = {
  marginBottom: 12,
  border: "1px solid var(--wc-border-strong)",
  borderRadius: 5,
  padding: 7,
  background: "var(--wc-bg-deepest)",
};

const previewHeader: React.CSSProperties = { display: "flex", justifyContent: "space-between", gap: 8, marginBottom: 6 };
const previewTitle: React.CSSProperties = { color: "var(--wc-text)", fontSize: 12, fontWeight: 600 };
const previewTime: React.CSSProperties = { color: "var(--wc-text-faint)", fontSize: 10, fontVariantNumeric: "tabular-nums" };
const previewViewport: React.CSSProperties = { width: "100%", aspectRatio: "16 / 9", minHeight: 90, display: "flex", alignItems: "center", justifyContent: "center", overflow: "hidden", background: "#000", borderRadius: 3 };
const previewFrame: React.CSSProperties = { position: "relative", width: "100%", height: "100%", overflow: "hidden", background: "#000" };
const previewHint: React.CSSProperties = { color: "var(--wc-text-faint)", fontSize: 11, textAlign: "center", padding: 10 };
