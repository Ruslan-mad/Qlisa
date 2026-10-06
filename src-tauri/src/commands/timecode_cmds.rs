//! Tauri commands for timecode configuration and per-cue TC trigger management.

use tauri::{Emitter, State};
use uuid::Uuid;

use crate::{
    engine::{
        timecode_receiver::{TimecodeReceiver, list_midi_input_ports},
        timecode_types::{CueListTcConfig, TcPosition, TcRate, TcTrigger},
    },
    machine_config::{self, TcMachineConfig},
    show::workspace::Workspace,
    state::AppState,
};

// ---------------------------------------------------------------------------
// MIDI input enumeration (for TC source selector in UI)
// ---------------------------------------------------------------------------

/// Return the names of all available MIDI **input** ports (for MTC source selection).
#[tauri::command]
pub fn list_tc_midi_input_ports() -> Vec<String> {
    list_midi_input_ports()
}

// ---------------------------------------------------------------------------
// TC machine config
// ---------------------------------------------------------------------------

/// Return the current TC machine config (enabled, source, MIDI port, …).
#[tauri::command]
pub fn get_tc_config() -> TcMachineConfig {
    machine_config::load_tc_config()
}

/// Save TC config and hot-apply (start / stop / reconfigure the receiver).
#[tauri::command]
pub fn set_tc_config(
    config: TcMachineConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    machine_config::save_tc_config(&config).map_err(|e| e.to_string())?;

    let mut slot = state.tc_receiver.lock().map_err(|e| e.to_string())?;
    if config.enabled {
        match slot.as_ref() {
            Some(r) => r.reconfigure(config.receiver_config),
            None => {
                *slot = Some(TimecodeReceiver::new(config.receiver_config));
            }
        }
    } else {
        *slot = None;
    }
    Ok(())
}

/// Current interpolated TC position (for the status widget in the UI).
/// Returns `null` when TC is not running.
#[tauri::command]
pub fn get_tc_position(state: State<'_, AppState>) -> Option<serde_json::Value> {
    let slot = state.tc_receiver.lock().ok()?;
    let receiver = slot.as_ref()?;
    let pos = receiver.current_position()?;
    Some(serde_json::json!({
        "h": pos.hours, "m": pos.minutes, "s": pos.seconds, "f": pos.frames,
        "rate": pos.rate.to_string(), "running": receiver.is_running(),
    }))
}

// ---------------------------------------------------------------------------
// Per-cue TC trigger
// ---------------------------------------------------------------------------

/// Set (or clear) the timecode trigger on a cue.
/// `position_str` is a SMPTE string `HH:MM:SS:FF` or `HH:MM:SS;FF` (DF).
/// `rate_str` is the rate tag, e.g. `"29.97df"`.
/// Pass `null` for `position_str` to clear the trigger.
#[tauri::command]
pub fn set_cue_tc_trigger(
    cue_id: String,
    position_str: Option<String>,
    rate_str: Option<String>,
    real_time: bool,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;

    let trigger: Option<TcTrigger> = if let Some(s) = position_str {
        let rate: TcRate = rate_str
            .as_deref()
            .and_then(|r| serde_json::from_value(serde_json::json!(r)).ok())
            .unwrap_or_default();
        if real_time {
            // s is milliseconds as a string
            let ms: u64 = s.parse().map_err(|_| format!("invalid ms: '{s}'"))?;
            let pos = TcPosition::from_millis(ms, rate);
            Some(TcTrigger { position: pos, real_time: true })
        } else {
            let mut pos: TcPosition = s.parse().map_err(|e: String| e)?;
            pos.rate = rate; // apply the chosen rate
            Some(TcTrigger { position: pos, real_time: false })
        }
    } else {
        None
    };

    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;

    match trigger {
        Some(t) => { cue_list.tc_triggers.insert(id, t); }
        None    => { cue_list.tc_triggers.remove(&id); }
    }

    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Return the TC trigger for a cue (if any).
#[tauri::command]
pub fn get_cue_tc_trigger(
    cue_id: String,
    state: State<'_, AppState>,
) -> Option<serde_json::Value> {
    let id: Uuid = cue_id.parse().ok()?;
    let ws = state.workspace.lock().ok()?;
    let cl = ws.active_cue_list()?;
    let t = cl.tc_triggers.get(&id)?;
    Some(serde_json::json!({
        "position": t.position.to_string(),
        "real_time": t.real_time,
        "rate": t.position.rate.to_string(),
    }))
}

// ---------------------------------------------------------------------------
// Per-CueList TC config
// ---------------------------------------------------------------------------

/// Return the TC sync config of the requested cue list, or the active list
/// when `cue_list_id` is omitted.
#[tauri::command]
pub fn get_cuelist_tc_config(
    cue_list_id: Option<String>,
    state: State<'_, AppState>,
) -> Option<CueListTcConfig> {
    let ws = state.workspace.lock().ok()?;
    let index = resolve_tc_config_list(&ws, cue_list_id.as_deref()).ok()?;
    Some(ws.cue_lists.get(index)?.tc_config.clone())
}

fn resolve_tc_config_list(workspace: &Workspace, cue_list_id: Option<&str>) -> Result<usize, String> {
    if let Some(id) = cue_list_id {
        let id = Uuid::parse_str(id).map_err(|error| error.to_string())?;
        return workspace.cue_lists.iter().position(|list| list.id == id)
            .ok_or_else(|| format!("Cue list not found: {id}"));
    }
    workspace.cue_lists.iter().position(|list| list.id == workspace.active_cue_list_id)
        .ok_or_else(|| "No active cue list".to_string())
}

fn set_tc_config_for_list(
    workspace: &mut Workspace,
    cue_list_id: Option<&str>,
    config: CueListTcConfig,
) -> Result<(), String> {
    let index = resolve_tc_config_list(workspace, cue_list_id)?;
    workspace.cue_lists[index].tc_config = config;
    workspace.mark_modified();
    Ok(())
}

/// Update the TC sync config for the requested cue list, or the active list
/// when `cue_list_id` is omitted.
#[tauri::command]
pub fn set_cuelist_tc_config(
    config: CueListTcConfig,
    cue_list_id: Option<String>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    set_tc_config_for_list(&mut ws, cue_list_id.as_deref(), config)?;
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

#[cfg(test)]
mod cue_list_tc_config_tests {
    use super::*;
    use crate::show::cue_list::CueList;

    #[test]
    fn selected_list_config_changes_without_changing_active_list() {
        let mut workspace = Workspace::new("TC config test");
        let active_id = workspace.active_cue_list_id;
        let remote = CueList::new("Remote");
        let remote_id = remote.id;
        workspace.cue_lists.push(remote);
        let config = CueListTcConfig { enabled: true, ..CueListTcConfig::default() };

        set_tc_config_for_list(&mut workspace, Some(&remote_id.to_string()), config.clone()).unwrap();

        assert_eq!(workspace.active_cue_list_id, active_id);
        assert!(!workspace.cue_lists[0].tc_config.enabled);
        assert!(workspace.cue_list_by_id(remote_id).unwrap().tc_config.enabled);
    }

    #[test]
    fn invalid_selected_list_does_not_mark_workspace_modified() {
        let mut workspace = Workspace::new("TC config test");
        let missing_id = Uuid::new_v4().to_string();
        let revision = workspace.revision;
        let config = CueListTcConfig { enabled: true, ..CueListTcConfig::default() };

        assert!(set_tc_config_for_list(&mut workspace, Some(&missing_id), config).is_err());

        assert!(!workspace.is_modified);
        assert_eq!(workspace.revision, revision);
    }
}
