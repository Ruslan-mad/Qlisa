//! Compact, read-only diagnostics snapshots for the permanent Diagnostics page.
//!
//! The audio callback only updates atomics.  This module gathers those values
//! on an ordinary Tauri command thread and performs all formatting/aggregation
//! outside realtime audio.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::{
    cue::media_decode::{self, StreamPlaybackState, StreamSourceDiagnostics},
    cue::types::{CueState, CueType},
    engine::network_io::{NetworkInputRuntimeDiagnostics, NetworkOutputDiagnostics, NetworkOutputState, NetworkProtocol},
    engine::{audio_engine::StreamingVoiceDiagnostics, voice::VoiceState},
    state::AppState,
};

/// Show and focus the pre-created diagnostics window. Repeated opens reuse it.
#[tauri::command]
pub fn open_diagnostics_window(app_handle: AppHandle) -> Result<(), String> {
    let window = app_handle
        .get_webview_window("diagnostics")
        .ok_or("diagnostics window not found")?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsSnapshot {
    pub system: SystemDiagnostics,
    pub audio: AudioDiagnostics,
    pub video: VideoDiagnostics,
    pub network: NetworkDiagnostics,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NetworkDiagnostics {
    pub active_connections: usize,
    pub incoming_streams: usize,
    pub outgoing_streams: usize,
    pub connected: usize,
    pub reconnecting: Option<usize>,
    pub errors: usize,
    pub incoming_bitrate_kbps: Option<f64>,
    pub outgoing_bitrate_kbps: Option<f64>,
    pub connections: Vec<NetworkConnectionDiagnostics>,
    pub events: Vec<NetworkDiagnosticEvent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkConnectionDiagnostics {
    pub id: String,
    pub cue_id: Option<Uuid>,
    pub name: String,
    pub source_name: Option<String>,
    pub protocols: Vec<NetworkProtocol>,
    pub direction: String,
    pub state: String,
    pub endpoint: Option<String>,
    pub srt_mode: Option<String>,
    pub latency_ms: Option<u32>,
    pub received_frames: Option<u64>,
    pub submitted_frames: Option<u64>,
    pub dropped_frames: Option<u64>,
    pub superseded_frames: Option<u64>,
    pub received_audio_samples: Option<u64>,
    pub dropped_audio_samples: Option<u64>,
    pub ffmpeg_running: Option<bool>,
    pub ffmpeg_pid: Option<u32>,
    pub last_error: Option<String>,
    pub last_warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkDiagnosticEvent {
    pub at_ms: u64,
    pub connection_id: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Default)]
struct NetworkDiagnosticHistory {
    previous: HashMap<String, (String, Option<String>)>,
    events: VecDeque<NetworkDiagnosticEvent>,
}

static NETWORK_DIAGNOSTIC_HISTORY: OnceLock<Mutex<NetworkDiagnosticHistory>> = OnceLock::new();

fn network_history() -> &'static Mutex<NetworkDiagnosticHistory> {
    NETWORK_DIAGNOSTIC_HISTORY.get_or_init(|| Mutex::new(NetworkDiagnosticHistory::default()))
}

fn network_event_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn observe_network_connections(
    connections: &[NetworkConnectionDiagnostics],
) -> Vec<NetworkDiagnosticEvent> {
    let Ok(mut history) = network_history().lock() else {
        return Vec::new();
    };
    for connection in connections {
        let current = (connection.state.clone(), connection.last_error.clone());
        let previous = history.previous.insert(connection.id.clone(), current.clone());
        if previous.is_none() {
            history.events.push_back(NetworkDiagnosticEvent {
                at_ms: network_event_time_ms(),
                connection_id: connection.id.clone(),
                kind: "observed".into(),
                detail: current.0.clone(),
            });
        } else if let Some((old_state, old_error)) = previous {
            if old_state != current.0 {
                history.events.push_back(NetworkDiagnosticEvent {
                    at_ms: network_event_time_ms(),
                    connection_id: connection.id.clone(),
                    kind: "state_changed".into(),
                    detail: format!("{old_state} → {}", current.0),
                });
            }
            if old_error != current.1 && current.1.is_some() {
                history.events.push_back(NetworkDiagnosticEvent {
                    at_ms: network_event_time_ms(),
                    connection_id: connection.id.clone(),
                    kind: "error".into(),
                    detail: current.1.clone().unwrap_or_default(),
                });
            }
        }
    }
    history.previous.retain(|id, _| connections.iter().any(|connection| connection.id == *id));
    while history.events.len() > 128 {
        history.events.pop_front();
    }
    history.events.iter().cloned().collect()
}

pub fn reset_network_diagnostics() {
    if let Ok(mut history) = network_history().lock() {
        history.previous.clear();
        history.events.clear();
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SystemDiagnostics {
    pub process_cpu_percent: Option<f64>,
    pub process_working_set_bytes: Option<u64>,
    pub process_private_bytes: Option<u64>,
    pub gpu_usage_percent: Option<f64>,
    pub gpu_video_decode_percent: Option<f64>,
    pub gpu_memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioDiagnostics {
    pub health: String,
    pub source_counts: SourceCounts,
    pub scheduler: SchedulerDiagnostics,
    pub memory: AudioMemoryDiagnostics,
    pub peaks: AudioPeakDiagnostics,
    pub events: Vec<AudioDiagnosticEvent>,
    pub sources: Vec<AudioSourceDiagnostics>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VideoDiagnostics {
    pub active_cues: usize,
    pub active_decoders: usize,
    pub mpv_contexts: usize,
    pub outputs: usize,
    pub dropped_frames: u64,
    pub cues_decoded_multiple_times: usize,
    pub cues: Vec<VideoCueDiagnostics>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VideoCueDiagnostics {
    pub cue_id: Uuid,
    pub number: Option<String>,
    pub name: String,
    pub state: String,
    pub file: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub outputs: Vec<String>,
    pub output_count: usize,
    pub mpv_contexts: usize,
    pub mpv_render_contexts: usize,
    pub hardware_decoding: Option<bool>,
    pub hwdec_backend: Option<String>,
    pub decoder_format: Option<String>,
    pub dropped_frames: Option<u64>,
    pub delayed_frames: Option<u64>,
    pub time_pos_ms: Option<u64>,
    pub max_output_desync_ms: Option<u64>,
    pub render_calls: Option<u64>,
    pub average_render_time_us: Option<f64>,
    pub maximum_render_time_us: Option<u64>,
    pub preload: bool,
    pub playing: bool,
    pub paused: bool,
    pub eof: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SourceCounts {
    pub total: usize,
    pub playing: usize,
    pub paused: usize,
    pub preloading: usize,
    pub completed: usize,
    pub active_decoders: usize,
    pub underruns: u64,
    pub silent_frames: u64,
    pub decode_errors: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerDiagnostics {
    pub workers_total: usize,
    pub workers_busy: usize,
    pub workers_free: usize,
    pub queued_jobs: usize,
    pub peak_workers_busy: usize,
    pub peak_queued_jobs: usize,
    pub completed_jobs: u64,
    pub priority: SchedulerPriorityDiagnostics,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerPriorityDiagnostics {
    pub urgent: usize,
    pub playback: usize,
    pub preload: usize,
    pub paused: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioMemoryDiagnostics {
    pub ring_capacity_bytes: u64,
    pub pcm_buffered_bytes: u64,
    pub source_count: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioPeakDiagnostics {
    pub maximum_workers_busy: usize,
    pub maximum_queued_jobs: usize,
    pub maximum_streaming_memory_bytes: u64,
    pub maximum_playing_sources: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDiagnosticEvent {
    pub at_ms: u64,
    pub source_id: Uuid,
    pub source_label: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSourceDiagnostics {
    pub source_id: Uuid,
    pub voice_id: Option<Uuid>,
    pub label: String,
    pub path: String,
    pub state: String,
    pub buffered_frames: usize,
    pub buffered_seconds: f64,
    pub buffered_bytes: u64,
    pub capacity_frames: usize,
    pub capacity_bytes: u64,
    pub capacity_mib: f64,
    pub minimum_buffered_seconds: f64,
    pub maximum_buffered_seconds: f64,
    pub ready: bool,
    pub eof: bool,
    pub cancelled: bool,
    pub queued: bool,
    pub decoding: bool,
    pub refill_requested: bool,
    pub underruns: u64,
    pub initial_underruns: u64,
    pub regular_underruns: u64,
    pub silent_frames: u64,
    pub silent_seconds: f64,
    pub decode_failures: u64,
    pub last_refill_wait_us: u64,
    pub maximum_refill_wait_us: u64,
    pub last_decode_us: u64,
    pub maximum_decode_us: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub decoder: &'static str,
}

fn process_memory() -> SystemDiagnostics {
    #[cfg(windows)]
    {
        windows_process_memory()
    }
    #[cfg(not(windows))]
    {
        SystemDiagnostics::default()
    }
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct ProcessCpuSample {
    process_time_100ns: u64,
    wall_time: Instant,
}

#[cfg(windows)]
static PROCESS_CPU_SAMPLE: OnceLock<Mutex<Option<ProcessCpuSample>>> = OnceLock::new();

/// Calculate process CPU as a percentage of the whole machine.  A process
/// using one full logical core on an eight-core machine reports 12.5%, not
/// 100%. The first sample has no interval and therefore returns `None`.
fn calculate_process_cpu_percent(
    previous_process_time_100ns: u64,
    current_process_time_100ns: u64,
    wall_interval: Duration,
    logical_processors: usize,
) -> Option<f64> {
    if logical_processors == 0 || wall_interval.is_zero() || current_process_time_100ns < previous_process_time_100ns {
        return None;
    }
    let wall_seconds = wall_interval.as_secs_f64();
    if wall_seconds <= f64::EPSILON {
        return None;
    }
    let process_seconds = current_process_time_100ns.saturating_sub(previous_process_time_100ns) as f64 / 10_000_000.0;
    Some((process_seconds / wall_seconds / logical_processors as f64 * 100.0).clamp(0.0, 100.0))
}

#[cfg(windows)]
fn process_cpu_percent() -> Option<f64> {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "Kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn GetProcessTimes(
            process: *mut std::ffi::c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    fn to_100ns(time: FileTime) -> u64 {
        (u64::from(time.high) << 32) | u64::from(time.low)
    }

    let mut creation = FileTime::default();
    let mut exit = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } != 0;
    if !ok {
        return None;
    }
    let now = Instant::now();
    let current = ProcessCpuSample {
        process_time_100ns: to_100ns(kernel).saturating_add(to_100ns(user)),
        wall_time: now,
    };
    let gate = PROCESS_CPU_SAMPLE.get_or_init(|| Mutex::new(None));
    let mut previous = gate.lock().ok()?;
    let value = previous.and_then(|sample| {
        calculate_process_cpu_percent(
            sample.process_time_100ns,
            current.process_time_100ns,
            current.wall_time.duration_since(sample.wall_time),
            std::thread::available_parallelism().map(|value| value.get()).unwrap_or(1),
        )
    });
    *previous = Some(current);
    value
}

#[cfg(not(windows))]
fn process_cpu_percent() -> Option<f64> {
    None
}

#[cfg(windows)]
fn windows_process_memory() -> SystemDiagnostics {
    #[repr(C)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    #[link(name = "Psapi")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut ProcessMemoryCountersEx,
            size: u32,
        ) -> i32;
    }
    unsafe {
        let mut counters = ProcessMemoryCountersEx {
            cb: std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 0,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
            private_usage: 0,
        };
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        ) != 0 {
            return SystemDiagnostics {
                process_cpu_percent: process_cpu_percent(),
                process_working_set_bytes: Some(counters.working_set_size as u64),
                process_private_bytes: Some(counters.private_usage as u64),
                gpu_usage_percent: None,
                gpu_video_decode_percent: None,
                gpu_memory_bytes: None,
            };
        }
    }
    SystemDiagnostics::default()
}

fn state_label(state: StreamPlaybackState, voice: Option<VoiceState>, source: &StreamSourceDiagnostics) -> &'static str {
    if source.cancelled || matches!(voice, Some(VoiceState::Stopped)) {
        return "completed";
    }
    match voice {
        Some(VoiceState::Playing | VoiceState::FadingOut) => "playing",
        Some(VoiceState::Paused) => "paused",
        _ => match state {
            StreamPlaybackState::Playing => "playing",
            StreamPlaybackState::Paused => "paused",
            StreamPlaybackState::Preload => "preloading",
        },
    }
}

fn make_source(source: StreamSourceDiagnostics, voice: Option<StreamingVoiceDiagnostics>, cue_label: Option<&str>) -> AudioSourceDiagnostics {
    let voice_state = voice.map(|v| v.state);
    let state = state_label(source.state, voice_state, &source).to_string();
    let buffered_bytes = (source.buffered_frames * source.channels.max(1) as usize * std::mem::size_of::<f32>()) as u64;
    let capacity_bytes = (source.capacity_frames * source.channels.max(1) as usize * std::mem::size_of::<f32>()) as u64;
    AudioSourceDiagnostics {
        source_id: source.source_id,
        voice_id: voice.map(|v| v.voice_id),
        label: cue_label.map(str::to_owned).unwrap_or_else(|| {
            std::path::Path::new(&source.path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&source.path)
                .to_string()
        }),
        path: source.path,
        state,
        buffered_frames: source.buffered_frames,
        buffered_seconds: source.buffered_seconds,
        buffered_bytes,
        capacity_frames: source.capacity_frames,
        capacity_bytes,
        capacity_mib: source.capacity_mib,
        minimum_buffered_seconds: source.min_buffered_frames as f64 / source.sample_rate.max(1) as f64,
        maximum_buffered_seconds: source.max_buffered_frames as f64 / source.sample_rate.max(1) as f64,
        ready: source.ready,
        eof: source.eof,
        cancelled: source.cancelled,
        queued: source.job_requested,
        decoding: source.job_running,
        refill_requested: source.refill_requested,
        underruns: source.underruns,
        initial_underruns: source.initial_underruns,
        regular_underruns: source.regular_underruns,
        silent_frames: source.silent_frames,
        silent_seconds: source.silent_frames as f64 / source.sample_rate.max(1) as f64,
        decode_failures: source.decode_failures,
        last_refill_wait_us: source.last_refill_wait_us,
        maximum_refill_wait_us: source.max_refill_wait_us,
        last_decode_us: source.last_decode_us,
        maximum_decode_us: source.max_decode_us,
        sample_rate: source.sample_rate,
        channels: source.channels,
        decoder: "symphonia",
    }
}

fn cue_state_code(state: CueState) -> String {
    match state {
        CueState::Standby => "standby",
        CueState::Running => "running",
        CueState::Paused => "paused",
        CueState::Completed => "completed",
    }
    .to_owned()
}

fn collect_video_cues<'a>(cues: &'a [Box<dyn crate::cue::traits::Cue>], out: &mut Vec<&'a dyn crate::cue::traits::Cue>) {
    for cue in cues {
        if cue.cue_type() == CueType::Video {
            out.push(cue.as_ref());
        }
        if let Some(children) = cue.child_cues() {
            collect_video_cues(children, out);
        }
    }
}

fn sum_optional(values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    let mut total = 0_u64;
    let mut found = false;
    for value in values.flatten() {
        found = true;
        total = total.saturating_add(value);
    }
    found.then_some(total)
}

fn max_desync_ms(values: &[u64]) -> Option<u64> {
    (values.len() >= 2).then(|| values.iter().max().unwrap().saturating_sub(*values.iter().min().unwrap()))
}

fn make_video_diagnostics(
    cue: &dyn crate::cue::traits::Cue,
    records: &[crate::engine::output_engine::VideoRuntimeDiagnostic],
) -> VideoCueDiagnostics {
    let mut outputs = records.iter().map(|record| record.output_id.clone()).collect::<Vec<_>>();
    outputs.sort();
    outputs.dedup();
    let times = records.iter().filter_map(|record| record.time_pos_ms).collect::<Vec<_>>();
    let max_output_desync_ms = max_desync_ms(&times);
    let first = records.first();
    let width = first.and_then(|record| record.width);
    let height = first.and_then(|record| record.height);
    let fps = first.and_then(|record| record.fps);
    let hwdec_backend = records.iter().find_map(|record| record.hwdec_backend.clone());
    let hardware_decoding = hwdec_backend.as_ref().map(|backend| !backend.eq_ignore_ascii_case("no"));
    let preload = records.iter().any(|record| record.preloaded);
    let eof = records.iter().any(|record| record.eof == Some(true));
    let paused = records.iter().any(|record| record.paused == Some(true));
    let playing = !records.is_empty() && !preload && !paused && !eof;
    VideoCueDiagnostics {
        cue_id: cue.id(),
        number: cue.number().map(str::to_owned),
        name: cue.name().to_owned(),
        state: cue_state_code(cue.state()),
        file: cue.media_file_path().map(|path| path.to_string_lossy().into_owned()),
        width,
        height,
        fps,
        output_count: outputs.len(),
        outputs,
        mpv_contexts: records.iter().filter(|record| record.mpv_context).count(),
        mpv_render_contexts: records.iter().filter(|record| record.render_context).count(),
        hardware_decoding,
        hwdec_backend,
        decoder_format: records.iter().find_map(|record| record.decoder_format.clone()),
        dropped_frames: sum_optional(records.iter().map(|record| record.dropped_frames)),
        delayed_frames: sum_optional(records.iter().map(|record| record.delayed_frames)),
        time_pos_ms: first.and_then(|record| record.time_pos_ms),
        max_output_desync_ms,
        // Render timing is intentionally unavailable: collecting it would
        // require changing the renderer hot path, which diagnostics must not do.
        render_calls: None,
        average_render_time_us: None,
        maximum_render_time_us: None,
        preload,
        playing,
        paused,
        eof,
    }
}

fn collect_video_snapshot(state: &AppState) -> VideoDiagnostics {
    let records = state.output_engine.video_runtime_diagnostics();
    let mut by_voice = HashMap::<Uuid, Vec<crate::engine::output_engine::VideoRuntimeDiagnostic>>::new();
    for record in records {
        by_voice.entry(record.cue_voice_id).or_default().push(record);
    }
    let mut cues = Vec::new();
    if let Ok(workspace) = state.workspace.lock() {
        if let Some(list) = workspace.active_cue_list() {
            let mut video_cues = Vec::new();
            collect_video_cues(&list.cues, &mut video_cues);
            for cue in video_cues {
                let records = cue.playing_voice_id().and_then(|voice| by_voice.get(&voice)).map(Vec::as_slice).unwrap_or(&[]);
                cues.push(make_video_diagnostics(cue, records));
            }
        }
    }
    let active_cues = cues.iter().filter(|cue| cue.playing || cue.paused || cue.preload).count();
    let active_decoders = cues.iter().map(|cue| cue.mpv_contexts).sum();
    let mpv_contexts = cues.iter().map(|cue| cue.mpv_contexts).sum();
    let outputs = cues.iter().flat_map(|cue| cue.outputs.iter().cloned()).collect::<std::collections::HashSet<_>>().len();
    let dropped_frames = cues.iter().filter_map(|cue| cue.dropped_frames).sum();
    let cues_decoded_multiple_times = cues.iter().filter(|cue| cue.mpv_contexts > 1).count();
    VideoDiagnostics { active_cues, active_decoders, mpv_contexts, outputs, dropped_frames, cues_decoded_multiple_times, cues }
}

fn srt_mode_label(mode: crate::engine::network_io::SrtMode) -> String {
    match mode {
        crate::engine::network_io::SrtMode::Listener => "listener",
        crate::engine::network_io::SrtMode::Caller => "caller",
        crate::engine::network_io::SrtMode::Rendezvous => "rendezvous",
    }
    .to_owned()
}

fn input_connection(
    cue: &dyn crate::cue::traits::Cue,
    runtime: NetworkInputRuntimeDiagnostics,
) -> NetworkConnectionDiagnostics {
    let state = if runtime.last_error.is_some() {
        "error"
    } else if runtime.receiving {
        "connected"
    } else if runtime.video_eof
        || runtime.audio_eof
        || runtime.ffmpeg_running == Some(false)
    {
        "disconnected"
    } else if matches!(cue.state(), CueState::Running | CueState::Paused) {
        "connecting"
    } else {
        "disconnected"
    };
    NetworkConnectionDiagnostics {
        id: cue.id().to_string(),
        cue_id: Some(cue.id()),
        name: cue.name().to_owned(),
        source_name: Some(runtime.source_name),
        protocols: vec![runtime.protocol],
        direction: "input".into(),
        state: state.into(),
        endpoint: runtime.endpoint,
        srt_mode: runtime.srt_mode.map(srt_mode_label),
        latency_ms: runtime.latency_ms,
        received_frames: Some(runtime.received_frames),
        submitted_frames: None,
        dropped_frames: runtime.dropped_frames,
        superseded_frames: runtime.superseded_frames,
        received_audio_samples: runtime.received_audio_samples,
        dropped_audio_samples: runtime.dropped_audio_samples,
        ffmpeg_running: runtime.ffmpeg_running,
        ffmpeg_pid: runtime.ffmpeg_pid,
        last_error: runtime.last_error,
        last_warning: runtime.last_warning,
    }
}

fn output_connection(status: NetworkOutputDiagnostics) -> NetworkConnectionDiagnostics {
    let state = match status.state {
        NetworkOutputState::Disabled => "disabled",
        NetworkOutputState::WaitingForFrame => "starting",
        NetworkOutputState::Streaming => "connected",
        NetworkOutputState::Error => "error",
        NetworkOutputState::Stopped => "disconnected",
    };
    NetworkConnectionDiagnostics {
        id: status.output_id.clone(),
        cue_id: None,
        name: status.output_id,
        source_name: None,
        protocols: status.protocols,
        direction: "output".into(),
        state: state.into(),
        endpoint: status.endpoint,
        srt_mode: status.srt_mode.map(srt_mode_label),
        latency_ms: status.latency_ms,
        received_frames: None,
        submitted_frames: Some(status.submitted_frames),
        dropped_frames: None,
        superseded_frames: Some(status.superseded_frames),
        received_audio_samples: None,
        dropped_audio_samples: None,
        ffmpeg_running: None,
        ffmpeg_pid: None,
        last_error: status.last_error,
        last_warning: None,
    }
}

fn collect_network_snapshot(
    state: &AppState,
    workspace: Option<&crate::show::Workspace>,
) -> NetworkDiagnostics {
    let mut connections = Vec::new();
    if let Some(workspace) = workspace {
        if let Some(list) = workspace.active_cue_list() {
            fn collect(cues: &[Box<dyn crate::cue::traits::Cue>], out: &mut Vec<NetworkConnectionDiagnostics>) {
                for cue in cues {
                    if let Some(runtime) = cue.network_input_diagnostics() {
                        out.push(input_connection(cue.as_ref(), runtime));
                    }
                    if let Some(children) = cue.child_cues() {
                        collect(children, out);
                    }
                }
            }
            collect(&list.cues, &mut connections);
        }
    }
    connections.extend(state.output_engine.network_output_diagnostics().into_iter().map(output_connection));
    let incoming_streams = connections.iter().filter(|item| item.direction == "input").count();
    let outgoing_streams = connections.iter().filter(|item| item.direction == "output").count();
    let connected = connections.iter().filter(|item| item.state == "connected").count();
    let errors = connections.iter().filter(|item| item.state == "error" || item.last_error.is_some()).count();
    let events = observe_network_connections(&connections);
    NetworkDiagnostics {
        active_connections: connections
            .iter()
            .filter(|item| !matches!(item.state.as_str(), "disabled" | "disconnected"))
            .count(),
        incoming_streams,
        outgoing_streams,
        connected,
        reconnecting: None,
        errors,
        incoming_bitrate_kbps: None,
        outgoing_bitrate_kbps: None,
        connections,
        events,
    }
}

fn collect_snapshot(state: &AppState) -> DiagnosticsSnapshot {
    let pool = media_decode::stream_pool_diagnostics();
    let raw_sources = media_decode::stream_source_diagnostics();
    let voices = state.audio_engine.streaming_voice_diagnostics();
    let voice_map: HashMap<Uuid, StreamingVoiceDiagnostics> = voices.into_iter().map(|v| (v.source_id, v)).collect();
    let mut cue_labels = HashMap::<Uuid, String>::new();
    if let Ok(workspace) = state.workspace.lock() {
        if let Some(list) = workspace.active_cue_list() {
            fn collect(cues: &[Box<dyn crate::cue::traits::Cue>], labels: &mut HashMap<Uuid, String>) {
                for cue in cues {
                    let label = cue.number()
                        .map(|number| format!("#{number} {}", cue.name()))
                        .unwrap_or_else(|| cue.name().to_string());
                    for voice_id in cue.all_voice_ids() { labels.insert(voice_id, label.clone()); }
                    if let Some(children) = cue.child_cues() { collect(children, labels); }
                }
            }
            collect(&list.cues, &mut cue_labels);
        }
    }
    let sources: Vec<_> = raw_sources
        .into_iter()
        .map(|source| {
            let source_id = source.source_id;
            let cue_label = voice_map.get(&source_id).and_then(|voice| cue_labels.get(&voice.voice_id));
            make_source(source, voice_map.get(&source_id).copied(), cue_label.map(String::as_str))
        })
        .collect();
    let mut counts = SourceCounts { total: sources.len(), ..SourceCounts::default() };
    let mut memory = AudioMemoryDiagnostics {
        ring_capacity_bytes: pool.current_streaming_memory_bytes,
        source_count: pool.current_sources,
        ..AudioMemoryDiagnostics::default()
    };
    for source in &sources {
        match source.state.as_str() {
            "playing" => counts.playing += 1,
            "paused" => counts.paused += 1,
            "preloading" => counts.preloading += 1,
            "completed" => counts.completed += 1,
            _ => {}
        }
        if source.decoding { counts.active_decoders += 1; }
        counts.underruns = counts.underruns.saturating_add(source.underruns);
        counts.silent_frames = counts.silent_frames.saturating_add(source.silent_frames);
        counts.decode_errors = counts.decode_errors.saturating_add(source.decode_failures);
        if !source.cancelled {
            memory.pcm_buffered_bytes = memory.pcm_buffered_bytes.saturating_add(source.buffered_bytes);
        }
    }
    let events = media_decode::stream_diagnostic_events()
        .into_iter()
        .map(|event| AudioDiagnosticEvent { at_ms: event.at_ms, source_id: event.source_id, source_label: event.source_label, kind: event.kind, detail: event.detail })
        .collect();
    let health = if counts.decode_errors > 0 { "error" } else if counts.underruns > 0 { "warning" } else { "ok" };
    let scheduler = SchedulerDiagnostics {
        workers_total: pool.worker_count,
        workers_busy: pool.active_workers,
        workers_free: pool.worker_count.saturating_sub(pool.active_workers),
        queued_jobs: pool.pending_jobs,
        peak_workers_busy: pool.peak_active_workers,
        peak_queued_jobs: pool.peak_pending_jobs,
        completed_jobs: pool.completed_jobs,
        priority: SchedulerPriorityDiagnostics {
            urgent: pool.priority_urgent,
            playback: pool.priority_playback,
            preload: pool.priority_preload,
            paused: pool.priority_paused,
        },
    };
    let peaks = AudioPeakDiagnostics {
        maximum_workers_busy: pool.peak_active_workers,
        maximum_queued_jobs: pool.peak_pending_jobs,
        maximum_streaming_memory_bytes: pool.peak_streaming_memory_bytes,
        maximum_playing_sources: pool.peak_playing_sources,
    };
    let network = state
        .workspace
        .lock()
        .ok()
        .map(|workspace| collect_network_snapshot(state, Some(&workspace)))
        .unwrap_or_else(|| collect_network_snapshot(state, None));
    DiagnosticsSnapshot {
        system: process_memory(),
        audio: AudioDiagnostics { health: health.to_string(), source_counts: counts, scheduler, memory, peaks, events, sources },
        video: collect_video_snapshot(state),
        network,
    }
}

#[tauri::command]
pub fn get_diagnostics_snapshot(state: State<'_, AppState>) -> DiagnosticsSnapshot {
    collect_snapshot(&state)
}

#[tauri::command]
pub fn reset_diagnostics_statistics() {
    media_decode::reset_stream_diagnostics();
    reset_network_diagnostics();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_snapshot_serializes_with_camel_case() {
        let json = serde_json::to_value(AudioSourceDiagnostics {
            source_id: Uuid::nil(), voice_id: None, label: "x.wav".into(), path: "x.wav".into(), state: "playing".into(),
            buffered_frames: 1, buffered_seconds: 1.0, buffered_bytes: 8, capacity_frames: 2,
            capacity_bytes: 16, capacity_mib: 0.1, minimum_buffered_seconds: 0.1,
            maximum_buffered_seconds: 1.0, ready: true, eof: false, cancelled: false,
            queued: false, decoding: false, refill_requested: false, underruns: 0,
            initial_underruns: 0, regular_underruns: 0, silent_frames: 0, silent_seconds: 0.0,
            decode_failures: 0, last_refill_wait_us: 0, maximum_refill_wait_us: 0,
            last_decode_us: 0, maximum_decode_us: 0, sample_rate: 48_000, channels: 2,
            decoder: "symphonia",
        }).unwrap();
        assert!(json.get("initialUnderruns").is_some());
        assert!(json.get("maximumDecodeUs").is_some());
    }

    #[test]
    fn unavailable_mpv_counters_stay_unavailable() {
        assert_eq!(sum_optional([None, None].into_iter()), None);
        assert_eq!(sum_optional([Some(2), None, Some(3)].into_iter()), Some(5));
    }

    #[test]
    fn desync_is_only_reported_for_multiple_positions() {
        assert_eq!(max_desync_ms(&[]), None);
        assert_eq!(max_desync_ms(&[14]), None);
        assert_eq!(max_desync_ms(&[100, 114, 107]), Some(14));
    }

    #[test]
    fn process_cpu_uses_wall_interval_and_logical_processor_count() {
        assert_eq!(calculate_process_cpu_percent(100, 100, Duration::from_millis(20), 2), None);
        let value = calculate_process_cpu_percent(0, 10_000_000, Duration::from_secs(1), 2).unwrap();
        assert!((value - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn network_snapshot_serializes_runtime_fields_without_fake_rates() {
        let snapshot = NetworkDiagnostics {
            active_connections: 1,
            incoming_streams: 1,
            outgoing_streams: 0,
            connected: 1,
            reconnecting: None,
            errors: 0,
            incoming_bitrate_kbps: None,
            outgoing_bitrate_kbps: None,
            connections: vec![NetworkConnectionDiagnostics {
                id: "cue".into(),
                cue_id: Some(Uuid::nil()),
                name: "Камера".into(),
                source_name: Some("NDI Camera".into()),
                protocols: vec![NetworkProtocol::Ndi],
                direction: "input".into(),
                state: "connected".into(),
                endpoint: None,
                srt_mode: None,
                latency_ms: None,
                received_frames: Some(12),
                submitted_frames: None,
                dropped_frames: Some(2),
                superseded_frames: Some(4),
                received_audio_samples: None,
                dropped_audio_samples: None,
                ffmpeg_running: None,
                ffmpeg_pid: None,
                last_error: None,
                last_warning: Some("non-existing PPS referenced".into()),
            }],
            events: Vec::new(),
        };
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(json["connections"][0]["sourceName"], "NDI Camera");
        assert_eq!(json["connections"][0]["lastWarning"], "non-existing PPS referenced");
        assert!(json["incomingBitrateKbps"].is_null());
    }
}
