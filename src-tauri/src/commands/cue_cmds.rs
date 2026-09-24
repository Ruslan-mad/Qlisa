//! Tauri commands for cue CRUD operations.

use std::{
    collections::{HashMap, HashSet},
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};
use uuid::Uuid;

use crate::{
    cue::{
        traits::Cue,
        types::{ContinueMode, CueColor, CueState, CueType, GroupMode},
    },
    engine::media_metadata::{
        MediaMetadata, MediaMetadataCache, MediaMetadataEntry, MediaMetadataFailure, MediaMetadataKey,
        MediaMetadataReservation,
    },
    engine::voice::Voice,
    preferences::GeneralPreferences,
    state::{AppState, PreviewSession},
};

/// Audio decoded by a media cue is runtime state, not part of its JSON.  A
/// rebuild of a Group/Number therefore has to carry every decoded descendant
/// over, not only the root cue.  Otherwise editing Number fade settings can
/// silently unload its audio master and the next GO fails with "audio not
/// loaded".
type PreservedDecodedAudio = (Arc<Vec<f32>>, u16, u32, Duration);

fn collect_decoded_audio_recursive(
    cue: &dyn Cue,
    out: &mut HashMap<Uuid, PreservedDecodedAudio>,
) {
    if let Some(decoded) = cue.extract_decoded_audio() {
        out.insert(cue.id(), decoded);
    }
    if let Some(children) = cue.child_cues() {
        for child in children {
            collect_decoded_audio_recursive(child.as_ref(), out);
        }
    }
}

fn restore_decoded_audio_recursive(
    cue: &mut dyn Cue,
    decoded: &HashMap<Uuid, PreservedDecodedAudio>,
) {
    if let Some((samples, channels, sample_rate, duration)) = decoded.get(&cue.id()) {
        cue.accept_preloaded_audio(samples.clone(), *channels, *sample_rate, *duration);
    }
    if let Some(children) = cue.child_cues_mut() {
        for child in children {
            restore_decoded_audio_recursive(child.as_mut(), decoded);
        }
    }
}

// ---------------------------------------------------------------------------
// DTO types
// ---------------------------------------------------------------------------

/// Compact summary of a cue, used to populate the cue list table in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CueSummary {
    pub id: String,
    pub cue_type: CueType,
    pub name: String,
    pub number: Option<String>,
    /// Free-form notes visible in the Notes column and inspector.
    pub notes: String,
    pub state: CueState,
    pub continue_mode: ContinueMode,
    pub color: CueColor,
    pub pre_wait_ms: u64,
    pub post_wait_ms: u64,
    pub duration_ms: Option<u64>,
    /// Assigned file path for media cues, `None` for other cue types.
    pub file_path: Option<String>,
    /// Resolved cue targets for Stop/Fade/Devamp/Command rows. `None` means
    /// this cue type has no target column content; an empty list means the
    /// command currently has no explicit targets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_cues: Option<Vec<CueTargetSummary>>,
    /// Stop Cue with no explicit targets means “all cues”.
    #[serde(default)]
    pub targets_all: bool,
    /// Memo Cue content shown in the Notes column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo_text: Option<String>,
    /// True while the audio file is being decoded in a background thread.
    pub is_loading: bool,
    /// True when this cue is disabled — skipped by the transport on GO.
    pub is_disabled: bool,
    /// True when this cue's media file was assigned but is now missing from disk.
    pub is_broken: bool,
    /// True specifically when the assigned media path could not be statted.
    /// Kept separate from `is_broken`, which also includes runtime failures.
    pub media_file_missing: bool,
    /// Media analysis is waiting in the bounded queue or actively probing.
    #[serde(default)]
    pub media_metadata_pending: bool,
    /// Non-fatal ffprobe diagnostic. File facts may still be present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_probe_error: Option<String>,
    /// A runtime failure from the currently/most-recently running cue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// True for non-critical problems (no file assigned, zero duration, empty group).
    pub is_warning: bool,
    /// Duration of one loop iteration (file duration without start/end markers applied
    /// and without the loop-count multiplier).  `None` for non-media cues.
    pub file_duration_ms: Option<u64>,
    /// File size cached by a background metadata job. `None` while pending or
    /// for cues without an assigned/readable media file.
    pub file_size_bytes: Option<u64>,
    /// Source pixel dimensions for Video/Image cues. Audio and MIDI File cues
    /// deliberately leave these empty.
    pub media_width: Option<u32>,
    pub media_height: Option<u32>,
    /// Cached ffprobe compatibility assessment.  The status is 0 compatible,
    /// 1 recommended, 2 unsupported; the reason is a stable translation code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_compatibility_status: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_compatibility_reason: Option<u8>,
    /// Human-readable description of the warning condition, when `is_warning` is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_message: Option<String>,
    /// Name of the Output Patch this cue plays through — explicit patch, or
    /// the workspace default for audio-producing cues with none assigned.
    /// `None` for cue types with no audio output (Output column stays empty).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_patch_name: Option<String>,
    /// Persisted audio level for audio-producing Audio/Video/Camera cues.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_db: Option<f64>,
    /// Persisted media loop count. `u32::MAX` means infinite looping. Omitted
    /// for cue types whose playback does not use the media loop setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_count: Option<u32>,
    /// Explicit named video/display destinations. An empty list means a
    /// visual cue uses the configured default destination; `None` means the
    /// cue has no visual output routing at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual_output_ids: Option<Vec<String>>,
    /// For Group cues: their direct children summaries (recursive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<CueSummary>>,
    /// For Group cues: the playback mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_mode: Option<GroupMode>,
    /// For Playlist Group cues: whether the playlist loops (wraps last → first).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playlist_loop: Option<bool>,
    /// For running Sequential Group cues: ID of the currently active child
    /// (running right now or next to fire on GO after a DoNotContinue pause).
    /// `None` for Simultaneous groups and non-Group cues.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_child_id: Option<String>,
    /// Number-only master and timed action metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_master_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_master_type: Option<CueType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_master_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_action_offsets_ms: Option<HashMap<String, u64>>,
    /// Runtime-independent readiness used by Number action rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_action_ready: Option<bool>,
    /// Number-only flow settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_start_stop_mode: Option<crate::cue::types::NumberStartStopMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_start_stop_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_finish_start_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CueTargetSummary {
    pub number: Option<String>,
    pub name: String,
}

/// One member of an atomic multi-cue inspector edit.  The frontend sends one
/// patch per cue rather than one shared JSON object because a semantic field
/// may have different persisted names for different cue types (for example,
/// Image's visual fades and Video's visual fades).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkCueUpdate {
    pub cue_id: String,
    pub properties: serde_json::Value,
}

#[derive(Default)]
struct CommonCuePatch {
    name: Option<String>,
    number: Option<Option<String>>,
    color: Option<CueColor>,
    notes: Option<String>,
    continue_mode: Option<ContinueMode>,
    is_disabled: Option<bool>,
    pre_wait_ms: Option<u64>,
    post_wait_ms: Option<u64>,
}

struct PreparedBulkUpdate {
    id: Uuid,
    common: CommonCuePatch,
    /// Safe live audio/visual fields are applied to the existing cue object.
    in_place: Option<SingleCueInPlacePatch>,
    /// Other type-specific fields require a rebuilt leaf cue. Group cues are
    /// deliberately never rebuilt here: doing so can discard decoded media on
    /// descendants which were not part of the edit.
    rebuilt: Option<Box<dyn Cue>>,
}

/// The safe subset of a single-cue Inspector edit. These setters do not
/// replace the cue object, so they are safe while an Audio/Video/Camera cue
/// owns runtime workers or media resources.
#[derive(Default)]
struct SingleCueDirectPatch {
    common: CommonCuePatch,
}

#[derive(Default)]
struct SingleCueInPlacePatch {
    direct: SingleCueDirectPatch,
    audio: Option<crate::cue::traits::LiveAudioPatch>,
    visual: Option<crate::cue::traits::LiveVisualPatch>,
}

enum SingleCueUpdateKind {
    Direct(SingleCueDirectPatch),
    Rebuild,
}

fn resolved_media_path(
    cue: &dyn Cue,
    workspace_dir: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    if !matches!(
        cue.cue_type(),
        CueType::Audio | CueType::Video | CueType::Image | CueType::MidiFile | CueType::Number
    ) {
        return None;
    }
    let path = cue.media_file_path()?;
    if path.as_os_str().is_empty() {
        return None;
    }
    Some(resolve_stored_media_path(path, workspace_dir))
}

fn resolve_stored_media_path(
    path: &std::path::Path,
    workspace_dir: Option<&std::path::Path>,
) -> std::path::PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(dir) = workspace_dir {
        dir.join(path)
    } else {
        path.to_path_buf()
    }
}

fn validate_video_preview_path(
    cue_type: &CueType,
    stored_path: Option<&std::path::Path>,
    workspace_dir: Option<&std::path::Path>,
) -> Result<std::path::PathBuf, String> {
    if cue_type != &CueType::Video {
        return Err("Media preview source is not a Video Cue".to_string());
    }
    let stored_path = stored_path
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "Video Cue has no media file".to_string())?;
    let resolved = resolve_stored_media_path(stored_path, workspace_dir);
    let canonical = resolved.canonicalize().map_err(|error| {
        format!(
            "Video preview file is unavailable ({}): {error}",
            resolved.display()
        )
    })?;
    if !canonical
        .metadata()
        .map_err(|error| format!("Cannot inspect video preview file: {error}"))?
        .is_file()
    {
        return Err("Video preview source is not a regular file".to_string());
    }
    Ok(canonical)
}

fn invalidate_media_metadata_path(
    state: &AppState,
    path: &std::path::Path,
    workspace_dir: Option<&std::path::Path>,
) {
    let resolved = resolve_stored_media_path(path, workspace_dir);
    if let Ok(mut cache) = state.media_metadata.lock() {
        cache.invalidate_path(&resolved);
    }
}

fn media_metadata_key(
    cue: &dyn Cue,
    workspace_dir: Option<&std::path::Path>,
) -> Option<MediaMetadataKey> {
    let path = resolved_media_path(cue, workspace_dir)?;
    let probe_dimensions = matches!(cue.cue_type(), CueType::Video | CueType::Image);
    Some(MediaMetadataKey::new(path, probe_dimensions))
}

/// Returns a warning message for non-critical problems, or `None` if the cue is healthy.
fn check_warning(cue: &dyn Cue) -> Option<String> {
    match cue.cue_type() {
        CueType::Audio | CueType::Video | CueType::Image | CueType::MidiFile => {
            match cue.media_file_path() {
                None => Some("No file assigned".to_string()),
                Some(p) if p.as_os_str().is_empty() => Some("No file assigned".to_string()),
                _ => None,
            }
        }
        CueType::Wait => {
            if cue.duration() == Some(std::time::Duration::ZERO) {
                Some("Duration is zero".to_string())
            } else {
                None
            }
        }
        CueType::Group => {
            if cue.child_cues().map(|c| c.is_empty()).unwrap_or(false) {
                Some("Group is empty".to_string())
            } else {
                None
            }
        }
        CueType::Number => {
            if cue.number_master_id().is_none() {
                Some("Number master is required".to_string())
            } else if cue.number_master_id().and_then(|master_id| cue.child_cues().and_then(|children| children.iter().find(|child| child.id() == master_id).map(|child| child.cue_type()))) == Some(CueType::Group) {
                cue.duration().is_none().then(|| "Number Group master has no finite duration".to_string())
            } else if cue.media_file_path().is_none() {
                Some("Number master has no file".to_string())
            } else if let Some(duration) = cue.duration() {
                let duration_ms = duration.as_millis() as u64;
                cue.number_action_offsets().iter().any(|(_, offset)| *offset > duration_ms)
                    .then(|| "Number action starts after master ends".to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Resolution table for the Output column: the workspace patch list plus the
/// default patch id, borrowed for the duration of one summary pass.
struct PatchTable<'a> {
    patches: &'a [crate::engine::device_manager::OutputPatch],
    default_id: Option<Uuid>,
}

impl PatchTable<'_> {
    /// The patch name a cue plays through, per the Output-column rules.
    fn name_for(&self, cue: &dyn Cue) -> Option<String> {
        let name_of = |id: Uuid| {
            self.patches
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "(missing)".to_string())
        };
        match cue.output_patch_id() {
            Some(id) => Some(name_of(id)),
            // No explicit patch: audio-producing cues use the default patch.
            None => match cue.cue_type() {
                CueType::Audio | CueType::Video | CueType::Mic => self.default_id.map(name_of),
                _ => None,
            },
        }
    }
}

#[derive(Default)]
struct CueTargetIndex {
    by_id: HashMap<String, CueTargetSummary>,
    by_number: HashMap<String, CueTargetSummary>,
}

fn index_cue_targets(cues: &[Box<dyn Cue>], index: &mut CueTargetIndex) {
    for cue in cues {
        let target = CueTargetSummary {
            number: cue.number().map(str::to_string),
            name: cue.name().to_string(),
        };
        index.by_id.insert(cue.id().to_string(), target.clone());
        if let Some(number) = target.number.as_deref() {
            index
                .by_number
                .entry(number.to_string())
                .or_insert_with(|| target.clone());
        }
        if let Some(children) = cue.child_cues() {
            index_cue_targets(children, index);
        }
    }
}

fn cue_target_ids(serialized: &serde_json::Value) -> Vec<String> {
    let mut ids: Vec<String> = serialized
        .get("target_cue_ids")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        if let Some(id) = serialized
            .get("target_cue_id")
            .and_then(serde_json::Value::as_str)
        {
            ids.push(id.to_string());
        }
    }
    ids
}

fn cue_target_numbers(serialized: &serde_json::Value) -> Vec<String> {
    let mut numbers: Vec<String> = serialized
        .get("target_cue_numbers")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_string)
        .collect();
    if numbers.is_empty() {
        if let Some(number) = serialized
            .get("target_cue_number")
            .and_then(serde_json::Value::as_str)
        {
            numbers.push(number.to_string());
        }
    }
    numbers
}

/// Return the named destinations authored on a visual cue. The serialized
/// representation is the common contract for Video, Image, Browser, Camera
/// and Text cues, while audio and control cues deliberately return `None`.
/// An empty vector is meaningful: it says the visual cue follows the default
/// destination.
fn visual_output_ids_for_cue(cue: &dyn Cue) -> Option<Vec<String>> {
    if !matches!(
        cue.cue_type(),
        CueType::Video | CueType::Image | CueType::Browser | CueType::Camera | CueType::Text
    ) {
        return None;
    }
    let serialized = cue.serialize();
    let mut ids = serialized
        .get("output_ids")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        if let Some(id) = serialized
            .get("output_id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.trim().is_empty())
        {
            ids.push(id.to_owned());
        }
    }
    Some(ids)
}

fn target_display_for_cue(
    cue: &dyn Cue,
    index: &CueTargetIndex,
) -> (Option<Vec<CueTargetSummary>>, bool) {
    let cue_type = cue.cue_type();
    if targeted_cue_label(&cue_type).is_none() {
        return (None, false);
    }

    let serialized = cue.serialize();
    let ids = cue_target_ids(&serialized);
    let numbers = cue_target_numbers(&serialized);
    let mut targets = Vec::new();
    let mut seen = HashSet::new();

    if !ids.is_empty() {
        for (position, id) in ids.iter().enumerate() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(target) = index.by_id.get(id) {
                let mut target = target.clone();
                if target.number.is_none() {
                    target.number = numbers.get(position).cloned();
                }
                targets.push(target);
            } else if let Some(number) = numbers.get(position) {
                targets.push(CueTargetSummary {
                    number: Some(number.clone()),
                    name: String::new(),
                });
            }
        }
    } else {
        for number in &numbers {
            if !seen.insert(number.clone()) {
                continue;
            }
            targets.push(index.by_number.get(number).cloned().unwrap_or_else(|| {
                CueTargetSummary {
                    number: Some(number.clone()),
                    name: String::new(),
                }
            }));
        }
    }

    let targets_all = cue_type == CueType::Stop && ids.is_empty() && numbers.is_empty();
    (Some(targets), targets_all)
}

fn summarise(
    cue: &dyn Cue,
    workspace_dir: Option<&std::path::Path>,
    patches: &PatchTable,
    metadata: &HashMap<MediaMetadataKey, MediaMetadataEntry>,
    target_index: &CueTargetIndex,
    metadata_requests: &mut HashSet<MediaMetadataKey>,
    loading: &HashSet<Uuid>,
) -> CueSummary {
    let warning_message = check_warning(cue);
    let error_message = cue.runtime_error().map(str::to_string);
    let runtime_warning = cue.runtime_warning().map(str::to_string);
    let warning_message = runtime_warning.or(warning_message);
    let metadata_entry = media_metadata_key(cue, workspace_dir).and_then(|key| {
        let entry = metadata.get(&key).cloned();
        // The cache decides whether the key is new or TTL-stale. Requesting
        // every referenced key keeps external replacements/deletions visible
        // without doing filesystem I/O in this summary path.
        metadata_requests.insert(key);
        entry
    });
    let media_metadata = metadata_entry.as_ref().and_then(|entry| match entry {
        MediaMetadataEntry::Ready(value) => Some(*value),
        MediaMetadataEntry::Queued | MediaMetadataEntry::Pending | MediaMetadataEntry::Failed(_) => None,
    });
    let metadata_failure = metadata_entry.as_ref().and_then(|entry| match entry {
        MediaMetadataEntry::Failed(error) => Some(error),
        _ => None,
    });
    let metadata_pending = matches!(
        metadata_entry.as_ref(),
        Some(MediaMetadataEntry::Queued | MediaMetadataEntry::Pending)
    );
    let metadata_error = metadata_failure.map(MediaMetadataFailure::user_message);
    let metadata_file_missing = metadata_failure.is_some_and(MediaMetadataFailure::is_missing_file);
    let probe_error = media_metadata
        .and_then(|value| value.probe_failure)
        .map(|failure| failure.user_message());
    let warning_message = warning_message.or_else(|| probe_error.map(str::to_string));
    let (target_cues, targets_all) = target_display_for_cue(cue, target_index);
    CueSummary {
        output_patch_name: patches.name_for(cue),
        volume_db: matches!(cue.cue_type(), CueType::Audio | CueType::Video | CueType::Camera)
            .then(|| cue.serialize().get("volume_db").and_then(serde_json::Value::as_f64))
            .flatten(),
        loop_count: matches!(cue.cue_type(), CueType::Audio | CueType::Video)
            .then(|| cue.serialize().get("loop_count").and_then(serde_json::Value::as_u64).map(|value| value as u32))
            .flatten(),
        visual_output_ids: visual_output_ids_for_cue(cue),
        id: cue.id().to_string(),
        cue_type: cue.cue_type(),
        name: cue.name().to_string(),
        number: cue.number().map(|s| s.to_string()),
        notes: cue.notes().to_string(),
        state: cue.state(),
        continue_mode: cue.continue_mode(),
        color: cue.color(),
        pre_wait_ms: cue.pre_wait().as_millis() as u64,
        post_wait_ms: cue.post_wait().as_millis() as u64,
        duration_ms: cue.duration().map(|d| d.as_millis() as u64),
        file_path: cue
            .media_file_path()
            .map(|p| p.to_string_lossy().into_owned()),
        target_cues,
        targets_all,
        memo_text: cue.memo_text().map(str::to_string),
        is_loading: loading.contains(&cue.id()),
        is_disabled: cue.is_disabled(),
        is_broken: metadata_failure.is_some() || error_message.is_some(),
        media_file_missing: metadata_file_missing,
        media_metadata_pending: metadata_pending,
        media_probe_error: probe_error.map(str::to_string),
        error_message: error_message.or_else(|| metadata_error.map(str::to_string)),
        is_warning: warning_message.is_some(),
        warning_message,
        file_duration_ms: cue.file_duration().map(|d| d.as_millis() as u64),
        file_size_bytes: media_metadata.map(|value| value.file_size_bytes),
        media_width: media_metadata.and_then(|value| value.width),
        media_height: media_metadata.and_then(|value| value.height),
        media_compatibility_status: media_metadata.map(|value| value.compatibility_status),
        media_compatibility_reason: media_metadata.and_then(|value| value.compatibility_reason),
        children: cue.child_cues().map(|ch| {
            ch.iter()
                .map(|c| {
                    summarise(
                        c.as_ref(),
                        workspace_dir,
                        patches,
                        metadata,
                        target_index,
                        metadata_requests,
                        loading,
                    )
                })
                .collect()
        }),
        group_mode: cue.group_mode(),
        playlist_loop: cue.playlist_loop(),
        active_child_id: cue.active_child_id().map(|id| id.to_string()),
        number_master_id: cue.number_master_id().map(|id| id.to_string()),
        number_master_type: cue.number_master_id().and_then(|id| cue.child_cues().and_then(|children| children.iter().find(|child| child.id() == id).map(|child| child.cue_type()))),
        number_master_name: cue.number_master_id().and_then(|id| cue.child_cues().and_then(|children| children.iter().find(|child| child.id() == id).map(|child| child.name().to_string()))),
        number_action_offsets_ms: {
            let offsets = cue.number_action_offsets();
            (!offsets.is_empty()).then(|| offsets.into_iter().map(|(id, ms)| (id.to_string(), ms)).collect())
        },
        number_action_ready: match cue.cue_type() {
            // `summarise` must remain filesystem-free. `media_file_missing`
            // comes from the already-computed metadata cache above.
            CueType::Audio | CueType::Video | CueType::Image => Some(cue.media_file_path().map(|path| !path.as_os_str().is_empty()).unwrap_or(false) && !metadata_file_missing),
            _ => None,
        },
        number_start_stop_mode: cue.number_start_stop_specification().map(|(mode, _)| mode),
        number_start_stop_ids: cue.number_start_stop_specification().and_then(|(_, ids)| {
            (!ids.is_empty()).then(|| ids.into_iter().map(|id| id.to_string()).collect())
        }),
        number_finish_start_ids: {
            let ids = cue.number_finish_start_ids();
            (!ids.is_empty()).then(|| ids.into_iter().map(|id| id.to_string()).collect())
        },
    }
}

fn spawn_media_metadata_batch(
    jobs: Vec<MediaMetadataReservation>,
    cache: Arc<std::sync::Mutex<MediaMetadataCache>>,
    mpv_lib: Option<Arc<crate::engine::mpv_sys::MpvLib>>,
    app_handle: tauri::AppHandle,
) {
    if jobs.is_empty() {
        return;
    }
    std::thread::Builder::new()
        .name("inkue-media-metadata-coord".into())
        .spawn(move || {
            let job_count = jobs.len();
            // ffprobe can briefly seek/read heavily, and starting several at
            // preset-open gives no useful speed benefit to the operator. Keep
            // one explicit FIFO worker: a Cue is Queued until its turn and
            // then Pending while its own bounded probe is active.
            let workers = 1usize;
            let (tx, rx) = crossbeam_channel::unbounded::<MediaMetadataReservation>();
            for job in jobs {
                let _ = tx.send(job);
            }
            drop(tx);
            let (done_tx, done_rx) = crossbeam_channel::unbounded::<bool>();

            let mut handles = Vec::with_capacity(workers);
            for worker_index in 0..workers {
                let rx = rx.clone();
                let cache = Arc::clone(&cache);
                let mpv_lib = mpv_lib.clone();
                let done_tx = done_tx.clone();
                let name = format!("inkue-media-metadata-{worker_index}");
                if let Ok(handle) = std::thread::Builder::new().name(name).spawn(move || {
                    while let Ok(job) = rx.recv() {
                        if let Ok(mut cache) = cache.lock() {
                            cache.mark_processing(&job);
                        }
                        let result = probe_media_metadata_file(&job, mpv_lib.as_deref());
                        let committed = cache
                            .lock()
                            .map(|mut cache| cache.finish(job, result))
                            .unwrap_or(false);
                        let _ = done_tx.send(committed);
                    }
                }) {
                    handles.push(handle);
                }
            }
            if handles.is_empty() {
                // Extremely rare OS thread-creation failure: finish the jobs
                // on this already-background coordinator rather than leaving
                // their cache entries Pending forever.
                let mut committed_any = false;
                while let Ok(job) = rx.try_recv() {
                    if let Ok(mut cache) = cache.lock() {
                        cache.mark_processing(&job);
                    }
                    let result = probe_media_metadata_file(&job, mpv_lib.as_deref());
                    if let Ok(mut cache) = cache.lock() {
                        committed_any |= cache.finish(job, result);
                    }
                }
                if committed_any {
                    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
                }
                return;
            }
            drop(done_tx);

            // Publish completed metadata incrementally without turning a
            // large playlist into an event storm. The first result is visible
            // immediately, subsequent refreshes are coalesced to at most 10Hz,
            // and the final completion always emits even inside that window.
            let interval = Duration::from_millis(100);
            let mut completed = 0usize;
            let mut dirty = false;
            let mut last_emit = Instant::now() - interval;
            while completed < job_count {
                match done_rx.recv_timeout(interval) {
                    Ok(committed) => {
                        completed += 1;
                        dirty |= committed;
                        while let Ok(committed) = done_rx.try_recv() {
                            completed += 1;
                            dirty |= committed;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
                let now = Instant::now();
                if dirty && (completed == job_count || now.duration_since(last_emit) >= interval) {
                    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
                    last_emit = now;
                    dirty = false;
                }
            }
            for handle in handles {
                let _ = handle.join();
            }
            if dirty {
                let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
            }
        })
        .ok();
}

fn can_reuse_cached_media_metadata(
    previous: MediaMetadata,
    file_size_bytes: u64,
    modified_at: Option<std::time::SystemTime>,
    probe_dimensions: bool,
) -> bool {
    previous.file_size_bytes == file_size_bytes
        && modified_at.is_some()
        && previous.modified_at == modified_at
        && (!probe_dimensions || (previous.width.is_some() && previous.height.is_some()))
}

fn probe_media_metadata_file(
    job: &MediaMetadataReservation,
    mpv_lib: Option<&crate::engine::mpv_sys::MpvLib>,
) -> Result<MediaMetadata, MediaMetadataFailure> {
    let file = std::fs::metadata(&job.key.path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            MediaMetadataFailure::MissingFile
        } else {
            MediaMetadataFailure::Unreadable
        }
    })?;
    if !file.is_file() {
        return Err(MediaMetadataFailure::NotAFile);
    }
    let modified_at = file.modified().ok();
    let unchanged = job.previous.is_some_and(|previous| {
        can_reuse_cached_media_metadata(
            previous,
            file.len(),
            modified_at,
            job.key.probe_dimensions,
        )
    });
    let probe = if job.key.probe_dimensions && !unchanged {
        mpv_lib.and_then(|lib| crate::engine::OutputEngine::probe_media_info(lib, &job.key.path))
    } else {
        None
    };
    let is_audio_or_video = matches!(
        job.key.path.extension().and_then(|extension| extension.to_str()).map(|extension| extension.to_ascii_lowercase()).as_deref(),
        Some("mp3" | "aac" | "flac" | "ogg" | "wav" | "aiff" | "aif" | "mp4" | "mov" | "m4v" | "mkv" | "webm" | "avi" | "mxf" | "mts" | "m2ts")
    );
    let (compatibility, probe_failure) = if !is_audio_or_video {
        (None, None)
    } else if unchanged {
        (
            job.previous.map(|value| (value.compatibility_status, value.compatibility_reason)),
            job.previous.and_then(|value| value.probe_failure),
        )
    } else {
        match crate::media_converter::probe_media_path(&job.key.path) {
            Ok(value) => (
                Some(crate::media_converter::compatibility_cache_codes(&value)),
                None,
            ),
            Err(error) => (None, Some(classify_media_probe_failure(&error))),
        }
    };
    Ok(MediaMetadata {
        file_size_bytes: file.len(),
        width: if unchanged {
            job.previous.and_then(|value| value.width)
        } else {
            probe.and_then(|value| value.width)
        },
        height: if unchanged {
            job.previous.and_then(|value| value.height)
        } else {
            probe.and_then(|value| value.height)
        },
        compatibility_status: compatibility.map(|value| value.0).unwrap_or(0),
        compatibility_reason: compatibility.and_then(|value| value.1),
        probe_failure,
        modified_at,
    })
}

fn classify_media_probe_failure(error: &str) -> crate::engine::media_metadata::MediaProbeFailure {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("timed out") {
        crate::engine::media_metadata::MediaProbeFailure::TimedOut
    } else if normalized.contains("runtime was not found")
        || normalized.contains("could not start ffprobe")
    {
        crate::engine::media_metadata::MediaProbeFailure::RuntimeUnavailable
    } else {
        crate::engine::media_metadata::MediaProbeFailure::Failed
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Return a compact summary of every cue in the active cue list.
#[tauri::command]
pub fn get_all_cues(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<Vec<CueSummary>, String> {
    let metadata = state
        .media_metadata
        .lock()
        .map_err(|e| e.to_string())?
        .snapshot();
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let loading = state.loading_cues.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
    let ws_dir = ws
        .file_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_owned());
    let ws_dir_ref = ws_dir.as_deref();
    let patch_table = PatchTable {
        patches: &ws.output_patches,
        default_id: ws.default_output_patch_id,
    };
    let mut target_index = CueTargetIndex::default();
    index_cue_targets(&cue_list.cues, &mut target_index);

    let mut metadata_requests = HashSet::new();
    let summaries: Vec<CueSummary> = cue_list
        .cues
        .iter()
        .map(|c| {
            summarise(
                c.as_ref(),
                ws_dir_ref,
                &patch_table,
                &metadata,
                &target_index,
                &mut metadata_requests,
                &loading,
            )
        })
        .collect();

    drop(loading);
    drop(ws);
    let jobs = state
        .media_metadata
        .lock()
        .map_err(|e| e.to_string())?
        .reserve_missing(metadata_requests);
    // The initial summary was built before newly discovered files entered the
    // queue. Publish once so the UI can show an analysis indicator right away.
    if !jobs.is_empty() {
        let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    }
    spawn_media_metadata_batch(
        jobs,
        Arc::clone(&state.media_metadata),
        state.output_engine.try_mpv_lib_arc(),
        app_handle,
    );

    Ok(summaries)
}

/// Return the full serialised JSON for a single cue.
#[tauri::command]
pub fn get_cue(cue_id: String, state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
    let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
    Ok(cue.serialize())
}

/// Return complete serialised data for several cues, preserving the requested
/// order.  One workspace lock makes this a consistent inspector snapshot; it
/// also works for leaves nested inside Group cues.
#[tauri::command]
pub fn get_cues(
    cue_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let ids: Vec<Uuid> = cue_ids
        .iter()
        .map(|id| id.parse::<Uuid>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
    let loading = state.loading_cues.lock().map_err(|e| e.to_string())?;
    ids.iter()
        .map(|id| {
            let cue = cue_list
                .get_recursive(id)
                .ok_or_else(|| "Cue not found".to_string())?;
            Ok(with_bulk_edit_metadata(
                cue.serialize(),
                is_fade_rebuild_editable(cue, loading.contains(id)),
            ))
        })
        .collect()
}

fn is_common_bulk_key(key: &str) -> bool {
    matches!(
        key,
        "name"
            | "number"
            | "color"
            | "notes"
            | "continue_mode"
            | "is_disabled"
            | "pre_wait_ms"
            | "post_wait_ms"
    )
}

fn is_protected_single_update_key(key: &str) -> bool {
    matches!(
        key,
        "id" | "type" | "cue_type" | "children" | "duration_ms" | "_bulk_edit"
    )
}

fn validate_common_bulk_value(
    key: &str,
    value: &serde_json::Value,
    out: &mut CommonCuePatch,
) -> Result<(), String> {
    match key {
        "name" => {
            out.name = Some(value.as_str().ok_or("name must be a string")?.to_string());
        }
        "number" => {
            out.number = Some(if value.is_null() {
                None
            } else {
                Some(
                    value
                        .as_str()
                        .ok_or("number must be a string or null")?
                        .to_string(),
                )
            });
        }
        "color" => {
            out.color = Some(
                serde_json::from_value(value.clone())
                    .map_err(|_| "color must be a valid cue colour".to_string())?,
            );
        }
        "notes" => {
            out.notes = Some(value.as_str().ok_or("notes must be a string")?.to_string());
        }
        "continue_mode" => {
            out.continue_mode = Some(
                serde_json::from_value(value.clone())
                    .map_err(|_| "continue_mode must be a valid continue mode".to_string())?,
            );
        }
        "is_disabled" => {
            out.is_disabled = Some(value.as_bool().ok_or("is_disabled must be a boolean")?);
        }
        "pre_wait_ms" => {
            out.pre_wait_ms = Some(
                value
                    .as_u64()
                    .ok_or("pre_wait_ms must be a non-negative integer")?,
            );
        }
        "post_wait_ms" => {
            out.post_wait_ms = Some(
                value
                    .as_u64()
                    .ok_or("post_wait_ms must be a non-negative integer")?,
            );
        }
        _ => unreachable!("caller checks common bulk keys"),
    }
    Ok(())
}

fn is_audio_fade_key(key: &str) -> bool {
    matches!(
        key,
        "fade_in_ms" | "fade_in_curve" | "fade_out_ms" | "fade_out_curve"
    )
}

fn is_visual_fade_key(key: &str) -> bool {
    matches!(
        key,
        "video_fade_in_ms" | "video_fade_in_curve" | "video_fade_out_ms" | "video_fade_out_curve"
    )
}

/// Rebuild fields exposed by the multi-cue inspector. Keep this explicit:
/// `Cue::serialize` also contains derived/cache data which must remain
/// read-only even though a factory happens to accept it on workspace load.
fn rebuild_key_allowed_for_type(key: &str, cue_type: CueType) -> bool {
    match cue_type {
        CueType::Audio => {
            is_audio_fade_key(key)
                || matches!(
                    key,
                    "start_time_ms" | "end_time_ms" | "loop_count" | "rate" | "output_patch_id"
                )
        }
        CueType::Video => {
            is_audio_fade_key(key)
                || is_visual_fade_key(key)
                || matches!(
                    key,
                    "start_time_ms"
                        | "end_time_ms"
                        | "loop_count"
                        | "hold_last_frame"
                        | "output_id"
                        | "output_ids"
                        | "output_patch_id"
                )
        }
        // Image uses the unprefixed fade keys for its visual fade.
        CueType::Image => {
            is_audio_fade_key(key)
                || matches!(key, "display_duration_ms" | "output_id" | "output_ids")
        }
        CueType::Camera => is_visual_fade_key(key) || matches!(key, "output_id" | "output_ids"),
        CueType::Browser => matches!(key, "url" | "output_id" | "output_ids" | "reload_on_go" | "zoom"),
        CueType::Text => matches!(key, "display_duration_ms" | "output_id" | "output_ids"),
        CueType::Wait => key == "wait_duration_ms",
        CueType::Fade => key == "fade_duration_ms",
        // Number uses the regular audio fade field names as a shared
        // envelope for all of its active audio/video children.
        CueType::Number => {
            is_audio_fade_key(key)
                || matches!(key, "number_start_stop_mode" | "number_start_stop_ids" | "number_finish_start_ids")
        }
        _ => false,
    }
}

fn validate_nullable_u64(key: &str, value: &serde_json::Value) -> Result<(), String> {
    if value.is_null() || value.as_u64().is_some() {
        Ok(())
    } else {
        Err(format!("{key} must be a non-negative integer or null"))
    }
}

fn validate_rebuild_bulk_value(key: &str, value: &serde_json::Value) -> Result<(), String> {
    match key {
        "fade_in_ms"
        | "fade_out_ms"
        | "video_fade_in_ms"
        | "video_fade_out_ms"
        | "start_time_ms"
        | "end_time_ms"
        | "display_duration_ms" => validate_nullable_u64(key, value),
        "fade_in_curve" | "fade_out_curve" | "video_fade_in_curve" | "video_fade_out_curve" => {
            if value.is_null()
                || serde_json::from_value::<crate::cue::types::FadeCurve>(value.clone()).is_ok()
            {
                Ok(())
            } else {
                Err(format!("{key} must be a valid fade curve or null"))
            }
        }
        "number_start_stop_mode" => serde_json::from_value::<crate::cue::types::NumberStartStopMode>(value.clone())
            .map(|_| ())
            .map_err(|_| "number_start_stop_mode must be a valid Number stop mode".to_string()),
        "number_start_stop_ids" | "number_finish_start_ids" => {
            let ids = value.as_array().ok_or_else(|| format!("{key} must be an array of cue IDs"))?;
            for id in ids {
                id.as_str().ok_or_else(|| format!("{key} must contain cue ID strings"))?
                    .parse::<Uuid>().map_err(|_| format!("{key} must contain valid cue IDs"))?;
            }
            Ok(())
        }
        "wait_duration_ms" | "fade_duration_ms" => value
            .as_u64()
            .map(|_| ())
            .ok_or_else(|| format!("{key} must be a non-negative integer")),
        "loop_count" => match value.as_u64() {
            Some(count) if count <= u32::MAX as u64 => Ok(()),
            _ => Err("loop_count must be an integer between 0 and 4294967295".to_string()),
        },
        "rate" => match value.as_f64() {
            Some(rate) if rate.is_finite() && (0.1..=4.0).contains(&rate) => Ok(()),
            _ => Err("rate must be between 0.1 and 4".to_string()),
        },
        "hold_last_frame" => value
            .as_bool()
            .map(|_| ())
            .ok_or_else(|| "hold_last_frame must be a boolean".to_string()),
        "output_patch_id" => {
            if value.is_null() {
                return Ok(());
            }
            let raw = value
                .as_str()
                .ok_or("output_patch_id must be a UUID string or null")?;
            raw.parse::<Uuid>()
                .map(|_| ())
                .map_err(|_| "output_patch_id must be a UUID string or null".to_string())
        }
        "output_id" => {
            if value.is_null() || value.as_str().is_some_and(|id| !id.is_empty()) {
                Ok(())
            } else {
                Err("output_id must be a non-empty string or null".to_string())
            }
        }
        "output_ids" => {
            let ids = value
                .as_array()
                .ok_or("output_ids must be an array of non-empty strings")?;
            let mut unique = HashSet::with_capacity(ids.len());
            for item in ids {
                let id = item
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or("output_ids must be an array of non-empty strings")?;
                if !unique.insert(id) {
                    return Err("output_ids must not contain duplicates".to_string());
                }
            }
            Ok(())
        }
        "url" => {
            let raw = value.as_str().ok_or("url must be an http or https URL")?;
            crate::engine::output_engine::validate_browser_url(raw)
                .map(|_| ())
                .map_err(|_| "url must be an http or https URL".to_string())
        }
        "reload_on_go" => value
            .as_bool()
            .map(|_| ())
            .ok_or_else(|| "reload_on_go must be a boolean".to_string()),
        "zoom" => match value.as_f64() {
            Some(zoom) if zoom.is_finite() && (0.25..=3.0).contains(&zoom) => Ok(()),
            _ => Err("zoom must be between 0.25 and 3".to_string()),
        },
        _ => unreachable!("caller checks the rebuild-field allowlist"),
    }
}

fn validate_visual_output_pair(
    properties: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let has_id = properties.contains_key("output_id");
    let has_ids = properties.contains_key("output_ids");
    if !has_id && !has_ids {
        return Ok(());
    }
    if !has_id || !has_ids {
        return Err("output_id and output_ids must be updated together".to_string());
    }

    let primary = properties["output_id"].as_str();
    let ids = properties["output_ids"]
        .as_array()
        .ok_or("output_ids must be an array of non-empty strings")?;
    let first = ids.first().and_then(|value| value.as_str());
    match (
        primary,
        first,
        ids.is_empty(),
        properties["output_id"].is_null(),
    ) {
        (Some(primary), Some(first), false, false) if primary == first => Ok(()),
        (None, None, true, true) => Ok(()),
        _ => Err(
            "output_id must equal the first output_ids entry, or both must be empty/default"
                .to_string(),
        ),
    }
}

fn apply_common_bulk_patch(cue: &mut dyn Cue, patch: CommonCuePatch) {
    if let Some(name) = patch.name {
        cue.set_name(name);
    }
    if let Some(number) = patch.number {
        cue.set_number(number);
    }
    if let Some(color) = patch.color {
        cue.set_color(color);
    }
    if let Some(notes) = patch.notes {
        cue.set_notes(notes);
    }
    if let Some(mode) = patch.continue_mode {
        cue.set_continue_mode(mode);
    }
    if let Some(disabled) = patch.is_disabled {
        cue.set_disabled(disabled);
    }
    if let Some(ms) = patch.pre_wait_ms {
        cue.set_pre_wait(Duration::from_millis(ms));
    }
    if let Some(ms) = patch.post_wait_ms {
        cue.set_post_wait(Duration::from_millis(ms));
    }
}

fn apply_single_direct_patch(cue: &mut dyn Cue, patch: SingleCueDirectPatch) {
    apply_common_bulk_patch(cue, patch.common);
}

fn apply_single_in_place_patch(cue: &mut dyn Cue, patch: SingleCueInPlacePatch) {
    apply_single_direct_patch(cue, patch.direct);
    if let Some(audio) = patch.audio {
        cue.apply_live_audio_patch(audio);
    }
    if let Some(visual) = patch.visual {
        cue.apply_live_visual_patch(visual);
    }
}

/// Validate protected and direct-field values before an update can create an
/// Undo entry. A mixture of direct and cue-specific fields still uses the
/// legacy serialise/rebuild path, but the direct values are validated here.
fn classify_single_cue_update(
    properties: &serde_json::Value,
) -> Result<SingleCueUpdateKind, String> {
    let properties = properties
        .as_object()
        .ok_or("properties must be an object")?;
    if properties.is_empty() {
        return Err("properties must not be empty".to_string());
    }

    let mut direct = SingleCueDirectPatch::default();
    let mut all_direct = true;
    for (key, value) in properties {
        if is_protected_single_update_key(key) {
            return Err(format!("{key} is protected and cannot be updated"));
        }
        match key.as_str() {
            key if is_common_bulk_key(key) => {
                validate_common_bulk_value(key, value, &mut direct.common)?;
            }
            _ => all_direct = false,
        }
    }

    Ok(if all_direct {
        SingleCueUpdateKind::Direct(direct)
    } else {
        SingleCueUpdateKind::Rebuild
    })
}

fn validated_live_db(value: &serde_json::Value) -> Result<f64, String> {
    let db = value.as_f64().ok_or("volume_db must be a number")?;
    if !db.is_finite() || !(-60.0..=12.0).contains(&db) {
        return Err("volume_db must be between -60 and 12 dB".to_string());
    }
    Ok(db)
}

fn validated_live_pan(value: &serde_json::Value) -> Result<f32, String> {
    let pan = value.as_f64().ok_or("pan must be a number")?;
    if !pan.is_finite() || !(-1.0..=1.0).contains(&pan) {
        return Err("pan must be between -1 and 1".to_string());
    }
    Ok(pan as f32)
}

/// Return an in-place patch only when every key is known-safe for the target
/// cue type. A mixed safe/unsafe patch deliberately falls back to the guarded
/// rebuild route, rather than applying only part of an operator's request.
fn classify_live_in_place_patch(
    properties: &serde_json::Value,
    cue_type: CueType,
) -> Result<Option<SingleCueInPlacePatch>, String> {
    let properties = properties
        .as_object()
        .ok_or("properties must be an object")?;
    let mut patch = SingleCueInPlacePatch::default();
    let mut has_live_field = false;

    for (key, value) in properties {
        if is_protected_single_update_key(key) {
            return Err(format!("{key} is protected and cannot be updated"));
        }
        match key.as_str() {
            key if is_common_bulk_key(key) => {
                validate_common_bulk_value(key, value, &mut patch.direct.common)?
            }
            "volume_db" if matches!(cue_type, CueType::Audio | CueType::Video | CueType::Mic | CueType::Camera) => {
                patch.audio.get_or_insert_with(Default::default).volume_db =
                    Some(validated_live_db(value)?);
                has_live_field = true;
            }
            "pan" if matches!(cue_type, CueType::Audio | CueType::Mic | CueType::Camera) => {
                patch.audio.get_or_insert_with(Default::default).pan =
                    Some(validated_live_pan(value)?);
                has_live_field = true;
            }
            "level_matrix" if matches!(cue_type, CueType::Audio | CueType::Video | CueType::Camera) => {
                let matrix: Option<Vec<Vec<f64>>> = serde_json::from_value(value.clone())
                    .map_err(|_| "level_matrix must be a matrix of numbers or null".to_string())?;
                if matrix
                    .as_ref()
                    .is_some_and(|rows| rows.iter().flatten().any(|entry| !entry.is_finite()))
                {
                    return Err("level_matrix must contain finite numbers".to_string());
                }
                patch
                    .audio
                    .get_or_insert_with(Default::default)
                    .level_matrix = Some(matrix);
                has_live_field = true;
            }
            "geometry" if matches!(cue_type, CueType::Video | CueType::Image | CueType::Camera) => {
                patch.visual.get_or_insert_with(Default::default).geometry = Some(
                    serde_json::from_value(value.clone())
                        .map_err(|_| "geometry must be valid".to_string())?,
                );
                has_live_field = true;
            }
            "layer_style"
                if matches!(cue_type, CueType::Video | CueType::Image | CueType::Camera) =>
            {
                patch
                    .visual
                    .get_or_insert_with(Default::default)
                    .layer_style = Some(
                    serde_json::from_value(value.clone())
                        .map_err(|_| "layer_style must be valid".to_string())?,
                );
                has_live_field = true;
            }
            _ => return Ok(None),
        }
    }

    Ok(has_live_field.then_some(patch))
}

fn reject_group_rebuild(cue: &dyn Cue, properties: &serde_json::Value) -> Result<(), String> {
    if cue.cue_type() != CueType::Group {
        return Ok(());
    }
    let properties = properties.as_object().expect("classified object patch");
    if properties.contains_key("group_mode") || properties.contains_key("playlist_loop") {
        return Err(
            "Update Group mode with set_group_mode or set_playlist_loop, not update_cue"
                .to_string(),
        );
    }
    Err(
        "Group cue properties cannot be rebuilt with update_cue; update common fields directly"
            .to_string(),
    )
}

fn apply_existing_cue_live_side_effects(
    cue: &dyn Cue,
    state: &AppState,
    patch_channels_by_id: &[(Uuid, Vec<u16>)],
    default_patch_id: Option<Uuid>,
) {
    if let Some(params) = cue.live_audio_params() {
        let audio_voice = state
            .output_engine
            .video_audio_voice(params.voice_id)
            .unwrap_or(params.voice_id);
        let _ = state.audio_engine.set_voice_gain(audio_voice, params.gain);
        let _ = state.audio_engine.set_voice_pan(audio_voice, params.pan);
        let patch_channels = cue
            .output_patch_id()
            .or(default_patch_id)
            .and_then(|id| {
                patch_channels_by_id
                    .iter()
                    .find(|(patch_id, _)| *patch_id == id)
            })
            .map(|(_, channels)| channels.clone())
            .unwrap_or_default();
        let matrix = params
            .level_matrix
            .as_ref()
            .and_then(|spec| crate::cue::audio_cue::build_level_matrix(spec, &patch_channels));
        let _ = state
            .audio_engine
            .set_voice_level_matrix(audio_voice, matrix.as_ref());
    }
    if let Some(voice_id) = cue
        .playing_voice_id()
        .filter(|voice_id| state.output_engine.is_current_voice(*voice_id))
    {
        if let Some(geometry) = cue.visual_geometry() {
            state.output_engine.apply_geometry(voice_id, &geometry);
        }
        if let Some(layer_style) = cue.layer_style() {
            state.output_engine.set_layer_props(voice_id, &layer_style);
        }
    }
}

/// Rebuilding a cue while it owns an engine resource can orphan that resource
/// or corrupt timing. Common inspector fields use direct trait setters and are
/// safe live; fade fields rebuild the cue and therefore require a quiescent
/// leaf. `loading_cues` covers background media decode, while the cue hook
/// covers Video/Image Load Cue resources whose state still reads Standby.
fn is_fade_rebuild_editable_state(
    state: CueState,
    is_preloaded_or_loading: bool,
    is_loading: bool,
) -> bool {
    !is_loading
        && !is_preloaded_or_loading
        && matches!(state, CueState::Standby | CueState::Completed)
}

fn is_fade_rebuild_editable(cue: &dyn Cue, is_loading: bool) -> bool {
    is_fade_rebuild_editable_state(cue.state(), cue.is_preloaded_or_loading(), is_loading)
}

/// Add inspector-only metadata to a read payload. This is deliberately added
/// after `Cue::serialize`, so it cannot be persisted in a project or accepted
/// as an editable cue property.
fn with_bulk_edit_metadata(
    mut payload: serde_json::Value,
    rebuild_editable: bool,
) -> serde_json::Value {
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "_bulk_edit".to_string(),
            serde_json::json!({
                "fade_editable": rebuild_editable,
                "rebuild_editable": rebuild_editable,
            }),
        );
    }
    payload
}

fn ensure_rebuild_safe(
    cue: &dyn Cue,
    is_loading: bool,
    change_description: &str,
) -> Result<(), String> {
    if is_fade_rebuild_editable(cue, is_loading) {
        return Ok(());
    }
    if is_loading {
        return Err(format!(
            "Cue {} is loading; wait for it to finish and stop it before changing {change_description}",
            cue.id(),
        ));
    }
    if cue.is_preloaded_or_loading() {
        return Err(format!(
            "Cue {} is preloaded; stop or reset it before changing {change_description}",
            cue.id(),
        ));
    }
    Err(format!(
        "Cue {} is {:?}; stop it before changing {change_description}",
        cue.id(),
        cue.state()
    ))
}

/// Validate a request to change the authored action duration. `CueSummary`
/// duration is computed, so the generic JSON update route must never be used
/// for it: only these five cue types own a corresponding persisted field.
fn validate_user_action_duration(cue: &dyn Cue, duration_ms: Option<u64>) -> Result<(), String> {
    match (cue.cue_type(), duration_ms) {
        (CueType::Wait, Some(_)) | (CueType::Fade, Some(_)) | (CueType::Light, Some(_)) => Ok(()),
        (CueType::Image, _) | (CueType::Text, _) => Ok(()),
        (CueType::Wait, None) => Err("Wait duration cannot be indefinite".to_string()),
        (CueType::Fade, None) => Err("Fade duration cannot be indefinite".to_string()),
        (CueType::Light, None) => Err("Light fade duration cannot be indefinite".to_string()),
        (cue_type, _) => Err(format!("Duration is not user-editable for {cue_type} cue")),
    }
}

fn apply_user_action_duration(cue: &mut dyn Cue, duration_ms: Option<u64>) -> Result<(), String> {
    validate_user_action_duration(cue, duration_ms)?;
    cue.set_user_action_duration(duration_ms.map(Duration::from_millis))
        .map_err(|e| e.to_string())
}

/// Validate and prepare one bulk entry without mutating the cue list. Safe
/// live fields keep the existing object; every other persisted type-specific
/// field is rebuilt eagerly so malformed input rejects the whole transaction
/// before an undo snapshot is created.
fn prepare_bulk_update(
    cue: &dyn Cue,
    update: &BulkCueUpdate,
    registry: &crate::cue::registry::CueRegistry,
    is_loading: bool,
) -> Result<PreparedBulkUpdate, String> {
    let properties = update
        .properties
        .as_object()
        .ok_or("properties must be an object")?;
    if properties.is_empty() {
        return Err("properties must not be empty".to_string());
    }

    let mut common = CommonCuePatch::default();
    // Validate common values even when they are combined with a rebuild field.
    // Cue factories are intentionally tolerant while loading old workspaces,
    // so relying on them here could silently ignore a malformed bulk value.
    for (key, value) in properties {
        if is_common_bulk_key(key) {
            validate_common_bulk_value(key, value, &mut common)?;
        }
    }
    let common_only = properties.keys().all(|key| is_common_bulk_key(key));
    if common_only {
        return Ok(PreparedBulkUpdate {
            id: cue.id(),
            common,
            in_place: None,
            rebuilt: None,
        });
    }

    if let Some(in_place) = classify_live_in_place_patch(&update.properties, cue.cue_type())? {
        return Ok(PreparedBulkUpdate {
            id: cue.id(),
            common,
            in_place: Some(in_place),
            rebuilt: None,
        });
    }

    reject_group_rebuild(cue, &update.properties)?;
    ensure_rebuild_safe(cue, is_loading, "properties")?;
    validate_visual_output_pair(properties)?;

    let mut json = cue.serialize();
    let json_object = json
        .as_object_mut()
        .ok_or("Cue serialisation is not an object")?;
    for (key, value) in properties {
        if matches!(
            key.as_str(),
            "id" | "type"
                | "cue_type"
                | "children"
                | "state"
                | "duration_ms"
                | "file_path"
                | "_bulk_edit"
        ) {
            return Err(format!("{key} is protected and cannot be bulk-updated"));
        }
        if is_common_bulk_key(key) {
            // Already validated above; it is merged into the rebuilt JSON
            // alongside the type-specific fields below.
        } else if !rebuild_key_allowed_for_type(key, cue.cue_type()) {
            return Err(format!("{key} is not supported for {} cue", cue.cue_type()));
        } else {
            validate_rebuild_bulk_value(key, value)?;
        }
        json_object.insert(key.clone(), value.clone());
    }
    let mut preserved_audio = HashMap::new();
    collect_decoded_audio_recursive(cue, &mut preserved_audio);
    let runtime = cue.runtime_state();
    let mut rebuilt = registry.from_json(json).map_err(|e| e.to_string())?;
    restore_decoded_audio_recursive(rebuilt.as_mut(), &preserved_audio);
    rebuilt.restore_runtime_state(runtime);

    Ok(PreparedBulkUpdate {
        id: cue.id(),
        common,
        in_place: None,
        rebuilt: Some(rebuilt),
    })
}

/// Atomically apply inspector edits to several cues.
///
/// Validation and replacement construction happen before the undo snapshot and
/// before any mutation.  Thus a failed multi-cue edit leaves the workspace,
/// undo history, dirty state and event stream untouched.
#[tauri::command]
pub fn bulk_update_cues(
    updates: Vec<BulkCueUpdate>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    if updates.is_empty() {
        return Err("updates must not be empty".to_string());
    }

    let mut ids = HashSet::with_capacity(updates.len());
    let parsed_ids: Vec<Uuid> = updates
        .iter()
        .map(|update| {
            let id = update.cue_id.parse::<Uuid>().map_err(|e| e.to_string())?;
            if !ids.insert(id) {
                return Err("duplicate cue ID in bulk update".to_string());
            }
            Ok(id)
        })
        .collect::<Result<_, String>>()?;

    // Retain the established lock order.  The registry is needed while all
    // prospective fade replacements are built, before the cue list changes.
    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let patch_channels_by_id: Vec<(Uuid, Vec<u16>)> = ws
        .output_patches
        .iter()
        .map(|patch| (patch.id, patch.channels.clone()))
        .collect();
    let default_patch_id = ws.default_output_patch_id;
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let loading = state.loading_cues.lock().map_err(|e| e.to_string())?;

    let mut prepared = Vec::with_capacity(updates.len());
    for (update, id) in updates.iter().zip(parsed_ids) {
        let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
        prepared.push(prepare_bulk_update(
            cue,
            update,
            &registry,
            loading.contains(&id),
        )?);
    }
    drop(loading);

    // All fallible work is complete.  Capture exactly one before-state while
    // holding the same locks as the mutation so no other command can slip in.
    let snapshot = super::undo_cmds::take_snapshot(cue_list);
    state
        .undo_stack
        .lock()
        .map_err(|e| e.to_string())?
        .push_action(snapshot);

    for mut update in prepared {
        if let Some(rebuilt) = update.rebuilt.take() {
            // Replacements are leaf-only by validation.  Recursive replacement
            // makes nested Audio/Video/Image/Camera leaves safe.
            if !cue_list.replace_cue_recursive(&update.id, rebuilt) {
                return Err("Cue disappeared during bulk update".to_string());
            }
        } else if let Some(in_place) = update.in_place.take() {
            let cue = cue_list
                .get_mut_recursive(&update.id)
                .ok_or("Cue disappeared during bulk update")?;
            apply_single_in_place_patch(cue, in_place);
            apply_existing_cue_live_side_effects(
                cue,
                &state,
                &patch_channels_by_id,
                default_patch_id,
            );
            continue;
        }
        let cue = cue_list
            .get_mut_recursive(&update.id)
            .ok_or("Cue disappeared during bulk update")?;
        apply_common_bulk_patch(cue, update.common);
    }
    ws.mark_modified();
    drop(ws);
    drop(registry);
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

#[cfg(test)]
mod bulk_update_tests {
    use super::*;
    use crate::{
        cue::{
            audio_cue::{AudioCue, AudioCueFactory},
            camera_cue::{CameraCue, CameraCueFactory},
            fade_cue::FadeCue,
            group_cue::GroupCue,
            image_cue::{ImageCue, ImageCueFactory},
            light_cue::LightCue,
            mic_cue::MicCue,
            registry::CueRegistry,
            text_cue::TextCue,
            traits::RuntimeState,
            video_cue::{VideoCue, VideoCueFactory},
            wait_cue::WaitCue,
        },
        show::cue_list::CueList,
    };

    /// Command paths which need more than one of these mutexes must acquire
    /// them in this sequence. Keeping the policy as executable test data makes
    /// a workspace → registry regression immediately visible in review.
    #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
    enum MutationLock {
        Registry,
        Workspace,
        LoadingCues,
        UndoStack,
    }

    fn follows_mutation_lock_order(locks: &[MutationLock]) -> bool {
        locks.windows(2).all(|pair| pair[0] <= pair[1])
    }

    #[test]
    fn number_preview_resolves_the_configured_master_media_cue() {
        let mut number = GroupCue::new_number();
        let master = AudioCue::new();
        let master_id = master.id();
        number.add_child(Box::new(master), -1).unwrap();
        number.set_number_master(master_id).unwrap();

        let target = preview_target_cue(&number).expect("Number master should be previewable");
        assert_eq!(target.id(), master_id);
        assert_eq!(target.cue_type(), CueType::Audio);
    }

    #[test]
    fn number_preview_reports_missing_or_malformed_master() {
        let number = GroupCue::new_number();
        match preview_target_cue(&number) {
            Err(error) => assert_eq!(error, "Number needs a master Audio or Video cue"),
            Ok(_) => panic!("an empty Number must not resolve a preview source"),
        }

        let image = ImageCue::new();
        let image_id = image.id();
        let number_json = serde_json::json!({
            "type": "number",
            "cue_type": "number",
            "master_child_id": image_id.to_string(),
            "children": [image.serialize()],
        });
        let number = GroupCue::from_json_with_registry(
            &number_json,
            &registry(),
        )
        .unwrap();
        match preview_target_cue(number.as_ref()) {
            Err(error) => assert_eq!(error, "Number master must be an Audio or Video cue"),
            Ok(_) => panic!("an Image cannot be a Number preview master"),
        }
    }

    #[test]
    fn duplicate_commands_follow_the_global_mutation_lock_order() {
        // duplicate_cue and duplicate_cues acquire Registry → Workspace, then
        // take UndoStack only after releasing the no-longer-needed registry.
        assert!(follows_mutation_lock_order(&[
            MutationLock::Registry,
            MutationLock::Workspace,
            MutationLock::UndoStack,
        ]));
        assert!(follows_mutation_lock_order(&[
            MutationLock::Registry,
            MutationLock::Workspace,
            MutationLock::LoadingCues,
            MutationLock::UndoStack,
        ]));
        assert!(!follows_mutation_lock_order(&[
            MutationLock::Workspace,
            MutationLock::Registry,
        ]));
    }

    fn registry() -> CueRegistry {
        let mut registry = CueRegistry::new();
        registry.register(CueType::Audio, Box::new(AudioCueFactory));
        registry.register(CueType::Video, Box::new(VideoCueFactory));
        registry.register(CueType::Image, Box::new(ImageCueFactory));
        registry.register(CueType::Camera, Box::new(CameraCueFactory));
        registry.register(CueType::Browser, Box::new(crate::cue::browser_cue::BrowserCueFactory));
        registry
    }

    fn entry(id: Uuid, properties: serde_json::Value) -> BulkCueUpdate {
        BulkCueUpdate {
            cue_id: id.to_string(),
            properties,
        }
    }

    #[test]
    fn nested_summary_uses_cached_metadata_with_workspace_relative_path() {
        let workspace_dir = std::path::PathBuf::from("C:/shows/demo");
        let relative_path = std::path::PathBuf::from("media/poster.png");
        let resolved_path = workspace_dir.join(&relative_path);

        let mut image = ImageCue::new();
        image.file_path = Some(relative_path);
        let image_id = image.id();
        let mut inner = GroupCue::new();
        inner.children.push(Box::new(image));
        let mut outer = GroupCue::new();
        outer.children.push(Box::new(inner));

        let key = MediaMetadataKey::new(resolved_path.clone(), true);
        let metadata = HashMap::from([(
            key.clone(),
            MediaMetadataEntry::Ready(MediaMetadata {
                file_size_bytes: 12_345,
                width: Some(7680),
                height: Some(4320),
                compatibility_status: 0,
                compatibility_reason: None,
                probe_failure: None,
                modified_at: None,
            }),
        )]);
        let mut requests = HashSet::new();
        let loading = HashSet::new();
        let patches = PatchTable {
            patches: &[],
            default_id: None,
        };

        let summary = summarise(
            &outer,
            Some(&workspace_dir),
            &patches,
            &metadata,
            &CueTargetIndex::default(),
            &mut requests,
            &loading,
        );
        let child = summary
            .children
            .as_ref()
            .and_then(|children| children.first())
            .and_then(|group| group.children.as_ref())
            .and_then(|children| children.first())
            .expect("nested image summary");
        assert_eq!(child.id, image_id.to_string());
        assert_eq!(child.file_size_bytes, Some(12_345));
        assert_eq!(
            (child.media_width, child.media_height),
            (Some(7680), Some(4320))
        );
        assert_eq!(
            requests,
            HashSet::from([key]),
            "cached media keys are offered to the cache for TTL validation"
        );
    }

    #[test]
    fn nested_uncached_media_requests_the_resolved_path_once() {
        let workspace_dir = std::path::PathBuf::from("C:/shows/demo");
        let mut image = ImageCue::new();
        image.file_path = Some(std::path::PathBuf::from("media/poster.png"));
        let mut group = GroupCue::new();
        group.children.push(Box::new(image));
        let patches = PatchTable {
            patches: &[],
            default_id: None,
        };
        let mut requests = HashSet::new();

        let summary = summarise(
            &group,
            Some(&workspace_dir),
            &patches,
            &HashMap::new(),
            &CueTargetIndex::default(),
            &mut requests,
            &HashSet::new(),
        );

        assert_eq!(summary.children.as_ref().unwrap()[0].file_size_bytes, None);
        assert_eq!(
            requests,
            HashSet::from([MediaMetadataKey::new(
                workspace_dir.join("media/poster.png"),
                true,
            )])
        );
    }

    #[test]
    fn missing_media_flag_is_distinct_from_general_broken_state() {
        let mut image = ImageCue::new();
        image.file_path = Some(std::path::PathBuf::from("C:/missing/poster.png"));
        let key = MediaMetadataKey::new(std::path::PathBuf::from("C:/missing/poster.png"), true);
        let metadata = HashMap::from([(
            key,
            MediaMetadataEntry::Failed(MediaMetadataFailure::MissingFile),
        )]);
        let patches = PatchTable {
            patches: &[],
            default_id: None,
        };

        let summary = summarise(
            &image,
            None,
            &patches,
            &metadata,
            &CueTargetIndex::default(),
            &mut HashSet::new(),
            &HashSet::new(),
        );

        assert!(summary.media_file_missing);
        assert!(summary.is_broken);
        assert_eq!(
            summary.error_message.as_deref(),
            Some("Media file was not found.")
        );
    }

    fn prepare_all(
        list: &CueList,
        updates: &[BulkCueUpdate],
        registry: &CueRegistry,
    ) -> Result<Vec<PreparedBulkUpdate>, String> {
        updates
            .iter()
            .map(|update| {
                let id = update.cue_id.parse::<Uuid>().map_err(|e| e.to_string())?;
                let cue = list.get_recursive(&id).ok_or("Cue not found")?;
                prepare_bulk_update(cue, update, registry, false)
            })
            .collect()
    }

    fn apply_all(list: &mut CueList, mut prepared: Vec<PreparedBulkUpdate>) {
        for mut update in prepared.drain(..) {
            if let Some(rebuilt) = update.rebuilt.take() {
                assert!(list.replace_cue_recursive(&update.id, rebuilt));
            } else if let Some(in_place) = update.in_place.take() {
                apply_single_in_place_patch(list.get_mut_recursive(&update.id).unwrap(), in_place);
                continue;
            }
            let cue = list.get_mut_recursive(&update.id).unwrap();
            apply_common_bulk_patch(cue, update.common);
        }
    }

    #[test]
    fn common_patch_updates_different_cue_types_without_rebuild() {
        let mut list = CueList::new("test");
        let audio = AudioCue::new();
        let audio_id = audio.id();
        let image = ImageCue::new();
        let image_id = image.id();
        list.cues.push(Box::new(audio));
        list.cues.push(Box::new(image));
        let registry = registry();
        let updates = vec![
            entry(
                audio_id,
                serde_json::json!({ "color": "green", "notes": "all set", "pre_wait_ms": 125 }),
            ),
            entry(
                image_id,
                serde_json::json!({ "color": "green", "notes": "all set", "pre_wait_ms": 125 }),
            ),
        ];

        let prepared = prepare_all(&list, &updates, &registry).unwrap();
        assert!(prepared.iter().all(|p| p.rebuilt.is_none()));
        apply_all(&mut list, prepared);
        for id in [audio_id, image_id] {
            let cue = list.get_recursive(&id).unwrap();
            assert_eq!(cue.color(), CueColor::Green);
            assert_eq!(cue.notes(), "all set");
            assert_eq!(cue.pre_wait(), Duration::from_millis(125));
        }
    }

    #[test]
    fn audio_fades_are_compatible_for_audio_and_video() {
        let mut list = CueList::new("test");
        let audio = AudioCue::new();
        let audio_id = audio.id();
        let video = VideoCue::new();
        let video_id = video.id();
        list.cues.push(Box::new(audio));
        list.cues.push(Box::new(video));
        let registry = registry();
        let updates = vec![
            entry(
                audio_id,
                serde_json::json!({ "fade_in_ms": 300, "fade_in_curve": "linear" }),
            ),
            entry(
                video_id,
                serde_json::json!({ "fade_in_ms": 300, "fade_in_curve": "linear" }),
            ),
        ];

        let prepared = prepare_all(&list, &updates, &registry).unwrap();
        assert!(prepared.iter().all(|p| p.rebuilt.is_some()));
        apply_all(&mut list, prepared);
        for id in [audio_id, video_id] {
            assert_eq!(
                list.get_recursive(&id).unwrap().serialize()["fade_in_ms"],
                300
            );
        }
    }

    #[test]
    fn bulk_live_levels_layers_and_geometry_keep_the_existing_cue_object() {
        let mut list = CueList::new("test");
        let video = VideoCue::new();
        let id = video.id();
        list.cues.push(Box::new(video));
        let before = list.get_recursive(&id).unwrap() as *const dyn Cue as *const ();
        let updates = vec![entry(
            id,
            serde_json::json!({
                "volume_db": -9.0,
                "level_matrix": [[0.0, -12.0], [-12.0, 0.0]],
                "geometry": {
                    "fit_mode": "fill", "pan_x": 0.1, "pan_y": -0.2,
                    "scale": 1.25, "rotation": 15,
                    "crop_left": 0.01, "crop_right": 0.02,
                    "crop_top": 0.03, "crop_bottom": 0.04
                },
                "layer_style": { "layer": 12, "opacity": 0.7, "blend_mode": "screen" }
            }),
        )];

        let prepared = prepare_all(&list, &updates, &registry()).unwrap();
        assert!(prepared[0].in_place.is_some());
        assert!(prepared[0].rebuilt.is_none());
        apply_all(&mut list, prepared);

        let cue = list.get_recursive(&id).unwrap();
        assert_eq!(before, cue as *const dyn Cue as *const ());
        let json = cue.serialize();
        assert_eq!(json["volume_db"], -9.0);
        assert_eq!(json["level_matrix"][0][1], -12.0);
        assert_eq!(json["geometry"]["fit_mode"], "fill");
        assert_eq!(json["layer_style"]["blend_mode"], "screen");
    }

    #[test]
    fn bulk_routing_rebuilds_a_stopped_leaf_and_rejects_a_running_one() {
        let mut list = CueList::new("test");
        let video = VideoCue::new();
        let id = video.id();
        list.cues.push(Box::new(video));
        let update = entry(
            id,
            serde_json::json!({
                "output_id": "projector",
                "output_ids": ["projector", "ndi-program"]
            }),
        );

        let prepared =
            prepare_all(&list, &[entry(id, update.properties.clone())], &registry()).unwrap();
        assert!(prepared[0].rebuilt.is_some());
        apply_all(&mut list, prepared);
        let json = list.get_recursive(&id).unwrap().serialize();
        assert_eq!(json["output_id"], "projector");
        assert_eq!(
            json["output_ids"],
            serde_json::json!(["projector", "ndi-program"])
        );

        list.get_mut_recursive(&id)
            .unwrap()
            .restore_runtime_state(RuntimeState {
                state: CueState::Running,
                voice_id: None,
                started_at: None,
                action_started_at: None,
            });
        assert!(prepare_all(&list, &[update], &registry()).is_err());
    }

    #[test]
    fn bulk_rebuild_rejects_derived_and_malformed_fields() {
        let mut list = CueList::new("test");
        let video = VideoCue::new();
        let id = video.id();
        list.cues.push(Box::new(video));
        let registry = registry();

        for properties in [
            serde_json::json!({ "cached_duration_ms": 1234 }),
            serde_json::json!({ "video_fade_in_curve": "not-a-curve" }),
            serde_json::json!({ "output_patch_id": "not-a-uuid" }),
            serde_json::json!({ "loop_count": u32::MAX as u64 + 1 }),
            serde_json::json!({ "hold_last_frame": "yes" }),
            serde_json::json!({ "notes": false, "video_fade_in_ms": 100 }),
        ] {
            assert!(
                prepare_all(&list, &[entry(id, properties)], &registry).is_err(),
                "malformed/derived rebuild field must be rejected",
            );
        }
    }

    #[test]
    fn bulk_rebuild_validates_numeric_field_ranges_and_nullability() {
        for (key, invalid) in [
            ("start_time_ms", serde_json::json!(-1)),
            ("display_duration_ms", serde_json::json!(1.5)),
            ("wait_duration_ms", serde_json::Value::Null),
            ("fade_duration_ms", serde_json::json!(-1)),
            ("rate", serde_json::json!(0.0)),
        ] {
            assert!(
                validate_rebuild_bulk_value(key, &invalid).is_err(),
                "{key} must reject {invalid}"
            );
        }
        assert!(validate_rebuild_bulk_value("start_time_ms", &serde_json::Value::Null).is_ok());
        assert!(
            validate_rebuild_bulk_value("display_duration_ms", &serde_json::Value::Null).is_ok()
        );
        assert!(validate_rebuild_bulk_value("wait_duration_ms", &serde_json::json!(0)).is_ok());
        assert!(validate_rebuild_bulk_value("fade_duration_ms", &serde_json::json!(0)).is_ok());
        assert!(validate_rebuild_bulk_value("rate", &serde_json::json!(4.0)).is_ok());
    }

    #[test]
    fn bulk_visual_routing_requires_one_consistent_pair() {
        let mut list = CueList::new("test");
        let image = ImageCue::new();
        let id = image.id();
        list.cues.push(Box::new(image));
        let registry = registry();

        for properties in [
            serde_json::json!({ "output_ids": ["projector"] }),
            serde_json::json!({ "output_id": "projector" }),
            serde_json::json!({ "output_id": "projector", "output_ids": ["ndi-program"] }),
            serde_json::json!({ "output_id": null, "output_ids": ["projector"] }),
            serde_json::json!({ "output_id": "projector", "output_ids": ["projector", "projector"] }),
        ] {
            assert!(prepare_all(&list, &[entry(id, properties)], &registry).is_err());
        }

        let clear = prepare_all(
            &list,
            &[entry(
                id,
                serde_json::json!({ "output_id": null, "output_ids": [] }),
            )],
            &registry,
        );
        assert!(clear.is_ok());
    }

    #[test]
    fn incompatible_or_protected_patch_is_rejected_before_mutation() {
        let mut list = CueList::new("test");
        let image = ImageCue::new();
        let id = image.id();
        list.cues.push(Box::new(image));
        let before = list.get_recursive(&id).unwrap().serialize();
        let registry = registry();

        let bad_visual = entry(id, serde_json::json!({ "video_fade_in_ms": 200 }));
        assert!(prepare_all(&list, &[bad_visual], &registry).is_err());
        let protected = entry(id, serde_json::json!({ "file_path": "nope.wav" }));
        assert!(prepare_all(&list, &[protected], &registry).is_err());
        assert_eq!(list.get_recursive(&id).unwrap().serialize(), before);
    }

    #[test]
    fn common_group_and_child_patch_keeps_nested_child_and_fade_rebuild_stays_leaf_only() {
        let mut list = CueList::new("test");
        let child = AudioCue::new();
        let child_id = child.id();
        let mut group = GroupCue::new();
        let group_id = group.id();
        group.children.push(Box::new(child));
        list.cues.push(Box::new(group));
        let registry = registry();
        let common = vec![
            entry(group_id, serde_json::json!({ "notes": "group note" })),
            entry(child_id, serde_json::json!({ "notes": "child note" })),
        ];
        let prepared = prepare_all(&list, &common, &registry).unwrap();
        apply_all(&mut list, prepared);
        assert_eq!(list.get_recursive(&group_id).unwrap().notes(), "group note");
        assert_eq!(list.get_recursive(&child_id).unwrap().notes(), "child note");

        let bad_group_fade = entry(group_id, serde_json::json!({ "fade_in_ms": 100 }));
        assert!(prepare_all(&list, &[bad_group_fade], &registry).is_err());
        assert_eq!(list.get_recursive(&child_id).unwrap().notes(), "child note");
    }

    #[test]
    fn fade_rebuild_rejects_each_unsafe_lifecycle_state_and_loading() {
        let mut cue = AudioCue::new();
        for state in [CueState::Running, CueState::Paused] {
            cue.restore_runtime_state(RuntimeState {
                state,
                voice_id: None,
                started_at: None,
                action_started_at: None,
            });
            assert!(ensure_rebuild_safe(&cue, false, "properties").is_err());
        }
        for state in [CueState::Standby, CueState::Completed] {
            cue.restore_runtime_state(RuntimeState {
                state,
                voice_id: None,
                started_at: None,
                action_started_at: None,
            });
            assert!(ensure_rebuild_safe(&cue, false, "properties").is_ok());
        }
        assert!(ensure_rebuild_safe(&cue, true, "properties").is_err());
    }

    #[test]
    fn unsafe_fade_in_a_mixed_batch_rejects_before_common_mutation() {
        let mut list = CueList::new("test");
        let audio = AudioCue::new();
        let audio_id = audio.id();
        let mut video = VideoCue::new();
        let video_id = video.id();
        video.restore_runtime_state(RuntimeState {
            state: CueState::Running,
            voice_id: None,
            started_at: None,
            action_started_at: None,
        });
        list.cues.push(Box::new(audio));
        list.cues.push(Box::new(video));
        let registry = registry();
        let updates = vec![
            entry(audio_id, serde_json::json!({ "notes": "must not apply" })),
            entry(video_id, serde_json::json!({ "fade_in_ms": 200 })),
        ];
        assert!(prepare_all(&list, &updates, &registry).is_err());
        assert_ne!(
            list.get_recursive(&audio_id).unwrap().notes(),
            "must not apply"
        );
    }

    #[test]
    fn get_cues_metadata_reports_preloaded_standby_as_not_fade_editable() {
        let cue = AudioCue::new();
        let ordinary =
            with_bulk_edit_metadata(cue.serialize(), is_fade_rebuild_editable(&cue, false));
        assert_eq!(ordinary["_bulk_edit"]["fade_editable"], true);
        assert_eq!(ordinary["_bulk_edit"]["rebuild_editable"], true);
        assert!(cue.serialize().get("_bulk_edit").is_none());

        // Video/Image preload hooks can remain Standby. This exercises the
        // same state predicate that their hook feeds in the command path.
        let preloaded = with_bulk_edit_metadata(
            serde_json::json!({ "cue_type": "video" }),
            is_fade_rebuild_editable_state(CueState::Standby, true, false),
        );
        assert_eq!(preloaded["_bulk_edit"]["fade_editable"], false);
        assert_eq!(preloaded["_bulk_edit"]["rebuild_editable"], false);
    }

    #[test]
    fn typed_action_duration_updates_all_supported_cue_types_and_serialises() {
        let mut wait = WaitCue::new();
        apply_user_action_duration(&mut wait, Some(0)).unwrap();
        assert_eq!(wait.duration(), Some(Duration::ZERO));
        assert_eq!(wait.serialize()["wait_duration_ms"], 0);

        let mut fade = FadeCue::new();
        apply_user_action_duration(&mut fade, Some(1_250)).unwrap();
        assert_eq!(fade.duration(), Some(Duration::from_millis(1_250)));
        assert_eq!(fade.serialize()["fade_duration_ms"], 1_250);

        let mut image = ImageCue::new();
        apply_user_action_duration(&mut image, Some(2_500)).unwrap();
        assert_eq!(image.duration(), Some(Duration::from_millis(2_500)));
        assert_eq!(image.serialize()["display_duration_ms"], 2_500);

        let mut text = TextCue::new();
        apply_user_action_duration(&mut text, Some(3_750)).unwrap();
        assert_eq!(text.duration(), Some(Duration::from_millis(3_750)));
        assert_eq!(text.serialize()["display_duration_ms"], 3_750);

        let mut light = LightCue::new();
        apply_user_action_duration(&mut light, Some(0)).unwrap();
        assert_eq!(light.duration(), Some(Duration::ZERO));
        assert_eq!(light.serialize()["fade"]["duration_ms"], 0);
    }

    #[test]
    fn typed_action_duration_only_allows_indefinite_image_and_text() {
        let mut image = ImageCue::new();
        apply_user_action_duration(&mut image, Some(500)).unwrap();
        apply_user_action_duration(&mut image, None).unwrap();
        assert_eq!(image.duration(), None);
        assert!(image.serialize()["display_duration_ms"].is_null());

        let mut text = TextCue::new();
        apply_user_action_duration(&mut text, Some(500)).unwrap();
        apply_user_action_duration(&mut text, None).unwrap();
        assert_eq!(text.duration(), None);
        assert!(text.serialize()["display_duration_ms"].is_null());

        let mut wait = WaitCue::new();
        let mut fade = FadeCue::new();
        let mut light = LightCue::new();
        for cue in [
            &mut wait as &mut dyn Cue,
            &mut fade as &mut dyn Cue,
            &mut light as &mut dyn Cue,
        ] {
            assert!(apply_user_action_duration(cue, None).is_err());
        }
    }

    #[test]
    fn typed_action_duration_rejects_computed_and_group_durations() {
        let mut audio = AudioCue::new();
        assert_eq!(
            apply_user_action_duration(&mut audio, Some(1_000)).unwrap_err(),
            "Duration is not user-editable for audio cue",
        );

        let mut group = GroupCue::new();
        assert_eq!(
            apply_user_action_duration(&mut group, Some(1_000)).unwrap_err(),
            "Duration is not user-editable for group cue",
        );
    }

    #[test]
    fn typed_action_duration_updates_a_nested_group_child_in_place() {
        let child = WaitCue::new();
        let child_id = child.id();
        let mut group = GroupCue::new();
        group.children.push(Box::new(child));
        let mut list = CueList::new("test");
        list.cues.push(Box::new(group));

        let before = {
            let cue = list.get_recursive(&child_id).unwrap();
            cue as *const dyn Cue as *const ()
        };
        apply_user_action_duration(list.get_mut_recursive(&child_id).unwrap(), Some(9_000))
            .unwrap();
        let after = {
            let cue = list.get_recursive(&child_id).unwrap();
            cue as *const dyn Cue as *const ()
        };
        assert_eq!(before, after, "duration updates do not replace nested cues");
        assert_eq!(
            list.get_recursive(&child_id).unwrap().duration(),
            Some(Duration::from_secs(9))
        );
    }

    #[test]
    fn typed_action_duration_uses_the_authoritative_lifecycle_guard() {
        let runtime = |state| RuntimeState {
            state,
            voice_id: None,
            started_at: None,
            action_started_at: None,
        };
        let mut fade = FadeCue::new();
        for state in [CueState::Running, CueState::Paused] {
            fade.restore_runtime_state(runtime(state));
            assert!(ensure_rebuild_safe(&fade, false, "duration").is_err());
        }
        fade.restore_runtime_state(runtime(CueState::Standby));
        assert!(ensure_rebuild_safe(&fade, true, "duration").is_err());
        assert!(ensure_rebuild_safe(&fade, false, "duration").is_ok());
    }

    #[test]
    fn raw_summary_duration_is_protected_from_generic_updates() {
        assert!(matches!(
            classify_single_cue_update(&serde_json::json!({ "duration_ms": 1_000 })),
            Err(error) if error == "duration_ms is protected and cannot be updated",
        ));
    }

    #[test]
    fn single_cue_direct_patch_keeps_a_running_camera_object_and_runtime() {
        let mut list = CueList::new("test");
        let mut camera = CameraCue::new();
        let camera_id = camera.id();
        camera.restore_runtime_state(RuntimeState {
            state: CueState::Running,
            voice_id: None,
            started_at: None,
            action_started_at: None,
        });
        list.cues.push(Box::new(camera));
        let before = {
            let cue = list.get_recursive(&camera_id).unwrap();
            cue as *const dyn Cue as *const ()
        };

        let patch = classify_single_cue_update(&serde_json::json!({
            "name": "FOH", "notes": "safe live", "color": "cyan",
            "continue_mode": "auto_continue", "is_disabled": true,
            "pre_wait_ms": 25, "post_wait_ms": 50,
        }))
        .unwrap();
        let SingleCueUpdateKind::Direct(patch) = patch else {
            panic!("common fields must use in-place update");
        };
        apply_single_direct_patch(list.get_mut_recursive(&camera_id).unwrap(), patch);

        let after = {
            let cue = list.get_recursive(&camera_id).unwrap();
            cue as *const dyn Cue as *const ()
        };
        let cue = list.get_recursive(&camera_id).unwrap();
        assert_eq!(before, after);
        assert_eq!(cue.name(), "FOH");
        assert_eq!(cue.notes(), "safe live");
        assert_eq!(cue.color(), CueColor::Cyan);
        assert_eq!(cue.state(), CueState::Running);
    }

    #[test]
    fn single_cue_rebuild_guard_rejects_unsafe_properties_and_invalid_patches() {
        let audio_volume = serde_json::json!({ "volume_db": -12.0 });
        let video_geometry = serde_json::json!({ "geometry": { "scale": 1.2 } });
        let camera_source = serde_json::json!({ "ndi_quality": "high" });
        for patch in [&audio_volume, &video_geometry, &camera_source] {
            assert!(matches!(
                classify_single_cue_update(patch),
                Ok(SingleCueUpdateKind::Rebuild)
            ));
        }

        let runtime = |state| RuntimeState {
            state,
            voice_id: None,
            started_at: None,
            action_started_at: None,
        };
        let mut camera = CameraCue::new();
        camera.restore_runtime_state(runtime(CueState::Running));
        assert!(ensure_rebuild_safe(&camera, false, "properties").is_err());

        let mut video = VideoCue::new();
        video.restore_runtime_state(runtime(CueState::Running));
        assert!(ensure_rebuild_safe(&video, false, "properties").is_err());

        let mut audio = AudioCue::new();
        for state in [CueState::Running, CueState::Paused] {
            audio.restore_runtime_state(runtime(state));
            assert!(ensure_rebuild_safe(&audio, false, "properties").is_err());
        }

        audio.restore_runtime_state(runtime(CueState::Standby));
        assert!(ensure_rebuild_safe(&audio, true, "properties").is_err());
        assert!(ensure_rebuild_safe(&audio, false, "properties").is_ok());
        assert!(!is_fade_rebuild_editable_state(
            CueState::Standby,
            true,
            false
        ));

        let mut json = audio.serialize();
        json["volume_db"] = serde_json::json!(-12.0);
        assert!(
            registry().from_json(json).is_ok(),
            "standby-specific update rebuilds"
        );
        assert!(classify_single_cue_update(&serde_json::json!({ "id": "nope" })).is_err());
        assert!(classify_single_cue_update(&serde_json::json!({ "_bulk_edit": {} })).is_err());
        assert!(classify_single_cue_update(&serde_json::json!({})).is_err());
        assert!(classify_single_cue_update(&serde_json::json!([])).is_err());
        assert!(classify_single_cue_update(&serde_json::json!({ "notes": false })).is_err());
    }

    #[test]
    fn live_audio_and_visual_patches_persist_without_replacing_cues() {
        let mut audio = AudioCue::new();
        let audio_patch = classify_live_in_place_patch(
            &serde_json::json!({ "volume_db": -8.0, "pan": 0.25, "level_matrix": [[0.0, -6.0]] }),
            CueType::Audio,
        )
        .unwrap()
        .unwrap();
        apply_single_in_place_patch(&mut audio, audio_patch);
        assert_eq!(audio.serialize()["volume_db"], -8.0);
        assert_eq!(audio.serialize()["pan"], 0.25);
        assert_eq!(audio.serialize()["level_matrix"][0][1], -6.0);

        let mut video = VideoCue::new();
        let video_patch = classify_live_in_place_patch(
            &serde_json::json!({
                "volume_db": -4.0, "level_matrix": [[0.0]],
                "geometry": { "fit_mode": "fit", "pan_x": 0.2, "pan_y": 0.0, "scale": 1.0, "rotation": 0, "crop_left": 0.0, "crop_right": 0.0, "crop_top": 0.0, "crop_bottom": 0.0 },
                "layer_style": { "layer": 4, "opacity": 0.5, "blend_mode": "normal" },
            }),
            CueType::Video,
        ).unwrap().unwrap();
        apply_single_in_place_patch(&mut video, video_patch);
        assert_eq!(video.serialize()["volume_db"], -4.0);
        assert_eq!(video.serialize()["level_matrix"][0][0], 0.0);
        assert_eq!(video.serialize()["geometry"]["pan_x"], 0.2);
        assert_eq!(video.serialize()["layer_style"]["layer"], 4);

        let mut mic = MicCue::new();
        let mic_patch = classify_live_in_place_patch(
            &serde_json::json!({ "volume_db": -3.0, "pan": -0.5 }),
            CueType::Mic,
        )
        .unwrap()
        .unwrap();
        apply_single_in_place_patch(&mut mic, mic_patch);
        assert_eq!(mic.serialize()["volume_db"], -3.0);
        assert_eq!(mic.serialize()["pan"], -0.5);

        for (cue_type, mut cue) in [
            (CueType::Image, Box::new(ImageCue::new()) as Box<dyn Cue>),
            (CueType::Camera, Box::new(CameraCue::new()) as Box<dyn Cue>),
        ] {
            let visual_patch = classify_live_in_place_patch(
                &serde_json::json!({
                    "geometry": { "fit_mode": "fit", "pan_x": -0.2, "pan_y": 0.0, "scale": 1.0, "rotation": 0, "crop_left": 0.0, "crop_right": 0.0, "crop_top": 0.0, "crop_bottom": 0.0 },
                    "layer_style": { "layer": 6, "opacity": 0.75, "blend_mode": "normal" },
                }),
                cue_type,
            ).unwrap().unwrap();
            let pointer = cue.as_ref() as *const dyn Cue as *const ();
            apply_single_in_place_patch(cue.as_mut(), visual_patch);
            assert_eq!(pointer, cue.as_ref() as *const dyn Cue as *const ());
            assert_eq!(cue.serialize()["geometry"]["pan_x"], -0.2);
            assert_eq!(cue.serialize()["layer_style"]["layer"], 6);
        }
    }

    #[test]
    fn group_children_and_dedicated_fields_are_rejected_before_rebuild() {
        let mut child = AudioCue::new();
        child.accept_preloaded_audio(
            Arc::new(vec![0.25; 16]),
            2,
            48_000,
            Duration::from_millis(1),
        );
        let mut group = GroupCue::new();
        group.children.push(Box::new(child));
        let before = group.serialize();
        let before_pcm = group.children[0].extract_decoded_audio().unwrap();

        assert!(classify_single_cue_update(&serde_json::json!({ "children": [] })).is_err());
        assert!(
            reject_group_rebuild(&group, &serde_json::json!({ "group_mode": "playlist" })).is_err()
        );
        assert!(reject_group_rebuild(&group, &serde_json::json!({ "number": "2" })).is_err());
        assert_eq!(
            group.serialize(),
            before,
            "rejection leaves the nested tree untouched"
        );
        let after_pcm = group.children[0].extract_decoded_audio().unwrap();
        assert!(
            Arc::ptr_eq(&before_pcm.0, &after_pcm.0),
            "nested decoded PCM remains owned by the original child"
        );
    }

    #[test]
    fn number_fade_rebuild_preserves_preloaded_nested_audio() {
        let mut list = CueList::new("test");
        let mut child = AudioCue::new();
        let child_id = child.id();
        let samples = Arc::new(vec![0.25; 32]);
        child.accept_preloaded_audio(
            samples.clone(),
            2,
            48_000,
            Duration::from_millis(1),
        );
        let mut number = GroupCue::new_number();
        let number_id = number.id();
        number.children.push(Box::new(child));
        number.set_number_master(child_id).unwrap();
        list.cues.push(Box::new(number));

        let prepared = prepare_all(
            &list,
            &[entry(number_id, serde_json::json!({ "fade_in_ms": 500 }))],
            &registry(),
        )
        .unwrap();
        apply_all(&mut list, prepared);

        let restored = list
            .get_recursive(&child_id)
            .and_then(|cue| cue.extract_decoded_audio())
            .expect("nested Number audio remains preloaded after fade edit");
        assert!(Arc::ptr_eq(&samples, &restored.0));
    }
}

/// Add a new cue of the given type at the given position (index).
/// Pass `position = -1` to append at the end.
#[tauri::command]
pub fn add_cue(
    cue_type: CueType,
    position: i64,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let cue = registry.create(&cue_type).map_err(|e| e.to_string())?;
    let id = cue.id().to_string();
    drop(registry);

    let mut cue = cue;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    // Cue colours are operator preferences, not an implicit property of the
    // cue class. Existing/imported cues keep their saved colour; only a newly
    // created cue receives the configured default for its type.
    apply_default_cue_color(cue.as_mut(), &ws.preferences.general);
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;

    // With auto-renumber off, push/insert won't number the new cue — assign the
    // next free number so it isn't blank (auto-renumber on: the resequence does it).
    if !cue_list.auto_renumber {
        cue.set_number(Some(cue_list.next_available_number()));
    }

    if position < 0 || position as usize >= cue_list.cues.len() {
        cue_list.push(cue);
    } else {
        cue_list.insert(position as usize, cue);
    }

    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(id)
}

/// The cue kinds whose action is explicitly directed at one or more other
/// cues.  This is deliberately kept on the backend as the authority: callers
/// cannot accidentally create a context-menu command which looks targeted in
/// the UI but still has the dangerous/default empty target set at runtime.
fn targeted_cue_label(cue_type: &CueType) -> Option<&'static str> {
    use crate::cue::control_cue::ControlAction;

    match cue_type {
        CueType::Stop => Some("Stop"),
        CueType::Fade => Some("Fade"),
        CueType::Devamp => Some("Devamp"),
        _ => ControlAction::from_cue_type(cue_type).map(ControlAction::label),
    }
}

fn targeted_cue_name(label: &str, target_number: Option<&str>, target_name: &str) -> String {
    std::iter::once(label)
        .chain(target_number.map(str::trim).filter(|value| !value.is_empty()))
        .chain(std::iter::once(target_name.trim()).filter(|value| !value.is_empty()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn default_media_cue_name(cue_type: &CueType) -> Option<&'static str> {
    match cue_type {
        CueType::Audio => Some("Audio Cue"),
        CueType::Video => Some("Video Cue"),
        CueType::Image => Some("Image Cue"),
        CueType::MidiFile => Some("MIDI File"),
        _ => None,
    }
}

fn generated_media_cue_name(cue_type: &CueType, file_path: &str) -> Option<String> {
    let type_name = match cue_type {
        CueType::Audio => "Audio",
        CueType::Video => "Video",
        CueType::Image => "Image",
        CueType::MidiFile => "MIDI",
        _ => return None,
    };
    let normalized_path = file_path.replace('\\', "/");
    let filename = normalized_path.rsplit('/').next()?.trim();
    if filename.is_empty() {
        return None;
    }
    let stem = match filename.rfind('.') {
        Some(extension_start) if extension_start > 0 => &filename[..extension_start],
        _ => filename,
    };
    if stem.is_empty() {
        return None;
    }
    Some(format!("{type_name} {stem}"))
}

fn should_replace_generated_name(
    current_name: &str,
    default_name: Option<&str>,
    previous_generated_name: Option<&str>,
) -> bool {
    current_name.trim().is_empty()
        || default_name.is_some_and(|name| current_name == name)
        || previous_generated_name.is_some_and(|name| current_name == name)
}

fn update_auto_media_name(
    serialized: &mut serde_json::Value,
    cue_type: &CueType,
    new_file_path: &str,
) {
    let current_name = serialized
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let old_file_path = serialized
        .get("file_path")
        .and_then(serde_json::Value::as_str);
    let previous_generated_name = old_file_path
        .and_then(|path| generated_media_cue_name(cue_type, path));
    let default_name = default_media_cue_name(cue_type);

    if should_replace_generated_name(
        current_name,
        default_name,
        previous_generated_name.as_deref(),
    ) {
        if let Some(name) = generated_media_cue_name(cue_type, new_file_path) {
            if let Some(object) = serialized.as_object_mut() {
                object.insert("name".to_string(), serde_json::Value::String(name));
            }
        }
    }
}

fn default_target_cue_name(cue_type: &CueType) -> Option<String> {
    if let Some(label) = targeted_cue_label(cue_type) {
        return Some(match cue_type {
            CueType::Stop => "Stop Cue".to_string(),
            CueType::Fade => "Fade".to_string(),
            CueType::Devamp => "Devamp Cue".to_string(),
            _ => format!("{label} Cue"),
        });
    }
    None
}

fn find_cue_by_number<'a>(
    cues: &'a [Box<dyn Cue>],
    number: &str,
) -> Option<&'a dyn Cue> {
    for cue in cues {
        if cue.number() == Some(number) {
            return Some(cue.as_ref());
        }
        if let Some(children) = cue.child_cues() {
            if let Some(found) = find_cue_by_number(children, number) {
                return Some(found);
            }
        }
    }
    None
}

fn generated_target_cue_name(
    cue_type: &CueType,
    serialized: &serde_json::Value,
    cues: &[Box<dyn Cue>],
) -> Option<String> {
    let label = targeted_cue_label(cue_type)?;
    let ids = cue_target_ids(serialized);
    let numbers = cue_target_numbers(serialized);
    let mut targets = Vec::<(Option<String>, String)>::new();
    let mut seen = HashSet::new();

    if !ids.is_empty() {
        for (position, id) in ids.iter().enumerate() {
            let parsed_id = id.parse::<Uuid>().ok();
            let target = parsed_id.and_then(|id| {
                fn find<'a>(cues: &'a [Box<dyn Cue>], id: &Uuid) -> Option<&'a dyn Cue> {
                    for cue in cues {
                        if cue.id() == *id {
                            return Some(cue.as_ref());
                        }
                        if let Some(children) = cue.child_cues() {
                            if let Some(found) = find(children, id) {
                                return Some(found);
                            }
                        }
                    }
                    None
                }
                find(cues, &id)
            });
            if let Some(target) = target {
                let key = id.clone();
                if seen.insert(key) {
                    targets.push((
                        target
                            .number()
                            .map(str::to_string)
                            .or_else(|| numbers.get(position).cloned()),
                        target.name().to_string(),
                    ));
                }
            } else if let Some(number) = numbers.get(position) {
                if seen.insert(format!("number:{number}")) {
                    targets.push((Some(number.clone()), String::new()));
                }
            }
        }
    } else {
        for number in &numbers {
            if !seen.insert(format!("number:{number}")) {
                continue;
            }
            if let Some(target) = find_cue_by_number(cues, number) {
                targets.push((
                    target.number().map(str::to_string),
                    target.name().to_string(),
                ));
            } else {
                targets.push((Some(number.clone()), String::new()));
            }
        }
    }

    match targets.as_slice() {
        [] => Some(label.to_string()),
        [(number, name)] => Some(targeted_cue_name(label, number.as_deref(), name)),
        many => {
            let suffix = many
                .iter()
                .map(|(number, name)| targeted_cue_name("", number.as_deref(), name))
                .filter(|target| !target.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            Some(if suffix.is_empty() {
                label.to_string()
            } else {
                format!("{label} {suffix}")
            })
        }
    }
}

fn targeted_cue_accepts_target(cue_type: &CueType, target_type: &CueType) -> bool {
    match cue_type {
        CueType::Fade => matches!(
            target_type,
            CueType::Audio | CueType::Video | CueType::Image | CueType::Camera | CueType::Group | CueType::Number
        ),
        CueType::Devamp => {
            matches!(target_type, CueType::Audio | CueType::Video | CueType::Group | CueType::Number)
        }
        _ => targeted_cue_label(cue_type).is_some(),
    }
}

/// Apply the configured colour to a cue that has just been created. Existing
/// cues are never revisited when preferences change.
fn apply_default_cue_color(cue: &mut dyn Cue, preferences: &GeneralPreferences) {
    let cue_type = cue.cue_type();
    cue.set_color(preferences.default_cue_color(&cue_type));
}

/// Synchronise target metadata only after insertion has run the cue list's
/// auto-renumber pass. The list stays exclusively locked throughout, so both
/// cue ids are invariants established immediately before this call.
fn finish_context_target(
    cue_list: &mut crate::show::cue_list::CueList,
    new_cue_id: Uuid,
    target_id: Uuid,
    label: &str,
) {
    let (target_number, target_name) = {
        let target = cue_list
            .get_recursive(&target_id)
            .expect("validated context target disappeared while list was locked");
        (target.number().map(str::to_string), target.name().to_string())
    };
    let cue = cue_list
        .get_mut_recursive(&new_cue_id)
        .expect("new context cue disappeared while list was locked");
    assert!(
        cue.set_context_target(target_id, target_number.clone()),
        "validated targeted cue type did not implement set_context_target"
    );
    cue.set_name(targeted_cue_name(label, target_number.as_deref(), &target_name));
}

/// Add a context-created command cue already bound to one specific cue.
///
/// Unlike [`add_cue`], this command is intentionally limited to cue kinds with
/// target fields.  It is used by a cue row's context menu; ordinary toolbar and
/// empty-list creation continue to call `add_cue` and keep their old defaults.
#[tauri::command]
pub fn add_targeted_cue(
    cue_type: CueType,
    target_cue_id: String,
    position: i64,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    let target_id: Uuid = target_cue_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;

    // Keep the established lock order used by duplicate/update: registry,
    // workspace, then undo stack.
    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let (label, mut cue) = {
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        let target = cue_list
            .get_recursive(&target_id)
            .ok_or("Target cue not found")?;
        let label = targeted_cue_label(&cue_type)
            .ok_or_else(|| format!("Cue type '{cue_type}' cannot target another cue"))?;
        if !targeted_cue_accepts_target(&cue_type, &target.cue_type()) {
            return Err(format!(
                "Cue type '{cue_type}' cannot target cue type '{}'",
                target.cue_type()
            ));
        }
        let mut cue = registry.create(&cue_type).map_err(|e| e.to_string())?;
        apply_default_cue_color(cue.as_mut(), &ws.preferences.general);
        (label, cue)
    };
    let id = cue.id();

    // Validation and construction above are fallible; only now create the one
    // undo entry and mark the workspace dirty, so rejected requests are atomic.
    {
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        let snapshot = super::undo_cmds::take_snapshot(cue_list);
        state
            .undo_stack
            .lock()
            .map_err(|e| e.to_string())?
            .push_action(snapshot);
    }
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    if !cue_list.auto_renumber {
        cue.set_number(Some(cue_list.next_available_number()));
    }
    if position < 0 || position as usize >= cue_list.cues.len() {
        cue_list.push(cue);
    } else {
        cue_list.insert(position as usize, cue);
    }
    // Insertion may resequence both cues. Read the target's final number and
    // update UUID, display number and generated name together in-place.
    finish_context_target(cue_list, id, target_id, label);
    drop(ws);
    drop(registry);

    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(id.to_string())
}

fn cue_tree_contains_id(cue: &dyn Cue, target: Uuid) -> bool {
    cue.id() == target
        || cue.child_cues().is_some_and(|children| {
            children
                .iter()
                .any(|child| cue_tree_contains_id(child.as_ref(), target))
        })
}

/// Stop a headphone preview when its cue is part of a tree being deleted.
pub(crate) fn stop_preview_if_owned_by_tree(
    state: &AppState,
    cue: &dyn Cue,
) -> Result<(), String> {
    let preview_cue_id = state
        .preview_session
        .lock()
        .map_err(|error| error.to_string())?
        .as_ref()
        .map(|session| session.cue_id);
    let Some(preview_cue_id) = preview_cue_id else {
        return Ok(());
    };
    if !cue_tree_contains_id(cue, preview_cue_id) {
        return Ok(());
    }

    // Invalidate an in-flight decode before detaching the session. The
    // compare-and-take keeps a newer preview from being stopped by a stale
    // deletion path.
    state.preview_generation.fetch_add(1, Ordering::SeqCst);
    if let Some(session) = take_preview_session_for_cue(&state.preview_session, preview_cue_id)? {
        for voice_id in session.all_voice_ids() {
            state.audio_engine.stop_preview_voice(*voice_id);
        }
    }
    Ok(())
}

/// Remove a cue by ID.
#[tauri::command]
pub fn remove_cue(
    cue_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    super::undo_cmds::push_current_snapshot(&state)?;
    let context = super::transport_cmds::make_context(&state, 0);
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    {
        let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
        let cue = cue_list
            .get_mut_recursive(&id)
            .ok_or_else(|| format!("Cue {id:?} not found"))?;
        stop_preview_if_owned_by_tree(&state, cue)?;
        crate::show::transport::hard_stop_cue_tree(&context, cue)
            .map_err(|e| e.to_string())?;
    }
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    cue_list.remove_anywhere(&id).map_err(|e| e.to_string())?;
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Move a cue to a new position.
#[tauri::command]
pub fn move_cue(
    cue_id: String,
    new_position: usize,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    cue_list
        .move_cue(&id, new_position)
        .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Remove multiple cues in one atomic operation.
#[tauri::command]
pub fn remove_cues(
    ids: Vec<String>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let ids: Vec<Uuid> = ids
        .iter()
        .map(|s| s.parse::<Uuid>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let mut ids = ids;
    ids.sort_unstable();
    ids.dedup();
    super::undo_cmds::push_current_snapshot(&state)?;
    let context = super::transport_cmds::make_context(&state, 0);
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    // Stop every selected tree while it is still reachable. This covers both
    // top-level groups and selected nested children, including duplicate IDs
    // in one request without relying on the UI to stop them first.
    {
        let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
        for id in &ids {
            let cue = cue_list
                .get_mut_recursive(id)
                .ok_or_else(|| format!("Cue {id:?} not found"))?;
            stop_preview_if_owned_by_tree(&state, cue)?;
            crate::show::transport::hard_stop_cue_tree(&context, cue)
                .map_err(|e| e.to_string())?;
        }
    }
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    cue_list
        .remove_many_anywhere(&ids)
        .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Move a group of cues immediately before `before_id`, or to the end if `None`.
#[tauri::command]
pub fn move_cues(
    ids: Vec<String>,
    before_id: Option<String>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let ids: Vec<Uuid> = ids
        .iter()
        .map(|s| s.parse::<Uuid>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let before_id: Option<Uuid> = before_id
        .as_deref()
        .map(|s| s.parse::<Uuid>().map_err(|e| e.to_string()))
        .transpose()?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    cue_list
        .move_before(&ids, before_id)
        .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Explicitly resequence every cue number in the active list (1, 2, 3…; nested
/// 3.1…). This is the on-demand "Renumber All Cues" action; it runs regardless
/// of the auto-renumber preference and is undoable.
#[tauri::command]
pub fn renumber_cues(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    cue_list.renumber_all();
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Resequence only the selected cues, starting at `start` and stepping by
/// `increment` (QLab's "Renumber Selected Cues").  Surrounding cues keep
/// their numbers.
#[tauri::command]
pub fn renumber_selected_cues(
    ids: Vec<String>,
    start: f64,
    increment: f64,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let parsed: Vec<Uuid> = ids
        .iter()
        .map(|s| s.parse::<Uuid>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    if parsed.is_empty() {
        return Ok(());
    }
    super::undo_cmds::push_current_snapshot(&state)?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    cue_list.renumber_selected(&parsed, start, increment);
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Clear every cue number in the active list.
#[tauri::command]
pub fn clear_cue_numbers(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    cue_list.clear_all_numbers();
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Duplicate a cue (creates a copy with a new ID, inserted immediately after).
#[tauri::command]
pub fn duplicate_cue(
    cue_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    // When a command needs both locks it takes registry → workspace (then
    // loading_cues → undo_stack when those are needed). Never acquire the
    // registry while holding the workspace.
    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;

    let (json, preserved_audio) = {
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
        let mut j = cue.serialize();
        // Assign a new UUID to the copy.
        j["id"] = serde_json::json!(Uuid::new_v4().to_string());
        // Transfer decoded audio so the copy is playable immediately,
        // without requiring a background re-decode.
        let audio = cue.extract_decoded_audio();
        (j, audio)
    };

    let mut new_cue = registry.from_json(json).map_err(|e| e.to_string())?;
    if let Some((samples, channels, sample_rate, duration)) = preserved_audio {
        new_cue.accept_preloaded_audio(samples, channels, sample_rate, duration);
    }
    let new_id = new_cue.id().to_string();

    // The registry is no longer needed; release it before history work so a
    // slow undo-stack consumer never blocks cue construction.
    drop(registry);

    // Replacement construction and all target lookup are complete before the
    // one undo snapshot. A failure above therefore cannot create history or
    // dirty the workspace.
    let snapshot = {
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        super::undo_cmds::take_snapshot(cue_list)
    };
    state
        .undo_stack
        .lock()
        .map_err(|e| e.to_string())?
        .push_action(snapshot);

    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    // Insert the copy right after its source, wherever it lives — so duplicating
    // a cue nested in a group keeps the copy in that same group.
    cue_list
        .insert_after_anywhere(&id, new_cue)
        .map_err(|e| e.to_string())?;

    drop(ws);
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(new_id)
}

/// Duplicate multiple cues, inserting each copy immediately after its own
/// source (so a copy of a cue nested in a group stays in that group).
#[tauri::command]
pub fn duplicate_cues(
    ids: Vec<String>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<Vec<String>, String> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let ids: Vec<Uuid> = ids
        .iter()
        .map(|s| s.parse::<Uuid>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;

    // Acquire the global order registry → workspace. This command does not
    // need loading_cues; it releases the registry as soon as every copy is
    // built, before taking the undo_stack lock.
    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;

    // Collect serialised copies (paired with their source ID) and preserved
    // audio for each source cue.  Recursive lookup so group children duplicate.
    let copies = {
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        ids.iter()
            .map(|id| {
                let cue = cue_list
                    .get_recursive(id)
                    .ok_or_else(|| format!("Cue {id:?} not found"))?;
                let mut j = cue.serialize();
                j["id"] = serde_json::json!(Uuid::new_v4().to_string());
                let audio = cue.extract_decoded_audio();
                Ok((*id, j, audio))
            })
            .collect::<Result<Vec<_>, String>>()?
    };

    // Build every replacement while validation can still fail, before making
    // the operation visible to undo/history or the workspace.
    let mut prepared = Vec::with_capacity(copies.len());
    for (src_id, json, audio) in copies {
        let mut new_cue = registry.from_json(json).map_err(|e| e.to_string())?;
        if let Some((samples, channels, sample_rate, duration)) = audio {
            new_cue.accept_preloaded_audio(samples, channels, sample_rate, duration);
        }
        prepared.push((src_id, new_cue));
    }
    drop(registry);

    let snapshot = {
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        super::undo_cmds::take_snapshot(cue_list)
    };
    state
        .undo_stack
        .lock()
        .map_err(|e| e.to_string())?
        .push_action(snapshot);

    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let mut new_ids = Vec::with_capacity(prepared.len());
    for (src_id, new_cue) in prepared {
        new_ids.push(new_cue.id().to_string());
        cue_list
            .insert_after_anywhere(&src_id, new_cue)
            .map_err(|e| e.to_string())?;
    }
    drop(ws);
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(new_ids)
}

/// Update cue properties from a partial JSON object.
///
/// All fields present in `properties` are merged into the cue's serialised
/// form and the cue is rebuilt via the [`CueRegistry`].  This correctly
/// handles both generic trait fields (name, number, …) and type-specific
/// fields (volume_db, pan, fade_in_ms, …) without any unsafe downcasting.
#[tauri::command]
pub fn update_cue(
    cue_id: String,
    properties: serde_json::Value,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let update_kind = classify_single_cue_update(&properties)?;

    match update_kind {
        SingleCueUpdateKind::Direct(patch) => {
            // This branch deliberately never obtains the registry or replaces
            // the cue. It is safe for a running Camera/Audio/Video cue because
            // its runtime workers and media handles remain owned by the same
            // object.
            let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
            {
                let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
                cue_list.get_recursive(&id).ok_or("Cue not found")?;
                let snapshot = super::undo_cmds::take_snapshot(cue_list);
                state
                    .undo_stack
                    .lock()
                    .map_err(|e| e.to_string())?
                    .push_action(snapshot);
            }
            ws.mark_modified();
            let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
            let cue = cue_list.get_mut_recursive(&id).ok_or("Cue not found")?;
            apply_single_direct_patch(cue, patch);
        }
        SingleCueUpdateKind::Rebuild => {
            // Lock order: registry first, then workspace (matches duplicate_cue).
            let registry = state.registry.lock().map_err(|e| e.to_string())?;
            let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
            // Snapshot the patch table before borrowing the cue list: a live matrix
            // edit needs the same channel mapping the GO path used.
            let patch_channels_by_id: Vec<(Uuid, Vec<u16>)> = ws
                .output_patches
                .iter()
                .map(|p| (p.id, p.channels.clone()))
                .collect();
            let default_patch_id = ws.default_output_patch_id;

            let in_place_patch = {
                let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
                let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
                let patch = classify_live_in_place_patch(&properties, cue.cue_type())?;
                if patch.is_none() {
                    reject_group_rebuild(cue, &properties)?;
                }
                patch
            };

            if let Some(patch) = in_place_patch {
                {
                    let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
                    let snapshot = super::undo_cmds::take_snapshot(cue_list);
                    state
                        .undo_stack
                        .lock()
                        .map_err(|e| e.to_string())?
                        .push_action(snapshot);
                }
                ws.mark_modified();
                let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
                let cue = cue_list.get_mut_recursive(&id).ok_or("Cue not found")?;
                apply_single_in_place_patch(cue, patch);
                apply_existing_cue_live_side_effects(
                    cue,
                    &state,
                    &patch_channels_by_id,
                    default_patch_id,
                );
            } else {
                let new_cue = {
                    let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
                    let loading = state.loading_cues.lock().map_err(|e| e.to_string())?;
                    let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
                    // Any serialise/rebuild edit can discard a live worker. Do not
                    // special-case fades here: all non-direct fields are guarded by
                    // the same authoritative lifecycle predicate.
                    ensure_rebuild_safe(cue, loading.contains(&id), "properties")?;

                    // Construct the replacement before snapshotting as well. This
                    // makes malformed type-specific data just as atomic as a
                    // lifecycle rejection: no mutation, Undo entry, or dirty bit.
                    let mut json = cue.serialize();
                    let old_file_path = json
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let cue_type = cue.cue_type();
                    let target_name_update = properties.as_object().is_some_and(|patch| {
                        !patch.contains_key("name")
                            && [
                                "target_cue_ids",
                                "target_cue_numbers",
                                "target_cue_id",
                                "target_cue_number",
                            ]
                            .iter()
                            .any(|key| patch.contains_key(*key))
                    }) && targeted_cue_label(&cue_type).is_some();
                    let old_generated_target_name = target_name_update
                        .then(|| generated_target_cue_name(&cue_type, &json, &cue_list.cues))
                        .flatten();
                    let should_rename_target = target_name_update
                        && should_replace_generated_name(
                            cue.name(),
                            default_target_cue_name(&cue_type).as_deref(),
                            old_generated_target_name.as_deref(),
                        );
                    let target = json
                        .as_object_mut()
                        .ok_or("Cue serialisation is not an object")?;
                    let src = properties.as_object().expect("classified object patch");
                    for (key, value) in src {
                        target.insert(key.clone(), value.clone());
                    }
                    let new_file_path = json
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let mut preserved_audio = HashMap::new();
                    if old_file_path == new_file_path {
                        collect_decoded_audio_recursive(cue, &mut preserved_audio);
                    }
                    if should_rename_target {
                        if let Some(name) =
                            generated_target_cue_name(&cue_type, &json, &cue_list.cues)
                        {
                            json["name"] = serde_json::Value::String(name);
                        }
                    }
                    let runtime = cue.runtime_state();
                    let mut new_cue = registry.from_json(json).map_err(|e| e.to_string())?;
                    restore_decoded_audio_recursive(new_cue.as_mut(), &preserved_audio);
                    new_cue.restore_runtime_state(runtime);
                    new_cue
                };

                {
                    let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
                    // Safety validation is intentionally complete before this
                    // snapshot. A rejected update must not dirty the project or
                    // add an Undo entry.
                    let snapshot = super::undo_cmds::take_snapshot(cue_list);
                    state
                        .undo_stack
                        .lock()
                        .map_err(|e| e.to_string())?
                        .push_action(snapshot);
                }
                ws.mark_modified();
                let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
                cue_list.replace_cue_recursive(&id, new_cue);
                let cue = cue_list
                    .get_recursive(&id)
                    .ok_or("Cue disappeared during update")?;
                apply_existing_cue_live_side_effects(
                    cue,
                    &state,
                    &patch_channels_by_id,
                    default_patch_id,
                );
            }
        }
    }

    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Change an authored cue action duration without serialising/rebuilding the
/// cue. This deliberately accepts the summary's `duration_ms` value rather
/// than a type-specific JSON key, then maps it to the owning cue field.
///
/// Only Wait, Fade, Image, Text and Light cues own an editable action
/// duration. `None` means hold indefinitely and is valid only for Image/Text.
/// The target may be nested in a Group; the Group itself is never editable.
#[tauri::command]
pub fn set_cue_duration(
    cue_id: String,
    duration_ms: Option<u64>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<CueSummary, String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;

    // Validate every failure case before changing the workspace or capturing
    // an Undo snapshot. The predicate is shared with guarded rebuild edits:
    // changing a running cue's timing is semantically unsafe even though this
    // command preserves its object and media handles.
    {
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        let loading = state.loading_cues.lock().map_err(|e| e.to_string())?;
        let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
        validate_user_action_duration(cue, duration_ms)?;
        ensure_rebuild_safe(cue, loading.contains(&id), "duration")?;
    }

    {
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        let snapshot = super::undo_cmds::take_snapshot(cue_list);
        state
            .undo_stack
            .lock()
            .map_err(|e| e.to_string())?
            .push_action(snapshot);
    }

    {
        let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
        let cue = cue_list
            .get_mut_recursive(&id)
            .ok_or("Cue disappeared during duration update")?;
        // `validate_user_action_duration` establishes the selected cue's
        // setter contract above, so this cannot leave a partial mutation.
        apply_user_action_duration(cue, duration_ms)?;
    }
    ws.mark_modified();

    let ws_dir = ws
        .file_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_owned());
    let patch_table = PatchTable {
        patches: &ws.output_patches,
        default_id: ws.default_output_patch_id,
    };
    let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
    let cue = cue_list
        .get_recursive(&id)
        .ok_or("Cue disappeared during duration update")?;
    let metadata = state
        .media_metadata
        .lock()
        .map_err(|e| e.to_string())?
        .snapshot();
    let loading = state.loading_cues.lock().map_err(|e| e.to_string())?;
    let mut metadata_requests = HashSet::new();
    let mut target_index = CueTargetIndex::default();
    index_cue_targets(&cue_list.cues, &mut target_index);
    let summary = summarise(
        cue,
        ws_dir.as_deref(),
        &patch_table,
        &metadata,
        &target_index,
        &mut metadata_requests,
        &loading,
    );

    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(summary)
}

/// Set the Playhead to a specific cue.
#[tauri::command]
pub fn set_playhead(
    cue_id: Option<String>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let id: Option<Uuid> = cue_id
        .as_deref()
        .map(|s| s.parse::<Uuid>().map_err(|e| e.to_string()))
        .transpose()?;

    // Selecting another cue leaves no hidden headphone audition playing.
    // This is independent of show transport state and never touches program
    // voices.
    stop_active_preview(&state)?;

    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    cue_list.set_playhead(id).map_err(|e| e.to_string())?;
    // The resulting outer Playhead may differ from `id` (a group child parks the
    // Playhead on its ancestor group), so emit the actual outer Playhead.
    let outer_playhead = cue_list.playhead_cue_id;

    let _ = app_handle.emit(
        "playhead-moved",
        serde_json::json!({ "cue_id": outer_playhead.map(|u| u.to_string()) }),
    );
    // Only when the target was a nested cue (resolved to a different outer
    // Playhead) refresh cues so the group's inner playhead (active_child_id)
    // updates — top-level clicks don't need the extra round-trip.
    if id != outer_playhead {
        let _ = app_handle.emit("cue-list-refresh", serde_json::json!({}));
    }
    Ok(())
}

/// Return the current Playhead cue ID (or null).
#[tauri::command]
pub fn get_playhead(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
    Ok(cue_list.playhead_cue_id.map(|u| u.to_string()))
}

// ---------------------------------------------------------------------------
// Waveform
// ---------------------------------------------------------------------------

/// Downsampled peak data returned by [`get_waveform_peaks`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveformData {
    /// Peak values (0.0 – 1.0), one per bin.
    pub peaks: Vec<f32>,
    /// RMS values (0.0 – 1.0), one per bin — the "body" of the sound, drawn
    /// inside the peak envelope for a two-tone DAW-style waveform.
    pub rms: Vec<f32>,
    /// Full file duration in seconds (ignoring start/end markers).
    pub file_duration_s: f64,
}

/// Downsample interleaved samples into per-bin peak + RMS values.
fn compute_waveform_bins(samples: &[f32], channels: usize, bins: usize) -> (Vec<f32>, Vec<f32>) {
    let total_frames = samples.len().checked_div(channels.max(1)).unwrap_or(0);
    if bins == 0 || total_frames == 0 {
        return (vec![], vec![]);
    }
    let mut peaks = Vec::with_capacity(bins);
    let mut rms = Vec::with_capacity(bins);
    for i in 0..bins {
        let start = (i * total_frames) / bins;
        let end = (((i + 1) * total_frames) / bins)
            .max(start + 1)
            .min(total_frames);
        let mut peak = 0.0f32;
        let mut sum_sq = 0.0f64;
        let mut count = 0usize;
        for frame in start..end {
            for ch in 0..channels {
                let v = samples[frame * channels + ch];
                let a = v.abs();
                if a > peak {
                    peak = a;
                }
                sum_sq += (v as f64) * (v as f64);
                count += 1;
            }
        }
        peaks.push(peak);
        rms.push(if count > 0 {
            (sum_sq / count as f64).sqrt() as f32
        } else {
            0.0
        });
    }
    (peaks, rms)
}

/// Return waveform peak data for an audio cue.
///
/// `bins` controls the number of columns (typically 400–800 for UI use).
/// Returns an error if the cue has not been decoded yet.
#[tauri::command]
pub fn get_waveform_peaks(
    cue_id: String,
    bins: usize,
    state: State<'_, AppState>,
) -> Result<WaveformData, String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;

    // Hold the workspace lock only long enough to clone decoded samples and the
    // source path. Number children can be moved into a Group without passing
    // through the normal top-level preload queue, so a nested media cue may not
    // have PCM in memory yet. Decode that source after releasing the lock below.
    let (decoded, file_path) = {
        let ws = state.workspace.lock().map_err(|e| e.to_string())?;
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
        (cue.extract_decoded_audio(), cue.media_file_path().map(std::path::Path::to_path_buf))
        // workspace lock dropped here
    };

    let (samples, channels, _sample_rate, file_duration) = match decoded {
        Some(decoded) => decoded,
        None => {
            let path = file_path.ok_or("Audio not loaded yet — assign a file first")?;
            let (samples, channels, sample_rate) = crate::cue::media_decode::decode_audio_track_legacy(&path)
                .map_err(|error| format!("Could not decode audio track: {error}"))?
                .ok_or("Media has no audio track")?;
            let duration = std::time::Duration::from_secs_f64(
                samples.len() as f64 / channels.max(1) as f64 / sample_rate.max(1) as f64,
            );
            (std::sync::Arc::new(samples), channels, sample_rate, duration)
        }
    };

    // Compute peaks + RMS outside the lock.
    let (peaks, rms) = compute_waveform_bins(&samples, channels as usize, bins);

    Ok(WaveformData {
        peaks,
        rms,
        file_duration_s: file_duration.as_secs_f64(),
    })
}

/// Push a level change straight to a playing cue's voice, **without** touching
/// the workspace or the undo stack.
///
/// This is the drag path: an inspector slider calls it continuously while it
/// moves, then persists once on release via `update_cue`. Routing every drag
/// step through `update_cue` instead would re-serialise the cue and push an
/// undo snapshot per pixel.
///
/// A no-op when the cue is not playing.
#[tauri::command]
pub fn set_live_level(
    cue_id: String,
    volume_db: f64,
    pan: f32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let Some(cue_list) = ws.active_cue_list() else {
        return Ok(());
    };
    let Some(cue) = cue_list.get_recursive(&id) else {
        return Ok(());
    };
    let Some(params) = cue.live_audio_params() else {
        return Ok(());
    };

    // A Video Cue reports its visual voice; the sound is on the paired one.
    let voice = state
        .output_engine
        .video_audio_voice(params.voice_id)
        .unwrap_or(params.voice_id);
    let _ = state
        .audio_engine
        .set_voice_gain(voice, crate::cue::types::db_to_linear(volume_db) as f32);
    let _ = state.audio_engine.set_voice_pan(voice, pan);
    Ok(())
}

/// Push one crosspoint of a playing cue's level matrix, live and without an
/// undo snapshot — the matrix-grid equivalent of [`set_live_level`].
///
/// Sends a single cell rather than the whole matrix: dragging one cell would
/// otherwise flood the real-time ring buffer with 32 commands per step.
#[tauri::command]
pub fn set_live_crosspoint(
    cue_id: String,
    input: u8,
    output: u8,
    gain_db: f64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let Some(cue_list) = ws.active_cue_list() else {
        return Ok(());
    };
    let Some(cue) = cue_list.get_recursive(&id) else {
        return Ok(());
    };
    let Some(params) = cue.live_audio_params() else {
        return Ok(());
    };

    // Resolve the column onto the patch channel, exactly as the GO path does.
    let patch_channels: Vec<u16> = cue
        .output_patch_id()
        .or(ws.default_output_patch_id)
        .and_then(|pid| ws.output_patches.iter().find(|p| p.id == pid))
        .map(|p| p.channels.clone())
        .unwrap_or_default();
    let device = patch_channels
        .get(output as usize)
        .map(|&c| c as u8)
        .unwrap_or(output);

    let voice = state
        .output_engine
        .video_audio_voice(params.voice_id)
        .unwrap_or(params.voice_id);
    state
        .audio_engine
        .set_voice_crosspoint(
            voice,
            input,
            device,
            crate::cue::types::db_to_linear(gain_db) as f32,
        )
        .map_err(|e| e.to_string())
}

/// Compute the `volume_db` that normalises this audio cue's peak to 0 dBFS.
///
/// Reads the already-decoded samples (non-destructively via `Arc::clone`),
/// finds the absolute peak, and returns `20 × log10(1 / peak)` — the gain
/// the fader must be set to so the loudest sample plays at exactly 0 dBFS.
///
/// Errors if the audio has not been decoded yet or if the file is silent.
#[tauri::command]
pub fn get_normalize_db(cue_id: String, state: State<'_, AppState>) -> Result<f64, String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;

    let samples = {
        let ws = state.workspace.lock().map_err(|e| e.to_string())?;
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
        cue.extract_decoded_audio()
            .ok_or("Audio not loaded yet — open the file first")?
            .0
        // workspace lock released here
    };

    let peak: f32 = samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);

    if peak < 1e-6 {
        return Err("File is silent — cannot normalize".into());
    }

    let normalize_db = (1.0_f64 / peak as f64).log10() * 20.0;
    Ok(normalize_db.clamp(-60.0, 12.0))
}

/// Stop `cue` first when it is currently running or paused.
///
/// The file setters replace the cue object wholesale (serialise → rebuild), so
/// a live cue would otherwise lose its `active_voice_id` and its engine voice
/// would keep playing with no owner — unreachable by Stop or even Hard Stop
/// (the rebuilt cue reports Standby, so the transport skips it).  Uses the
/// soft stop so the operator hears the normal fade-out; the engine completes
/// the fade autonomously after the cue object is replaced.
fn stop_if_live(cue: &mut dyn Cue, state: &AppState, stop_fade_ms: u32) {
    if cue.is_running() || cue.is_paused() {
        let context = super::transport_cmds::make_context(state, stop_fade_ms);
        let _ = cue.stop(&context);
    }
}

/// Swap `file_path` into a serialized cue and, when the path actually
/// changed, reset the clip window — start/end times and slice markers
/// describe positions in the *old* media and break playback on a shorter
/// file (start time past EOF = silent audio; ab-loop segments past the end
/// = video looping with Loop unchecked).  The cached duration is cleared
/// too so a stale length never outlives the file it measured.
fn set_file_path_resetting_clip(json: &mut serde_json::Value, file_path: &str) {
    let Some(obj) = json.as_object_mut() else {
        return;
    };
    let changed = obj.get("file_path").and_then(|v| v.as_str()) != Some(file_path);
    obj.insert("file_path".to_string(), serde_json::json!(file_path));
    if changed {
        obj.insert("start_time_ms".to_string(), serde_json::Value::Null);
        obj.insert("end_time_ms".to_string(), serde_json::Value::Null);
        obj.insert(
            "slices".to_string(),
            serde_json::json!({ "markers": [], "play_counts": [1] }),
        );
        obj.insert("cached_duration_ms".to_string(), serde_json::Value::Null);
    }
}

/// Set the file path of an audio cue.
/// Uses the same JSON-merge-and-rebuild strategy as [`update_cue`].
#[tauri::command]
pub fn set_audio_file(
    cue_id: String,
    file_path: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;

    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let workspace_dir = ws
        .file_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(|path| path.to_owned());
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;

    let json = {
        let cue = cue_list.get_mut_recursive(&id).ok_or("Cue not found")?;
        if cue.cue_type() != CueType::Audio {
            return Err("set_audio_file only applies to Audio Cues".to_string());
        }
        stop_if_live(cue, &state, stop_fade_ms);
        let mut json = cue.serialize();
        update_auto_media_name(&mut json, &CueType::Audio, &file_path);
        set_file_path_resetting_clip(&mut json, &file_path);
        json
    };
    let new_cue = registry.from_json(json).map_err(|e| e.to_string())?;
    drop(registry);
    cue_list.replace_cue_recursive(&id, new_cue);
    // Mark as loading before dropping the workspace lock.
    {
        let mut loading = state.loading_cues.lock().map_err(|e| e.to_string())?;
        loading.insert(id);
    }

    drop(ws); // release workspace lock immediately — do NOT decode while locked
    invalidate_media_metadata_path(
        &state,
        std::path::Path::new(&file_path),
        workspace_dir.as_deref(),
    );

    // Notify the frontend: the assigned basename appears in File, and loading state is refreshed.
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));

    // Spawn a background metadata probe and start a bounded stream. The
    // workspace mutex is only held while storing metadata, never during I/O.
    let workspace = state.workspace.clone();
    let loading_cues = state.loading_cues.clone();
    let app_handle2 = app_handle.clone();
    let file_path_buf = std::path::PathBuf::from(file_path);

    std::thread::Builder::new()
        .name("inkue-preload".into())
        .spawn(move || {
            // Run at below-normal OS priority so decoding never starves the
            // audio callback thread or causes fan spin-up on the host machine.
            // The decode is I/O + CPU bound; at BELOW_NORMAL it finishes in the
            // same wall-clock time when the system is otherwise idle, but yields
            // automatically under load.
            #[cfg(windows)]
            // SAFETY: only changes the scheduling priority of this thread.
            unsafe {
                use std::os::raw::c_void;
                extern "system" {
                    fn GetCurrentThread() -> *mut c_void;
                    fn SetThreadPriority(h_thread: *mut c_void, n_priority: i32) -> i32;
                }
                SetThreadPriority(GetCurrentThread(), -1); // THREAD_PRIORITY_BELOW_NORMAL
            }

            match crate::cue::media_decode::probe_audio_track(&file_path_buf) {
                Ok(Some(info)) => {
                    let duration = info.total_frames.map(|frames| std::time::Duration::from_secs_f64(
                        frames as f64 / info.sample_rate.max(1) as f64,
                    ));
                    // Brief lock: store the stream metadata in whichever list contains the cue.
                    // We search all lists (not just the active one) because the user may
                    // have switched cue lists while the background decode was running.
                    if let Ok(mut ws) = workspace.lock() {
                        'store: {
                            for cl in ws.cue_lists.iter_mut() {
                                if let Some(cue) = cl.get_mut(&id) {
                                    cue.accept_preloaded_stream(
                                        file_path_buf.clone(),
                                        info.channels,
                                        info.sample_rate,
                                        duration,
                                    );
                                    break 'store;
                                }
                            }
                        }
                    }
                    if let Ok(mut loading) = loading_cues.lock() {
                        loading.remove(&id);
                    }
                    // Decoded fine — retire any prior decode-failure banner.
                    super::clear_decode_failure(id);
                    // Update duration column in the UI.
                    let _ = app_handle2.emit("workspace-modified", serde_json::json!({}));
                }
                Ok(None) => {
                    if let Ok(mut loading) = loading_cues.lock() {
                        loading.remove(&id);
                    }
                    let e = anyhow::anyhow!("Media has no audio track: {}", file_path_buf.display());
                    log::warn!("Background preload failed for {:?}: {e}", file_path_buf);
                    super::surface_decode_failure(id, &file_path_buf);
                    let _ = app_handle2.emit("workspace-modified", serde_json::json!({}));
                    let _ = app_handle2.emit(
                        "cue-load-error",
                        serde_json::json!({
                            "cue_id": id.to_string(),
                            "error": e.to_string(),
                        }),
                    );
                }
                Err(e) => {
                    if let Ok(mut loading) = loading_cues.lock() {
                        loading.remove(&id);
                    }
                    log::warn!("Background preload failed for {:?}: {e}", file_path_buf);
                    // Persistent banner (the event above is transient) so a cue
                    // that will silently no-op at GO stays visible until fixed.
                    super::surface_decode_failure(id, &file_path_buf);
                    let _ = app_handle2.emit("workspace-modified", serde_json::json!({}));
                    let _ = app_handle2.emit(
                        "cue-load-error",
                        serde_json::json!({
                            "cue_id": id.to_string(),
                            "error": e.to_string(),
                        }),
                    );
                }
            }
        })
        .expect("Failed to spawn preload thread");

    Ok(())
}

// ---------------------------------------------------------------------------
// Headphone preview (plays a temporary voice without touching cue state)
// ---------------------------------------------------------------------------

struct PreviewSource {
    samples: Arc<Vec<f32>>,
    channels: u16,
    sample_rate: u32,
    volume_db: f64,
    pan: f32,
    trim_start_ms: Option<u64>,
    trim_end_ms: Option<u64>,
    rate: f32,
    loop_count: u32,
}

/// Resolve the media cue that owns the audio for a headphone preview.
///
/// A Number is a transport container, so the preview session keeps the
/// Number's id while the voice itself is built from its configured master.
/// Keeping those identities separate means the UI can toggle/stop the Number
/// and deleting the Number still tears down its preview session correctly.
fn preview_target_cue<'a>(cue: &'a dyn Cue) -> Result<&'a dyn Cue, String> {
    if cue.cue_type() != CueType::Number {
        if matches!(cue.cue_type(), CueType::Audio | CueType::Video) {
            return Ok(cue);
        }
        return Err("Headphone preview is available only for Audio, Video, or a Number with an Audio/Video master".to_string());
    }

    let children = cue
        .child_cues()
        .ok_or_else(|| "Number needs a master Audio or Video cue".to_string())?;
    let master_id = cue
        .number_master_id()
        .ok_or_else(|| "Number needs a master Audio or Video cue".to_string())?;
    let master = children
        .iter()
        .find(|child| child.id() == master_id)
        .map(|child| child.as_ref())
        .ok_or_else(|| "Number master cue no longer exists".to_string())?;
    if !matches!(master.cue_type(), CueType::Audio | CueType::Video) {
        return Err("Number master must be an Audio or Video cue".to_string());
    }
    if master.is_disabled() {
        return Err("Number master is disabled".to_string());
    }
    Ok(master)
}

/// Build an independent preview voice. `position_ms` is always file time, not
/// elapsed cue time; trim markers constrain it so an audition cannot run into
/// media excluded from the cue.
fn build_preview_voice(
    source: PreviewSource,
    position_ms: Option<u64>,
    requested_end_ms: Option<u64>,
) -> Result<Voice, String> {
    let sample_rate = source.sample_rate.max(1);
    let total_frames = source.samples.len() as u64 / source.channels.max(1) as u64;
    let total_ms = total_frames.saturating_mul(1000) / sample_rate as u64;
    let trim_start_ms = source.trim_start_ms.unwrap_or(0).min(total_ms);
    let start_ms = position_ms
        .unwrap_or(trim_start_ms)
        .max(trim_start_ms)
        .min(total_ms);
    let trim_end_ms = source.trim_end_ms.unwrap_or(total_ms).min(total_ms);
    let end_ms = requested_end_ms.unwrap_or(trim_end_ms).min(trim_end_ms);
    if start_ms >= end_ms {
        return Err("Preview position is outside this cue's clip range".to_string());
    }

    let voice = Voice::new(
        source.samples,
        source.channels.max(1),
        sample_rate,
        crate::cue::types::db_to_linear(source.volume_db) as f32,
        source.pan.clamp(-1.0, 1.0),
    );
    // `fill_buffer` already performs source/output sample-rate conversion, so
    // rate here is only the cue's user-selected rate multiplier.
    voice.inner.set_rate(source.rate.clamp(0.1, 4.0));
    voice
        .inner
        .loops_remaining
        .store(source.loop_count, Ordering::Relaxed);
    voice.frame_pos.store(
        start_ms.saturating_mul(sample_rate as u64) / 1000,
        Ordering::Relaxed,
    );
    // SAFETY: the marker is written before the voice reaches an aux callback.
    unsafe {
        *voice.inner.end_frame.get() = Some(end_ms.saturating_mul(sample_rate as u64) / 1000);
    }
    Ok(voice)
}

/// Get preloaded Audio/Video PCM or decode the cue's media audio on demand.
/// Video is intentionally decoded through `AudioEngine`'s normal voice path;
/// no video output is created for a headphone preview.
fn preview_source_from_parts(
    decoded: Option<(Arc<Vec<f32>>, u16, u32, std::time::Duration)>,
    file_path: Option<std::path::PathBuf>,
    cue_type: CueType,
    json: &serde_json::Value,
) -> Result<PreviewSource, String> {
    let volume_db = json
        .get("volume_db")
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    let pan = json
        .get("pan")
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0) as f32;
    let trim_start_ms = json.get("start_time_ms").and_then(|value| value.as_u64());
    let trim_end_ms = json.get("end_time_ms").and_then(|value| value.as_u64());
    let rate = json
        .get("rate")
        .and_then(|value| value.as_f64())
        .unwrap_or(1.0) as f32;
    let (samples, channels, sample_rate) = match decoded {
        Some((samples, channels, sample_rate, _)) => (samples, channels, sample_rate),
        None => {
            let path = file_path.ok_or("No media file is assigned to this cue")?;
            crate::cue::media_decode::decode_audio_track_legacy(&path)
                .map_err(|error| format!("Could not decode preview audio: {error}"))?
                .map(|(samples, channels, sample_rate)| (Arc::new(samples), channels, sample_rate))
                .ok_or_else(|| {
                    format!(
                        "{} cue has no audio track",
                        match cue_type {
                            CueType::Video => "Video",
                            _ => "Audio",
                        }
                    )
                })?
        }
    };

    Ok(PreviewSource {
        samples,
        channels,
        sample_rate,
        volume_db,
        pan,
        trim_start_ms,
        trim_end_ms,
        rate,
        loop_count: json
            .get("loop_count")
            .and_then(|value| value.as_u64())
            .map(|value| value.min(u32::MAX as u64) as u32)
            .unwrap_or(0),
    })
}

fn preview_source(cue_id: Uuid, state: &AppState) -> Result<PreviewSource, String> {
    let (decoded, file_path, cue_type, json) = {
        let ws = state.workspace.lock().map_err(|e| e.to_string())?;
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        let cue = cue_list.get_recursive(&cue_id).ok_or("Cue not found")?;
        let source_cue = preview_target_cue(cue)?;
        (
            source_cue.extract_decoded_audio(),
            source_cue.media_file_path().map(|path| path.to_path_buf()),
            source_cue.cue_type(),
            source_cue.serialize(),
        )
    };
    preview_source_from_parts(decoded, file_path, cue_type, &json)
}

/// One audio-bearing Number child and its position on the Number clock.
struct NumberPreviewSource {
    source: PreviewSource,
    offset_ms: u64,
    is_master: bool,
}

/// Pure timing/gating rule shared by Number preview selection and its tests.
/// A child is audible at the Number clock only after its authored offset and
/// only while its cue is enabled and not effectively muted.
fn number_preview_child_is_active(
    cue_type: &CueType,
    is_disabled: bool,
    volume_db: f64,
    offset_ms: u64,
    clock_ms: u64,
) -> bool {
    matches!(cue_type, CueType::Audio | CueType::Video)
        && !is_disabled
        && volume_db > -59.0
        && offset_ms <= clock_ms
}

fn number_source_elapsed_ms(source: &PreviewSource, elapsed_ms: u64) -> Option<u64> {
    let total_frames = source.samples.len() as u64 / source.channels.max(1) as u64;
    let total_ms = total_frames.saturating_mul(1000) / source.sample_rate.max(1) as u64;
    let start = source.trim_start_ms.unwrap_or(0).min(total_ms);
    let end = source.trim_end_ms.unwrap_or(total_ms).min(total_ms).max(start);
    let span = end.saturating_sub(start);
    if span == 0 {
        return None;
    }
    let rate = source.rate.clamp(0.1, 4.0) as f64;
    let progressed = (elapsed_ms as f64 * rate).round() as u64;
    let total_action_ms = if source.loop_count == u32::MAX {
        u64::MAX
    } else {
        ((span as f64 / rate).ceil() as u64).saturating_mul(source.loop_count as u64 + 1)
    };
    if progressed >= total_action_ms {
        return None;
    }
    let source_offset = if source.loop_count == 0 {
        progressed.min(span)
    } else {
        progressed % span
    };
    Some(start.saturating_add(source_offset).min(end))
}

/// Collect every direct Number child that has an enabled audio track. The
/// first entry is always the master so its voice remains the session clock.
fn number_preview_sources(
    cue_id: Uuid,
    position_ms: Option<u64>,
    state: &AppState,
) -> Result<Vec<NumberPreviewSource>, String> {
    let is_number = {
        let ws = state.workspace.lock().map_err(|e| e.to_string())?;
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        cue_list
            .get_recursive(&cue_id)
            .ok_or("Cue not found")?
            .cue_type()
            == CueType::Number
    };
    if !is_number {
        return Ok(vec![NumberPreviewSource {
            source: preview_source(cue_id, state)?,
            offset_ms: 0,
            is_master: true,
        }]);
    }
    let (children, master_id, offsets) = {
        let ws = state.workspace.lock().map_err(|e| e.to_string())?;
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        let cue = cue_list.get_recursive(&cue_id).ok_or("Cue not found")?;
        let master_id = cue.number_master_id().ok_or_else(|| "Number needs a master Audio or Video cue".to_string())?;
        let offsets = cue.number_action_offsets().into_iter().collect::<HashMap<_, _>>();
        let children = cue
            .child_cues()
            .ok_or_else(|| "Number has no child cues".to_string())?
            .iter()
            .filter(|child| matches!(child.cue_type(), CueType::Audio | CueType::Video))
            .filter_map(|child| {
                let json = child.serialize();
                let volume = json.get("volume_db").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let id = child.id();
                let offset_ms = if id == master_id { 0 } else { offsets.get(&id).copied().unwrap_or(0) };
                number_preview_child_is_active(
                    &child.cue_type(),
                    child.is_disabled(),
                    volume,
                    offset_ms,
                    position_ms.unwrap_or(0),
                )
                .then(|| {
                    (
                        id,
                        child.cue_type(),
                        child.extract_decoded_audio(),
                        child.media_file_path().map(|path| path.to_path_buf()),
                        json,
                    )
                })
            })
            .collect::<Vec<_>>();
        (children, master_id, offsets)
    };

    let clock_ms = position_ms.unwrap_or(0);
    let mut sources = Vec::new();
    for (id, cue_type, decoded, file_path, json) in children {
        let is_master = id == master_id;
        let offset_ms = if is_master { 0 } else { offsets.get(&id).copied().unwrap_or(0) };
        if clock_ms < offset_ms {
            continue;
        }
        let source = match preview_source_from_parts(decoded, file_path, cue_type.clone(), &json) {
            Ok(source) => source,
            Err(error) if cue_type == CueType::Video && error.contains("has no audio track") => continue,
            Err(error) => return Err(error),
        };
        let Some(source_position_ms) = number_source_elapsed_ms(&source, clock_ms.saturating_sub(offset_ms)) else {
            continue;
        };
        sources.push(NumberPreviewSource { source, offset_ms, is_master });
        // Keep the calculated position in the source's trim start marker by
        // storing it in a temporary local below; this branch only collects
        // sources and the caller calculates it again deterministically.
        let _ = source_position_ms;
    }
    sources.sort_by_key(|item| (!item.is_master, item.offset_ms));
    if sources.is_empty() {
        return Err("Number has no enabled Audio/Video child with an audio track at this position".to_string());
    }
    Ok(sources)
}

fn start_cue_preview(
    cue_id: Uuid,
    position_ms: Option<u64>,
    end_ms: Option<u64>,
    state: &AppState,
) -> Result<PreviewStartResult, String> {
    // Reserve the request before decoding. Decode can take hundreds of ms and
    // Tauri may run several pointer-up replacements concurrently.
    let generation = state
        .preview_generation
        .fetch_add(1, Ordering::SeqCst)
        .wrapping_add(1);
    let source_items = number_preview_sources(cue_id, position_ms, state)?;
    let mut voices = Vec::with_capacity(source_items.len());
    for item in source_items {
        let source_position_ms = number_source_elapsed_ms(
            &item.source,
            position_ms.unwrap_or(0).saturating_sub(item.offset_ms),
        )
        .ok_or_else(|| "Preview position is outside this cue's clip range".to_string())?;
        let requested_end_ms = end_ms.or(item.source.trim_end_ms);
        voices.push(build_preview_voice(
            item.source,
            Some(source_position_ms),
            requested_end_ms,
        )?);
    }
    let mut session = state.preview_session.lock().map_err(|e| e.to_string())?;
    if !preview_request_is_current(state.preview_generation.load(Ordering::SeqCst), generation) {
        return Err("Preview request superseded".into());
    }
    if let Some(previous) = session.take() {
        for voice_id in previous.all_voice_ids() {
            state.audio_engine.stop_preview_voice(*voice_id);
        }
    }
    let mut voice_ids = Vec::with_capacity(voices.len());
    for voice in voices {
        match state.audio_engine.play_preview_voice(voice) {
            Ok(voice_id) => voice_ids.push(voice_id),
            Err(error) => {
                for voice_id in &voice_ids {
                    state.audio_engine.stop_preview_voice(*voice_id);
                }
                return Err(error.to_string());
            }
        }
    }
    let started = PreviewSession::new(cue_id, voice_ids, generation)
        .ok_or_else(|| "Preview has no playable audio voice".to_string())?;
    let started_result = PreviewStartResult {
        voice_id: started.voice_id.to_string(),
        generation: started.generation,
    };
    *session = Some(started);
    Ok(started_result)
}

fn preview_request_is_current(current: u64, request: u64) -> bool {
    current == request
}

/// Atomically detach the only preview session. The caller receives an owned
/// voice id and may stop it after dropping the lock; a preview started in that
/// interval is therefore a replacement, never a victim of the old stop.
fn take_preview_session(
    sessions: &std::sync::Mutex<Option<PreviewSession>>,
) -> Result<Option<PreviewSession>, String> {
    sessions
        .lock()
        .map(|mut slot| slot.take())
        .map_err(|e| e.to_string())
}

/// Compare-and-take one cue's session. This is the race-safe primitive used by
/// Toggle: a stale observation can never stop a newer cue's preview.
fn take_preview_session_for_cue(
    sessions: &std::sync::Mutex<Option<PreviewSession>>,
    cue_id: Uuid,
) -> Result<Option<PreviewSession>, String> {
    let mut slot = sessions.lock().map_err(|e| e.to_string())?;
    if slot.as_ref().is_some_and(|session| session.cue_id == cue_id) {
        Ok(slot.take())
    } else {
        Ok(None)
    }
}

/// Compare-and-take an explicit preview voice. This legacy API must not let an
/// old panel close stop the preview that replaced its voice id.
fn take_preview_session_for_voice(
    sessions: &std::sync::Mutex<Option<PreviewSession>>,
    voice_id: Uuid,
) -> Result<Option<PreviewSession>, String> {
    let mut slot = sessions.lock().map_err(|e| e.to_string())?;
    if slot.as_ref().is_some_and(|session| session.all_voice_ids().contains(&voice_id)) {
        Ok(slot.take())
    } else {
        Ok(None)
    }
}

/// Snapshot first, inspect the audio engine without holding the session mutex,
/// then compare-and-take. This lock order prevents an aux callback/lifecycle
/// operation from deadlocking a preview start while still preserving a newer
/// session that arrived during the inspection.
fn take_inactive_preview_session(
    sessions: &std::sync::Mutex<Option<PreviewSession>>,
    is_alive: impl Fn(Uuid) -> bool,
) -> Result<Option<PreviewSession>, String> {
    let session = sessions
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .cloned();
    let Some(session) = session else {
        return Ok(None);
    };
    if session
        .all_voice_ids()
        .iter()
        .any(|voice_id| is_alive(*voice_id))
    {
        return Ok(None);
    }
    take_preview_session_for_voice(sessions, session.voice_id)
}

/// A completed aux voice may have sent its status just before the event loop
/// reaps it. Reconcile the control-plane slot before toggling so the next click
/// starts a fresh audition instead of reporting a phantom "playing" session.
fn prune_inactive_preview_session(state: &AppState) -> Result<bool, String> {
    Ok(
        take_inactive_preview_session(&state.preview_session, |voice_id| {
            state.audio_engine.preview_voice_is_alive(voice_id)
        })?
        .is_some(),
    )
}

fn stop_active_preview(state: &AppState) -> Result<bool, String> {
    state.preview_generation.fetch_add(1, Ordering::SeqCst);
    let Some(session) = take_preview_session(&state.preview_session)? else {
        return Ok(false);
    };
    for voice_id in session.all_voice_ids() {
        state.audio_engine.stop_preview_voice(*voice_id);
    }
    Ok(true)
}

/// Start headphone preview of an Audio or Video Cue. It always opens the
/// configured preview aux output; it returns an error (and stays silent) when
/// that output is absent, unavailable, or points at the main PA.
///
/// The response includes the exact session generation emitted by
/// `preview-playhead`. This closes the command/event ordering race for a very
/// short file that reaches EOF before its first active 30 Hz event.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PreviewStartResult {
    pub voice_id: String,
    pub generation: u64,
}

#[tauri::command]
pub fn preview_cue(
    cue_id: String,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
    state: State<'_, AppState>,
) -> Result<PreviewStartResult, String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    start_cue_preview(id, start_ms, end_ms, &state)
}

/// Stop whichever cue currently owns the headphone preview session. This is
/// the preferred panel-close / selection-change command.
#[tauri::command]
pub fn stop_cue_preview(state: State<'_, AppState>) -> Result<(), String> {
    stop_active_preview(&state)?;
    Ok(())
}

/// Toggle the sole preview session for a cue. Starting a different cue stops
/// the prior session first; toggling the same cue stops it.
#[derive(Debug, Serialize)]
pub struct PreviewToggleResult {
    pub playing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

#[tauri::command]
pub fn toggle_cue_preview(
    cue_id: String,
    position_ms: Option<u64>,
    end_ms: Option<u64>,
    state: State<'_, AppState>,
) -> Result<PreviewToggleResult, String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    // Every toggle is a new lifecycle edge; invalidate any decode still in
    // flight before deciding whether this is an on or off transition.
    state.preview_generation.fetch_add(1, Ordering::SeqCst);
    prune_inactive_preview_session(&state)?;
    if let Some(session) = take_preview_session_for_cue(&state.preview_session, id)? {
        for voice_id in session.all_voice_ids() {
            state.audio_engine.stop_preview_voice(*voice_id);
        }
        return Ok(PreviewToggleResult {
            playing: false,
            voice_id: None,
            generation: None,
        });
    }
    let started = start_cue_preview(id, position_ms, end_ms, &state)?;
    Ok(PreviewToggleResult {
        playing: true,
        voice_id: Some(started.voice_id),
        generation: Some(started.generation),
    })
}

/// Legacy explicit-stop wrapper retained for existing callers. It only stops
/// the matching preview session, never a show/program voice supplied by ID.
#[tauri::command]
pub fn stop_preview(voice_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let id: Uuid = voice_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    if let Some(session) = take_preview_session_for_voice(&state.preview_session, id)? {
        for voice_id in session.all_voice_ids() {
            state.audio_engine.stop_preview_voice(*voice_id);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Video file management
// ---------------------------------------------------------------------------

/// Set the file path of a Video Cue.
///
/// Unlike [`set_audio_file`], no background decoding is needed — the video
/// streams directly from disk when the cue is triggered.
#[tauri::command]
pub fn set_video_file(
    cue_id: String,
    file_path: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;

    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let workspace_dir = ws
        .file_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(|path| path.to_owned());
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;

    let idx = cue_list.index_of(&id).ok_or("Cue not found")?;
    if cue_list.cues[idx].cue_type() != CueType::Video {
        return Err("set_video_file only applies to Video Cues".to_string());
    }

    stop_if_live(cue_list.cues[idx].as_mut(), &state, stop_fade_ms);
    let mut json = cue_list.cues[idx].serialize();
    update_auto_media_name(&mut json, &CueType::Video, &file_path);
    set_file_path_resetting_clip(&mut json, &file_path);
    let new_cue = registry.from_json(json).map_err(|e| e.to_string())?;
    drop(registry);
    cue_list.cues[idx] = new_cue;

    // Mark as loading — the audio track is decoded off-thread (the indicator
    // clears when decoding finishes), mirroring Audio Cues.
    {
        let mut loading = state.loading_cues.lock().map_err(|e| e.to_string())?;
        loading.insert(id);
    }

    drop(ws); // release the workspace lock before any background work
    invalidate_media_metadata_path(
        &state,
        std::path::Path::new(&file_path),
        workspace_dir.as_deref(),
    );

    // Probe the video duration (mpv) and audio metadata (symphonia). The audio
    // preload starts a bounded stream instead of retaining a full PCM copy.
    {
        let path = std::path::PathBuf::from(&file_path);
        let cue_id = id;
        let output_engine = Arc::clone(&state.output_engine);
        let workspace2 = Arc::clone(&state.workspace);
        let loading_cues = state.loading_cues.clone();
        let handle2 = app_handle.clone();
        std::thread::Builder::new()
            .name("inkue-video-load".into())
            .spawn(move || {
                let duration = output_engine
                    .try_mpv_lib()
                    .and_then(|lib| crate::engine::OutputEngine::probe_duration(lib, &path));
                let audio = crate::cue::media_decode::probe_audio_track(&path);

                // Search all cue lists — the user may have switched lists while loading.
                if let Ok(mut ws) = workspace2.lock() {
                    'store: {
                        for cl in ws.cue_lists.iter_mut() {
                            if let Some(idx2) = cl.index_of(&cue_id) {
                                if let Some(dur) = duration {
                                    cl.cues[idx2].set_runtime_duration(dur);
                                }
                                match audio {
                                    Ok(Some(info)) => cl.cues[idx2].accept_preloaded_stream(
                                        path.clone(),
                                        info.channels,
                                        info.sample_rate,
                                        info.total_frames.map(|frames| std::time::Duration::from_secs_f64(
                                            frames as f64 / info.sample_rate.max(1) as f64,
                                        )),
                                    ),
                                    Ok(None) => {} // silent video — no audio track
                                    Err(e) => {
                                        log::warn!("Video audio decode failed for {path:?}: {e}");
                                    }
                                }
                                break 'store;
                            }
                        }
                    }
                }
                if let Ok(mut loading) = loading_cues.lock() {
                    loading.remove(&cue_id);
                }
                let _ = handle2.emit("workspace-modified", serde_json::json!({}));
            })
            .ok();
    }

    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Return the list of connected monitors for the Screen selector in the inspector.
#[tauri::command]
pub fn list_video_screens(
    state: tauri::State<crate::state::AppState>,
) -> Vec<crate::engine::output_engine::ScreenInfo> {
    state.output_engine.list_screens()
}

/// Enumerate connected cameras / capture devices for the Camera Cue inspector.
///
/// Async + `spawn_blocking`: device enumeration can stall on a flaky driver,
/// and Tauri runs sync commands on the main thread (same rule as
/// `list_audio_devices` — see 1.1.5).
#[tauri::command]
pub async fn list_camera_devices(
) -> Result<Vec<crate::engine::camera_enum::CameraDeviceInfo>, String> {
    tauri::async_runtime::spawn_blocking(crate::engine::camera_enum::list_camera_devices)
        .await
        .map_err(|e| e.to_string())
}

/// Discover NDI senders currently announced on the local network. This is
/// intentionally a bounded scan: opening the Camera inspector must never
/// wait forever on a network service or a stale interface.
#[tauri::command]
pub async fn list_ndi_sources() -> Result<Vec<crate::engine::network_io::NdiSourceInfo>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        // The Core NDI finder joins discovery asynchronously. Five seconds is
        // still a bounded inspector refresh, but reliably sees an already-live
        // local vMix sender instead of racing its first announcement.
        crate::engine::network_io::NdiRuntime::load()?.list_sources(5_000)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Monotonic token so a rapid second Identify cancels the first one's cleanup.
static IDENTIFY_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Flash a big identification label on the output window, positioned on the
/// given screen, so the operator can verify before a show that "Screen 2"
/// really is the projector.  Cleans up after ~2.5 s: the label is removed and
/// the window is hidden again if it was hidden before.
#[tauri::command]
pub fn identify_output_screen(
    screen_index: Option<u32>,
    state: tauri::State<crate::state::AppState>,
) -> Result<(), String> {
    use std::sync::atomic::Ordering;

    let engine = std::sync::Arc::clone(&state.output_engine);
    let was_visible = engine.is_output_visible();
    let generation = IDENTIFY_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;

    let label = match screen_index {
        Some(idx) => format!("SCREEN {}", idx + 1),
        None => "OUTPUT WINDOW".to_string(),
    };
    let ass = format!(
        "{{\\an5\\fs140\\bord6\\1c&H00FFFFFF&\\3c&H00000000&}}{label}\\N{{\\fs42}}Inkue output identification",
    );
    engine.show_text_overlay(&ass, screen_index);

    std::thread::Builder::new()
        .name("inkue-identify-screen".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(2500));
            // A newer Identify owns the overlay now — let its cleanup handle it.
            if IDENTIFY_GENERATION.load(Ordering::Relaxed) != generation {
                return;
            }
            engine.clear_text_overlay();
            if !was_visible {
                engine.hide_output();
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Image file management
// ---------------------------------------------------------------------------

/// Set the file path of an Image Cue.
///
/// Unlike [`set_audio_file`], no background decoding is needed — the image is
/// passed to the OutputEngine at GO time via mpv loadfile.
#[tauri::command]
pub fn set_image_file(
    cue_id: String,
    file_path: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;

    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let workspace_dir = ws
        .file_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(|path| path.to_owned());
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;

    let idx = cue_list.index_of(&id).ok_or("Cue not found")?;
    if cue_list.cues[idx].cue_type() != CueType::Image {
        return Err("set_image_file only applies to Image Cues".to_string());
    }

    stop_if_live(cue_list.cues[idx].as_mut(), &state, stop_fade_ms);
    let mut json = cue_list.cues[idx].serialize();
    update_auto_media_name(&mut json, &CueType::Image, &file_path);
    if let Some(obj) = json.as_object_mut() {
        obj.insert("file_path".to_string(), serde_json::json!(file_path));
    }
    let new_cue = registry.from_json(json).map_err(|e| e.to_string())?;
    drop(registry);
    cue_list.cues[idx] = new_cue;

    drop(ws);
    invalidate_media_metadata_path(
        &state,
        std::path::Path::new(&file_path),
        workspace_dir.as_deref(),
    );

    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

// ---------------------------------------------------------------------------
// MIDI file management
// ---------------------------------------------------------------------------

/// Set the file path of a MIDI File Cue.
///
/// The rebuild parses the new file, so the cue's duration is correct by the
/// time the command returns — MIDI files are small enough that no background
/// decode is warranted.
#[tauri::command]
pub fn set_midi_file(
    cue_id: String,
    file_path: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;

    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let workspace_dir = ws
        .file_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(|path| path.to_owned());
    let stop_fade_ms = ws.preferences.audio.default_fade_out_ms;
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;

    let json = {
        let cue = cue_list.get_mut_recursive(&id).ok_or("Cue not found")?;
        if cue.cue_type() != CueType::MidiFile {
            return Err("set_midi_file only applies to MIDI File Cues".to_string());
        }
        stop_if_live(cue, &state, stop_fade_ms);
        let mut json = cue.serialize();
        update_auto_media_name(&mut json, &CueType::MidiFile, &file_path);
        if let Some(obj) = json.as_object_mut() {
            obj.insert("file_path".to_string(), serde_json::json!(file_path));
        }
        json
    };
    let new_cue = registry.from_json(json).map_err(|e| e.to_string())?;
    drop(registry);
    cue_list.replace_cue_recursive(&id, new_cue);

    drop(ws);
    invalidate_media_metadata_path(
        &state,
        std::path::Path::new(&file_path),
        workspace_dir.as_deref(),
    );

    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Return a `data:` URL thumbnail for a media file (image or video), for the
/// inspector's Media preview. Generated headlessly via libmpv (`vo=image`) and
/// cached on disk, so only the first request per file decodes anything.
///
/// `seek_into` picks a frame ~15 % in (videos — frame 0 is often black).
/// Async + `spawn_blocking`: decode can take a few hundred ms and sync
/// commands run on the main thread.
#[tauri::command]
pub async fn get_media_thumbnail(
    path: String,
    seek_into: bool,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let lib = state
        .output_engine
        .try_mpv_lib_arc()
        .ok_or_else(|| crate::engine::output_engine::NO_VIDEO_OUTPUT.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::engine::thumbnails::media_thumbnail(&lib, std::path::Path::new(&path), seek_into)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Authorise the current Video Cue's exact file for the local asset protocol.
///
/// The protocol supports the HTTP range requests needed for streaming and
/// seeking. Its static scope is deliberately empty; only a canonical existing
/// file already referenced by the active workspace can be added here.
#[tauri::command]
pub fn prepare_video_preview(
    cue_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    let id: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let canonical = {
        let ws = state.workspace.lock().map_err(|e| e.to_string())?;
        let workspace_dir = ws.file_path.as_ref().and_then(|path| path.parent());
        let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
        let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
        validate_video_preview_path(&cue.cue_type(), cue.media_file_path(), workspace_dir)?
    };

    app_handle
        .asset_protocol_scope()
        .allow_file(&canonical)
        .map_err(|error| format!("Cannot authorise video preview file: {error}"))?;

    canonical
        .into_os_string()
        .into_string()
        .map_err(|_| "Video preview path is not valid Unicode".to_string())
}

/// Return a filmstrip for the video trimmer: `tiles` JPEG `data:` URLs evenly
/// spread across the file. Same headless-libmpv path and disk cache as
/// [`get_media_thumbnail`].
#[tauri::command]
pub async fn get_video_filmstrip(
    path: String,
    tiles: usize,
    tile_width: u32,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let lib = state
        .output_engine
        .try_mpv_lib_arc()
        .ok_or_else(|| crate::engine::output_engine::NO_VIDEO_OUTPUT.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::engine::thumbnails::video_filmstrip(
            &lib,
            std::path::Path::new(&path),
            tiles,
            tile_width,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Filmstrip over a time range (zoomed clip editor): `tiles` frames spread
/// across `[start_s, end_s]`.  Same cache/headless-mpv path as the full strip.
#[tauri::command]
pub async fn get_video_filmstrip_range(
    path: String,
    start_s: f64,
    end_s: f64,
    tiles: usize,
    tile_width: u32,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let lib = state
        .output_engine
        .try_mpv_lib_arc()
        .ok_or_else(|| crate::engine::output_engine::NO_VIDEO_OUTPUT.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::engine::thumbnails::video_filmstrip_range(
            &lib,
            std::path::Path::new(&path),
            start_s,
            end_s,
            tiles,
            tile_width,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Toggle the output window visibility (F9 / View menu).
#[tauri::command]
pub fn toggle_output_window(state: State<'_, AppState>) {
    state.output_engine.toggle_visibility();
}

/// Return whether the output window is currently visible.
#[tauri::command]
pub fn get_output_window_visible(state: State<'_, AppState>) -> bool {
    state.output_engine.is_visible()
}

// ---------------------------------------------------------------------------
// Group Cue commands
// ---------------------------------------------------------------------------

/// Wrap the given cues in a new Group Cue inserted at the first selected position.
/// Returns the new Group's ID.
#[tauri::command]
pub fn group_cues(
    ids: Vec<String>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let ids: Vec<Uuid> = ids
        .iter()
        .map(|s| s.parse::<Uuid>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let group_default_color = ws.preferences.general.default_cue_color(&CueType::Group);
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let group_id = cue_list.group_cues(&ids).map_err(|e| e.to_string())?;
    cue_list
        .get_mut_recursive(&group_id)
        .ok_or("New Group Cue not found")?
        .set_color(group_default_color);
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(group_id.to_string())
}

/// Add an Audio/Video/Image/Group action inside a Number. The generated cue
/// is a normal registered Cue, but remains hidden while the Number is folded.
fn number_audio_should_be_master(cue_type: CueType, master_type: Option<CueType>) -> bool {
    cue_type == CueType::Audio && matches!(master_type, None | Some(CueType::Video))
}

#[tauri::command]
pub fn add_number_action(
    number_id: String,
    cue_type: CueType,
    offset_ms: u64,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    if !matches!(cue_type, CueType::Audio | CueType::Video | CueType::Image | CueType::Group) {
        return Err("Number actions support Audio, Video, Image, and Group cues".into());
    }
    super::undo_cmds::push_current_snapshot(&state)?;
    let number_uuid: Uuid = number_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let child = registry.create(&cue_type).map_err(|e| e.to_string())?;
    let child_id = child.id();
    drop(registry);
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let number = cue_list.get_mut_recursive(&number_uuid).ok_or("Number not found")?;
    if number.cue_type() != CueType::Number {
        return Err("Target is not a Number".into());
    }
    number.add_child(child, -1).map_err(|e| e.to_string())?;
    // The first Audio added to a Number is the convenient default master.
    // Video may still be selected explicitly; an image never becomes master.
    let master_type = number.number_master_id().and_then(|id| {
        number.child_cues()?.iter().find(|child| child.id() == id).map(|child| child.cue_type())
    });
    if number_audio_should_be_master(cue_type, master_type) {
        number.set_number_master(child_id).map_err(|e| e.to_string())?;
    } else {
        number.set_number_action_offset(child_id, offset_ms).map_err(|e| e.to_string())?;
    }
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(child_id.to_string())
}

/// Assign one nested Audio/Video cue as the Number master.
#[tauri::command]
pub fn set_number_master(
    number_id: String,
    child_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let number_uuid: Uuid = number_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let child_uuid: Uuid = child_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let number = cue_list.get_mut_recursive(&number_uuid).ok_or("Number not found")?;
    if number.cue_type() != CueType::Number {
        return Err("Target is not a Number".into());
    }
    number.set_number_master(child_uuid).map_err(|e| e.to_string())?;
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Change the offset of one Number action.
#[tauri::command]
pub fn set_number_action_offset(
    number_id: String,
    action_id: String,
    offset_ms: u64,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let number_uuid: Uuid = number_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let action_uuid: Uuid = action_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let number = cue_list.get_mut_recursive(&number_uuid).ok_or("Number not found")?;
    if number.cue_type() != CueType::Number {
        return Err("Target is not a Number".into());
    }
    number.set_number_action_offset(action_uuid, offset_ms).map_err(|e| e.to_string())?;
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Remove one Number child and clear its master/offset metadata atomically.
#[tauri::command]
pub fn remove_number_action(
    number_id: String,
    action_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let number_uuid: Uuid = number_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let action_uuid: Uuid = action_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let context = super::transport_cmds::make_context(&state, 0);
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let number = cue_list.get_mut_recursive(&number_uuid).ok_or("Number not found")?;
    if number.cue_type() != CueType::Number {
        return Err("Target is not a Number".into());
    }
    if let Some(children) = number.child_cues_mut() {
        let child = children.iter_mut().find(|child| child.id() == action_uuid)
            .ok_or("Number action not found")?;
        stop_preview_if_owned_by_tree(&state, child.as_mut())?;
        crate::show::transport::hard_stop_cue_tree(&context, child.as_mut())
            .map_err(|e| e.to_string())?;
    }
    number.remove_child(&action_uuid).map_err(|e| e.to_string())?;
    ws.mark_modified();
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Dissolve a Group: move its children into the parent list and remove the Group.
#[tauri::command]
pub fn ungroup(
    group_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let id: Uuid = group_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    cue_list.ungroup(&id).map_err(|e| e.to_string())?;
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Change the playback mode of a Group Cue (simultaneous | sequential).
#[tauri::command]
pub fn set_group_mode(
    group_id: String,
    mode: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let id: Uuid = group_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let group_mode: GroupMode = serde_json::from_value(serde_json::json!(mode))
        .map_err(|_| format!("Unknown group mode: {mode}"))?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let cue = cue_list.get_mut_recursive(&id).ok_or("Group cue not found")?;
    cue.set_group_mode(group_mode);
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Enable/disable looping for a Playlist Group Cue (wrap last child → first).
#[tauri::command]
pub fn set_playlist_loop(
    group_id: String,
    loop_on: bool,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let id: Uuid = group_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let cue = cue_list.get_mut_recursive(&id).ok_or("Group cue not found")?;
    cue.set_playlist_loop(loop_on);
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Move a top-level cue into a Group's children (position = −1 for append).
#[tauri::command]
pub fn add_cue_to_group(
    cue_id: String,
    group_id: String,
    position: i32,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let cue_uuid: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let grp_uuid: Uuid = group_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let child_type = cue_list
        .get_recursive(&cue_uuid)
        .map(|cue| cue.cue_type())
        .ok_or("Cue not found")?;
    let promote_audio = if child_type == CueType::Audio {
        cue_list.get_recursive(&grp_uuid).and_then(|group| {
            if group.cue_type() != CueType::Number { return None; }
            let master_type = group.number_master_id().and_then(|id| {
                group.child_cues()?.iter().find(|child| child.id() == id).map(|child| child.cue_type())
            });
            Some(number_audio_should_be_master(child_type, master_type))
        }).unwrap_or(false)
    } else { false };
    cue_list
        .add_to_group(&cue_uuid, &grp_uuid, position)
        .map_err(|e| e.to_string())?;
    if promote_audio {
        cue_list
            .get_mut_recursive(&grp_uuid)
            .ok_or("Group cue not found")?
            .set_number_master(cue_uuid)
            .map_err(|e| e.to_string())?;
    }
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Move a cue from anywhere in the hierarchy to the top-level list, immediately
/// before `before_id` (or at the end if `before_id` is `null`).
#[tauri::command]
pub fn move_to_top_level(
    cue_id: String,
    before_id: Option<String>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let cue_uuid: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let before_uuid: Option<Uuid> = before_id
        .as_deref()
        .map(|s| s.parse::<Uuid>().map_err(|e| e.to_string()))
        .transpose()?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    cue_list
        .move_to_top_level_before(&cue_uuid, before_uuid.as_ref())
        .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

/// Remove a child cue from a Group and place it after the Group in the main list.
#[tauri::command]
pub fn remove_cue_from_group(
    group_id: String,
    cue_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    super::undo_cmds::push_current_snapshot(&state)?;
    let grp_uuid: Uuid = group_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let cue_uuid: Uuid = cue_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    ws.mark_modified();
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    cue_list
        .remove_from_group(&grp_uuid, &cue_uuid)
        .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::{mpsc, Arc, Mutex};

    use super::{
        apply_default_cue_color, build_preview_voice, can_reuse_cached_media_metadata,
        compute_waveform_bins,
        finish_context_target,
        generated_media_cue_name, index_cue_targets, number_audio_should_be_master,
        number_preview_child_is_active, number_source_elapsed_ms,
        preview_request_is_current,
        probe_media_metadata_file, set_file_path_resetting_clip, should_replace_generated_name,
        take_inactive_preview_session, take_preview_session_for_cue,
        take_preview_session_for_voice, targeted_cue_accepts_target, targeted_cue_label,
        targeted_cue_name, target_display_for_cue, update_auto_media_name,
        validate_video_preview_path, PreviewSession, PreviewSource, PreviewStartResult,
    };
    use crate::cue::{
        audio_cue::AudioCue,
        control_cue::{ControlCueFactory, ALL_CONTROL_ACTIONS},
        devamp_cue::DevampCueFactory,
        fade_cue::FadeCueFactory,
        registry::CueRegistry,
        stop_cue::StopCue,
        stop_cue::StopCueFactory,
        traits::Cue,
        types::{CueColor, CueType},
    };
    use crate::engine::media_metadata::{
        MediaMetadata, MediaMetadataFailure, MediaMetadataKey, MediaMetadataReservation,
    };
    use crate::show::cue_list::CueList;
    use crate::preferences::GeneralPreferences;
    use uuid::Uuid;

    fn targeted_test_registry() -> CueRegistry {
        let mut registry = CueRegistry::new();
        registry.register(CueType::Stop, Box::new(StopCueFactory));
        registry.register(CueType::Fade, Box::new(FadeCueFactory));
        registry.register(CueType::Devamp, Box::new(DevampCueFactory));
        for action in ALL_CONTROL_ACTIONS {
            registry.register(action.cue_type(), Box::new(ControlCueFactory(action)));
        }
        registry
    }

    #[test]
    fn context_target_creation_binds_and_names_every_targeted_cue_type() {
        let registry = targeted_test_registry();
        let cases = [
            (CueType::Stop, "Stop"),
            (CueType::Fade, "Fade"),
            (CueType::Devamp, "Devamp"),
            (CueType::Start, "Start"),
            (CueType::Pause, "Pause"),
            (CueType::Resume, "Resume"),
            (CueType::Load, "Load"),
            (CueType::Reset, "Reset"),
            (CueType::Goto, "Goto"),
            (CueType::Arm, "Arm"),
            (CueType::Disarm, "Disarm"),
        ];

        for (cue_type, label) in cases {
            let mut list = CueList::new("test");
            let mut target = AudioCue::new();
            target.set_number(Some("10".to_string()));
            target.set_name("Opening music".to_string());
            let target_id = target.id();
            list.push(Box::new(target));
            let cue = registry.create(&cue_type).unwrap();
            let cue_id = cue.id();
            list.push(cue);
            finish_context_target(&mut list, cue_id, target_id, label);
            let cue = list.get_recursive(&cue_id).unwrap();
            let json = cue.serialize();
            assert_eq!(cue.cue_type(), cue_type);
            assert_eq!(cue.name(), format!("{label} 10 Opening music"));
            assert_eq!(json["target_cue_ids"], serde_json::json!([target_id.to_string()]));
            assert_eq!(json["target_cue_numbers"], serde_json::json!(["10"]));
        }
    }

    #[test]
    fn context_target_creation_rejects_regular_and_incompatible_pairs() {
        assert!(targeted_cue_label(&CueType::Audio).is_none());
        assert!(targeted_cue_accepts_target(&CueType::Stop, &CueType::Memo));
        assert!(targeted_cue_accepts_target(&CueType::Start, &CueType::Memo));
        assert!(targeted_cue_accepts_target(&CueType::Fade, &CueType::Camera));
        assert!(!targeted_cue_accepts_target(&CueType::Fade, &CueType::Memo));
        assert!(targeted_cue_accepts_target(&CueType::Devamp, &CueType::Video));
        assert!(!targeted_cue_accepts_target(&CueType::Devamp, &CueType::Image));
    }

    #[test]
    fn grouped_cue_uses_group_default_without_recoloring_children() {
        let mut list = CueList::new("test");
        let mut child = AudioCue::new();
        child.set_color(CueColor::Orange);
        let child_id = child.id();
        list.push(Box::new(child));

        let group_id = list.group_cues(&[child_id]).unwrap();
        let mut preferences = GeneralPreferences::default();
        preferences.default_cue_colors.insert(CueType::Group, CueColor::Purple);
        let group = list.get_mut_recursive(&group_id).unwrap();
        apply_default_cue_color(group, &preferences);

        assert_eq!(group.color(), CueColor::Purple);
        assert_eq!(group.child_cues().unwrap()[0].color(), CueColor::Orange);
    }

    #[test]
    fn context_target_uses_final_auto_number_when_inserted_above() {
        let registry = targeted_test_registry();
        let mut list = CueList::new("test");
        list.auto_renumber = true;
        let mut target = AudioCue::new();
        target.set_name("Clip".to_string());
        let target_id = target.id();
        list.push(Box::new(target));

        let cue = registry.create(&CueType::Stop).unwrap();
        let cue_id = cue.id();
        list.insert(0, cue);
        finish_context_target(&mut list, cue_id, target_id, "Stop");

        let cue = list.get_recursive(&cue_id).unwrap();
        assert_eq!(cue.name(), "Stop 2 Clip");
        assert_eq!(cue.serialize()["target_cue_numbers"], serde_json::json!(["2"]));
        assert_eq!(list.get_recursive(&target_id).unwrap().number(), Some("2"));
    }

    #[test]
    fn context_target_uses_final_auto_number_when_inserted_below() {
        let registry = targeted_test_registry();
        let mut list = CueList::new("test");
        list.auto_renumber = true;
        let mut target = AudioCue::new();
        target.set_name("Clip".to_string());
        let target_id = target.id();
        list.push(Box::new(target));

        let cue = registry.create(&CueType::Stop).unwrap();
        let cue_id = cue.id();
        list.insert(1, cue);
        finish_context_target(&mut list, cue_id, target_id, "Stop");

        let cue = list.get_recursive(&cue_id).unwrap();
        assert_eq!(cue.name(), "Stop 1 Clip");
        assert_eq!(cue.serialize()["target_cue_numbers"], serde_json::json!(["1"]));
        assert_eq!(list.get_recursive(&target_id).unwrap().number(), Some("1"));
    }

    #[test]
    fn context_target_name_omits_blank_number_and_name_cleanly() {
        assert_eq!(targeted_cue_name("Stop", None, ""), "Stop");
        assert_eq!(targeted_cue_name("Stop", Some(" 10 "), " Clip "), "Stop 10 Clip");
    }

    #[test]
    fn media_cue_names_use_the_cue_type_and_extensionless_filename() {
        let cases = [
            (CueType::Audio, r"C:\\Show\\Audio\\Intro theme.wav", "Audio Intro theme"),
            (CueType::Video, "/show/video/Opening.MOV", "Video Opening"),
            (CueType::Image, "/show/stills/title card.png", "Image title card"),
            (CueType::MidiFile, "/show/midi/stings.mid", "MIDI stings"),
        ];
        for (cue_type, path, expected) in cases {
            assert_eq!(generated_media_cue_name(&cue_type, path).as_deref(), Some(expected));
        }
        assert_eq!(generated_media_cue_name(&CueType::Audio, ""), None);
        assert_eq!(
            generated_media_cue_name(&CueType::Audio, "/show/.hidden").as_deref(),
            Some("Audio .hidden")
        );
    }

    #[test]
    fn assigning_media_updates_only_default_or_previously_generated_names() {
        let mut default = serde_json::json!({ "name": "Audio Cue", "file_path": null });
        update_auto_media_name(&mut default, &CueType::Audio, r"C:\\Show\\intro.wav");
        assert_eq!(default["name"], "Audio intro");

        let mut generated = serde_json::json!({ "name": "Audio old", "file_path": "old.wav" });
        update_auto_media_name(&mut generated, &CueType::Audio, "new.wav");
        assert_eq!(generated["name"], "Audio new");

        let mut manual = serde_json::json!({ "name": "Opening sting", "file_path": "old.wav" });
        update_auto_media_name(&mut manual, &CueType::Audio, "new.wav");
        assert_eq!(manual["name"], "Opening sting");

        assert!(should_replace_generated_name("  ", Some("Audio Cue"), None));
        assert!(!should_replace_generated_name("Operator label", Some("Audio Cue"), None));
    }

    #[test]
    fn target_summary_resolves_numbers_and_distinguishes_stop_all() {
        let mut target = AudioCue::new();
        target.set_number(Some("10".to_string()));
        target.set_name("Opening music".to_string());
        let target_id = target.id();
        let cues = vec![Box::new(target) as Box<dyn Cue>];
        let mut index = super::CueTargetIndex::default();
        index_cue_targets(&cues, &mut index);

        let mut stop = StopCue::new();
        stop.set_context_target(target_id, Some("10".to_string()));
        let (targets, targets_all) = target_display_for_cue(&stop, &index);
        assert!(!targets_all);
        let targets = targets.expect("Stop Cue has target details");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].number.as_deref(), Some("10"));
        assert_eq!(targets[0].name, "Opening music");

        let stop_all = StopCue::new();
        let (targets, targets_all) = target_display_for_cue(&stop_all, &index);
        assert!(targets_all);
        assert_eq!(targets.expect("Stop All has an empty target list").len(), 0);
    }

    #[test]
    fn video_preview_resolves_and_canonicalizes_a_relative_cue_file() {
        let dir = std::env::temp_dir().join(format!("qlisa-video-preview-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("clip.mp4");
        std::fs::write(&file, b"preview fixture").unwrap();

        let resolved = validate_video_preview_path(
            &CueType::Video,
            Some(std::path::Path::new("clip.mp4")),
            Some(&dir),
        )
        .expect("valid current Video Cue file");
        assert_eq!(resolved, file.canonicalize().unwrap());

        let _ = std::fs::remove_file(file);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn video_preview_rejects_non_video_cues_and_missing_sources() {
        let arbitrary = std::path::Path::new("C:/private/not-a-cue.txt");
        assert!(validate_video_preview_path(&CueType::Audio, Some(arbitrary), None).is_err());
        assert!(validate_video_preview_path(&CueType::Video, None, None).is_err());
        assert!(validate_video_preview_path(
            &CueType::Video,
            Some(std::path::Path::new("Z:/definitely/not/here.mp4")),
            None,
        )
        .is_err());
    }

    #[test]
    fn video_preview_rejects_directories() {
        let dir = std::env::temp_dir().join(format!("qlisa-video-preview-dir-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        assert!(validate_video_preview_path(&CueType::Video, Some(&dir), None).is_err());
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn stat_only_media_probe_reports_exact_file_size() {
        let path = std::env::temp_dir().join(format!("qlisa-media-meta-{}.bin", Uuid::new_v4()));
        std::fs::write(&path, [7_u8; 137]).unwrap();
        let job = MediaMetadataReservation {
            key: MediaMetadataKey::new(path.clone(), false),
            generation: 1,
            previous: None,
        };
        let result = probe_media_metadata_file(&job, None).expect("stat metadata");
        assert_eq!(result.file_size_bytes, 137);
        assert_eq!((result.width, result.height), (None, None));
        assert!(result.modified_at.is_some());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn media_probe_rejects_a_directory_path() {
        let path = std::env::temp_dir().join(format!("qlisa-media-meta-dir-{}", Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        let job = MediaMetadataReservation {
            key: MediaMetadataKey::new(path.clone(), false),
            generation: 1,
            previous: None,
        };
        assert_eq!(
            probe_media_metadata_file(&job, None),
            Err(MediaMetadataFailure::NotAFile)
        );
        let _ = std::fs::remove_dir(path);
    }

    #[test]
    fn visual_metadata_without_dimensions_is_reprobed_after_ttl() {
        let modified_at = Some(std::time::SystemTime::UNIX_EPOCH);
        let previous = MediaMetadata {
            file_size_bytes: 137,
            width: None,
            height: None,
            compatibility_status: 0,
            compatibility_reason: None,
            probe_failure: None,
            modified_at,
        };

        assert!(!can_reuse_cached_media_metadata(
            previous,
            137,
            modified_at,
            true,
        ));
        assert!(can_reuse_cached_media_metadata(
            previous,
            137,
            modified_at,
            false,
        ));
    }

    #[test]
    fn changing_the_file_resets_the_clip_window() {
        let mut json = serde_json::json!({
            "file_path": "old.wav",
            "start_time_ms": 81_314,
            "end_time_ms": 147_104,
            "slices": { "markers": [90_000, 120_000], "play_counts": [1, u32::MAX, 1] },
            "cached_duration_ms": 274_250,
            "volume_db": -6.0,
        });
        set_file_path_resetting_clip(&mut json, "new.wav");
        assert_eq!(json["file_path"], "new.wav");
        assert!(json["start_time_ms"].is_null());
        assert!(json["end_time_ms"].is_null());
        assert!(json["cached_duration_ms"].is_null());
        assert_eq!(json["slices"]["markers"].as_array().unwrap().len(), 0);
        assert_eq!(json["volume_db"], -6.0, "unrelated fields untouched");
    }

    #[test]
    fn repicking_the_same_file_keeps_the_clip_window() {
        let mut json = serde_json::json!({
            "file_path": "same.wav",
            "start_time_ms": 1_000,
            "end_time_ms": 2_000,
            "slices": { "markers": [1_500], "play_counts": [1, 1] },
        });
        set_file_path_resetting_clip(&mut json, "same.wav");
        assert_eq!(json["start_time_ms"], 1_000);
        assert_eq!(json["end_time_ms"], 2_000);
        assert_eq!(json["slices"]["markers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn waveform_bins_empty_input() {
        assert_eq!(compute_waveform_bins(&[], 2, 10), (vec![], vec![]));
        assert_eq!(compute_waveform_bins(&[0.5, 0.5], 2, 0), (vec![], vec![]));
    }

    #[test]
    fn waveform_bins_rms_never_exceeds_peak() {
        let samples: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.13).sin() * 0.8).collect();
        let (peaks, rms) = compute_waveform_bins(&samples, 2, 40);
        assert_eq!(peaks.len(), 40);
        assert_eq!(rms.len(), 40);
        for (p, r) in peaks.iter().zip(&rms) {
            assert!(r <= p, "rms {r} > peak {p}");
        }
    }

    #[test]
    fn waveform_bins_sine_rms_is_peak_over_sqrt2() {
        // Full-scale sine: peak ≈ 1.0, RMS ≈ 1/√2 ≈ 0.707.
        let samples: Vec<f32> = (0..48000)
            .map(|i| (i as f32 * std::f32::consts::TAU / 480.0).sin())
            .collect();
        let (peaks, rms) = compute_waveform_bins(&samples, 1, 4);
        for p in &peaks {
            assert!((p - 1.0).abs() < 0.01, "peak {p}");
        }
        for r in &rms {
            assert!(
                (r - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01,
                "rms {r}"
            );
        }
    }

    #[test]
    fn waveform_bins_constant_signal_rms_equals_peak() {
        let samples = vec![0.5f32; 1000];
        let (peaks, rms) = compute_waveform_bins(&samples, 1, 5);
        for (p, r) in peaks.iter().zip(&rms) {
            assert!((p - 0.5).abs() < 1e-6);
            assert!((r - 0.5).abs() < 1e-4);
        }
    }

    #[test]
    fn number_preview_selects_master_and_active_unmuted_children_only() {
        let master = Uuid::new_v4();
        let action = Uuid::new_v4();
        let future = Uuid::new_v4();
        let disabled = Uuid::new_v4();
        let muted = Uuid::new_v4();
        let video = Uuid::new_v4();
        let candidates = vec![
            (master, CueType::Audio, false, 0.0, 0),
            (action, CueType::Audio, false, -3.0, 1_000),
            (future, CueType::Audio, false, 0.0, 9_000),
            (disabled, CueType::Video, true, 0.0, 0),
            (muted, CueType::Audio, false, -60.0, 0),
            (video, CueType::Video, false, -6.0, 1_000),
        ];
        let selected: Vec<Uuid> = candidates
            .iter()
            .filter(|(_, cue_type, is_disabled, volume_db, offset_ms)| {
                number_preview_child_is_active(
                    cue_type,
                    *is_disabled,
                    *volume_db,
                    *offset_ms,
                    2_000,
                )
            })
            .map(|(id, ..)| *id)
            .collect();
        assert_eq!(selected, vec![master, action, video]);
    }

    #[test]
    fn number_preview_maps_clock_position_through_offset_crop_and_loop() {
        let source = PreviewSource {
            samples: std::sync::Arc::new(vec![0.0; 10_000]),
            channels: 1,
            sample_rate: 1_000,
            volume_db: 0.0,
            pan: 0.0,
            trim_start_ms: Some(2_000),
            trim_end_ms: Some(6_000),
            rate: 1.0,
            loop_count: 2,
        };
        // Number clock 7.5 s, action starts at 1 s: 6.5 s into a 4 s crop.
        // The second pass is at source 4.5 s (2 s crop start + 2.5 s).
        assert_eq!(number_source_elapsed_ms(&source, 6_500), Some(4_500));
        // Three passes total = 12 s of action time; this is already finished.
        assert_eq!(number_source_elapsed_ms(&source, 12_000), None);
    }

    #[test]
    fn preview_uses_the_requested_file_position_inside_the_clip_window() {
        let voice = build_preview_voice(
            PreviewSource {
                samples: std::sync::Arc::new(vec![0.0; 10_000]),
                channels: 1,
                sample_rate: 1_000,
                volume_db: 0.0,
                pan: 0.0,
                trim_start_ms: Some(2_000),
                trim_end_ms: Some(8_000),
                rate: 1.5,
                loop_count: 0,
            },
            Some(5_250),
            None,
        )
        .expect("preview voice");

        assert_eq!(
            voice.current_frame(),
            5_250,
            "file position becomes the source-frame offset"
        );
        // SAFETY: build_preview_voice has not submitted this voice to an RT callback.
        assert_eq!(unsafe { *voice.inner.end_frame.get() }, Some(8_000));
        assert_eq!(voice.inner.rate(), 1.5);
    }

    #[test]
    fn preview_position_cannot_escape_the_cue_trim() {
        let result = build_preview_voice(
            PreviewSource {
                samples: std::sync::Arc::new(vec![0.0; 10_000]),
                channels: 1,
                sample_rate: 1_000,
                volume_db: 0.0,
                pan: 0.0,
                trim_start_ms: Some(2_000),
                trim_end_ms: Some(8_000),
                rate: 1.0,
                loop_count: 0,
            },
            Some(8_000),
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn stale_preview_session_is_pruned_when_its_voice_disappears() {
        let voice_id = Uuid::new_v4();
        let session = PreviewSession::new(Uuid::new_v4(), vec![voice_id], 1).unwrap();
        let sessions = Mutex::new(Some(session.clone()));

        assert_eq!(
            take_inactive_preview_session(&sessions, |_| false).unwrap(),
            Some(session),
        );
        assert_eq!(*sessions.lock().unwrap(), None);
    }

    #[test]
    fn live_preview_session_is_retained_during_lifecycle_prune() {
        let voice_id = Uuid::new_v4();
        let session = PreviewSession::new(Uuid::new_v4(), vec![voice_id], 1).unwrap();
        let sessions = Mutex::new(Some(session.clone()));

        assert_eq!(
            take_inactive_preview_session(&sessions, |_| true).unwrap(),
            None
        );
        assert_eq!(*sessions.lock().unwrap(), Some(session));
    }

    #[test]
    fn stopping_old_preview_cannot_take_a_replacement_session() {
        let old = PreviewSession::new(Uuid::new_v4(), vec![Uuid::new_v4(), Uuid::new_v4()], 1).unwrap();
        let replacement = PreviewSession::new(Uuid::new_v4(), vec![Uuid::new_v4()], 2).unwrap();
        let sessions = Arc::new(Mutex::new(Some(old.clone())));
        let (taken_tx, taken_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker_sessions = Arc::clone(&sessions);
        let old_voice_id = old.voice_id;

        let worker = std::thread::spawn(move || {
            let taken = take_preview_session_for_voice(&worker_sessions, old_voice_id).unwrap();
            taken_tx.send(taken.clone()).unwrap();
            resume_rx.recv().unwrap();
            // This is where the command stops `taken`'s voice. It has no
            // session lock, so it cannot remove the replacement below.
            taken
        });

        assert_eq!(taken_rx.recv().unwrap(), Some(old.clone()));
        *sessions.lock().unwrap() = Some(replacement.clone());
        resume_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), Some(old.clone()));
        assert_eq!(*sessions.lock().unwrap(), Some(replacement.clone()));
    }

    #[test]
    fn cue_compare_and_take_only_stops_the_requested_cue() {
        let first = PreviewSession::new(Uuid::new_v4(), vec![Uuid::new_v4()], 1).unwrap();
        let second = PreviewSession::new(Uuid::new_v4(), vec![Uuid::new_v4()], 2).unwrap();
        let sessions = Mutex::new(Some(second.clone()));

        assert_eq!(
            take_preview_session_for_cue(&sessions, first.cue_id).unwrap(),
            None
        );
        assert_eq!(*sessions.lock().unwrap(), Some(second.clone()));
        assert_eq!(
            take_preview_session_for_cue(&sessions, second.cue_id).unwrap(),
            Some(second.clone())
        );
        assert_eq!(*sessions.lock().unwrap(), None);
    }

    #[test]
    fn preview_session_secondary_voice_identifies_the_whole_mix_for_stop() {
        let voices = vec![Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        let session = PreviewSession::new(Uuid::new_v4(), voices.clone(), 1).unwrap();
        let sessions = Mutex::new(Some(session));

        let taken = take_preview_session_for_voice(&sessions, voices[1])
            .unwrap()
            .expect("secondary preview voice belongs to the session");
        assert_eq!(taken.all_voice_ids(), voices.as_slice());
        assert!(sessions.lock().unwrap().is_none());
    }

    #[test]
    fn stale_preview_decode_cannot_install_after_newer_request() {
        assert!(!preview_request_is_current(12, 11));
        assert!(preview_request_is_current(12, 12));
    }

    #[test]
    fn preview_start_response_exposes_the_session_generation() {
        let response = PreviewStartResult {
            voice_id: Uuid::new_v4().to_string(),
            generation: 42,
        };
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json["generation"], 42);
        assert!(json["voice_id"].as_str().is_some_and(|id| !id.is_empty()));
    }

    #[test]
    fn first_audio_number_action_defaults_to_master_only() {
        assert!(number_audio_should_be_master(CueType::Audio, None));
        assert!(number_audio_should_be_master(CueType::Audio, Some(CueType::Video)));
        assert!(!number_audio_should_be_master(CueType::Audio, Some(CueType::Audio)));
        assert!(!number_audio_should_be_master(CueType::Video, None));
        assert!(!number_audio_should_be_master(CueType::Image, None));
    }
}
