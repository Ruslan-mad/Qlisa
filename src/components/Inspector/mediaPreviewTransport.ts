import { create } from "zustand";
import { clampPreviewPosition, DEFAULT_PREVIEW_FRAME_RATE, stepPreviewFrame } from "./mediaPreviewModel";

export interface VideoPreviewTransportState {
  identity: string | null;
  startMs: number;
  durationMs: number;
  positionMs: number;
  frameRate: number;
  /** A seek serial is separate from the clock position so timeupdate never seeks the element back. */
  seekVersion: number;
  mounted: boolean;
  ready: boolean;
  visible: boolean;
  playing: boolean;
  activate: (identity: string, startMs: number, durationMs: number) => void;
  deactivate: (identity: string) => void;
  setReady: (identity: string, ready: boolean) => void;
  setVisible: (identity: string, visible: boolean) => void;
  setDurationFallback: (identity: string, durationMs: number) => void;
  setFrameRate: (identity: string, frameRate: number) => void;
  seek: (identity: string, positionMs: number) => void;
  toggle: (identity: string) => void;
  stepFrame: (identity: string, direction: -1 | 1, frameRate?: number) => void;
  updateFromVideo: (identity: string, positionMs: number) => void;
  stop: (identity: string) => void;
}

const isAvailable = (state: VideoPreviewTransportState) =>
  state.mounted && state.ready && state.visible;

export const useVideoPreviewTransport = create<VideoPreviewTransportState>((set) => ({
  identity: null,
  startMs: 0,
  durationMs: 0,
  positionMs: 0,
  frameRate: DEFAULT_PREVIEW_FRAME_RATE,
  seekVersion: 0,
  mounted: false,
  ready: false,
  visible: false,
  playing: false,

  activate: (identity, startMs, durationMs) => set((state) => {
    if (state.identity === identity) {
      const nextDuration = Number.isFinite(durationMs) && durationMs > 0 ? durationMs : state.durationMs;
      const nextPosition = clampPreviewPosition(state.positionMs, nextDuration);
      return {
        ...state,
        mounted: true,
        startMs: clampPreviewPosition(startMs, nextDuration),
        durationMs: nextDuration,
        positionMs: nextPosition,
        seekVersion: state.seekVersion + (nextPosition === state.positionMs ? 0 : 1),
      };
    }
    const start = clampPreviewPosition(startMs, durationMs);
    return {
      ...state,
      identity,
      startMs: start,
      durationMs,
      positionMs: start,
      frameRate: DEFAULT_PREVIEW_FRAME_RATE,
      seekVersion: state.seekVersion + 1,
      mounted: true,
      ready: false,
      visible: false,
      playing: false,
    };
  }),

  deactivate: (identity) => set((state) => state.identity === identity
    ? {
      ...state,
      identity: null,
      positionMs: 0,
      frameRate: DEFAULT_PREVIEW_FRAME_RATE,
      seekVersion: state.seekVersion + 1,
      mounted: false,
      ready: false,
      visible: false,
      playing: false,
    }
    : state),

  setReady: (identity, ready) => set((state) => state.identity === identity
    ? { ...state, ready, playing: ready ? state.playing : false }
    : state),

  setVisible: (identity, visible) => set((state) => state.identity === identity
    ? { ...state, visible, playing: visible ? state.playing : false }
    : state),

  setDurationFallback: (identity, durationMs) => set((state) => {
    if (state.identity !== identity || state.durationMs > 0 || !Number.isFinite(durationMs) || durationMs <= 0) return state;
    const positionMs = clampPreviewPosition(state.positionMs, durationMs);
    return {
      ...state,
      durationMs,
      startMs: clampPreviewPosition(state.startMs, durationMs),
      positionMs,
      seekVersion: state.seekVersion + (positionMs === state.positionMs ? 0 : 1),
    };
  }),

  setFrameRate: (identity, frameRate) => set((state) =>
    state.identity === identity && Number.isFinite(frameRate) && frameRate >= 1 && frameRate <= 240
      ? { ...state, frameRate }
      : state),

  seek: (identity, positionMs) => set((state) => state.identity === identity
    ? {
      ...state,
      positionMs: clampPreviewPosition(positionMs, state.durationMs),
      seekVersion: state.seekVersion + 1,
    }
    : state),

  toggle: (identity) => set((state) => {
    if (state.identity !== identity || !isAvailable(state)) return state;
    if (state.playing) return { ...state, playing: false };
    const atEnd = state.durationMs > 0 && state.positionMs >= state.durationMs;
    return {
      ...state,
      positionMs: atEnd ? state.startMs : state.positionMs,
      seekVersion: state.seekVersion + (atEnd ? 1 : 0),
      playing: true,
    };
  }),

  stepFrame: (identity, direction, frameRate) => set((state) => {
    if (state.identity !== identity || !isAvailable(state)) return state;
    const positionMs = stepPreviewFrame(state.positionMs, direction, state.durationMs, frameRate ?? state.frameRate);
    return { ...state, positionMs, seekVersion: state.seekVersion + 1, playing: false };
  }),

  updateFromVideo: (identity, positionMs) => set((state) => state.identity === identity && state.playing
    ? { ...state, positionMs: clampPreviewPosition(positionMs, state.durationMs) }
    : state),

  stop: (identity) => set((state) => state.identity === identity && state.playing
    ? { ...state, playing: false }
    : state),
}));
