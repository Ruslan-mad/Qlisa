//! Background event loop running at ~30 fps.
//!
//! This task bridges the engines and the Tauri frontend:
//! - Drains [`AudioStatus`] messages from the audio engine's ring buffer.
//! - Drains [`OutputStatus`] messages from the output engine's channel.
//! - Marks cues as completed when their voice ends.
//! - Applies video duration updates to the owning cue.
//! - Fires Auto-Continue chains (Post-Wait based).
//! - Emits `cue-state-changed`, `cue-time-update`, and `master-level`
//!   Tauri events so the UI stays in sync without polling.
//! - Calls [`AudioEngine::gc_voices`] to release stopped audio voice memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::Emitter;

use crate::{
    cue::{
        context::{CueContext, CueEvent},
        types::{ContinueMode, CueId, CueState, CueType},
    },
    engine::{
        output_engine::{OutputEngine, OutputStatus},
        ring_command::AudioStatus,
        timecode_types::TcEvent,
        AudioEngine, DmxEngine,
    },
    show::{cue_list::CueList, transport::Transport, workspace::Workspace},
    state::PreviewSession,
};

/// Target tick interval for the main event loop (~30 fps).
const TICK_MS: u64 = 33;
/// Timer overlay refresh interval — fast enough for smooth millisecond display.
const TIMER_TICK_MS: u64 = 16;
/// If the output stream produces no callback for this long, the audio is frozen
/// (device lost / mid-switch) and running audio cues are paused so their
/// timeline does not drift past the frozen audio.  Above a planned switch's
/// ~tens-of-ms gap, below a perceptible delay.
const AUDIO_FREEZE_MS: u64 = 250;

#[derive(Clone, Debug)]
struct PendingContinuation {
    due: Instant,
    list_id: uuid::Uuid,
    source_generation: u64,
    continuation_token: u64,
    continue_mode: ContinueMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingContinuationStatus {
    Invalid,
    Waiting,
    Due,
}

type ReadyContinuation = (uuid::Uuid, CueId, u64);

/// HashSet is useful for deduplicating same-tick triggers, but its iteration
/// order is unspecified. Dispatch by cue-list ID, then continuation token, and
/// source ID as a stable tie-break so simultaneous completions are repeatable.
fn ordered_ready_continuations(
    ready: &std::collections::HashSet<ReadyContinuation>,
) -> Vec<ReadyContinuation> {
    let mut ordered: Vec<_> = ready.iter().copied().collect();
    ordered.sort_by_key(|(list_id, source_id, token)| (*list_id, *token, *source_id));
    ordered
}

fn pending_continuation_is_valid(
    pending: &PendingContinuation,
    cue_id: CueId,
    cue_list: Option<&CueList>,
) -> bool {
    let Some(cue_list) = cue_list else {
        return false;
    };
    if cue_list.id != pending.list_id {
        return false;
    }
    // The execution's successor path is frozen at GO time. Selection and cue
    // reordering while playback/post-wait is pending must not retarget it.
    cue_list.get_recursive(&cue_id).is_some_and(|cue| {
        cue.continue_mode() == pending.continue_mode
            && matches!(pending.continue_mode, ContinueMode::AutoFollow | ContinueMode::AutoContinue)
            && !cue.is_disabled()
            && cue.play_generation() == pending.source_generation
            && cue.auto_continue_marker() == Some(true)
            && cue_list.continuation_plan(cue_id).is_some_and(|plan| {
                plan.source_generation == pending.source_generation
                    && plan.token == pending.continuation_token
            })
    })
}

fn pending_continuation_status(
    pending: &PendingContinuation,
    cue_id: CueId,
    cue_list: Option<&CueList>,
    now: Instant,
) -> PendingContinuationStatus {
    if !pending_continuation_is_valid(pending, cue_id, cue_list) {
        PendingContinuationStatus::Invalid
    } else if now >= pending.due {
        PendingContinuationStatus::Due
    } else {
        PendingContinuationStatus::Waiting
    }
}

/// Entry point for the event loop thread.  Loops indefinitely.
pub fn run(
    handle: tauri::AppHandle,
    audio_engine: Arc<AudioEngine>,
    output_engine: Arc<OutputEngine>,
    dmx_engine: Arc<DmxEngine>,
    workspace: Arc<Mutex<Workspace>>,
    preview_session: Arc<Mutex<Option<PreviewSession>>>,
    tc_rx: Option<crossbeam_channel::Receiver<TcEvent>>,
    midi_listener: Arc<Mutex<Option<Arc<crate::engine::midi_trigger::MidiTriggerListener>>>>,
) {
    // Spawn a dedicated thread that refreshes the OSD timer overlay at ~60 fps.
    // This is independent of the main 30 fps tick so the millisecond display
    // stays smooth even when the workspace lock is briefly held by a command.
    {
        let ws2 = Arc::clone(&workspace);
        let oe2 = Arc::clone(&output_engine);
        std::thread::Builder::new()
            .name("inkue-timer-refresh".into())
            .spawn(move || timer_refresh_loop(ws2, oe2))
            .expect("Failed to spawn timer refresh thread");
    }

    // Maps a completed cue's ID to the deadline and the ID of its cue list.
    let mut pending_continuations: HashMap<CueId, PendingContinuation> = HashMap::new();
    // Last TC position seen — for the TC dispatcher monotone guard.
    let mut prev_tc_frame: Option<u64> = None;
    // Per-group snapshot: (active_child_id, any_child_running).
    // Used to detect inner-sequence progress and emit cue-list-refresh.
    let mut prev_group_state: HashMap<CueId, (Option<CueId>, bool)> = HashMap::new();
    // Cue sets tracked for OSC feedback (compared each tick to detect changes).
    let mut prev_running_cues: Vec<CueId> = Vec::new();
    let mut prev_playhead_cue: Option<CueId> = None;
    // Fingerprint of the full cue list (number+name). Sending on change.
    let mut prev_cue_list_hash: u64 = 0;
    // Audio-freeze guard state (device-loss timeline pause).
    let mut last_cb_count: u64 = audio_engine.callback_count();
    let mut cb_last_advance: Instant = Instant::now();
    let mut audio_frozen = false;
    let mut auto_paused: std::collections::HashSet<CueId> = std::collections::HashSet::new();
    // Last session sent to the UI. Kept separately from the live session so a
    // stop or replacement always gets a terminal event on the next 30 Hz tick.
    let mut last_preview_session: Option<PreviewSession> = None;

    loop {
        std::thread::sleep(Duration::from_millis(TICK_MS));
        tick(
            &handle,
            &audio_engine,
            &output_engine,
            &dmx_engine,
            &workspace,
            &preview_session,
            tc_rx.as_ref(),
            &midi_listener,
            &mut prev_tc_frame,
            &mut pending_continuations,
            &mut prev_group_state,
            &mut prev_running_cues,
            &mut prev_playhead_cue,
            &mut prev_cue_list_hash,
            &mut last_cb_count,
            &mut cb_last_advance,
            &mut audio_frozen,
            &mut auto_paused,
            &mut last_preview_session,
        );
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn make_context(
    audio_engine: &Arc<AudioEngine>,
    output_engine: &Arc<OutputEngine>,
    dmx_engine: &Arc<DmxEngine>,
    stop_fade_ms: u32,
    output_patches: Vec<crate::engine::device_manager::OutputPatch>,
    default_patch_id: Option<uuid::Uuid>,
    output_screen: Option<u32>,
    osc_patches: Vec<crate::engine::osc_patch::OscPatch>,
    fixtures: Vec<crate::engine::fixture::PatchedFixture>,
    fixture_groups: Vec<crate::engine::fixture::FixtureGroup>,
    input_patches: Vec<crate::engine::audio_input::InputPatch>,
    audio_buffer_size: u32,
) -> CueContext {
    let (tx, _rx) = crossbeam_channel::unbounded::<CueEvent>();
    CueContext::new(
        audio_engine.clone(),
        output_engine.clone(),
        tx,
        stop_fade_ms,
        output_patches,
        default_patch_id,
        output_screen,
        osc_patches,
        dmx_engine.clone(),
        fixtures,
        fixture_groups,
        input_patches,
        audio_buffer_size,
    )
}

/// OSC progress values for one cue: `(progress 0..1, elapsed s, remaining s,
/// duration s)`; `remaining`/`duration` are `-1.0` when unknown.
///
/// Without a total duration (vamp, ∞ loop, live feed) the progress falls back
/// to the position within one file pass — a gauge then follows the loop just
/// like the in-app bars.
fn progress_values(
    elapsed_ms: u64,
    duration_ms: Option<u64>,
    file_duration_ms: Option<u64>,
) -> (f32, f32, f32, f32) {
    let elapsed_s = elapsed_ms as f32 / 1000.0;
    match duration_ms {
        Some(d) if d > 0 => {
            let progress = (elapsed_ms as f32 / d as f32).clamp(0.0, 1.0);
            let remaining_s = d.saturating_sub(elapsed_ms) as f32 / 1000.0;
            (progress, elapsed_s, remaining_s, d as f32 / 1000.0)
        }
        _ => {
            let progress = match file_duration_ms {
                Some(fd) if fd > 0 => ((elapsed_ms % fd) as f32 / fd as f32).clamp(0.0, 1.0),
                _ => 0.0,
            };
            (progress, elapsed_s, -1.0, -1.0)
        }
    }
}

/// Collect `cue-time-update` snapshots recursively, including children of
/// running Group cues.
fn collect_time_snapshots(
    cues: &[Box<dyn crate::cue::traits::Cue>],
    audio_engine: &Arc<AudioEngine>,
    output_engine: &Arc<OutputEngine>,
) -> Vec<(CueId, u64, u64, Option<u64>, Option<u64>)> {
    let mut result = Vec::new();
    for cue in cues {
        if cue.state() == CueState::Running || cue.state() == CueState::Paused {
            // Keep the action clock separate from the file-relative media
            // playhead.  Slice jumps and loop wrapping belong only in the
            // media position, never in elapsed/remaining duration values.
            let action_elapsed = cue.action_elapsed().as_millis() as u64;
            let media_position_ms = cue.media_position_ms(audio_engine, output_engine);
            result.push((
                cue.id(),
                cue.elapsed().as_millis() as u64,
                action_elapsed,
                cue.duration()
                    .map(|d| (d.as_millis() as u64).saturating_sub(action_elapsed)),
                media_position_ms,
            ));
        }
        if let Some(children) = cue.child_cues() {
            result.extend(collect_time_snapshots(
                children,
                audio_engine,
                output_engine,
            ));
        }
    }
    result
}

/// Collect the ids of `cue` and every descendant that is Running or Paused.
/// Used when a Fade force-stops a target so the UI can be told which cues
/// (including group children) are no longer active.
fn collect_running_ids(cue: &dyn crate::cue::traits::Cue, out: &mut Vec<CueId>) {
    if cue.state() == CueState::Running || cue.state() == CueState::Paused {
        out.push(cue.id());
    }
    if let Some(children) = cue.child_cues() {
        for ch in children {
            collect_running_ids(ch.as_ref(), out);
        }
    }
}

/// Reset any RUNNING group child whose audio voice has completed.
///
/// The top-level completion detector (step 6) does not descend into group
/// children, and audio cues never self-complete, so a group must reap its own
/// children.  A group can only detect a child via wall-clock `duration()`, which
/// is `None` while the child is still preloading (its buffer is played but
/// `cached_duration` isn't set yet).  Such a child would linger in Running
/// forever — stalling the group.  This closes the gap by resetting children
/// whose voice the engine reported completed, at any nesting depth.
fn reap_voice_completed_children(
    cues: &mut [Box<dyn crate::cue::traits::Cue>],
    completed: &[CueId],
    audio_engine: &AudioEngine,
    output_engine: &OutputEngine,
) {
    for cue in cues.iter_mut() {
        if let Some(children) = cue.child_cues_mut() {
            for child in children.iter_mut() {
                let voice_done = child.state() == CueState::Running
                    && child
                        .playing_voice_id()
                        .map(|v| {
                            completed.contains(&v)
                                && !voice_is_alive(v, audio_engine, output_engine)
                        })
                        .unwrap_or(false);
                if voice_done {
                    let _ = child.reset();
                }
            }
            reap_voice_completed_children(children, completed, audio_engine, output_engine);
        }
    }
}

fn voice_is_alive(
    voice_id: CueId,
    audio_engine: &AudioEngine,
    output_engine: &OutputEngine,
) -> bool {
    audio_engine.voice_is_alive(voice_id) || output_engine.is_voice_playing(voice_id)
}

/// A voice's engine state is authoritative over the wall clock. Seeking can
/// move a cue's elapsed clock past its nominal duration while playback remains
/// live; resetting then would drop the only handle capable of stopping it.
fn should_complete_cue(
    voice_id: Option<CueId>,
    completed_voice_ids: &[CueId],
    time_done: bool,
    audio_engine: &AudioEngine,
    output_engine: &OutputEngine,
) -> bool {
    let Some(id) = voice_id else { return time_done };
    should_complete_with_voice_state(
        true,
        voice_is_alive(id, audio_engine, output_engine),
        completed_voice_ids.contains(&id),
        time_done,
    )
}

fn should_complete_with_voice_state(
    has_voice: bool,
    voice_is_alive: bool,
    completion_received: bool,
    time_done: bool,
) -> bool {
    if has_voice && voice_is_alive {
        return false;
    }
    if has_voice {
        completion_received || time_done
    } else {
        time_done
    }
}

/// Clear the operator-only session after a natural EOF or any other aux voice
/// disappearance. Snapshotting and compare-and-taking avoid holding the
/// session mutex while inspecting audio state, and avoid removing a session
/// that another control replaced in the meantime.
fn prune_preview_session(
    preview_session: &Arc<Mutex<Option<PreviewSession>>>,
    audio_engine: &AudioEngine,
    completed_voice_ids: &[CueId],
) -> Option<PreviewSession> {
    let session = preview_session
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().cloned());
    let Some(session) = session else { return None };
    if !completed_voice_ids.contains(&session.voice_id)
        && audio_engine.preview_voice_is_alive(session.voice_id)
    {
        return None;
    }
    if let Ok(mut slot) = preview_session.lock() {
        if slot.as_ref() == Some(&session) {
            *slot = None;
            for voice_id in session.all_voice_ids() {
                audio_engine.stop_preview_voice(*voice_id);
            }
            return Some(session);
        }
    }
    None
}

fn preview_playhead_payload(
    session: PreviewSession,
    media_position_ms: Option<u64>,
    playing: bool,
    active: bool,
) -> serde_json::Value {
    serde_json::json!({
        "cue_id": session.cue_id,
        "media_position_ms": media_position_ms,
        "playing": playing,
        "active": active,
        "generation": session.generation,
    })
}

fn emit_preview_playhead(
    handle: &tauri::AppHandle,
    session: PreviewSession,
    media_position_ms: Option<u64>,
    playing: bool,
    active: bool,
) {
    let _ = handle.emit(
        "preview-playhead",
        preview_playhead_payload(session, media_position_ms, playing, active),
    );
}

/// Publish the headphone preview's file-relative cursor independently of show
/// cue timing. The session is rechecked after the audio lookup so a delayed
/// tick for an old voice cannot emit an active event after a newer generation
/// has replaced it.
fn publish_preview_playhead(
    handle: &tauri::AppHandle,
    preview_session: &Arc<Mutex<Option<PreviewSession>>>,
    audio_engine: &AudioEngine,
    last_reported: &mut Option<PreviewSession>,
) {
    let session = preview_session
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().cloned());
    let Some(session) = session else {
        if let Some(previous) = last_reported.take() {
            emit_preview_playhead(handle, previous, None, false, false);
        }
        return;
    };
    if !audio_engine.preview_voice_is_alive(session.voice_id) {
        return;
    }
    let position = audio_engine.voice_position_ms(session.voice_id);
    let is_still_current = preview_session
        .lock()
        .map(|slot| slot.as_ref() == Some(&session))
        .unwrap_or(false);
    if !is_still_current {
        return;
    }
    if let Some(previous) = last_reported.replace(session.clone()) {
        if previous != session {
            emit_preview_playhead(handle, previous, None, false, false);
        }
    }
    emit_preview_playhead(handle, session, position, true, true);
}

#[allow(clippy::too_many_arguments)]
fn tick(
    handle: &tauri::AppHandle,
    audio_engine: &Arc<AudioEngine>,
    output_engine: &Arc<OutputEngine>,
    dmx_engine: &Arc<DmxEngine>,
    workspace: &Arc<Mutex<Workspace>>,
    preview_session: &Arc<Mutex<Option<PreviewSession>>>,
    tc_rx: Option<&crossbeam_channel::Receiver<TcEvent>>,
    midi_listener: &Arc<Mutex<Option<Arc<crate::engine::midi_trigger::MidiTriggerListener>>>>,
    prev_tc_frame: &mut Option<u64>,
    pending_continuations: &mut HashMap<CueId, PendingContinuation>,
    prev_group_state: &mut HashMap<CueId, (Option<CueId>, bool)>,
    prev_running_cues: &mut Vec<CueId>,
    prev_playhead_cue: &mut Option<CueId>,
    prev_cue_list_hash: &mut u64,
    last_cb_count: &mut u64,
    cb_last_advance: &mut Instant,
    audio_frozen: &mut bool,
    auto_paused: &mut std::collections::HashSet<CueId>,
    last_preview_session: &mut Option<PreviewSession>,
) {
    // ------------------------------------------------------------------
    // 0. Drain incoming timecode events and fire TC-triggered cues.
    // ------------------------------------------------------------------
    // (list_id, cue_id) pairs whose TC trigger was crossed this tick. Fired via
    // the real GO path further down, once the engine context is assembled.
    let mut tc_fire: Vec<(uuid::Uuid, CueId)> = Vec::new();
    if let Some(rx) = tc_rx {
        // Collect all pending TC events without blocking.
        let mut latest_pos = None;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                TcEvent::Position(pos) | TcEvent::Started(pos) => {
                    latest_pos = Some(pos);
                }
                TcEvent::Stopped => {
                    // On-Stop policy is applied per-list below when ws is locked.
                    let _ = handle.emit("timecode-stopped", serde_json::json!({}));
                }
            }
        }

        if let Some(pos) = latest_pos {
            let abs_frame = pos.to_frame_number();
            // Only process when the position has actually advanced (monotone guard).
            let advanced = prev_tc_frame.map(|prev| abs_frame > prev).unwrap_or(true);
            // Jump backwards: re-arm all triggers.
            let jumped_back = prev_tc_frame.map(|prev| abs_frame < prev).unwrap_or(false);

            *prev_tc_frame = Some(abs_frame);

            // Emit the current position for the UI status widget.
            let _ = handle.emit(
                "timecode",
                serde_json::json!({
                    "h": pos.hours, "m": pos.minutes, "s": pos.seconds, "f": pos.frames,
                    "rate": pos.rate.to_string(),
                }),
            );

            if advanced || jumped_back {
                if let Ok(mut ws) = workspace.try_lock() {
                    for cl in &mut ws.cue_lists {
                        if !cl.tc_config.enabled {
                            continue;
                        }

                        // Re-arm on jump back: lower the covered mark to the
                        // rewound position so triggers ahead of it fire again.
                        if jumped_back {
                            cl.tc_last_triggered_frame = abs_frame;
                        }

                        // Fire every cue whose trigger sits in (last_triggered_frame, abs_frame].
                        let crossed = cl.tc_triggers_crossed(abs_frame);
                        if crossed.is_empty() {
                            continue;
                        }
                        cl.tc_last_triggered_frame = abs_frame;

                        for cue_id in crossed {
                            // Verify the cue exists, then queue it for the real GO
                            // path below (the engine context isn't assembled here).
                            if cl.get_mut_recursive(&cue_id).is_none() {
                                continue;
                            };
                            tc_fire.push((cl.id, cue_id));
                        }
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // 0b. Drain incoming MIDI and fire cues bound to what arrived.
    // ------------------------------------------------------------------
    // Joins `tc_fire`: both are "something outside told us to GO", and both go
    // through the same real GO path in section 9b so playback actually starts
    // and Auto-Continue / Auto-Follow still chain.
    //
    // Unlike TC there is no monotone guard — a MIDI message *is* the event, so
    // pressing the same key twice fires the cue twice, which is what an
    // operator with a pad expects.
    {
        let messages = midi_listener
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|l| l.drain()))
            .unwrap_or_default();
        if !messages.is_empty() {
            if let Ok(ws) = workspace.try_lock() {
                for message in &messages {
                    for cl in &ws.cue_lists {
                        for cue_id in cl.midi_triggers_matching(message) {
                            // A trigger left behind by a deleted cue fires nothing.
                            if cl.get_recursive(&cue_id).is_none() {
                                continue;
                            }
                            tc_fire.push((cl.id, cue_id));
                        }
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // 1. Drain the audio status ring buffer.
    // ------------------------------------------------------------------
    let audio_statuses = audio_engine.drain_status();

    let mut completed_voice_ids: Vec<CueId> = Vec::new();
    let mut master_peak_l = 0.0_f32;
    let mut master_peak_r = 0.0_f32;
    let mut has_master = false;
    // Per-Output-Patch peaks for the mixer VUs, keyed by patch slot.
    let mut patch_peaks: [(f32, f32); crate::engine::audio_engine::PATCH_VU_SLOTS] =
        [(0.0, 0.0); crate::engine::audio_engine::PATCH_VU_SLOTS];
    let mut has_patch_levels = false;

    for s in audio_statuses {
        match s {
            AudioStatus::Completed { voice_id } => {
                // An EOF queued before a seek must not complete a voice that
                // has since resumed from the requested position.
                if !audio_engine.voice_is_alive(voice_id) {
                    completed_voice_ids.push(voice_id);
                }
            }
            AudioStatus::MasterLevels { peak_l, peak_r } => {
                master_peak_l = master_peak_l.max(peak_l);
                master_peak_r = master_peak_r.max(peak_r);
                has_master = true;
            }
            AudioStatus::PatchLevels {
                slot,
                peak_l,
                peak_r,
            } => {
                if let Some(p) = patch_peaks.get_mut(slot as usize) {
                    p.0 = p.0.max(peak_l);
                    p.1 = p.1.max(peak_r);
                    has_patch_levels = true;
                }
            }
            AudioStatus::Underrun { voice_id, count } => {
                let cue_info = workspace
                    .try_lock()
                    .ok()
                    .and_then(|ws| find_audio_cue_info(&ws.cue_lists, voice_id, output_engine));
                let message = match cue_info {
                    Some((_, number, name)) if !number.is_empty() => format!(
                        "Streaming audio underrun: Cue #{number} \u{201c}{name}\u{201d} ({count} silent frames)"
                    ),
                    Some((_, _, name)) if !name.is_empty() => format!(
                        "Streaming audio underrun: Cue \u{201c}{name}\u{201d} ({count} silent frames)"
                    ),
                    _ => format!(
                        "Streaming audio underrun on voice {voice_id} ({count} silent frames)"
                    ),
                };
                crate::health::set(crate::health::HealthAlert::new(
                    "audio-stream-underrun",
                    crate::health::HealthLevel::Warning,
                    message,
                ));
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // 2. Drain the output engine status channel.
    // ------------------------------------------------------------------
    let output_statuses = output_engine.drain_status();

    let mut video_duration_updates: Vec<(CueId, Duration)> = Vec::new();
    let mut emit_workspace_modified = false;

    for s in output_statuses {
        match s {
            OutputStatus::Completed { voice_id } => {
                // mpv can deliver an EOF notification just before a seek-back
                // is applied. Keep the cue/voice ownership while mpv reports
                // active playback; the stale completion will no longer reset
                // it or collect the live slot.
                if !output_engine.is_voice_playing(voice_id) {
                    completed_voice_ids.push(voice_id);
                    output_engine.gc_voice(voice_id);
                }
            }
            OutputStatus::Duration {
                voice_id,
                duration_ms,
            } => {
                video_duration_updates.push((voice_id, Duration::from_millis(duration_ms)));
                emit_workspace_modified = true;
            }
            OutputStatus::Error { voice_id, message } => {
                log::warn!("Output voice {voice_id} error: {message}");
            }
        }
    }

    // Emit master-level whenever there is any active signal.
    if has_master {
        let _ = handle.emit(
            "master-level",
            serde_json::json!({ "peak_l": master_peak_l, "peak_r": master_peak_r }),
        );
    }

    // Per-patch levels for the mixer VUs (slot = index in ws.output_patches).
    if has_patch_levels {
        let levels: Vec<serde_json::Value> = patch_peaks
            .iter()
            .enumerate()
            .filter(|(_, (l, r))| *l > 0.0 || *r > 0.0)
            .map(|(slot, (l, r))| serde_json::json!({ "slot": slot, "peak_l": l, "peak_r": r }))
            .collect();
        let _ = handle.emit("patch-levels", serde_json::json!({ "levels": levels }));
    }

    // ------------------------------------------------------------------
    // 3. Lock the workspace (non-blocking; skip tick if a command holds it).
    // ------------------------------------------------------------------
    let mut ws = match workspace.try_lock() {
        Ok(w) => w,
        Err(_) => return,
    };

    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let ws_patches = ws.output_patches.clone();
    let ws_default_patch = ws.default_output_patch_id;
    let ws_output_screen = ws.preferences.display.output_screen;
    // Keep the engine's projector-alignment mirror in sync with the loaded
    // workspace (no-op unless it actually changed — covers open/new/recovery
    // without hooking every load path).
    output_engine.set_output_transform(ws.preferences.display.output_transform);
    let ws_osc_patches = ws.osc_patches.clone();
    let ws_fixtures = ws.fixtures.clone();
    let ws_fixture_groups = ws.fixture_groups.clone();
    let ws_input_patches = ws.input_patches.clone();
    let ws_buffer_size = ws.preferences.audio.audio_buffer_size;
    let active_list_id = ws.active_cue_list_id;

    if ws.cue_lists.is_empty() {
        return;
    }

    let tick_ctx = make_context(
        audio_engine,
        output_engine,
        dmx_engine,
        stop_fade_ms,
        ws_patches.clone(),
        ws_default_patch,
        ws_output_screen,
        ws_osc_patches.clone(),
        ws_fixtures.clone(),
        ws_fixture_groups.clone(),
        ws_input_patches.clone(),
        ws_buffer_size,
    );

    // ------------------------------------------------------------------
    // 3b. Audio-freeze guard.  When the output stream stops producing
    //     callbacks (device pulled, or briefly during a switch), pause every
    //     running audio cue so its wall-clock timeline freezes in sync with
    //     the frozen (preserved) audio — otherwise the clock keeps advancing,
    //     hits `duration`, and the cue completes while its audio is still
    //     queued and unstoppable.  Resume them when callbacks return.
    // ------------------------------------------------------------------
    let cb = audio_engine.callback_count();
    if cb != *last_cb_count {
        *last_cb_count = cb;
        *cb_last_advance = Instant::now();
    }
    let frozen_now = cb_last_advance.elapsed() >= Duration::from_millis(AUDIO_FREEZE_MS);

    let mut just_paused: Vec<CueId> = Vec::new();
    let mut just_resumed: Vec<CueId> = Vec::new();
    if frozen_now && !*audio_frozen {
        for cl in ws.cue_lists.iter_mut() {
            for cue in cl.cues.iter_mut() {
                if cue.state() == CueState::Running
                    && cue.playing_voice_id().is_some()
                    && cue.pause(&tick_ctx).is_ok()
                {
                    auto_paused.insert(cue.id());
                    just_paused.push(cue.id());
                }
            }
        }
    } else if !frozen_now && *audio_frozen {
        for cl in ws.cue_lists.iter_mut() {
            for cue in cl.cues.iter_mut() {
                if !(auto_paused.contains(&cue.id()) && cue.state() == CueState::Paused) {
                    continue;
                }
                // Video: mpv (its own clock) kept playing during the ~250 ms
                // detection window while the paired audio voice was frozen, so
                // they desynced.  Re-anchor the audio voice to mpv's *actual*
                // position (time-pos) — without moving the picture — so audio
                // catches up precisely before playback resumes.
                if cue.cue_type() == CueType::Video {
                    if let Some(voice) = cue.playing_voice_id() {
                        tick_ctx.output_engine.resync_audio_to_video(voice);
                    }
                }
                if cue.resume(&tick_ctx).is_ok() {
                    just_resumed.push(cue.id());
                }
            }
        }
        auto_paused.clear();
    }
    *audio_frozen = frozen_now;

    // ------------------------------------------------------------------
    // 4. Apply video duration updates — search every cue list.
    // ------------------------------------------------------------------
    'duration_update: for (voice_id, duration) in &video_duration_updates {
        for cl in ws.cue_lists.iter_mut() {
            for cue in cl.cues.iter_mut() {
                if cue.playing_voice_id() == Some(*voice_id) {
                    cue.set_runtime_duration(*duration);
                    continue 'duration_update;
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // 5-9. Per-list: tick, completion, auto-continue/follow, GO.
    //      We collect cue list IDs first to avoid borrow issues.
    // ------------------------------------------------------------------
    let cue_list_ids: Vec<uuid::Uuid> = ws.cue_lists.iter().map(|cl| cl.id).collect();

    // Aggregate results across all lists for event emission.
    let mut all_newly_completed: Vec<(CueId, ContinueMode, Duration)> = Vec::new();
    let mut all_go_fired: Vec<CueId> = Vec::new();
    let mut all_time_snapshots: Vec<(CueId, u64, u64, Option<u64>, Option<u64>)> = Vec::new();
    let mut all_go_triggered: Vec<CueId> = Vec::new();
    let mut all_go_stopped: Vec<CueId> = Vec::new();
    let mut all_seq_group_playheads: Vec<Option<CueId>> = Vec::new();
    let mut number_finish_starts: Vec<(CueId, Vec<CueId>)> = Vec::new();
    let mut group_child_changed = false;
    // A cue can fail between GO commands. Do not discard that Result: its
    // runtime diagnostic is persisted on the cue and this list drives the UI
    // state change/refresh that makes it visible to the operator.
    let mut runtime_tick_failures: Vec<(CueId, String)> = Vec::new();
    let mut runtime_diagnostic_changed = false;

    // Source execution tokens whose captured successor should GO this tick.
    let mut ready_continuations: std::collections::HashSet<ReadyContinuation> =
        std::collections::HashSet::new();

    // Resolve pending auto-follow delays and collect which lists need a GO.
    let now = Instant::now();
    pending_continuations.retain(|cue_id, pending| {
        match pending_continuation_status(pending, *cue_id, ws.cue_list_by_id(pending.list_id), now)
        {
            PendingContinuationStatus::Invalid => false,
            PendingContinuationStatus::Waiting => true,
            PendingContinuationStatus::Due => {
                ready_continuations.insert((pending.list_id, *cue_id, pending.continuation_token));
                false
            }
        }
    });

    for &list_id in &cue_list_ids {
        // 5. Tick all Running cues.
        let mut tick_fired_ids = Vec::new();
        if let Some(cl) = ws.cue_list_by_id_mut(list_id) {
            for cue in cl.cues.iter_mut() {
                if cue.state() == CueState::Running {
                    if let Err(error) = cue.tick(&tick_ctx) {
                        log::warn!("Cue '{}' runtime tick failed: {error}", cue.name());
                        runtime_tick_failures.push((cue.id(), error.to_string()));
                    }
                    tick_fired_ids.extend(cue.take_fired_cue_ids());
                    runtime_diagnostic_changed |= cue.take_runtime_diagnostic_changed();
                }
            }
        }

        // Group children can start on a tick (sequence/playlist advancement).
        // Their action specifications need the same transport dispatcher as
        // cues started by an explicit GO.
        // Dispatch even when no child action fired: a Browser with a pre-wait
        // can take ownership during tick() without producing a fired-child ID.
        // The transport drains its actual-start queue here and reconciles all
        // nested Browser owners.
        if !tick_fired_ids.is_empty() || tick_ctx.has_browser_starts() {
            if let Some(cl) = ws.cue_list_by_id_mut(list_id) {
                let mut transport = Transport::new(tick_ctx.clone());
                let result = transport.dispatch_fired_cues(cl, &tick_fired_ids);
                all_go_fired.extend(result.fired);
                all_go_triggered.extend(result.triggered);
                all_go_stopped.extend(result.stopped);
                group_child_changed = true;
            }
            all_go_fired.extend(tick_fired_ids);
        }

        // 5a. A `stop_at_end` Fade that just finished asks (via take_fade_stop_targets)
        //     for its target CUES to be stopped — stopping voices alone leaves the
        //     targets (and group children) in the Running state.  hard_stop resets
        //     their state and recurses into groups.  We record every id that was
        //     running (target + descendants) so step 11 emits `cue-state-changed`
        //     for them; otherwise the UI keeps showing them RUNNING until the next
        //     GO forces a full refresh.
        let mut fade_stopped_ids: Vec<CueId> = Vec::new();
        let fade_stop_targets: Vec<CueId> = ws
            .cue_list_by_id_mut(list_id)
            .map(|cl| {
                cl.cues
                    .iter_mut()
                    .flat_map(|c| c.take_fade_stop_targets())
                    .collect()
            })
            .unwrap_or_default();
        if !fade_stop_targets.is_empty() {
            if let Some(cl) = ws.cue_list_by_id_mut(list_id) {
                for tid in fade_stop_targets {
                    if let Some(target) = cl.get_mut_recursive(&tid) {
                        if target.is_running() || target.is_paused() {
                            collect_running_ids(target, &mut fade_stopped_ids);
                            let _ = target.hard_stop(&tick_ctx);
                        }
                    }
                }
            }
        }

        // 5b. Release the outer Playhead once a Sequential group has fired its
        //     last child (overlapping children may still be playing out).  This
        //     covers Auto-Continue/Follow reaching the last child without a GO;
        //     the transport handles the manual-GO case synchronously.
        let release_ph = ws.cue_list_by_id(list_id).and_then(|cl| {
            cl.playhead_cue_id.filter(|ph| {
                cl.cues
                    .iter()
                    .any(|c| c.id() == *ph && c.released_playhead())
            })
        });
        if release_ph.is_some() {
            if let Some(cl) = ws.cue_list_by_id_mut(list_id) {
                cl.advance_playhead();
            }
            let ph = ws.cue_list_by_id(list_id).and_then(|cl| cl.playhead_cue_id);
            all_seq_group_playheads.push(ph);
        }

        // 6. Detect completions.
        let mut newly_completed: Vec<(CueId, ContinueMode, Duration)> = Vec::new();
        let mut completed_auto_continue: Vec<(CueId, Duration)> = Vec::new();
        let mut advance_playhead_ids: Vec<CueId> = Vec::new();

        if let Some(cl) = ws.cue_list_by_id_mut(list_id) {
            // Reap nested children whose voice completed first, so a group whose
            // last child just finished can be detected complete this same tick.
            reap_voice_completed_children(
                &mut cl.cues,
                &completed_voice_ids,
                audio_engine,
                output_engine,
            );

            let current_playhead = cl.playhead_cue_id;
            for cue in cl.cues.iter_mut() {
                if cue.state() != CueState::Running {
                    continue;
                }
                let time_done = cue
                    .duration()
                    .map(|d| cue.action_elapsed() >= d)
                    .unwrap_or(false);
                let group_done = cue.is_complete();
                let cue_done = should_complete_cue(
                    cue.playing_voice_id(),
                    &completed_voice_ids,
                    time_done,
                    audio_engine,
                    output_engine,
                );
                if cue_done || group_done {
                    let id = cue.id();
                    if group_done {
                        let targets = cue.take_number_completion_start_ids();
                        if !targets.is_empty() {
                            number_finish_starts.push((id, targets));
                        }
                    }
                    let cm = cue.continue_mode();
                    let pw = cue.post_wait();
                    let auto_continue_remaining = (cm == ContinueMode::AutoContinue)
                        .then(|| pw.saturating_sub(cue.auto_continue_elapsed()));
                    if cue.holds_playhead() && current_playhead == Some(id) {
                        advance_playhead_ids.push(id);
                    }
                    let _ = cue.reset();
                    if let Some(remaining) = auto_continue_remaining {
                        completed_auto_continue.push((id, remaining));
                    }
                    newly_completed.push((id, cm, pw));
                }
            }
        }

        if !number_finish_starts.is_empty() {
            if let Some(cl) = ws.cue_list_by_id_mut(list_id) {
                let mut transport = Transport::new(tick_ctx.clone());
                for (source_id, targets) in number_finish_starts.drain(..) {
                    let result = transport.start_number_targets(cl, source_id, &targets);
                    all_go_fired.extend(result.fired);
                    all_go_triggered.extend(result.triggered);
                    all_go_stopped.extend(result.stopped);
                }
                group_child_changed = true;
            }
        }

        // Fade-stopped cues (target + descendants) are reported as completed so
        // the UI clears them.  DoNotContinue so they never chain a follow-on cue.
        if !fade_stopped_ids.is_empty() {
            // Force a full cue-list refresh so nested state + green playhead
            // highlights resync (they derive from the whole cue tree).
            group_child_changed = true;
            for id in fade_stopped_ids {
                newly_completed.push((id, ContinueMode::DoNotContinue, Duration::ZERO));
            }
        }

        // Advance playhead for sequential groups that held it.
        let mut playhead_advanced = false;
        if let Some(cl) = ws.cue_list_by_id_mut(list_id) {
            for id in &advance_playhead_ids {
                if cl.playhead_cue_id == Some(*id) {
                    cl.advance_playhead();
                    playhead_advanced = true;
                }
            }
        }
        if playhead_advanced {
            let ph = ws.cue_list_by_id(list_id).and_then(|cl| cl.playhead_cue_id);
            all_seq_group_playheads.push(ph);
        }

        // 7. Time snapshots.
        if let Some(cl) = ws.cue_list_by_id(list_id) {
            all_time_snapshots.extend(collect_time_snapshots(
                &cl.cues,
                audio_engine,
                output_engine,
            ));
        }

        // 8. Auto-Continue / Auto-Follow detection.
        let delayed_ac_ids: Vec<CueId> = ws
            .cue_list_by_id(list_id)
            .map(|cl| {
                cl.cues
                    .iter()
                    .filter(|c| {
                        c.state() == CueState::Running
                            && c.continue_mode() == ContinueMode::AutoContinue
                            && !c.is_auto_continue_fired()
                            && c.is_action_started()
                            && c.auto_continue_elapsed() >= c.post_wait()
                            && cl.continuation_plan(c.id()).is_some()
                    })
                    .map(|c| c.id())
                    .collect()
            })
            .unwrap_or_default();

        // Action cues started by another cue may complete synchronously and
        // never pass through the Running completion scan. Their continuation
        // still uses the successor snapshot made at their actual start.
        let instant_continuation_ids: Vec<CueId> = ws
            .cue_list_by_id(list_id)
            .map(|cl| {
                cl.cues
                    .iter()
                    .filter(|c| {
                        c.state() == CueState::Completed
                            && matches!(
                                c.continue_mode(),
                                ContinueMode::AutoContinue | ContinueMode::AutoFollow
                            )
                            && !c.is_auto_continue_fired()
                            && c.action_elapsed() >= c.post_wait()
                            && cl.continuation_plan(c.id()).is_some()
                    })
                    .map(|c| c.id())
                    .collect()
            })
            .unwrap_or_default();

        let due_continuation_ids: std::collections::HashSet<CueId> = delayed_ac_ids
            .iter()
            .chain(instant_continuation_ids.iter())
            .copied()
            .collect();

        if let Some(cl) = ws.cue_list_by_id_mut(list_id) {
            for id in &due_continuation_ids {
                if let Some(cue) = cl.get_mut_recursive(id) {
                    cue.mark_auto_continue_fired();
                }
                if let Some(plan) = cl.continuation_plan(*id) {
                    ready_continuations.insert((list_id, *id, plan.token));
                }
            }
        }

        // Auto-Continue's post-wait is measured from action start and may
        // outlive a short media cue. Preserve the remaining deadline when the
        // cue naturally completes before that clock expires.
        for (cue_id, remaining) in completed_auto_continue {
            if let Some(cl) = ws.cue_list_by_id_mut(list_id) {
                let Some(plan) = cl.continuation_plan(cue_id).cloned() else {
                    continue;
                };
                let source_generation = cl.get_mut_recursive(&cue_id).map(|cue| {
                    cue.mark_auto_continue_fired();
                    cue.play_generation()
                });
                let Some(source_generation) = source_generation else {
                    continue;
                };
                if remaining.is_zero() {
                    ready_continuations.insert((list_id, cue_id, plan.token));
                } else {
                    pending_continuations.insert(
                        cue_id,
                        PendingContinuation {
                            due: Instant::now() + remaining,
                            list_id,
                            source_generation,
                            continuation_token: plan.token,
                            continue_mode: ContinueMode::AutoContinue,
                        },
                    );
                }
            }
        }

        for (cue_id, cm, pw) in &newly_completed {
            if *cm == ContinueMode::AutoFollow {
                if let Some(cl) = ws.cue_list_by_id_mut(list_id) {
                    let plan_token = cl.continuation_plan(*cue_id).map(|plan| plan.token);
                    if let Some(plan_token) = plan_token {
                        let source_generation = if let Some(cue) = cl.get_mut_recursive(cue_id) {
                            // This flag is cleared by Stop/Reset/GO on cue types
                            // that support it. Captured after the natural reset,
                            // it serves as a lightweight pending-execution token.
                            cue.mark_auto_continue_fired();
                            Some(cue.play_generation())
                        } else {
                            None
                        };
                        if let Some(source_generation) = source_generation {
                            if pw.is_zero() {
                                ready_continuations.insert((list_id, *cue_id, plan_token));
                            } else {
                                pending_continuations.insert(
                                    *cue_id,
                                    PendingContinuation {
                                        due: Instant::now() + *pw,
                                        list_id,
                                        source_generation,
                                        continuation_token: plan_token,
                                        continue_mode: ContinueMode::AutoFollow,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }

        all_newly_completed.extend(newly_completed);

        // Group-child change detection (for cue-list-refresh event).
        if let Some(cl) = ws.cue_list_by_id(list_id) {
            let current: Vec<(CueId, Option<CueId>, bool)> = cl
                .cues
                .iter()
                .filter(|c| c.child_cues().is_some() && c.state() == CueState::Running)
                .map(|c| {
                    let active = c.active_child_id();
                    let any_running = c
                        .child_cues()
                        .map(|ch| ch.iter().any(|child| child.state() == CueState::Running))
                        .unwrap_or(false);
                    (c.id(), active, any_running)
                })
                .collect();
            for (id, active, any_running) in &current {
                let (prev_active, prev_running) =
                    prev_group_state.get(id).copied().unwrap_or((None, false));
                if *active != prev_active || *any_running != prev_running {
                    group_child_changed = true;
                }
            }
            for (id, active, any_running) in current {
                prev_group_state.insert(id, (active, any_running));
            }
        }
    }

    // 9. Fire captured Auto-Continue / Auto-Follow successors. Never consult
    // the mutable UI Playhead here: it may have moved during the source cue.
    for (list_id, source_id, token) in ordered_ready_continuations(&ready_continuations) {
        if let Some(cl) = ws.cue_list_by_id_mut(list_id) {
            let context = make_context(
                audio_engine,
                output_engine,
                dmx_engine,
                stop_fade_ms,
                ws_patches.clone(),
                ws_default_patch,
                ws_output_screen,
                ws_osc_patches.clone(),
                ws_fixtures.clone(),
                ws_fixture_groups.clone(),
                ws_input_patches.clone(),
                ws_buffer_size,
            );
            let mut transport = Transport::new(context);
            if let Ok(result) = transport.continue_from_source(cl, source_id, token) {
                all_go_fired.extend(result.fired);
                all_go_triggered.extend(result.triggered);
                all_go_stopped.extend(result.stopped);
            }
        }
    }

    // 9b. Fire cues whose timecode trigger was crossed this tick (section 0),
    //     through the same real GO path so playback actually starts and
    //     Auto-Continue / Auto-Follow chains still work.
    for (list_id, cue_id) in &tc_fire {
        if let Some(cl) = ws.cue_list_by_id_mut(*list_id) {
            let context = make_context(
                audio_engine,
                output_engine,
                dmx_engine,
                stop_fade_ms,
                ws_patches.clone(),
                ws_default_patch,
                ws_output_screen,
                ws_osc_patches.clone(),
                ws_fixtures.clone(),
                ws_fixture_groups.clone(),
                ws_input_patches.clone(),
                ws_buffer_size,
            );
            let mut transport = Transport::new(context);
            if let Ok(result) = transport.go_by_id(cl, cue_id) {
                all_go_fired.extend(result.fired);
                all_go_triggered.extend(result.triggered);
                all_go_stopped.extend(result.stopped);
            }
        }
    }

    // Capture final playhead for the GO event (active list only, matching QLab).
    let go_final_playhead: Option<CueId> = if !all_go_triggered.is_empty() {
        ws.active_cue_list().and_then(|cl| cl.playhead_cue_id)
    } else {
        None
    };

    // ------------------------------------------------------------------
    // 10. Detect running-cue-set / playhead changes for OSC feedback
    //     (active cue list only — matches QLab OSC behavior).
    //
    // This is the ONLY consumer of the per-tick cue-list flatten + fingerprint,
    // which is O(cues) and, in a large show (200+ cues), a real 30 fps drag.
    // Skip it entirely unless OSC feedback is on (or a one-shot refresh was
    // requested) — the common case pays nothing.
    // ------------------------------------------------------------------
    // Non-consuming peek — the inner block still consumes the flags when it sends.
    let osc_feedback_active = crate::engine::osc_feedback::is_enabled()
        || crate::engine::osc_feedback::any_request_pending();
    let (running_payload, progress_payload, playhead_payload, cue_list_payload) =
        if !osc_feedback_active {
            (None, None, None, None)
        } else if let Some(active_cl) = ws.cue_list_by_id(active_list_id) {
            let running_now = all_running_cues_info(&active_cl.cues, audio_engine, output_engine);
            let playhead_now = active_cl
                .playhead_cue_id
                .and_then(|ph_id| find_cue_info(&active_cl.cues, ph_id));

            // Media progress: a continuous stream (rate-gated), unlike the
            // change-driven payloads below.  Same slot order as send_running.
            let progress_p: Option<Vec<crate::engine::osc_feedback::ProgressSlot>> =
                if crate::engine::osc_feedback::progress_due() {
                    Some(running_now.iter().map(|(_, _, _, p)| *p).collect())
                } else {
                    None
                };

            let running_ids: Vec<CueId> = running_now.iter().map(|(id, _, _, _)| *id).collect();
            let running_p: Option<Vec<(String, String)>> = if running_ids != *prev_running_cues {
                *prev_running_cues = running_ids;
                Some(
                    running_now
                        .into_iter()
                        .map(|(_, n, name, _)| (n, name))
                        .collect(),
                )
            } else {
                None
            };

            let playhead_p: Option<(String, String)> = {
                let id = playhead_now.as_ref().map(|(id, _, _)| *id);
                if id != *prev_playhead_cue || crate::engine::osc_feedback::is_playhead_requested()
                {
                    *prev_playhead_cue = id;
                    Some(
                        playhead_now
                            .map(|(_, n, name)| (n, name))
                            .unwrap_or_default(),
                    )
                } else {
                    None
                }
            };

            let all_cues = all_cues_flat(&active_cl.cues);
            let cue_list_hash = fingerprint_cue_list(&all_cues);
            let cue_list_p: Option<Vec<(String, String)>> = if cue_list_hash != *prev_cue_list_hash
                || crate::engine::osc_feedback::is_cue_list_requested()
            {
                *prev_cue_list_hash = cue_list_hash;
                Some(all_cues)
            } else {
                None
            };

            (running_p, progress_p, playhead_p, cue_list_p)
        } else {
            (None, None, None, None)
        };

    // Rename for clarity in the rest of the function.
    let newly_completed = all_newly_completed;
    let time_snapshots = all_time_snapshots;
    let go_triggered = all_go_triggered;
    let go_stopped = all_go_stopped;

    drop(ws);

    if let Some(cues) = running_payload {
        crate::engine::osc_feedback::send_running(&cues);
    }
    if let Some(slots) = progress_payload {
        crate::engine::osc_feedback::send_progress(&slots);
    }
    if let Some((number, name)) = playhead_payload {
        crate::engine::osc_feedback::send_playhead(&number, &name);
    }
    if let Some(cues) = cue_list_payload {
        crate::engine::osc_feedback::send_cue_list(&cues);
    }

    // ------------------------------------------------------------------
    // 11. Emit all events.
    // ------------------------------------------------------------------

    for (cue_id, _, _) in &newly_completed {
        let _ = handle.emit(
            "cue-state-changed",
            serde_json::json!({
                "cue_id": cue_id,
                "old_state": "running",
                "new_state": "standby",
            }),
        );
    }

    // `triggered` also includes command targets whose state was changed by
    // Pause / Reset / Arm. Fire history uses the transport's exact GO targets.
    for cue_id in &all_go_fired {
        let _ = handle.emit("cue-fired", serde_json::json!({ "cue_id": cue_id }));
    }

    // Audio-freeze pause / resume (device-loss timeline guard).
    for cue_id in &just_paused {
        let _ = handle.emit(
            "cue-state-changed",
            serde_json::json!({
                "cue_id": cue_id, "old_state": "running", "new_state": "paused",
            }),
        );
    }
    for cue_id in &just_resumed {
        let _ = handle.emit(
            "cue-state-changed",
            serde_json::json!({
                "cue_id": cue_id, "old_state": "paused", "new_state": "running",
            }),
        );
    }

    // Emit playhead-moved for each sequential-group completion advance.
    for new_ph in &all_seq_group_playheads {
        let _ = handle.emit("playhead-moved", serde_json::json!({ "cue_id": new_ph }));
    }

    for (cue_id, elapsed_ms, action_elapsed_ms, remaining_ms, media_position_ms) in &time_snapshots
    {
        let _ = handle.emit(
            "cue-time-update",
            serde_json::json!({
                "cue_id": cue_id,
                "elapsed_ms": elapsed_ms,
                "action_elapsed_ms": action_elapsed_ms,
                "remaining_ms": remaining_ms,
                "media_position_ms": media_position_ms,
            }),
        );
    }

    // Emit cue-list-refresh when a sequential group's active child changed.
    if group_child_changed {
        let _ = handle.emit("cue-list-refresh", serde_json::json!({}));
    }

    // Camera/SRT diagnostics are held on the cue rather than emitted as a
    // transient toast, so an operator can inspect the red error or yellow
    // warning badge after the moment of failure. Refresh only on a change,
    // not once per 30 fps tick.
    if runtime_diagnostic_changed {
        let _ = handle.emit("cue-list-refresh", serde_json::json!({}));
    }
    for (cue_id, _) in &runtime_tick_failures {
        let _ = handle.emit(
            "cue-state-changed",
            serde_json::json!({
                "cue_id": cue_id,
                "old_state": "running",
                "new_state": "standby",
            }),
        );
    }

    for stopped_id in &go_stopped {
        let _ = handle.emit(
            "cue-state-changed",
            serde_json::json!({
                "cue_id": stopped_id,
                "old_state": "running",
                "new_state": "standby",
            }),
        );
    }

    if !go_triggered.is_empty() {
        if let Some(phid) = go_final_playhead {
            let _ = handle.emit("playhead-moved", serde_json::json!({ "cue_id": phid }));
        }
        for triggered_id in &go_triggered {
            let _ = handle.emit(
                "cue-state-changed",
                serde_json::json!({
                    "cue_id": triggered_id,
                    "old_state": "standby",
                    "new_state": "running",
                }),
            );
        }
    }

    if emit_workspace_modified {
        let _ = handle.emit("workspace-modified", serde_json::json!({}));
    }

    // ------------------------------------------------------------------
    // 12. Garbage-collect finished audio voices.
    // ------------------------------------------------------------------
    audio_engine.gc_voices();
    if let Some(ended) = prune_preview_session(preview_session, audio_engine, &completed_voice_ids)
    {
        emit_preview_playhead(handle, ended.clone(), None, false, false);
        if *last_preview_session == Some(ended) {
            *last_preview_session = None;
        }
    }
    publish_preview_playhead(handle, preview_session, audio_engine, last_preview_session);
}

// ---------------------------------------------------------------------------
// Fast timer refresh (runs on its own thread at ~60 fps)
// ---------------------------------------------------------------------------

/// Runs on a dedicated thread.  Reads the current running-cue position at
/// `TIMER_TICK_MS` intervals and updates the mpv OSD timer overlay.
///
/// Using a separate thread (rather than doing it inside the 30 fps main tick)
/// lets the millisecond display update smoothly without coupling the refresh
/// rate to all the other, heavier work the main tick performs.
fn timer_refresh_loop(workspace: Arc<Mutex<Workspace>>, output_engine: Arc<OutputEngine>) {
    loop {
        std::thread::sleep(Duration::from_millis(TIMER_TICK_MS));

        // Non-blocking lock — skip this frame if a command handler holds the lock.
        let Ok(ws) = workspace.try_lock() else {
            continue;
        };

        let show = ws.preferences.display.show_output_timer;
        let floating = ws.preferences.display.timer_floating;
        let countdn = ws.preferences.display.timer_count_down;
        let show_ms = ws.preferences.display.timer_show_ms;

        // Preview mode overrides live cue time — show placeholder regardless of
        // whether a cue is playing or the show_output_timer setting.
        let preview = output_engine.get_timer_preview();
        let live_text = ws
            .active_cue_list()
            .and_then(|cl| first_running_timer_text(&cl.cues, countdn, show_ms));
        let text = if preview.is_some() {
            preview
        } else if show {
            live_text.clone()
        } else {
            None
        };
        drop(ws); // release workspace lock before calling into mpv

        if show && floating {
            // Floating mode: drive the Win32 window, silence the OSD.
            output_engine.set_output_timer(None);
            output_engine.update_floating_timer(text.as_deref());
        } else {
            // Normal mode: drive the OSD, clear the floating window.
            output_engine.set_output_timer(text.as_deref());
            output_engine.update_floating_timer(None);
        }
    }
}

/// Find the first running cue with time data (recursive — checks group children)
/// and format its position as a timer string.
fn first_running_timer_text(
    cues: &[Box<dyn crate::cue::traits::Cue>],
    count_down: bool,
    show_ms: bool,
) -> Option<String> {
    for cue in cues {
        if cue.state() == CueState::Running {
            let ms = if count_down {
                let remaining = cue
                    .duration()?
                    .as_millis()
                    .saturating_sub(cue.action_elapsed().as_millis());
                remaining as u64
            } else {
                cue.action_elapsed().as_millis() as u64
            };
            return Some(format_timer(ms, show_ms));
        }
        if let Some(children) = cue.child_cues() {
            if let Some(text) = first_running_timer_text(children, count_down, show_ms) {
                return Some(text);
            }
        }
    }
    None
}

/// Return `(id, number, name)` for the cue with the given ID (recursive lookup).
fn find_cue_info(
    cues: &[Box<dyn crate::cue::traits::Cue>],
    target: CueId,
) -> Option<(CueId, String, String)> {
    for cue in cues {
        if cue.id() == target {
            return Some((
                cue.id(),
                cue.number().unwrap_or("").to_owned(),
                cue.name().to_owned(),
            ));
        }
        if let Some(children) = cue.child_cues() {
            if let Some(found) = find_cue_info(children, target) {
                return Some(found);
            }
        }
    }
    None
}

/// Return the owning cue for an audio voice. AudioCue exposes its own voice
/// directly; VideoCue exposes the visual voice, so resolve its paired audio
/// voice through the output engine.
fn find_audio_cue_info(
    cue_lists: &[CueList],
    target_voice: crate::engine::ring_command::VoiceId,
    output_engine: &Arc<OutputEngine>,
) -> Option<(CueId, String, String)> {
    cue_lists.iter().find_map(|cue_list| {
        find_audio_cue_info_in_cues(&cue_list.cues, target_voice, output_engine)
    })
}

fn find_audio_cue_info_in_cues(
    cues: &[Box<dyn crate::cue::traits::Cue>],
    target_voice: crate::engine::ring_command::VoiceId,
    output_engine: &Arc<OutputEngine>,
) -> Option<(CueId, String, String)> {
    for cue in cues {
        if cue.playing_voice_id() == Some(target_voice)
            || cue
                .playing_voice_id()
                .and_then(|visual_voice| output_engine.video_audio_voice(visual_voice))
                == Some(target_voice)
        {
            return Some((
                cue.id(),
                cue.number().unwrap_or("").to_owned(),
                cue.name().to_owned(),
            ));
        }
        if let Some(children) = cue.child_cues() {
            if let Some(found) = find_audio_cue_info_in_cues(children, target_voice, output_engine)
            {
                return Some(found);
            }
        }
    }
    None
}

/// Collect `(id, number, name)` for every running cue (recursive, ordered).
fn all_running_cues_info(
    cues: &[Box<dyn crate::cue::traits::Cue>],
    audio_engine: &Arc<AudioEngine>,
    output_engine: &Arc<OutputEngine>,
) -> Vec<(
    CueId,
    String,
    String,
    crate::engine::osc_feedback::ProgressSlot,
)> {
    let mut out = Vec::new();
    collect_running(cues, audio_engine, output_engine, &mut out);
    out
}

fn collect_running(
    cues: &[Box<dyn crate::cue::traits::Cue>],
    audio_engine: &Arc<AudioEngine>,
    output_engine: &Arc<OutputEngine>,
    out: &mut Vec<(
        CueId,
        String,
        String,
        crate::engine::osc_feedback::ProgressSlot,
    )>,
) {
    for cue in cues {
        if cue.state() == CueState::Running {
            let elapsed = cue
                .media_position_ms(audio_engine, output_engine)
                .unwrap_or_else(|| cue.action_elapsed().as_millis() as u64);
            let progress = progress_values(
                elapsed,
                cue.duration().map(|d| d.as_millis() as u64),
                cue.file_duration().map(|d| d.as_millis() as u64),
            );
            out.push((
                cue.id(),
                cue.number().unwrap_or("").to_owned(),
                cue.name().to_owned(),
                progress,
            ));
        }
        if let Some(children) = cue.child_cues() {
            collect_running(children, audio_engine, output_engine, out);
        }
    }
}

/// Collect `(number, name)` for every cue in display order (recursive).
fn all_cues_flat(cues: &[Box<dyn crate::cue::traits::Cue>]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    collect_all_flat(cues, &mut out);
    out
}

fn collect_all_flat(cues: &[Box<dyn crate::cue::traits::Cue>], out: &mut Vec<(String, String)>) {
    for cue in cues {
        out.push((cue.number().unwrap_or("").to_owned(), cue.name().to_owned()));
        if let Some(children) = cue.child_cues() {
            collect_all_flat(children, out);
        }
    }
}

/// Cheap fingerprint of the full cue list (number + name pairs).
fn fingerprint_cue_list(cues: &[(String, String)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cues.hash(&mut h);
    h.finish()
}

fn format_timer(ms: u64, show_ms: bool) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    if show_ms {
        let millis = ms % 1000;
        format!("{mins:02}:{secs:02}.{millis:03}")
    } else {
        format!("{mins:02}:{secs:02}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    #[cfg(test)]
    mod tests {
    use super::{
        ordered_ready_continuations, pending_continuation_is_valid,
        pending_continuation_status, preview_playhead_payload, progress_values,
        should_complete_with_voice_state, PendingContinuation, PendingContinuationStatus,
    };
    use crate::cue::{memo_cue::MemoCue, traits::Cue, types::ContinueMode, wait_cue::WaitCue};
    use crate::show::cue_list::CueList;
    use crate::state::PreviewSession;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    #[test]
    fn ready_continuations_have_deterministic_list_and_token_order() {
        let list_a = Uuid::from_u128(1);
        let list_b = Uuid::from_u128(2);
        let source_a = Uuid::from_u128(10);
        let source_b = Uuid::from_u128(11);
        let source_other_list = Uuid::from_u128(12);
        let ready = std::collections::HashSet::from([
            (list_b, source_other_list, 0),
            (list_a, source_b, 2),
            (list_a, source_b, 1),
            (list_a, source_a, 1),
        ]);

        assert_eq!(
            ordered_ready_continuations(&ready),
            vec![
                (list_a, source_a, 1),
                (list_a, source_b, 1),
                (list_a, source_b, 2),
                (list_b, source_other_list, 0),
            ]
        );
    }

    #[test]
    fn live_voice_wins_over_time_done_and_stale_completed_status() {
        assert!(!should_complete_with_voice_state(true, true, false, true));
        assert!(!should_complete_with_voice_state(true, true, true, true));
        assert!(should_complete_with_voice_state(true, false, true, false));
        assert!(should_complete_with_voice_state(true, false, false, true));
        assert!(!should_complete_with_voice_state(false, false, true, false));
        assert!(should_complete_with_voice_state(false, false, false, true));
    }

    #[test]
    fn delayed_auto_follow_is_scoped_to_the_same_execution_not_the_ui_playhead() {
        let mut cue_list = CueList::new("Auto-Follow token test");
        let mut cue = WaitCue::new();
        cue.set_continue_mode(ContinueMode::AutoFollow);
        cue.mark_auto_continue_fired();
        let cue_id = cue.id();
        cue_list.push(Box::new(cue));
        let generation = cue_list.get(&cue_id).unwrap().play_generation();
        let targets = cue_list.continuation_successors(cue_id).unwrap();
        let token = cue_list.bind_continuation(cue_id, generation, targets);
        let pending = PendingContinuation {
            due: Instant::now() + Duration::from_secs(1),
            list_id: cue_list.id,
            source_generation: generation,
            continuation_token: token,
            continue_mode: ContinueMode::AutoFollow,
        };

        assert!(pending_continuation_is_valid(
            &pending,
            cue_id,
            Some(&cue_list)
        ));

        cue_list
            .get_mut(&cue_id)
            .unwrap()
            .clear_auto_continue_fired();
        assert!(!pending_continuation_is_valid(
            &pending,
            cue_id,
            Some(&cue_list)
        ));
        cue_list
            .get_mut(&cue_id)
            .unwrap()
            .mark_auto_continue_fired();

        let other_list = CueList::new("Different list");
        assert!(!pending_continuation_is_valid(
            &pending,
            cue_id,
            Some(&other_list)
        ));

        cue_list.playhead_cue_id = Some(Uuid::new_v4());
        assert!(pending_continuation_is_valid(
            &pending,
            cue_id,
            Some(&cue_list)
        ));
    }

    #[test]
    fn delayed_auto_follow_rejects_cues_without_a_fired_execution_marker() {
        let mut cue_list = CueList::new("Unscoped Auto-Follow token test");
        let mut cue = MemoCue::new();
        cue.set_continue_mode(ContinueMode::AutoFollow);
        let cue_id = cue.id();
        cue_list.push(Box::new(cue));
        let generation = cue_list.get(&cue_id).unwrap().play_generation();
        let targets = cue_list.continuation_successors(cue_id).unwrap();
        let token = cue_list.bind_continuation(cue_id, generation, targets);
        let pending = PendingContinuation {
            due: Instant::now() + Duration::from_secs(1),
            list_id: cue_list.id,
            source_generation: generation,
            continuation_token: token,
            continue_mode: ContinueMode::AutoFollow,
        };

        assert!(!pending_continuation_is_valid(
            &pending,
            cue_id,
            Some(&cue_list)
        ));
    }

    #[test]
    fn instant_memo_auto_follow_waits_for_deadline_and_reset_invalidates_it() {
        let mut cue_list = CueList::new("Instant Auto-Follow marker test");
        let mut cue = MemoCue::new();
        cue.set_continue_mode(ContinueMode::AutoFollow);
        cue.set_post_wait(Duration::from_millis(50));
        cue.mark_auto_continue_fired();
        let cue_id = cue.id();
        cue_list.push(Box::new(cue));
        let generation = cue_list.get(&cue_id).unwrap().play_generation();
        let targets = cue_list.continuation_successors(cue_id).unwrap();
        let token = cue_list.bind_continuation(cue_id, generation, targets);
        let pending = PendingContinuation {
            due: Instant::now() + Duration::from_millis(50),
            list_id: cue_list.id,
            source_generation: generation,
            continuation_token: token,
            continue_mode: ContinueMode::AutoFollow,
        };

        assert_eq!(
            pending_continuation_status(&pending, cue_id, Some(&cue_list), Instant::now(),),
            PendingContinuationStatus::Waiting,
            "an instant Auto-Follow cue with a fired marker must remain pending until its deadline"
        );
        let due_pending = PendingContinuation {
            due: Instant::now() - Duration::from_millis(1),
            ..pending.clone()
        };
        assert_eq!(
            pending_continuation_status(&due_pending, cue_id, Some(&cue_list), Instant::now(),),
            PendingContinuationStatus::Due,
            "a valid elapsed delay must enqueue the Auto-Follow GO"
        );

        cue_list.get_mut(&cue_id).unwrap().reset().unwrap();
        assert_eq!(
            pending_continuation_status(&pending, cue_id, Some(&cue_list), Instant::now(),),
            PendingContinuationStatus::Invalid,
            "Reset before the deadline must cancel the delayed chain"
        );
        assert_eq!(
            pending_continuation_status(&due_pending, cue_id, Some(&cue_list), Instant::now(),),
            PendingContinuationStatus::Invalid,
            "Reset must also prevent an already-due stale chain"
        );
    }

    #[test]
    fn completed_auto_continue_can_keep_a_post_wait_deadline() {
        let mut cue_list = CueList::new("Completed Auto-Continue marker test");
        let mut cue = MemoCue::new();
        cue.set_continue_mode(ContinueMode::AutoContinue);
        cue.set_post_wait(Duration::from_millis(50));
        cue.mark_auto_continue_fired();
        let cue_id = cue.id();
        cue_list.push(Box::new(cue));
        let generation = cue_list.get(&cue_id).unwrap().play_generation();
        let targets = cue_list.continuation_successors(cue_id).unwrap();
        let token = cue_list.bind_continuation(cue_id, generation, targets);
        let pending = PendingContinuation {
            due: Instant::now() + Duration::from_millis(50),
            list_id: cue_list.id,
            source_generation: generation,
            continuation_token: token,
            continue_mode: ContinueMode::AutoContinue,
        };

        assert_eq!(
            pending_continuation_status(&pending, cue_id, Some(&cue_list), Instant::now()),
            PendingContinuationStatus::Waiting,
            "a naturally completed Auto-Continue source may wait out the remaining post-wait"
        );
        cue_list.get_mut(&cue_id).unwrap().reset().unwrap();
        assert_eq!(
            pending_continuation_status(&pending, cue_id, Some(&cue_list), Instant::now()),
            PendingContinuationStatus::Invalid,
            "an explicit reset cancels the remaining Auto-Continue"
        );
    }

    #[test]
    fn progress_with_known_duration() {
        let (progress, elapsed, remaining, duration) =
            progress_values(12_600, Some(30_000), Some(30_000));
        assert!((progress - 0.42).abs() < 1e-3);
        assert!((elapsed - 12.6).abs() < 1e-3);
        assert!((remaining - 17.4).abs() < 1e-3);
        assert!((duration - 30.0).abs() < 1e-3);
    }

    #[test]
    fn progress_clamps_past_the_end() {
        let (progress, _, remaining, _) = progress_values(31_000, Some(30_000), None);
        assert_eq!(progress, 1.0);
        assert_eq!(remaining, 0.0);
    }

    #[test]
    fn unknown_duration_falls_back_to_file_position() {
        // Vamp / infinite loop: no total duration — the gauge follows the
        // position within one file pass; remaining/duration report -1.
        let (progress, elapsed, remaining, duration) = progress_values(75_000, None, Some(60_000));
        assert!((progress - 0.25).abs() < 1e-3, "75s % 60s = 15s / 60s");
        assert!((elapsed - 75.0).abs() < 1e-3);
        assert_eq!(remaining, -1.0);
        assert_eq!(duration, -1.0);
    }

    #[test]
    fn nothing_known_reports_zero_progress() {
        // Live feed: no duration at all.
        let (progress, elapsed, remaining, duration) = progress_values(5_000, None, None);
        assert_eq!(progress, 0.0);
        assert!((elapsed - 5.0).abs() < 1e-3);
        assert_eq!(remaining, -1.0);
        assert_eq!(duration, -1.0);
    }

    #[test]
    fn preview_playhead_payload_is_file_relative_and_generation_scoped() {
        let session = PreviewSession::new(Uuid::new_v4(), vec![Uuid::new_v4()], 17).unwrap();
        let payload = preview_playhead_payload(session.clone(), Some(12_345), true, true);

        assert_eq!(payload["cue_id"], serde_json::json!(session.cue_id));
        assert_eq!(payload["media_position_ms"], 12_345);
        assert_eq!(payload["playing"], true);
        assert_eq!(payload["active"], true);
        assert_eq!(payload["generation"], 17);
    }
}
