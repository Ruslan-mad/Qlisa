// Inline media preview for the inspector's Media section. Images stay static;
// videos play silently in-place and fall back to the generated thumbnail when
// the WebView cannot decode the source codec.

import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getMediaThumbnail, prepareVideoPreview } from "../../lib/commands";
import { useLocale } from "../../i18n";
import {
  createMediaPreviewIdentity,
  measurePreviewFrameRate,
  shouldPlayMediaPreview,
  toMediaAssetUrl,
  type MediaPreviewKind,
} from "./mediaPreviewModel";
import { useVideoPreviewTransport } from "./mediaPreviewTransport";

// Session-lifetime cache: the backend caches JPEGs on disk, but this avoids
// re-invoking (and re-transferring the data URL) on every cue re-selection.
const thumbnailCache = new Map<string, string>();

export function MediaThumbnail({
  path,
  seekInto,
}: {
  path: string;
  /** Pick a frame ~15% in (videos — frame 0 is often black). */
  seekInto: boolean;
}) {
  const { t } = useLocale();
  const [url, setUrl] = useState<string | null>(() => thumbnailCache.get(path) ?? null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const cached = thumbnailCache.get(path);
    setUrl(cached ?? null);
    setFailed(false);
    if (cached) return;

    let stale = false;
    getMediaThumbnail(path, seekInto)
      .then((dataUrl) => {
        thumbnailCache.set(path, dataUrl);
        if (!stale) setUrl(dataUrl);
      })
      .catch(() => {
        if (!stale) setFailed(true);
      });
    return () => { stale = true; };
  }, [path, seekInto]);

  if (failed) return null;

  return (
    <div
      style={{
        marginBottom: 10,
        borderRadius: 4,
        overflow: "hidden",
        border: "1px solid var(--wc-border-strong)",
        background: "#000",
        minHeight: url ? undefined : 90,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      {url ? (
        <img
          src={url}
          alt=""
          style={{ display: "block", width: "100%", maxHeight: 180, objectFit: "contain" }}
        />
      ) : (
        <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>{t("status.loading")}</span>
      )}
    </div>
  );
}

export function MediaPreview({
  cueId,
  path,
  kind,
  startMs = 0,
  durationMs = 0,
}: {
  cueId: string;
  path: string;
  kind: MediaPreviewKind;
  startMs?: number;
  durationMs?: number;
}) {
  if (kind === "image") {
    return <MediaThumbnail path={path} seekInto={false} />;
  }

  return <VideoPreview cueId={cueId} path={path} startMs={startMs} durationMs={durationMs} />;
}

type PreviewFrameMetadata = { mediaTime: number; presentedFrames: number };
type VideoWithFrameCallbacks = HTMLVideoElement & {
  requestVideoFrameCallback?: (callback: (now: number, metadata: PreviewFrameMetadata) => void) => number;
  cancelVideoFrameCallback?: (handle: number) => void;
};

function VideoPreview({
  cueId,
  path,
  startMs,
  durationMs,
}: {
  cueId: string;
  path: string;
  startMs: number;
  durationMs: number;
}) {
  const { t } = useLocale();
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [failed, setFailed] = useState(false);
  const identity = createMediaPreviewIdentity(cueId, path);
  const transport = useVideoPreviewTransport((state) => state.identity === identity ? state : null);
  const [authorizedSource, setAuthorizedSource] = useState<{
    cueId: string;
    path: string;
    url: string;
  } | null>(null);
  const [intersection, setIntersection] = useState<{
    source: string;
    visible: boolean;
  } | null>(null);
  const [documentVisible, setDocumentVisible] = useState(
    () => typeof document === "undefined" || document.visibilityState === "visible",
  );
  // Never render a URL authorized for the previously selected cue, not even
  // for the single render before the async authorization effect resets state.
  const source = authorizedSource?.cueId === cueId && authorizedSource.path === path
    ? authorizedSource.url
    : null;
  const intersecting = source !== null
    && intersection?.source === source
    && intersection.visible;

  useEffect(() => {
    useVideoPreviewTransport.getState().activate(identity, startMs, durationMs);
    // Same-identity activation refreshes trim/duration bounds while preserving the current preview position.
  }, [durationMs, identity, startMs]);

  useEffect(() => () => useVideoPreviewTransport.getState().deactivate(identity), [identity]);

  useEffect(() => {
    let stale = false;
    setFailed(false);
    setAuthorizedSource(null);

    prepareVideoPreview(cueId)
      .then((resolvedPath) => {
        if (stale) return;
        const url = toMediaAssetUrl(resolvedPath, convertFileSrc);
        if (url) setAuthorizedSource({ cueId, path, url });
        else setFailed(true);
      })
      .catch(() => {
        if (!stale) setFailed(true);
      });

    return () => { stale = true; };
  }, [cueId, path]);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;

    if (typeof IntersectionObserver === "undefined") {
      setIntersection(source ? { source, visible: true } : null);
      return;
    }

    const observer = new IntersectionObserver(
      ([entry]) => {
        if (source) {
          setIntersection({
            source,
            visible: entry.isIntersecting && entry.intersectionRatio > 0,
          });
        }
      },
      { threshold: 0.01 },
    );
    observer.observe(video);
    return () => observer.disconnect();
  }, [failed, source]);

  useEffect(() => {
    const updateVisibility = () => setDocumentVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", updateVisibility);
    return () => document.removeEventListener("visibilitychange", updateVisibility);
  }, []);

  useEffect(() => {
    useVideoPreviewTransport.getState().setVisible(
      identity,
      shouldPlayMediaPreview({ documentVisible, intersecting, failed }) && source !== null,
    );
  }, [documentVisible, failed, identity, intersecting, source]);

  useEffect(() => {
    const video = videoRef.current;
    if (!video || !source || !transport || video.readyState < HTMLMediaElement.HAVE_METADATA) return;
    const targetSeconds = transport.positionMs / 1000;
    if (Math.abs(video.currentTime - targetSeconds) > 0.012) {
      try { video.currentTime = targetSeconds; } catch { /* wait for metadata/canplay */ }
    }
  }, [identity, source, transport?.seekVersion]);

  useEffect(() => {
    const video = videoRef.current;
    if (!video || !source || !transport?.playing || !transport.ready || !transport.visible) {
      video?.pause();
      return;
    }

    let currentAttempt = true;
    void video.play().then(() => {
      const current = useVideoPreviewTransport.getState();
      if (videoRef.current !== video || current.identity !== identity || !current.playing || !current.visible) {
        video.pause();
      }
    }).catch(() => {
      if (currentAttempt) useVideoPreviewTransport.getState().stop(identity);
    });
    return () => { currentAttempt = false; };
  }, [identity, source, transport?.playing, transport?.ready, transport?.visible]);

  useEffect(() => {
    const video = videoRef.current as VideoWithFrameCallbacks | null;
    if (!video || !source || !transport?.playing || !transport.ready || !transport.visible || !video.requestVideoFrameCallback) {
      return;
    }
    let active = true;
    let callbackId: number | null = null;
    let previous: PreviewFrameMetadata | null = null;
    const onFrame = (_now: number, metadata: PreviewFrameMetadata) => {
      if (!active || videoRef.current !== video) return;
      const current = useVideoPreviewTransport.getState();
      if (current.identity !== identity || !current.playing || !current.visible) return;
      current.updateFromVideo(identity, metadata.mediaTime * 1000);
      if (previous) {
        const measuredRate = measurePreviewFrameRate(previous, metadata);
        if (measuredRate != null) current.setFrameRate(identity, measuredRate);
      }
      previous = metadata;
      callbackId = video.requestVideoFrameCallback!(onFrame);
    };
    callbackId = video.requestVideoFrameCallback(onFrame);
    return () => {
      active = false;
      if (callbackId != null) video.cancelVideoFrameCallback?.(callbackId);
    };
  }, [identity, source, transport?.playing, transport?.ready, transport?.visible]);

  useEffect(() => {
    const video = videoRef.current;
    return () => {
      // Stop decoding immediately when another cue/file replaces this preview.
      video?.pause();
      video?.removeAttribute("src");
      video?.load();
    };
  }, [failed, source]);

  if (failed) {
    return <MediaThumbnail path={path} seekInto />;
  }

  return (
    <div
      style={{
        marginBottom: 10,
        borderRadius: 4,
        overflow: "hidden",
        border: "1px solid var(--wc-border-strong)",
        background: "#000",
        minHeight: 90,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      {source ? (
        <video
          key={`${identity}\u0000${source}`}
          ref={videoRef}
          src={source}
          muted
          playsInline
          preload="metadata"
          onLoadedMetadata={(event) => {
            const video = event.currentTarget;
            if (videoRef.current !== video) return;
            let current = useVideoPreviewTransport.getState();
            if (video && current.identity === identity) {
              if (Number.isFinite(video.duration) && video.duration > 0) {
                current.setDurationFallback(identity, video.duration * 1000);
                current = useVideoPreviewTransport.getState();
              }
              try { video.currentTime = current.positionMs / 1000; } catch { /* source is not seekable yet */ }
            }
          }}
          onCanPlay={(event) => {
            if (videoRef.current === event.currentTarget) useVideoPreviewTransport.getState().setReady(identity, true);
          }}
          onTimeUpdate={(event) => {
            const video = event.currentTarget;
            if (videoRef.current === video) useVideoPreviewTransport.getState().updateFromVideo(identity, video.currentTime * 1000);
          }}
          onEnded={(event) => {
            const video = event.currentTarget;
            if (videoRef.current !== video) return;
            const state = useVideoPreviewTransport.getState();
            if (video) state.updateFromVideo(identity, video.currentTime * 1000);
            state.stop(identity);
          }}
          onError={(event) => {
            if (videoRef.current !== event.currentTarget) return;
            setFailed(true);
            const state = useVideoPreviewTransport.getState();
            state.setReady(identity, false);
            state.stop(identity);
          }}
          style={{ display: "block", width: "100%", maxHeight: 180, objectFit: "contain" }}
        />
      ) : (
        <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>{t("status.loading")}</span>
      )}
    </div>
  );
}
