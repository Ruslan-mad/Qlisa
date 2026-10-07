//! Lightweight status bar runtime snapshots. Expensive probes run off the
//! command path and return the most recent cached result.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};

use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusRuntimeSnapshot {
    pub generation: u64,
    pub timestamp_ms: u64,
    /// Timestamp for the separately cached workspace network diagnostics.
    /// Other fields use `timestamp_ms` and remain fresh when the workspace is busy.
    pub network_timestamp_ms: u64,
    pub audio: Option<AudioGapSnapshot>,
    pub video_outputs: Vec<VideoOutputSnapshot>,
    pub network: Vec<NetworkSnapshot>,
    pub project_disk: Option<DiskSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioGapSnapshot {
    pub underrun_events: Option<u64>,
    pub silent_frames: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoOutputSnapshot {
    pub id: String,
    pub name: String,
    pub fps: Option<f64>,
    pub target_fps: Option<f64>,
    pub dropped_frames: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSnapshot {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub direction: String,
    pub active: bool,
    pub dropped_frames: Option<u64>,
    pub dropped_audio_samples: Option<u64>,
    pub dropped_audio_frames: Option<u64>,
    pub superseded_frames: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskSnapshot {
    pub path: Option<String>,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub source: Option<String>,
}

static GENERATION: AtomicU64 = AtomicU64::new(0);
static DISK_REFRESHING: AtomicBool = AtomicBool::new(false);
static VIDEO_DROP_REFRESHING: AtomicBool = AtomicBool::new(false);
static DISK_CACHE: OnceLock<Mutex<Option<(PathBuf, Instant, DiskSnapshot)>>> = OnceLock::new();
static VIDEO_DROP_CACHE: OnceLock<Mutex<Option<(Instant, HashMap<String, Option<u64>>)>>>
    = OnceLock::new();
static RENDER_SAMPLE: OnceLock<Mutex<HashMap<String, (u64, Instant)>>> = OnceLock::new();
static NETWORK_INPUT_CACHE: OnceLock<Mutex<Option<(String, Instant, u64, Vec<NetworkSnapshot>)>>> = OnceLock::new();

/// Return cached runtime counters. Disk space is sampled in a background
/// worker at most once every ten seconds; no workspace lock is held during it.
#[tauri::command]
pub fn get_status_runtime_snapshot(
    state: State<'_, AppState>,
    app: AppHandle,
) -> StatusRuntimeSnapshot {
    let (underrun_events, silent_frames) = state.audio_engine.runtime_audio_gap_counters();
    let now = Instant::now();
    let counters = state.output_engine.physical_present_counters();
    let video_drops = cached_video_drop_counts(Arc::clone(&state.output_engine));
    let video_outputs = if let Ok(mut samples) = RENDER_SAMPLE.get_or_init(|| Mutex::new(HashMap::new())).try_lock() {
        let current_ids = counters.iter().map(|(id, _, _)| id.clone()).collect::<HashSet<_>>();
        samples.retain(|id, _| current_ids.contains(id));
        counters.into_iter().map(|(id, name, frames)| {
            let fps = samples.get(&id).and_then(|(previous, sampled_at)| {
                let elapsed = now.duration_since(*sampled_at).as_secs_f64();
                let delta = frames.saturating_sub(*previous);
                (elapsed > 0.0 && delta > 0).then_some(delta as f64 / elapsed)
            });
            samples.insert(id.clone(), (frames, now));
            let dropped_frames = video_drops.get(&id).copied().flatten();
            VideoOutputSnapshot { id, name, fps, target_fps: None, dropped_frames }
        }).collect()
    } else {
        counters.into_iter().map(|(id, name, _)| VideoOutputSnapshot {
            dropped_frames: video_drops.get(&id).copied().flatten(),
            id, name, fps: None, target_fps: None,
        }).collect()
    };

    let path = state.workspace.try_lock().ok().map(|workspace| {
        workspace.file_path.clone().map(|file| {
            file.parent().map(Path::to_path_buf).unwrap_or(file)
        })
    });
    let disk = match path {
        Some(path) => cached_disk_snapshot(path, app),
        None => latest_disk_snapshot(),
    };
    let (mut network, network_timestamp_ms) = cached_network_inputs(&state.workspace);
    network.extend(state.output_engine.network_output_diagnostics().into_iter().map(|output| {
        let kind = output.protocols.iter().map(|protocol| match protocol {
            crate::engine::network_io::NetworkProtocol::Ndi => "NDI",
            crate::engine::network_io::NetworkProtocol::Srt => "SRT",
        }).collect::<Vec<_>>().join("/");
        let active = !matches!(output.state,
            crate::engine::network_io::NetworkOutputState::Disabled
                | crate::engine::network_io::NetworkOutputState::Stopped);
        NetworkSnapshot {
            id: output.output_id.clone(),
            name: output.output_id,
            kind,
            direction: "output".into(),
            active,
            dropped_frames: None,
            dropped_audio_samples: None,
            dropped_audio_frames: output.dropped_audio_frames,
            // This is bounded latest-frame queue replacement. It can be
            // normal output pacing, so the UI labels it separately.
            superseded_frames: Some(output.superseded_frames),
        }
    }));

    StatusRuntimeSnapshot {
        generation: GENERATION.fetch_add(1, Ordering::Relaxed).wrapping_add(1),
        timestamp_ms: SystemTime::now().duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64).unwrap_or(0),
        network_timestamp_ms,
        audio: Some(AudioGapSnapshot {
            underrun_events: Some(underrun_events),
            silent_frames: Some(silent_frames),
        }),
        video_outputs,
        network,
        project_disk: disk,
    }
}

fn cached_video_drop_counts(
    output_engine: Arc<crate::engine::OutputEngine>,
) -> HashMap<String, Option<u64>> {
    let cache = VIDEO_DROP_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(snapshot) = cache.lock() {
        if let Some((sampled_at, values)) = snapshot.as_ref() {
            if sampled_at.elapsed() < Duration::from_secs(1) {
                return values.clone();
            }
        }
    }
    if !VIDEO_DROP_REFRESHING.swap(true, Ordering::AcqRel) {
        let spawned = std::thread::Builder::new()
            .name("qlisa-status-video-drops".into())
            .spawn(move || {
                let mut values = HashMap::<String, Option<u64>>::new();
                for diagnostic in output_engine.video_runtime_diagnostics() {
                    let value = values.entry(diagnostic.output_id).or_insert(None);
                    if let Some(dropped) = diagnostic.dropped_frames {
                        *value = Some(value.unwrap_or(0).saturating_add(dropped));
                    }
                }
                if let Ok(mut snapshot) = VIDEO_DROP_CACHE.get_or_init(|| Mutex::new(None)).lock() {
                    *snapshot = Some((Instant::now(), values));
                }
                VIDEO_DROP_REFRESHING.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            VIDEO_DROP_REFRESHING.store(false, Ordering::Release);
        }
    }
    cache.lock().ok().and_then(|snapshot| snapshot.as_ref().map(|(_, values)| values.clone()))
        .unwrap_or_default()
}

fn cached_network_inputs(workspace: &Mutex<crate::show::Workspace>) -> (Vec<NetworkSnapshot>, u64) {
    let cache = NETWORK_INPUT_CACHE.get_or_init(|| Mutex::new(None));
    let Ok(workspace) = workspace.try_lock() else {
        return cache.lock().ok()
            .and_then(|snapshot| snapshot.as_ref().map(|(_, _, timestamp, rows)| (rows.clone(), *timestamp)))
            .unwrap_or_default();
    };
    let workspace_key = format!(
        "{}|{}|{:?}",
        workspace.metadata.created_at.to_rfc3339(),
        workspace.file_path.as_deref().unwrap_or(Path::new("")).display(),
        workspace.cue_lists.iter().map(|list| list.id).collect::<Vec<_>>(),
    );
    if let Ok(snapshot) = cache.lock() {
        if let Some((cached_key, sampled_at, timestamp, rows)) = snapshot.as_ref() {
            if *cached_key == workspace_key && sampled_at.elapsed() < Duration::from_secs(1) {
                return (rows.clone(), *timestamp);
            }
        }
    }
    let mut rows = Vec::new();
    for list in &workspace.cue_lists {
        collect_network_inputs(&list.cues, &mut rows);
    }
    let timestamp_ms = SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64).unwrap_or(0);
    if let Ok(mut snapshot) = cache.lock() {
        *snapshot = Some((workspace_key, Instant::now(), timestamp_ms, rows.clone()));
    }
    (rows, timestamp_ms)
}

fn collect_network_inputs(
    cues: &[Box<dyn crate::cue::traits::Cue>],
    rows: &mut Vec<NetworkSnapshot>,
) {
    for cue in cues {
        if let Some(runtime) = cue.network_input_diagnostics() {
            let kind = match runtime.protocol {
                crate::engine::network_io::NetworkProtocol::Ndi => "NDI",
                crate::engine::network_io::NetworkProtocol::Srt => "SRT",
            };
            rows.push(NetworkSnapshot {
                id: format!("input:{}", cue.id()),
                name: cue.name().to_owned(),
                kind: kind.into(),
                direction: "input".into(),
                active: runtime.receiving,
                dropped_frames: runtime.dropped_frames,
                dropped_audio_samples: runtime.dropped_audio_samples,
                dropped_audio_frames: None,
                superseded_frames: runtime.superseded_frames,
            });
        }
        if let Some(children) = cue.child_cues() {
            collect_network_inputs(children, rows);
        }
    }
}

fn cached_disk_snapshot(path: Option<PathBuf>, app: AppHandle) -> Option<DiskSnapshot> {
    let cache = DISK_CACHE.get_or_init(|| Mutex::new(None));
    let cached = cache.lock().ok()?.clone();
    let project_path = path.is_some();
    let target = match path {
        Some(path) => path,
        None => app.path().app_data_dir().ok()?,
    };
    let target = std::fs::canonicalize(&target).ok()?;
    if let Some((cached_path, sampled_at, snapshot)) = cached {
        if cached_path == target && sampled_at.elapsed() < Duration::from_secs(10) {
            return Some(snapshot);
        }
    }
    if !DISK_REFRESHING.swap(true, Ordering::AcqRel) {
        let probe_path = target.clone();
        let spawned = std::thread::Builder::new()
            .name("qlisa-status-disk".into())
            .spawn(move || {
                let result = probe_disk(&probe_path, project_path);
                if let (Ok(mut cache), Some(snapshot)) = (DISK_CACHE.get_or_init(|| Mutex::new(None)).lock(), result) {
                    *cache = Some((probe_path, Instant::now(), snapshot));
                }
                DISK_REFRESHING.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            DISK_REFRESHING.store(false, Ordering::Release);
        }
    }
    cache.lock().ok()?.as_ref()
        .filter(|(cached_path, _, _)| cached_path == &target)
        .map(|(_, _, snapshot)| snapshot.clone())
}

fn latest_disk_snapshot() -> Option<DiskSnapshot> {
    DISK_CACHE.get_or_init(|| Mutex::new(None)).lock().ok()?
        .as_ref().map(|(_, _, snapshot)| snapshot.clone())
}

fn probe_disk(path: &Path, project_path: bool) -> Option<DiskSnapshot> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);
        let mut free = 0u64;
        let mut total = 0u64;
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut(),
            )
        };
        return (ok != 0).then(|| DiskSnapshot {
            path: Some(path.to_string_lossy().into_owned()),
            total_bytes: Some(total),
            free_bytes: Some(free),
            source: Some(if project_path { "project volume" } else { "application profile volume" }.into()),
        });
    }
    #[cfg(not(windows))]
    {
        let _ = (path, project_path);
        None
    }
}
