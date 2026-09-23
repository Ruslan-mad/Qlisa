import type { CSSProperties } from "react";
import { useLocale } from "../../i18n";
import { useVideoPreviewTransport } from "./mediaPreviewTransport";

export function VideoPreviewControls({
  identity,
  buttonStyle,
}: {
  identity: string;
  buttonStyle: CSSProperties;
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
        onClick={() => useVideoPreviewTransport.getState().toggle(identity)}
        style={{ ...buttonStyle, opacity: available ? 1 : 0.5, cursor: available ? "pointer" : "default" }}
      >
        {playing ? "❚❚" : "▶"}
      </button>
      <button
        type="button"
        disabled={!available}
        aria-label={t("editorUi.videoPreviewPrevFrame")}
        title={available ? t("editorUi.videoPreviewPrevFrame") : unavailableTitle}
        onClick={() => useVideoPreviewTransport.getState().stepFrame(identity, -1, transport?.frameRate)}
        style={{ ...buttonStyle, opacity: available ? 1 : 0.5, cursor: available ? "pointer" : "default" }}
      >
        |◀
      </button>
      <button
        type="button"
        disabled={!available}
        aria-label={t("editorUi.videoPreviewNextFrame")}
        title={available ? t("editorUi.videoPreviewNextFrame") : unavailableTitle}
        onClick={() => useVideoPreviewTransport.getState().stepFrame(identity, 1, transport?.frameRate)}
        style={{ ...buttonStyle, opacity: available ? 1 : 0.5, cursor: available ? "pointer" : "default" }}
      >
        ▶|
      </button>
    </div>
  );
}
