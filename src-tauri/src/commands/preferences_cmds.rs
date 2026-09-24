//! Tauri commands for reading and writing application preferences.

use std::f32::consts::PI;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    engine::device_manager::DeviceInfo,
    preferences::{
        AppPreferences, AudioPreferences, DisplayPreferences, GeneralPreferences,
        MachineAudioConfig, OutputDestination, OutputSinkKind,
    },
    state::AppState,
};

/// Serializes destination application after the command moved to a blocking
/// worker. Native output creation and workspace commit must remain one ordered
/// operation when two Preferences applies arrive close together.
static OUTPUT_DESTINATIONS_APPLY_GATE: Mutex<()> = Mutex::new(());

fn machine_audio_config_requires_restart(
    current: &MachineAudioConfig,
    requested: &MachineAudioConfig,
) -> bool {
    current != requested
}

fn validate_preview_pair_output_patches(
    config: &MachineAudioConfig,
    patches: &[crate::engine::device_manager::OutputPatch],
) -> Result<(), String> {
    if !matches!(config.backend, crate::preferences::AudioBackend::Asio) {
        return Ok(());
    }
    let Some(pair) = config.preview_asio_pair else {
        return Ok(());
    };
    let Some(main_device) = config.device_id.as_deref() else {
        return Ok(());
    };
    let reserved = [pair as u16 * 2, pair as u16 * 2 + 1];
    if let Some(patch) = patches.iter().find(|patch| {
        patch.device_id == main_device
            && patch.channels.iter().any(|channel| reserved.contains(channel))
    }) {
        return Err(format!(
            "ASIO preview pair Out {}-{} overlaps Output Patch '{}'. Change the preview pair or patch channels.",
            pair * 2 + 1,
            pair * 2 + 2,
            patch.name
        ));
    }
    Ok(())
}

fn output_screen_requires_apply(current: Option<u32>, requested: Option<u32>) -> bool {
    current != requested
}

/// Mutate one physical destination's persisted monitor assignment. If the
/// requested monitor is occupied, swap the two physical assignments. Returns
/// all previous monitors so callers can restore native pipelines if the
/// Preferences write fails.
fn set_display_output_monitor_preference(
    preferences: &mut AppPreferences,
    output_id: &str,
    monitor: Option<u32>,
) -> Result<Vec<(String, Option<u32>)>, String> {
    let target_index = preferences
        .display
        .output_destinations
        .iter()
        .position(|destination| destination.id == output_id)
        .ok_or_else(|| format!("Output '{output_id}' is not configured"))?;
    let target = &preferences.display.output_destinations[target_index];
    if !target.enabled {
        return Err(format!("Output '{output_id}' is disabled"));
    }
    if !matches!(&target.sink_kind, OutputSinkKind::Display) {
        return Err(format!("Output '{output_id}' is not a physical display"));
    }

    let target_previous = target.monitor;
    let owner_indices: Vec<usize> = monitor
        .map(|monitor| {
            preferences
                .display
                .output_destinations
                .iter()
                .enumerate()
                .filter_map(|(index, other)| {
                    (other.id != output_id
                        && other.enabled
                        && matches!(&other.sink_kind, OutputSinkKind::Display)
                        && other.monitor == Some(monitor))
                        .then_some(index)
                })
                .collect()
        })
        .unwrap_or_default();
    if owner_indices.len() > 1 {
        let owners = owner_indices
            .iter()
            .map(|index| {
                let owner = &preferences.display.output_destinations[*index];
                format!("'{}' ({})", owner.name, owner.id)
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "Monitor {} has multiple physical output owners: {owners}; resolve this Preferences conflict first",
            monitor.expect("owner indices require a monitor") + 1,
        ));
    }
    let owner_index = owner_indices.into_iter().next();
    let owner_previous = owner_index.map(|index| {
        preferences.display.output_destinations[index].monitor
    });

    preferences.display.output_destinations[target_index].monitor = monitor;
    if let Some(index) = owner_index {
        // A monitor is a one-output resource. Moving onto an occupied monitor
        // swaps the two assignments, including a floating `None` assignment.
        preferences.display.output_destinations[index].monitor = target_previous;
    }

    let default_id = preferences.display.default_output_id.clone();
    preferences.display.output_screen = preferences
        .display
        .output_destinations
        .iter()
        .find(|destination| destination.id == default_id)
        .and_then(|destination| destination.monitor);

    let mut changes = vec![(output_id.to_owned(), target_previous)];
    if let Some(index) = owner_index {
        changes.push((
            preferences.display.output_destinations[index].id.clone(),
            owner_previous.expect("owner has a monitor assignment"),
        ));
    }
    Ok(changes)
}

/// Reject duplicate physical monitor assignments during a new Preferences
/// apply. Existing legacy files may still contain duplicates; this validator
/// is intentionally called only on explicit updates.
fn validate_unique_display_monitors(
    destinations: &[OutputDestination],
) -> Result<(), String> {
    let mut owners = std::collections::HashMap::<u32, (&str, &str)>::new();
    for destination in destinations.iter().filter(|destination| {
        destination.enabled && matches!(&destination.sink_kind, OutputSinkKind::Display)
    }) {
        let Some(monitor) = destination.monitor else {
            continue;
        };
        if let Some((owner_name, owner_id)) = owners.get(&monitor) {
            return Err(format!(
                "Monitor {} is already assigned to display output '{}' ({}); cannot assign it to '{}' ({})",
                monitor + 1,
                owner_name,
                owner_id,
                destination.name,
                destination.id,
            ));
        }
        owners.insert(monitor, (&destination.name, &destination.id));
    }
    Ok(())
}

fn monitor_assignments_for_changes(
    preferences: &AppPreferences,
    changes: &[(String, Option<u32>)],
) -> Vec<(String, Option<u32>)> {
    changes
        .iter()
        .filter_map(|(id, _)| {
            preferences
                .display
                .output_destinations
                .iter()
                .find(|destination| destination.id == *id)
                .map(|destination| (id.clone(), destination.monitor))
        })
        .collect()
}
/// Replace the workspace's legacy/runtime mirror after a successful global
/// Preferences write.  This never marks the workspace dirty: these settings
/// belong to the machine-wide preferences file, not the project document.
fn mirror_global_preferences(
    state: &AppState,
    preferences: &AppPreferences,
) -> Result<(), String> {
    let mut ws = state.workspace.lock().map_err(|error| error.to_string())?;
    ws.preferences = preferences.clone();
    ws.sync_auto_renumber();
    Ok(())
}

fn update_global_preferences<F>(
    state: &AppState,
    update: F,
) -> Result<AppPreferences, String>
where
    F: FnOnce(&mut AppPreferences),
{
    let _write_guard = crate::state::app_state::GLOBAL_PREFERENCES_WRITE_GATE
        .lock()
        .map_err(|_| "Global Preferences write gate is poisoned".to_string())?;
    let mut next = state.global_preferences_snapshot()?;
    update(&mut next);
    let saved = state.save_global_preferences(next)?;
    mirror_global_preferences(state, &saved)?;
    Ok(saved)
}

#[derive(Debug, Serialize)]
pub struct OutputMonitorSource {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
pub struct OutputMonitorFrame {
    source_id: String,
    status: &'static str,
    sequence: u64,
    width: u32,
    height: u32,
    data_url: Option<String>,
    error: Option<String>,
}

fn output_monitor_response(source_id: String, status: &'static str) -> OutputMonitorFrame {
    OutputMonitorFrame {
        source_id,
        status,
        sequence: 0,
        width: 0,
        height: 0,
        data_url: None,
        error: None,
    }
}

fn bgra_bmp_data_url(frame: &crate::engine::network_io::BgraFrame) -> Result<String, String> {
    frame.validate()?;
    let row_bytes = frame.width.checked_mul(4).ok_or("monitor frame width overflow")? as usize;
    let image_bytes = row_bytes.checked_mul(frame.height as usize).ok_or("monitor frame size overflow")?;
    let file_bytes = 54usize.checked_add(image_bytes).ok_or("monitor bitmap size overflow")?;
    let mut bitmap = Vec::with_capacity(file_bytes);
    bitmap.extend_from_slice(b"BM");
    bitmap.extend_from_slice(&(file_bytes as u32).to_le_bytes());
    bitmap.extend_from_slice(&[0; 4]);
    bitmap.extend_from_slice(&54_u32.to_le_bytes());
    bitmap.extend_from_slice(&40_u32.to_le_bytes());
    bitmap.extend_from_slice(&(frame.width as i32).to_le_bytes());
    // Negative height declares the existing top-down BGRA row order.
    bitmap.extend_from_slice(&(-(frame.height as i32)).to_le_bytes());
    bitmap.extend_from_slice(&1_u16.to_le_bytes());
    bitmap.extend_from_slice(&32_u16.to_le_bytes());
    bitmap.extend_from_slice(&0_u32.to_le_bytes());
    bitmap.extend_from_slice(&(image_bytes as u32).to_le_bytes());
    bitmap.extend_from_slice(&[0; 16]);
    for row in 0..frame.height as usize {
        let offset = row * frame.stride as usize;
        bitmap.extend_from_slice(&frame.data[offset..offset + row_bytes]);
    }
    Ok(format!(
        "data:image/bmp;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bitmap)
    ))
}

/// Return named output destinations from the machine-wide Preferences file.
#[tauri::command]
pub fn list_output_destinations(
    state: State<'_, AppState>,
) -> Result<Vec<OutputDestination>, String> {
    Ok(state
        .global_preferences_snapshot()?
        .display
        .output_destinations)
}

/// Return volatile per-output operator state. This is separate from
/// Preferences so FTB is never written to disk.
#[tauri::command]
pub fn get_output_control_statuses(
    state: State<'_, AppState>,
) -> Vec<crate::engine::output_engine::OutputControlStatus> {
    state.output_engine.output_control_statuses()
}

/// Toggle FTB for one stable output id. The output pipeline stays alive and
/// audio routing is not changed.
#[tauri::command]
pub fn toggle_output_ftb(
    output_id: String,
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<bool, String> {
    let enabled = state
        .output_engine
        .toggle_output_ftb(&output_id)
        .map_err(|error| error.to_string())?;
    let _ = app_handle.emit("output-control-status-changed", ());
    Ok(enabled)
}

#[tauri::command]
pub fn list_output_monitor_sources(
    state: State<'_, AppState>,
) -> Result<Vec<OutputMonitorSource>, String> {
    Ok(state
        .global_preferences_snapshot()?
        .display
        .output_destinations
        .iter()
        .filter(|destination| {
            destination.enabled && matches!(destination.sink_kind, OutputSinkKind::Display)
        })
        .map(|destination| OutputMonitorSource {
            id: destination.id.clone(),
            name: destination.name.clone(),
        })
        .collect())
}

#[tauri::command]
pub fn set_output_monitor_source(
    source_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .output_engine
        .set_output_monitor_source(source_id.as_deref())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_output_monitor_frame(
    source_id: String,
    after_sequence: Option<u64>,
    app_handle: AppHandle,
) -> Result<OutputMonitorFrame, String> {
    use crate::engine::output_engine::OutputMonitorFrameRead;

    let output_engine = Arc::clone(&app_handle.state::<AppState>().output_engine);
    let read = match output_engine
        .output_monitor_frame(&source_id, after_sequence.unwrap_or(0))
    {
        Ok(read) => read,
        Err(error) => {
            let message = error.to_string();
            let status = if message.contains("unavailable") || message.contains("not active") {
                "unavailable"
            } else {
                "error"
            };
            let mut response = output_monitor_response(source_id, status);
            response.error = Some(message);
            return Ok(response);
        }
    };
    Ok(match read {
        OutputMonitorFrameRead::NoFrame => output_monitor_response(source_id, "no_frame"),
        OutputMonitorFrameRead::Unchanged { sequence } => OutputMonitorFrame {
            sequence,
            ..output_monitor_response(source_id, "unchanged")
        },
        OutputMonitorFrameRead::Frame { sequence, frame } => {
            let failure_source_id = source_id.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                let black = frame
                    .data
                    .chunks_exact(4)
                    .all(|pixel| pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0);
                if black {
                    return OutputMonitorFrame {
                        sequence,
                        width: frame.width,
                        height: frame.height,
                        ..output_monitor_response(source_id, "black")
                    };
                }
                match bgra_bmp_data_url(&frame) {
                    Ok(data_url) => OutputMonitorFrame {
                        sequence,
                        width: frame.width,
                        height: frame.height,
                        data_url: Some(data_url),
                        ..output_monitor_response(source_id, "frame")
                    },
                    Err(error) => {
                        let mut response = output_monitor_response(source_id, "error");
                        response.sequence = sequence;
                        response.error = Some(error);
                        response
                    }
                }
            }).await {
                Ok(response) => response,
                Err(error) => {
                    let mut response = output_monitor_response(failure_source_id, "error");
                    response.error = Some(format!("Output monitor encoder task failed: {error}"));
                    response
                }
            }
        }
    })
}

/// Replace output destinations while preserving the legacy fields for older
/// Qlisa/Inkue builds. At least one destination is always retained.
#[tauri::command]
pub async fn update_output_destinations(
    destinations: Vec<OutputDestination>,
    default_output_id: String,
    app_handle: AppHandle,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        apply_output_destinations(destinations, default_output_id, state.inner(), &app_handle)
    })
    .await
    .map_err(|error| format!("Could not apply output destinations: {error}"))?
}

/// Validate and apply output destinations off the WebView/Tauri callback thread.
/// Native output creation can synchronously wait for a winit/GL pipeline, so
/// this helper must only run from the blocking worker used by the command above.
fn apply_output_destinations(
    mut destinations: Vec<OutputDestination>,
    default_output_id: String,
    state: &AppState,
    app_handle: &AppHandle,
) -> Result<(), String> {
    let _apply_guard = OUTPUT_DESTINATIONS_APPLY_GATE
        .lock()
        .map_err(|_| "Output destination apply gate is poisoned".to_string())?;
    let _global_write_guard = crate::state::app_state::GLOBAL_PREFERENCES_WRITE_GATE
        .lock()
        .map_err(|_| "Global Preferences write gate is poisoned".to_string())?;

    if destinations.is_empty() {
        return Err("At least one output is required".into());
    }
    // Do not silently substitute the first destination.  The selected id is
    // persisted in the global Preferences file and must be an actual configured
    // output; otherwise a stale UI value could mutate the global mirror
    // before the native output engine rejects the configuration.
    if default_output_id.trim().is_empty()
        || !destinations.iter().any(|o| o.id == default_output_id)
    {
        return Err("Default output destination is not configured".into());
    }
    let selected_id = default_output_id;
    if destinations
        .iter()
        .any(|o| o.id.trim().is_empty() || o.name.trim().is_empty())
    {
        return Err("Output ids and names must be non-empty".into());
    }
    let mut seen = std::collections::HashSet::new();
    if destinations.iter().any(|o| !seen.insert(o.id.as_str())) {
        return Err("Output ids must be unique".into());
    }
    // Network destinations deliberately have no monitor/window.  Keep their
    // nested enabled bit in sync with the top-level routing bit so old files
    // and future sender workers observe one unambiguous operator intent.
    for destination in &mut destinations {
        match destination.sink_kind {
            OutputSinkKind::Display => {}
            OutputSinkKind::Ndi => {
                destination.monitor = None;
                destination.floating_window = None;
                destination.network.ndi.enabled = destination.enabled;
                if destination.enabled && destination.network.ndi.stream_name.trim().is_empty() {
                    return Err("NDI stream name is required for an enabled output".into());
                }
            }
            OutputSinkKind::Srt => {
                destination.monitor = None;
                destination.floating_window = None;
                destination.network.srt.enabled = destination.enabled;
                if destination.enabled {
                    destination.network.srt.validate()?;
                }
            }
        }
    }
    validate_unique_display_monitors(&destinations)?;
    // The native floating window records its rect as the operator moves it.
    // Preferences may have been opened before that drag and therefore hold a
    // stale draft; applying an unrelated setting must never snap the output
    // back to an older position.  Geometry is not an editable Preferences
    // field, so the live runtime mirror wins for an unchanged floating destination.
    let previous_preferences = state.global_preferences_snapshot()?;
    if let Ok(ws) = state.workspace.lock() {
        for destination in &mut destinations {
            if destination.monitor.is_none() {
                if let Some(previous) = ws
                    .preferences
                    .display
                    .output_destinations
                    .iter()
                    .find(|previous| previous.id == destination.id && previous.monitor.is_none())
                {
                    destination.floating_window = previous.floating_window.clone();
                }
            }
        }
    }
    // Reconcile native state first.  This keeps workspace data unchanged if
    // creating a new live output fails (missing libmpv, invalid monitor, etc.).
    state
        .output_engine
        .sync_output_destinations(&destinations, &selected_id)
        .map_err(|e| e.to_string())?;

    let (selected_monitor, selected_transform) = destinations
        .iter()
        .find(|o| o.id == selected_id)
        .map(|o| (o.monitor, o.transform))
        .expect("selected output exists");
    let mut next_preferences = previous_preferences.clone();
    next_preferences.display.output_destinations = destinations;
    next_preferences.display.default_output_id = selected_id.clone();
    // Keep old clients and old engines pointed at the default output.
    next_preferences.display.output_screen = selected_monitor;
    next_preferences.display.output_transform = selected_transform;
    let saved_preferences = match state.save_global_preferences(next_preferences) {
        // Native state changed first so an engine failure cannot dirty the
        // document. If persistence itself fails, restore the prior graph too.
        Ok(saved) => saved,
        Err(error) => {
            let previous_id = previous_preferences.display.default_output_id.clone();
            let _ = state.output_engine.sync_output_destinations(
                &previous_preferences.display.output_destinations,
                &previous_id,
            );
            return Err(error);
        }
    };
    mirror_global_preferences(state, &saved_preferences)?;
    let _ = app_handle.emit("preferences-updated", serde_json::json!({}));
    Ok(())
}

/// Return the number of stereo output pairs the current ASIO engine stream
/// is using.  Call this after Apply to populate the pair selector.
///
/// Reads the channel count stored by the last successful `restart()`.
/// Returns 1 if the engine has not yet been switched to ASIO.
#[tauri::command]
pub fn get_asio_output_pairs(state: State<'_, AppState>) -> u32 {
    let ch = state.audio_engine.output_channels();
    (ch / 2).max(1)
}

/// Return the list of audio backends available on this platform.
///
/// Windows: `wasapi_shared`, `wasapi_exclusive`, and `asio` (when installed).
/// Mac / Linux: `system_default` — cpal picks CoreAudio / ALSA automatically.
#[tauri::command]
pub fn get_available_backends() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        #[allow(unused_mut)]
        let mut backends = vec!["wasapi_shared".to_string(), "wasapi_exclusive".to_string()];
        #[cfg(feature = "asio-support")]
        if asio_drivers_installed() {
            backends.push("asio".to_string());
        }
        backends
    }
    #[cfg(not(target_os = "windows"))]
    vec!["system_default".to_string()]
}

/// Returns `true` when at least one ASIO driver is registered under
/// `HKEY_LOCAL_MACHINE\SOFTWARE\ASIO` (checked only when the
/// `asio-support` feature is enabled).
#[cfg(all(windows, feature = "asio-support"))]
fn asio_drivers_installed() -> bool {
    use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    hklm.open_subkey("SOFTWARE\\ASIO")
        .map(|k| k.enum_keys().count() > 0)
        .unwrap_or(false)
}

/// Enumerate installed ASIO drivers by reading the Windows registry directly.
///
/// Returns one `DeviceInfo` per subkey under `HKLM\SOFTWARE\ASIO`.
/// Channels and sample rate are left at defaults — ASIO drivers report their
/// actual capabilities only after they are opened.
#[cfg(all(windows, feature = "asio-support"))]
pub(crate) fn list_asio_drivers_from_registry() -> Vec<crate::engine::device_manager::DeviceInfo> {
    use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let Ok(key) = hklm.open_subkey("SOFTWARE\\ASIO") else {
        return vec![];
    };
    key.enum_keys()
        .filter_map(|k| k.ok())
        .map(|name| crate::engine::device_manager::DeviceInfo {
            id: name.clone(),
            name,
            channels: 2,
            sample_rate: 44100,
        })
        .collect()
}

/// Return the authoritative machine-wide Preferences tree.
#[tauri::command]
pub fn get_preferences(state: State<'_, AppState>) -> Result<AppPreferences, String> {
    state.global_preferences_snapshot()
}

/// Return the machine audio config from disk, normalised for the current build.
///
/// - Mac / Linux: any Windows-specific backend (`wasapi_*`, `asio`) → `system_default`.
/// - Windows without `--features asio-support`: `asio` → `wasapi_shared` so the
///   preferences UI shows a usable backend.  The file on disk is NOT rewritten, so
///   switching to `pnpm tauri:dev` (with ASIO) restores the real choice automatically.
#[tauri::command]
pub fn get_machine_audio_config() -> MachineAudioConfig {
    // `machine_config::load` already coerces a backend this OS cannot offer
    // (see `AudioBackend::for_this_platform`), so the panel always shows a
    // value that is in `get_available_backends`.
    crate::machine_config::load()
}

#[derive(Debug, Serialize)]
pub struct AudioRuntimeStatus {
    pub main_state: &'static str,
    pub main_device_name: Option<String>,
    pub preview_state: &'static str,
    pub preview_device_name: Option<String>,
}

fn audio_main_runtime_state(failed: bool, in_fallback: bool) -> &'static str {
    if failed {
        "error"
    } else if in_fallback {
        "fallback"
    } else {
        "working"
    }
}

fn audio_preview_runtime_state(
    backend: &crate::preferences::AudioBackend,
    preview_asio_pair: Option<u32>,
    preview_device_id: Option<&str>,
    main_state: &'static str,
) -> &'static str {
    if matches!(backend, crate::preferences::AudioBackend::Asio) && preview_asio_pair.is_some() {
        if main_state == "working" {
            "working"
        } else {
            "error"
        }
    } else if preview_device_id.is_some() {
        "not_tested"
    } else {
        "not_configured"
    }
}

/// Return runtime audio state. The preview aux stream is lazy, so it is marked
/// `not_tested` until an actual preview test or cue opens it.
#[tauri::command]
pub async fn get_audio_runtime_status(
    state: State<'_, AppState>,
) -> Result<AudioRuntimeStatus, String> {
    let engine = Arc::clone(&state.audio_engine);
    let (config, health) = tauri::async_runtime::spawn_blocking(move || {
        (crate::machine_config::load(), engine.audio_health())
    })
    .await
    .map_err(|error| error.to_string())?;
    let main_state = audio_main_runtime_state(health.failed, health.in_fallback);
    let preview_state = audio_preview_runtime_state(
        &config.backend,
        config.preview_asio_pair,
        config.preview_device_id.as_deref(),
        main_state,
    );
    Ok(AudioRuntimeStatus {
        main_state,
        main_device_name: health.desired_device,
        preview_state,
        preview_device_name: config.preview_device_name,
    })
}

/// Persist machine audio config to `%APPDATA%\Inkue\audio.json` and re-open the
/// main audio engine on the new device. Running cue voices keep playing across
/// that restart; the operator-only preview is stopped and its aux stream is
/// reopened lazily on the next preview request.
#[tauri::command]
pub fn update_machine_audio_config(
    config: MachineAudioConfig,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // Apply is also used as a general Preferences save.  Re-opening an
    // identical audio stream is destructive while a cue is live: it creates a
    // short callback gap and races output reconciliation, which can make a
    // network sender lose its live program.  The persisted machine config is
    // the operator's requested configuration, so an identical request is a
    // strict no-op (including preview teardown and all engine work).
    // Serialize hardware transitions with output-destination transitions.  A
    // changed audio sample rate may legitimately require network workers to be
    // rebuilt, but it must never race the output graph's configuration probe.
    let _apply_guard = OUTPUT_DESTINATIONS_APPLY_GATE
        .lock()
        .map_err(|_| "Output destination apply gate is poisoned".to_string())?;
    let current = crate::machine_config::load();
    let workspace_patches = state
        .workspace
        .lock()
        .map_err(|_| "Workspace is unavailable while validating ASIO preview routing".to_string())?
        .output_patches
        .clone();
    validate_preview_pair_output_patches(&config, &workspace_patches)?;
    if matches!(config.backend, crate::preferences::AudioBackend::Asio) {
        if let Some(pair) = config.preview_asio_pair {
            if pair == config.asio_out_pair {
                return Err(format!(
                    "ASIO preview pair Out {}-{} conflicts with the main program pair",
                    pair * 2 + 1,
                    pair * 2 + 2
                ));
            }
            // Validate against the live stream when the requested device is
            // already open. A simultaneous device change is validated again
            // by play_preview_voice after the restart.
            let same_stream = matches!(current.backend, crate::preferences::AudioBackend::Asio)
                && current.device_id == config.device_id;
            if same_stream {
                let pairs = (state.audio_engine.output_channels() / 2).max(1);
                if pair >= pairs {
                    return Err(format!(
                        "ASIO preview pair Out {}-{} is unavailable; the current stream has only {} stereo pairs",
                        pair * 2 + 1,
                        pair * 2 + 2,
                        pairs
                    ));
                }
            }
        }
    }
    if !machine_audio_config_requires_restart(&current, &config) {
        return Ok(());
    }

    // An aux stream belongs to the previous device universe. Stop (and remove)
    // the operator-only preview before restart closes aux streams so no stale
    // session can be toggled after Settings changes.
    if let Ok(mut preview) = state.preview_session.lock() {
        if let Some(session) = preview.take() {
            state.audio_engine.stop_preview_voice(session.voice_id);
        }
    }
    crate::machine_config::save(&config).map_err(|e| e.to_string())?;

    // Record this as the operator's desired device (clears any auto-fallback +
    // its banner) and re-open the stream on it.
    state
        .audio_engine
        .apply_user_config(&config)
        .map_err(|e| e.to_string())?;
    crate::health::clear("audio-device");
    crate::health::clear("preview-output-reserved");

    let new_buffer_size = config.buffer_size;
    // Keep only the runtime buffer-size hint in both preference mirrors.
    // Running cues are NOT reset: voices are preserved across the device switch.
    let _write_guard = crate::state::app_state::GLOBAL_PREFERENCES_WRITE_GATE
        .lock()
        .map_err(|_| "Global Preferences write gate is poisoned".to_string())?;
    let mut global = state.global_preferences_snapshot()?;
    global.audio.audio_buffer_size = new_buffer_size;
    {
        let mut current = state.global_preferences.lock().map_err(|e| e.to_string())?;
        *current = global.clone();
    }
    mirror_global_preferences(&state, &global)?;

    // The output-device universe changed (backend and/or device): tell the
    // Output Patches panel to refetch its device list immediately, so patches
    // pointing into the old universe show their warning without a reopen.
    let _ = app_handle.emit("device-changed", serde_json::json!({}));
    let _ = app_handle.emit("preferences-updated", serde_json::json!({}));
    Ok(())
}

/// Overwrite the global audio defaults (volume, fade).
/// Does not restart the engine — use `update_machine_audio_config` for hardware changes.
#[tauri::command]
pub fn update_audio_preferences(
    prefs: AudioPreferences,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    update_global_preferences(&state, |next| next.audio = prefs)?;
    let _ = app_handle.emit("preferences-updated", serde_json::json!({}));
    Ok(())
}

/// Overwrite the global general section of Preferences.
///
/// Unlike audio preferences, no engine restart is needed.
#[tauri::command]
pub fn update_general_preferences(
    prefs: GeneralPreferences,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    update_global_preferences(&state, |next| next.general = prefs)?;
    let _ = app_handle.emit("preferences-updated", serde_json::json!({}));
    Ok(())
}

/// Return the current global output screen index.
///
/// `None` means floating windowed; `Some(n)` means fullscreen on monitor n.
#[tauri::command]
pub fn get_output_screen(state: State<'_, AppState>) -> Result<Option<u32>, String> {
    Ok(state.global_preferences_snapshot()?.display.output_screen)
}

/// Assign one enabled physical output to a connected monitor immediately.
///
/// This command intentionally updates one native display pipeline in place.
/// It does not run the full destination graph apply, so NDI/SRT workers and
/// their frame sinks are not recreated or otherwise touched.
#[tauri::command]
pub fn set_display_output_monitor(
    output_id: String,
    monitor: u32,
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    let _apply_guard = OUTPUT_DESTINATIONS_APPLY_GATE
        .lock()
        .map_err(|_| "Output destination apply gate is poisoned".to_string())?;
    let _write_guard = crate::state::app_state::GLOBAL_PREFERENCES_WRITE_GATE
        .lock()
        .map_err(|_| "Global Preferences write gate is poisoned".to_string())?;

    let previous_preferences = state.global_preferences_snapshot()?;
    let mut next_preferences = previous_preferences.clone();
    let previous_assignments =
        set_display_output_monitor_preference(&mut next_preferences, &output_id, Some(monitor))?;

    state
        .output_engine
        .set_display_output_monitors(&monitor_assignments_for_changes(
            &next_preferences,
            &previous_assignments,
        ))
        .map_err(|error| error.to_string())?;

    let saved_preferences = match state.save_global_preferences(next_preferences) {
        Ok(saved) => saved,
        Err(error) => {
            let restore_error = state
                .output_engine
                .restore_display_output_monitors(&previous_assignments)
                .err()
                .map(|restore| restore.to_string());
            return Err(match restore_error {
                Some(restore) => format!("{error}; runtime rollback failed: {restore}"),
                None => error,
            });
        }
    };

    if let Err(error) = mirror_global_preferences(state.inner(), &saved_preferences) {
        // Saving succeeded but updating the runtime mirror did not. Restore
        // both the native placement and the machine-wide Preferences tree so
        // the command remains all-or-nothing from the operator's perspective.
        let mut rollback_errors = Vec::new();
        if let Err(restore) = state
            .output_engine
            .restore_display_output_monitors(&previous_assignments)
        {
            rollback_errors.push(format!("runtime: {restore}"));
        }
        if let Err(restore) = state.save_global_preferences(previous_preferences) {
            rollback_errors.push(format!("Preferences: {restore}"));
        }
        return Err(if rollback_errors.is_empty() {
            error
        } else {
            format!("{error}; rollback failed: {}", rollback_errors.join("; "))
        });
    }

    let _ = app_handle.emit("preferences-updated", serde_json::json!({}));
    let _ = app_handle.emit("output-window-visible", state.output_engine.is_visible());
    let _ = app_handle.emit("output-control-status-changed", ());
    Ok(())
}

/// Set the global output screen index.
///
/// Pass `None` for floating windowed, `Some(n)` for fullscreen on monitor n.
/// The window is repositioned immediately (fullscreen on the selected screen,
/// or restored to the floating rect) — not just on the next visual GO.
#[tauri::command]
pub fn set_output_screen(
    screen: Option<u32>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let _apply_guard = OUTPUT_DESTINATIONS_APPLY_GATE
        .lock()
        .map_err(|_| "Output destination apply gate is poisoned".to_string())?;
    let _write_guard = crate::state::app_state::GLOBAL_PREFERENCES_WRITE_GATE
        .lock()
        .map_err(|_| "Global Preferences write gate is poisoned".to_string())?;

    let previous_preferences = state.global_preferences_snapshot()?;
    let default_id = previous_preferences.display.default_output_id.clone();
    let current_default_monitor = previous_preferences
        .display
        .output_destinations
        .iter()
        .find(|destination| destination.id == default_id)
        .and_then(|destination| destination.monitor);
    if !output_screen_requires_apply(previous_preferences.display.output_screen, screen)
        && current_default_monitor == screen
    {
        return Ok(());
    }

    let mut next_preferences = previous_preferences.clone();
    let previous_assignments =
        set_display_output_monitor_preference(&mut next_preferences, &default_id, screen)?;
    let assignments = monitor_assignments_for_changes(&next_preferences, &previous_assignments);
    state
        .output_engine
        .set_display_output_monitors(&assignments)
        .map_err(|error| error.to_string())?;

    let saved_preferences = match state.save_global_preferences(next_preferences) {
        Ok(saved) => saved,
        Err(error) => {
            let restore_error = state
                .output_engine
                .restore_display_output_monitors(&previous_assignments)
                .err()
                .map(|restore| restore.to_string());
            return Err(match restore_error {
                Some(restore) => format!("{error}; runtime rollback failed: {restore}"),
                None => error,
            });
        }
    };
    if let Err(error) = mirror_global_preferences(state.inner(), &saved_preferences) {
        let mut rollback_errors = Vec::new();
        if let Err(restore) = state
            .output_engine
            .restore_display_output_monitors(&previous_assignments)
        {
            rollback_errors.push(format!("runtime: {restore}"));
        }
        if let Err(restore) = state.save_global_preferences(previous_preferences) {
            rollback_errors.push(format!("Preferences: {restore}"));
        }
        return Err(if rollback_errors.is_empty() {
            error
        } else {
            format!("{error}; rollback failed: {}", rollback_errors.join("; "))
        });
    }

    let _ = app_handle.emit("preferences-updated", serde_json::json!({}));
    let _ = app_handle.emit("output-window-visible", state.output_engine.is_visible());
    let _ = app_handle.emit("output-control-status-changed", ());
    Ok(())
}

/// Return the global projector-alignment transform.
#[tauri::command]
pub fn get_output_transform(
    state: State<'_, AppState>,
) -> Result<crate::engine::output_engine::OutputTransform, String> {
    Ok(state.global_preferences_snapshot()?.display.output_transform)
}

/// Set the global projector-alignment transform.
///
/// Persisted in global display preferences and applied to the output window
/// immediately (recomposed with the current cue's geometry), so the operator
/// sees the change live while adjusting values.
#[tauri::command]
pub fn set_output_transform(
    transform: crate::engine::output_engine::OutputTransform,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    update_global_preferences(&state, |next| next.display.output_transform = transform)?;
    state.output_engine.set_output_transform(transform);
    let _ = app_handle.emit("preferences-updated", serde_json::json!({}));
    Ok(())
}

/// Show a calibration pattern (grid, colour bars, custom image, …) fullscreen
/// on the configured output screen.  Replaces whatever is playing.
#[tauri::command]
pub fn show_test_pattern(
    pattern: crate::engine::output_engine::TestPattern,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let screen = state.global_preferences_snapshot()?.display.output_screen;
    state.output_engine.show_test_pattern(&pattern, screen);
    Ok(())
}

/// Clear the test pattern: the output returns to opaque black.
#[tauri::command]
pub fn clear_test_pattern(state: State<'_, AppState>) -> Result<(), String> {
    state.output_engine.clear_test_pattern();
    Ok(())
}

/// Overwrite the global colour-theme and timer fields in display Preferences.
///
/// The frontend applies colour values as CSS variables; timer style is applied
/// immediately to the mpv OSD — no engine restart needed.
#[tauri::command]
pub fn update_display_preferences(
    prefs: DisplayPreferences,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let saved = update_global_preferences(&state, |next| {
        // Preserve fields managed by their dedicated commands and the output
        // destination apply gate.
        next.display.theme = prefs.theme;
        next.display.show_output_timer = prefs.show_output_timer;
        next.display.timer_floating = prefs.timer_floating;
        next.display.timer_count_down = prefs.timer_count_down;
        next.display.timer_show_ms = prefs.timer_show_ms;
        next.display.timer_font = prefs.timer_font;
        next.display.timer_font_size = prefs.timer_font_size;
        next.display.timer_position = prefs.timer_position;
        next.display.timer_margin = prefs.timer_margin;
        next.display.cue_color_style = prefs.cue_color_style;
        next.display.show_live_panel = prefs.show_live_panel;
        next.display.show_slice_panel = prefs.show_slice_panel;
        next.display.clip_editor_active_tab = prefs.clip_editor_active_tab;
    })?;
    let font = saved.display.timer_font.clone();
    let font_size = saved.display.timer_font_size;
    let position = saved.display.timer_position;
    let margin = saved.display.timer_margin;
    let show_floating = saved.display.show_output_timer && saved.display.timer_floating;

    state
        .output_engine
        .set_timer_style(&font, font_size, position, margin);
    state
        .output_engine
        .set_floating_timer_visible(show_floating);
    // Clear any active preview — live cue timer takes over from here.
    state.output_engine.set_timer_preview(None);
    let _ = app_handle.emit("preferences-updated", serde_json::json!({}));
    Ok(())
}

/// Apply timer style settings immediately (without persisting) and show or hide
/// a preview placeholder on the output window.
///
/// Used by the preferences panel while the user adjusts timer settings.
/// `text = Some("00:00.000")` → show placeholder; `text = None` → clear preview.
/// Restoring the persisted style after cancel is the caller's responsibility.
#[tauri::command]
pub fn preview_output_timer(
    font: String,
    font_size: u32,
    position: crate::preferences::TimerPosition,
    margin: u32,
    text: Option<String>,
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    state
        .output_engine
        .set_timer_style(&font, font_size, position, margin);
    state.output_engine.set_timer_preview(text);
    Ok(())
}

/// Enumerate all font family names installed on the system.
///
/// On Windows: uses GDI `EnumFontFamiliesExW` so names match exactly what
/// mpv's `osd-font` property accepts. Vertical-text (`@`-prefixed) families
/// are excluded.
/// On macOS / Linux: shells out to fontconfig's `fc-list`, which both mpv
/// (libass) and WebKit/WebView resolve font names through — so the names
/// returned are guaranteed to match what `osd-font` and the floating timer's
/// CSS `font-family` actually render. Returns an empty list if `fc-list`
/// isn't on PATH (the font field stays free-text in that case).
#[tauri::command]
pub fn list_system_fonts() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Graphics::Gdi::{
            CreateCompatibleDC, DeleteDC, EnumFontFamiliesExW, LOGFONTW, TEXTMETRICW,
        };

        unsafe extern "system" fn enum_cb(
            lpelfe: *const LOGFONTW,
            _: *const TEXTMETRICW,
            _: u32,
            lparam: isize,
        ) -> i32 {
            let list = &mut *(lparam as *mut Vec<String>);
            let face = (*lpelfe).lfFaceName;
            let len = face.iter().position(|&c| c == 0).unwrap_or(32);
            let name = String::from_utf16_lossy(&face[..len]);
            if !name.starts_with('@') {
                list.push(name);
            }
            1
        }

        let mut fonts: Vec<String> = Vec::new();
        unsafe {
            let hdc = CreateCompatibleDC(0);
            if hdc != 0 {
                let mut lf: LOGFONTW = std::mem::zeroed();
                lf.lfCharSet = 1;
                EnumFontFamiliesExW(
                    hdc,
                    &lf,
                    Some(enum_cb),
                    &mut fonts as *mut Vec<String> as isize,
                    0,
                );
                DeleteDC(hdc);
            }
        }
        fonts.sort_by_key(|a| a.to_lowercase());
        fonts.dedup();
        fonts
    }
    #[cfg(not(target_os = "windows"))]
    {
        let output = match std::process::Command::new("fc-list")
            .arg(":")
            .arg("family")
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };
        let mut fonts: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .flat_map(|line| line.split(','))
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect();
        fonts.sort_by_key(|a| a.to_lowercase());
        fonts.dedup();
        fonts
    }
}

/// Return all available audio output devices for the given backend.
///
/// Enumeration runs **off the main thread** (async command → `spawn_blocking`)
/// and is **time-bounded** (see [`ENUM_TIMEOUT`](crate::engine::device_manager::ENUM_TIMEOUT)),
/// so a slow or hung WASAPI device (cpal #867) can never leave the Preferences
/// panel stuck on "Loading…" — the historical Windows failure mode.
#[tauri::command]
pub async fn list_audio_devices(
    backend: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<DeviceInfo>, String> {
    use crate::preferences::AudioBackend;

    #[allow(unused_variables)] // used only in #[cfg(feature = "asio-support")] block
    let ab: AudioBackend = match backend.as_deref() {
        Some("wasapi_exclusive") => AudioBackend::WasapiExclusive,
        Some("asio") => AudioBackend::Asio,
        _ => AudioBackend::WasapiShared,
    };

    // For ASIO: cpal's output_devices() is unreliable (COM/thread issues).
    // Read driver names directly from the Windows registry instead — fast, no
    // cpal, so this stays on the calling thread.
    #[cfg(all(windows, feature = "asio-support"))]
    if matches!(ab, AudioBackend::Asio) {
        return Ok(list_asio_drivers_from_registry());
    }

    list_system_output_devices(state.audio_engine.clone()).await
}

/// Enumerate the generic system host outputs used by the independent preview
/// aux stream. On Windows this is WASAPI shared regardless of the main output
/// backend; in particular, ASIO driver identifiers are never preview targets.
async fn list_system_output_devices(
    engine: std::sync::Arc<crate::engine::AudioEngine>,
) -> Result<Vec<DeviceInfo>, String> {
    // WASAPI / CoreAudio / ALSA enumeration is the slow path: run it on a
    // blocking-pool thread, bounded, then warm the engine's shared cache.
    let devices = tauri::async_runtime::spawn_blocking(move || {
        let devices = crate::engine::device_manager::run_bounded(
            crate::engine::device_manager::ENUM_TIMEOUT,
            crate::engine::device_manager::enumerate_output_devices,
        )
        .unwrap_or_default();
        if let Ok(mut mgr) = engine.device_manager.lock() {
            mgr.replace_cache(devices.clone());
        }
        devices
    })
    .await
    .map_err(|e| e.to_string())?;

    #[cfg(target_os = "linux")]
    return Ok(crate::engine::device_manager::linux_devices(false, devices));
    #[cfg(not(target_os = "linux"))]
    Ok(devices)
}

/// Return the generic system-host device list for headphone preview. This is
/// deliberately independent of the selected main backend: preview is an aux
/// WASAPI shared stream on Windows (or the normal system host elsewhere), not
/// an ASIO secondary output.
#[tauri::command]
pub async fn list_preview_audio_devices(
    state: State<'_, AppState>,
) -> Result<Vec<DeviceInfo>, String> {
    list_system_output_devices(state.audio_engine.clone()).await
}

/// Play a short 440 Hz sine-wave beep on the specified device and backend.
///
/// For WASAPI backends a temporary stream is opened directly on the selected
/// device — no need to Apply first.  For ASIO (exclusive, single-stream) the
/// beep is routed through the main engine instead.
#[tauri::command]
pub fn test_audio_device(
    device_id: String,
    backend: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use crate::preferences::AudioBackend;
    let ab: AudioBackend = match backend.as_str() {
        "asio" => AudioBackend::Asio,
        "wasapi_exclusive" => AudioBackend::WasapiExclusive,
        _ => AudioBackend::WasapiShared,
    };

    // ASIO uses exclusive access — can't open a second stream alongside the
    // engine, so play through the existing engine voice path.
    if matches!(ab, AudioBackend::Asio) {
        let sample_rate = state.audio_engine.sample_rate();
        return play_beep_via_engine(sample_rate, &state);
    }

    // For WASAPI: open a temporary independent stream on the selected device.
    // Everything is done inside a thread so cpal's Stream (which contains raw
    // Win32 handles) never crosses a thread boundary.
    let device_id_opt = if device_id.is_empty() {
        None
    } else {
        Some(device_id)
    };
    play_beep_on_device(device_id_opt);
    Ok(())
}

/// Test one Output Patch using its current physical device and channel pair.
/// Unlike the generic device test this never ignores the patch's channels.
#[tauri::command]
pub fn test_output_patch(
    device_id: String,
    channels: Vec<u16>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use crate::engine::voice::Voice;
    let sample_rate = state.audio_engine.sample_rate();
    let voice = Voice::new(Arc::new(build_beep(sample_rate, 2)), 2, sample_rate, 1.0, 0.0);
    state
        .audio_engine
        .play_test_output_patch_voice(voice, &device_id, &channels)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Play a test tone through the configured preview route. ASIO uses the
/// selected stereo pair on the already-open stream; other backends use the
/// selected independent aux device. The route is never the program bus.
#[tauri::command]
pub fn test_preview_audio(
    preview_device_id: Option<String>,
    backend: String,
    preview_asio_pair: Option<u32>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use crate::engine::voice::Voice;
    use crate::preferences::AudioBackend;
    let backend = match backend.as_str() {
        "asio" => AudioBackend::Asio,
        "wasapi_exclusive" => AudioBackend::WasapiExclusive,
        _ => AudioBackend::WasapiShared,
    };
    let sample_rate = state.audio_engine.sample_rate();
    let voice = Voice::new(Arc::new(build_beep(sample_rate, 2)), 2, sample_rate, 1.0, 0.0);
    state
        .audio_engine
        .play_test_preview_voice(voice, preview_device_id.filter(|id| !id.is_empty()), preview_asio_pair, backend)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Set the machine-local operator preview fader without restarting the main
/// audio stream. The active preview voice is updated atomically if present.
#[tauri::command]
pub fn set_preview_gain(gain_db: f32, state: State<'_, AppState>) -> Result<(), String> {
    let gain_db = gain_db.clamp(-60.0, 12.0);
    let mut config = crate::machine_config::load();
    config.preview_gain_db = gain_db;
    crate::machine_config::save(&config).map_err(|e| e.to_string())?;
    state.audio_engine.set_preview_gain_db(gain_db).map_err(|e| e.to_string())?;
    Ok(())
}

/// Spawn a background thread that opens a WASAPI stream on `device_name` (or
/// the default device if `None`) and plays a 440 Hz beep for ~400 ms.
///
/// The cpal `Stream` is created and destroyed entirely inside the thread to
/// avoid Send issues with the underlying Win32 handles.
fn play_beep_on_device(device_name: Option<String>) {
    std::thread::spawn(move || {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        let host = cpal::default_host();
        let device = match &device_name {
            Some(name) => host
                .output_devices()
                .ok()
                .and_then(|mut it| {
                    it.find(|d| d.id().ok().map(|id| id.id() == *name).unwrap_or(false))
                })
                .or_else(|| host.default_output_device()),
            None => host.default_output_device(),
        };

        let device = match device {
            Some(d) => d,
            None => return,
        };
        let config = match device.default_output_config() {
            Ok(c) => c,
            Err(_) => return,
        };

        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;
        let samples = Arc::new(build_beep(sample_rate, channels));
        let pos = Arc::new(AtomicUsize::new(0));
        let (s_cb, p_cb) = (samples.clone(), pos.clone());

        let Ok(stream) = device.build_output_stream(
            config.into(),
            move |data: &mut [f32], _| {
                let start = p_cb.fetch_add(data.len(), Ordering::Relaxed);
                for (i, s) in data.iter_mut().enumerate() {
                    *s = s_cb.get(start + i).copied().unwrap_or(0.0);
                }
            },
            |_| {},
            None,
        ) else {
            return;
        };

        if stream.play().is_ok() {
            std::thread::sleep(std::time::Duration::from_millis(600));
        }
        // stream drops here, closing the device
    });
}

/// Play a 440 Hz beep through the main audio engine (used for ASIO where only
/// one exclusive stream can be open at a time).
fn play_beep_via_engine(sample_rate: u32, state: &State<'_, AppState>) -> Result<(), String> {
    use crate::engine::voice::Voice;
    use std::sync::Arc;

    let samples = Arc::new(build_beep(sample_rate, 2));
    let voice = Voice::new(samples, 2, sample_rate, 1.0, 0.0);
    state
        .audio_engine
        .play_main_test_voice(voice)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Generate a 400 ms 440 Hz sine wave with 20 ms fade-in/out, interleaved for
/// `channels` output channels.
fn build_beep(sample_rate: u32, channels: usize) -> Vec<f32> {
    let n_frames = (sample_rate as f32 * 0.4) as usize;
    let fade = (sample_rate as f32 * 0.02) as usize;
    let mut buf = Vec::with_capacity(n_frames * channels);
    for i in 0..n_frames {
        let t = i as f32 / sample_rate as f32;
        let mut amp = (2.0 * PI * 440.0 * t).sin() * 0.4;
        if i < fade {
            amp *= i as f32 / fade as f32;
        }
        if i >= n_frames - fade {
            amp *= (n_frames - i) as f32 / fade as f32;
        }
        for _ in 0..channels {
            buf.push(amp);
        }
    }
    buf
}

/// Show the Preferences window (pre-created at startup, hidden by default).
///
/// Calling this a second time just brings the window to front.
#[tauri::command]
pub fn open_preferences_window(app_handle: AppHandle) -> Result<(), String> {
    let w = app_handle
        .get_webview_window("preferences")
        .ok_or("preferences window not found")?;
    w.show().map_err(|e| e.to_string())?;
    w.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod output_monitor_tests {
    use super::*;

    #[test]
    fn monitor_bitmap_is_a_top_down_32_bit_bmp() {
        let frame = crate::engine::network_io::BgraFrame {
            width: 2,
            height: 1,
            stride: 8,
            data: vec![0, 0, 255, 255, 0, 255, 0, 255],
        };
        let url = bgra_bmp_data_url(&frame).unwrap();
        let encoded = url.strip_prefix("data:image/bmp;base64,").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
        assert_eq!(&bytes[0..2], b"BM");
        assert_eq!(i32::from_le_bytes(bytes[18..22].try_into().unwrap()), 2);
        assert_eq!(i32::from_le_bytes(bytes[22..26].try_into().unwrap()), -1);
        assert_eq!(u16::from_le_bytes(bytes[28..30].try_into().unwrap()), 32);
        assert_eq!(&bytes[54..], frame.data.as_slice());
    }
}

#[cfg(test)]
mod display_output_monitor_preference_tests {
    use super::*;

    fn add_network_destination(preferences: &mut AppPreferences) {
        let mut ndi = preferences.display.output_destinations[0].clone();
        ndi.id = "program-ndi".into();
        ndi.name = "Program NDI".into();
        ndi.sink_kind = OutputSinkKind::Ndi;
        ndi.network.ndi.enabled = true;
        ndi.network.ndi.stream_name = "Program".into();
        preferences.display.output_destinations.push(ndi);
    }

    #[test]
    fn default_display_monitor_update_syncs_legacy_screen_only() {
        let mut preferences = AppPreferences::default();
        add_network_destination(&mut preferences);
        let previous_network = preferences.display.output_destinations[1].clone();

        let previous =
            set_display_output_monitor_preference(&mut preferences, "default", Some(2)).unwrap();

        assert_eq!(previous, vec![("default".to_owned(), None)]);
        assert_eq!(preferences.display.output_destinations[0].monitor, Some(2));
        assert_eq!(preferences.display.output_screen, Some(2));
        assert_eq!(preferences.display.output_destinations[1], previous_network);
    }

    #[test]
    fn non_default_display_update_does_not_change_legacy_or_network_state() {
        let mut preferences = AppPreferences::default();
        preferences.display.output_screen = Some(1);
        let mut aux = preferences.display.output_destinations[0].clone();
        // Keep the legacy mirror aligned with the default destination, as it
        // is after preferences migration from the legacy format.
        preferences.display.output_destinations[0].monitor = Some(1);
        aux.id = "confidence".into();
        aux.name = "Confidence".into();
        preferences.display.output_destinations.push(aux);
        add_network_destination(&mut preferences);
        let previous_network = preferences.display.output_destinations[2].clone();

        let previous = set_display_output_monitor_preference(
            &mut preferences,
            "confidence",
            Some(3),
        )
        .unwrap();

        assert_eq!(previous, vec![("confidence".to_owned(), None)]);
        assert_eq!(preferences.display.output_destinations[1].monitor, Some(3));
        assert_eq!(preferences.display.output_screen, Some(1));
        assert_eq!(preferences.display.output_destinations[2], previous_network);
    }

    #[test]
    fn network_destination_cannot_receive_a_physical_monitor_assignment() {
        let mut preferences = AppPreferences::default();
        add_network_destination(&mut preferences);
        let before = serde_json::to_value(&preferences).unwrap();

        let error =
            set_display_output_monitor_preference(&mut preferences, "program-ndi", Some(2))
                .unwrap_err();

        assert!(error.contains("not a physical display"));
        assert_eq!(serde_json::to_value(&preferences).unwrap(), before);
    }

    #[test]
    fn occupied_monitor_assignment_swaps_the_two_physical_outputs() {
        let mut preferences = AppPreferences::default();
        let mut confidence = preferences.display.output_destinations[0].clone();
        confidence.id = "confidence".into();
        confidence.name = "Confidence".into();
        confidence.monitor = Some(2);
        preferences.display.output_destinations[0].monitor = None;
        preferences.display.output_destinations.push(confidence);

        let previous = set_display_output_monitor_preference(
            &mut preferences,
            "default",
            Some(2),
        )
        .unwrap();

        assert_eq!(previous, vec![("default".to_owned(), None), ("confidence".to_owned(), Some(2))]);
        assert_eq!(preferences.display.output_destinations[0].monitor, Some(2));
        assert_eq!(preferences.display.output_destinations[1].monitor, None);
        assert_eq!(preferences.display.output_screen, Some(2));
    }

    #[test]
    fn duplicate_monitor_validator_ignores_network_and_floating_outputs() {
        let mut preferences = AppPreferences::default();
        let mut confidence = preferences.display.output_destinations[0].clone();
        confidence.id = "confidence".into();
        confidence.name = "Confidence".into();
        confidence.monitor = Some(2);
        preferences.display.output_destinations.push(confidence);
        assert!(validate_unique_display_monitors(
            &preferences.display.output_destinations
        )
        .is_ok());

        let mut duplicate = preferences.display.output_destinations[0].clone();
        duplicate.monitor = Some(2);
        preferences.display.output_destinations.push(duplicate);
        assert!(validate_unique_display_monitors(
            &preferences.display.output_destinations
        )
        .is_err());
    }

    #[test]
    fn legacy_duplicate_monitor_owner_is_rejected_without_mutation() {
        let mut preferences = AppPreferences::default();
        let target = preferences.display.output_destinations[0].clone();
        let mut first = preferences.display.output_destinations[0].clone();
        first.id = "first".into();
        first.name = "First".into();
        first.monitor = Some(2);
        let mut second = first.clone();
        second.id = "second".into();
        second.name = "Second".into();
        preferences.display.output_destinations = vec![target, first, second];
        let before = serde_json::to_value(&preferences).unwrap();

        let error = set_display_output_monitor_preference(
            &mut preferences,
            "default",
            Some(2),
        )
        .unwrap_err();

        assert!(error.contains("multiple physical output owners"));
        assert_eq!(serde_json::to_value(&preferences).unwrap(), before);
    }
}

#[cfg(test)]
mod machine_audio_apply_tests {
    use super::*;
    use crate::engine::device_manager::OutputPatch;

    #[test]
    fn identical_machine_apply_does_not_require_audio_restart() {
        let current = MachineAudioConfig::default();
        assert!(!machine_audio_config_requires_restart(&current, &current.clone()));
    }

    #[test]
    fn changed_machine_setting_requires_audio_restart() {
        let current = MachineAudioConfig::default();
        let requested = MachineAudioConfig {
            buffer_size: current.buffer_size + 1,
            ..current.clone()
        };
        assert!(machine_audio_config_requires_restart(&current, &requested));
    }

    #[test]
    fn asio_preview_pair_rejects_overlapping_main_output_patch() {
        let config = MachineAudioConfig {
            backend: crate::preferences::AudioBackend::Asio,
            device_id: Some("yamaha".to_owned()),
            preview_asio_pair: Some(2),
            ..MachineAudioConfig::default()
        };
        let patch = OutputPatch::new("Monitors", "yamaha", vec![4, 5]);
        let error = validate_preview_pair_output_patches(&config, &[patch]).unwrap_err();
        assert!(error.contains("Monitors"));
        assert!(error.contains("Out 5-6"));
    }

    #[test]
    fn unchanged_output_screen_does_not_reposition_live_output() {
        assert!(!output_screen_requires_apply(None, None));
        assert!(!output_screen_requires_apply(Some(1), Some(1)));
        assert!(output_screen_requires_apply(None, Some(0)));
    }

    #[test]
    fn runtime_status_never_calls_a_fallback_or_failed_stream_working() {
        assert_eq!(audio_main_runtime_state(false, false), "working");
        assert_eq!(audio_main_runtime_state(false, true), "fallback");
        assert_eq!(audio_main_runtime_state(true, false), "error");
        assert_eq!(audio_preview_runtime_state(
            &crate::preferences::AudioBackend::WasapiShared,
            None,
            Some("headphones"),
            "working",
        ), "not_tested");
        assert_eq!(audio_preview_runtime_state(
            &crate::preferences::AudioBackend::Asio,
            Some(1),
            None,
            "error",
        ), "error");
    }
}
