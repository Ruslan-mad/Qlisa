import { describe, expect, it } from "vitest";
import type { CueSummary } from "../types";
import { findCueIsLoading, isCurrentClipLoad, isCurrentPreviewStart, previewUiForCue, shouldRetainCueDataForTarget, shouldRetryWaveform } from "../clipEditorLoadGuards";
import { useTimingStore } from "../../stores/timingStore";

describe("clip editor async load guards", () => {
  it("rejects a result from an earlier cue generation", () => {
    expect(isCurrentClipLoad(1, 2)).toBe(false);
    expect(isCurrentClipLoad(2, 2)).toBe(true);
  });

  it("retains loaded cue data only while refreshing the same target", () => {
    expect(shouldRetainCueDataForTarget("cue-a\u0000video", "cue-a\u0000video")).toBe(true);
    expect(shouldRetainCueDataForTarget("cue-a\u0000video", "cue-b\u0000video")).toBe(false);
    expect(shouldRetainCueDataForTarget("cue-a\u0000video", "cue-a\u0000image")).toBe(false);
    expect(shouldRetainCueDataForTarget(null, "cue-a\u0000video")).toBe(false);
  });

  it("finds nested cue loading state and retries only on its false edge", () => {
    const nested = {
      id: "group",
      children: [{ id: "video", is_loading: true }],
    } as unknown as CueSummary;
    expect(findCueIsLoading([nested], "video")).toBe(true);
    expect(findCueIsLoading([nested], "missing")).toBeUndefined();
    expect(shouldRetryWaveform("video", true, "video", false)).toBe(true);
    expect(shouldRetryWaveform("other", true, "video", false)).toBe(false);
    expect(shouldRetryWaveform("video", false, "video", false)).toBe(false);
    expect(shouldRetryWaveform("video", true, "video", undefined)).toBe(false);
  });

  it("hides stale headphone state immediately when the displayed cue changes", () => {
    const preview = { cueId: "cue-a", voiceId: "voice-a", positionMs: 250, error: null };
    expect(previewUiForCue(preview, "cue-a")).toEqual(preview);
    expect(previewUiForCue(preview, "cue-b")).toBeNull();
    expect(previewUiForCue(preview, null)).toBeNull();
  });

  it("turns the button back to Play at EOF so the next click can start again", () => {
    const activeUi = { cueId: "cue-a", voiceId: "voice-a", positionMs: 500, error: null, generation: 4 };
    useTimingStore.setState({ previewPlayhead: null, previewPlayheadGeneration: -1 });
    useTimingStore.getState().setPreviewPlayhead({ cue_id: "cue-a", media_position_ms: 500, playing: true, active: true, generation: 4 });
    useTimingStore.getState().setPreviewPlayhead({ cue_id: "cue-a", media_position_ms: null, playing: false, active: false, generation: 4 });
    const state = useTimingStore.getState();
    const terminalUi = previewUiForCue(activeUi, "cue-a", state.previewPlayhead, state.previewPlayheadGeneration);
    expect(terminalUi?.voiceId).toBeNull();
  });

  it("clears a short preview when its first observed event is terminal", () => {
    // The command response already supplied generation 12. The file can reach
    // EOF before a 30 Hz active event, so the store has only this terminal.
    const startedUi = { cueId: "cue-a", voiceId: "voice-short", positionMs: 995, error: null, generation: 12 };
    useTimingStore.setState({ previewPlayhead: null, previewPlayheadGeneration: -1 });
    useTimingStore.getState().setPreviewPlayhead({
      cue_id: "cue-a", media_position_ms: null, playing: false, active: false, generation: 12,
    });

    const state = useTimingStore.getState();
    expect(previewUiForCue(startedUi, "cue-a", state.previewPlayhead, state.previewPlayheadGeneration)?.voiceId).toBeNull();
  });

  it("keeps the new session button active when an older terminal is stale", () => {
    const newUi = { cueId: "cue-a", voiceId: "voice-new", positionMs: 500, error: null, generation: 5 };
    useTimingStore.setState({ previewPlayhead: null, previewPlayheadGeneration: -1 });
    useTimingStore.getState().setPreviewPlayhead({ cue_id: "cue-a", media_position_ms: 500, playing: true, active: true, generation: 5 });
    useTimingStore.getState().setPreviewPlayhead({ cue_id: "cue-a", media_position_ms: null, playing: false, active: false, generation: 4 });
    const state = useTimingStore.getState();
    expect(previewUiForCue(newUi, "cue-a", state.previewPlayhead, state.previewPlayheadGeneration)?.voiceId).toBe("voice-new");
  });

  it("clears the voice after a keyboard seek replaces its session generation", () => {
    let ui = { cueId: "cue-a", voiceId: "voice-before-seek", positionMs: 500, error: null as string | null, generation: 4 };
    // Keyboard frame-step uses the same seek handler as the timeline. Its
    // command response replaces both the voice ID and authoritative generation.
    ui = { ...ui, voiceId: "voice-after-seek", positionMs: 533, generation: 5 };
    useTimingStore.setState({ previewPlayhead: null, previewPlayheadGeneration: -1 });
    useTimingStore.getState().setPreviewPlayhead({ cue_id: "cue-a", media_position_ms: 533, playing: false, active: true, generation: 5 });
    useTimingStore.getState().setPreviewPlayhead({ cue_id: "cue-a", media_position_ms: null, playing: false, active: false, generation: 5 });
    const state = useTimingStore.getState();
    expect(previewUiForCue(ui, "cue-a", state.previewPlayhead, state.previewPlayheadGeneration)?.voiceId).toBeNull();
  });

  it("commits a replacement start after the old voice reaches EOF", async () => {
    let currentRequestSequence = 8;
    let previewActive = true;
    let ui: { cueId: string; voiceId: string; generation: number } | null = null;
    const ticket = { requestSequence: currentRequestSequence, cueId: "cue-a" };
    let resolveStart!: (value: { voice_id: string; generation: number }) => void;
    const pendingStart = new Promise<{ voice_id: string; generation: number }>((resolve) => { resolveStart = resolve; });
    const completion = pendingStart.then((started) => {
      if (!isCurrentPreviewStart(ticket, currentRequestSequence, "cue-a")) return;
      previewActive = true;
      ui = { cueId: "cue-a", voiceId: started.voice_id, generation: started.generation };
    });

    // A terminal event belongs to the old voice only; it does not invalidate
    // the replacement ticket, even though the old session was marked inactive.
    previewActive = false;
    resolveStart({ voice_id: "voice-replacement", generation: 9 });
    await completion;

    expect(previewActive).toBe(true);
    expect(ui).toEqual({ cueId: "cue-a", voiceId: "voice-replacement", generation: 9 });
  });

  it("ignores a pending start after an explicit stop invalidates its ticket", async () => {
    let currentRequestSequence = 12;
    let ui: { cueId: string; voiceId: string; generation: number } | null = null;
    const ticket = { requestSequence: currentRequestSequence, cueId: "cue-a" };
    let resolveStart!: (value: { voice_id: string; generation: number }) => void;
    const pendingStart = new Promise<{ voice_id: string; generation: number }>((resolve) => { resolveStart = resolve; });
    const completion = pendingStart.then((started) => {
      if (!isCurrentPreviewStart(ticket, currentRequestSequence, "cue-a")) return;
      ui = { cueId: "cue-a", voiceId: started.voice_id, generation: started.generation };
    });

    // Explicit stop increments the same request token before stopping the
    // backend, so a late command response can never revive its button state.
    currentRequestSequence += 1;
    resolveStart({ voice_id: "voice-stopped", generation: 13 });
    await completion;

    expect(ui).toBeNull();
  });
});
