import type { CSSProperties } from "react";
import { useLocale } from "../../i18n";
import { useVideoPreviewTransport } from "./mediaPreviewTransport";
import { stepPreviewAndSyncHeadphones } from "../Editor/headphonePreviewTransport";

export function VideoPreviewControls({
  identity,
  buttonStyle,
  onTransportToggle,
  onSeek,
}: {
  identity: string;
  buttonStyle: CSSProperties;
  onTransportToggle?: (playing: boolean) => void;
  onSeek?: (positionMs: number, pauseBeforeSeek?: boolean) => void;
}) {
  const { t } = useLocale();
  const transport = useVideoPreviewTransport((state) => state.identity === identity ? state : null);
  const available = !!transport?.mounted && !!transport.ready && !!transport.visible;
  const playing = !!transport?.playing;
  const unavailableTitle = t("editorUi.videoPreviewUnavailable");

  return (
    <div style={{ display: "flex", alignItems: "center", gap: 3 }}>
      <button
        type="button"
        disabled={!available}
        aria-label={playing ? t("editorUi.videoPreviewPause") : t("editorUi.videoPreviewPlay")}
        aria-pressed={playing}
        title={available
          ? (playing ? t("editorUi.videoPreviewPause") : t("editorUi.videoPreviewPlay"))
          : unavailableTitle}
        onClick={() => {
          const state = useVideoPreviewTransport.getState();
          state.toggle(identity);
          onTransportToggle?.(state.playing);
        }}
        style={{ ...buttonStyle, opacity: available ? 1 : 0.5, cursor: available ? "pointer" : "default" }}
      >
        {playing ? "❚❚" : "▶"}
      </button>
      <button
        type="button"
        disabled={!available}
        aria-label={t("editorUi.videoPreviewPrevFrame")}
        title={available ? t("editorUi.videoPreviewPrevFrame") : unavailableTitle}
        onClick={() => {
          const state = useVideoPreviewTransport.getState();
          stepPreviewAndSyncHeadphones(() => {
            state.stepFrame(identity, -1, transport?.frameRate);
            return useVideoPreviewTransport.getState().positionMs;
          }, onSeek);
        }}
        style={{ ...buttonStyle, opacity: available ? 1 : 0.5, cursor: available ? "pointer" : "default" }}
      >
        |◀
      </button>
      <button
        type="button"
        disabled={!available}
        aria-label={t("editorUi.videoPreviewNextFrame")}
        title={available ? t("editorUi.videoPreviewNextFrame") : unavailableTitle}
        onClick={() => {
          const state = useVideoPreviewTransport.getState();
          stepPreviewAndSyncHeadphones(() => {
            state.stepFrame(identity, 1, transport?.frameRate);
            return useVideoPreviewTransport.getState().positionMs;
          }, onSeek);
        }}
        style={{ ...buttonStyle, opacity: available ? 1 : 0.5, cursor: available ? "pointer" : "default" }}
      >
        ▶|
      </button>
    </div>
  );
}
