import { create } from "zustand";

/**
 * The Number clock is shared by the editor timeline and the Inspector
 * preview. Keeping it outside either panel prevents the preview from becoming
 * a second, disconnected timeline.
 */
interface NumberPreviewState {
  numberId: string | null;
  positionMs: number;
  playing: boolean;
  select: (numberId: string, durationMs: number) => void;
  setPosition: (numberId: string, positionMs: number, durationMs?: number) => void;
  setPlaying: (numberId: string, playing: boolean) => void;
  toggle: (numberId: string, durationMs: number) => void;
  stepFrame: (numberId: string, direction: -1 | 1, frameRate?: number, durationMs?: number) => void;
  clear: (numberId?: string) => void;
}

const clamp = (positionMs: number, durationMs: number) =>
  Math.max(0, Math.min(Number.isFinite(durationMs) && durationMs > 0 ? durationMs : Number.MAX_SAFE_INTEGER,
    Number.isFinite(positionMs) ? positionMs : 0));

export const useNumberPreviewStore = create<NumberPreviewState>((set) => ({
  numberId: null,
  positionMs: 0,
  playing: false,

  select: (numberId, durationMs) => set((state) => ({
    numberId,
    positionMs: state.numberId === numberId ? clamp(state.positionMs, durationMs) : 0,
    playing: state.numberId === numberId ? state.playing : false,
  })),

  setPosition: (numberId, positionMs, durationMs = Number.MAX_SAFE_INTEGER) => set((state) =>
    state.numberId === numberId
      ? { ...state, positionMs: clamp(positionMs, durationMs) }
      : { numberId, positionMs: clamp(positionMs, durationMs), playing: false }),

  setPlaying: (numberId, playing) => set((state) =>
    state.numberId === numberId ? { ...state, playing } : state),

  toggle: (numberId, durationMs) => set((state) => {
    if (state.numberId !== numberId) return state;
    if (state.playing) return { ...state, playing: false };
    const positionMs = clamp(state.positionMs, durationMs);
    const atEnd = durationMs > 0 && positionMs >= durationMs;
    return {
      ...state,
      positionMs: atEnd ? 0 : positionMs,
      playing: true,
    };
  }),

  stepFrame: (numberId, direction, frameRate = 30, durationMs = Number.MAX_SAFE_INTEGER) => set((state) => {
    if (state.numberId !== numberId) return state;
    const frameMs = 1000 / Math.max(1, frameRate);
    return {
      ...state,
      positionMs: clamp(state.positionMs + direction * frameMs, durationMs),
      playing: false,
    };
  }),

  clear: (numberId) => set((state) =>
    numberId == null || state.numberId === numberId
      ? { numberId: null, positionMs: 0, playing: false }
      : state),
}));
