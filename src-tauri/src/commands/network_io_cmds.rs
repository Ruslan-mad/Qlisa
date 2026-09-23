//! Tauri read commands for the NDI/SRT Preferences tab.
//!
//! The runtime probe does not initialise an NDI sender/receiver or bind an SRT
//! socket; it is safe to call while a show is running and is intentionally a
//! status read rather than a streaming control command.

use crate::engine::network_io::{runtime_status, NetworkIoStatus, NetworkOutputRuntimeStatus};
use crate::state::AppState;

#[tauri::command]
pub fn get_network_io_status() -> NetworkIoStatus {
    runtime_status()
}

/// Read the actual per-destination sender state without exposing endpoint
/// configuration (including SRT passphrases).
#[tauri::command]
pub fn get_network_output_statuses(state: tauri::State<'_, AppState>) -> Vec<NetworkOutputRuntimeStatus> {
    state.output_engine.network_output_statuses()
}
