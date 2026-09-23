//! Tauri commands for undo, redo, copy and paste.
//!
//! The undo/redo system is snapshot-based: before every mutating command
//! [`push_current_snapshot`] is called, which captures the full cue list state
//! (serialised JSON + decoded audio Arcs) and pushes it onto the [`UndoStack`].
//! Undo/redo swap the current state with the stored snapshot atomically while
//! holding the registry, workspace, loading-cue set and undo_stack locks in
//! that order.
//!
//! Lock ordering (always respected to prevent deadlocks):
//!   registry → workspace → loading_cues → undo_stack → clipboard

use std::collections::{HashMap, HashSet};

use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::{
    cue::registry::CueRegistry,
    show::{
        cue_list::CueList,
        undo_stack::{CueSnapshot, DecodedAudio, Snapshot},
    },
    state::AppState,
};

// ---------------------------------------------------------------------------
// Snapshot helpers (pub so cue_cmds can call push_current_snapshot)
// ---------------------------------------------------------------------------

/// Serialise the current cue list into a [`Snapshot`].
///
/// The decoded audio `Arc` for each cue is cloned — this is a reference-count
/// bump, not a data copy, so it is O(n_cues) not O(total_samples).
pub fn take_snapshot(cue_list: &CueList) -> Snapshot {
    fn collect_decoded(cues: &[Box<dyn crate::cue::traits::Cue>], out: &mut HashMap<Uuid, DecodedAudio>) {
        for cue in cues {
            if let Some(decoded) = cue.extract_decoded_audio() {
                out.insert(cue.id(), decoded);
            }
            if let Some(children) = cue.child_cues() {
                collect_decoded(children, out);
            }
        }
    }

    let mut decoded_audio = HashMap::new();
    collect_decoded(&cue_list.cues, &mut decoded_audio);
    Snapshot {
        cues: cue_list
            .cues
            .iter()
            .map(|c| CueSnapshot {
                json: c.serialize(),
            })
            .collect(),
        decoded_audio,
        playhead_id: cue_list.playhead_cue_id,
    }
}

/// Restore a [`Snapshot`] into `cue_list`, rebuilding every cue via the
/// registry and re-injecting decoded audio so there is no re-decode round-trip.
fn restore_snapshot(
    snapshot: Snapshot,
    cue_list: &mut CueList,
    registry: &CueRegistry,
) -> anyhow::Result<()> {
    fn restore_decoded(cue: &mut dyn crate::cue::traits::Cue, decoded_audio: &HashMap<Uuid, DecodedAudio>) {
        if let Some((samples, channels, sr, duration)) = decoded_audio.get(&cue.id()) {
            cue.accept_preloaded_audio(samples.clone(), *channels, *sr, *duration);
        }
        if let Some(children) = cue.child_cues_mut() {
            for child in children {
                restore_decoded(child.as_mut(), decoded_audio);
            }
        }
    }

    let Snapshot {
        cues,
        decoded_audio,
        playhead_id,
    } = snapshot;
    cue_list.cues.clear();
    for cs in cues {
        let mut cue = registry.from_json(cs.json)?;
        restore_decoded(cue.as_mut(), &decoded_audio);
        cue_list.cues.push(cue);
    }
    // Restore playhead; clear it if the referenced cue no longer exists.
    cue_list.playhead_cue_id = playhead_id
        .filter(|id| cue_list.cues.iter().any(|c| c.id() == *id));
    Ok(())
}

/// Reject an operation that would replace the cue hierarchy while cue runtime
/// resources are active.  A Group is not itself enough: every descendant is
/// inspected so an active leaf can never be orphaned by undo/redo.
pub(crate) fn ensure_transport_safe(
    cue_list: &CueList,
    loading_cues: &HashSet<Uuid>,
) -> Result<(), String> {
    fn visit(cues: &[Box<dyn crate::cue::traits::Cue>], loading: &HashSet<Uuid>) -> Option<String> {
        for cue in cues {
            let id = cue.id();
            if loading.contains(&id) {
                return Some(format!("Cue {} is loading; wait for it to finish before undoing or redoing", id));
            }
            if cue.is_preloaded_or_loading() {
                return Some(format!("Cue {} is preloaded; stop it before undoing or redoing", id));
            }
            if !matches!(cue.state(), crate::cue::types::CueState::Standby | crate::cue::types::CueState::Completed) {
                return Some(format!("Cue {} is {:?}; stop it before undoing or redoing", id, cue.state()));
            }
            if let Some(children) = cue.child_cues() {
                if let Some(reason) = visit(children, loading) {
                    return Some(reason);
                }
            }
        }
        None
    }
    visit(&cue_list.cues, loading_cues).map_or(Ok(()), Err)
}

/// Capture the current cue list state and push it onto the undo stack.
///
/// **Call this at the very start of every mutating command, before applying
/// any change.**  The function acquires and releases the workspace lock
/// separately from the undo_stack lock so no deadlock is possible.
pub fn push_current_snapshot(state: &AppState) -> Result<(), String> {
    // 1. Briefly lock the workspace to read the current state.
    let snapshot = {
        let ws = state.workspace.lock().map_err(|e| e.to_string())?;
        let cl = ws.active_cue_list().ok_or("No active cue list")?;
        take_snapshot(cl)
        // workspace lock released here
    };
    // 2. Push onto the undo stack (separate lock, no deadlock risk).
    state
        .undo_stack
        .lock()
        .map_err(|e| e.to_string())?
        .push_action(snapshot);
    Ok(())
}

// ---------------------------------------------------------------------------
// Undo / Redo
// ---------------------------------------------------------------------------

/// Whether there is at least one action that can be undone.
#[tauri::command]
pub fn can_undo(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state
        .undo_stack
        .lock()
        .map_err(|e| e.to_string())?
        .can_undo())
}

/// Whether there is at least one action that can be re-done.
#[tauri::command]
pub fn can_redo(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state
        .undo_stack
        .lock()
        .map_err(|e| e.to_string())?
        .can_redo())
}

/// Restore the cue list to its state before the most-recent mutating action.
#[tauri::command]
pub fn undo(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // Lock order: registry → workspace → loading_cues → undo_stack.
    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let loading = state.loading_cues.lock().map_err(|e| e.to_string())?;
    ensure_transport_safe(cue_list, &loading)?;
    drop(loading);
    let mut stack = state.undo_stack.lock().map_err(|e| e.to_string())?;
    let current = take_snapshot(cue_list);

    if let Some(prev) = stack.undo(current) {
        restore_snapshot(prev, cue_list, &registry).map_err(|e| e.to_string())?;
        ws.mark_modified();
    } else {
        return Ok(()); // nothing to undo
    }

    let playhead_id = ws
        .active_cue_list()
        .and_then(|cl| cl.playhead_cue_id)
        .map(|id| id.to_string());
    drop(stack);
    drop(ws);
    drop(registry);

    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    let _ = app_handle.emit(
        "playhead-moved",
        serde_json::json!({ "cue_id": playhead_id }),
    );
    Ok(())
}

/// Re-apply the most-recently undone action.
#[tauri::command]
pub fn redo(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // Lock order: registry → workspace → loading_cues → undo_stack.
    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let loading = state.loading_cues.lock().map_err(|e| e.to_string())?;
    ensure_transport_safe(cue_list, &loading)?;
    drop(loading);
    let mut stack = state.undo_stack.lock().map_err(|e| e.to_string())?;
    let current = take_snapshot(cue_list);

    if let Some(next) = stack.redo(current) {
        restore_snapshot(next, cue_list, &registry).map_err(|e| e.to_string())?;
        ws.mark_modified();
    } else {
        return Ok(()); // nothing to redo
    }

    let playhead_id = ws
        .active_cue_list()
        .and_then(|cl| cl.playhead_cue_id)
        .map(|id| id.to_string());
    drop(stack);
    drop(ws);
    drop(registry);

    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    let _ = app_handle.emit(
        "playhead-moved",
        serde_json::json!({ "cue_id": playhead_id }),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Copy / Paste
// ---------------------------------------------------------------------------

/// Copy one cue into the in-app clipboard by serialising it to JSON.
///
/// This legacy command intentionally keeps the original object-shaped
/// clipboard format so single-cue paste remains backwards compatible.
#[tauri::command]
pub fn copy_cue(
    cue_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
    let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
    let json = cue.serialize();
    drop(ws);
    *state.clipboard.lock().map_err(|e| e.to_string())? = Some(json);
    Ok(())
}

/// Collect selected cues in playlist order, treating a selected Group as one
/// root so its children are not copied a second time.  Selected children whose
/// parent is not selected become independent top-level clipboard roots.
fn collect_selected_clipboard_cues(
    cues: &[Box<dyn crate::cue::traits::Cue>],
    selected: &HashSet<Uuid>,
    ancestor_selected: bool,
    out: &mut Vec<serde_json::Value>,
) {
    for cue in cues {
        let selected_here = selected.contains(&cue.id());
        if selected_here && !ancestor_selected {
            out.push(cue.serialize());
        }
        if let Some(children) = cue.child_cues() {
            collect_selected_clipboard_cues(
                children,
                selected,
                ancestor_selected || selected_here,
                out,
            );
        }
    }
}

/// Copy a playlist selection.  The wrapper object is versioned so old
/// single-cue clipboard values can still be pasted by the same command.
#[tauri::command]
pub fn copy_cues(
    cue_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if cue_ids.is_empty() {
        return Err("No cues selected".into());
    }
    let selected: HashSet<Uuid> = cue_ids
        .iter()
        .map(|id| id.parse::<Uuid>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
    let mut cues = Vec::new();
    collect_selected_clipboard_cues(&cue_list.cues, &selected, false, &mut cues);
    if cues.is_empty() {
        return Err("Cue not found".into());
    }
    drop(ws);
    *state.clipboard.lock().map_err(|e| e.to_string())? = Some(serde_json::json!({
        "__inkue_clipboard": "cue-set-v1",
        "cues": cues,
    }));
    Ok(())
}

/// Build an old→new UUID map for a serialized cue tree, including all Group
/// descendants, then replace every matching UUID string in the JSON.  The
/// latter also updates target_cue_id(s) and other internal references without
/// needing a per-cue-type list of fields.
fn collect_serialized_cue_ids(value: &serde_json::Value, map: &mut HashMap<Uuid, Uuid>) {
    if let Some(object) = value.as_object() {
        if let Some(old) = object.get("id").and_then(|v| v.as_str()).and_then(|v| v.parse().ok()) {
            map.entry(old).or_insert_with(Uuid::new_v4);
        }
        if let Some(children) = object.get("children") {
            collect_serialized_cue_ids(children, map);
        }
    } else if let Some(array) = value.as_array() {
        for item in array {
            collect_serialized_cue_ids(item, map);
        }
    }
}

fn replace_serialized_cue_ids(value: &mut serde_json::Value, map: &HashMap<Uuid, Uuid>) {
    match value {
        serde_json::Value::String(text) => {
            if let Ok(old) = text.parse::<Uuid>() {
                if let Some(new) = map.get(&old) {
                    *text = new.to_string();
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                replace_serialized_cue_ids(item, map);
            }
        }
        serde_json::Value::Object(object) => {
            for item in object.values_mut() {
                replace_serialized_cue_ids(item, map);
            }
        }
        _ => {}
    }
}

/// Remap every ID across a set of roots with one shared map. This matters for
/// cross-root target references (for example a selected Stop cue targeting a
/// separately selected Audio cue).
fn remap_serialized_cue_set_ids(values: &mut [serde_json::Value]) -> HashMap<Uuid, Uuid> {
    let mut map = HashMap::new();
    for value in values.iter() {
        collect_serialized_cue_ids(value, &mut map);
    }
    for value in values {
        replace_serialized_cue_ids(value, &map);
    }
    map
}

fn collect_decoded_audio(
    cue_list: &CueList,
    value: &serde_json::Value,
    id_map: &HashMap<Uuid, Uuid>,
    out: &mut HashMap<Uuid, DecodedAudio>,
) {
    if let Some(old_id) = value
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<Uuid>().ok())
    {
        if let Some(new_id) = id_map.get(&old_id) {
            if let Some(cue) = cue_list.get_recursive(&old_id) {
                if let Some(decoded) = cue.extract_decoded_audio() {
                    out.insert(*new_id, decoded);
                }
            }
        }
    }
    if let Some(children) = value.get("children") {
        if let Some(items) = children.as_array() {
            for child in items {
                collect_decoded_audio(cue_list, child, id_map, out);
            }
        }
    }
}

fn collect_audio_fallbacks(
    value: &serde_json::Value,
    id_map: &HashMap<Uuid, Uuid>,
    decoded: &HashMap<Uuid, DecodedAudio>,
    out: &mut Vec<(Uuid, std::path::PathBuf)>,
) {
    let old_id = value
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<Uuid>().ok());
    if let Some(new_id) = old_id.and_then(|id| id_map.get(&id).copied()) {
        if !decoded.contains_key(&new_id) {
            if let Some(path) = value
                .get("file_path")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
            {
                out.push((new_id, std::path::PathBuf::from(path)));
            }
        }
    }
    if let Some(items) = value.get("children").and_then(|v| v.as_array()) {
        for child in items {
            collect_audio_fallbacks(child, id_map, decoded, out);
        }
    }
}

fn accept_decoded_audio_recursive(
    cue: &mut dyn crate::cue::traits::Cue,
    decoded: &HashMap<Uuid, DecodedAudio>,
) {
    if let Some((samples, channels, sample_rate, duration)) = decoded.get(&cue.id()) {
        cue.accept_preloaded_audio(samples.clone(), *channels, *sample_rate, *duration);
    }
    if let Some(children) = cue.child_cues_mut() {
        for child in children {
            accept_decoded_audio_recursive(child.as_mut(), decoded);
        }
    }
}

fn spawn_paste_preload(
    state: &AppState,
    app_handle: &AppHandle,
    new_id: Uuid,
    file_path: std::path::PathBuf,
) -> Result<(), String> {
    {
        let mut loading = state.loading_cues.lock().map_err(|e| e.to_string())?;
        loading.insert(new_id);
    }
    let workspace = state.workspace.clone();
    let loading_cues = state.loading_cues.clone();
    let app_handle2 = app_handle.clone();
    std::thread::Builder::new()
        .name("inkue-preload-paste".into())
        .spawn(move || {
            #[cfg(windows)]
            // SAFETY: only changes the scheduling priority of this thread.
            unsafe {
                use std::os::raw::c_void;
                extern "system" {
                    fn GetCurrentThread() -> *mut c_void;
                    fn SetThreadPriority(h_thread: *mut c_void, n_priority: i32) -> i32;
                }
                SetThreadPriority(GetCurrentThread(), -1);
            }
            match crate::cue::media_decode::probe_audio_track(&file_path) {
                Ok(Some(info)) => {
                    let duration = info.total_frames.map(|frames| std::time::Duration::from_secs_f64(
                        frames as f64 / info.sample_rate.max(1) as f64,
                    ));
                    if let Ok(mut ws) = workspace.lock() {
                        if let Some(cl) = ws.active_cue_list_mut() {
                            if let Some(cue) = cl.get_mut_recursive(&new_id) {
                                cue.accept_preloaded_stream(file_path.clone(), info.channels, info.sample_rate, duration);
                            }
                        }
                    }
                    if let Ok(mut loading) = loading_cues.lock() {
                        loading.remove(&new_id);
                    }
                    let _ = app_handle2.emit("workspace-modified", serde_json::json!({}));
                }
                Ok(None) => {
                    if let Ok(mut loading) = loading_cues.lock() {
                        loading.remove(&new_id);
                    }
                    log::warn!("Background preload (paste fallback) found no audio in {:?}", file_path);
                    let _ = app_handle2.emit("workspace-modified", serde_json::json!({}));
                }
                Err(e) => {
                    if let Ok(mut loading) = loading_cues.lock() {
                        loading.remove(&new_id);
                    }
                    log::warn!("Background preload (paste fallback) failed for {:?}: {e}", file_path);
                    let _ = app_handle2.emit("workspace-modified", serde_json::json!({}));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Insert prepared roots in clipboard order. Inserting backwards after one
/// anchor keeps a multi-paste ordered while using the hierarchy-aware list
/// operation already shared by duplicate commands.
fn insert_pasted_cues(
    cue_list: &mut CueList,
    prepared: Vec<(Uuid, Box<dyn crate::cue::traits::Cue>)>,
    anchor: Option<Uuid>,
) -> Result<Vec<String>, String> {
    let anchor_exists = anchor.is_some_and(|id| cue_list.get_recursive(&id).is_some());
    let pasted_ids: Vec<String> = prepared.iter().map(|(id, _)| id.to_string()).collect();

    if anchor_exists {
        for (_, cue) in prepared.into_iter().rev() {
            cue_list
                .insert_after_anywhere(&anchor.expect("anchor_exists implies anchor"), cue)
                .map_err(|e| e.to_string())?;
        }
    } else {
        for (_, cue) in prepared {
            let end = cue_list.cues.len();
            cue_list.insert(end, cue);
        }
    }
    Ok(pasted_ids)
}

fn clipboard_paste_result(
    is_set: bool,
    pasted_ids: Vec<String>,
) -> Result<serde_json::Value, String> {
    if pasted_ids.is_empty() {
        return Err("Paste produced no cues".into());
    }
    if is_set {
        Ok(serde_json::Value::Array(
            pasted_ids.into_iter().map(serde_json::Value::String).collect(),
        ))
    } else {
        Ok(serde_json::Value::String(pasted_ids.into_iter().next().unwrap()))
    }
}

/// Paste the clipboard cue or cue set as new cue(s) inserted after
/// `after_cue_id`.  A legacy single-cue clipboard returns a string; a copied
/// set returns an array of fresh IDs.
///
/// - If `after_cue_id` is `Some`, the new cue is inserted immediately after
///   the specified cue.
/// - If `after_cue_id` is `None`, the new cue is appended at the end.
///
/// Every pasted cue gets fresh UUIDs so it is independent of the original.
/// Returns a string for legacy single-cue clipboard data and an array for a
/// cue-set clipboard.
#[tauri::command]
pub fn paste_cue(
    after_cue_id: Option<String>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    // 1. Clone the clipboard JSON (brief clipboard lock).
    let clipboard = {
        let clip = state.clipboard.lock().map_err(|e| e.to_string())?;
        clip.clone().ok_or("Clipboard is empty — copy a cue first")?
    };

    let is_set = clipboard.get("__inkue_clipboard").and_then(|v| v.as_str()) == Some("cue-set-v1");
    let templates = if is_set {
        clipboard
            .get("cues")
            .and_then(|v| v.as_array())
            .cloned()
            .ok_or("Invalid cue-set clipboard")?
    } else {
        vec![clipboard]
    };
    if templates.is_empty() {
        return Err("Clipboard is empty — copy a cue first".into());
    }

    // Preserve decoded media from the source workspace before replacing every
    // ID in each serialized tree with a fresh UUID.
    let originals = templates.clone();
    let mut templates = templates;
    let id_map = remap_serialized_cue_set_ids(&mut templates);
    let mut prepared_json = Vec::with_capacity(templates.len());
    let mut decoded = HashMap::new();
    {
        let ws = state.workspace.lock().map_err(|e| e.to_string())?;
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        for (template, original_template) in templates.into_iter().zip(originals) {
            collect_decoded_audio(cue_list, &original_template, &id_map, &mut decoded);
            prepared_json.push((template, original_template));
        }
    }

    let mut fallbacks = Vec::new();
    let mut prepared = Vec::with_capacity(prepared_json.len());
    {
        let registry = state.registry.lock().map_err(|e| e.to_string())?;
        for (template, original_template) in prepared_json {
            collect_audio_fallbacks(&original_template, &id_map, &decoded, &mut fallbacks);
            let mut cue = registry.from_json(template).map_err(|e| e.to_string())?;
            accept_decoded_audio_recursive(cue.as_mut(), &decoded);
            prepared.push((cue.id(), cue));
        }
    }

    let anchor = after_cue_id
        .map(|value| value.parse::<Uuid>().map_err(|e| e.to_string()))
        .transpose()?;
    push_current_snapshot(&state)?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let pasted_ids = insert_pasted_cues(cue_list, prepared, anchor)?;
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    drop(ws);

    for (new_id, path) in fallbacks {
        spawn_paste_preload(&*state, &app_handle, new_id, path)?;
    }

    clipboard_paste_result(is_set, pasted_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Duration};

    use crate::{
        cue::{
            audio_cue::{AudioCue, AudioCueFactory},
            group_cue::GroupCue,
            traits::{Cue, RuntimeState},
            types::CueState,
            video_cue::{VideoCue, VideoCueFactory},
        },
        show::undo_stack::UndoStack,
    };

    fn registry() -> CueRegistry {
        let mut registry = CueRegistry::new();
        registry.register(crate::cue::types::CueType::Audio, Box::new(AudioCueFactory));
        registry.register(crate::cue::types::CueType::Video, Box::new(VideoCueFactory));
        registry
    }

    #[test]
    fn clipboard_selection_keeps_group_root_without_duplicate_children() {
        let child = AudioCue::new();
        let child_id = child.id();
        let mut group = GroupCue::new();
        let group_id = group.id;
        group.children.push(Box::new(child));
        let sibling = AudioCue::new();
        let sibling_id = sibling.id();
        let cues: Vec<Box<dyn Cue>> = vec![Box::new(group), Box::new(sibling)];
        let selected = HashSet::from([group_id, child_id, sibling_id]);
        let mut copied = Vec::new();

        collect_selected_clipboard_cues(&cues, &selected, false, &mut copied);

        assert_eq!(copied.len(), 2);
        assert_eq!(copied[0]["id"].as_str().unwrap(), group_id.to_string());
        assert_eq!(copied[1]["id"].as_str().unwrap(), sibling_id.to_string());
        assert_eq!(copied[0]["children"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn clipboard_id_remap_updates_hierarchy_and_internal_references() {
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        let mut value = serde_json::json!({
            "id": root.to_string(),
            "target_cue_ids": [child.to_string()],
            "children": [{ "id": child.to_string() }],
        });

        let map = remap_serialized_cue_set_ids(std::slice::from_mut(&mut value));
        let new_root = value["id"].as_str().unwrap();
        let new_child = value["children"][0]["id"].as_str().unwrap();
        assert_ne!(new_root, root.to_string());
        assert_ne!(new_child, child.to_string());
        assert_eq!(value["target_cue_ids"][0].as_str(), Some(new_child));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn clipboard_id_remap_updates_cross_root_references() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut values = vec![
            serde_json::json!({ "id": first.to_string(), "target_cue_id": second.to_string() }),
            serde_json::json!({ "id": second.to_string() }),
        ];
        let map = remap_serialized_cue_set_ids(&mut values);
        let new_target = values[0]["target_cue_id"].as_str().unwrap();
        assert_eq!(new_target, values[1]["id"].as_str().unwrap());
        assert_ne!(new_target, second.to_string());
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn multi_paste_inserts_roots_in_clipboard_order_after_anchor() {
        let anchor = AudioCue::new();
        let anchor_id = anchor.id();
        let first = AudioCue::new();
        let first_id = first.id();
        let second = AudioCue::new();
        let second_id = second.id();
        let mut list = CueList::new("test");
        list.cues.push(Box::new(anchor));

        let pasted = insert_pasted_cues(
            &mut list,
            vec![(first_id, Box::new(first)), (second_id, Box::new(second))],
            Some(anchor_id),
        )
        .unwrap();

        assert_eq!(pasted, vec![first_id.to_string(), second_id.to_string()]);
        assert_eq!(
            list.cues.iter().map(|cue| cue.id()).collect::<Vec<_>>(),
            vec![anchor_id, first_id, second_id],
        );
    }

    #[test]
    fn legacy_single_paste_keeps_string_result_while_sets_return_arrays() {
        assert_eq!(
            clipboard_paste_result(false, vec!["new-one".into()]).unwrap(),
            serde_json::json!("new-one"),
        );
        assert_eq!(
            clipboard_paste_result(true, vec!["new-one".into(), "new-two".into()]).unwrap(),
            serde_json::json!(["new-one", "new-two"]),
        );
    }

    #[test]
    fn nested_audio_and_video_pcm_survive_undo_and_redo() {
        let mut audio = AudioCue::new();
        let audio_id = audio.id();
        audio.accept_preloaded_audio(Arc::new(vec![0.1; 32]), 1, 48_000, Duration::from_millis(1));
        let mut video = VideoCue::new();
        let video_id = video.id();
        video.set_runtime_duration(Duration::from_millis(1));
        video.accept_preloaded_audio(Arc::new(vec![0.2; 64]), 2, 48_000, Duration::from_millis(1));
        let mut group = GroupCue::new();
        group.children.push(Box::new(audio));
        group.children.push(Box::new(video));
        let mut list = CueList::new("test");
        list.cues.push(Box::new(group));

        let mut stack = UndoStack::new();
        stack.push_action(take_snapshot(&list));
        list.get_mut_recursive(&audio_id).unwrap().set_notes("changed".to_string());
        let previous = stack.undo(take_snapshot(&list)).unwrap();
        restore_snapshot(previous, &mut list, &registry()).unwrap();
        assert_eq!(list.get_recursive(&audio_id).unwrap().extract_decoded_audio().unwrap().0.len(), 32);
        assert_eq!(list.get_recursive(&video_id).unwrap().extract_decoded_audio().unwrap().0.len(), 64);

        let next = stack.redo(take_snapshot(&list)).unwrap();
        restore_snapshot(next, &mut list, &registry()).unwrap();
        assert_eq!(list.get_recursive(&audio_id).unwrap().notes(), "changed");
        assert_eq!(list.get_recursive(&audio_id).unwrap().extract_decoded_audio().unwrap().0.len(), 32);
        assert_eq!(list.get_recursive(&video_id).unwrap().extract_decoded_audio().unwrap().0.len(), 64);
    }

    #[test]
    fn transport_guard_descends_into_group_and_checks_loading() {
        let mut child = AudioCue::new();
        let child_id = child.id();
        child.restore_runtime_state(RuntimeState {
            state: CueState::Running,
            voice_id: None,
            started_at: None,
            action_started_at: None,
        });
        let mut group = GroupCue::new();
        group.children.push(Box::new(child));
        let mut list = CueList::new("test");
        list.cues.push(Box::new(group));
        assert!(ensure_transport_safe(&list, &HashSet::new()).is_err());

        list.get_mut_recursive(&child_id).unwrap().restore_runtime_state(RuntimeState {
            state: CueState::Standby,
            voice_id: None,
            started_at: None,
            action_started_at: None,
        });
        assert!(ensure_transport_safe(&list, &HashSet::new()).is_ok());
        assert!(ensure_transport_safe(&list, &HashSet::from([child_id])).is_err());
    }
}
