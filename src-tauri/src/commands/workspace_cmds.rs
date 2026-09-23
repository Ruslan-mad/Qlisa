//! Tauri commands for workspace save / load / new.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{Emitter, State};

use crate::{
    commands::cue_list_cmds,
    cue::types::CueType,
    show::{workspace::CollectReport, Workspace},
    state::AppState,
};

/// Overlay the authoritative machine-wide Preferences onto a workspace
/// runtime mirror.  Legacy workspace values are used only when the global
/// file has not been created yet; after that, unknown destination IDs are
/// merged so old cues keep routing without replacing global settings.
fn apply_global_preferences(state: &AppState, workspace: &mut Workspace) -> Result<(), String> {
    let _write_guard = crate::state::app_state::GLOBAL_PREFERENCES_WRITE_GATE
        .lock()
        .map_err(|_| "Global Preferences write gate is poisoned".to_string())?;
    let existing = if state
        .global_preferences_file_present
        .load(std::sync::atomic::Ordering::Acquire)
    {
        Some(state.global_preferences_snapshot()?)
    } else {
        None
    };
    let (mut resolved, changed) =
        crate::preferences::resolve_global_preferences(existing, &workspace.preferences);
    crate::preferences::inject_runtime_audio_buffer_size(
        &mut resolved,
        crate::machine_config::load().buffer_size,
    );
    let file_present = state
        .global_preferences_file_present
        .load(std::sync::atomic::Ordering::Acquire);
    let migration_allowed = state
        .global_preferences_migration_allowed
        .load(std::sync::atomic::Ordering::Acquire);
    if crate::preferences::should_persist_global_migration(file_present, migration_allowed, changed)
    {
        resolved = state.save_global_preferences(resolved)?;
    }
    workspace.preferences = resolved;
    Ok(())
}

fn effective_default_output_id(preferences: &crate::preferences::AppPreferences) -> String {
    preferences
        .display
        .output_destinations
        .iter()
        .find(|destination| destination.id == preferences.display.default_output_id)
        .map(|destination| destination.id.clone())
        .or_else(|| {
            preferences
                .display
                .output_destinations
                .first()
                .map(|destination| destination.id.clone())
        })
        .unwrap_or_else(|| "default".into())
}

/// Recursively collect (id, path, type) tuples for every Audio and Video cue,
/// including those nested inside groups at any depth.  The type lets the preload
/// worker surface a decode *failure* for Audio cues only (a video without an
/// audio track, or whose track fails, still plays — no operator warning).
fn collect_media_cues(
    cues: &[Box<dyn crate::cue::traits::Cue>],
    out: &mut Vec<(uuid::Uuid, PathBuf, CueType)>,
) {
    for cue in cues {
        if matches!(cue.cue_type(), CueType::Audio | CueType::Video) {
            let json = cue.serialize();
            if let Some(p) = json.get("file_path").and_then(|v| v.as_str()) {
                if !p.is_empty() {
                    out.push((cue.id(), PathBuf::from(p), cue.cue_type()));
                }
            }
        }
        if let Some(children) = cue.child_cues() {
            collect_media_cues(children, out);
        }
    }
}

/// Create a new empty workspace, discarding the current one.
#[tauri::command]
pub fn new_workspace(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    state.media_converter.cancel_all();
    let mut new_workspace =
        Workspace::new_with_preferences("Untitled", state.global_preferences_snapshot()?);
    apply_global_preferences(state.inner(), &mut new_workspace)?;
    let output_destinations = new_workspace
        .preferences
        .display
        .output_destinations
        .clone();
    let default_output_id = effective_default_output_id(&new_workspace.preferences);
    let show_floating = new_workspace.preferences.display.show_output_timer
        && new_workspace.preferences.display.timer_floating;
    let timer_font = new_workspace.preferences.display.timer_font.clone();
    let timer_font_size = new_workspace.preferences.display.timer_font_size;
    let timer_position = new_workspace.preferences.display.timer_position;
    let timer_margin = new_workspace.preferences.display.timer_margin;

    // Reconcile the native output graph before replacing the workspace.  In
    // particular this retires any NDI/SRT destinations from the previous show.
    state
        .output_engine
        .sync_output_destinations(&output_destinations, &default_output_id)
        .map_err(|e| format!("Could not activate default output destinations: {e}"))?;

    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    *ws = new_workspace;
    cue_list_cmds::emit_cue_lists_changed(&app_handle, &ws);
    let outputs = ws.universe_outputs.clone();
    drop(ws);
    if let Ok(mut metadata) = state.media_metadata.lock() {
        metadata.clear();
    }
    state
        .output_engine
        .set_floating_timer_visible(show_floating);
    state
        .output_engine
        .set_timer_style(&timer_font, timer_font_size, timer_position, timer_margin);
    // A fresh workspace has no DMX outputs — clear any from the previous show.
    state.dmx_engine.set_outputs(outputs);
    // Starting clean — retire any load-skip banner from the previous show.
    crate::health::clear("workspace-load-skips");
    let _ = app_handle.emit("workspace-replaced", serde_json::json!({}));
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Save the workspace to the given path.
#[tauri::command]
pub fn save_workspace(path: String, state: State<'_, AppState>) -> Result<(), String> {
    {
        let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
        ws.save(Some(PathBuf::from(path)))
            .map_err(|e| e.to_string())?;
    }
    // Work is now persisted to the real `.inkue` file — drop the crash-recovery
    // snapshot so a crash right after saving does not prompt a redundant restore.
    crate::recovery::delete();
    Ok(())
}

/// Load a workspace from the given path.
#[tauri::command]
pub fn load_workspace(
    path: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let loaded = Workspace::load(PathBuf::from(&path), &registry).map_err(|e| e.to_string())?;
    drop(registry);

    install_workspace(state.inner(), &app_handle, loaded)
}

/// Install a freshly loaded/restored workspace into the running app: store it,
/// emit cue-list + modified events, rebind DMX outputs + floating timer, and
/// kick off background media preload.  Shared by [`load_workspace`] and the
/// crash-recovery restore path.
pub(crate) fn install_workspace(
    state: &AppState,
    app_handle: &tauri::AppHandle,
    mut loaded: Workspace,
) -> Result<(), String> {
    state.media_converter.cancel_all();
    // Resolve machine-local audio bus bindings before the workspace enters the
    // transport snapshot. Legacy physical fields seed audio.json once; new
    // shows carry only logical bus IDs.
    let mut audio_config = crate::machine_config::load();
    if loaded.apply_machine_audio_bindings(&mut audio_config) {
        crate::machine_config::save(&audio_config).map_err(|e| e.to_string())?;
    }
    apply_global_preferences(state, &mut loaded)?;
    if let Ok(mut metadata) = state.media_metadata.lock() {
        metadata.clear();
    }
    // Collect audio + video cue IDs + file paths before storing the workspace.
    // Scan ALL cue lists so non-active lists are also preloaded on open.
    let cues_to_preload: Vec<(uuid::Uuid, PathBuf, CueType)> = {
        let mut result = Vec::new();
        for cl in &loaded.cue_lists {
            collect_media_cues(&cl.cues, &mut result);
        }
        result
    };

    // Warn the operator when the file carried cues that could not be loaded
    // (unknown type / corrupt data) — otherwise they vanish with no signal.
    if loaded.cues_skipped_on_load > 0 {
        let n = loaded.cues_skipped_on_load;
        crate::health::set(crate::health::HealthAlert::new(
            "workspace-load-skips",
            crate::health::HealthLevel::Warning,
            format!(
                "{n} cue(s) could not be loaded (unknown type or corrupt data) and were skipped."
            ),
        ));
    } else {
        crate::health::clear("workspace-load-skips");
    }

    // Store the new workspace and apply global display/output preferences.
    let show_floating =
        loaded.preferences.display.show_output_timer && loaded.preferences.display.timer_floating;
    let output_screen = loaded.preferences.display.output_screen;
    let timer_font = loaded.preferences.display.timer_font.clone();
    let timer_font_size = loaded.preferences.display.timer_font_size;
    let timer_position = loaded.preferences.display.timer_position;
    let timer_margin = loaded.preferences.display.timer_margin;
    let output_destinations = loaded.preferences.display.output_destinations.clone();
    let default_output_id = effective_default_output_id(&loaded.preferences);
    let dmx_outputs = loaded.universe_outputs.clone();

    // Reconcile the global output graph before replacing the live workspace.
    // This makes NDI/SRT and secondary displays available immediately after
    // opening a show, and preserves the old runtime graph if native creation
    // fails (for example, a missing libmpv or an invalid network destination).
    state
        .output_engine
        .sync_output_destinations(&output_destinations, &default_output_id)
        .map_err(|e| format!("Could not activate saved output destinations: {e}"))?;

    {
        let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
        *ws = loaded;
        // Mirror the (just-loaded) auto-renumber preference onto the cue lists.
        ws.sync_auto_renumber();
        cue_list_cmds::emit_cue_lists_changed(app_handle, &ws);
    }
    let _ = app_handle.emit("workspace-replaced", serde_json::json!({}));
    state
        .output_engine
        .set_floating_timer_visible(show_floating);
    state
        .output_engine
        .set_timer_style(&timer_font, timer_font_size, timer_position, timer_margin);
    // Light the configured output screen right away (black fullscreen surface)
    // instead of waiting for the first visual GO.
    state
        .output_engine
        .apply_output_screen_on_load(output_screen);
    // Bind the engine's sinks to the loaded show's universe outputs.
    state.dmx_engine.set_outputs(dmx_outputs);
    {
        let mut loading = state.loading_cues.lock().map_err(|e| e.to_string())?;
        // Clear any stale entries from a previous workspace.
        loading.clear();
        for (id, _, _) in &cues_to_preload {
            loading.insert(*id);
        }
    }

    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));

    // Background media preload. A show can hold dozens of audio cues (a 58-cue
    // QLab import melted the machine): spawning one decode thread per cue was a
    // thundering herd — every thread fought for the workspace lock and emitted
    // `workspace-modified`, and each of those triggers a full UI refresh.
    //
    // Three fixes: (1) dedup by file — many cues point at the same file (a Canon
    // reuses one clip across sections), so probe each distinct file once and
    // let each cue start a bounded source; (2) drain the work with a small
    // bounded pool; (3) let one coordinator coalesce the UI refresh to a few
    // times a second instead of once per cue.
    let mut cues_by_path: HashMap<PathBuf, Vec<(uuid::Uuid, bool)>> = HashMap::new();
    for (id, path, cue_type) in cues_to_preload {
        cues_by_path
            .entry(path)
            .or_default()
            .push((id, matches!(cue_type, CueType::Audio)));
    }

    let job_count = cues_by_path.len();
    if job_count > 0 {
        let (tx, rx) = crossbeam_channel::unbounded::<(PathBuf, Vec<(uuid::Uuid, bool)>)>();
        for job in cues_by_path {
            let _ = tx.send(job);
        }
        drop(tx); // close the queue so workers exit once it is drained

        let remaining = Arc::new(AtomicUsize::new(job_count));
        let dirty = Arc::new(AtomicBool::new(false));

        // Leave a core free for the audio callback + UI; cap peak decode memory.
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        let workers = cpus.saturating_sub(1).clamp(1, 4).min(job_count);

        for _ in 0..workers {
            let rx = rx.clone();
            let workspace = state.workspace.clone();
            let loading_cues = state.loading_cues.clone();
            let dirty = Arc::clone(&dirty);
            let remaining = Arc::clone(&remaining);
            std::thread::Builder::new()
                .name("inkue-preload".into())
                .spawn(move || {
                    while let Ok((file_path, cues)) = rx.recv() {
                        match crate::cue::media_decode::probe_audio_track(&file_path) {
                            Ok(Some(info)) => {
                                let duration = info.total_frames.map(|frames| {
                                    Duration::from_secs_f64(frames as f64 / info.sample_rate.max(1) as f64)
                                });
                                // Store only probed metadata. PCM is opened and
                                // streamed into a bounded ring on GO; workspace
                                // load never allocates a whole-file sample buffer.
                                if let Ok(mut ws) = workspace.lock() {
                                    for (cue_id, _) in &cues {
                                        'store: for cl in ws.cue_lists.iter_mut() {
                                            if let Some(cue) = cl.get_mut_recursive(cue_id) {
                                                cue.accept_preloaded_stream(file_path.clone(), info.channels, info.sample_rate, duration);
                                                break 'store;
                                            }
                                        }
                                    }
                                }
                                // Decoded fine — retire any stale decode-failure banners.
                                for (cue_id, is_audio) in &cues {
                                    if *is_audio {
                                        super::clear_decode_failure(*cue_id);
                                    }
                                }
                            }
                            Ok(None) => {
                                for (cue_id, is_audio) in &cues {
                                    if *is_audio {
                                        super::surface_decode_failure(*cue_id, &file_path);
                                    }
                                }
                            } // silent video — nothing to preload
                            Err(e) => {
                                log::warn!("Preload failed for {}: {e}", file_path.display());
                                // An Audio cue that can't decode would be a silent
                                // no-op at GO — surface it instead of only logging.
                                for (cue_id, is_audio) in &cues {
                                    if *is_audio {
                                        super::surface_decode_failure(*cue_id, &file_path);
                                    }
                                }
                            }
                        }
                        if let Ok(mut loading) = loading_cues.lock() {
                            for (cue_id, _) in &cues {
                                loading.remove(cue_id);
                            }
                        }
                        dirty.store(true, Ordering::Relaxed);
                        remaining.fetch_sub(1, Ordering::Relaxed);
                    }
                })
                .expect("Failed to spawn preload worker");
        }

        // Coordinator: emit one coalesced `workspace-modified` at most ~4×/s while
        // preloads land, plus a final one when the batch is done. This replaces the
        // per-cue emit storm that re-rendered the whole cue list dozens of times.
        let app_handle2 = app_handle.clone();
        std::thread::Builder::new()
            .name("inkue-preload-coord".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(250));
                let done = remaining.load(Ordering::Relaxed) == 0;
                if dirty.swap(false, Ordering::Relaxed) || done {
                    let _ = app_handle2.emit("workspace-modified", serde_json::json!({}));
                }
                if done {
                    break;
                }
            })
            .expect("Failed to spawn preload coordinator");
    }

    Ok(())
}

/// Copy all media files into a self-contained project folder and write a
/// new `.inkue` file there with updated relative paths.
///
/// `target_dir` is the parent directory chosen by the user; the command
/// creates `{target_dir}/{workspace_name}/` automatically.
///
/// The workspace currently open in memory is not affected.
#[tauri::command]
pub fn collect_and_save_workspace(
    target_dir: String,
    state: State<'_, AppState>,
) -> Result<CollectReport, String> {
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.collect_and_save(std::path::Path::new(&target_dir))
        .map_err(|e| e.to_string())
}

/// Return basic workspace metadata for the title bar.
#[tauri::command]
pub fn get_workspace_info(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "name": ws.metadata.name,
        "is_modified": ws.is_modified,
        "file_path": ws.file_path.as_ref().map(|p| p.to_string_lossy().to_string()),
    }))
}

// ---------------------------------------------------------------------------
// QLab import
// ---------------------------------------------------------------------------

/// Import a QLab workspace (`.qlab4` / `.qlab5`) and make it the current show.
///
/// The imported workspace is **unsaved and has no file path**: media paths are
/// resolved against the QLab bundle folder, so the show plays straight away
/// without anything being written into the user's QLab project. Save As puts
/// it where the operator wants it; Collect and Save makes it self-contained.
#[tauri::command]
pub fn import_qlab_workspace(
    path: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<crate::qlab_import::ImportReport, String> {
    let source = PathBuf::from(&path);
    let (document, report) =
        crate::qlab_import::import_workspace(&source).map_err(|e| format!("{e:#}"))?;

    // The bundle folder is the base for the document's relative media paths.
    let base_dir = source.parent().map(|p| p.to_path_buf());
    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut imported = Workspace::from_json_str(&document, base_dir.as_deref(), &registry)
        .map_err(|e| format!("{e:#}"))?;
    drop(registry);

    // No file path: this is an import, not an open. The title bar shows it as
    // modified, and Save As is the next step.
    imported.file_path = None;
    imported.mark_modified();

    install_workspace(state.inner(), &app_handle, imported)?;
    Ok(report)
}
