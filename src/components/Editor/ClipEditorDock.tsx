// Clip editor dock: a second inspector under the cue list for precise trim +
// slice editing on Audio and Video cues. Opened by the ⤢ button next to the
// inline waveform / filmstrip; audio cues can be auditioned in place.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AudioCueData, NumberCueData, SliceList, VideoCueData, WaveformData } from "../../lib/types";
import { PLAY_COUNT_INFINITE } from "../../lib/types";
import { getCue, getVideoFilmstrip, getVideoFilmstripRange, getWaveformPeaks, previewCueOnHeadphones, stopCuePreview, toggleCuePreview, seekCue, seekCueMedia, updateCue, setNumberActionOffset } from "../../lib/commands";
import { SliceTimeline } from "./SliceTimeline";
import { LiveTimeline, type NumberTimelineTrack } from "./LiveTimeline";
import type { TrimPainter, TrimView } from "../Inspector/TrimStrip";
import { useLocale } from "../../i18n";
import { useTimingStore } from "../../stores/timingStore";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { CLIP_EDITOR_TAB_LABELS, previewPlayheadForCue } from "../../lib/clipEditorPrefs";
import { findCueIsLoading, isCurrentClipLoad, isCurrentPreviewStart, previewUiForCue, shouldRetryWaveform, type CueBoundPreviewUi } from "../../lib/clipEditorLoadGuards";
import { formatInspectorSaveError, isCurrentCueRequest, persistThenCommit } from "../Inspector/singleCueSave";
import { normalizeNumberCueData } from "../Inspector/numberModel";
import { CueTypeIcon } from "../common/CueTypeIcon";
import { EditorErrorBoundary } from "../common/EditorErrorBoundary";
import { createMediaPreviewIdentity, hasBlockingPreviewOverlay, shouldStepPreviewFrame } from "../Inspector/mediaPreviewModel";
import { VideoPreviewControls } from "../Inspector/VideoPreviewControls";
import { NumberPreviewControls } from "../Inspector/NumberPreview";
import { useVideoPreviewTransport } from "../Inspector/mediaPreviewTransport";
import { applyNumberDragSnap, composeGroupWaveform, numberActionDuration, numberGroupTimelineActions, numberMasterDuration, numberVisualActions, snapNumberTime } from "./numberTimelineModel";
import { useNumberPreviewStore } from "../../stores/numberPreviewStore";
import {
  CLIP_EDITOR_BODY_PADDING_BOTTOM,
  CLIP_EDITOR_BODY_PADDING_TOP,
  CLIP_EDITOR_DOCK_HEIGHT,
  CLIP_EDITOR_DOCK_HEADER_HEIGHT,
  CLIP_EDITOR_TIMELINE_HEIGHT,
  splitVideoTimelineHeight,
} from "./clipEditorLayout";

const FILMSTRIP_TILES = 16;
const FILMSTRIP_TILE_WIDTH = 160;
const VIDEO_CONTROLS_SLOT_WIDTH = 98;
const HEADER_CONTROLS_SLOT_WIDTH = 138;

// Number data is loaded through the same dock, but its child-media fields are
// intentionally optional here because the ordinary clip editor does not use them.
type ClipCue = AudioCueData | VideoCueData | (NumberCueData & Partial<VideoCueData>);

type NumberEditableChild = NumberCueData["children"][number] & {
  start_time_ms?: number | null;
  end_time_ms?: number | null;
  display_duration_ms?: number | null;
};

function normalizeSlices(s: SliceList | undefined): SliceList {
  return { markers: s?.markers ?? [], play_counts: s?.play_counts ?? [1] };
}

function findCueState(cues: { id: string; state: string; children?: unknown[] }[], id: string): string | undefined {
  for (const cue of cues) {
    if (cue.id === id) return cue.state;
    if (Array.isArray(cue.children)) {
      const found = findCueState(cue.children as { id: string; state: string; children?: unknown[] }[], id);
      if (found) return found;
    }
  }
  return undefined;
}

function findCueTreeLoading(cues: { id: string; is_loading?: boolean; children?: unknown[] }[], id: string): boolean {
  for (const cue of cues) {
    if (cue.id === id) return Boolean(cue.is_loading) || (Array.isArray(cue.children)
      && findCueTreeChildrenLoading(cue.children as { id: string; is_loading?: boolean; children?: unknown[] }[]));
    if (Array.isArray(cue.children) && findCueTreeLoading(cue.children as { id: string; is_loading?: boolean; children?: unknown[] }[], id)) return true;
  }
  return false;
}

function findCueTreeChildrenLoading(cues: { id: string; is_loading?: boolean; children?: unknown[] }[]): boolean {
  return cues.some((cue) => Boolean(cue.is_loading)
    || (Array.isArray(cue.children) && findCueTreeChildrenLoading(cue.children as { id: string; is_loading?: boolean; children?: unknown[] }[])));
}

function loadImage(dataUrl: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = dataUrl;
  });
}

export function ClipEditorDock({
  cueId,
  onClose,
  onSaved,
  reloadToken,
  showLivePanel = true,
  showSlicePanel = true,
  activeTab: controlledActiveTab,
  onActiveTabChange,
}: {
  cueId: string | null;
  onClose: () => void;
  /** Called after every save so the cue list + inspector refresh. */
  onSaved: () => void;
  /** Bump to re-fetch the cue (the inspector edited it). */
  reloadToken?: number;
  /** View-menu visibility controls, persisted by the owning workspace. */
  showLivePanel?: boolean;
  showSlicePanel?: boolean;
  activeTab?: "Live" | "Slice";
  onActiveTabChange?: (tab: "Live" | "Slice") => void;
}) {
  const { t } = useLocale();
  const [cue, setCue] = useState<ClipCue | null>(null);
  const [waveform, setWaveform] = useState<WaveformData | null>(null);
  const [tiles, setTiles] = useState<HTMLImageElement[] | null>(null);
  const [numberAssets, setNumberAssets] = useState<Record<string, { waveform: WaveformData | null; tiles: HTMLImageElement[] | null }>>({});
  const [previewUi, setPreviewUi] = useState<CueBoundPreviewUi | null>(null);
  /** Set on first zoom ≥ 2× — swaps in high-resolution media data. */
  const [wantDetail, setWantDetail] = useState(false);
  /** Visible window while zoomed (video: drives the range filmstrip). */
  const [zoomView, setZoomView] = useState<TrimView | null>(null);
  /** Window-matched frames streamed in while zoomed. */
  const [rangeStrip, setRangeStrip] =
    useState<{ startMs: number; endMs: number; tiles: HTMLImageElement[] } | null>(null);
  const [localActiveTab, setLocalActiveTab] = useState<"Live" | "Slice">("Live");
  const previewActiveRef = useRef(false);
  const previewRequestRef = useRef(0);
  const previewTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cueLoadGenerationRef = useRef(0);
  const assetGenerationRef = useRef(0);
  const waveformRequestRef = useRef(0);
  const previousLoadingRef = useRef<{ cueId: string | null; isLoading: boolean | undefined }>({ cueId: null, isLoading: undefined });
  const saveGenerationRef = useRef(0);
  const saveCueIdRef = useRef(cueId);
  const [saveError, setSaveError] = useState<string | null>(null);
  const displayedCueIdRef = useRef(cueId);
  const timing = useTimingStore((state) => cueId ? state.timings[cueId] : undefined);
  const previewStoreEvent = useTimingStore((state) => state.previewPlayhead);
  const previewStoreGeneration = useTimingStore((state) => state.previewPlayheadGeneration);
  const previewPlayhead = previewPlayheadForCue(previewStoreEvent, cueId);
  const displayedPreviewUi = previewUiForCue(previewUi, cueId, previewStoreEvent, previewStoreGeneration);
  const previewVoice = displayedPreviewUi?.voiceId ?? null;
  const previewPosition = displayedPreviewUi?.positionMs ?? null;
  const previewError = displayedPreviewUi?.error ?? null;
  const runtimeCueState = useWorkspaceStore((state) => cueId ? findCueState(state.cues, cueId) : undefined);
  const cueIsLoading = useWorkspaceStore((state) => cueId ? findCueIsLoading(state.cues, cueId) : undefined);
  const cueTreeIsLoading = useWorkspaceStore((state) => cueId ? findCueTreeLoading(state.cues, cueId) : false);
  const isVideo = cue?.cue_type === "video";
  const isNumber = cue?.cue_type === "number";
  const numberPreviewPositionMs = useNumberPreviewStore((state) =>
    isNumber && state.numberId === cueId ? state.positionMs : 0,
  );
  const mediaIdentity = isVideo && cue?.file_path
    ? createMediaPreviewIdentity(cueId ?? "", cue.file_path)
    : null;
  const videoTransport = useVideoPreviewTransport((state) => state.identity === mediaIdentity ? state : null);
  const activeTab = controlledActiveTab ?? localActiveTab;
  const setActiveTab = (tab: "Live" | "Slice") => {
    setLocalActiveTab(tab);
    onActiveTabChange?.(tab);
  };

  useEffect(() => {
    if (!mediaIdentity || activeTab !== "Live" || !showLivePanel) {
      if (mediaIdentity) useVideoPreviewTransport.getState().stop(mediaIdentity);
    }
    return () => {
      if (mediaIdentity) useVideoPreviewTransport.getState().stop(mediaIdentity);
    };
  }, [activeTab, mediaIdentity, showLivePanel]);

  useEffect(() => {
    if (!mediaIdentity || !isVideo || activeTab !== "Live" || !showLivePanel) return;
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (
        event.defaultPrevented
        || event.isComposing
        || hasBlockingPreviewOverlay((selector) => document.querySelector(selector))
        || !target
        || !shouldStepPreviewFrame(event.key, target, event)
      ) return;
      const state = useVideoPreviewTransport.getState();
      if (state.identity !== mediaIdentity || !state.mounted || !state.ready || !state.visible) return;
      event.preventDefault();
      state.stepFrame(mediaIdentity, event.key === "ArrowLeft" ? -1 : 1, state.frameRate);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activeTab, isVideo, mediaIdentity, showLivePanel]);

  // The event/store, not a local promise result, owns whether the headphone
  // voice is still alive. A terminal event makes the button actionable before
  // another React effect runs; this effect then removes the stale local voice.
  useEffect(() => {
    const ui = previewUi;
    if (!ui || ui.cueId !== cueId || !ui.voiceId) return;
    if (previewStoreEvent?.active && previewStoreEvent.cue_id === cueId) {
      // A start command now provides this exact generation. Only bridge the
      // tiny pre-response interval; never let another session's event rewrite
      // an already-authoritative local session.
      if (ui.generation == null) {
        setPreviewUi((current) => current?.cueId === cueId && current.voiceId === ui.voiceId
          ? { ...current, generation: previewStoreEvent.generation }
          : current);
      }
      return;
    }
    // A terminal event can be the *first* event for a short file. The start
    // response already seeded its generation, so equality is both sufficient
    // and safe: an old terminal must never clear a newer preview.
    const endedUi = ui.generation != null && previewStoreGeneration === ui.generation;
    if (!endedUi) return;
    previewActiveRef.current = false;
    if (previewTimerRef.current) clearTimeout(previewTimerRef.current);
    previewTimerRef.current = null;
    setPreviewUi((current) => current?.cueId === ui.cueId
      && current.generation === ui.generation
      && current.voiceId === ui.voiceId
      ? { ...current, voiceId: null }
      : current);
  }, [cueId, previewUi, previewStoreEvent, previewStoreGeneration]);

  // Invalidate pending saves as soon as the dock target or authoritative
  // reload changes. The monotonic ticket also guards A→B→A selection races.
  useLayoutEffect(() => {
    saveCueIdRef.current = cueId;
    ++saveGenerationRef.current;
    setSaveError(null);
    return () => { ++saveGenerationRef.current; };
  }, [cueId, reloadToken]);

  // Fetch the cue JSON — again whenever the inspector saved it (reloadToken).
  useEffect(() => {
    const generation = ++cueLoadGenerationRef.current;
    let cancelled = false;
    setCue(null);
    if (!cueId) return () => { cancelled = true; };
    getCue(cueId)
      .then((data) => {
        if (!cancelled && isCurrentClipLoad(generation, cueLoadGenerationRef.current) && data.id === cueId) {
          // get_cue returns the persisted Rust field names for Numbers
          // (master_child_id/action_offsets_ms), while the timeline consumes
          // the frontend summary names. Normalize at this boundary so the
          // native Number timeline receives the selected master as well.
          const normalized = data.cue_type === "number"
            ? normalizeNumberCueData(data as NumberCueData & { master_child_id?: string | null; action_offsets_ms?: Record<string, number> })
            : data;
          setCue(normalized as ClipCue);
        }
      })
      .catch(() => {
        if (!cancelled && isCurrentClipLoad(generation, cueLoadGenerationRef.current)) onClose();
      }); // cue deleted — close the dock
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cueId, reloadToken]);

  // (Re)load the media preview when the target's file changes — not on every
  // cue re-fetch, so inspector edits don't re-decode the waveform/filmstrip.
  useEffect(() => {
    const generation = ++assetGenerationRef.current;
    let cancelled = false;
    setWaveform(null);
    setTiles(null);
    setWantDetail(false);
    setZoomView(null);
    setRangeStrip(null);
    const waveformRequest = ++waveformRequestRef.current;
    if (!cueId || !cue?.file_path) return;
    if (cue.cue_type === "video") {
      getWaveformPeaks(cueId, 2000)
        .then((data) => {
          if (!cancelled && waveformRequest === waveformRequestRef.current
            && isCurrentClipLoad(generation, assetGenerationRef.current)) setWaveform(data);
        })
        .catch(() => {
          if (!cancelled && waveformRequest === waveformRequestRef.current
            && isCurrentClipLoad(generation, assetGenerationRef.current)) {
            setWaveform({ peaks: [], rms: [], file_duration_s: 0 });
          }
        });
      getVideoFilmstrip(cue.file_path, FILMSTRIP_TILES, FILMSTRIP_TILE_WIDTH)
        .then((urls) => Promise.all(urls.map(loadImage)))
        .then((images) => {
          if (!cancelled && isCurrentClipLoad(generation, assetGenerationRef.current)) setTiles(images);
        })
        .catch(() => {
          if (!cancelled && isCurrentClipLoad(generation, assetGenerationRef.current)) setTiles([]);
        });
    } else {
      getWaveformPeaks(cueId, 2000)
        .then((data) => {
          if (!cancelled && isCurrentClipLoad(generation, assetGenerationRef.current)) setWaveform(data);
        })
        .catch(() => {
          if (!cancelled && isCurrentClipLoad(generation, assetGenerationRef.current)) setWaveform({ peaks: [], rms: [], file_duration_s: 0 });
        });
    }
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cueId, cue?.file_path, cue?.cue_type]);

  // Number tracks use the same decoded media primitives as the ordinary
  // timeline, but each child owns its waveform/filmstrip.
  // Use an explicit nested signature: some cue refresh paths retain the
  // parent object while replacing a trimmed child in-place, which otherwise
  // leaves the old filmstrip cached in this effect.
  const numberMediaSignature = cue?.cue_type === "number"
    ? JSON.stringify(cue.children, (_key, value) => {
      if (value && typeof value === "object") {
        const item = value as Record<string, unknown>;
        if ("id" in item || "cue_type" in item) {
          return {
            id: item.id,
            cue_type: item.cue_type,
            file_path: item.file_path,
            start_time_ms: item.start_time_ms,
            end_time_ms: item.end_time_ms,
            duration_ms: item.duration_ms,
            file_duration_ms: item.file_duration_ms,
            children: item.children,
          };
        }
      }
      return value;
    })
    : "";
  useEffect(() => {
    let cancelled = false;
    if (!cue || cue.cue_type !== "number") { setNumberAssets({}); return () => { cancelled = true; }; }
    // Do not keep painting the previous trim's media while the refreshed
    // child assets are being requested. This is important when a child is
    // edited from its own Inspector while its parent Number remains open.
    setNumberAssets({});
    const numberCue = cue as NumberCueData;
    const mediaChildren: typeof numberCue.children = [];
    const collectMediaChildren = (children: typeof numberCue.children) => {
      for (const child of children) {
        if (child.cue_type === "audio" || child.cue_type === "video") mediaChildren.push(child);
        else if (child.cue_type === "group") collectMediaChildren(child.children ?? []);
      }
    };
    collectMediaChildren(numberCue.children);
    Promise.all(mediaChildren.map(async (child) => {
        const waveform = await getWaveformPeaks(child.id, 1200).catch(() => null);
        const timedChild = child as typeof child & { start_time_ms?: number | null; end_time_ms?: number | null };
        const sourceStartMs = Math.max(0, timedChild.start_time_ms ?? 0);
        const sourceEndMs = Math.max(sourceStartMs, timedChild.end_time_ms ?? child.file_duration_ms ?? child.duration_ms ?? child.cached_duration_ms ?? 0);
        const tiles = child.cue_type === "video" && child.file_path
          ? await (sourceEndMs > sourceStartMs
            ? getVideoFilmstripRange(child.file_path, sourceStartMs / 1000, sourceEndMs / 1000, FILMSTRIP_TILES, FILMSTRIP_TILE_WIDTH)
            : getVideoFilmstrip(child.file_path, FILMSTRIP_TILES, FILMSTRIP_TILE_WIDTH))
            .then((urls) => Promise.all(urls.map(loadImage))).catch(() => null)
          : null;
        return [child.id, { waveform, tiles }] as const;
      }))
      .then((entries) => {
        if (cancelled) return;
        const assets = Object.fromEntries(entries) as Record<string, { waveform: WaveformData | null; tiles: HTMLImageElement[] | null }>;
        const addGroupAssets = (children: typeof numberCue.children) => {
          for (const child of children) {
            if (child.cue_type === "group") {
              addGroupAssets(child.children ?? []);
              // Compose after descendants so nested Group waveforms are
              // available in the asset map. The group's derived duration is
              // calculated by composeGroupWaveform when no stored duration
              // exists in the serialized summary.
              assets[child.id] = { waveform: composeGroupWaveform(child, Object.fromEntries(Object.entries(assets).map(([id, asset]) => [id, asset.waveform]))), tiles: null };
            }
          }
        };
        addGroupAssets(numberCue.children);
        setNumberAssets(assets);
      });
    return () => { cancelled = true; };
  }, [cue, cueTreeIsLoading, numberMediaSignature]);

  // A background media decode can still be running when the first waveform
  // request arrives. The workspace summary is the authoritative completion
  // signal; retry only for the selected cue's true → false edge, without
  // reloading its already decoded filmstrip or reacting to runtime state.
  useEffect(() => {
    const previous = previousLoadingRef.current;
    const retry = shouldRetryWaveform(previous.cueId, previous.isLoading, cueId, cueIsLoading);
    previousLoadingRef.current = { cueId, isLoading: cueIsLoading };
    if (!retry || !isVideo || !cue?.file_path || !cueId) return;

    const generation = assetGenerationRef.current;
    const request = ++waveformRequestRef.current;
    let cancelled = false;
    setWaveform(null);
    getWaveformPeaks(cueId, wantDetail ? 16000 : 2000)
      .then((data) => {
        if (!cancelled && request === waveformRequestRef.current
          && isCurrentClipLoad(generation, assetGenerationRef.current)) setWaveform(data);
      })
      .catch(() => {
        if (!cancelled && request === waveformRequestRef.current
          && isCurrentClipLoad(generation, assetGenerationRef.current)) {
          setWaveform({ peaks: [], rms: [], file_duration_s: 0 });
        }
      });
    return () => { cancelled = true; };
  }, [cueId, cueIsLoading, cue?.file_path, isVideo, wantDetail]);

  // Cue identity is a hard preview boundary. useLayoutEffect clears the old
  // button/position/error before the new cue can be clicked, and invalidates
  // in-flight frontend requests before asking the backend to stop its session.
  useEffect(() => {
    return () => {
      ++previewRequestRef.current;
      previewActiveRef.current = false;
      if (previewTimerRef.current) clearTimeout(previewTimerRef.current);
      previewTimerRef.current = null;
      void stopCuePreview().catch(() => {});
    };
  }, []);

  useLayoutEffect(() => {
    if (displayedCueIdRef.current === cueId) return;
    displayedCueIdRef.current = cueId;
    ++previewRequestRef.current;
    previewActiveRef.current = false;
    if (previewTimerRef.current) clearTimeout(previewTimerRef.current);
    previewTimerRef.current = null;
    setPreviewUi(null);
    void stopCuePreview().catch(() => {});
  }, [cueId]);

  // Hiding Live or switching to Slice is a lifecycle boundary for the
  // operator-only preview. It can never leak into the next selected cue.
  useEffect(() => {
    if (activeTab !== "Live" || !showLivePanel) {
      ++previewRequestRef.current;
      previewActiveRef.current = false;
      if (previewTimerRef.current) clearTimeout(previewTimerRef.current);
      previewTimerRef.current = null;
      setPreviewUi(null);
      void stopCuePreview().catch(() => {});
    }
  }, [activeTab, showLivePanel]);

  // Repair an active tab when the View menu hides it.
  useEffect(() => {
    if (activeTab === "Live" && !showLivePanel && showSlicePanel) setActiveTab("Slice");
    if (activeTab === "Slice" && !showSlicePanel && showLivePanel) setActiveTab("Live");
  }, [activeTab, showLivePanel, showSlicePanel]);

  // High-resolution swap once the operator zooms in: finer waveform bins /
  // a denser filmstrip (both cached, so this costs once per file).
  useEffect(() => {
    if (!cueId || !wantDetail || !cue?.file_path) return;
    const generation = assetGenerationRef.current;
    let cancelled = false;
    if (cue.cue_type === "video") {
      const waveformRequest = ++waveformRequestRef.current;
      getWaveformPeaks(cueId, 16000)
        .then((data) => {
          if (!cancelled && waveformRequest === waveformRequestRef.current
            && isCurrentClipLoad(generation, assetGenerationRef.current)) setWaveform(data);
        })
        .catch(() => {});
      getVideoFilmstrip(cue.file_path, 48, FILMSTRIP_TILE_WIDTH)
        .then((urls) => Promise.all(urls.map(loadImage)))
        .then((images) => {
          if (!cancelled && isCurrentClipLoad(generation, assetGenerationRef.current)) setTiles(images);
        })
        .catch(() => {});
    } else {
      getWaveformPeaks(cueId, 16000)
        .then((data) => {
          if (!cancelled && isCurrentClipLoad(generation, assetGenerationRef.current)) setWaveform(data);
        })
        .catch(() => {});
    }
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cueId, cue?.file_path, cue?.cue_type, wantDetail]);

  // Zoomed video: stream in frames for the visible window (debounced — waits
  // for the zoom/pan to settle; results are disk-cached on a ½ s grid).
  useEffect(() => {
    if (!isVideo || !zoomView || !cue?.file_path) return;
    const path = cue.file_path;
    const generation = assetGenerationRef.current;
    const startS = Math.max(0, Math.floor((zoomView.startMs / 1000) * 2) / 2);
    const endS = Math.ceil((zoomView.endMs / 1000) * 2) / 2;
    let stale = false;
    const timer = setTimeout(() => {
      getVideoFilmstripRange(path, startS, endS, 12, FILMSTRIP_TILE_WIDTH)
        .then((urls) => Promise.all(urls.map(loadImage)))
        .then((images) => {
          if (!stale && isCurrentClipLoad(generation, assetGenerationRef.current)) {
            setRangeStrip({ startMs: startS * 1000, endMs: endS * 1000, tiles: images });
          }
        })
        .catch(() => {});
    }, 300);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  }, [isVideo, zoomView, cue?.file_path]);

  const save = async (partial: Partial<ClipCue>) => {
    if (!cueId) return;
    const ticket = { cueId, generation: ++saveGenerationRef.current };
    const isCurrent = () => isCurrentCueRequest(ticket, saveGenerationRef.current, saveCueIdRef.current);
    await persistThenCommit(
      () => updateCue(ticket.cueId, partial),
      () => {
        if (!isCurrent()) return;
        setCue((prev) => (prev?.id === ticket.cueId ? ({ ...prev, ...partial } as ClipCue) : prev));
        setSaveError(null);
        onSaved();
      },
      (error) => {
        if (!isCurrent()) return;
        setSaveError(formatInspectorSaveError(error, false, t));
        // Keep the displayed cue authoritative after a rejection.
        void getCue(ticket.cueId).then((data) => {
          if (isCurrent() && data.id === ticket.cueId) setCue(data as ClipCue);
        }).catch(() => {});
      },
    );
  };

  // getCue returns the *serialized* cue: video duration lives in
  // cached_duration_ms there (file_duration_ms only exists on summaries).
  // If it is not populated yet, the inline preview's decoded metadata can supply it.
  const durationMs = isVideo
    ? (cue as VideoCueData | null)?.cached_duration_ms ??
      cue?.file_duration_ms ?? cue?.duration_ms ?? videoTransport?.durationMs ??
      ((waveform?.file_duration_s ?? 0) * 1000)
    : ((waveform?.file_duration_s ?? 0) * 1000 || cue?.file_duration_ms || cue?.duration_ms || 0);

  // Stable identity: a fresh object every render would retrigger the
  // timeline's reset effect and cancel drags / close the badge editor.
  const slices = useMemo(() => normalizeSlices(cue?.slices), [cue?.slices]);
  const videoPaintKey = useMemo(() => ({ tiles, waveform }), [tiles, waveform]);

  // ── Painters ─────────────────────────────────────────────────────────────
  const fileDurationS = waveform?.file_duration_s ?? 0;
  const paintWaveform = useCallback<TrimPainter>(
    (ctx, W, H, startX, endX, view) => {
      ctx.fillStyle = "#0f172a";
      ctx.fillRect(0, 0, W, H);
      const peaks = waveform?.peaks ?? [];
      const rms = waveform?.rms ?? [];
      if (peaks.length === 0) {
        // Number timelines paint each child (including composed Group media)
        // inside LiveTimeline. The selected Number itself has no source PCM,
        // so its root waveform must never claim that it is still loading.
        if (isNumber) return;
        ctx.fillStyle = "#475569";
        ctx.font = "12px sans-serif";
        ctx.textAlign = "center";
        ctx.fillText(waveform === null ? t("sweepUi.loadingWaveform") : t("sweepUi.noAudioData"), W / 2, H / 2);
        return;
      }
      ctx.fillStyle = "#0d2818";
      ctx.fillRect(startX, 0, endX - startX, H);
      const mid = H / 2;
      const amp = H * 0.46;
      const fileMs = fileDurationS * 1000;
      const viewSpan = view.endMs - view.startMs;
      // Bin index for a time (ms) — bins always cover the whole file.
      const binAt = (ms: number) =>
        fileMs > 0 ? Math.floor((ms / fileMs) * peaks.length) : 0;
      const rangeMax = (v: number[], a: number, b: number) => {
        let m = 0;
        for (let i = Math.max(0, a); i < b && i < v.length; i++) if (v[i] > m) m = v[i];
        return m;
      };
      for (let x = 0; x < W; x++) {
        const t0 = view.startMs + (x / W) * viewSpan;
        const t1 = view.startMs + ((x + 1) / W) * viewSpan;
        const from = binAt(t0);
        const to = Math.max(from + 1, binAt(t1));
        const peak = rangeMax(peaks, from, to);
        const body = rangeMax(rms, from, to);
        const inRegion = x >= startX && x <= endX;
        const peakH = Math.max(1, peak * amp);
        ctx.fillStyle = inRegion ? "#15803d" : "#14532d";
        ctx.fillRect(x, mid - peakH, 1, peakH * 2);
        if (body > 0) {
          const bodyH = Math.max(1, body * amp);
          ctx.fillStyle = inRegion ? "#4ade80" : "#166534";
          ctx.fillRect(x, mid - bodyH, 1, bodyH * 2);
        }
      }
      ctx.fillStyle = "rgba(74, 222, 128, 0.35)";
      ctx.fillRect(0, mid - 0.5, W, 1);
    },
    [waveform, fileDurationS, isNumber],
  );

  const paintFilmstrip = useCallback<TrimPainter>(
    (ctx, W, H, startX, endX, view) => {
      ctx.fillStyle = "#000";
      ctx.fillRect(0, 0, W, H);
      if (!tiles || tiles.length === 0) {
        ctx.fillStyle = "#475569";
        ctx.font = "12px sans-serif";
        ctx.textAlign = "center";
        ctx.fillText(tiles === null ? t("sweepUi.generatingPreview") : t("sweepUi.noPreview"), W / 2, H / 2);
        return;
      }
      // Tiles are time-anchored: map each tile's time range into the visible
      // window so zooming stays aligned.
      const viewSpan = view.endMs - view.startMs;
      const drawStrip = (
        strip: HTMLImageElement[],
        stripStartMs: number,
        stripEndMs: number,
      ) => {
        const tileMs = (stripEndMs - stripStartMs) / strip.length;
        for (let i = 0; i < strip.length; i++) {
          const t0 = stripStartMs + i * tileMs;
          const x0 = ((t0 - view.startMs) / viewSpan) * W;
          const x1 = ((t0 + tileMs - view.startMs) / viewSpan) * W;
          if (x1 < 0 || x0 > W) continue;
          const img = strip[i];
          const cellW = x1 - x0;
          const scale = Math.max(cellW / img.width, H / img.height);
          const dw = img.width * scale;
          const dh = img.height * scale;
          ctx.save();
          ctx.beginPath();
          ctx.rect(x0, 0, cellW, H);
          ctx.clip();
          ctx.drawImage(img, x0 + (cellW - dw) / 2, (H - dh) / 2, dw, dh);
          ctx.restore();
        }
      };
      drawStrip(tiles, 0, durationMs);
      // Window-matched frames on top while zoomed in (crisper than the
      // stretched whole-file tiles).
      if (rangeStrip && rangeStrip.tiles.length > 0 && viewSpan < durationMs) {
        drawStrip(rangeStrip.tiles, rangeStrip.startMs, rangeStrip.endMs);
      }
      ctx.fillStyle = "rgba(0, 0, 0, 0.65)";
      ctx.fillRect(0, 0, startX, H);
      ctx.fillRect(endX, 0, W - endX, H);
    },
    [tiles, durationMs, rangeStrip],
  );

  const paintVideoTimeline = useCallback<TrimPainter>(
    (ctx, W, H, startX, endX, view) => {
      const { filmstripHeight, waveformHeight } = splitVideoTimelineHeight(H);
      ctx.save();
      ctx.beginPath();
      ctx.rect(0, 0, W, filmstripHeight);
      ctx.clip();
      paintFilmstrip(ctx, W, filmstripHeight, startX, endX, view);
      ctx.restore();

      ctx.save();
      ctx.translate(0, filmstripHeight);
      paintWaveform(ctx, W, waveformHeight, startX, endX, view);
      ctx.restore();

      // Keep the two media bands visually distinct without consuming any
      // additional canvas height or affecting the shared overlay geometry.
      ctx.fillStyle = "#334155";
      ctx.fillRect(0, filmstripHeight - 1, W, 1);
    },
    [paintFilmstrip, paintWaveform],
  );

  const dragPreview = useCallback(
    (ms: number) => {
      if (!tiles || tiles.length === 0 || durationMs <= 0) return null;
      const idx = Math.min(
        tiles.length - 1,
        Math.max(0, Math.round((ms / durationMs) * (tiles.length - 1))),
      );
      return (
        <div style={{ background: "#000", border: "1px solid var(--wc-border-strong)", borderRadius: 4, overflow: "hidden", boxShadow: "0 4px 16px rgba(0,0,0,0.6)" }}>
          <img src={tiles[idx].src} alt="" style={{ display: "block", width: 200 }} />
          <div style={{ textAlign: "center", fontSize: 11, padding: "2px 0", color: "var(--wc-text)", fontVariantNumeric: "tabular-nums", background: "var(--wc-bg-deepest)" }}>
            {(ms / 1000).toFixed(3)}s
          </div>
        </div>
      );
    },
    [tiles, durationMs],
  );

  // ── Dedicated headphone preview (never the main program output) ───────────
  const previewAt = async (positionMs: number) => {
    if (!cueId || !cue) return;
    const request = ++previewRequestRef.current;
    const ticket = { requestSequence: request, cueId };
    const clamped = Math.max(0, Math.min(durationMs || Number.MAX_SAFE_INTEGER, Math.round(positionMs)));
    setPreviewUi((prev) => ({
      ...(prev?.cueId === cueId ? prev : { cueId, voiceId: null, positionMs: null, error: null }),
      generation: null,
      error: null,
    }));
    try {
      const started = await previewCueOnHeadphones(cueId, clamped, cue?.end_time_ms ?? undefined);
      // The old voice can reach EOF while this replacement decodes. That only
      // ends the previous generation; the ticket is cancelled exclusively by
      // a newer request, explicit stop, hide, or displayed-cue change.
      if (!isCurrentPreviewStart(ticket, previewRequestRef.current, displayedCueIdRef.current)) {
        return;
      }
      previewActiveRef.current = true;
      setPreviewUi((prev) => ({
        ...(prev?.cueId === cueId ? prev : { cueId, voiceId: null, positionMs: null, error: null }),
        voiceId: started.voice_id,
        generation: started.generation,
        error: null,
      }));
    } catch (error) {
      if (request !== previewRequestRef.current) return;
      previewActiveRef.current = false;
      setPreviewUi((prev) => ({
        ...(prev?.cueId === cueId ? prev : { cueId, voiceId: null, positionMs: null, error: null }),
        voiceId: null,
        error: error instanceof Error ? error.message : String(error),
      }));
    }
  };

  const schedulePreviewAt = (positionMs: number, immediate = false) => {
    if (previewTimerRef.current) clearTimeout(previewTimerRef.current);
    if (immediate) {
      void previewAt(positionMs);
      return;
    }
    previewTimerRef.current = setTimeout(() => {
      previewTimerRef.current = null;
      void previewAt(positionMs);
    }, 120);
  };

  const toggleAudition = async () => {
    if (!cueId || !cue) return;
    if (previewVoice) {
      ++previewRequestRef.current;
      previewActiveRef.current = false;
      if (previewTimerRef.current) clearTimeout(previewTimerRef.current);
      previewTimerRef.current = null;
      setPreviewUi((prev) => prev?.cueId === cueId ? { ...prev, voiceId: null, error: null } : prev);
      await stopCuePreview().catch(() => {});
      return;
    }
    const request = ++previewRequestRef.current;
    setPreviewUi((prev) => ({
      ...(prev?.cueId === cueId ? prev : { cueId, voiceId: null, positionMs: null, error: null }),
      voiceId: null,
      generation: null,
      error: null,
    }));
    try {
      const start = timing?.media_position_ms ?? previewPosition ?? cue?.start_time_ms ?? 0;
      const result = await toggleCuePreview(cueId, start, cue?.end_time_ms ?? undefined);
      if (request !== previewRequestRef.current) return;
      if (result.playing && result.voice_id) {
        previewActiveRef.current = true;
        setPreviewUi((prev) => ({
          ...(prev?.cueId === cueId ? prev : { cueId, voiceId: null, positionMs: null, error: null }),
          voiceId: result.voice_id!,
          generation: result.generation ?? null,
          error: null,
        }));
      } else {
        previewActiveRef.current = false;
        setPreviewUi((prev) => prev?.cueId === cueId ? { ...prev, voiceId: null, error: null } : prev);
      }
    } catch (error) {
      if (request !== previewRequestRef.current) return;
      setPreviewUi((prev) => ({
        ...(prev?.cueId === cueId ? prev : { cueId, voiceId: null, positionMs: null, error: null }),
        voiceId: null,
        error: error instanceof Error ? error.message : String(error),
      }));
    }
  };

  const tabControls = (
    <div role="tablist" aria-label="Clip editor view" style={{ display: "flex", gap: 2 }}>
      {showLivePanel && (
        <button
          type="button" role="tab" aria-selected={activeTab === "Live"}
          onClick={() => setActiveTab("Live")}
          style={tabButton(activeTab === "Live")}
        >{CLIP_EDITOR_TAB_LABELS.timeline}</button>
      )}
      {showSlicePanel && !isNumber && (
        <button
          type="button" role="tab" aria-selected={activeTab === "Slice"}
          onClick={() => setActiveTab("Slice")}
          style={tabButton(activeTab === "Slice")}
        >{CLIP_EDITOR_TAB_LABELS.slice}</button>
      )}
    </div>
  );

  const numberCue = isNumber && cue ? cue as NumberCueData : null;
  const timelineDurationMs = numberCue ? numberMasterDuration(numberCue) : durationMs;
  const numberHasVisualPreview = !!numberCue?.children.some((child) =>
    (child.cue_type === "video" || child.cue_type === "image") && !!child.file_path,
  );
  useEffect(() => {
    if (!numberCue) {
      if (cueId) useNumberPreviewStore.getState().clear(cueId);
      return;
    }
    useNumberPreviewStore.getState().select(numberCue.id, numberMasterDuration(numberCue));
  }, [numberCue?.id, timelineDurationMs]);

  const numberTracks = useMemo<NumberTimelineTrack[]>(() => {
    if (!numberCue) return [];
    const master = numberCue.children.find((child) => child.id === numberCue.number_master_id);
    const actions = numberVisualActions(numberCue);
    const groupTracks = (group: typeof numberCue.children[number], baseMs: number, limitMs: number, prefix: string): NumberTimelineTrack[] => {
      if (group.cue_type !== "group") return [];
      const segments = numberGroupTimelineActions(group, baseMs, limitMs, prefix).map((action) => ({
        id: action.id,
        name: action.name,
        startMs: action.timeline_start_ms ?? baseMs,
        endMs: action.timeline_end_ms ?? baseMs,
        cueType: action.cue_type as NumberTimelineTrack["cueType"],
        media: numberAssets[action.id],
        sourceStartMs: Math.max(0, action.start_time_ms ?? 0),
        sourceEndMs: Math.max(0, action.end_time_ms ?? action.file_duration_ms ?? action.duration_ms ?? action.cached_duration_ms ?? 0),
        maxDurationMs: numberActionDuration(action, limitMs),
        looped: action.looped,
        readonly: true,
      }));
      return [{
        id: group.id,
        name: `Group · ${group.name}`,
        startMs: baseMs,
        endMs: limitMs,
        cueType: "group" as const,
        readonly: true,
        segments,
      }];
    };
    const masterTracks = master?.cue_type === "group"
      ? groupTracks(master, 0, timelineDurationMs, master.name)
      : master
        ? [{ id: master.id, name: master.name, startMs: 0, endMs: numberMasterDuration(numberCue), master: true, cueType: master.cue_type as NumberTimelineTrack["cueType"], media: numberAssets[master.id] }]
        : [];
    const actionTracks: NumberTimelineTrack[] = [];
    for (const action of actions) {
      if (action.cue_type === "group") {
        actionTracks.push(...groupTracks(action, action.timeline_start_ms ?? 0, action.timeline_end_ms ?? timelineDurationMs, action.name));
        continue;
      }
        const sourceStartMs = Math.max(0, action.start_time_ms ?? 0);
        const fileDurationMs = Math.max(0, action.file_duration_ms ?? action.duration_ms ?? action.cached_duration_ms ?? 0);
        const sourceDurationMs = fileDurationMs;
        actionTracks.push({
          id: action.id,
          name: action.name,
          startMs: action.timeline_start_ms ?? 0,
          endMs: action.timeline_end_ms ?? 0,
          cueType: action.cue_type as NumberTimelineTrack["cueType"],
          media: numberAssets[action.id],
          sourceStartMs,
          sourceEndMs: Math.max(sourceStartMs, action.end_time_ms ?? sourceDurationMs),
          maxDurationMs: Math.max(0, sourceDurationMs - sourceStartMs),
          looped: action.looped,
        });
    }
    return [...masterTracks, ...actionTracks];
  }, [numberCue, numberAssets]);

  const applyNumberTrackEdit = useCallback((
    numberId: string,
    actionId: string,
    kind: "move" | "start" | "end",
    startMs: number,
    endMs: number,
    action: ReturnType<typeof numberVisualActions>[number],
  ) => {
    setCue((previous) => {
      if (!previous || previous.cue_type !== "number" || previous.id !== numberId) return previous;
      const sourceStart = Math.max(0, action.start_time_ms ?? 0);
      const timelineStart = action.timeline_start_ms ?? 0;
      const nextChildren = (previous.children ?? []).map((child) => {
        if (child.id !== actionId) return child;
        const next = { ...(child as NumberEditableChild) };
        if (kind === "start") next.start_time_ms = Math.max(0, Math.round(sourceStart + startMs - timelineStart));
        if (kind === "end") next.end_time_ms = Math.max(0, Math.round(sourceStart + (endMs - startMs)));
        if (action.cue_type === "image" && kind !== "move") next.display_duration_ms = Math.max(0, Math.round(endMs - startMs));
        return next;
      });
      const nextOffsets = { ...(previous.number_action_offsets_ms ?? {}) };
      if (kind === "move" || kind === "start") nextOffsets[actionId] = Math.round(startMs);
      return { ...previous, children: nextChildren, number_action_offsets_ms: nextOffsets };
    });
  }, []);

  if (!cue || cue.id !== cueId) {
    return (
      <div style={dockShell}>
        <div style={dockHeader}>
          <span aria-hidden="true" style={{ width: HEADER_CONTROLS_SLOT_WIDTH, flex: `0 0 ${HEADER_CONTROLS_SLOT_WIDTH}px` }} />
          <span style={{ color: "var(--wc-text-muted)", fontSize: 12 }}>{t("editorUi.emptyCueTitle")}</span>
          <span style={{ flex: 1 }} />
          {tabControls}
          <button onClick={onClose} style={headerBtn} title={t("editorUi.closeEditor")}>✕</button>
        </div>
        <div style={dockBody}>
          {t("editorUi.emptyCueMessage")}
        </div>
      </div>
    );
  }

  const hasVamp = slices.play_counts.some((c) => c === PLAY_COUNT_INFINITE);
  if (!cueId) return null;
  const mediaPositionMs = isNumber
    ? timing?.action_elapsed_ms ?? null
    : timing?.media_position_ms ?? null;
  // Keep the silent inline-video cursor separate from both the main transport
  // playhead and the configured-headphone audition cursor.
  const previewPositionMs = isNumber
    ? numberPreviewPositionMs
    : isVideo ? videoTransport?.positionMs ?? null : previewPlayhead?.media_position_ms ?? null;
  // Cue data fetched when the dock opened can be stale after GO/PAUSE. Timing
  // samples are the authoritative running/paused signal and remain present on
  // pause; the store state covers the short interval before the first sample.
  // A retained timing sample is enough for ordinary media, but it is not a
  // safe seek authority for a Number: after a stop the children may already
  // be gone while the last sample is still in the store. In that state a
  // timeline click is an inspector-preview seek, never a runtime seek.
  const seekEnabled = isNumber
    ? runtimeCueState === "running" || runtimeCueState === "paused"
    : timing != null || runtimeCueState === "running" || runtimeCueState === "paused";

  if (!showLivePanel && !showSlicePanel) return null;

  return (
    <div style={dockShell}>
      {/* Header */}
      <div style={dockHeader}>
        <div style={controlsSlot}>
          {activeTab === "Live" || isNumber ? (
            <>
              {mediaIdentity
                ? <div style={videoControlsGroup}><VideoPreviewControls identity={mediaIdentity} buttonStyle={headerBtn} /></div>
                : isNumber
                  ? <div style={videoControlsGroup}><NumberPreviewControls numberId={cueId} durationMs={timelineDurationMs} available={numberHasVisualPreview} buttonStyle={headerBtn} /></div>
                : <span aria-hidden="true" style={{ display: "block", width: VIDEO_CONTROLS_SLOT_WIDTH, flex: `0 0 ${VIDEO_CONTROLS_SLOT_WIDTH}px` }} />}
              <button
                onClick={() => void toggleAudition()}
                style={{ ...headerBtn, color: previewVoice ? "#4ade80" : "var(--wc-text)" }}
                title={previewVoice ? t("editorUi.stopPreviewHeadphones") : t("editorUi.previewHeadphones")}
                aria-label={previewVoice ? t("editorUi.stopPreviewHeadphones") : t("editorUi.previewHeadphones")}
              >
                {previewVoice ? "🎧 ■" : "🎧"}
              </button>
            </>
          ) : null}
        </div>
        <div style={cueTitleGroup}>
          <CueTypeIcon type={isNumber ? "number" : isVideo ? "video" : "audio"} size={16} tone="neutral" />
          <span title={cue.name} style={cueTitle}>{cue.name}</span>
          {cue.number && (
            <span style={{ flexShrink: 0, fontSize: 11, color: "var(--wc-text-muted)" }}>#{cue.number}</span>
          )}
          <span style={{ flexShrink: 0, fontSize: 11, color: "var(--wc-text-muted)" }}>
            {timelineDurationMs > 0 ? `${(timelineDurationMs / 1000).toFixed(2)}s` : ""}
          </span>
        </div>
        {previewError && (
          <span style={headerMessage} title={previewError}>
            {previewError}
          </span>
        )}
        {hasVamp && (
          <span style={{ ...headerMessage, color: "#facc15" }} title={t("editorUi.vamps")}>
            ∞ vamp — release with a Devamp Cue
          </span>
        )}
        {saveError && (
          <span role="alert" style={headerMessage} title={saveError}>
            {saveError}
          </span>
        )}
        <span style={{ flex: 1 }} />
        {activeTab === "Slice" && !isNumber && (
          <span style={{ fontSize: 11, color: "var(--wc-text-faint)" }}>
            Double-click: add slice · drag ▾: move · right-click: remove · badge: play count
          </span>
        )}
        {tabControls}
        <button onClick={onClose} style={headerBtn} title={t("editorUi.closeEditor")}>✕</button>
      </div>

      {/* Timeline */}
      <div style={dockTimelineBody}>
        {timelineDurationMs > 0 ? (
          activeTab === "Live" || isNumber ? (
            <EditorErrorBoundary label="Таймлайн временно недоступен">
            <LiveTimeline
              durationMs={timelineDurationMs}
              startMs={cue.start_time_ms ?? null}
              endMs={cue.end_time_ms ?? null}
              slices={slices}
              height={CLIP_EDITOR_TIMELINE_HEIGHT}
              paint={isVideo ? paintVideoTimeline : paintWaveform}
              paintKey={isVideo ? videoPaintKey : waveform}
              resetKey={cue.file_path}
              mediaPositionMs={mediaPositionMs}
              previewPositionMs={previewPositionMs}
              seekEnabled={seekEnabled}
              numberPreWaitMs={isNumber ? cue.pre_wait_ms : 0}
              numberPostWaitMs={isNumber ? cue.post_wait_ms : 0}
              numberRuntimeElapsedMs={isNumber ? timing?.elapsed_ms ?? null : null}
              onSeek={(ms) => {
                const targetCueId = cueId;
                if (isNumber) {
                  // Number's master audio and visual children share one
                  // clock. Always move the local cursor; only a live/paused
                  // Number is allowed to receive a runtime seek command.
                  useNumberPreviewStore.getState().setPosition(targetCueId, ms, timelineDurationMs);
                  if (!seekEnabled) return;
                  // Tauri deserializes Rust `u64` arguments strictly. The
                  // canvas maps a pointer to a fractional millisecond, so
                  // normalize it before crossing the command boundary.
                  void seekCue(targetCueId, Math.max(0, Math.round(ms))).catch((err) => {
                    if (displayedCueIdRef.current !== targetCueId) return;
                    setPreviewUi((prev) => ({
                      ...(prev?.cueId === targetCueId ? prev : { cueId: targetCueId, voiceId: null, positionMs: null, error: null }),
                      error: String(err),
                    }));
                  });
                  return;
                }
                if (mediaIdentity) useVideoPreviewTransport.getState().seek(mediaIdentity, ms);
                const seek = seekCueMedia(targetCueId, Math.max(0, Math.round(ms)));
                void seek.catch((err) => {
                  if (displayedCueIdRef.current !== targetCueId) return;
                  setPreviewUi((prev) => ({
                    ...(prev?.cueId === targetCueId ? prev : { cueId: targetCueId, voiceId: null, positionMs: null, error: null }),
                    error: String(err),
                  }));
                });
                if (!isVideo && previewActiveRef.current) schedulePreviewAt(ms);
              }}
              onSeekEnd={(ms) => { if (!isVideo && previewActiveRef.current) schedulePreviewAt(ms, true); }}
              onPreviewPosition={(ms) => {
                if (isNumber) {
                  useNumberPreviewStore.getState().setPosition(cueId, ms, timelineDurationMs);
                  return;
                }
                if (isVideo && mediaIdentity) {
                  useVideoPreviewTransport.getState().seek(mediaIdentity, ms);
                  return;
                }
                setPreviewUi((prev) => ({
                  ...(prev?.cueId === cueId ? prev : { cueId, voiceId: null, positionMs: null, error: null }),
                  positionMs: ms,
                }));
                if (previewActiveRef.current) schedulePreviewAt(ms);
              }}
              onPreviewPositionEnd={(ms) => {
                if (isNumber) {
                  useNumberPreviewStore.getState().setPosition(cueId, ms, timelineDurationMs);
                  return;
                }
                if (!isVideo && previewActiveRef.current) schedulePreviewAt(ms, true);
              }}
              onZoomDetail={() => setWantDetail(true)}
              onViewChange={setZoomView}
              numberTracks={numberTracks}
              onNumberTrackSnap={(track, _kind, rawStart, rawEnd) => {
                const edges = numberTracks.filter((item) => item.id !== track.id).flatMap((item) => [item.startMs, item.endMs]);
                const threshold = Math.max(1, timelineDurationMs * 9 / 600);
                const snappedStart = snapNumberTime(rawStart, timelineDurationMs, edges, threshold);
                const snappedEnd = snapNumberTime(rawEnd, timelineDurationMs, edges, threshold);
                return applyNumberDragSnap(_kind, { startMs: rawStart, endMs: rawEnd }, { startMs: snappedStart, endMs: snappedEnd });
              }}
              onNumberTrackChange={(track, kind, rawStart, rawEnd) => {
                if (!numberCue || track.master) return;
                const edges = numberTracks.filter((item) => item.id !== track.id).flatMap((item) => [item.startMs, item.endMs]);
                // Approximate the native canvas 9px snap radius at its usual
                // 600px width while keeping it proportional to clip length.
                const threshold = Math.max(1, timelineDurationMs * 9 / 600);
                const snappedStart = snapNumberTime(rawStart, timelineDurationMs, edges, threshold);
                const snappedEnd = snapNumberTime(rawEnd, timelineDurationMs, edges, threshold);
                const snapped = applyNumberDragSnap(kind, { startMs: rawStart, endMs: rawEnd }, { startMs: snappedStart, endMs: snappedEnd });
                const start = Math.max(0, Math.min(timelineDurationMs, snapped.startMs));
                const end = Math.max(start, Math.min(timelineDurationMs, snapped.endMs));
                const action = numberVisualActions(numberCue).find((item) => item.id === track.id);
                if (!action) return;
                applyNumberTrackEdit(numberCue.id, action.id, kind, start, end, action);
                void (async () => {
                  try {
                    if (kind === "move" || kind === "start") await setNumberActionOffset(numberCue.id, action.id, start);
                    if (kind !== "move") {
                      if (action.cue_type === "image") await updateCue(action.id, { display_duration_ms: Math.max(0, end - start) });
                      else {
                        const sourceStart = Math.max(0, action.start_time_ms ?? 0);
                        const sourceDelta = start - (action.timeline_start_ms ?? 0);
                        await updateCue(action.id, kind === "start"
                          ? { start_time_ms: Math.max(0, Math.round(sourceStart + sourceDelta)) }
                          : { end_time_ms: Math.max(0, Math.round(sourceStart + (end - start))) });
                      }
                    }
                    const refreshed = await getCue(numberCue.id);
                    if (displayedCueIdRef.current === numberCue.id && refreshed.id === numberCue.id) {
                      const normalized = normalizeNumberCueData(refreshed as NumberCueData & { master_child_id?: string | null; action_offsets_ms?: Record<string, number> });
                      setCue(normalized as ClipCue);
                    }
                    setSaveError(null);
                    onSaved();
                  } catch (error) {
                    setSaveError(formatInspectorSaveError(error, false, t));
                    const refreshed = await getCue(numberCue.id).catch(() => null);
                    if (refreshed && displayedCueIdRef.current === numberCue.id) {
                      setCue(normalizeNumberCueData(refreshed as NumberCueData & { master_child_id?: string | null; action_offsets_ms?: Record<string, number> }) as ClipCue);
                    }
                  }
                })();
              }}
            />
            </EditorErrorBoundary>
          ) : (
            <SliceTimeline
              durationMs={durationMs}
              startMs={cue.start_time_ms ?? null}
              endMs={cue.end_time_ms ?? null}
              slices={slices}
              height={CLIP_EDITOR_TIMELINE_HEIGHT}
              paint={isVideo ? paintVideoTimeline : paintWaveform}
              paintKey={isVideo ? videoPaintKey : waveform}
              onCommitStart={(ms) => save({ start_time_ms: ms })}
              onCommitEnd={(ms) => save({ end_time_ms: ms })}
              onSlicesChange={(s) => save({ slices: s })}
              dragPreview={isVideo ? dragPreview : undefined}
              onZoomDetail={() => setWantDetail(true)}
              onViewChange={setZoomView}
            />
          )
        ) : (
          <div style={{ color: "var(--wc-text-faint)", fontSize: 12, padding: 16, textAlign: "center" }}>
            {cue.file_path ? t("sweepUi.waitingMedia") : t("sweepUi.noFileAssigned")}
          </div>
        )}
      </div>
    </div>
  );
}

const dockShell: React.CSSProperties = {
  borderTop: "1px solid var(--wc-border)",
  background: "var(--wc-bg-app)",
  display: "flex",
  flexDirection: "column",
  height: CLIP_EDITOR_DOCK_HEIGHT,
  minHeight: CLIP_EDITOR_DOCK_HEIGHT,
  maxHeight: CLIP_EDITOR_DOCK_HEIGHT,
  flex: `0 0 ${CLIP_EDITOR_DOCK_HEIGHT}px`,
  boxSizing: "border-box",
  overflow: "hidden",
};

const dockHeader: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 10,
  padding: "6px 12px",
  height: CLIP_EDITOR_DOCK_HEADER_HEIGHT,
  minHeight: CLIP_EDITOR_DOCK_HEADER_HEIGHT,
  boxSizing: "border-box",
  flexShrink: 0,
  borderBottom: "1px solid var(--wc-border)",
  background: "var(--wc-bg-deepest)",
};

const dockBody: React.CSSProperties = {
  flex: 1,
  minHeight: 0,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  color: "var(--wc-text-faint)",
  fontSize: 12,
  overflow: "hidden",
};

const dockTimelineBody: React.CSSProperties = {
  flex: 1,
  minHeight: 0,
  padding: `${CLIP_EDITOR_BODY_PADDING_TOP}px 12px ${CLIP_EDITOR_BODY_PADDING_BOTTOM}px`,
  boxSizing: "border-box",
  overflow: "hidden",
};

const controlsSlot: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 4,
  flex: `0 0 ${HEADER_CONTROLS_SLOT_WIDTH}px`,
  width: HEADER_CONTROLS_SLOT_WIDTH,
  minWidth: HEADER_CONTROLS_SLOT_WIDTH,
};

const videoControlsGroup: React.CSSProperties = {
  width: VIDEO_CONTROLS_SLOT_WIDTH,
  flex: `0 0 ${VIDEO_CONTROLS_SLOT_WIDTH}px`,
};

const cueTitleGroup: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 7,
  flex: "1 1 auto",
  minWidth: 0,
  fontWeight: 600,
  fontSize: 13,
  overflow: "hidden",
};

const cueTitle: React.CSSProperties = {
  flex: "1 1 auto",
  minWidth: 0,
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
};

const headerMessage: React.CSSProperties = {
  flex: "0 1 auto",
  minWidth: 0,
  maxWidth: 180,
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  fontSize: 11,
  color: "#f87171",
};

const headerBtn: React.CSSProperties = {
  background: "var(--wc-bg-surface)",
  border: "1px solid var(--wc-border-strong)",
  borderRadius: 4,
  color: "var(--wc-text)",
  cursor: "pointer",
  fontSize: 11,
  padding: "2px 8px",
};

const tabButton = (active: boolean): React.CSSProperties => ({
  background: active ? "var(--wc-accent)" : "var(--wc-bg-surface)",
  border: `1px solid ${active ? "var(--wc-accent)" : "var(--wc-border-strong)"}`,
  borderRadius: 4,
  color: active ? "var(--wc-accent-fg)" : "var(--wc-text-secondary)",
  cursor: "pointer",
  fontSize: 11,
  fontWeight: active ? 600 : 400,
  padding: "2px 9px",
});
