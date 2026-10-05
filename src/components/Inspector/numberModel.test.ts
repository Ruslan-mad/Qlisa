import { describe, expect, it } from "vitest";
import { isNumberActionSupported, isNumberAudioCapable, isNumberMasterCandidate, normalizeNumberCueData, numberActionConfigured, toggleNumberChildMuted } from "./numberModel";
import type { CueSummary, NumberCueData } from "../../lib/types";
import { renderToStaticMarkup } from "react-dom/server";
import React from "react";
import { NumberTab } from "./NumberTab";

const cue = (patch: Partial<CueSummary>): CueSummary => ({
  id: "child", cue_type: "audio", name: "Audio", number: null, notes: "", state: "standby",
  continue_mode: "do_not_continue", color: "none", pre_wait_ms: 0, post_wait_ms: 0,
  duration_ms: null, file_path: null, is_loading: false, is_disabled: false, is_broken: false,
  media_file_missing: false, is_warning: false, file_duration_ms: null, ...patch,
});

describe("Number action readiness", () => {
  it("normalizes persisted Number master fields for the controlled selector", () => {
    const normalized = normalizeNumberCueData({
      ...cue({ cue_type: "number" }),
      children: [],
      number_master_id: undefined,
      master_child_id: "master-2",
      action_offsets_ms: { "action-1": 1250 },
    });
    expect(normalized.number_master_id).toBe("master-2");
    expect(normalized.number_action_offsets_ms).toEqual({ "action-1": 1250 });
    expect(normalized.fade_in_ms).toBeNull();
    expect(normalized.fade_out_curve).toBeNull();
  });

  it("keeps the Number-level shared fade fields", () => {
    const normalized = normalizeNumberCueData({
      ...cue({ cue_type: "number" }), children: [], number_master_id: null,
      fade_in_ms: 800, fade_in_curve: "linear", fade_out_ms: 1200, fade_out_curve: "exponential",
    });
    expect(normalized.fade_in_ms).toBe(800);
    expect(normalized.fade_in_curve).toBe("linear");
    expect(normalized.fade_out_ms).toBe(1200);
    expect(normalized.fade_out_curve).toBe("exponential");
  });

  it("uses backend readiness for action status", () => {
    expect(numberActionConfigured(cue({ number_action_ready: true }))).toBe(true);
    expect(numberActionConfigured(cue({ number_action_ready: false }))).toBe(false);
    expect(numberActionConfigured(cue({ cue_type: "image", file_path: "image.png" }))).toBe(true);
    expect(numberActionConfigured(cue({ cue_type: "image", file_path: "image.png", media_file_missing: true }))).toBe(false);
  });
  it("allows Audio, Video, and Group children as Number masters", () => {
    expect(isNumberMasterCandidate(cue({ cue_type: "audio" }))).toBe(true);
    expect(isNumberMasterCandidate(cue({ cue_type: "video" }))).toBe(true);
    expect(isNumberMasterCandidate(cue({ cue_type: "light" }))).toBe(false);
    expect(isNumberMasterCandidate(cue({ cue_type: "browser" }))).toBe(false);
    expect(isNumberMasterCandidate(cue({ cue_type: "group", children: [cue({})] }))).toBe(true);
  });
  it("supports only Audio, Video, and Image timed actions", () => {
    expect(isNumberActionSupported(cue({ cue_type: "audio" }))).toBe(true);
    expect(isNumberActionSupported(cue({ cue_type: "video" }))).toBe(true);
    expect(isNumberActionSupported(cue({ cue_type: "image" }))).toBe(true);
    expect(isNumberActionSupported(cue({ cue_type: "group" }))).toBe(true);
    expect(isNumberActionSupported(cue({ cue_type: "light" }))).toBe(false);
    expect(isNumberActionSupported(cue({ cue_type: "browser" }))).toBe(false);
  });

  it("toggles persisted mute for Audio and Video without changing gain or settings", () => {
    for (const cueType of ["audio", "video"] as const) {
      const child = cue({ cue_type: cueType, muted: false, volume_db: -12, loop_count: 2 });
      const number = { ...cue({ cue_type: "number" }), children: [child] } as NumberCueData;
      const muted = toggleNumberChildMuted(number, child.id).children[0];
      expect(muted.muted).toBe(true);
      expect(muted.volume_db).toBe(-12);
      expect(muted.loop_count).toBe(2);
      const unmuted = toggleNumberChildMuted({ ...number, children: [muted] }, child.id).children[0];
      expect(unmuted.muted).toBe(false);
      expect(unmuted.volume_db).toBe(-12);
    }
  });

  it("renders an enabled, pressed unmute button and disables unsupported cue types", () => {
    const number = {
      ...cue({ cue_type: "number" }),
      children: [
        cue({ id: "muted-audio", cue_type: "audio", muted: true, volume_db: -12 }),
        cue({ id: "quiet-video", cue_type: "video", muted: false, volume_db: -60 }),
        cue({ id: "image", cue_type: "image" }),
      ],
    } as NumberCueData;
    const markup = renderToStaticMarkup(React.createElement(NumberTab, { cue: number, onRefresh: () => {} }));
    expect(markup).toContain('aria-label="Вкл. звук"');
    expect(markup).toContain('aria-pressed="true"');
    expect(markup).not.toMatch(/<button[^>]*aria-label="Вкл\. звук"[^>]*disabled/);
    expect(markup).toMatch(/<button[^>]*aria-label="Выкл\. звук"[^>]*aria-pressed="false"/);
    expect(markup).toMatch(/<button[^>]*aria-label="Выкл\. звук"[^>]*disabled/);
    expect(isNumberAudioCapable(cue({ cue_type: "image" }))).toBe(false);
  });
});
