//! Isolated media conversion through the bundled FFmpeg runtime.
//!
//! The converter deliberately uses the same managed child helper as network
//! playback. Every conversion has a private Windows Job Object, a temporary
//! output, and an atomic final rename.
//!
//! `ffmpeg-sidecar` was evaluated but is intentionally not a dependency here:
//! its convenience runner owns a plain `std::process::Child`, while Qlisa must
//! assign the child to the existing per-process Windows Job Object immediately
//! after spawn. The shared `spawn_managed_ffmpeg` helper provides progress via
//! FFmpeg's `-progress pipe:1` protocol and preserves the stronger lifecycle
//! guarantee without changing network playback.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Emitter;
use uuid::Uuid;

use crate::engine::network_io::{find_ffmpeg_runtime, spawn_managed_ffmpeg, ManagedFfmpegChild};

pub const MEDIA_CONVERSION_EVENT: &str = "media-conversion-event";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversionMode {
    Optimize,
    Size,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConversionRequest {
    pub cue_id: Option<String>,
    pub input_path: String,
    pub mode: ConversionMode,
    /// H.264 CRF. Lower values retain more quality. The safe range is 0..=51.
    pub quality: Option<u8>,
    /// Maximum width or height, preserving aspect ratio.
    pub max_resolution: Option<u32>,
    /// Output format for Image Cues. Ignored for audio and video.
    #[serde(default)]
    pub image_format: Option<String>,
}

/// The authored Cue type determines which stream should be converted.  In
/// particular, audio files often carry an attached cover-art image stream;
/// that stream is metadata, not video, and must never make an audio Cue take
/// the video conversion path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaKind {
    Audio,
    Video,
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProbeMetadata {
    pub path: String,
    pub file_size: u64,
    pub format_name: Option<String>,
    pub duration_seconds: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub video_codec: Option<String>,
    pub video_profile: Option<String>,
    pub video_bit_depth: Option<u8>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<u32>,
    pub sample_rate: Option<u32>,
    pub pixel_format: Option<String>,
    pub frame_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityStatus {
    Compatible,
    Recommended,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaCompatibility {
    pub status: CompatibilityStatus,
    pub compatible: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversionStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversionErrorCategory {
    Input,
    RuntimeMissing,
    Cancelled,
    Process,
    Output,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConversionJob {
    pub id: String,
    pub cue_id: Option<String>,
    pub input_path: String,
    pub output_path: Option<String>,
    pub mode: ConversionMode,
    pub status: ConversionStatus,
    pub progress: Option<f32>,
    pub processed: Option<f64>,
    pub total: Option<f64>,
    pub speed: Option<f64>,
    pub source_size: Option<u64>,
    pub new_size: Option<u64>,
    pub error: Option<String>,
    pub error_category: Option<ConversionErrorCategory>,
    pub diagnostic_log_path: Option<String>,
    pub applied_to_cue: bool,
}

struct JobRecord {
    state: Mutex<MediaConversionJob>,
    cancel: AtomicBool,
    child: Mutex<Option<ManagedFfmpegChild>>,
    temp_path: PathBuf,
    reservation_path: PathBuf,
    diagnostic_path: PathBuf,
    app: tauri::AppHandle,
}

impl JobRecord {
    fn snapshot(&self) -> MediaConversionJob {
        self.state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| MediaConversionJob {
                id: String::new(),
                cue_id: None,
                input_path: String::new(),
                output_path: None,
                mode: ConversionMode::Optimize,
                status: ConversionStatus::Failed,
                progress: None,
                processed: None,
                total: None,
                speed: None,
                source_size: None,
                new_size: None,
                error: Some("conversion state lock poisoned".into()),
                error_category: Some(ConversionErrorCategory::Internal),
                diagnostic_log_path: None,
                applied_to_cue: false,
            })
    }
}

/// Process/job ownership for all conversion workers in the application.
pub struct MediaConverterManager {
    jobs: Arc<Mutex<HashMap<String, Arc<JobRecord>>>>,
    queue: Arc<QueueState>,
    shutting_down: Arc<AtomicBool>,
}

struct QueueState {
    pending: Mutex<VecDeque<PendingConversion>>,
    active: AtomicBool,
}

struct PendingConversion {
    record: Arc<JobRecord>,
    ffmpeg: PathBuf,
    args: Vec<String>,
    output: PathBuf,
    duration: Option<f64>,
}

impl Default for MediaConverterManager {
    fn default() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            queue: Arc::new(QueueState {
                pending: Mutex::new(VecDeque::new()),
                active: AtomicBool::new(false),
            }),
            shutting_down: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Drop for MediaConverterManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl MediaConverterManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list(&self) -> Vec<MediaConversionJob> {
        let mut jobs: Vec<MediaConversionJob> = self
            .jobs
            .lock()
            .map(|map| map.values().map(|job| job.snapshot()).collect())
            .unwrap_or_default();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        jobs
    }

    pub fn completed_output(&self, id: &str) -> Result<(String, PathBuf), String> {
        let (cue_id, _input, output) = self.completed_paths(id)?;
        Ok((cue_id, output))
    }

    pub fn completed_paths(&self, id: &str) -> Result<(String, PathBuf, PathBuf), String> {
        let record = self
            .jobs
            .lock()
            .map_err(|e| e.to_string())?
            .get(id)
            .cloned()
            .ok_or("Conversion job not found")?;
        let job = record.snapshot();
        if job.status != ConversionStatus::Completed {
            return Err("Conversion job has not completed".into());
        }
        Ok((
            job.cue_id.ok_or("Conversion job has no cue id")?,
            PathBuf::from(job.input_path),
            PathBuf::from(job.output_path.ok_or("Conversion job has no output path")?),
        ))
    }

    pub fn mark_applied(&self, id: &str, applied: bool) -> Result<(), String> {
        let record = self
            .jobs
            .lock()
            .map_err(|e| e.to_string())?
            .get(id)
            .cloned()
            .ok_or("Conversion job not found")?;
        record
            .state
            .lock()
            .map_err(|e| e.to_string())?
            .applied_to_cue = applied;
        emit_job(&record.app, &record);
        Ok(())
    }

    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let record = self
            .jobs
            .lock()
            .map_err(|e| e.to_string())?
            .get(id)
            .cloned()
            .ok_or_else(|| "Conversion job not found".to_string())?;
        let queued = record
            .state
            .lock()
            .map(|state| state.status == ConversionStatus::Queued)
            .unwrap_or(false);
        record.cancel.store(true, Ordering::Release);
        if queued {
            if let Ok(mut state) = record.state.lock() {
                state.status = ConversionStatus::Cancelled;
                state.progress = None;
                state.error_category = Some(ConversionErrorCategory::Cancelled);
            }
            let _ = fs::remove_file(&record.temp_path);
            let _ = fs::remove_file(&record.reservation_path);
            emit_job(&record.app, &record);
            start_next_queued(self.queue.clone(), self.shutting_down.clone());
            return Ok(());
        }
        if let Ok(mut child) = record.child.lock() {
            if let Some(child) = child.as_mut() {
                let _ = child.kill();
            }
        }
        Ok(())
    }

    pub fn cancel_all(&self) {
        let jobs = self
            .jobs
            .lock()
            .map(|map| map.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for record in jobs {
            let queued = record
                .state
                .lock()
                .map(|state| state.status == ConversionStatus::Queued)
                .unwrap_or(false);
            record.cancel.store(true, Ordering::Release);
            if queued {
                if let Ok(mut state) = record.state.lock() {
                    state.status = ConversionStatus::Cancelled;
                    state.progress = None;
                    state.error_category = Some(ConversionErrorCategory::Cancelled);
                }
                let _ = fs::remove_file(&record.temp_path);
                let _ = fs::remove_file(&record.reservation_path);
                emit_job(&record.app, &record);
                continue;
            }
            if let Ok(mut child) = record.child.lock() {
                if let Some(child) = child.as_mut() {
                    let _ = child.kill();
                }
            }
        }
    }

    /// Cancel and kill every process. Workers remove their temporary files as
    /// soon as they observe the cancellation and reap their child.
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        let jobs = self
            .jobs
            .lock()
            .map(|map| map.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for record in jobs {
            let queued = record
                .state
                .lock()
                .map(|state| state.status == ConversionStatus::Queued)
                .unwrap_or(false);
            record.cancel.store(true, Ordering::Release);
            if queued {
                if let Ok(mut state) = record.state.lock() {
                    state.status = ConversionStatus::Cancelled;
                    state.progress = None;
                    state.error_category = Some(ConversionErrorCategory::Cancelled);
                }
                let _ = fs::remove_file(&record.temp_path);
                let _ = fs::remove_file(&record.reservation_path);
                emit_job(&record.app, &record);
                continue;
            }
            if let Ok(mut child) = record.child.lock() {
                if let Some(child) = child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            let _ = fs::remove_file(&record.temp_path);
        }
        if let Ok(mut pending) = self.queue.pending.lock() {
            pending.clear();
        }
    }

    pub(crate) fn start(
        &self,
        request: MediaConversionRequest,
        app: tauri::AppHandle,
        media_kind: MediaKind,
    ) -> Result<MediaConversionJob, String> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("Media conversion is shutting down".into());
        }
        validate_request(&request)?;
        let input = PathBuf::from(&request.input_path);
        if !input.is_file() {
            return Err(format!(
                "Input media file does not exist: {}",
                input.display()
            ));
        }
        let ffmpeg = find_ffmpeg_runtime()
            .ok_or_else(|| "Bundled FFmpeg runtime was not found".to_string())?;
        if !is_media_extension(&input) {
            return Err("Input path does not have a supported media extension".into());
        }
        let output_extension = match media_kind {
            MediaKind::Audio => "m4a",
            MediaKind::Video => "mp4",
            MediaKind::Image => match request
                .image_format
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("jpg") | Some("jpeg") => "jpg",
                _ => "png",
            },
        };
        let output = unique_output_path(&input, output_extension)?;
        let reservation_path = output.with_extension("lock");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&reservation_path)
            .map_err(|_| "Another conversion already reserved this output name".to_string())?;
        let temp_extension = output
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("mp4");
        let temp = output.with_file_name(format!(
            ".{}.part-{}.{}",
            output
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("qlisa"),
            Uuid::new_v4(),
            temp_extension,
        ));
        let diagnostic_path = output.with_file_name(format!(
            ".{}.ffmpeg-{}.log",
            output
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("qlisa"),
            Uuid::new_v4()
        ));
        let probe = probe_media_path(&input).ok();
        let args = match media_kind {
            // Audio output is deliberately encoded as M4A/AAC.  Remuxing an
            // MP3 into the old MP4 output path can fail, and mapping the
            // attached cover-art stream would fabricate a video track.
            MediaKind::Audio => audio_preset_args(&request, &input, &temp),
            MediaKind::Video => {
                if probe.as_ref().is_some_and(|metadata| {
                    analyze_compatibility(metadata).status == CompatibilityStatus::Compatible
                }) {
                    remux_args(&input, &temp)
                } else {
                    preset_args(&request, &input, &temp)
                }
            }
            MediaKind::Image => image_preset_args(&request, &input, &temp, output_extension),
        };
        let id = Uuid::new_v4().to_string();
        let record = Arc::new(JobRecord {
            state: Mutex::new(MediaConversionJob {
                id: id.clone(),
                cue_id: request.cue_id.clone(),
                input_path: request.input_path.clone(),
                output_path: Some(output.to_string_lossy().into_owned()),
                mode: request.mode.clone(),
                status: ConversionStatus::Queued,
                progress: Some(0.0),
                processed: Some(0.0),
                total: probe
                    .as_ref()
                    .and_then(|metadata| metadata.duration_seconds),
                speed: None,
                source_size: probe.as_ref().map(|metadata| metadata.file_size),
                new_size: None,
                error: None,
                error_category: None,
                diagnostic_log_path: Some(diagnostic_path.to_string_lossy().into_owned()),
                applied_to_cue: false,
            }),
            cancel: AtomicBool::new(false),
            child: Mutex::new(None),
            temp_path: temp.clone(),
            reservation_path: reservation_path.clone(),
            diagnostic_path: diagnostic_path.clone(),
            app: app.clone(),
        });
        self.jobs
            .lock()
            .map_err(|e| e.to_string())?
            .insert(id.clone(), record.clone());
        let duration = probe
            .as_ref()
            .and_then(|metadata| metadata.duration_seconds);
        emit_job(&app, &record);
        self.queue
            .pending
            .lock()
            .map_err(|e| e.to_string())?
            .push_back(PendingConversion {
                record: record.clone(),
                ffmpeg,
                args,
                output,
                duration,
            });
        start_next_queued(self.queue.clone(), self.shutting_down.clone());
        Ok(self
            .jobs
            .lock()
            .map_err(|e| e.to_string())?
            .get(&id)
            .map(|j| j.snapshot())
            .unwrap_or_else(|| MediaConversionJob {
                id,
                cue_id: request.cue_id,
                input_path: request.input_path,
                output_path: None,
                mode: request.mode,
                status: ConversionStatus::Failed,
                progress: None,
                processed: None,
                total: None,
                speed: None,
                source_size: None,
                new_size: None,
                error: Some("job disappeared".into()),
                error_category: Some(ConversionErrorCategory::Internal),
                diagnostic_log_path: None,
                applied_to_cue: false,
            }))
    }
}

fn validate_request(request: &MediaConversionRequest) -> Result<(), String> {
    if request.input_path.trim().is_empty() {
        return Err("Input path is required".into());
    }
    if let Some(q) = request.quality {
        if q > 51 {
            return Err("Quality must be between 0 and 51".into());
        }
        if request.mode == ConversionMode::Size && !(1..=3).contains(&q) {
            return Err("Size quality must be 1 (best), 2 (balanced), or 3 (smallest)".into());
        }
    }
    if let Some(max) = request.max_resolution {
        if !(64..=16_384).contains(&max) {
            return Err("Maximum resolution must be between 64 and 16384".into());
        }
    }
    if let Some(format) = request.image_format.as_deref() {
        if !matches!(format.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg") {
            return Err("Image format must be png or jpeg".into());
        }
    }
    if let Some(id) = request.cue_id.as_deref() {
        Uuid::parse_str(id).map_err(|_| "Cue id is not a valid UUID".to_string())?;
    }
    Ok(())
}

fn is_media_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some(
            "mp4"
                | "mov"
                | "m4v"
                | "mkv"
                | "webm"
                | "avi"
                | "mxf"
                | "mts"
                | "m2ts"
                | "wav"
                | "mp3"
                | "aac"
                | "flac"
                | "ogg"
                | "aiff"
                | "aif"
                | "m4a"
                | "png"
                | "jpg"
                | "jpeg"
                | "webp"
                | "bmp"
                | "gif"
                | "tif"
                | "tiff"
        )
    )
}

/// Return arguments only. Keeping this separate makes the no-shell contract
/// easy to verify and prevents user text from becoming an FFmpeg option.
pub fn preset_args(request: &MediaConversionRequest, input: &Path, temp: &Path) -> Vec<String> {
    let quality = match request.mode {
        ConversionMode::Optimize => 18,
        ConversionMode::Size => match request.quality.unwrap_or(2) {
            1 => 18,
            3 => 28,
            _ => 23,
        },
    };
    let mut args = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-map".into(),
        "0:v:0?".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "medium".into(),
        "-crf".into(),
        quality.to_string().into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        "-movflags".into(),
        "+faststart".into(),
    ];
    if let Some(max) = request.max_resolution {
        args.extend(["-vf".into(), format!("scale=trunc(min(iw\\,{max})/2)*2:trunc(min(ih\\,{max})/2)*2:force_original_aspect_ratio=decrease")]);
    }
    args.extend([
        "-ar".into(),
        "48000".into(),
        "-ac".into(),
        "2".into(),
        "-f".into(),
        "mp4".into(),
        temp.to_string_lossy().into_owned(),
    ]);
    args
}

/// Encode an audio Cue without inventing a video stream.  The old generic
/// preset always mapped `0:v:0?` and wrote MP4 output, which is wrong for MP3
/// files carrying attached PNG/JPEG artwork and could make FFmpeg reject the
/// job.  M4A/AAC is a predictable output understood by the audio engine.
pub fn audio_preset_args(
    request: &MediaConversionRequest,
    input: &Path,
    temp: &Path,
) -> Vec<String> {
    let bitrate = match request.mode {
        ConversionMode::Optimize => "192k",
        ConversionMode::Size => match request.quality.unwrap_or(2) {
            1 => "192k",
            3 => "96k",
            _ => "128k",
        },
    };
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-map".into(),
        "0:a:0".into(),
        "-vn".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        bitrate.into(),
        "-ar".into(),
        "48000".into(),
        "-ac".into(),
        "2".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-f".into(),
        "ipod".into(),
        temp.to_string_lossy().into_owned(),
    ]
}

/// Encode a still Image Cue as one PNG or JPEG frame. The output is not
/// treated as a video: no synthetic duration or frame rate is introduced.
pub fn image_preset_args(
    request: &MediaConversionRequest,
    input: &Path,
    temp: &Path,
    output_extension: &str,
) -> Vec<String> {
    let codec = if output_extension == "jpg" {
        "mjpeg"
    } else {
        "png"
    };
    let mut args = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-frames:v".into(),
        "1".into(),
        "-an".into(),
        "-c:v".into(),
        codec.into(),
    ];
    if output_extension == "jpg" {
        let quality = match request.mode {
            ConversionMode::Optimize => 2,
            ConversionMode::Size => match request.quality.unwrap_or(2) {
                1 => 2,
                3 => 8,
                _ => 5,
            },
        };
        args.extend(["-q:v".into(), quality.to_string()]);
    }
    if let Some(max) = request.max_resolution {
        args.extend([
            "-vf".into(),
            format!("scale=min(iw\\,{max}):min(ih\\,{max}):force_original_aspect_ratio=decrease"),
        ]);
    }
    args.extend([
        "-f".into(),
        "image2".into(),
        temp.to_string_lossy().into_owned(),
    ]);
    args
}

pub fn remux_args(input: &Path, temp: &Path) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-map".into(),
        "0:v:0?".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-c".into(),
        "copy".into(),
        "-movflags".into(),
        "+faststart".into(),
        temp.to_string_lossy().into_owned(),
    ]
}

fn unique_output_path(input: &Path, extension: &str) -> Result<PathBuf, String> {
    let parent = input.parent().ok_or("Input has no parent directory")?;
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("media");
    for n in 0..10_000_u32 {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!("_{n}")
        };
        let candidate = parent.join(format!("{stem}_converted{suffix}.{extension}"));
        if candidate != input && !candidate.exists() && !candidate.with_extension("lock").exists() {
            return Ok(candidate);
        }
    }
    Err("Could not find a unique converted output name".into())
}

fn read_lines<T: Read + Send + 'static>(
    reader: T,
    tx: mpsc::Sender<String>,
    diagnostic_path: Option<PathBuf>,
) {
    thread::spawn(move || {
        let reader = io::BufReader::new(reader);
        let mut log = diagnostic_path.and_then(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        });
        for line in reader.lines().map_while(Result::ok) {
            if let Some(file) = log.as_mut() {
                use std::io::Write;
                let _ = writeln!(file, "{line}");
            }
            let _ = tx.send(line);
        }
    });
}

/// Start the next queued conversion, if no other conversion is active.  The
/// compare/exchange is important: `start()` and a finishing worker can call
/// this concurrently, but only one of them may own the FFmpeg slot.
fn start_next_queued(queue: Arc<QueueState>, shutting_down: Arc<AtomicBool>) {
    if queue
        .active
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    loop {
        let Some(pending) = queue.pending.lock().ok().and_then(|mut q| q.pop_front()) else {
            queue.active.store(false, Ordering::Release);
            return;
        };
        let record = pending.record;
        if shutting_down.load(Ordering::Acquire) || record.cancel.load(Ordering::Acquire) {
            if let Ok(mut state) = record.state.lock() {
                state.status = ConversionStatus::Cancelled;
                state.progress = None;
                state.error_category = Some(ConversionErrorCategory::Cancelled);
            }
            let _ = fs::remove_file(&record.temp_path);
            let _ = fs::remove_file(&record.reservation_path);
            emit_job(&record.app, &record);
            continue;
        }
        let mut command = Command::new(&pending.ffmpeg);
        command
            .args(&pending.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = match spawn_managed_ffmpeg(&mut command) {
            Ok(child) => child,
            Err(error) => {
                if let Ok(mut state) = record.state.lock() {
                    state.status = ConversionStatus::Failed;
                    state.progress = None;
                    state.error = Some("Не удалось запустить FFmpeg.".into());
                    state.error_category = Some(ConversionErrorCategory::Process);
                }
                append_diagnostic_line(
                    &record.diagnostic_path,
                    &format!("FFmpeg start error: {error}"),
                );
                let _ = fs::remove_file(&record.temp_path);
                let _ = fs::remove_file(&record.reservation_path);
                emit_job(&record.app, &record);
                continue;
            }
        };
        if let Ok(mut slot) = record.child.lock() {
            *slot = Some(child);
        }
        // A queued cancellation may race with the short spawn/assignment
        // window.  Do not resurrect that job as Running after it was already
        // reported Cancelled.
        if shutting_down.load(Ordering::Acquire) || record.cancel.load(Ordering::Acquire) {
            if let Ok(mut child) = record.child.lock() {
                if let Some(child) = child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let _ = child.take();
            }
            if let Ok(mut state) = record.state.lock() {
                state.status = ConversionStatus::Cancelled;
                state.progress = None;
                state.error_category = Some(ConversionErrorCategory::Cancelled);
            }
            let _ = fs::remove_file(&record.temp_path);
            let _ = fs::remove_file(&record.reservation_path);
            emit_job(&record.app, &record);
            continue;
        }
        if let Ok(mut state) = record.state.lock() {
            state.status = ConversionStatus::Running;
            state.progress = Some(0.0);
        }
        emit_job(&record.app, &record);
        log::info!(
            "[media-conversion] started id={} input={} output={}",
            record.snapshot().id,
            record.snapshot().input_path,
            pending.output.display()
        );
        let worker_record = record.clone();
        let worker_queue = queue.clone();
        let worker_shutdown = shutting_down.clone();
        let output = pending.output;
        let duration = pending.duration;
        if let Err(error) = thread::Builder::new()
            .name("qlisa-media-converter".into())
            .spawn(move || {
                conversion_worker(
                    worker_record,
                    output,
                    duration,
                    worker_queue,
                    worker_shutdown,
                )
            })
        {
            record.cancel.store(true, Ordering::Release);
            if let Ok(mut child) = record.child.lock() {
                if let Some(child) = child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            if let Ok(mut state) = record.state.lock() {
                state.status = ConversionStatus::Failed;
                state.progress = None;
                state.error = Some("Не удалось создать рабочий поток конвертации.".into());
                state.error_category = Some(ConversionErrorCategory::Internal);
            }
            append_diagnostic_line(
                &record.diagnostic_path,
                &format!("Worker start error: {error}"),
            );
            let _ = fs::remove_file(&record.temp_path);
            let _ = fs::remove_file(&record.reservation_path);
            emit_job(&record.app, &record);
            continue;
        }
        return;
    }
}

fn conversion_worker(
    record: Arc<JobRecord>,
    output: PathBuf,
    duration: Option<f64>,
    queue: Arc<QueueState>,
    shutting_down: Arc<AtomicBool>,
) {
    let app = record.app.clone();
    let (tx, rx) = mpsc::channel();
    if let Ok(mut child) = record.child.lock() {
        if let Some(child) = child.as_mut() {
            if let Some(stdout) = child.take_stdout() {
                read_lines(stdout, tx.clone(), None);
            }
            if let Some(stderr) = child.take_stderr() {
                read_lines(stderr, tx.clone(), Some(record.diagnostic_path.clone()));
            }
        }
    }
    let mut saw_error = false;
    let mut exit_status = None;
    loop {
        while let Ok(line) = rx.try_recv() {
            if let Some(value) = line
                .strip_prefix("out_time_ms=")
                .and_then(|v| v.parse::<f64>().ok())
            {
                if let Ok(mut state) = record.state.lock() {
                    state.processed = Some(value / 1_000_000.0);
                    if let Some(seconds) = duration.filter(|d| *d > 0.0) {
                        state.progress =
                            Some((value / 1_000_000.0 / seconds).clamp(0.0, 0.999) as f32);
                    }
                }
                emit_job(&app, &record);
            } else if let Some(value) = line
                .strip_prefix("out_time_us=")
                .and_then(|v| v.parse::<f64>().ok())
            {
                if let Ok(mut state) = record.state.lock() {
                    state.processed = Some(value / 1_000_000.0);
                }
                emit_job(&app, &record);
            } else if let Some(value) = line
                .strip_prefix("speed=")
                .and_then(|v| v.strip_suffix('x'))
                .and_then(|v| v.parse::<f64>().ok())
            {
                if let Ok(mut state) = record.state.lock() {
                    state.speed = Some(value);
                }
                emit_job(&app, &record);
            } else if !line.starts_with("progress=")
                && !line.starts_with("frame=")
                && !line.starts_with("out_")
                && !line.trim().is_empty()
            {
                saw_error = true;
            }
        }
        let exited = record
            .child
            .lock()
            .ok()
            .and_then(|mut child| child.as_mut().and_then(|c| c.try_wait().ok().flatten()));
        if exited.is_some() {
            exit_status = exited;
        }
        if exited.is_some() {
            break;
        }
        if record.cancel.load(Ordering::Acquire) {
            if let Ok(mut child) = record.child.lock() {
                if let Some(child) = child.as_mut() {
                    let _ = child.kill();
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    // Release the managed process handle as soon as the worker observes exit.
    if let Ok(mut child) = record.child.lock() {
        let _ = child.take();
    }
    let cancelled = record.cancel.load(Ordering::Acquire);
    let mut validation_error = None;
    let mut finalization_error = None;
    let finalized_by_this_job = if !cancelled
        && exit_status
            .as_ref()
            .is_some_and(std::process::ExitStatus::success)
    {
        match output_from_temp(&record.temp_path, &output) {
            Ok(()) => true,
            Err(error) => {
                finalization_error = Some(error);
                false
            }
        }
    } else {
        false
    };
    let success = finalized_by_this_job
        && match probe_media_path(&output) {
            Ok(metadata)
                if analyze_compatibility(&metadata).status == CompatibilityStatus::Compatible =>
            {
                true
            }
            Ok(metadata) => {
                validation_error = Some(format!(
                    "incompatible: {}",
                    analyze_compatibility(&metadata).reasons.join("; ")
                ));
                false
            }
            Err(error) => {
                // Keep the probe detail in the diagnostic log, but never expose
                // ffprobe's raw stderr or parser internals in the user-facing job.
                append_diagnostic_line(
                    &record.diagnostic_path,
                    &format!("ffprobe validation error: {error}"),
                );
                validation_error = Some(format!("validation: {error}"));
                false
            }
        };
    let cancelled = cancelled || record.cancel.load(Ordering::Acquire);
    if finalized_by_this_job && (!success || cancelled) {
        let _ = fs::remove_file(&output);
    }
    let _ = fs::remove_file(&record.temp_path);
    let _ = fs::remove_file(&record.reservation_path);
    if fs::metadata(&record.diagnostic_path)
        .map(|m| m.len() == 0)
        .unwrap_or(false)
    {
        let _ = fs::remove_file(&record.diagnostic_path);
    }
    if let Ok(mut state) = record.state.lock() {
        if cancelled {
            state.status = ConversionStatus::Cancelled;
            state.progress = None;
            state.error_category = Some(ConversionErrorCategory::Cancelled);
            log::warn!(
                "[media-conversion] cancelled id={} input={} output={}",
                state.id,
                state.input_path,
                output.display()
            );
        } else if success {
            state.status = ConversionStatus::Completed;
            state.progress = Some(1.0);
            state.new_size = fs::metadata(&output).ok().map(|m| m.len());
            log::info!(
                "[media-conversion] completed id={} input={} output={}",
                state.id,
                state.input_path,
                output.display()
            );
        } else {
            state.status = ConversionStatus::Failed;
            state.progress = None;
            let (message, category) = if finalization_error.is_some() {
                (
                    "Не удалось сохранить результат конвертации. Проверьте свободное место и права доступа.",
                    ConversionErrorCategory::Output,
                )
            } else if validation_error
                .as_deref()
                .is_some_and(|error| error.starts_with("incompatible:"))
            {
                (
                    "Результат конвертации несовместим с Qlisa.",
                    ConversionErrorCategory::Output,
                )
            } else if validation_error.is_some() {
                (
                    "Не удалось проверить результат конвертации.",
                    ConversionErrorCategory::Output,
                )
            } else if saw_error {
                (
                    "FFmpeg не смог обработать файл. Проверьте исходный файл или его формат.",
                    ConversionErrorCategory::Process,
                )
            } else {
                (
                    "Конвертация завершилась с ошибкой.",
                    ConversionErrorCategory::Process,
                )
            };
            state.error = Some(message.into());
            state.error_category = Some(category.clone());
            log::error!(
                "[media-conversion] failed id={} input={} output={} category={:?}",
                state.id,
                state.input_path,
                output.display(),
                category
            );
        }
    }
    emit_job(&app, &record);
    // Release the single FFmpeg slot only after this job has fully reaped its
    // child and finalized its output.  Then immediately promote the next job.
    queue.active.store(false, Ordering::Release);
    start_next_queued(queue, shutting_down);
}

fn output_from_temp(temp: &Path, output: &Path) -> Result<(), String> {
    if !temp.is_file() {
        return Err("FFmpeg did not produce an output file".into());
    }
    fs::rename(temp, output).map_err(|e| format!("Could not finalize output: {e}"))
}

fn append_diagnostic_line(path: &Path, line: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        use std::io::Write;
        let _ = writeln!(file, "{line}");
    }
}

fn emit_job(app: &tauri::AppHandle, record: &JobRecord) {
    let _ = app.emit(MEDIA_CONVERSION_EVENT, record.snapshot());
}

fn ffprobe_path() -> Option<PathBuf> {
    let name = if cfg!(target_os = "windows") {
        "ffprobe.exe"
    } else {
        "ffprobe"
    };
    let mut candidates = Vec::new();
    if let Some(ffmpeg) = find_ffmpeg_runtime() {
        if let Some(parent) = ffmpeg.parent() {
            candidates.push(parent.join(name));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("resources").join("ffmpeg").join(name));
            candidates.push(dir.join(name));
        }
    }
    if let Ok(override_path) = std::env::var("QLISA_FFMPEG_PATH") {
        if let Some(parent) = Path::new(&override_path).parent() {
            candidates.push(parent.join(name));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn run_probe(path: &Path) -> Result<Vec<u8>, String> {
    let probe =
        ffprobe_path().ok_or_else(|| "Bundled ffprobe runtime was not found".to_string())?;
    let mut command = Command::new(probe);
    command
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "--",
            &path.to_string_lossy(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child =
        spawn_managed_ffmpeg(&mut command).map_err(|e| format!("Could not start ffprobe: {e}"))?;
    let (tx, rx) = mpsc::channel();
    if let Some(mut stream) = child.take_stdout() {
        let tx2 = tx.clone();
        thread::spawn(move || {
            let mut data = Vec::new();
            let _ = stream.read_to_end(&mut data);
            let _ = tx2.send((true, data));
        });
    }
    if let Some(mut stream) = child.take_stderr() {
        let tx2 = tx.clone();
        thread::spawn(move || {
            let mut data = Vec::new();
            let _ = stream.read_to_end(&mut data);
            let _ = tx2.send((false, data));
        });
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("ffprobe timed out".into());
        }
        thread::sleep(Duration::from_millis(25));
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    for _ in 0..2 {
        if let Ok((is_stdout, data)) = rx.recv_timeout(Duration::from_secs(1)) {
            if is_stdout {
                stdout = data;
            } else {
                stderr = data;
            }
        }
    }
    if !status.success() {
        return Err(String::from_utf8_lossy(&stderr)
            .trim()
            .to_string()
            .if_empty_then(|| "ffprobe failed".into()));
    }
    Ok(stdout)
}

trait EmptyFallback {
    fn if_empty_then(self, fallback: impl FnOnce() -> String) -> String;
}
impl EmptyFallback for String {
    fn if_empty_then(self, fallback: impl FnOnce() -> String) -> String {
        if self.trim().is_empty() {
            fallback()
        } else {
            self
        }
    }
}

pub(crate) fn probe_media_path(path: &Path) -> Result<ProbeMetadata, String> {
    if !path.is_file() {
        return Err(format!(
            "Input media file does not exist: {}",
            path.display()
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&run_probe(path)?)
        .map_err(|e| format!("Invalid ffprobe output: {e}"))?;
    let streams = value
        .get("streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    // MP3/M4A files may contain an attached cover image encoded as a video
    // stream.  It is metadata, not a playable video stream.  Selecting it as
    // the first video is what produced PNG/MJPEG + 90000 fps in the dialog and
    // caused the converter to fail on otherwise ordinary audio files.
    let audio_container = value
        .get("format")
        .and_then(|format| format.get("format_name"))
        .and_then(|format| format.as_str())
        .map(|format| {
            matches!(
                format.split(',').next().unwrap_or(""),
                "mp3" | "mpeg" | "adts" | "aac" | "flac" | "ogg" | "wav" | "aiff"
            )
        })
        .unwrap_or(false);
    let video = streams
        .iter()
        .find(|stream| is_primary_video_stream(stream, audio_container));
    let audio = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(|v| v.as_str()) == Some("audio"));
    let fps = video
        .and_then(|v| v.get("r_frame_rate").and_then(|v| v.as_str()))
        .and_then(parse_frame_rate);
    Ok(ProbeMetadata {
        path: path.to_string_lossy().into_owned(),
        file_size: fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        format_name: value
            .get("format")
            .and_then(|f| f.get("format_name"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        duration_seconds: value
            .get("format")
            .and_then(|f| f.get("duration"))
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse().ok()),
        width: video
            .and_then(|v| v.get("width"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        height: video
            .and_then(|v| v.get("height"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        video_codec: video
            .and_then(|v| v.get("codec_name"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        video_profile: video
            .and_then(|v| v.get("profile"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        video_bit_depth: video
            .and_then(|v| v.get("bits_per_raw_sample"))
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse().ok())
            .or_else(|| {
                video
                    .and_then(|v| v.get("pix_fmt"))
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.contains("10").then_some(10))
            }),
        audio_codec: audio
            .and_then(|v| v.get("codec_name"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        audio_channels: audio
            .and_then(|v| v.get("channels"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        sample_rate: audio
            .and_then(|v| v.get("sample_rate"))
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse().ok()),
        pixel_format: video
            .and_then(|v| v.get("pix_fmt"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        frame_rate: fps,
    })
}

fn is_attached_picture(stream: &serde_json::Value) -> bool {
    stream
        .get("disposition")
        .and_then(|value| value.get("attached_pic"))
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|value| value != 0)
        || stream
            .get("disposition")
            .and_then(|value| value.get("attached_pic"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
}

fn is_known_cover_codec(stream: &serde_json::Value) -> bool {
    matches!(
        stream.get("codec_name").and_then(|value| value.as_str()),
        Some("png" | "mjpeg" | "jpeg")
    )
}

fn is_primary_video_stream(stream: &serde_json::Value, audio_container: bool) -> bool {
    stream.get("codec_type").and_then(|value| value.as_str()) == Some("video")
        && !is_attached_picture(stream)
        && !(audio_container && is_known_cover_codec(stream))
}

fn parse_frame_rate(raw: &str) -> Option<f64> {
    let (numerator, denominator) = raw
        .split_once('/')
        .map(|(n, d)| (n, d))
        .unwrap_or((raw, "1"));
    let numerator = numerator.parse::<f64>().ok()?;
    let denominator = denominator.parse::<f64>().ok()?;
    if !numerator.is_finite() || !denominator.is_finite() || denominator == 0.0 {
        return None;
    }
    let fps = numerator / denominator;
    fps.is_finite().then_some(fps)
}

pub fn analyze_compatibility(metadata: &ProbeMetadata) -> MediaCompatibility {
    let mut reasons = Vec::new();
    let mut unsupported = false;
    let image_only = metadata.audio_codec.is_none()
        && metadata.video_codec.is_some()
        && metadata
            .format_name
            .as_deref()
            .map(|format| {
                matches!(
                    format.split(',').next().unwrap_or(""),
                    "png"
                        | "png_pipe"
                        | "jpeg_pipe"
                        | "image2"
                        | "webp"
                        | "bmp_pipe"
                        | "tiff"
                        | "tiff_pipe"
                )
            })
            .unwrap_or(false);
    if image_only {
        return MediaCompatibility {
            status: CompatibilityStatus::Compatible,
            compatible: true,
            reasons,
        };
    }
    let audio_only = metadata.video_codec.is_none() && metadata.audio_codec.is_some();
    if metadata.video_codec.is_none() && metadata.audio_codec.is_none() {
        return MediaCompatibility {
            status: CompatibilityStatus::Unsupported,
            compatible: false,
            reasons: vec!["No audio or video stream".into()],
        };
    }
    // Audio cues are decoded by the normal audio engine.  Do not apply the
    // stage-video preset's MP4/AAC/48 kHz/stereo rules to them: an ordinary
    // MP3, FLAC, WAV or OGG is already a valid playback source and should not
    // get a conversion recommendation merely because it is not an MP4.
    if audio_only {
        let codec = metadata
            .audio_codec
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        // Keep this list aligned with the Symphonia features in Cargo.toml.
        // Container names are deliberately not used: an OGG with an Opus
        // stream is not covered by the enabled `ogg`/`vorbis` decoder.
        if !is_supported_audio_codec(&codec) {
            unsupported = true;
            reasons.push(format!(
                "Audio codec {codec} may not be supported by the playback engine"
            ));
        }
        return MediaCompatibility {
            status: if unsupported {
                CompatibilityStatus::Unsupported
            } else {
                CompatibilityStatus::Compatible
            },
            compatible: !unsupported,
            reasons,
        };
    }

    if matches!(metadata.video_codec.as_deref(), Some("av1" | "vp9")) {
        reasons.push("AV1/VP9 video may need conversion".into());
    }
    if metadata.video_codec.as_deref() == Some("hevc") {
        if metadata.video_bit_depth.unwrap_or(8) > 8 {
            reasons.push("10-bit HEVC may be unstable".into());
        } else {
            reasons.push("HEVC video may be resource-intensive".into());
        }
    }
    if metadata
        .audio_codec
        .as_deref()
        .is_some_and(|codec| !is_supported_audio_codec(codec))
    {
        unsupported = true;
        reasons.push(format!(
            "Audio codec {codec} may not be supported by the playback engine",
            codec = metadata.audio_codec.as_deref().unwrap_or("unknown")
        ));
    }
    if metadata
        .pixel_format
        .as_deref()
        .is_some_and(|format| format != "yuv420p")
    {
        reasons.push("Video pixel format is not yuv420p".into());
    }
    if metadata.width.is_some_and(|v| v > 3840) || metadata.height.is_some_and(|v| v > 2160) {
        reasons.push("Resolution exceeds 4K UHD".into());
    }
    if metadata.frame_rate.is_some_and(|v| v > 60.0) {
        reasons.push("Frame rate exceeds 60 fps".into());
    }
    let status = if unsupported {
        CompatibilityStatus::Unsupported
    } else if reasons.is_empty() {
        CompatibilityStatus::Compatible
    } else {
        CompatibilityStatus::Recommended
    };
    MediaCompatibility {
        compatible: status == CompatibilityStatus::Compatible,
        status,
        reasons,
    }
}

fn is_supported_audio_codec(codec: &str) -> bool {
    matches!(
        codec,
        "mp3"
            | "aac"
            | "flac"
            | "vorbis"
            | "pcm_s16le"
            | "pcm_s24le"
            | "pcm_s32le"
            | "pcm_s16be"
            | "pcm_s24be"
            | "pcm_s32be"
            | "pcm_f32le"
            | "pcm_f64le"
            | "pcm_f32be"
            | "pcm_f64be"
    )
}

/// Compact cache representation used by the cue-list metadata worker.
/// Keeping codes instead of English text lets the frontend localize the row
/// warning without storing user-facing strings in the runtime cache.
pub(crate) fn compatibility_cache_codes(metadata: &ProbeMetadata) -> (u8, Option<u8>) {
    let result = analyze_compatibility(metadata);
    let status = match result.status {
        CompatibilityStatus::Compatible => 0,
        CompatibilityStatus::Recommended => 1,
        CompatibilityStatus::Unsupported => 2,
    };
    let reason = result.reasons.first().map(|reason| {
        if reason.starts_with("Audio codec ") {
            1
        } else if reason == "Container is not MP4/MOV" {
            2
        } else if reason.contains("AV1/VP9") || reason.contains("Video codec") {
            3
        } else if reason.contains("10-bit HEVC") {
            4
        } else if reason.contains("HEVC video") {
            13
        } else if reason.contains("pixel format") {
            5
        } else if reason.contains("Resolution") {
            6
        } else if reason.contains("Frame rate") {
            7
        } else if reason.contains("Audio codec is not AAC") {
            8
        } else if reason.contains("sample rate") {
            9
        } else if reason.contains("stereo") {
            10
        } else if reason.contains("dimensions") {
            11
        } else {
            12
        }
    });
    (status, reason)
}

#[tauri::command]
pub fn probe_cue_media(
    cue_id: String,
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<ProbeMetadata, String> {
    let path = cue_media_path(&cue_id, &state)?;
    probe_media_path(&path)
}

#[tauri::command]
pub fn analyze_media_compatibility(metadata: ProbeMetadata) -> MediaCompatibility {
    analyze_compatibility(&metadata)
}

#[tauri::command]
pub fn start_media_conversion(
    request: MediaConversionRequest,
    state: tauri::State<'_, crate::state::AppState>,
    app_handle: tauri::AppHandle,
) -> Result<MediaConversionJob, String> {
    let cue_id = request
        .cue_id
        .as_deref()
        .ok_or("cue_id is required for media conversion")?;
    let media_kind = validate_cue_input(cue_id, Path::new(&request.input_path), &state)?;
    state.media_converter.start(request, app_handle, media_kind)
}

#[tauri::command]
pub fn list_media_conversions(
    state: tauri::State<'_, crate::state::AppState>,
) -> Vec<MediaConversionJob> {
    state.media_converter.list()
}

#[tauri::command]
pub fn cancel_media_conversion(
    job_id: String,
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    state.media_converter.cancel(&job_id)
}

/// Replace an Audio, Video, or Image cue's authored file path. The caller can
/// pass the completed job's output path after reviewing its status.
#[tauri::command]
pub fn replace_cue_media_path(
    job_id: String,
    state: tauri::State<'_, crate::state::AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let (cue_id, output) = state.media_converter.completed_output(&job_id)?;
    let file_path = output.to_string_lossy().into_owned();
    if !output.is_file() {
        return Err("Completed conversion output does not exist".into());
    }
    let id = Uuid::parse_str(&cue_id).map_err(|_| "Cue id is not a valid UUID".to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
    if !matches!(
        cue.cue_type(),
        crate::cue::types::CueType::Audio
            | crate::cue::types::CueType::Video
            | crate::cue::types::CueType::Image
    ) {
        return Err("Cue does not support media replacement".into());
    }
    if cue.is_running() || cue.is_paused() {
        return Err("Stop the cue before replacing its media".into());
    }
    let cue_type = cue.cue_type();
    {
        drop(ws);
        let result = match cue_type {
            crate::cue::types::CueType::Audio => crate::commands::cue_cmds::set_audio_file(
                cue_id,
                file_path,
                state.clone(),
                app_handle,
            ),
            crate::cue::types::CueType::Video => crate::commands::cue_cmds::set_video_file(
                cue_id,
                file_path,
                state.clone(),
                app_handle,
            ),
            crate::cue::types::CueType::Image => crate::commands::cue_cmds::set_image_file(
                cue_id,
                file_path,
                state.clone(),
                app_handle,
            ),
            _ => unreachable!(),
        };
        if result.is_ok() {
            state.media_converter.mark_applied(&job_id, true)?;
        }
        return result;
    }
}

/// Restore a completed conversion's original input, but only if this Cue still
/// points at that job's output. This prevents an old job from overwriting a
/// later user assignment.
#[tauri::command]
pub fn restore_cue_media_path(
    job_id: String,
    state: tauri::State<'_, crate::state::AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let (cue_id, input, output) = state.media_converter.completed_paths(&job_id)?;
    let id = Uuid::parse_str(&cue_id).map_err(|_| "Cue id is not a valid UUID".to_string())?;
    let registry = state.registry.lock().map_err(|e| e.to_string())?;
    let mut ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let workspace_dir = ws
        .file_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_owned());
    let cue_list = ws.active_cue_list_mut().ok_or("No active cue list")?;
    let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
    if cue.is_running() || cue.is_paused() {
        return Err("Stop the cue before restoring its media".into());
    }
    let current = cue.media_file_path().ok_or("Cue has no media path")?;
    let current = if current.is_absolute() {
        current.to_path_buf()
    } else {
        workspace_dir
            .map(|p| p.join(current))
            .unwrap_or_else(|| current.to_path_buf())
    };
    let current =
        fs::canonicalize(&current).map_err(|_| "Current Cue media path could not be resolved")?;
    let expected =
        fs::canonicalize(&output).map_err(|_| "Completed conversion output does not exist")?;
    if current != expected {
        return Err("Cue media no longer points to this conversion output".into());
    }
    let cue_type = cue.cue_type();
    drop(registry);
    drop(ws);
    let input = input.to_string_lossy().into_owned();
    let result = match cue_type {
        crate::cue::types::CueType::Audio => {
            crate::commands::cue_cmds::set_audio_file(cue_id, input, state.clone(), app_handle)
        }
        crate::cue::types::CueType::Video => {
            crate::commands::cue_cmds::set_video_file(cue_id, input, state.clone(), app_handle)
        }
        crate::cue::types::CueType::Image => {
            crate::commands::cue_cmds::set_image_file(cue_id, input, state.clone(), app_handle)
        }
        _ => Err("Cue does not support media restoration".into()),
    };
    if result.is_ok() {
        state.media_converter.mark_applied(&job_id, false)?;
    }
    result
}

fn validate_cue_input(
    cue_id: &str,
    input: &Path,
    state: &crate::state::AppState,
) -> Result<MediaKind, String> {
    let id = Uuid::parse_str(cue_id).map_err(|_| "Cue id is not a valid UUID".to_string())?;
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue_list = ws.active_cue_list().ok_or("No active cue list")?;
    let cue = cue_list.get_recursive(&id).ok_or("Cue not found")?;
    let media_kind = match cue.cue_type() {
        crate::cue::types::CueType::Audio => MediaKind::Audio,
        crate::cue::types::CueType::Video => MediaKind::Video,
        crate::cue::types::CueType::Image => MediaKind::Image,
        _ => return Err("Cue does not support media conversion".into()),
    };
    let stored = cue.media_file_path().ok_or("Cue has no media path")?;
    let resolved = if stored.is_absolute() {
        stored.to_path_buf()
    } else if let Some(path) = ws.file_path.as_ref().and_then(|p| p.parent()) {
        path.join(stored)
    } else {
        stored.to_path_buf()
    };
    let requested =
        std::fs::canonicalize(input).map_err(|_| "Input media file does not exist".to_string())?;
    let authored =
        std::fs::canonicalize(resolved).map_err(|_| "Cue media file does not exist".to_string())?;
    if requested != authored {
        return Err("Input path does not match the cue's authored media".into());
    }
    Ok(media_kind)
}

fn cue_media_path(cue_id: &str, state: &crate::state::AppState) -> Result<PathBuf, String> {
    let id = Uuid::parse_str(cue_id).map_err(|_| "Cue id is not a valid UUID".to_string())?;
    let ws = state.workspace.lock().map_err(|e| e.to_string())?;
    let cue = ws
        .active_cue_list()
        .ok_or("No active cue list")?
        .get_recursive(&id)
        .ok_or("Cue not found")?;
    let stored = cue.media_file_path().ok_or("Cue has no media path")?;
    Ok(if stored.is_absolute() {
        stored.to_path_buf()
    } else if let Some(path) = ws.file_path.as_ref().and_then(|p| p.parent()) {
        path.join(stored)
    } else {
        stored.to_path_buf()
    })
}

#[tauri::command]
pub fn open_media_output_folder(
    job_id: String,
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    let (_, path) = state.media_converter.completed_output(&job_id)?;
    let dir = path.parent().ok_or("Output path has no parent directory")?;
    if !dir.is_dir() {
        return Err("Output folder does not exist".into());
    }
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    command
        .arg(dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_args_are_vectorized_and_include_safe_defaults() {
        let request = MediaConversionRequest {
            cue_id: None,
            input_path: "C:/in clip.mov".into(),
            mode: ConversionMode::Optimize,
            quality: None,
            max_resolution: Some(1920),
            image_format: None,
        };
        let args = preset_args(
            &request,
            Path::new("C:/in clip.mov"),
            Path::new("C:/out.mp4"),
        );
        assert!(args.contains(&"-i".into()));
        assert!(args.contains(&"libx264".into()));
        assert!(args.iter().any(|arg| arg == "scale=trunc(min(iw\\,1920)/2)*2:trunc(min(ih\\,1920)/2)*2:force_original_aspect_ratio=decrease"));
        assert!(!args
            .iter()
            .any(|arg| arg.contains("cmd.exe") || arg.contains("/bin/sh")));
    }

    #[test]
    fn compatibility_reports_non_native_codecs() {
        let metadata = ProbeMetadata {
            path: "x.mkv".into(),
            file_size: 1,
            format_name: Some("matroska".into()),
            duration_seconds: Some(1.0),
            width: Some(1279),
            height: Some(720),
            video_codec: Some("vp9".into()),
            video_profile: None,
            video_bit_depth: Some(8),
            audio_codec: Some("opus".into()),
            audio_channels: Some(2),
            sample_rate: Some(48_000),
            pixel_format: Some("yuv420p".into()),
            frame_rate: Some(30.0),
        };
        let result = analyze_compatibility(&metadata);
        assert!(!result.compatible);
        assert!(result.reasons.len() >= 3);
    }

    #[test]
    fn ordinary_mp3_is_compatible_without_video_preset_requirements() {
        let metadata = ProbeMetadata {
            path: "song.mp3".into(),
            file_size: 4_000_000,
            format_name: Some("mp3".into()),
            duration_seconds: Some(264.0),
            width: None,
            height: None,
            video_codec: None,
            video_profile: None,
            video_bit_depth: None,
            audio_codec: Some("mp3".into()),
            audio_channels: Some(2),
            sample_rate: Some(44_100),
            pixel_format: None,
            frame_rate: None,
        };
        let result = analyze_compatibility(&metadata);
        assert_eq!(result.status, CompatibilityStatus::Compatible);
        assert!(result.compatible);
        assert!(result.reasons.is_empty());
    }

    #[test]
    fn audio_codec_outside_enabled_symphonia_features_is_unsupported() {
        let metadata = ProbeMetadata {
            path: "song.opus".into(),
            file_size: 1_000,
            format_name: Some("ogg".into()),
            duration_seconds: Some(1.0),
            width: None,
            height: None,
            video_codec: None,
            video_profile: None,
            video_bit_depth: None,
            audio_codec: Some("opus".into()),
            audio_channels: Some(2),
            sample_rate: Some(48_000),
            pixel_format: None,
            frame_rate: None,
        };
        let result = analyze_compatibility(&metadata);
        assert_eq!(result.status, CompatibilityStatus::Unsupported);
        assert!(!result.compatible);
        assert!(result.reasons.iter().any(|reason| reason.contains("opus")));
    }

    #[test]
    fn playable_video_is_compatible_even_without_stage_preset_shape() {
        let metadata = ProbeMetadata {
            path: "show.mkv".into(),
            file_size: 1_000,
            format_name: Some("matroska,webm".into()),
            duration_seconds: Some(1.0),
            width: Some(1920),
            height: Some(1080),
            video_codec: Some("h264".into()),
            video_profile: Some("High".into()),
            video_bit_depth: Some(8),
            audio_codec: Some("mp3".into()),
            audio_channels: Some(1),
            sample_rate: Some(44_100),
            pixel_format: Some("yuv420p".into()),
            frame_rate: Some(30.0),
        };
        let result = analyze_compatibility(&metadata);
        assert_eq!(result.status, CompatibilityStatus::Compatible);
        assert!(result.compatible);
        assert!(result.reasons.is_empty());
    }

    #[test]
    fn output_name_is_unique_and_uses_converted_suffix() {
        let dir = std::env::temp_dir().join(format!("qlisa-converter-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("show.mov");
        fs::write(&input, b"x").unwrap();
        let first = unique_output_path(&input, "mp4").unwrap();
        fs::write(&first, b"x").unwrap();
        let second = unique_output_path(&input, "mp4").unwrap();
        assert_eq!(first.file_name().unwrap(), "show_converted.mp4");
        assert_eq!(second.file_name().unwrap(), "show_converted_1.mp4");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_quality_and_missing_input_are_rejected() {
        let request = MediaConversionRequest {
            cue_id: None,
            input_path: "".into(),
            mode: ConversionMode::Size,
            quality: Some(52),
            max_resolution: None,
            image_format: None,
        };
        assert!(validate_request(&request).is_err());
    }

    #[test]
    fn invalid_frame_rates_do_not_become_nan() {
        assert_eq!(parse_frame_rate("0/0"), None);
        assert_eq!(parse_frame_rate("30000/1001"), Some(30000.0 / 1001.0));
        assert_eq!(parse_frame_rate("25"), Some(25.0));
        assert_eq!(parse_frame_rate("not-a-rate"), None);
    }

    #[test]
    fn attached_cover_art_is_not_reported_as_video() {
        let value = serde_json::json!({
            "format": {"format_name": "mp3", "duration": "12.5"},
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "png", "width": 300, "height": 300,
                 "r_frame_rate": "90000/1"},
                {"index": 1, "codec_type": "audio", "codec_name": "mp3", "channels": 2,
                 "sample_rate": "44100"}
            ]
        });
        let streams = value.get("streams").unwrap().as_array().unwrap();
        let video = streams
            .iter()
            .find(|stream| is_primary_video_stream(stream, true));
        let audio = streams
            .iter()
            .find(|stream| stream.get("codec_type").and_then(|v| v.as_str()) == Some("audio"));
        assert!(video.is_none());
        assert_eq!(
            audio
                .and_then(|s| s.get("codec_name"))
                .and_then(|v| v.as_str()),
            Some("mp3")
        );
    }

    #[test]
    fn audio_preset_does_not_map_cover_art_or_video() {
        let request = MediaConversionRequest {
            cue_id: None,
            input_path: "C:/music/track with cover.mp3".into(),
            mode: ConversionMode::Optimize,
            quality: None,
            max_resolution: None,
            image_format: None,
        };
        let args = audio_preset_args(
            &request,
            Path::new("C:/music/track with cover.mp3"),
            Path::new("C:/music/track with cover_converted.m4a"),
        );
        assert!(args.windows(2).any(|pair| pair == ["-map", "0:a:0"]));
        assert!(args.windows(2).any(|pair| pair == ["-vn", "-c:a"]));
        assert!(args.iter().any(|arg| arg == "ipod"));
        assert!(!args.iter().any(|arg| arg == "0:v:0?"));
        assert!(!args.iter().any(|arg| arg == "libx264"));
    }

    #[test]
    fn image_preset_uses_requested_format_and_never_maps_audio() {
        let request = MediaConversionRequest {
            cue_id: None,
            input_path: "C:/pictures/large image.png".into(),
            mode: ConversionMode::Size,
            quality: Some(3),
            max_resolution: Some(1920),
            image_format: Some("jpg".into()),
        };
        let args = image_preset_args(
            &request,
            Path::new("C:/pictures/large image.png"),
            Path::new("C:/pictures/large image_converted.jpg"),
            "jpg",
        );
        assert!(args.windows(2).any(|pair| pair == ["-frames:v", "1"]));
        assert!(args.windows(2).any(|pair| pair == ["-c:v", "mjpeg"]));
        assert!(args.windows(2).any(|pair| pair == ["-q:v", "8"]));
        assert!(args.iter().any(|arg| arg.contains("scale=min(iw\\,1920)")));
        assert!(!args.iter().any(|arg| arg == "0:a:0"));
    }

    #[test]
    fn image_output_name_keeps_converted_suffix_and_format() {
        let dir = std::env::temp_dir().join(format!("qlisa-image-converter-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("poster.png");
        fs::write(&input, b"x").unwrap();
        let output = unique_output_path(&input, "jpg").unwrap();
        assert_eq!(output.file_name().unwrap(), "poster_converted.jpg");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn conversion_queue_slot_allows_only_one_owner() {
        let queue = QueueState {
            pending: Mutex::new(VecDeque::new()),
            active: AtomicBool::new(false),
        };
        assert!(queue
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok());
        assert!(queue
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err());
        queue.active.store(false, Ordering::Release);
        assert!(queue
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok());
    }
}
