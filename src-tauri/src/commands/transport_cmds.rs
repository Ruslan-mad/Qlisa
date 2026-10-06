//! Tauri commands for transport control (GO, STOP, PAUSE, RESUME).

use std::sync::atomic::Ordering;

use tauri::{Emitter, State};

use crate::{
    cue::{
        context::{CueContext, CueEvent},
        traits::Cue,
        types::{CueState, CueType},
    },
    show::{cue_list::CueList, transport::Transport},
    state::AppState,
};
use crate::engine::output_engine::BrowserSurfaceState;


// ---------------------------------------------------------------------------
// Shared helper
// ---------------------------------------------------------------------------

/// Build a [`CueContext`] wired to both engines, with a snapshot of the
/// workspace's Output Patch table and current audio device so cues can
/// resolve their patch and route video audio at GO time.
///
/// `stop_fade_ms` comes from `ws.preferences.audio.default_fade_out_ms` and
/// is used by [`AudioCue::stop`] when no per-cue fade-out spec is set.
///
/// Note: when the caller already holds the workspace lock, the `try_lock`
/// below fails and the context falls back to empty patch tables — fine for
/// stop paths, which only need the engines and `stop_fade_ms`.
pub(crate) fn make_context(state: &AppState, stop_fade_ms: u32) -> CueContext {
    let (tx, _rx) = crossbeam_channel::unbounded::<CueEvent>();
    let (patches, default_patch_id, output_screen, osc_patches, fixtures, groups, input_patches, audio_buffer_size) = state
        .workspace
        .try_lock()
        .map(|ws| (
            ws.output_patches.clone(),
            ws.default_output_patch_id,
            ws.preferences.display.output_screen,
            ws.osc_patches.clone(),
            ws.fixtures.clone(),
            ws.fixture_groups.clone(),
            ws.input_patches.clone(),
            ws.preferences.audio.audio_buffer_size,
        ))
        .unwrap_or_else(|_| (Vec::new(), None, None, Vec::new(), Vec::new(), Vec::new(), Vec::new(), 256));
    CueContext::new(
        state.audio_engine.clone(),
        state.output_engine.clone(),
        tx,
        stop_fade_ms,
        patches,
        default_patch_id,
        output_screen,
        osc_patches,
        state.dmx_engine.clone(),
        fixtures,
        groups,
        input_patches,
        audio_buffer_size,
    )
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Return the current snapshot for the one shared Browser WebView. This lets
/// a newly-opened Inspector render an already-running surface immediately;
/// subsequent changes arrive through `browser-surface-state` events.
#[tauri::command]
pub fn get_browser_surface_state(
    state: State<'_, AppState>,
) -> Result<BrowserSurfaceState, String> {
    Ok(state.output_engine.browser_surface_state())
}

/// Trigger the cue at the Playhead.
///
/// Audio files must already be decoded (pre-loaded) before GO is called.
/// Decoding is triggered automatically when a workspace is loaded or when a
/// file is assigned to a cue.  GO never decodes — if an Audio Cue is still
/// loading it is silently skipped so the command always returns instantly.
/// Video Cues stream directly from disk and are never skipped.
#[tauri::command]
pub fn go(state: State<'_, AppState>, app_handle: tauri::AppHandle) -> Result<(), String> {
    // Double-GO protection: silently ignore a second GO that arrives within
    // `double_go_protection_ms` of the previous one (default 500 ms).
    // This catches duplicate UDP packets from OSC controllers and accidental
    // rapid double-presses without affecting intentional fast GOs.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let protection_ms = ws.preferences.general.double_go_protection_ms as u64;
    if protection_ms > 0 {
        let last = state.last_go_at.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) < protection_ms {
            return Ok(());
        }
    }
    state.last_go_at.store(now_ms, Ordering::Relaxed);

    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;

    let (tx, _rx) = crossbeam_channel::unbounded::<CueEvent>();
    let context = CueContext::new(
        state.audio_engine.clone(),
        state.output_engine.clone(),
        tx,
        stop_fade_ms,
        ws.output_patches.clone(),
        ws.default_output_patch_id,
        ws.preferences.display.output_screen,
        ws.osc_patches.clone(),
        state.dmx_engine.clone(),
        ws.fixtures.clone(),
        ws.fixture_groups.clone(),
        ws.input_patches.clone(),
        ws.preferences.audio.audio_buffer_size,
    );
    let mut transport = Transport::new(context);

    let active_list_id = ws.active_cue_list_id;
    let active_index = ws.cue_lists.iter().position(|list| list.id == active_list_id)
        .ok_or("No active cue list")?;
    let cue_list = &ws.cue_lists[active_index];

    if let Some(cue) = cue_list.playhead_cue() {
        let issues = crate::cue::group_cue::number_readiness_issues(cue);
        if let Some(issue) = issues.first() {
            return Err(format!("Number is not ready: {issue}"));
        }
    }

    // If the Audio Cue at the playhead has no decoded samples yet (still
    // loading), skip silently.  Video Cues are exempt — they stream directly
    // from disk and need no pre-loading step.
    // NOTE: use file_duration() (raw decoded length) rather than duration()
    // to avoid blocking infinite-loop cues (loop_count = u32::MAX makes
    // duration() return None even when the file is fully decoded).
    if let Some(cue) = cue_list.playhead_cue() {
        if cue.cue_type() == CueType::Audio
            && cue.file_duration().is_none()
            && cue
                .serialize()
                .get("file_path")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        {
            return Ok(());
        }
    }

    let before_states = workspace_cue_states(&ws.cue_lists);
    let before_playheads = workspace_playheads(&ws.cue_lists);
    let result = transport.go_in_workspace(&mut ws.cue_lists, active_index).map_err(|e| e.to_string())?;

    for id in &result.fired {
        let _ = app_handle.emit("cue-fired", serde_json::json!({ "cue_id": id }));
    }

    emit_workspace_state_changes(&app_handle, &before_states, &workspace_cue_states(&ws.cue_lists));
    emit_remote_playhead_changes(&app_handle, &before_playheads, &ws.cue_lists, active_list_id);

    // Always emit — even when playhead_cue_id is None (cue was last in list).
    let _ = app_handle.emit("playhead-moved", serde_json::json!({
        "cue_list_id": active_list_id,
        "cue_id": ws.cue_lists[active_index].playhead_cue_id.map(|u| u.to_string())
    }));
    // Refresh the cue list so the frontend sees updated group inner-playhead state.
    let _ = app_handle.emit("cue-list-refresh", serde_json::json!({}));
    Ok(())
}

/// Trigger a specific cue by ID (used in Cart Mode).
///
/// Fires the given cue without moving either the active list or its Playhead.
/// The same loading guard as `go` applies to Audio Cues whose file is loading.
#[tauri::command]
pub fn go_cue(
    cue_id: String,
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id: uuid::Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let active_list_id = ws.active_cue_list_id;
    let active_playhead_before = ws.active_cue_list().and_then(|list| list.playhead_cue_id);
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let context = make_context(&state, stop_fade_ms);
    let mut transport = Transport::new(context);
    if let Some(cue) = ws.cue_lists.iter().find_map(|list| list.get_recursive(&id)) {
        let issues = crate::cue::group_cue::number_readiness_issues(cue);
        if let Some(issue) = issues.first() {
            return Err(format!("Number is not ready: {issue}"));
        }
        if cue.cue_type() == CueType::Audio
            && cue.file_duration().is_none()
            && cue
                .serialize()
                .get("file_path")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        {
            return Ok(());
        }
    }

    let before_states = workspace_cue_states(&ws.cue_lists);
    let before_playheads = workspace_playheads(&ws.cue_lists);
    let result = transport.go_by_id_in_workspace(&mut ws.cue_lists, &id).map_err(|e| e.to_string())?;

    for fired_id in &result.fired {
        let _ = app_handle.emit("cue-fired", serde_json::json!({ "cue_id": fired_id }));
    }

    emit_workspace_state_changes(&app_handle, &before_states, &workspace_cue_states(&ws.cue_lists));
    emit_remote_playhead_changes(&app_handle, &before_playheads, &ws.cue_lists, ws.active_cue_list_id);
    let active_playhead_after = ws.active_cue_list().and_then(|list| list.playhead_cue_id);
    if active_playhead_after != active_playhead_before {
        let _ = app_handle.emit("playhead-moved", serde_json::json!({
            "cue_list_id": active_list_id,
            "cue_id": active_playhead_after,
        }));
    }
    let _ = app_handle.emit("cue-list-refresh", serde_json::json!({}));
    Ok(())
}

/// Stop all running cues with a soft fade-out.
#[tauri::command]
pub fn stop_all(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let context = make_context(&state, stop_fade_ms);
    let mut transport = Transport::new(context);
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let before = cue_list_states(cue_list);
    transport.stop_all(cue_list).map_err(|e| e.to_string())?;
    emit_stop_notifications(&app_handle, &before, &cue_list_states(cue_list));
    Ok(())
}

/// Hard-stop all running cues (immediate cut, no fades).
#[tauri::command]
pub fn hard_stop_all(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let context = make_context(&state, stop_fade_ms);
    let mut transport = Transport::new(context);
    let before = workspace_cue_states(&ws.cue_lists);
    transport
        .hard_stop_all(&mut ws.cue_lists)
        .map_err(|e| e.to_string())?;
    emit_stop_notifications(&app_handle, &before, &workspace_cue_states(&ws.cue_lists));
    Ok(())
}

/// Stop a specific cue with a soft fade-out.
#[tauri::command]
pub fn stop_cue(
    cue_id: String,
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id: uuid::Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let context = make_context(&state, stop_fade_ms);
    let mut transport = Transport::new(context);
    let before = ws.cue_lists.iter().find_map(|list| list.get_recursive(&id))
        .map(cue_subtree_states)
        .ok_or_else(|| format!("Cue not found: {id:?}"))?;
    transport.stop_cue_in_workspace(&mut ws.cue_lists, &id).map_err(|e| e.to_string())?;
    let after = ws.cue_lists.iter().find_map(|list| list.get_recursive(&id))
        .map(cue_subtree_states)
        .unwrap_or_default();
    emit_stop_notifications(&app_handle, &before, &after);
    Ok(())
}

/// Pause a specific cue.
#[tauri::command]
pub fn pause_cue(
    cue_id: String,
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id: uuid::Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let context = make_context(&state, stop_fade_ms);
    let mut transport = Transport::new(context);
    let old_state = ws.cue_lists.iter().find_map(|list| list.get_recursive(&id)).map(|cue| cue.state()).ok_or_else(|| format!("Cue not found: {id:?}"))?;
    transport.pause_cue_in_workspace(&mut ws.cue_lists, &id).map_err(|e| e.to_string())?;
    let new_state = ws.cue_lists.iter().find_map(|list| list.get_recursive(&id)).map(|cue| cue.state()).ok_or_else(|| format!("Cue not found: {id:?}"))?;
    if let Some(payload) = cue_state_change_payload(&cue_id, old_state, new_state) {
        let _ = app_handle.emit("cue-state-changed", payload);
    }
    Ok(())
}

/// Set the master output gain from a dB value.
///
/// The gain is applied atomically in the audio callback without any lock.
/// Values ≤ −60 dB are treated as silence (gain = 0.0).
#[tauri::command]
pub fn set_master_volume(db: f32, state: State<'_, AppState>) -> Result<(), String> {
    let gain = if db <= -60.0 {
        0.0_f32
    } else {
        10_f32.powf(db / 20.0)
    };
    state.audio_engine.set_master_gain(gain);
    Ok(())
}

/// Resume a paused cue.
#[tauri::command]
pub fn resume_cue(
    cue_id: String,
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id: uuid::Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let context = make_context(&state, stop_fade_ms);
    let mut transport = Transport::new(context);
    let old_state = ws.cue_lists.iter().find_map(|list| list.get_recursive(&id)).map(|cue| cue.state()).ok_or_else(|| format!("Cue not found: {id:?}"))?;
    transport.resume_cue_in_workspace(&mut ws.cue_lists, &id).map_err(|e| e.to_string())?;
    let new_state = ws.cue_lists.iter().find_map(|list| list.get_recursive(&id)).map(|cue| cue.state()).ok_or_else(|| format!("Cue not found: {id:?}"))?;
    if let Some(payload) = cue_state_change_payload(&cue_id, old_state, new_state) {
        let _ = app_handle.emit("cue-state-changed", payload);
    }
    Ok(())
}

/// Seek a running or paused cue to `position_ms` from its action start.
///
/// No-op for non-seekable cue types (Memo, Stop, …).  Does not change the
/// cue's [`CueState`] — the cue keeps running or stays paused at the new
/// position.
#[tauri::command]
pub fn seek_cue(
    cue_id: String,
    position_ms: u64,
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id: uuid::Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let context = make_context(&state, stop_fade_ms);
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    if let Some(cue) = cue_list.get_mut_recursive(&id) {
        cue.seek(position_ms, &context);
        emit_seek_timing(&app_handle, cue, cue.action_elapsed().as_millis() as u64);
    }
    Ok(())
}

/// Seek a running or paused media cue using a file-relative position.  This is
/// the contract for waveform/filmstrip timeline clicks; the legacy
/// `seek_cue` command remains action-relative for the Inspector scrub bar.
#[tauri::command]
pub fn seek_cue_media(
    cue_id: String,
    file_position_ms: u64,
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id: uuid::Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let context = make_context(&state, stop_fade_ms);
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    if let Some(cue) = cue_list.get_mut_recursive(&id) {
        cue.seek_file_position(file_position_ms, &context);
        let action_ms = cue.action_elapsed().as_millis() as u64;
        emit_seek_timing(&app_handle, cue, action_ms);
    }
    Ok(())
}

fn emit_seek_timing(
    app_handle: &tauri::AppHandle,
    cue: &dyn crate::cue::traits::Cue,
    action_position_ms: u64,
) {
    let remaining_ms = cue
        .duration()
        .map(|d| (d.as_millis() as u64).saturating_sub(action_position_ms));
    let payload = cue_time_update_payload(
        cue.id(),
        cue.elapsed().as_millis() as u64,
        action_position_ms,
        remaining_ms,
        cue.media_position_for_action_ms(std::time::Duration::from_millis(action_position_ms)),
    );
    let _ = app_handle.emit("cue-time-update", payload);
}

fn cue_time_update_payload(
    cue_id: uuid::Uuid,
    elapsed_ms: u64,
    action_elapsed_ms: u64,
    remaining_ms: Option<u64>,
    media_position_ms: Option<u64>,
) -> serde_json::Value {
    serde_json::json!({
        "cue_id": cue_id,
        "elapsed_ms": elapsed_ms,
        "action_elapsed_ms": action_elapsed_ms,
        "remaining_ms": remaining_ms,
        "media_position_ms": media_position_ms,
    })
}

fn cue_state_change_payload(
    cue_id: &str,
    old_state: CueState,
    new_state: CueState,
) -> Option<serde_json::Value> {
    (old_state != new_state).then(|| serde_json::json!({
        "cue_id": cue_id,
        "old_state": old_state,
        "new_state": new_state,
    }))
}

type CueStateSnapshot = Vec<(uuid::Uuid, CueState)>;
type PlayheadSnapshot = Vec<(uuid::Uuid, Option<uuid::Uuid>)>;

fn workspace_playheads(cue_lists: &[CueList]) -> PlayheadSnapshot {
    cue_lists.iter().map(|list| (list.id, list.playhead_cue_id)).collect()
}

fn emit_remote_playhead_changes(
    app_handle: &tauri::AppHandle,
    before: &PlayheadSnapshot,
    cue_lists: &[CueList],
    active_list_id: uuid::Uuid,
) {
    for (list_id, old_playhead) in before {
        if *list_id == active_list_id { continue; }
        if let Some(list) = cue_lists.iter().find(|list| list.id == *list_id) {
            if list.playhead_cue_id != *old_playhead {
                let _ = app_handle.emit("playhead-moved", serde_json::json!({
                    "cue_list_id": list_id,
                    "cue_id": list.playhead_cue_id,
                }));
            }
        }
    }
}

fn cue_subtree_states(cue: &dyn Cue) -> CueStateSnapshot {
    let mut states = Vec::new();
    collect_cue_states(cue, &mut states);
    states
}

fn collect_cue_states(cue: &dyn Cue, states: &mut CueStateSnapshot) {
    states.push((cue.id(), cue.state()));
    if let Some(children) = cue.child_cues() {
        for child in children {
            collect_cue_states(child.as_ref(), states);
        }
    }
}

fn cue_list_states(cue_list: &CueList) -> CueStateSnapshot {
    let mut states = Vec::new();
    for cue in &cue_list.cues {
        collect_cue_states(cue.as_ref(), &mut states);
    }
    states
}

fn workspace_cue_states(cue_lists: &[CueList]) -> CueStateSnapshot {
    cue_lists.iter().flat_map(cue_list_states).collect()
}

fn emit_workspace_state_changes(
    app_handle: &tauri::AppHandle,
    before: &CueStateSnapshot,
    after: &CueStateSnapshot,
) {
    let after_by_id: std::collections::HashMap<_, _> = after.iter().copied().collect();
    for (id, old_state) in before {
        if let Some(&new_state) = after_by_id.get(id) {
            if let Some(payload) = cue_state_change_payload(&id.to_string(), *old_state, new_state) {
                let _ = app_handle.emit("cue-state-changed", payload);
            }
        }
    }
}

fn stop_notification_payloads(
    before: &[(uuid::Uuid, CueState)],
    after: &[(uuid::Uuid, CueState)],
) -> Vec<(&'static str, serde_json::Value)> {
    let after_by_id: std::collections::HashMap<_, _> = after.iter().copied().collect();
    let mut events = Vec::new();
    for (id, old_state) in before {
        if let Some(&new_state) = after_by_id.get(id) {
            if let Some(payload) =
                cue_state_change_payload(&id.to_string(), *old_state, new_state)
            {
                events.push(("cue-state-changed", payload));
            }
        }
    }
    // A STOP can also repair a stale frontend row when the engine was already
    // in Standby, so refresh after every successful stop command.
    events.push(("cue-list-refresh", serde_json::json!({})));
    events
}

fn emit_stop_notifications(
    app_handle: &tauri::AppHandle,
    before: &[(uuid::Uuid, CueState)],
    after: &[(uuid::Uuid, CueState)],
) {
    for (event, payload) in stop_notification_payloads(before, after) {
        let _ = app_handle.emit(event, payload);
    }
}

#[cfg(test)]
mod timing_tests {
    use super::cue_time_update_payload;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn seek_timing_payload_keeps_media_position_separate() {
        let id = Uuid::nil();
        let payload = cue_time_update_payload(id, 1_250, 250, Some(750), Some(2_250));
        assert_eq!(payload["cue_id"], json!(id));
        assert_eq!(payload["action_elapsed_ms"], 250);
        assert_eq!(payload["remaining_ms"], 750);
        assert_eq!(payload["media_position_ms"], 2_250);
    }
}

#[cfg(test)]
mod cue_state_event_tests {
    use super::cue_state_change_payload;
    use crate::cue::types::CueState;
    use serde_json::json;

    #[test]
    fn no_op_pause_does_not_publish_a_false_paused_state() {
        assert!(cue_state_change_payload("cue", CueState::Running, CueState::Running).is_none());
    }

    #[test]
    fn real_pause_publishes_the_actual_state_transition() {
        let payload = cue_state_change_payload("cue", CueState::Running, CueState::Paused).unwrap();
        assert_eq!(payload["old_state"], json!("running"));
        assert_eq!(payload["new_state"], json!("paused"));
    }
}

#[cfg(test)]
mod stop_tree_notification_tests {
    use super::{cue_list_states, stop_notification_payloads};
    use crate::{
        cue::{
            audio_cue::AudioCue,
            context::{CueContext, CueEvent},
            control_cue::{ControlAction, ControlCue},
            group_cue::GroupCue,
            memo_cue::MemoCue,
            stop_cue::StopCue,
            text_cue::TextCue,
            traits::{Cue, RuntimeState},
            types::{ContinueMode, CueState},
            video_cue::VideoCue,
        },
        engine::{
            audio_engine::AudioEngine,
            dmx_engine::DmxEngine,
            engine_traits::OutputEngineApi,
            output_engine::ContentRequest,
            ring_command::VoiceId,
        },
        preferences::MachineAudioConfig,
        show::{cue_list::CueList, transport::Transport},
    };
    use anyhow::Result;
    use crossbeam_channel::unbounded;
    use std::{sync::Arc, time::Duration};

    struct NullOutput;

    impl OutputEngineApi for NullOutput {
        fn show_content(&self, _req: ContentRequest<'_>) -> Result<VoiceId> {
            anyhow::bail!("unused")
        }
        fn stop_content(&self, _voice_id: VoiceId, _visual_fade_ms: u32, _audio_fade_ms: u32) {}
        fn hard_stop_current(&self) {}
        fn panic_stop(&self) {}
        fn video_audio_voice(&self, _voice_id: VoiceId) -> Option<VoiceId> {
            None
        }
        fn resync_audio_to_video(&self, _voice_id: VoiceId) {}
        fn get_voice_opacity(&self, _voice_id: VoiceId) -> f32 {
            1.0
        }
        fn set_voice_opacity(&self, _voice_id: VoiceId, _opacity: f32) {}
        fn stop_voice(&self, _voice_id: VoiceId, _fade_ms: u32) -> Result<()> {
            Ok(())
        }
        fn pause_voice(&self, _voice_id: VoiceId) -> Result<()> {
            Ok(())
        }
        fn resume_voice(&self, _voice_id: VoiceId) -> Result<()> {
            Ok(())
        }
        fn seek_voice_ms(&self, _voice_id: VoiceId, _position_ms: u64) {}
        fn show_text_overlay(&self, _ass_text: &str, _screen_index: Option<u32>) {}
        fn clear_text_overlay(&self) {}
        fn begin_eof_fade_out(&self, _voice_id: VoiceId, _fade_ms: u32) -> bool {
            false
        }
        fn devamp_voice(&self, _voice_id: VoiceId, _stop_at_end: bool) {}
        fn start_preloaded(&self, _voice_id: VoiceId) -> bool {
            false
        }
    }

    fn set_state(cue: &mut dyn Cue, state: CueState) {
        cue.restore_runtime_state(RuntimeState {
            state,
            voice_id: None,
            started_at: None,
            action_started_at: None,
        });
    }

    fn context() -> CueContext {
        let (events, _receiver) = unbounded::<CueEvent>();
        CueContext::new(
            AudioEngine::new_silent(&MachineAudioConfig::default()),
            Arc::new(NullOutput),
            events,
            0,
            Vec::new(),
            None,
            None,
            Vec::new(),
            Arc::new(DmxEngine::new()),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            256,
        )
    }

    #[test]
    fn workspace_start_dispatch_shares_cycle_guard_across_lists() {
        let mut start_a = ControlCue::new(ControlAction::Start);
        let mut start_b = ControlCue::new(ControlAction::Start);
        let a_id = start_a.id();
        let b_id = start_b.id();
        let mut disabled = MemoCue::new();
        let disabled_id = disabled.id();
        disabled.set_disabled(true);
        start_a.target_cue_ids = vec![b_id, b_id, disabled_id, uuid::Uuid::new_v4()];
        start_b.target_cue_ids = vec![a_id];

        let mut list_a = CueList::new("A");
        let mut list_b = CueList::new("B");
        list_a.push(Box::new(start_a));
        list_b.push(Box::new(start_b));
        list_b.push(Box::new(disabled));
        let mut lists = vec![list_a, list_b];
        let mut transport = Transport::new(context());

        let result = transport.go_in_workspace(&mut lists, 0).unwrap();

        assert!(result.fired.contains(&a_id));
        assert!(result.fired.contains(&b_id));
        assert!(!result.fired.contains(&disabled_id));
        assert_eq!(lists[0].get(&a_id).unwrap().state(), CueState::Completed);
        assert_eq!(lists[1].get(&b_id).unwrap().state(), CueState::Completed);
        assert_eq!(lists[1].get(&disabled_id).unwrap().state(), CueState::Standby);
    }

    #[test]
    fn immediate_remote_autocontinue_and_autofollow_cycles_keep_dispatch_guard() {
        for mode in [ContinueMode::AutoContinue, ContinueMode::AutoFollow] {
            let mut start_a = ControlCue::new(ControlAction::Start);
            let start_a_id = start_a.id();
            let mut continuation = MemoCue::new();
            continuation.set_continue_mode(mode);
            continuation.set_post_wait(Duration::ZERO);
            let continuation_id = continuation.id();
            let mut start_b = ControlCue::new(ControlAction::Start);
            let start_b_id = start_b.id();
            start_b.target_cue_ids = vec![start_a_id];
            start_a.target_cue_ids = vec![continuation_id];

            let mut list_a = CueList::new("A");
            list_a.push(Box::new(start_a));
            let mut list_b = CueList::new("B");
            list_b.push(Box::new(continuation));
            list_b.push(Box::new(start_b));
            let mut lists = vec![list_a, list_b];
            let mut transport = Transport::new(context());

            let result = transport.go_in_workspace(&mut lists, 0).unwrap();

            assert!(result.fired.contains(&continuation_id));
            assert!(result.fired.contains(&start_b_id));
            assert!(result.fired.contains(&start_a_id));
            assert!(result.fired.len() < 8, "immediate {mode:?} cross-list cycle must terminate");
        }
    }

    #[test]
    fn workspace_cart_go_keeps_text_stop_on_next_go_behavior() {
        let mut text = TextCue::new();
        let text_id = text.id();
        set_state(&mut text, CueState::Running);
        let target = MemoCue::new();
        let target_id = target.id();
        let mut list = CueList::new("Cart GO");
        list.push(Box::new(text));
        list.push(Box::new(target));
        let mut lists = vec![list];
        let mut transport = Transport::new(context());

        transport.go_by_id_in_workspace(&mut lists, &target_id).unwrap();

        assert_eq!(lists[0].get(&text_id).unwrap().state(), CueState::Standby);
        assert_eq!(lists[0].playhead_cue_id, None);
    }

    #[test]
    fn workspace_cart_go_applies_number_selected_stops_across_lists() {
        let target = MemoCue::new();
        let target_id = target.id();
        let mut target_list = CueList::new("Target");
        target_list.push(Box::new(target));
        set_state(target_list.get_mut(&target_id).unwrap(), CueState::Running);

        let number = GroupCue::new_number();
        let mut number_json = number.serialize();
        number_json["number_start_stop_mode"] = serde_json::json!("selected");
        number_json["number_start_stop_ids"] = serde_json::json!([target_id.to_string()]);
        let mut number = GroupCue::from_json_with_registry(&number_json, &crate::cue::registry::CueRegistry::new()).unwrap();
        let master = GroupCue::new();
        let master_id = master.id();
        number.add_child(Box::new(master), -1).unwrap();
        number.set_number_master(master_id).unwrap();
        let number_id = number.id();
        let mut number_list = CueList::new("Number");
        number_list.push(number);
        let mut lists = vec![number_list, target_list];
        let mut transport = Transport::new(context());

        transport.go_by_id_in_workspace(&mut lists, &number_id).unwrap();

        assert_eq!(lists[1].get(&target_id).unwrap().state(), CueState::Standby);
    }

    #[test]
    fn fade_stop_at_end_hard_stops_remote_group_descendants() {
        let mut child = AudioCue::new();
        let child_id = child.id();
        set_state(&mut child, CueState::Running);
        assert_eq!(child.state(), CueState::Running);
        let mut group = GroupCue::new();
        group.children.push(Box::new(child));
        let group_id = group.id();
        let mut lists = vec![CueList::new("Fade"), CueList::new("Remote target")];
        lists[1].push(Box::new(group));
        let mut transport = Transport::new(context());

        let stopped = transport.hard_stop_targets_in_workspace(&mut lists, &[group_id]);

        assert_eq!(stopped, vec![child_id]);
        assert_eq!(lists[1].get_recursive(&child_id).unwrap().state(), CueState::Standby);
    }

    #[test]
    fn workspace_goto_moves_only_the_target_owner_playhead() {
        let mut goto = ControlCue::new(ControlAction::Goto);
        let target_b = MemoCue::new();
        let target_b_id = target_b.id();
        goto.target_cue_ids = vec![target_b_id];
        let mut list_a = CueList::new("A");
        list_a.push(Box::new(goto));
        let next_a = MemoCue::new();
        let next_a_id = next_a.id();
        list_a.push(Box::new(next_a));

        let mut list_b = CueList::new("B");
        let first_b = MemoCue::new();
        let first_b_id = first_b.id();
        list_b.push(Box::new(first_b));
        list_b.push(Box::new(target_b));
        let mut lists = vec![list_a, list_b];
        let mut transport = Transport::new(context());

        transport.go_in_workspace(&mut lists, 0).unwrap();

        assert_eq!(lists[0].playhead_cue_id, Some(next_a_id));
        assert_eq!(lists[1].playhead_cue_id, Some(target_b_id));
        assert_ne!(lists[1].playhead_cue_id, Some(first_b_id));
    }

    #[test]
    fn selected_pause_resume_reset_and_stop_actions_resolve_remote_targets() {
        let mut media = AudioCue::new();
        let media_id = media.id();
        set_state(&mut media, CueState::Running);
        let mut target_list = CueList::new("Target");
        target_list.push(Box::new(media));

        let mut pause = ControlCue::new(ControlAction::Pause);
        pause.target_cue_ids = vec![media_id];
        let pause_id = pause.id();
        let mut resume = ControlCue::new(ControlAction::Resume);
        resume.target_cue_ids = vec![media_id];
        let resume_id = resume.id();
        let mut reset = ControlCue::new(ControlAction::Reset);
        reset.target_cue_ids = vec![media_id];
        let reset_id = reset.id();
        let mut stop = StopCue::new();
        stop.target_cue_ids = vec![media_id];
        let stop_id = stop.id();
        let mut command_list = CueList::new("Commands");
        command_list.push(Box::new(pause));
        command_list.push(Box::new(resume));
        command_list.push(Box::new(reset));
        command_list.push(Box::new(stop));
        let mut lists = vec![command_list, target_list];
        let mut transport = Transport::new(context());

        transport.go_by_id_in_workspace(&mut lists, &pause_id).unwrap();
        assert_eq!(lists[1].get(&media_id).unwrap().state(), CueState::Paused);
        transport.go_by_id_in_workspace(&mut lists, &resume_id).unwrap();
        assert_eq!(lists[1].get(&media_id).unwrap().state(), CueState::Running);
        transport.go_by_id_in_workspace(&mut lists, &reset_id).unwrap();
        assert_eq!(lists[1].get(&media_id).unwrap().state(), CueState::Standby);

        set_state(lists[1].get_mut(&media_id).unwrap(), CueState::Running);
        transport.go_by_id_in_workspace(&mut lists, &stop_id).unwrap();
        assert_eq!(lists[1].get(&media_id).unwrap().state(), CueState::Standby);
    }

    #[test]
    fn delayed_number_start_binds_autofollow_to_target_owner_list() {
        let number = GroupCue::new_number();
        let number_id = number.id();
        let mut list_a = CueList::new("Number");
        list_a.push(Box::new(number));

        let mut target = MemoCue::new();
        target.set_continue_mode(crate::cue::types::ContinueMode::AutoFollow);
        target.set_post_wait(Duration::from_secs(1));
        let target_id = target.id();
        let successor = MemoCue::new();
        let successor_id = successor.id();
        let mut list_b = CueList::new("Targets");
        list_b.push(Box::new(target));
        list_b.push(Box::new(successor));
        let mut lists = vec![list_a, list_b];
        let mut transport = Transport::new(context());

        let result = transport.start_number_targets_in_workspace(&mut lists, 0, number_id, &[target_id]);
        assert!(result.fired.contains(&target_id));
        let plan = lists[1].continuation_plan(target_id).expect("continuation belongs to target list");
        assert_eq!(plan.source_id, target_id);
        assert_eq!(plan.source_generation, lists[1].get(&target_id).unwrap().play_generation());
        assert!(plan.target_ids.contains(&successor_id));
    }

    #[test]
    fn stopping_number_reports_nested_media_resets_and_keeps_unrelated_cue() {
        let ctx = context();
        let mut number = GroupCue::new_number();
        let number_id = number.id();
        number.set_pre_wait(Duration::from_secs(3_600));
        number.go(&ctx).unwrap();
        let mut nested = GroupCue::new();
        let nested_id = nested.id();
        nested.set_pre_wait(Duration::from_secs(3_600));
        nested.go(&ctx).unwrap();
        let mut audio = AudioCue::new();
        let audio_id = audio.id();
        set_state(&mut audio, CueState::Running);
        let mut video = VideoCue::new();
        let video_id = video.id();
        set_state(&mut video, CueState::Paused);
        nested.children.push(Box::new(audio));
        nested.children.push(Box::new(video));
        number.children.push(Box::new(nested));

        let mut unrelated = AudioCue::new();
        let unrelated_id = unrelated.id();
        set_state(&mut unrelated, CueState::Running);
        let mut list = CueList::new("Nested stop");
        list.push(Box::new(number));
        list.push(Box::new(unrelated));

        let before = cue_list_states(&list);
        let mut transport = Transport::new(context());
        transport.stop_cue(&mut list, &number_id).unwrap();
        let after = cue_list_states(&list);
        let events = stop_notification_payloads(&before, &after);
        let changes: std::collections::HashMap<_, _> = events
            .iter()
            .filter(|(event, _)| *event == "cue-state-changed")
            .map(|(_, payload)| (payload["cue_id"].as_str().unwrap().to_owned(), payload.clone()))
            .collect();

        for (id, old_state) in [
            (number_id, "running"),
            (nested_id, "running"),
            (audio_id, "running"),
            (video_id, "paused"),
        ] {
            let payload = changes
                .get(&id.to_string())
                .expect("expected child state event");
            assert_eq!(payload["old_state"], old_state);
            assert_eq!(payload["new_state"], "standby");
        }
        assert!(!changes.contains_key(&unrelated_id.to_string()));
        assert_eq!(list.get(&unrelated_id).unwrap().state(), CueState::Running);
        assert_eq!(events.last().unwrap().0, "cue-list-refresh");
    }

    #[test]
    fn successful_stop_always_refreshes_even_when_state_is_already_standby() {
        let id = uuid::Uuid::new_v4();
        let events = stop_notification_payloads(
            &[(id, CueState::Standby)],
            &[(id, CueState::Standby)],
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "cue-list-refresh");
    }
}

#[cfg(test)]
mod seek_recursive_tests {
    use crate::cue::group_cue::GroupCue;
    use crate::cue::memo_cue::MemoCue;
    use crate::cue::traits::Cue;
    use crate::show::cue_list::CueList;

    #[test]
    fn media_seek_target_lookup_reaches_group_child() {
        let mut list = CueList::new("Nested seek");
        let mut group = GroupCue::new();
        let child = MemoCue::new();
        let child_id = child.id();
        group.children.push(Box::new(child));
        list.push(Box::new(group));

        // seek_cue_media uses this exact recursive mutable lookup rather than
        // the top-level-only get_mut used by the action-relative seek command.
        assert!(list.get_mut_recursive(&child_id).is_some());
    }
}
