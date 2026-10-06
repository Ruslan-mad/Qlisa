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

    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;

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

    let result = transport.go(cue_list).map_err(|e| e.to_string())?;

    for id in &result.fired {
        let _ = app_handle.emit("cue-fired", serde_json::json!({ "cue_id": id }));
    }

    // Emit state changes for cues stopped by a Stop Cue action.
    for id in &result.stopped {
        let _ = app_handle.emit(
            "cue-state-changed",
            serde_json::json!({
                "cue_id": id,
                "old_state": "running",
                "new_state": "standby",
            }),
        );
    }

    // Emit state changes for chained cues (skip the primary — the frontend
    // already shows it as triggered via playhead-moved + cue-list-refresh).
    for &id in result.triggered.iter().skip(1) {
        let _ = app_handle.emit(
            "cue-state-changed",
            serde_json::json!({
                "cue_id": id,
                "old_state": "standby",
                "new_state": "running",
            }),
        );
    }

    // Always emit — even when playhead_cue_id is None (cue was last in list).
    let _ = app_handle.emit("playhead-moved", serde_json::json!({
        "cue_id": cue_list.playhead_cue_id.map(|u| u.to_string())
    }));
    // Refresh the cue list so the frontend sees updated group inner-playhead state.
    let _ = app_handle.emit("cue-list-refresh", serde_json::json!({}));
    Ok(())
}

/// Trigger a specific cue by ID (used in Cart Mode).
///
/// Parks the Playhead on the given cue and fires it via the normal GO path.
/// Auto-Continue / Auto-Follow chains still work. The same loading guard as
/// `go` applies to Audio Cues whose file is still being decoded.
#[tauri::command]
pub fn go_cue(
    cue_id: String,
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id: uuid::Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let context = make_context(&state, stop_fade_ms);
    let mut transport = Transport::new(context);
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;

    if let Some(cue) = cue_list.get_recursive(&id) {
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

    let result = transport.go_by_id(cue_list, &id).map_err(|e| e.to_string())?;

    for fired_id in &result.fired {
        let _ = app_handle.emit("cue-fired", serde_json::json!({ "cue_id": fired_id }));
    }

    for stopped_id in &result.stopped {
        let _ = app_handle.emit(
            "cue-state-changed",
            serde_json::json!({
                "cue_id": stopped_id,
                "old_state": "running",
                "new_state": "standby",
            }),
        );
    }
    for &triggered_id in result.triggered.iter().skip(1) {
        let _ = app_handle.emit(
            "cue-state-changed",
            serde_json::json!({
                "cue_id": triggered_id,
                "old_state": "standby",
                "new_state": "running",
            }),
        );
    }
    let _ = app_handle.emit("playhead-moved", serde_json::json!({
        "cue_id": cue_list.playhead_cue_id.map(|u| u.to_string())
    }));
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
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let before = cue_list
        .get_recursive(&id)
        .map(cue_subtree_states)
        .ok_or_else(|| format!("Cue not found: {id:?}"))?;
    transport.stop_cue(cue_list, &id).map_err(|e| e.to_string())?;
    let after = cue_list
        .get_recursive(&id)
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
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let old_state = cue_list.get_recursive(&id).map(|cue| cue.state()).ok_or_else(|| format!("Cue not found: {id:?}"))?;
    transport.pause_cue(cue_list, &id).map_err(|e| e.to_string())?;
    let new_state = cue_list.get_recursive(&id).map(|cue| cue.state()).ok_or_else(|| format!("Cue not found: {id:?}"))?;
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
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let old_state = cue_list.get_recursive(&id).map(|cue| cue.state()).ok_or_else(|| format!("Cue not found: {id:?}"))?;
    transport.resume_cue(cue_list, &id).map_err(|e| e.to_string())?;
    let new_state = cue_list.get_recursive(&id).map(|cue| cue.state()).ok_or_else(|| format!("Cue not found: {id:?}"))?;
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
            group_cue::GroupCue,
            traits::{Cue, RuntimeState},
            types::CueState,
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
