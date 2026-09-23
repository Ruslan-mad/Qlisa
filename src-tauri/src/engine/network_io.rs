//! Network video input/output workers and runtime discovery.
//!
//! This module implements NDI and SRT output workers, NDI/SRT camera input,
//! and their runtime status. Output workers consume composited video and
//! program audio; input workers feed network sources to camera cues. NDI uses
//! the user's installed runtime, while SRT uses FFmpeg with SRT support.
//!
//! NDI is dynamically loaded. The NDI SDK explicitly supports this model for
//! open-source applications: Qlisa locates the user-installed runtime through
//! its environment variables or standard Windows install directories. No NDI
//! headers, import library, or SDK is needed to compile Qlisa.

use std::{
    collections::VecDeque,
    ffi::{c_char, c_void, CString},
    io::{Read, Write},
    ops::{Deref, DerefMut},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Condvar, Mutex, OnceLock},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::audio_engine::{
    ProgramAudioFormat, ProgramAudioFormatSnapshot, ProgramAudioReceiver, SyntheticFeedProducer,
};

/// A network protocol carried by a source or destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkProtocol {
    Ndi,
    Srt,
}

/// The NDI sender profile exposed to an operator.
///
/// Core NDI has no sender-side bandwidth profile in this API. `LowBandwidth`
/// caps the submitted raster at 1280×720 while preserving aspect ratio;
/// `Highest` preserves the compositor's original raster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NdiQuality {
    /// Preserve the composited program image; the normal production default.
    #[default]
    Highest,
    /// Favour network headroom for monitoring or constrained links.
    LowBandwidth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NdiOutputSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ndi_stream_name")]
    pub stream_name: String,
    #[serde(default)]
    pub quality: NdiQuality,
    /// Optional NDI group. Empty uses the NDI runtime's default group.
    #[serde(default)]
    pub group: String,
}

fn default_ndi_stream_name() -> String {
    "Qlisa Program".into()
}

impl Default for NdiOutputSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            stream_name: default_ndi_stream_name(),
            quality: NdiQuality::Highest,
            group: String::new(),
        }
    }
}

/// SRT handshake role.  Listener is the safe default for a local receiving
/// endpoint; Caller and Rendezvous cover the usual WAN workflows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SrtMode {
    #[default]
    Listener,
    Caller,
    Rendezvous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mode: SrtMode,
    /// Remote host for caller/rendezvous; bind host for listener.  An empty
    /// listener host means all local interfaces (`0.0.0.0`).
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_srt_port")]
    pub port: u16,
    /// Receiver latency in milliseconds.  The default is deliberately
    /// conservative for a show network; the UI may offer lower presets.
    #[serde(default = "default_srt_latency_ms")]
    pub latency_ms: u32,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub stream_id: Option<String>,
    /// `None` lets the transport choose MPEG-TS's normal payload size.
    #[serde(default)]
    pub payload_size: Option<u16>,
    #[serde(default = "default_true")]
    pub too_late_packet_drop: bool,
    /// Optional encoder hints retained for the future sender worker.  They do
    /// not change transport negotiation and are intentionally not sent to an
    /// SRT receiver while frame transport is not implemented.
    #[serde(default)]
    pub bitrate_kbps: Option<u32>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub fps: Option<u32>,
    #[serde(default)]
    pub codec: Option<String>,
}

fn default_srt_port() -> u16 {
    9000
}
fn default_srt_latency_ms() -> u32 {
    120
}
fn default_true() -> bool {
    true
}

impl Default for SrtSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: SrtMode::Listener,
            host: String::new(),
            port: default_srt_port(),
            latency_ms: default_srt_latency_ms(),
            passphrase: None,
            stream_id: None,
            payload_size: None,
            too_late_packet_drop: true,
            bitrate_kbps: None,
            width: None,
            height: None,
            fps: None,
            codec: None,
        }
    }
}

impl SrtSettings {
    /// Validate settings before a worker opens a socket.  Password length and
    /// payload limits are the limits documented by libsrt/FFmpeg.
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("SRT port must be between 1 and 65535".into());
        }
        if self.latency_ms < 20 || self.latency_ms > 10_000 {
            return Err("SRT latency must be between 20 and 10000 ms".into());
        }
        if matches!(self.mode, SrtMode::Caller | SrtMode::Rendezvous) && self.host.trim().is_empty()
        {
            return Err("SRT caller/rendezvous mode requires a host".into());
        }
        let host = self.host.trim();
        if host.contains(char::is_whitespace)
            || host
                .chars()
                .any(|value| matches!(value, '?' | '#' | '/' | '@'))
        {
            return Err("SRT host contains unsupported characters".into());
        }
        if let Some(passphrase) = &self.passphrase {
            let n = passphrase.chars().count();
            if n != 0 && !(10..=79).contains(&n) {
                return Err("SRT passphrase must be 10–79 characters".into());
            }
        }
        if let Some(payload) = self.payload_size {
            if payload == 0 || payload > 1456 {
                return Err("SRT payload size must be between 1 and 1456 bytes".into());
            }
        }
        if let Some(rate) = self.bitrate_kbps {
            if !(100..=200_000).contains(&rate) {
                return Err("SRT bitrate must be between 100 and 200000 kbps".into());
            }
        }
        match (self.width, self.height) {
            (Some(width), Some(height))
                if (16..=16_384).contains(&width) && (16..=16_384).contains(&height) => {}
            (None, None) => {}
            _ => {
                return Err("SRT resolution requires width and height between 16 and 16384".into())
            }
        }
        if let Some(fps) = self.fps {
            if !(1..=240).contains(&fps) {
                return Err("SRT frame rate must be between 1 and 240 fps".into());
            }
        }
        if let Some(codec) = self.codec.as_deref() {
            if !codec.trim().is_empty()
                && !matches!(
                    codec.to_ascii_lowercase().as_str(),
                    "h264" | "hevc" | "h265"
                )
            {
                return Err("SRT codec must be H.264 or HEVC".into());
            }
        }
        Ok(())
    }

    /// A standards-compatible URL for libavformat/libmpv input or the future
    /// FFmpeg encoder worker.  Secrets remain percent-encoded and must never
    /// be written to diagnostics or logs.
    pub fn url(&self) -> Result<String, String> {
        self.validate()?;
        let host = if self.host.trim().is_empty() {
            "0.0.0.0"
        } else {
            self.host.trim()
        };
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        let mode = match self.mode {
            SrtMode::Caller => "caller",
            SrtMode::Listener => "listener",
            SrtMode::Rendezvous => "rendezvous",
        };
        let mut options = vec![
            format!("mode={mode}"),
            format!("latency={}", self.latency_ms * 1_000),
            format!(
                "tlpktdrop={}",
                if self.too_late_packet_drop { 1 } else { 0 }
            ),
        ];
        if let Some(payload) = self.payload_size {
            options.push(format!("pkt_size={payload}"));
        }
        if let Some(stream_id) = self.stream_id.as_deref().filter(|v| !v.is_empty()) {
            options.push(format!("streamid={}", percent_encode(stream_id)));
        }
        if let Some(passphrase) = self.passphrase.as_deref().filter(|v| !v.is_empty()) {
            options.push(format!("passphrase={}", percent_encode(passphrase)));
        }
        Ok(format!("srt://{host}:{}?{}", self.port, options.join("&")))
    }
}

/// Host text used in diagnostics. Credentials and query parameters are never
/// included; an empty listener host means all local interfaces.
pub fn srt_host(settings: &SrtSettings) -> &str {
    let host = settings.host.trim();
    if host.is_empty() { "0.0.0.0" } else { host }
}

/// A persistent network input reference.  NDI input is received by the future
/// SDK worker; SRT input can be supplied to libmpv/FFmpeg using `srt_url()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "protocol")]
pub enum NetworkInputSource {
    Ndi { source_name: String },
    Srt { settings: SrtSettings },
}

impl NetworkInputSource {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Ndi { source_name } if source_name.trim().is_empty() => {
                Err("NDI source name is required".into())
            }
            Self::Ndi { .. } => Ok(()),
            Self::Srt { settings } => settings.validate(),
        }
    }

    pub fn srt_url(&self) -> Result<Option<String>, String> {
        match self {
            Self::Ndi { .. } => Ok(None),
            Self::Srt { settings } => settings.url().map(Some),
        }
    }
}

/// The optional transport configuration attached to one named output
/// destination.  Multiple destinations therefore naturally map to multiple
/// independent network streams, just as they map to independent screens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NetworkOutputSettings {
    #[serde(default)]
    pub ndi: NdiOutputSettings,
    #[serde(default)]
    pub srt: SrtSettings,
}

impl NetworkOutputSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.ndi.enabled && self.ndi.stream_name.trim().is_empty() {
            return Err("NDI stream name is required when NDI is enabled".into());
        }
        if self.srt.enabled {
            self.srt.validate()?;
        }
        Ok(())
    }
}

/// Runtime truth used by Preferences.  `Ready` only means the required local
/// library was found; `Streaming` is reserved for the future running worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportAvailability {
    Ready,
    Unavailable,
    NotImplemented,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportProviderStatus {
    pub protocol: NetworkProtocol,
    pub availability: TransportAvailability,
    pub detail: String,
    pub library_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkIoStatus {
    pub ndi: TransportProviderStatus,
    pub srt: TransportProviderStatus,
}

/// Probe optional dependencies without starting a sender or opening sockets.
pub fn runtime_status() -> NetworkIoStatus {
    // Go through the same process-wide runtime holder used by all senders and
    // receivers.  A short-lived probe Library here could otherwise be the
    // last handle which unloads the DLL while Core NDI-owned threads still
    // execute code from it.
    let ndi = match NdiRuntime::load() {
        Ok(runtime) => TransportProviderStatus {
            protocol: NetworkProtocol::Ndi,
            availability: TransportAvailability::Ready,
            detail: "NDI Runtime found. Sender/receiver workers are available when an NDI output or input is started.".into(),
            library_path: Some(runtime.library_path().display().to_string()),
        },
        Err(_error) if find_ndi_runtime().is_none() => TransportProviderStatus {
            protocol: NetworkProtocol::Ndi, availability: TransportAvailability::Unavailable,
            detail: "NDI Runtime was not found. Install the NDI Runtime, restart Qlisa, and try again.".into(),
            library_path: None,
        },
        Err(error) => TransportProviderStatus {
            protocol: NetworkProtocol::Ndi,
            availability: TransportAvailability::Unavailable,
            detail: format!("NDI Runtime found but could not be initialized: {error}"),
            library_path: find_ndi_runtime().map(|path| path.display().to_string()),
        },
    };
    // SRT is ready only when the self-contained FFmpeg executable is actually
    // present. This prevents a stale setting from implying a stream can start.
    let srt = match find_ffmpeg_runtime() {
        Some(path) => TransportProviderStatus {
            protocol: NetworkProtocol::Srt,
            availability: TransportAvailability::Ready,
            detail: srt_provider_detail().into(),
            library_path: Some(path.display().to_string()),
        },
        None => TransportProviderStatus {
            protocol: NetworkProtocol::Srt,
            availability: TransportAvailability::Unavailable,
            detail: "FFmpeg with SRT was not found in this Qlisa installation.".into(),
            library_path: None,
        },
    };
    NetworkIoStatus { ndi, srt }
}

#[cfg(windows)]
fn srt_provider_detail() -> &'static str {
    "FFmpeg with SRT was found. The SRT sender starts on the first composited frame and carries program video and audio on Windows."
}

#[cfg(not(windows))]
fn srt_provider_detail() -> &'static str {
    "FFmpeg with SRT was found. The SRT sender starts on the first composited frame; SRT is video-only on this platform (NDI still carries program audio)."
}

/// Find only a user-installed NDI Runtime. A DLL beside the app or in its
/// resources may be a stale bundled version left by an older Qlisa install.
pub fn find_ndi_runtime() -> Option<PathBuf> {
    ndi_candidates().into_iter().find(|path| path.is_file())
}

fn ndi_candidates() -> Vec<PathBuf> {
    let dll = if cfg!(target_arch = "x86_64") {
        "Processing.NDI.Lib.x64.dll"
    } else {
        "Processing.NDI.Lib.x86.dll"
    };
    let mut paths = Vec::new();
    for variable in ["NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"] {
        if let Ok(dir) = std::env::var(variable) {
            paths.push(PathBuf::from(dir).join(dll));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            let root = PathBuf::from(program_files).join("NDI");
            paths.push(root.join("NDI 6 Runtime").join("v6").join(dll));
            paths.push(root.join("NDI 5 Runtime").join(dll));
        }
    }
    paths
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}

/// A composited BGRA program frame.  This is the neutral hand-off between the
/// OpenGL output engine and transport workers: neither SRT nor NDI code is
/// allowed to call into the display render thread or hold its GL context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgraFrame {
    pub width: u32,
    pub height: u32,
    /// Bytes per row.  It may be greater than `width * 4` when a GL readback
    /// buffer has alignment padding.
    pub stride: u32,
    pub data: Vec<u8>,
}

impl BgraFrame {
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("frame dimensions must be non-zero".into());
        }
        let min_stride = self.width.checked_mul(4).ok_or("frame width overflow")?;
        if self.stride < min_stride {
            return Err("BGRA frame stride is smaller than width × 4".into());
        }
        let required = (self.stride as usize)
            .checked_mul(self.height as usize)
            .ok_or("frame buffer size overflow")?;
        if self.data.len() < required {
            return Err("BGRA frame buffer is shorter than stride × height".into());
        }
        Ok(())
    }

    /// Copies a padded OpenGL readback into FFmpeg's tightly packed BGRA input.
    fn packed_bgra(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let packed_stride = (self.width * 4) as usize;
        if self.stride as usize == packed_stride {
            return Ok(self.data[..packed_stride * self.height as usize].to_vec());
        }
        let mut packed = Vec::with_capacity(packed_stride * self.height as usize);
        for row in 0..self.height as usize {
            let offset = row * self.stride as usize;
            packed.extend_from_slice(&self.data[offset..offset + packed_stride]);
        }
        Ok(packed)
    }

    /// Deterministic, tightly-packed nearest-neighbour downscale used by the
    /// NDI low-bandwidth sender path.
    fn downscaled_to_fit(&self, max_width: u32, max_height: u32) -> Result<Self, String> {
        self.validate()?;
        if max_width == 0 || max_height == 0 {
            return Err("downscale bounds must be non-zero".into());
        }
        if self.width <= max_width && self.height <= max_height {
            return Ok(self.clone());
        }
        let scale =
            (max_width as f64 / self.width as f64).min(max_height as f64 / self.height as f64);
        let width = ((self.width as f64 * scale).floor() as u32).max(1);
        let height = ((self.height as f64 * scale).floor() as u32).max(1);
        let stride = width
            .checked_mul(4)
            .ok_or("downscaled frame width overflow")?;
        let buffer_len = (stride as usize)
            .checked_mul(height as usize)
            .ok_or("downscaled frame buffer size overflow")?;
        let mut data = vec![0; buffer_len];
        for y in 0..height as usize {
            let source_y = (y * self.height as usize) / height as usize;
            for x in 0..width as usize {
                let source_x = (x * self.width as usize) / width as usize;
                let source_offset = source_y * self.stride as usize + source_x * 4;
                let target_offset = y * stride as usize + x * 4;
                data[target_offset..target_offset + 4]
                    .copy_from_slice(&self.data[source_offset..source_offset + 4]);
            }
        }
        Ok(Self {
            width,
            height,
            stride,
            data,
        })
    }
}

/// A one-frame mailbox for crossing a real-time boundary.  Producers replace
/// an unconsumed frame instead of accumulating latency; consumers take
/// ownership of the most recent frame.  It is shared by the NDI input worker
/// and the compositor upload path, and is intentionally independent from any
/// OpenGL or window type.
#[derive(Default)]
pub struct BgraFrameMailbox {
    latest: Mutex<Option<(u64, BgraFrame)>>,
    next_sequence: std::sync::atomic::AtomicU64,
}

impl BgraFrameMailbox {
    pub fn publish(&self, frame: BgraFrame) -> Option<BgraFrame> {
        let sequence = self
            .next_sequence
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        self.latest
            .lock()
            .ok()?
            .replace((sequence, frame))
            .map(|(_, frame)| frame)
    }

    /// Read the latest frame newer than this consumer's cursor. Unlike the
    /// former destructive `take_latest`, every output pipeline gets the same
    /// bounded newest NDI frame when one Camera Cue targets several outputs.
    pub fn latest_after(&self, sequence: u64) -> Option<(u64, BgraFrame)> {
        let latest = self.latest.lock().ok()?;
        let (current, frame) = latest.as_ref()?;
        (*current > sequence).then(|| (*current, frame.clone()))
    }

    /// Legacy single-consumer convenience. New rendering code must use a
    /// sequence cursor via [`Self::latest_after`] so it cannot steal frames
    /// from another output destination.
    pub fn take_latest(&self) -> Option<BgraFrame> {
        self.latest.lock().ok()?.take().map(|(_, frame)| frame)
    }
}

/// Cumulative process-side statistics. `submitted_frames` means Qlisa wrote a
/// raw frame to FFmpeg; it is deliberately not reported as a network-delivered
/// frame until the future SRT metrics worker reads libav/libsrt telemetry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SrtSenderStats {
    pub submitted_frames: u64,
    pub submitted_bytes: u64,
    pub failed_writes: u64,
    pub started_at_unix_ms: u64,
}

const FFMPEG_DIAGNOSTIC_LIMIT: usize = 8 * 1024;
const FFMPEG_DIAGNOSTIC_LINE_LIMIT: usize = 4 * 1024;
const SRT_GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_millis(250);
const SRT_WORKER_STOP_TIMEOUT: Duration = Duration::from_millis(750);
const SRT_AUDIO_QUEUE_MAX_MS: u32 = 500;

// The numeric value is the documented Win32 JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
// bit. Keeping the bit-setting logic platform independent lets its policy be
// covered by unit tests without creating an OS process or Job Object.
const FFMPEG_JOB_KILL_ON_CLOSE: u32 = 0x0000_2000;

fn ffmpeg_job_limit_flags(existing: u32) -> u32 {
    existing | FFMPEG_JOB_KILL_ON_CLOSE
}

#[cfg(windows)]
struct FfmpegJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl FfmpegJob {
    fn create() -> std::io::Result<Self> {
        use std::{mem, ptr};
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        };

        // Null SECURITY_ATTRIBUTES makes this handle non-inheritable; each
        // FFmpeg owns its own Job Object through ManagedFfmpegChild.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = ffmpeg_job_limit_flags(0);
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                mem::size_of_val(&limits) as u32,
            )
        };
        if configured == 0 {
            let error = std::io::Error::last_os_error();
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
            return Err(error);
        }
        Ok(Self(handle))
    }

    fn assign(&self, child: &Child) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        let assigned = unsafe {
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                self.0,
                child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
            )
        };
        if assigned == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
impl Drop for FfmpegJob {
    fn drop(&mut self) {
        // Closing the last process-owned handle terminates every process in the
        // job. This remains effective when `std::process::exit` bypasses Rust
        // destructors: Windows closes this handle as part of process teardown.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// A child process owned by a per-process Job Object on Windows.  Other
/// modules use this for isolated FFmpeg work so playback and conversion share
/// the same shutdown semantics.
pub(crate) struct ManagedFfmpegChild {
    child: Child,
    #[cfg(windows)]
    _job: FfmpegJob,
}

impl Deref for ManagedFfmpegChild {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl DerefMut for ManagedFfmpegChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

/// Spawn a managed FFmpeg child. The per-process Windows Job Object is
/// assigned immediately after spawn; `std::process::Command` has no suspended
/// process/job-list API, so a small spawn-to-assignment window remains. If
/// assignment fails, the child is killed and reaped before returning.
pub(crate) fn spawn_managed_ffmpeg(command: &mut Command) -> std::io::Result<ManagedFfmpegChild> {
    // FFmpeg and ffprobe are console applications. Qlisa starts them only for
    // background work, so callers must never flash a terminal on Windows.
    // Keeping this in the shared spawn path makes it impossible for a new
    // ffprobe/FFmpeg caller to accidentally omit the flag.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(windows)]
    let job = FfmpegJob::create()?;
    let mut child = command.spawn()?;
    #[cfg(windows)]
    if let Err(assign_error) = job.assign(&child) {
        let kill_error = child.kill().err();
        let wait_error = child.wait().err();
        let mut message =
            format!("Could not assign FFmpeg to its lifetime Job Object: {assign_error}");
        if let Some(error) = kill_error {
            message.push_str(&format!("; could not kill child: {error}"));
        }
        if let Some(error) = wait_error {
            message.push_str(&format!("; could not reap child: {error}"));
        }
        return Err(std::io::Error::new(std::io::ErrorKind::Other, message));
    }
    Ok(ManagedFfmpegChild {
        child,
        #[cfg(windows)]
        _job: job,
    })
}

impl ManagedFfmpegChild {
    pub(crate) fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.stdout.take()
    }

    pub(crate) fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.stderr.take()
    }
}

/// A worker whose OS thread handle stays owned by the sender until it has
/// observed cancellation and exited. Rust has no timed `join`; the done flag
/// lets us wait only a bounded interval and call `join` solely once it cannot
/// block. Windows named-pipe workers use a persistent cancellation event, so
/// no one-shot thread-cancellation race can strand one here.
struct ManagedThread {
    join: Option<JoinHandle<()>>,
    finished: Arc<AtomicBool>,
}

impl ManagedThread {
    fn spawn(name: String, work: impl FnOnce() + Send + 'static) -> std::io::Result<Self> {
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let join = thread::Builder::new().name(name).spawn(move || {
            struct FinishOnDrop(Arc<AtomicBool>);

            impl Drop for FinishOnDrop {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::Release);
                }
            }

            // A panicking worker is still joinable.  Never lose ownership of
            // its JoinHandle merely because its work closure unwound.
            let _finished = FinishOnDrop(worker_finished);
            work();
        })?;
        Ok(Self {
            join: Some(join),
            finished,
        })
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn join_finished(&mut self) {
        if self.is_finished() {
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }
}

impl Drop for ManagedThread {
    fn drop(&mut self) {
        // Dropping a JoinHandle detaches its thread. That is unacceptable for
        // a worker which can still own a named pipe during a failed start or
        // a timed-out stop: keep ownership until the cancellation path has
        // actually returned. Normal teardown takes the fast `join_finished`
        // branch above, so this only blocks on a broken/cancelled path.
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn cancel_and_join_workers(workers: &mut [&mut Option<ManagedThread>]) -> Result<(), String> {
    let deadline = Instant::now() + SRT_WORKER_STOP_TIMEOUT;
    while workers
        .iter()
        .filter_map(|worker| worker.as_ref())
        .any(|worker| !worker.is_finished())
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(5));
    }
    let mut pending = false;
    for worker in workers.iter_mut().filter_map(|worker| worker.as_mut()) {
        if worker.is_finished() {
            worker.join_finished();
        } else {
            pending = true;
        }
    }
    if pending {
        Err("FFmpeg SRT worker did not exit after cancellation".into())
    } else {
        Ok(())
    }
}

#[derive(Default)]
struct SrtWriterState {
    stats: SrtSenderStats,
    last_error: Option<String>,
    pending_audio_drop_warning_frames: u64,
    last_audio_drop_warning_at: Option<Instant>,
}

fn report_srt_audio_drops(state: &Mutex<SrtWriterState>, dropped_frames: usize) {
    if dropped_frames == 0 {
        return;
    }
    let Ok(mut state) = state.lock() else {
        return;
    };
    state.pending_audio_drop_warning_frames = state
        .pending_audio_drop_warning_frames
        .saturating_add(dropped_frames as u64);
    let should_warn = state
        .last_audio_drop_warning_at
        .map_or(true, |last| last.elapsed() >= Duration::from_secs(5));
    if should_warn {
        let dropped = std::mem::take(&mut state.pending_audio_drop_warning_frames);
        state.last_audio_drop_warning_at = Some(Instant::now());
        log::warn!(
            "[srt-output] dropped {dropped} oldest program-audio frames to keep the queue within {SRT_AUDIO_QUEUE_MAX_MS} ms"
        );
    }
}

/// Monotonic cadence which never catches up by bursting stale program frames.
struct FramePacer {
    interval: Duration,
    next_frame_at: Option<Instant>,
}
impl FramePacer {
    fn new(fps: u32) -> Self {
        Self {
            interval: Duration::from_secs_f64(1.0 / f64::from(fps)),
            next_frame_at: None,
        }
    }
    fn is_due(&self, now: Instant) -> bool {
        self.next_frame_at.map_or(true, |next| now >= next)
    }
    fn wait_duration(&self, now: Instant) -> Option<Duration> {
        self.next_frame_at
            .map(|next| next.saturating_duration_since(now))
    }
    fn mark_submitted(&mut self, now: Instant) {
        // Keep the cadence anchored to the previous deadline.  `now` is
        // observed after packing and writing the frame, so basing every next
        // deadline on it would add the write cost to every frame interval and
        // slowly reduce the actual stream rate.  If the writer fell behind by
        // a full interval, reset from now instead of catching up with bursts.
        let next = self
            .next_frame_at
            .map(|previous| {
                let scheduled = previous + self.interval;
                if scheduled > now {
                    scheduled
                } else {
                    now + self.interval
                }
            })
            .unwrap_or(now + self.interval);
        self.next_frame_at = Some(next);
    }
}

/// Remove SRT endpoints and passphrases before a command, decoder, or FFmpeg
/// diagnostic is allowed into Qlisa's log/status surface.  This intentionally
/// handles shell-like quoted `passphrase='…'` forms too: FFmpeg and libmpv use
/// both that spelling and URL query strings in their errors.
pub fn redact_srt_diagnostic(input: &str) -> String {
    // FFmpeg repeats its output URL in errors. It must never surface an SRT
    // endpoint or passphrase through our status/logging path.
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes.len() - i >= 6 && bytes[i..i + 6].eq_ignore_ascii_case(b"srt://") {
            out.push_str("[SRT endpoint redacted]");
            // Do not stop at a quote: diagnostics sometimes spell a query as
            // `srt://…?passphrase='secret'`, where the first quote belongs to
            // the value rather than the surrounding command argument.
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && !matches!(bytes[i], b')' | b']' | b'}')
            {
                i += 1;
            }
            continue;
        }
        if bytes.len() - i >= 10 && bytes[i..i + 10].eq_ignore_ascii_case(b"passphrase") {
            out.push_str("passphrase=[redacted]");
            i += 10;
            while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'=' | b':') {
                i += 1;
            }
            if i < bytes.len() && matches!(bytes[i], b'\'' | b'\"') {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else if bytes[i] == quote {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            } else {
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b'&' | b',' | b')' | b']' | b'}')
                {
                    i += 1;
                }
            }
            continue;
        }
        let ch = input[i..].chars().next().expect("index in bounds");
        out.push(ch);
        i += ch.len_utf8();
    }
    out.trim().to_string()
}

fn sanitized_ffmpeg_diagnostic(input: &str) -> String {
    redact_srt_diagnostic(input)
}

/// A process reader can split secrets between reads. Keep an incomplete line
/// private, bounded, and sanitize only complete lines before retaining them.
#[derive(Default)]
struct FfmpegDiagnostics {
    rendered: String,
    pending: String,
    discarding_long_line: bool,
}
impl FfmpegDiagnostics {
    fn append(&mut self, text: &str) {
        for piece in text.split_inclusive('\n') {
            if self.discarding_long_line {
                if piece.ends_with('\n') {
                    self.discarding_long_line = false;
                }
                continue;
            }
            self.pending.push_str(piece);
            while let Some(pos) = self.pending.find('\n') {
                let line: String = self.pending.drain(..=pos).collect();
                self.push_sanitized(&line);
            }
            if self.pending.len() > FFMPEG_DIAGNOSTIC_LINE_LIMIT {
                self.pending.clear();
                self.discarding_long_line = true;
            }
        }
    }
    fn push_sanitized(&mut self, text: &str) {
        let text = sanitized_ffmpeg_diagnostic(text);
        if text.is_empty() {
            return;
        }
        self.rendered.push_str(&text);
        self.rendered.push('\n');
        if self.rendered.len() > FFMPEG_DIAGNOSTIC_LIMIT {
            let cut = self
                .rendered
                .char_indices()
                .find_map(|(n, _)| {
                    (self.rendered.len() - n <= FFMPEG_DIAGNOSTIC_LIMIT).then_some(n)
                })
                .unwrap_or(self.rendered.len());
            self.rendered.drain(..cut);
        }
    }
    fn snapshot(&self) -> String {
        let mut result = self.rendered.clone();
        if !self.discarding_long_line {
            result.push_str(&sanitized_ffmpeg_diagnostic(&self.pending));
        }
        result.trim().to_string()
    }
}

fn spawn_stderr_drain(
    mut stderr: std::process::ChildStderr,
    diagnostic: Arc<Mutex<FfmpegDiagnostics>>,
) -> ManagedThread {
    ManagedThread::spawn("qlisa-srt-ffmpeg-stderr".into(), move || {
        let mut buffer = [0; 1024];
        while let Ok(count) = stderr.read(&mut buffer) {
            if count == 0 {
                break;
            }
            if let Ok(mut diagnostic) = diagnostic.lock() {
                diagnostic.append(&String::from_utf8_lossy(&buffer[..count]));
            }
        }
    })
    .expect("spawn FFmpeg stderr drain")
}

/// Fallible variant used while constructing a receiver. A receiver may own an
/// already-spawned FFmpeg child when OS thread creation fails, so that path
/// must return an error to its explicit kill/wait cleanup instead of panicking
/// or silently dropping the child handle.
fn try_spawn_stderr_drain(
    mut stderr: std::process::ChildStderr,
    diagnostic: Arc<Mutex<FfmpegDiagnostics>>,
) -> Result<ManagedThread, String> {
    ManagedThread::spawn("qlisa-srt-ffmpeg-stderr".into(), move || {
        let mut buffer = [0; 1024];
        while let Ok(count) = stderr.read(&mut buffer) {
            if count == 0 {
                break;
            }
            if let Ok(mut diagnostic) = diagnostic.lock() {
                diagnostic.append(&String::from_utf8_lossy(&buffer[..count]));
            }
        }
    })
    .map_err(|error| format!("Could not start FFmpeg SRT diagnostic reader: {error}"))
}

fn set_srt_writer_error(state: &Mutex<SrtWriterState>, error: String) {
    if let Ok(mut state) = state.lock() {
        state.stats.failed_writes += 1;
        state.last_error = Some(sanitized_ffmpeg_diagnostic(&error));
    }
}

/// One FFmpeg raw-input pipe. Windows uses named pipes so video and program
/// audio can be muxed by the *same* hidden FFmpeg process; Unix keeps stdin as
/// the video fallback until its native multiplexer implementation lands.
enum SrtPipeWriter {
    Stdin(ChildStdin),
    #[cfg(windows)]
    Named(WindowsPipeServer),
}

impl SrtPipeWriter {
    fn connect(&mut self) -> std::io::Result<()> {
        match self {
            Self::Stdin(_) => Ok(()),
            #[cfg(windows)]
            Self::Named(pipe) => pipe.connect(),
        }
    }

    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Stdin(input) => input.write_all(data),
            #[cfg(windows)]
            Self::Named(pipe) => pipe.write_all(data),
        }
    }
}

/// FFmpeg's decoded SRT-camera audio. Windows uses the same cancellable
/// overlapped named-pipe transport as program output; Unix keeps its ordinary
/// stdout pipe because SRT program audio is intentionally video-only there.
enum SrtPipeReader {
    #[cfg(not(windows))]
    Stdout(std::process::ChildStdout),
    #[cfg(windows)]
    Named(WindowsPipeServer),
}

impl SrtPipeReader {
    fn read(&mut self, data: &mut [u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(not(windows))]
            Self::Stdout(stdout) => stdout.read(data),
            #[cfg(windows)]
            Self::Named(pipe) => pipe.read(data),
        }
    }
}

#[cfg(windows)]
struct WindowsPipeCancellation {
    /// Manual-reset event owned independently of the worker. Once set, every
    /// current *and future* named-pipe operation observes cancellation. This
    /// is what closes the stop-before-ConnectNamedPipe race.
    event: windows_sys::Win32::Foundation::HANDLE,
    cancelled: AtomicBool,
}

#[cfg(windows)]
impl WindowsPipeCancellation {
    fn new() -> Result<Arc<Self>, String> {
        use windows_sys::Win32::System::Threading::CreateEventW;
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if event == 0 {
            return Err(format!(
                "Could not create SRT pipe cancellation event: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Arc::new(Self {
            event,
            cancelled: AtomicBool::new(false),
        }))
    }

    fn cancel(&self) {
        use windows_sys::Win32::System::Threading::SetEvent;
        self.cancelled.store(true, Ordering::Release);
        unsafe {
            let _ = SetEvent(self.event);
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[cfg(windows)]
impl Drop for WindowsPipeCancellation {
    fn drop(&mut self) {
        if self.event != 0 {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.event);
            }
        }
    }
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum WindowsPipeDirection {
    /// Qlisa writes raw media and FFmpeg reads it.
    Outbound,
    /// FFmpeg writes decoded media and Qlisa reads it.
    Inbound,
}

#[cfg(windows)]
struct WindowsPipeServer {
    name: String,
    handle: windows_sys::Win32::Foundation::HANDLE,
    io_event: windows_sys::Win32::Foundation::HANDLE,
    connected: bool,
    cancellation: Arc<WindowsPipeCancellation>,
}

#[cfg(windows)]
impl WindowsPipeServer {
    fn create(
        kind: &str,
        direction: WindowsPipeDirection,
        cancellation: Arc<WindowsPipeCancellation>,
    ) -> Result<Self, String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            Storage::FileSystem::FILE_FLAG_OVERLAPPED,
            System::Pipes::{CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT},
            System::Threading::CreateEventW,
        };
        static NEXT_PIPE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let serial = NEXT_PIPE.fetch_add(1, Ordering::Relaxed);
        let name = format!(r"\\.\pipe\qlisa-srt-{kind}-{}-{serial}", std::process::id());
        let wide: Vec<u16> = std::ffi::OsStr::new(&name)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let handle = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                match direction {
                    WindowsPipeDirection::Outbound => {
                        windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_OUTBOUND
                    }
                    WindowsPipeDirection::Inbound => {
                        windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND
                    }
                } | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                1_048_576,
                1_048_576,
                0,
                ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "Could not create FFmpeg {kind} named pipe: {}",
                std::io::Error::last_os_error()
            ));
        }
        let io_event = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
        if io_event == 0 {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
            return Err(format!(
                "Could not create FFmpeg {kind} pipe I/O event: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self {
            name,
            handle,
            io_event,
            connected: false,
            cancellation,
        })
    }

    fn cancelled_error() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::Interrupted, "SRT named pipe cancelled")
    }

    fn wait_overlapped(
        &self,
        overlapped: &mut windows_sys::Win32::System::IO::OVERLAPPED,
    ) -> std::io::Result<u32> {
        use windows_sys::Win32::{
            Foundation::{GetLastError, WAIT_FAILED, WAIT_OBJECT_0},
            System::{
                Threading::{WaitForMultipleObjects, INFINITE},
                IO::{CancelIoEx, GetOverlappedResult},
            },
        };
        let handles = [self.io_event, self.cancellation.event];
        let waited = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
        if waited == WAIT_OBJECT_0 + 1 {
            // This call is made by the I/O-owning thread *after* its operation
            // was submitted. Therefore it cannot miss the operation as the old
            // cross-thread CancelSynchronousIo approach could.
            unsafe {
                let _ = CancelIoEx(self.handle, overlapped);
                let mut ignored = 0;
                let _ = GetOverlappedResult(self.handle, overlapped, &mut ignored, 1);
            }
            return Err(Self::cancelled_error());
        }
        if waited == WAIT_FAILED {
            return Err(std::io::Error::from_raw_os_error(unsafe {
                GetLastError() as i32
            }));
        }
        if waited != WAIT_OBJECT_0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "unexpected SRT named-pipe wait result",
            ));
        }
        let mut transferred = 0;
        let result = unsafe { GetOverlappedResult(self.handle, overlapped, &mut transferred, 0) };
        if result == 0 {
            return Err(std::io::Error::from_raw_os_error(unsafe {
                GetLastError() as i32
            }));
        }
        Ok(transferred)
    }

    fn connect(&mut self) -> std::io::Result<()> {
        if self.connected {
            return Ok(());
        }
        if self.cancellation.is_cancelled() {
            return Err(Self::cancelled_error());
        }
        use windows_sys::Win32::{
            Foundation::{GetLastError, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED},
            System::Pipes::ConnectNamedPipe,
            System::Threading::ResetEvent,
            System::IO::OVERLAPPED,
        };
        unsafe {
            let _ = ResetEvent(self.io_event);
        }
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = self.io_event;
        let result = unsafe { ConnectNamedPipe(self.handle, &mut overlapped) };
        if result == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_IO_PENDING {
                let _ = self.wait_overlapped(&mut overlapped)?;
            } else if error != ERROR_PIPE_CONNECTED {
                return Err(std::io::Error::from_raw_os_error(error as i32));
            }
        }
        self.connected = true;
        Ok(())
    }

    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.connect()?;
        use windows_sys::Win32::{
            Foundation::{GetLastError, ERROR_IO_PENDING},
            Storage::FileSystem::WriteFile,
            System::Threading::ResetEvent,
            System::IO::OVERLAPPED,
        };
        let mut remaining = data;
        while !remaining.is_empty() {
            if self.cancellation.is_cancelled() {
                return Err(Self::cancelled_error());
            }
            unsafe {
                let _ = ResetEvent(self.io_event);
            }
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            overlapped.hEvent = self.io_event;
            let mut written = 0;
            let chunk_len = u32::try_from(remaining.len()).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "SRT pipe write is too large",
                )
            })?;
            let result = unsafe {
                WriteFile(
                    self.handle,
                    remaining.as_ptr(),
                    chunk_len,
                    &mut written,
                    &mut overlapped,
                )
            };
            if result == 0 {
                let error = unsafe { GetLastError() };
                if error != ERROR_IO_PENDING {
                    return Err(std::io::Error::from_raw_os_error(error as i32));
                }
                written = self.wait_overlapped(&mut overlapped)?;
            }
            if written == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "FFmpeg SRT pipe accepted no bytes",
                ));
            }
            remaining = &remaining[written as usize..];
        }
        Ok(())
    }

    fn read(&mut self, data: &mut [u8]) -> std::io::Result<usize> {
        self.connect()?;
        if self.cancellation.is_cancelled() {
            return Err(Self::cancelled_error());
        }
        use windows_sys::Win32::{
            Foundation::{GetLastError, ERROR_IO_PENDING},
            Storage::FileSystem::ReadFile,
            System::Threading::ResetEvent,
            System::IO::OVERLAPPED,
        };
        unsafe {
            let _ = ResetEvent(self.io_event);
        }
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = self.io_event;
        let mut read = 0;
        let length = u32::try_from(data.len()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "SRT pipe read is too large",
            )
        })?;
        let result = unsafe {
            ReadFile(
                self.handle,
                data.as_mut_ptr(),
                length,
                &mut read,
                &mut overlapped,
            )
        };
        if result == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_IO_PENDING {
                return Err(std::io::Error::from_raw_os_error(error as i32));
            }
            read = self.wait_overlapped(&mut overlapped)?;
        }
        Ok(read as usize)
    }
}

#[cfg(windows)]
impl Drop for WindowsPipeServer {
    fn drop(&mut self) {
        if self.handle != 0 && self.handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.handle);
            }
        }
        if self.io_event != 0 {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.io_event);
            }
        }
    }
}

fn spawn_srt_pipe_writer(
    mut input: SrtPipeWriter,
    width: u32,
    height: u32,
    fps: u32,
    latest: Arc<(Mutex<Option<BgraFrame>>, Condvar)>,
    running: Arc<AtomicBool>,
    state: Arc<Mutex<SrtWriterState>>,
    diagnostic: Arc<Mutex<FfmpegDiagnostics>>,
) -> ManagedThread {
    ManagedThread::spawn("qlisa-srt-ffmpeg-stdin".into(), move || {
        if let Err(error) = input.connect() {
            set_srt_writer_error(
                &state,
                format!("Could not connect FFmpeg video pipe: {error}"),
            );
            return;
        }
        let mut pacer = FramePacer::new(fps);
        let mut last_packed: Option<Vec<u8>> = None;
        while running.load(Ordering::Relaxed) {
            let next_frame = {
                let (lock, signal) = latest.as_ref();
                let mut pending = match lock.lock() {
                    Ok(value) => value,
                    Err(_) => {
                        set_srt_writer_error(&state, "SRT frame mailbox lock poisoned".into());
                        return;
                    }
                };
                loop {
                    if !running.load(Ordering::Relaxed) {
                        return;
                    }
                    let now = Instant::now();
                    // After the first program image, the declared output rate
                    // owns cadence. A static compositor therefore remains a
                    // valid continuous video stream rather than freezing on
                    // the last input callback.
                    if pacer.is_due(now) && (pending.is_some() || last_packed.is_some()) {
                        break pending.take();
                    }
                    let timeout = if last_packed.is_some() || pending.is_some() {
                        pacer.wait_duration(now).unwrap_or(Duration::ZERO)
                    } else {
                        Duration::from_millis(100)
                    };
                    let (next, _) = signal
                        .wait_timeout(pending, timeout)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    pending = next;
                }
            };
            if let Some(frame) = next_frame {
                if frame.width != width || frame.height != height {
                    set_srt_writer_error(&state, "SRT frame size changed while streaming".into());
                    return;
                }
                last_packed = Some(match frame.packed_bgra() {
                    Ok(value) => value,
                    Err(error) => {
                        set_srt_writer_error(&state, error);
                        return;
                    }
                });
            }
            let Some(packed) = last_packed.as_deref() else {
                continue;
            };
            if let Err(error) = input.write_all(packed) {
                let detail = diagnostic
                    .lock()
                    .ok()
                    .map(|value| value.snapshot())
                    .unwrap_or_default();
                let summary = format!("Could not write a frame to FFmpeg SRT sender: {error}");
                set_srt_writer_error(
                    &state,
                    if detail.is_empty() {
                        summary
                    } else {
                        format!("{summary}: {detail}")
                    },
                );
                return;
            }
            pacer.mark_submitted(Instant::now());
            if let Ok(mut state) = state.lock() {
                state.stats.submitted_frames += 1;
                state.stats.submitted_bytes += packed.len() as u64;
            }
        }
    })
    .expect("spawn FFmpeg stdin writer")
}

#[derive(Debug, PartialEq)]
struct SrtAudioPacket {
    sample_rate: u32,
    channels: u32,
    samples: Vec<f32>,
}

/// A bounded, duration-based FIFO between the program-audio tap and FFmpeg.
/// When the encoder falls behind, discard the oldest frames so live audio
/// remains close to the current program position instead of restarting SRT.
struct SrtAudioQueue {
    packets: VecDeque<SrtAudioPacket>,
    queued_frames: usize,
    capacity_frames: usize,
    sample_rate: u32,
}

impl SrtAudioQueue {
    fn new(sample_rate: u32) -> Self {
        let capacity_frames = (sample_rate as usize)
            .saturating_mul(SRT_AUDIO_QUEUE_MAX_MS as usize)
            .div_ceil(1_000)
            .max(1);
        Self::with_capacity_frames(sample_rate, capacity_frames)
    }

    fn with_capacity_frames(sample_rate: u32, capacity_frames: usize) -> Self {
        Self {
            packets: VecDeque::new(),
            queued_frames: 0,
            capacity_frames: capacity_frames.max(1),
            sample_rate,
        }
    }

    fn push(&mut self, mut packet: SrtAudioPacket) -> Result<usize, String> {
        if packet.channels != 2
            || packet.sample_rate == 0
            || packet.sample_rate != self.sample_rate
            || packet.samples.len() % 2 != 0
        {
            return Err("Invalid SRT program-audio block".into());
        }
        let frames = packet.samples.len() / 2;
        if frames == 0 {
            return Ok(0);
        }

        let mut dropped_frames = 0;
        let retained_frames = frames.min(self.capacity_frames);
        if frames > retained_frames {
            let trim_frames = frames - retained_frames;
            packet.samples.drain(..trim_frames * 2);
            dropped_frames += trim_frames;
        }

        let mut remaining_to_drop = self
            .queued_frames
            .saturating_add(retained_frames)
            .saturating_sub(self.capacity_frames);
        while remaining_to_drop > 0 {
            let Some(oldest_frames) = self.packets.front().map(|p| p.samples.len() / 2) else {
                break;
            };
            if oldest_frames <= remaining_to_drop {
                self.packets.pop_front();
                self.queued_frames = self.queued_frames.saturating_sub(oldest_frames);
                remaining_to_drop -= oldest_frames;
                dropped_frames += oldest_frames;
            } else {
                let trim_samples = remaining_to_drop * 2;
                self.packets
                    .front_mut()
                    .expect("front packet was checked")
                    .samples
                    .drain(..trim_samples);
                self.queued_frames = self.queued_frames.saturating_sub(remaining_to_drop);
                dropped_frames += remaining_to_drop;
                remaining_to_drop = 0;
            }
        }

        self.queued_frames += retained_frames;
        self.packets.push_back(packet);
        Ok(dropped_frames)
    }

    fn pop(&mut self) -> Option<SrtAudioPacket> {
        let packet = self.packets.pop_front()?;
        self.queued_frames = self.queued_frames.saturating_sub(packet.samples.len() / 2);
        Some(packet)
    }

    fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
}

fn spawn_srt_audio_writer(
    mut input: SrtPipeWriter,
    queue: Arc<(Mutex<SrtAudioQueue>, Condvar)>,
    running: Arc<AtomicBool>,
    state: Arc<Mutex<SrtWriterState>>,
    diagnostic: Arc<Mutex<FfmpegDiagnostics>>,
) -> ManagedThread {
    ManagedThread::spawn("qlisa-srt-ffmpeg-audio".into(), move || {
        if let Err(error) = input.connect() {
            set_srt_writer_error(
                &state,
                format!("Could not connect FFmpeg audio pipe: {error}"),
            );
            return;
        }
        while running.load(Ordering::Relaxed) {
            let packet = {
                let (lock, signal) = queue.as_ref();
                let mut pending = match lock.lock() {
                    Ok(value) => value,
                    Err(_) => {
                        set_srt_writer_error(&state, "SRT audio queue lock poisoned".into());
                        return;
                    }
                };
                while pending.is_empty() && running.load(Ordering::Relaxed) {
                    pending = signal
                        .wait(pending)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                if !running.load(Ordering::Relaxed) {
                    return;
                }
                pending.pop()
            };
            let Some(packet) = packet else { continue };
            if packet.channels != 2 || packet.sample_rate == 0 || packet.samples.len() % 2 != 0 {
                set_srt_writer_error(&state, "Invalid SRT program-audio block".into());
                return;
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    packet.samples.as_ptr() as *const u8,
                    std::mem::size_of_val(packet.samples.as_slice()),
                )
            };
            if let Err(error) = input.write_all(bytes) {
                let detail = diagnostic
                    .lock()
                    .ok()
                    .map(|value| value.snapshot())
                    .unwrap_or_default();
                let summary = format!("Could not write audio to FFmpeg SRT sender: {error}");
                set_srt_writer_error(
                    &state,
                    if detail.is_empty() {
                        summary
                    } else {
                        format!("{summary}: {detail}")
                    },
                );
                return;
            }
        }
    })
    .expect("spawn FFmpeg audio writer")
}

/// A real SRT program sender backed by the self-contained FFmpeg binary.
///
/// Call this from a dedicated network worker, never the GL render callback:
/// operating-system pipe backpressure can block `write_all`.  The sender owns
/// the child and terminates it on drop.  It has no network fallback; a process
/// failure is returned to the caller for an explicit per-output error status.
pub struct SrtFfmpegSender {
    child: ManagedFfmpegChild,
    width: u32,
    height: u32,
    audio_sample_rate: u32,
    latest: Arc<(Mutex<Option<BgraFrame>>, Condvar)>,
    audio_queue: Arc<(Mutex<SrtAudioQueue>, Condvar)>,
    running: Arc<AtomicBool>,
    writer_state: Arc<Mutex<SrtWriterState>>,
    diagnostic: Arc<Mutex<FfmpegDiagnostics>>,
    #[cfg(windows)]
    pipe_cancellation: Arc<WindowsPipeCancellation>,
    // Retained until a bounded cancellation confirms the pipe readers/writers
    // are gone. In particular, Windows `ConnectNamedPipe` must never outlive a
    // failed child or a retry attempt.
    writer: Option<ManagedThread>,
    audio_writer: Option<ManagedThread>,
    stderr_drain: Option<ManagedThread>,
}

impl SrtFfmpegSender {
    /// Start H.264/MPEG-TS over the supplied SRT endpoint.  `fps` is the
    /// program cadence and must be a positive integer.
    pub fn start(
        settings: &SrtSettings,
        width: u32,
        height: u32,
        fps: u32,
        audio_sample_rate: u32,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 || fps == 0 || audio_sample_rate == 0 {
            return Err("SRT program dimensions and frame rate must be non-zero".into());
        }
        let url = settings.url()?;
        let ffmpeg = find_ffmpeg_runtime()
            .ok_or_else(|| "Bundled FFmpeg with SRT was not found".to_string())?;
        let size = format!("{width}x{height}");
        let mut command = Command::new(&ffmpeg);
        // FFmpeg is a background transport worker. On Windows the ordinary
        // process default would create a visible console for every SRT output.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let codec = match settings.codec.as_deref().map(str::trim) {
            Some(value)
                if value.eq_ignore_ascii_case("hevc") || value.eq_ignore_ascii_case("h265") =>
            {
                "libx265"
            }
            _ => "libx264",
        };
        let bitrate = settings.bitrate_kbps.map(|rate| format!("{rate}k"));
        let scale = match (settings.width, settings.height) {
            (Some(target_width), Some(target_height))
                if target_width != width || target_height != height =>
            {
                Some(format!(
                    "scale={target_width}:{target_height}:flags=lanczos"
                ))
            }
            _ => None,
        };
        command.args(["-hide_banner", "-loglevel", "warning", "-nostdin"]);
        #[cfg(windows)]
        let pipe_cancellation = WindowsPipeCancellation::new()?;
        #[cfg(windows)]
        let (video_input, audio_input) = (
            WindowsPipeServer::create(
                "video",
                WindowsPipeDirection::Outbound,
                Arc::clone(&pipe_cancellation),
            )?,
            WindowsPipeServer::create(
                "audio",
                WindowsPipeDirection::Outbound,
                Arc::clone(&pipe_cancellation),
            )?,
        );
        #[cfg(windows)]
        command.args([
            "-f",
            "rawvideo",
            "-pixel_format",
            "bgra",
            "-video_size",
            &size,
            "-framerate",
            &fps.to_string(),
            "-i",
            &video_input.name,
            "-f",
            "f32le",
            "-ar",
            &audio_sample_rate.to_string(),
            "-ac",
            "2",
            "-i",
            &audio_input.name,
        ]);
        #[cfg(not(windows))]
        command.args([
            "-f",
            "rawvideo",
            "-pixel_format",
            "bgra",
            "-video_size",
            &size,
            "-framerate",
            &fps.to_string(),
            "-i",
            "pipe:0",
        ]);
        command
            .args([
                "-c:v",
                codec,
                "-preset",
                "veryfast",
                "-tune",
                "zerolatency",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
            ])
            .args(
                bitrate
                    .as_deref()
                    .map(|value| ["-b:v", value])
                    .into_iter()
                    .flatten(),
            )
            .args(
                scale
                    .as_deref()
                    .map(|value| ["-vf", value])
                    .into_iter()
                    .flatten(),
            )
            .args(["-f", "mpegts", &url])
            .stdin(if cfg!(windows) {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = spawn_managed_ffmpeg(&mut command).map_err(|e| {
            format!(
                "Could not start bundled FFmpeg at {}: {e}",
                ffmpeg.display()
            )
        })?;
        #[cfg(windows)]
        let video_writer = SrtPipeWriter::Named(video_input);
        #[cfg(windows)]
        let audio_writer = Some(SrtPipeWriter::Named(audio_input));
        #[cfg(not(windows))]
        let video_writer = SrtPipeWriter::Stdin(
            child
                .stdin
                .take()
                .ok_or_else(|| "FFmpeg did not provide a video input pipe".to_string())?,
        );
        #[cfg(not(windows))]
        let audio_writer: Option<SrtPipeWriter> = None;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "FFmpeg did not provide a diagnostic pipe".to_string())?;
        let latest = Arc::new((Mutex::new(None), Condvar::new()));
        let audio_queue = Arc::new((
            Mutex::new(SrtAudioQueue::new(audio_sample_rate)),
            Condvar::new(),
        ));
        let running = Arc::new(AtomicBool::new(true));
        let writer_state = Arc::new(Mutex::new(SrtWriterState {
            stats: SrtSenderStats {
                started_at_unix_ms: unix_ms(),
                ..Default::default()
            },
            last_error: None,
            pending_audio_drop_warning_frames: 0,
            last_audio_drop_warning_at: None,
        }));
        let diagnostic = Arc::new(Mutex::new(FfmpegDiagnostics::default()));
        let stderr_drain = spawn_stderr_drain(stderr, Arc::clone(&diagnostic));
        let writer = spawn_srt_pipe_writer(
            video_writer,
            width,
            height,
            fps,
            Arc::clone(&latest),
            Arc::clone(&running),
            Arc::clone(&writer_state),
            Arc::clone(&diagnostic),
        );
        let audio_writer = audio_writer.map(|input| {
            spawn_srt_audio_writer(
                input,
                Arc::clone(&audio_queue),
                Arc::clone(&running),
                Arc::clone(&writer_state),
                Arc::clone(&diagnostic),
            )
        });
        Ok(Self {
            child,
            width,
            height,
            audio_sample_rate,
            latest,
            audio_queue,
            running,
            writer_state,
            diagnostic,
            #[cfg(windows)]
            pipe_cancellation,
            writer: Some(writer),
            audio_writer,
            stderr_drain: Some(stderr_drain),
        })
    }

    /// Submit one program frame. Frames must match the negotiated size. The
    /// method first observes early FFmpeg exits so a disconnected process is
    /// never mistaken for a running stream.
    pub fn send(&mut self, frame: &BgraFrame) -> Result<(), String> {
        if frame.width != self.width || frame.height != self.height {
            return Err(format!(
                "SRT frame is {}x{} but sender was started at {}x{}",
                frame.width, frame.height, self.width, self.height
            ));
        }
        self.check_health()?;
        frame.validate()?;
        let (lock, signal) = self.latest.as_ref();
        *lock
            .lock()
            .map_err(|_| "SRT frame mailbox lock poisoned".to_string())? = Some(frame.clone());
        signal.notify_one();
        Ok(())
    }

    /// Observe a failed child or a canceled pipe even when the compositor has
    /// not produced another image. This makes retry tear down the old workers
    /// promptly instead of waiting for an unrelated video change.
    pub fn check_health(&mut self) -> Result<(), String> {
        if let Some(error) = self
            .writer_state
            .lock()
            .ok()
            .and_then(|state| state.last_error.clone())
        {
            return Err(error);
        }
        if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
            if let Ok(mut state) = self.writer_state.lock() {
                state.stats.failed_writes += 1;
            }
            let detail = self
                .diagnostic
                .lock()
                .ok()
                .map(|value| value.snapshot())
                .unwrap_or_default();
            let summary = format!("FFmpeg SRT sender exited with {status}");
            return Err(if detail.is_empty() {
                summary
            } else {
                format!("{summary}: {detail}")
            });
        }
        Ok(())
    }

    /// Submit a bounded block from the post-master Qlisa program bus. The
    /// audio writer owns the pipe, so this call only updates its bounded queue
    /// and cannot block the compositor or the audio callback.
    pub fn send_audio_interleaved(
        &mut self,
        sample_rate: u32,
        channels: u32,
        samples: &[f32],
    ) -> Result<(), String> {
        #[cfg(not(windows))]
        {
            let _ = (sample_rate, channels, samples);
            return Err("SRT program audio is currently implemented for Windows builds".into());
        }
        #[cfg(windows)]
        {
            if sample_rate != self.audio_sample_rate || channels != 2 || samples.len() % 2 != 0 {
                return Err("SRT program audio format changed while streaming".into());
            }
            if let Some(error) = self
                .writer_state
                .lock()
                .ok()
                .and_then(|state| state.last_error.clone())
            {
                return Err(error);
            }
            let (lock, signal) = self.audio_queue.as_ref();
            let dropped_frames = lock
                .lock()
                .map_err(|_| "SRT audio queue lock poisoned".to_string())?
                .push(SrtAudioPacket {
                    sample_rate,
                    channels,
                    samples: samples.to_vec(),
                })?;
            report_srt_audio_drops(&self.writer_state, dropped_frames);
            signal.notify_one();
            Ok(())
        }
    }

    pub fn stats(&self) -> SrtSenderStats {
        self.writer_state
            .lock()
            .map(|state| state.stats.clone())
            .unwrap_or_default()
    }

    /// Stop the encoder deterministically before rebuilding/removing an output.
    pub fn stop(mut self) -> Result<(), String> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<(), String> {
        self.running.store(false, Ordering::Relaxed);
        #[cfg(windows)]
        self.pipe_cancellation.cancel();
        self.latest.1.notify_all();
        self.audio_queue.1.notify_all();
        let deadline = Instant::now() + SRT_GRACEFUL_STOP_TIMEOUT;
        while self.child.try_wait().map_err(|e| e.to_string())?.is_none()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        if self.child.try_wait().map_err(|e| e.to_string())?.is_none() {
            // A killed child closes its pipe ends. The persistent event above
            // also wakes any Windows worker that has not reached (or is inside)
            // an overlapped named-pipe operation. Every wait remains bounded.
            if let Err(error) = self.child.kill() {
                if self.child.try_wait().map_err(|e| e.to_string())?.is_none() {
                    return Err(format!("Could not stop FFmpeg SRT sender: {error}"));
                }
            }
        }
        let worker_result = cancel_and_join_workers(&mut [
            &mut self.writer,
            &mut self.audio_writer,
            &mut self.stderr_drain,
        ]);
        let child_deadline = Instant::now() + SRT_WORKER_STOP_TIMEOUT;
        loop {
            if self.child.try_wait().map_err(|e| e.to_string())?.is_some() {
                break;
            }
            if Instant::now() >= child_deadline {
                return Err("FFmpeg SRT sender did not exit after termination".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        worker_result
    }
}

impl Drop for SrtFfmpegSender {
    fn drop(&mut self) {
        if let Err(error) = self.stop_inner() {
            // The endpoint/status path has already been redacted. This log is
            // deliberately equally safe and explains why automatic retry was
            // abandoned rather than silently leaking a blocked pipe worker.
            log::warn!(
                "[srt-output] bounded sender teardown failed: {}",
                sanitized_ffmpeg_diagnostic(&error)
            );
        }
    }
}

/// Upper bounds for a decoded bitmap received from an untrusted network
/// source.  They deliberately mirror the output-side SRT dimensions, while
/// retaining a byte cap so a corrupt BMP header cannot make the reader grow a
/// `Vec` until the machine is out of memory.
const SRT_INPUT_MAX_DIMENSION: u32 = 8_192;
const SRT_INPUT_MAX_BMP_BYTES: usize = 128 * 1024 * 1024;
const BMP_FILE_HEADER_BYTES: usize = 14;
const BMP_INFO_HEADER_BYTES: usize = 40;

/// Parse concatenated BMP frames from FFmpeg's `image2pipe` output.  A BMP
/// has a file-size field in its first header, unlike rawvideo, so raster size
/// changes in an SRT contribution stay framed and safe.
#[derive(Default)]
struct BmpPipeParser {
    pending: Vec<u8>,
}

impl BmpPipeParser {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<BgraFrame>, String> {
        let mut frames = Vec::new();
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let space = SRT_INPUT_MAX_BMP_BYTES
                .checked_sub(self.pending.len())
                .ok_or("SRT BMP pipe frame exceeds the configured safety bound")?;
            if space == 0 {
                return Err("SRT BMP pipe frame exceeds the configured safety bound".into());
            }
            let take = remaining.len().min(space);
            self.pending.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];

            loop {
                let Some(frame_len) = bmp_announced_size(&self.pending)? else {
                    break;
                };
                if self.pending.len() < frame_len {
                    break;
                }
                // Decode directly from the pending pipe buffer, then drain it.
                // Collecting `drain(..frame_len)` first made a third full-size
                // allocation (encoded BMP + copied BMP + BGRA mailbox frame).
                frames.push(parse_bgra_bmp(&self.pending[..frame_len])?);
                self.pending.drain(..frame_len);
            }
        }
        Ok(frames)
    }
}

/// Return a BMP's declared frame length once its file header is complete.
fn bmp_announced_size(bytes: &[u8]) -> Result<Option<usize>, String> {
    if bytes.len() < 2 {
        return Ok(None);
    }
    if &bytes[..2] != b"BM" {
        return Err("FFmpeg SRT video pipe did not begin with a BMP frame".into());
    }
    if bytes.len() < 6 {
        return Ok(None);
    }
    let size = u32::from_le_bytes(bytes[2..6].try_into().expect("slice length checked")) as usize;
    if size < BMP_FILE_HEADER_BYTES + BMP_INFO_HEADER_BYTES {
        return Err("FFmpeg SRT BMP frame has an invalid file size".into());
    }
    if size > SRT_INPUT_MAX_BMP_BYTES {
        return Err("FFmpeg SRT BMP frame exceeds the configured safety bound".into());
    }
    Ok(Some(size))
}

/// Convert one complete 32-bit BI_RGB BMP to the compositor's top-down BGRA
/// frame.  BMP scanlines can be bottom-up and may contain row padding, so the
/// output is copied into a tightly packed, independently owned buffer.
fn parse_bgra_bmp(bytes: &[u8]) -> Result<BgraFrame, String> {
    let announced =
        bmp_announced_size(bytes)?.ok_or("FFmpeg SRT BMP frame header is incomplete")?;
    if announced != bytes.len() {
        return Err("FFmpeg SRT BMP frame length does not match its header".into());
    }
    if bytes.len() < BMP_FILE_HEADER_BYTES + BMP_INFO_HEADER_BYTES {
        return Err("FFmpeg SRT BMP frame is shorter than its headers".into());
    }
    let read_u16 = |at: usize| -> u16 {
        u16::from_le_bytes(bytes[at..at + 2].try_into().expect("BMP bounds checked"))
    };
    let read_u32 = |at: usize| -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().expect("BMP bounds checked"))
    };
    let dib_size = read_u32(14) as usize;
    if dib_size < BMP_INFO_HEADER_BYTES
        || BMP_FILE_HEADER_BYTES
            .checked_add(dib_size)
            .ok_or("SRT BMP DIB size overflow")?
            > bytes.len()
    {
        return Err("FFmpeg SRT BMP has an unsupported DIB header".into());
    }
    let pixel_offset = read_u32(10) as usize;
    let width_i = i32::from_le_bytes(bytes[18..22].try_into().expect("BMP bounds checked"));
    let height_i = i32::from_le_bytes(bytes[22..26].try_into().expect("BMP bounds checked"));
    if width_i <= 0 || height_i == 0 || height_i == i32::MIN {
        return Err("FFmpeg SRT BMP has invalid dimensions".into());
    }
    let width = width_i as u32;
    let height = height_i.unsigned_abs();
    if width > SRT_INPUT_MAX_DIMENSION || height > SRT_INPUT_MAX_DIMENSION {
        return Err("FFmpeg SRT BMP dimensions exceed the configured safety bound".into());
    }
    if read_u16(26) != 1 || read_u16(28) != 32 || read_u32(30) != 0 {
        return Err("FFmpeg SRT BMP must be uncompressed 32-bit BGRA".into());
    }
    let row_bytes = (width as usize)
        .checked_mul(4)
        .ok_or("SRT BMP row size overflow")?;
    // 32-bit pixels have no additional BMP row padding, but retain the
    // formula so this invariant is clear and remains correct if validation is
    // widened later.
    let source_stride = row_bytes.checked_add(3).ok_or("SRT BMP stride overflow")? & !3;
    let image_bytes = source_stride
        .checked_mul(height as usize)
        .ok_or("SRT BMP image size overflow")?;
    let image_end = pixel_offset
        .checked_add(image_bytes)
        .ok_or("SRT BMP pixel offset overflow")?;
    if pixel_offset < BMP_FILE_HEADER_BYTES + dib_size || image_end > bytes.len() {
        return Err("FFmpeg SRT BMP pixel data is outside the frame".into());
    }
    let output_len = row_bytes
        .checked_mul(height as usize)
        .ok_or("SRT BMP output size overflow")?;
    let mut data = vec![0_u8; output_len];
    for destination_row in 0..height as usize {
        let source_row = if height_i > 0 {
            height as usize - 1 - destination_row
        } else {
            destination_row
        };
        let source_start = pixel_offset + source_row * source_stride;
        let destination_start = destination_row * row_bytes;
        data[destination_start..destination_start + row_bytes]
            .copy_from_slice(&bytes[source_start..source_start + row_bytes]);
    }
    Ok(BgraFrame {
        width,
        height,
        stride: row_bytes as u32,
        data,
    })
}

/// A redacted snapshot of a running SRT camera contribution.  It intentionally
/// contains no endpoint, stream id, or passphrase; FFmpeg diagnostics pass
/// through [`redact_srt_diagnostic`] before reaching this value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SrtInputRuntimeStatus {
    pub receiving_video: bool,
    pub receiving_audio: bool,
    pub received_frames: u64,
    pub received_audio_samples: u64,
    pub dropped_audio_samples: u64,
    /// `true` for a real audio EOF and also when FFmpeg's optional tee audio
    /// output was not created because the source did not advertise audio.
    pub audio_eof: bool,
    pub video_eof: bool,
    pub child_exit: Option<String>,
    pub last_error: Option<String>,
    pub diagnostics: Option<String>,
}

/// Read-only diagnostics for one managed network input. This is assembled
/// from the existing receiver status and deliberately contains no credentials
/// or raw SRT URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkInputRuntimeDiagnostics {
    pub protocol: NetworkProtocol,
    pub source_name: String,
    pub endpoint: Option<String>,
    pub srt_mode: Option<SrtMode>,
    pub latency_ms: Option<u32>,
    pub receiving: bool,
    pub received_frames: u64,
    pub dropped_frames: Option<u64>,
    pub superseded_frames: Option<u64>,
    pub received_audio_samples: Option<u64>,
    pub dropped_audio_samples: Option<u64>,
    pub video_eof: bool,
    pub audio_eof: bool,
    pub ffmpeg_running: Option<bool>,
    pub ffmpeg_pid: Option<u32>,
    pub last_error: Option<String>,
    /// Redacted FFmpeg stderr retained as a non-fatal operator warning.
    pub last_warning: Option<String>,
}

impl SrtInputRuntimeStatus {
    /// `true` once both decode feeds have ended or failed. A missing audio map
    /// alone is not terminal while video is still receiving.
    pub fn feeds_unavailable(&self) -> bool {
        !self.receiving_video
            && !self.receiving_audio
            && (self.video_eof || self.audio_eof || self.last_error.is_some())
    }

    /// A redacted terminal reason suitable for a cue/status surface. This is
    /// intentionally a status primitive rather than forcing callers to infer
    /// failure from concrete child/thread state.
    pub fn terminal_diagnostic(&self) -> Option<String> {
        if let Some(exit) = &self.child_exit {
            let detail = self
                .last_error
                .as_deref()
                .or(self.diagnostics.as_deref())
                .unwrap_or_default();
            let summary = format!("FFmpeg SRT input exited with {exit}");
            let message = if detail.is_empty() {
                summary
            } else {
                format!("{summary}: {detail}")
            };
            return Some(sanitized_ffmpeg_diagnostic(&message));
        }
        // Video is the required half of a Camera Cue.  Audio is optional, so
        // its EOF/error must remain a status warning while video keeps
        // receiving, but a video EOF cannot leave a frozen camera layer up
        // merely because the audio tee is still draining.
        if self.video_eof && !self.receiving_video {
            return Some(sanitized_ffmpeg_diagnostic(
                self.last_error
                    .as_deref()
                    .or(self.diagnostics.as_deref())
                    .unwrap_or("FFmpeg SRT video stream ended"),
            ));
        }
        if self.feeds_unavailable() {
            return Some(sanitized_ffmpeg_diagnostic(
                self.last_error
                    .as_deref()
                    .or(self.diagnostics.as_deref())
                    .unwrap_or("FFmpeg SRT input has no active video or audio feed"),
            ));
        }
        None
    }
}

fn set_srt_input_error(status: &Arc<Mutex<SrtInputRuntimeStatus>>, error: impl AsRef<str>) {
    if let Ok(mut status) = status.lock() {
        status.last_error = Some(sanitized_ffmpeg_diagnostic(error.as_ref()));
    }
}

fn ffmpeg_reports_no_audio(diagnostic: &str) -> bool {
    // The tee muxer keeps its video slave running when the optional audio map
    // has no stream. FFmpeg reports that precise state before it declines the
    // audio-only slave. Treat this as a non-fatal audio EOF, not a video error.
    let diagnostic = diagnostic.to_ascii_lowercase();
    diagnostic.contains("no streams to mux were specified")
        && diagnostic.contains("slave")
        && diagnostic.contains("continuing")
}

fn srt_input_ffmpeg_args(
    url: &str,
    target_sample_rate: u32,
    video_target: &str,
    audio_target: &str,
) -> Vec<String> {
    // `tee` is crucial here. Separate FFmpeg output files make an optional
    // `-map 0:a:0?` fatal when no audio track exists. A single tee output has
    // a valid video stream; its audio-only slave uses `onfail=ignore`, leaving
    // the video pipe alive when the source is video-only.
    let video_target = ffmpeg_tee_escape_target(video_target);
    let audio_target = ffmpeg_tee_escape_target(audio_target);
    let tee_target = format!(
        "[select=v:f=image2pipe]{video_target}|[onfail=ignore:select=a:f=f32le]{audio_target}"
    );
    // Preserve the source audio clock while converting into the engine's fixed
    // sample rate. `async` performs bounded timestamp compensation (including
    // silence insertion/cutting), and `first_pts=0` gives every worker/restart
    // a deterministic PCM epoch even though the f32le pipe itself has no PTS.
    let audio_resample = format!("aresample={target_sample_rate}:async=1000:first_pts=0");
    [
        "-hide_banner".into(),
        "-loglevel".into(),
        "warning".into(),
        "-nostdin".into(),
        "-i".into(),
        url.into(),
        "-map".into(),
        "0:v:0?".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-c:v".into(),
        "bmp".into(),
        "-pix_fmt".into(),
        "bgra".into(),
        "-c:a".into(),
        "pcm_f32le".into(),
        "-af".into(),
        audio_resample,
        "-ac".into(),
        "2".into(),
        "-ar".into(),
        target_sample_rate.to_string(),
        "-f".into(),
        "tee".into(),
        tee_target,
    ]
    .to_vec()
}

/// `tee` parses its output-slave value once more after `Command` has already
/// passed the argument verbatim to FFmpeg. Windows named-pipe paths therefore
/// need their backslashes doubled here: otherwise `\\.\pipe\name` becomes
/// `\.pipename` and FFmpeg fails to open the video slave with Permission
/// denied. Escape the other tee separators too so this stays safe for future
/// output path variants.
fn ffmpeg_tee_escape_target(target: &str) -> String {
    target
        .replace('\\', r"\\")
        .replace(':', r"\:")
        .replace('|', r"\|")
}

#[cfg(windows)]
fn terminate_srt_input_child(
    child: &Arc<Mutex<Option<ManagedFfmpegChild>>>,
    status: &Arc<Mutex<SrtInputRuntimeStatus>>,
) -> Result<(), String> {
    let mut guard = child
        .lock()
        .map_err(|_| "SRT input child lock poisoned".to_string())?;
    let Some(process) = guard.as_mut() else {
        return Ok(());
    };
    let exit = match process.try_wait().map_err(|error| error.to_string())? {
        Some(exit) => exit,
        None => {
            if let Err(error) = process.kill() {
                // `kill` can race a natural exit. `wait` is still required to
                // reap that child; retaining the handle is safer than letting
                // Child::drop detach an unobserved process.
                log::debug!("[srt-input] FFmpeg kill raced/failed during reap: {error}");
            }
            process
                .wait()
                .map_err(|error| format!("Could not reap FFmpeg SRT input: {error}"))?
        }
    };
    if let Ok(mut status) = status.lock() {
        status.receiving_video = false;
        status.receiving_audio = false;
        status.child_exit = Some(exit.to_string());
    }
    *guard = None;
    Ok(())
}

#[cfg(windows)]
fn cleanup_failed_srt_input_start(
    running: &Arc<AtomicBool>,
    cancellation: &WindowsPipeCancellation,
    child: &Arc<Mutex<Option<ManagedFfmpegChild>>>,
    status: &Arc<Mutex<SrtInputRuntimeStatus>>,
    video_reader: &mut Option<ManagedThread>,
    audio_reader: &mut Option<ManagedThread>,
    stderr_drain: &mut Option<ManagedThread>,
) {
    running.store(false, Ordering::Release);
    cancellation.cancel();
    let child_result = terminate_srt_input_child(child, status);
    let worker_result = cancel_and_join_workers(&mut [video_reader, audio_reader, stderr_drain]);
    if let Err(error) = child_result {
        set_srt_input_error(status, format!("SRT input start cleanup failed: {error}"));
        log::warn!("[srt-input] start cleanup child failure: {error}");
    }
    if let Err(error) = worker_result {
        set_srt_input_error(status, format!("SRT input start cleanup failed: {error}"));
        log::warn!("[srt-input] start cleanup worker failure: {error}");
    }
}

#[cfg(windows)]
fn spawn_srt_input_video_reader(
    mut reader: SrtPipeReader,
    mailbox: Arc<BgraFrameMailbox>,
    running: Arc<AtomicBool>,
    status: Arc<Mutex<SrtInputRuntimeStatus>>,
    child: Arc<Mutex<Option<ManagedFfmpegChild>>>,
    cancellation: Arc<WindowsPipeCancellation>,
) -> Result<ManagedThread, String> {
    ManagedThread::spawn("qlisa-srt-input-video".into(), move || {
        let mut parser = BmpPipeParser::default();
        let mut buffer = [0_u8; 16 * 1024];
        while running.load(Ordering::Relaxed) {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    if let Ok(mut state) = status.lock() {
                        state.receiving_video = false;
                        state.video_eof = true;
                    }
                    break;
                }
                Ok(count) => match parser.push(&buffer[..count]) {
                    Ok(frames) => {
                        if !frames.is_empty() {
                            let frame_count = frames.len() as u64;
                            for frame in frames {
                                let _ = mailbox.publish(frame);
                            }
                            if let Ok(mut state) = status.lock() {
                                state.receiving_video = true;
                                state.video_eof = false;
                                state.received_frames =
                                    state.received_frames.saturating_add(frame_count);
                            }
                        }
                    }
                    Err(error) => {
                        running.store(false, Ordering::Release);
                        cancellation.cancel();
                        if let Ok(mut child) = child.lock() {
                            if let Some(child) = child.as_mut() {
                                let _ = child.kill();
                            }
                        }
                        set_srt_input_error(
                            &status,
                            format!("Could not parse FFmpeg SRT video frame: {error}"),
                        );
                        if let Ok(mut state) = status.lock() {
                            state.receiving_video = false;
                            state.video_eof = true;
                        }
                        break;
                    }
                },
                Err(error) => {
                    if running.load(Ordering::Relaxed) {
                        set_srt_input_error(
                            &status,
                            format!("Could not read FFmpeg SRT video pipe: {error}"),
                        );
                    }
                    if let Ok(mut state) = status.lock() {
                        state.receiving_video = false;
                        state.video_eof = true;
                    }
                    break;
                }
            }
        }
    })
    .map_err(|error| format!("Could not start FFmpeg SRT video pipe reader: {error}"))
}

#[cfg(windows)]
fn push_srt_stereo_sample(
    producer: &mut SyntheticFeedProducer,
    pending_left: &mut Option<f32>,
    sample: f32,
) -> u64 {
    let Some(left) = pending_left.take() else {
        *pending_left = Some(sample);
        return 0;
    };
    producer.try_push_frame(&[left, sample]).err().unwrap_or(0) as u64
}

#[cfg(windows)]
fn spawn_srt_input_audio_reader(
    mut reader: SrtPipeReader,
    running: Arc<AtomicBool>,
    status: Arc<Mutex<SrtInputRuntimeStatus>>,
    mut producer: SyntheticFeedProducer,
) -> Result<ManagedThread, String> {
    ManagedThread::spawn("qlisa-srt-input-audio".into(), move || {
        let mut trailing = Vec::with_capacity(3);
        let mut pending_left = None;
        let mut buffer = [0_u8; 16 * 1024];
        while running.load(Ordering::Relaxed) {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    if let Ok(mut state) = status.lock() {
                        state.receiving_audio = false;
                        state.audio_eof = true;
                    }
                    break;
                }
                Ok(count) => {
                    let mut decoded = 0_u64;
                    let mut dropped = 0_u64;
                    let mut offset = 0;
                    if !trailing.is_empty() {
                        let needed = 4 - trailing.len();
                        let take = needed.min(count);
                        trailing.extend_from_slice(&buffer[..take]);
                        offset += take;
                        if trailing.len() == 4 {
                            let sample =
                                f32::from_le_bytes(trailing[..4].try_into().expect("four bytes"));
                            dropped +=
                                push_srt_stereo_sample(&mut producer, &mut pending_left, sample);
                            decoded += 1;
                            trailing.clear();
                        }
                    }
                    while offset + 4 <= count {
                        let sample = f32::from_le_bytes(
                            buffer[offset..offset + 4].try_into().expect("four bytes"),
                        );
                        dropped += push_srt_stereo_sample(&mut producer, &mut pending_left, sample);
                        decoded += 1;
                        offset += 4;
                    }
                    trailing.extend_from_slice(&buffer[offset..count]);
                    if decoded > 0 {
                        if let Ok(mut state) = status.lock() {
                            state.receiving_audio = true;
                            state.audio_eof = false;
                            state.received_audio_samples =
                                state.received_audio_samples.saturating_add(decoded);
                            state.dropped_audio_samples =
                                state.dropped_audio_samples.saturating_add(dropped);
                        }
                    }
                }
                Err(error) => {
                    if running.load(Ordering::Relaxed) {
                        set_srt_input_error(
                            &status,
                            format!("Could not read FFmpeg SRT audio pipe: {error}"),
                        );
                    }
                    if let Ok(mut state) = status.lock() {
                        state.receiving_audio = false;
                        state.audio_eof = true;
                    }
                    break;
                }
            }
        }
    })
    .map_err(|error| format!("Could not start FFmpeg SRT audio pipe reader: {error}"))
}

/// Unified live SRT receiver. One hidden FFmpeg process opens one SRT URL,
/// demuxes both streams, and publishes BGRA video plus synthetic-feed audio.
/// The reader threads own no process handles and are cancelled/joined before
/// the worker is dropped, preventing retry/reconfigure from leaving a pipe or
/// FFmpeg child behind.
pub struct SrtInputWorker {
    mailbox: Arc<BgraFrameMailbox>,
    running: Arc<AtomicBool>,
    status: Arc<Mutex<SrtInputRuntimeStatus>>,
    #[cfg(windows)]
    child: Arc<Mutex<Option<ManagedFfmpegChild>>>,
    #[cfg(windows)]
    diagnostic: Arc<Mutex<FfmpegDiagnostics>>,
    #[cfg(windows)]
    video_reader: Mutex<Option<ManagedThread>>,
    #[cfg(windows)]
    audio_reader: Mutex<Option<ManagedThread>>,
    #[cfg(windows)]
    stderr_drain: Mutex<Option<ManagedThread>>,
    #[cfg(windows)]
    pipe_cancellation: Arc<WindowsPipeCancellation>,
}

impl SrtInputWorker {
    #[cfg(windows)]
    pub fn start(
        settings: &SrtSettings,
        target_sample_rate: u32,
        producer: SyntheticFeedProducer,
    ) -> Result<Self, String> {
        if target_sample_rate == 0 {
            return Err("SRT audio target sample rate must be non-zero".into());
        }
        let ffmpeg = find_ffmpeg_runtime()
            .ok_or_else(|| "Bundled FFmpeg with SRT was not found".to_string())?;
        let url = settings.url()?;
        let pipe_cancellation = WindowsPipeCancellation::new()?;
        let video_output = WindowsPipeServer::create(
            "input-video",
            WindowsPipeDirection::Inbound,
            Arc::clone(&pipe_cancellation),
        )?;
        let audio_output = WindowsPipeServer::create(
            "input-audio",
            WindowsPipeDirection::Inbound,
            Arc::clone(&pipe_cancellation),
        )?;
        let args = srt_input_ffmpeg_args(
            &url,
            target_sample_rate,
            &video_output.name,
            &audio_output.name,
        );
        let mut command = Command::new(&ffmpeg);
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command
            .creation_flags(CREATE_NO_WINDOW)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let spawned = spawn_managed_ffmpeg(&mut command).map_err(|error| {
            format!(
                "Could not start bundled FFmpeg SRT input at {}: {error}",
                ffmpeg.display()
            )
        })?;
        let mailbox = Arc::new(BgraFrameMailbox::default());
        let running = Arc::new(AtomicBool::new(true));
        let status = Arc::new(Mutex::new(SrtInputRuntimeStatus::default()));
        let child = Arc::new(Mutex::new(Some(spawned)));
        let diagnostic = Arc::new(Mutex::new(FfmpegDiagnostics::default()));
        let mut video_reader = None;
        let mut audio_reader = None;
        let mut stderr_drain = None;
        let start_result = (|| -> Result<(), String> {
            let stderr = child
                .lock()
                .map_err(|_| "SRT input child lock poisoned".to_string())?
                .as_mut()
                .and_then(|process| process.stderr.take())
                .ok_or_else(|| "FFmpeg did not provide an SRT input diagnostic pipe".to_string())?;
            stderr_drain = Some(try_spawn_stderr_drain(stderr, Arc::clone(&diagnostic))?);
            video_reader = Some(spawn_srt_input_video_reader(
                SrtPipeReader::Named(video_output),
                Arc::clone(&mailbox),
                Arc::clone(&running),
                Arc::clone(&status),
                Arc::clone(&child),
                Arc::clone(&pipe_cancellation),
            )?);
            audio_reader = Some(spawn_srt_input_audio_reader(
                SrtPipeReader::Named(audio_output),
                Arc::clone(&running),
                Arc::clone(&status),
                producer,
            )?);
            Ok(())
        })();
        if let Err(error) = start_result {
            cleanup_failed_srt_input_start(
                &running,
                &pipe_cancellation,
                &child,
                &status,
                &mut video_reader,
                &mut audio_reader,
                &mut stderr_drain,
            );
            return Err(error);
        }
        Ok(Self {
            mailbox,
            running,
            status,
            child,
            diagnostic,
            video_reader: Mutex::new(video_reader),
            audio_reader: Mutex::new(audio_reader),
            stderr_drain: Mutex::new(stderr_drain),
            pipe_cancellation,
        })
    }

    #[cfg(not(windows))]
    pub fn start(
        _settings: &SrtSettings,
        _target_sample_rate: u32,
        _producer: SyntheticFeedProducer,
    ) -> Result<Self, String> {
        Err("Unified SRT camera input currently requires the Windows named-pipe runtime".into())
    }

    pub fn mailbox(&self) -> Arc<BgraFrameMailbox> {
        Arc::clone(&self.mailbox)
    }

    pub fn status(&self) -> SrtInputRuntimeStatus {
        #[cfg(windows)]
        {
            let diagnostic = self
                .diagnostic
                .lock()
                .ok()
                .map(|value| value.snapshot())
                .unwrap_or_default();
            let child_exit = self.child.lock().ok().and_then(|mut child| {
                child
                    .as_mut()
                    .and_then(|child| child.try_wait().ok().flatten())
            });
            if let Ok(mut status) = self.status.lock() {
                if let Some(exit) = child_exit {
                    if !exit.success() && status.last_error.is_none() {
                        status.last_error = Some(format!("FFmpeg SRT input exited with {exit}"));
                    }
                    status.receiving_video = false;
                    status.receiving_audio = false;
                    status.child_exit = Some(exit.to_string());
                }
                status.diagnostics = (!diagnostic.is_empty()).then_some(diagnostic.clone());
                if !status.receiving_audio && ffmpeg_reports_no_audio(&diagnostic) {
                    status.audio_eof = true;
                }
                return status.clone();
            }
        }
        self.status
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| SrtInputRuntimeStatus {
                last_error: Some("SRT input status lock poisoned".into()),
                ..Default::default()
            })
    }

    /// Return process state already owned by this worker. The probe is a
    /// non-blocking `try_wait`; it never starts, stops, or waits for FFmpeg.
    #[cfg(windows)]
    pub fn process_diagnostics(&self) -> (Option<bool>, Option<u32>) {
        let Ok(mut child) = self.child.lock() else {
            return (None, None);
        };
        let pid = child.as_ref().map(|process| process.id());
        let running = child
            .as_mut()
            .map(|process| process.try_wait().ok().flatten().is_none());
        (running, pid)
    }

    #[cfg(not(windows))]
    pub fn process_diagnostics(&self) -> (Option<bool>, Option<u32>) {
        (None, None)
    }

    pub fn diagnostics(&self, settings: &SrtSettings) -> NetworkInputRuntimeDiagnostics {
        let status = self.status();
        let (ffmpeg_running, ffmpeg_pid) = self.process_diagnostics();
        NetworkInputRuntimeDiagnostics {
            protocol: NetworkProtocol::Srt,
            source_name: "SRT".into(),
            endpoint: Some(format!("srt://{}:{}", srt_host(settings), settings.port)),
            srt_mode: Some(settings.mode),
            latency_ms: Some(settings.latency_ms),
            receiving: status.receiving_video || status.receiving_audio,
            received_frames: status.received_frames,
            dropped_frames: None,
            superseded_frames: None,
            received_audio_samples: Some(status.received_audio_samples),
            dropped_audio_samples: Some(status.dropped_audio_samples),
            video_eof: status.video_eof,
            audio_eof: status.audio_eof,
            ffmpeg_running,
            ffmpeg_pid,
            last_error: status.last_error,
            last_warning: status.diagnostics,
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
        #[cfg(windows)]
        {
            self.pipe_cancellation.cancel();
            let child_result = terminate_srt_input_child(&self.child, &self.status);
            if let (Ok(mut video), Ok(mut audio), Ok(mut stderr)) = (
                self.video_reader.lock(),
                self.audio_reader.lock(),
                self.stderr_drain.lock(),
            ) {
                if let Err(error) =
                    cancel_and_join_workers(&mut [&mut *video, &mut *audio, &mut *stderr])
                {
                    set_srt_input_error(
                        &self.status,
                        format!("SRT input teardown failed: {error}"),
                    );
                    log::warn!("[srt-input] bounded teardown failed: {error}");
                }
            }
            if let Err(error) = child_result {
                // Keep the child handle in `self.child` on an OS reap error.
                // Drop calls `stop` again, and we never replace it with an
                // unowned/detached process handle.
                set_srt_input_error(
                    &self.status,
                    format!("SRT input child teardown failed: {error}"),
                );
                log::warn!("[srt-input] child teardown failed: {error}");
            }
        }
    }
}

impl Drop for SrtInputWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Locate the self-contained SRT-capable FFmpeg binary before accepting a
/// system override. This mirrors the NDI lookup and makes another PC's
/// installer independent of PATH.
pub fn find_ffmpeg_runtime() -> Option<PathBuf> {
    let executable = if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("resources").join("ffmpeg").join(executable));
            candidates.push(dir.join(executable));
        }
    }
    if let Ok(override_path) = std::env::var("QLISA_FFMPEG_PATH") {
        candidates.push(PathBuf::from(override_path));
    }
    // Makes `cargo test`/development use the checked-in runtime without
    // changing the packaged application's bundle-first resolution.
    if cfg!(debug_assertions) {
        if let Some(manifest) = option_env!("CARGO_MANIFEST_DIR") {
            candidates.push(
                PathBuf::from(manifest)
                    .join("vendor")
                    .join("ffmpeg")
                    .join(executable),
            );
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// A running state reported for one network destination.  This is deliberately
/// about the actual worker, not merely whether preferences have `enabled=true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkOutputState {
    Disabled,
    WaitingForFrame,
    Streaming,
    Error,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkOutputRuntimeStatus {
    pub output_id: String,
    pub state: NetworkOutputState,
    pub submitted_frames: u64,
    pub superseded_frames: u64,
    pub last_error: Option<String>,
}

/// Read-only diagnostics for one configured network destination. The output
/// worker status remains the source of truth; config is only copied here to
/// label the transport and safe endpoint details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkOutputDiagnostics {
    pub output_id: String,
    pub protocols: Vec<NetworkProtocol>,
    pub state: NetworkOutputState,
    pub endpoint: Option<String>,
    pub srt_mode: Option<SrtMode>,
    pub latency_ms: Option<u32>,
    pub submitted_frames: u64,
    pub superseded_frames: u64,
    pub last_error: Option<String>,
}

impl NetworkOutputRuntimeStatus {
    fn waiting(id: String) -> Self {
        Self {
            output_id: id,
            state: NetworkOutputState::WaitingForFrame,
            submitted_frames: 0,
            superseded_frames: 0,
            last_error: None,
        }
    }
}

/// Network-only part of an output destination.  The preferences/UI layer maps
/// its `OutputDestination` to this isolated type, keeping the worker usable by
/// headless integration tests and an eventual off-screen compositor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkOutputConfig {
    pub id: String,
    pub ndi: Option<NdiOutputSettings>,
    pub srt: Option<SrtSettings>,
}

impl NetworkOutputConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("Network output id is required".into());
        }
        if self.ndi.is_none() && self.srt.is_none() {
            return Err("Network output needs an NDI or SRT transport".into());
        }
        if let Some(ndi) = &self.ndi {
            if !ndi.enabled {
                return Err("NDI network output is disabled".into());
            }
            if ndi.stream_name.trim().is_empty() {
                return Err("NDI stream name is required".into());
            }
        }
        if let Some(srt) = &self.srt {
            if !srt.enabled {
                return Err("SRT network output is disabled".into());
            }
            srt.validate()?;
        }
        Ok(())
    }
}

/// Owns per-destination bounded workers. `publish` replaces an unsent frame
/// instead of queueing it, so a slow network can never build latency or block
/// the render producer on pipe/NDI I/O.
pub struct NetworkOutputManager {
    endpoints: Mutex<std::collections::HashMap<String, Arc<NetworkOutputEndpoint>>>,
    /// Serialises endpoint transitions without blocking the render publisher;
    /// `publish` only takes the short `endpoints` map lock.
    configure_gate: Mutex<()>,
}

/// Keep the number of render copies, network threads, and real-time audio
/// fan-outs finite. This is intentionally a hard error instead of silently
/// creating a destination which cannot receive program audio.
const MAX_NETWORK_OUTPUT_DESTINATIONS: usize = 8;
const NETWORK_AUDIO_DRAIN_INTERVAL: Duration = Duration::from_millis(10);

/// Cloneable, destination-scoped producer handle used by the OpenGL
/// compositor.  Its `publish` method only replaces a value in a bounded slot;
/// it never opens sockets or writes to an encoder pipe on the render thread.
#[derive(Clone)]
pub struct NetworkFrameSink {
    output_id: String,
    manager: Arc<NetworkOutputManager>,
}

impl NetworkFrameSink {
    pub fn publish(&self, frame: BgraFrame) {
        self.manager.publish(&self.output_id, frame);
    }

    pub fn output_id(&self) -> &str {
        &self.output_id
    }
}

impl Default for NetworkOutputManager {
    fn default() -> Self {
        Self {
            endpoints: Mutex::new(std::collections::HashMap::new()),
            configure_gate: Mutex::new(()),
        }
    }
}

fn network_configurations_match(
    requested: &[NetworkOutputConfig],
    audio_sample_rate: u32,
    current: &std::collections::HashMap<String, (NetworkOutputConfig, u32)>,
) -> bool {
    requested.len() == current.len()
        && requested.iter().all(|config| {
            current
                .get(&config.id)
                .is_some_and(|(current_config, current_rate)| {
                    current_config == config && *current_rate == audio_sample_rate
                })
        })
}

fn reusable_network_configurations(
    endpoints: &std::collections::HashMap<String, Arc<NetworkOutputEndpoint>>,
) -> Option<std::collections::HashMap<String, (NetworkOutputConfig, u32)>> {
    let mut current = std::collections::HashMap::with_capacity(endpoints.len());
    for (id, endpoint) in endpoints {
        if !endpoint.is_reusable() {
            return None;
        }
        current.insert(
            id.clone(),
            (endpoint.config.clone(), endpoint.audio_sample_rate),
        );
    }
    Some(current)
}

/// Testable transition seam: old workers are stopped before the prepared map
/// is published. The endpoints mutex is intentionally owned by `publish` and
/// the caller's final closure, never held while `stop` runs.
fn stop_endpoints_before_publish<T, E>(
    previous: std::collections::HashMap<String, Arc<T>>,
    next: std::collections::HashMap<String, Arc<T>>,
    mut stop: impl FnMut(&str, &Arc<T>),
    publish: impl FnOnce(std::collections::HashMap<String, Arc<T>>) -> Result<(), E>,
) -> Result<(), E> {
    for (id, endpoint) in &previous {
        stop(id, endpoint);
    }
    publish(next)
}

impl NetworkOutputManager {
    pub fn configure(&self, configs: &[NetworkOutputConfig]) -> Result<(), String> {
        self.configure_with_program_audio(configs, Vec::new(), 48_000)
    }

    /// Check the currently published endpoints without depending on insertion
    /// order. The negotiated audio rate is part of each worker's contract.
    pub fn is_current_configuration(
        &self,
        configs: &[NetworkOutputConfig],
        audio_sample_rate: u32,
    ) -> bool {
        let Ok(_configure_guard) = self.configure_gate.lock() else {
            return false;
        };
        let Ok(endpoints) = self.endpoints.lock() else {
            return false;
        };
        reusable_network_configurations(&endpoints).is_some_and(|current| {
            network_configurations_match(configs, audio_sample_rate, &current)
        })
    }

    /// Configure workers together with one post-master stereo audio consumer
    /// for each active destination.  The GL producer remains video-only; the
    /// audio callback owns the matching producers and never waits on these
    /// network threads.
    pub fn configure_with_program_audio(
        &self,
        configs: &[NetworkOutputConfig],
        audio_receivers: Vec<ProgramAudioReceiver>,
        audio_sample_rate: u32,
    ) -> Result<(), String> {
        let _configure_guard = self
            .configure_gate
            .lock()
            .map_err(|_| "network output configure gate poisoned".to_string())?;
        let started_at = Instant::now();
        if configs.len() > MAX_NETWORK_OUTPUT_DESTINATIONS {
            return Err(format!(
                "at most {MAX_NETWORK_OUTPUT_DESTINATIONS} network output destinations are supported"
            ));
        }
        if !audio_receivers.is_empty() && audio_receivers.len() != configs.len() {
            return Err("network audio subscriptions do not match network destinations".into());
        }
        log::debug!(
            "[network-output] reconfigure begin: destinations={}, audio_rate={audio_sample_rate}",
            configs.len()
        );
        {
            let endpoints = self
                .endpoints
                .lock()
                .map_err(|_| "network output manager poisoned")?;
            if reusable_network_configurations(&endpoints).is_some_and(|current| {
                network_configurations_match(configs, audio_sample_rate, &current)
            }) {
                log::debug!(
                    "[network-output] reconfigure skipped: identical configuration, elapsed_ms={}",
                    started_at.elapsed().as_millis()
                );
                return Ok(());
            }
        }
        let mut next = std::collections::HashMap::new();
        let mut audio_receivers = audio_receivers.into_iter();
        for config in configs {
            config.validate()?;
            if next.contains_key(&config.id) {
                return Err(format!("Duplicate network output id '{}'", config.id));
            }
            let audio = audio_receivers.next();
            let format = audio.as_ref().map(ProgramAudioReceiver::format);
            let expected_format = format.as_deref().map(ProgramAudioFormat::snapshot);
            next.insert(
                config.id.clone(),
                Arc::new(NetworkOutputEndpoint::start(
                    config.clone(),
                    audio,
                    audio_sample_rate,
                    format,
                    expected_format,
                )),
            );
        }
        let previous = {
            let mut endpoints = self
                .endpoints
                .lock()
                .map_err(|_| "network output manager poisoned")?;
            // Publish an empty gap while old workers stop. New endpoints are
            // not visible to `publish` until the stop phase has completed.
            std::mem::take(&mut *endpoints)
        };

        stop_endpoints_before_publish(
            previous,
            next,
            |id, endpoint| {
                let stop_started = Instant::now();
                log::debug!("[network-output] stopping endpoint id={id}");
                endpoint.stop();
                log::debug!(
                    "[network-output] stopped endpoint id={id}, elapsed_ms={}",
                    stop_started.elapsed().as_millis()
                );
            },
            |next| {
                let mut endpoints = self
                    .endpoints
                    .lock()
                    .map_err(|_| "network output manager poisoned".to_string())?;
                *endpoints = next;
                Ok::<(), String>(())
            },
        )?;
        log::debug!(
            "[network-output] reconfigure complete: destinations={}, elapsed_ms={}",
            configs.len(),
            started_at.elapsed().as_millis()
        );
        Ok(())
    }

    /// Non-blocking producer entry point for a final compositor frame.
    pub fn publish(&self, output_id: &str, frame: BgraFrame) {
        if let Some(endpoint) = self
            .endpoints
            .lock()
            .ok()
            .and_then(|all| all.get(output_id).cloned())
        {
            endpoint.publish(frame);
        }
    }

    pub fn frame_sink(self: &Arc<Self>, output_id: impl Into<String>) -> NetworkFrameSink {
        NetworkFrameSink {
            output_id: output_id.into(),
            manager: Arc::clone(self),
        }
    }

    pub fn statuses(&self) -> Vec<NetworkOutputRuntimeStatus> {
        let mut statuses: Vec<_> = match self.endpoints.lock() {
            Ok(all) => all.values().map(|endpoint| endpoint.status()).collect(),
            Err(_) => Vec::new(),
        };
        statuses.sort_by(|a, b| a.output_id.cmp(&b.output_id));
        statuses
    }

    /// Build a lightweight diagnostics snapshot from the existing endpoint
    /// map. No transport is opened and no worker is queried beyond its
    /// already-held status mutex.
    pub fn diagnostics(&self) -> Vec<NetworkOutputDiagnostics> {
        let mut diagnostics = match self.endpoints.lock() {
            Ok(all) => all
                .values()
                .map(|endpoint| {
                    let status = endpoint.status();
                    let mut protocols = Vec::new();
                    if endpoint.config.ndi.is_some() {
                        protocols.push(NetworkProtocol::Ndi);
                    }
                    if endpoint.config.srt.is_some() {
                        protocols.push(NetworkProtocol::Srt);
                    }
                    let (endpoint_url, srt_mode, latency_ms) = endpoint
                        .config
                        .srt
                        .as_ref()
                        .map(|settings| {
                            (
                                Some(format!("srt://{}:{}", srt_host(settings), settings.port)),
                                Some(settings.mode),
                                Some(settings.latency_ms),
                            )
                        })
                        .unwrap_or((None, None, None));
                    NetworkOutputDiagnostics {
                        output_id: status.output_id,
                        protocols,
                        state: status.state,
                        endpoint: endpoint_url.or_else(|| {
                            endpoint.config.ndi.as_ref().map(|settings| {
                                format!("ndi://{}", settings.stream_name)
                            })
                        }),
                        srt_mode,
                        latency_ms,
                        submitted_frames: status.submitted_frames,
                        superseded_frames: status.superseded_frames,
                        last_error: status.last_error,
                    }
                })
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        diagnostics.sort_by(|a, b| a.output_id.cmp(&b.output_id));
        diagnostics
    }

    pub fn stop_all(&self) {
        let _configure_guard = self
            .configure_gate
            .lock()
            .expect("network output configure gate poisoned");
        let old = std::mem::take(
            &mut *self
                .endpoints
                .lock()
                .expect("network output manager poisoned"),
        );
        for endpoint in old.values() {
            endpoint.stop();
        }
    }
}

impl Drop for NetworkOutputManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

struct NetworkOutputEndpoint {
    config: NetworkOutputConfig,
    audio_sample_rate: u32,
    latest: Arc<(Mutex<Option<BgraFrame>>, Condvar)>,
    running: Arc<AtomicBool>,
    status: Arc<Mutex<NetworkOutputRuntimeStatus>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl NetworkOutputEndpoint {
    fn start(
        config: NetworkOutputConfig,
        audio: Option<ProgramAudioReceiver>,
        audio_sample_rate: u32,
        audio_format: Option<Arc<ProgramAudioFormat>>,
        expected_format: Option<ProgramAudioFormatSnapshot>,
    ) -> Self {
        let latest = Arc::new((Mutex::new(None), Condvar::new()));
        let running = Arc::new(AtomicBool::new(true));
        let status = Arc::new(Mutex::new(NetworkOutputRuntimeStatus::waiting(
            config.id.clone(),
        )));
        let thread_latest = latest.clone();
        let thread_running = running.clone();
        let thread_status = status.clone();
        let thread_config = config.clone();
        let thread = thread::Builder::new()
            .name(format!("qlisa-network-output-{}", config.id))
            .spawn(move || {
                network_output_loop(
                    thread_config,
                    thread_latest,
                    thread_running,
                    thread_status,
                    audio,
                    audio_sample_rate,
                    audio_format,
                    expected_format,
                );
            })
            .expect("spawn network output worker");
        Self {
            config,
            audio_sample_rate,
            latest,
            running,
            status,
            thread: Mutex::new(Some(thread)),
        }
    }

    fn publish(&self, frame: BgraFrame) {
        if !self.running.load(Ordering::Relaxed) {
            return;
        }
        let (lock, signal) = self.latest.as_ref();
        if let Ok(mut latest) = lock.lock() {
            let superseded = latest.replace(frame).is_some();
            if superseded {
                if let Ok(mut status) = self.status.lock() {
                    status.superseded_frames += 1;
                }
            }
            signal.notify_one();
        }
    }

    fn is_reusable(&self) -> bool {
        if !self.running.load(Ordering::Acquire) {
            return false;
        }
        let worker_alive = self
            .thread
            .lock()
            .ok()
            .and_then(|worker| worker.as_ref().map(|worker| !worker.is_finished()))
            .unwrap_or(false);
        if !worker_alive {
            return false;
        }
        // A transport error is recoverable: the worker deliberately keeps
        // its retry loop alive and reports `Error` while it backs off.  Do
        // not replace an otherwise live endpoint merely because Preferences
        // was applied during that interval; replacing it tears down the
        // sender identity (and makes receivers such as vMix lose the source).
        // A worker that cannot recover exits its thread, which is the actual
        // signal that the endpoint must be rebuilt.
        true
    }

    fn status(&self) -> NetworkOutputRuntimeStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_else(|_| NetworkOutputRuntimeStatus {
                output_id: self.config.id.clone(),
                state: NetworkOutputState::Error,
                submitted_frames: 0,
                superseded_frames: 0,
                last_error: Some("Network output status lock poisoned".into()),
            })
    }

    fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        self.latest.1.notify_all();
        if let Ok(mut thread) = self.thread.lock() {
            if let Some(thread) = thread.take() {
                let _ = thread.join();
            }
        }
        if let Ok(mut status) = self.status.lock() {
            if status.state != NetworkOutputState::Error {
                status.state = NetworkOutputState::Stopped;
            }
        }
    }
}

fn network_output_loop(
    config: NetworkOutputConfig,
    latest: Arc<(Mutex<Option<BgraFrame>>, Condvar)>,
    running: Arc<AtomicBool>,
    status: Arc<Mutex<NetworkOutputRuntimeStatus>>,
    mut audio: Option<ProgramAudioReceiver>,
    audio_sample_rate: u32,
    audio_format: Option<Arc<ProgramAudioFormat>>,
    expected_format: Option<ProgramAudioFormatSnapshot>,
) {
    let mut senders: Option<(Option<NdiSender>, Option<SrtFfmpegSender>)> = None;
    let mut retry = NetworkRetryBackoff::default();
    let mut audio_samples = Vec::with_capacity(8_192);
    let mut latest_frame: Option<BgraFrame> = None;
    let configured_audio_sample_rate = expected_format
        .map(|format| format.sample_rate)
        .unwrap_or(audio_sample_rate);
    while running.load(Ordering::Relaxed) {
        if let (Some(format), Some(expected)) = (&audio_format, expected_format) {
            if format.snapshot() != expected {
                // Sender parameters include the sample rate, so carrying on is
                // not merely a cosmetic status error: it plays program audio
                // at the wrong speed. Stop this endpoint deterministically;
                // the output configuration path replaces taps and workers on
                // the next explicit apply/retry.
                drop(senders.take());
                set_network_error(
                    &status,
                    "Audio engine format changed; reapply network outputs to restart this destination".into(),
                );
                return;
            }
        }
        let new_frame = {
            let (lock, signal) = latest.as_ref();
            let mut pending = match lock.lock() {
                Ok(value) => value,
                Err(_) => {
                    set_network_error(&status, "Network frame queue poisoned".into());
                    return;
                }
            };
            if pending.is_none() && running.load(Ordering::Relaxed) {
                let (next, _) = signal
                    .wait_timeout(pending, NETWORK_AUDIO_DRAIN_INTERVAL)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                pending = next;
            }
            pending.take()
        };
        let has_new_frame = new_frame.is_some();
        if let Some(frame) = new_frame {
            latest_frame = Some(frame);
        }
        if senders.is_none() {
            let Some(frame) = latest_frame.as_ref() else {
                continue;
            };
            match start_network_senders(&config, frame, configured_audio_sample_rate) {
                Ok(value) => {
                    senders = Some(value);
                    if let Some(audio) = audio.as_mut() {
                        let discarded = audio.discard_queued();
                        if discarded > 0 {
                            log::debug!(
                                "[network-output] dropped {discarded} stale program-audio samples while starting transport"
                            );
                        }
                    }
                }
                Err(error) => {
                    set_network_error(&status, error);
                    network_retry_backoff(&running, retry.failed());
                    continue;
                }
            }
        }
        let (ndi, srt) = senders.as_mut().expect("started above");
        if let Some(sender) = srt.as_mut() {
            if let Err(error) = sender.check_health() {
                set_network_error(&status, error);
                senders = None;
                network_retry_backoff(&running, retry.failed());
                continue;
            }
        }
        // A static program image still streams: the sender repeats its last
        // frame and may continue sending audio while no new compositor frame
        // arrives. Count that healthy interval so backoff can reset after a
        // stable stream instead of requiring motion in the picture.
        if status
            .lock()
            .is_ok_and(|current| current.state == NetworkOutputState::Streaming)
        {
            retry.note_streaming(Instant::now());
        }
        if let Some(audio) = audio.as_mut() {
            // This wake-up is independent of compositor frames. A static
            // program image must never stall NDI/SRT audio draining; the tap's
            // overwrite-oldest policy keeps a slow encoder at live latency.
            audio.drain_into(&mut audio_samples, 8_192);
            if !audio_samples.is_empty() {
                if let Some(sender) = ndi.as_mut() {
                    if let Err(error) = sender.send_audio_interleaved(
                        configured_audio_sample_rate,
                        2,
                        &audio_samples,
                    ) {
                        set_network_error(&status, error);
                        senders = None;
                        network_retry_backoff(&running, retry.failed());
                        continue;
                    }
                }
                // Unix SRT currently has only one raw-video pipe. Do not turn
                // that documented video-only transport into a teardown/retry
                // loop just because the shared NDI audio tap has data.
                #[cfg(windows)]
                if let Some(sender) = srt.as_mut() {
                    if let Err(error) = sender.send_audio_interleaved(
                        configured_audio_sample_rate,
                        2,
                        &audio_samples,
                    ) {
                        set_network_error(&status, error);
                        senders = None;
                        network_retry_backoff(&running, retry.failed());
                        continue;
                    }
                }
            }
        }
        // SRT repeats the last packed program image inside its paced pipe
        // writer. NDI intentionally receives only a fresh compositor image.
        if !has_new_frame {
            continue;
        }
        let Some(frame) = latest_frame.as_ref() else {
            continue;
        };
        let result = (|| -> Result<bool, String> {
            let mut accepted = false;
            if let Some(sender) = ndi.as_mut() {
                accepted |= sender.send(frame)?;
            }
            if let Some(sender) = srt.as_mut() {
                sender.send(frame)?;
                accepted = true;
            }
            Ok(accepted)
        })();
        match result {
            Ok(accepted) => {
                if let Ok(mut current) = status.lock() {
                    current.state = NetworkOutputState::Streaming;
                    current.last_error = None;
                    current.submitted_frames = ndi
                        .as_ref()
                        .map(|sender| sender.submitted_frames())
                        .into_iter()
                        .chain(srt.as_ref().map(|sender| sender.stats().submitted_frames))
                        .max()
                        .unwrap_or(0);
                    if !accepted {
                        current.superseded_frames += 1;
                    }
                }
            }
            Err(error) => {
                set_network_error(&status, error);
                senders = None;
                network_retry_backoff(&running, retry.failed());
            }
        }
    }
}

const NETWORK_RETRY_DELAYS: [Duration; 6] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(15),
    Duration::from_secs(30),
];
const NETWORK_RETRY_RESET_AFTER: Duration = Duration::from_secs(30);

#[derive(Default)]
struct NetworkRetryBackoff {
    consecutive_failures: usize,
    streaming_since: Option<Instant>,
}

impl NetworkRetryBackoff {
    fn failed(&mut self) -> Duration {
        self.streaming_since = None;
        let index = self
            .consecutive_failures
            .min(NETWORK_RETRY_DELAYS.len() - 1);
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        NETWORK_RETRY_DELAYS[index]
    }

    fn note_streaming(&mut self, now: Instant) {
        let since = *self.streaming_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= NETWORK_RETRY_RESET_AFTER {
            self.consecutive_failures = 0;
        }
    }
}

fn network_retry_backoff(running: &AtomicBool, delay: Duration) {
    let deadline = Instant::now() + delay;
    while running.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50)),
        );
    }
}

fn start_network_senders(
    config: &NetworkOutputConfig,
    frame: &BgraFrame,
    audio_sample_rate: u32,
) -> Result<(Option<NdiSender>, Option<SrtFfmpegSender>), String> {
    frame.validate()?;
    let ndi = match &config.ndi {
        Some(settings) => Some(NdiRuntime::load()?.create_sender(settings, 30)?),
        None => None,
    };
    let srt = match &config.srt {
        Some(settings) => Some(SrtFfmpegSender::start(
            settings,
            frame.width,
            frame.height,
            settings.fps.unwrap_or(30),
            audio_sample_rate,
        )?),
        None => None,
    };
    Ok((ndi, srt))
}

fn set_network_error(status: &Mutex<NetworkOutputRuntimeStatus>, error: String) {
    let error = sanitized_ffmpeg_diagnostic(&error);
    log::warn!("[network-output] {error}");
    if let Ok(mut status) = status.lock() {
        status.state = NetworkOutputState::Error;
        status.last_error = Some(error);
    }
}

// NDI's public C declarations below mirror the versioned SDK headers.  We use
// only the stable direct entry points and load them with `libloading`, so the
// GPL application has no NDI import library or build-time SDK dependency.
// Keep this small ABI boundary here; higher-level code must not use `unsafe`.
#[repr(C)]
struct NdiSendCreate {
    name: *const c_char,
    groups: *const c_char,
    clock_video: bool,
    clock_audio: bool,
}

#[repr(C)]
#[derive(Default)]
struct NdiSource {
    name: *const c_char,
    url_address: *const c_char,
}

#[repr(C)]
struct NdiFindCreate {
    show_local_sources: bool,
    groups: *const c_char,
    extra_ips: *const c_char,
}

/// One NDI source discovered by the user-installed runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NdiSourceInfo {
    pub name: String,
    pub url_address: Option<String>,
}

#[repr(C)]
struct NdiRecvCreateV3 {
    source: NdiSource,
    color_format: i32,
    bandwidth: i32,
    allow_video_fields: bool,
    name: *const c_char,
}

#[repr(C)]
#[derive(Default)]
struct NdiVideoFrameV2 {
    xres: i32,
    yres: i32,
    fourcc: i32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    picture_aspect_ratio: f32,
    frame_format: i32,
    timecode: i64,
    data: *mut u8,
    line_stride: i32,
    metadata: *const c_char,
    timestamp: i64,
}

/// NDI's legacy-but-still-supported planar floating-point audio frame.  Core
/// NDI v6 continues to expose the v2 send/capture APIs, which keeps the
/// boundary compatible with both v5 and v6 senders such as vMix.
#[repr(C)]
#[derive(Default)]
struct NdiAudioFrameV2 {
    sample_rate: i32,
    no_channels: i32,
    no_samples: i32,
    timecode: i64,
    data: *mut f32,
    channel_stride_in_bytes: i32,
    metadata: *const c_char,
    timestamp: i64,
}

type NdiInitialize = unsafe extern "C" fn() -> bool;
type NdiSendCreateFn = unsafe extern "C" fn(*const NdiSendCreate) -> *mut c_void;
type NdiSendDestroyFn = unsafe extern "C" fn(*mut c_void);
type NdiSendVideoFn = unsafe extern "C" fn(*mut c_void, *const NdiVideoFrameV2);
type NdiSendAudioFn = unsafe extern "C" fn(*mut c_void, *const NdiAudioFrameV2);
type NdiSendConnectionsFn = unsafe extern "C" fn(*mut c_void, u32) -> i32;
type NdiRecvCreateFn = unsafe extern "C" fn(*const NdiRecvCreateV3) -> *mut c_void;
type NdiRecvDestroyFn = unsafe extern "C" fn(*mut c_void);
type NdiRecvCaptureFn = unsafe extern "C" fn(
    *mut c_void,
    *mut NdiVideoFrameV2,
    *mut NdiAudioFrameV2,
    *mut c_void,
    u32,
) -> i32;
type NdiRecvFreeVideoFn = unsafe extern "C" fn(*mut c_void, *const NdiVideoFrameV2);
type NdiRecvFreeAudioFn = unsafe extern "C" fn(*mut c_void, *const NdiAudioFrameV2);
type NdiFindCreateFn = unsafe extern "C" fn(*const NdiFindCreate) -> *mut c_void;
type NdiFindDestroyFn = unsafe extern "C" fn(*mut c_void);
type NdiFindWaitForSourcesFn = unsafe extern "C" fn(*mut c_void, u32) -> bool;
type NdiFindGetCurrentSourcesFn = unsafe extern "C" fn(*mut c_void, *mut u32) -> *const NdiSource;

const NDI_FOURCC_BGRA: i32 = i32::from_le_bytes(*b"BGRA");
const NDI_FRAME_PROGRESSIVE: i32 = 1;
const NDI_FRAME_VIDEO: i32 = 1;
const NDI_RECV_BGRX_BGRA: i32 = 0;
// `NDIlib_recv_bandwidth_e` uses 0 for the preview/lowest stream and 100 for
// the upstream program/highest stream. Keep the SDK values here rather than
// treating this as a boolean: these are not ordered quality indexes.
const NDI_RECV_BANDWIDTH_LOWEST: i32 = 0;
const NDI_RECV_BANDWIDTH_HIGHEST: i32 = 100;
const NDI_SEND_TIMECODE_SYNTHESIZE: i64 = i64::MAX;

fn ndi_receiver_bandwidth(quality: NdiQuality) -> i32 {
    match quality {
        NdiQuality::Highest => NDI_RECV_BANDWIDTH_HIGHEST,
        NdiQuality::LowBandwidth => NDI_RECV_BANDWIDTH_LOWEST,
    }
}

/// The one Core NDI runtime held for the life of this process.
///
/// Core NDI creates global worker threads. It is therefore unsound to let the
/// last Rust `Library` handle call `FreeLibrary` merely because a temporary
/// sender, receiver, finder, or status probe went away. `OnceLock` statics are
/// intentionally never dropped, pinning the DLL and avoiding `NDIlib_destroy`
/// ordering against those global threads at process shutdown.
static NDI_RUNTIME: OnceLock<Result<Arc<NdiRuntimeInner>, String>> = OnceLock::new();

struct NdiRuntimeInner {
    library: libloading::Library,
    initialize: NdiInitialize,
    path: PathBuf,
}

fn load_ndi_runtime_once() -> Result<Arc<NdiRuntimeInner>, String> {
    let path = find_ndi_runtime().ok_or_else(|| "NDI Runtime was not found".to_string())?;
    let library = unsafe { libloading::Library::new(&path) }
        .map_err(|error| format!("Could not load NDI Runtime at {}: {error}", path.display()))?;
    let initialize = unsafe { ndi_symbol::<NdiInitialize>(&library, b"NDIlib_initialize\0")? };
    if !unsafe { initialize() } {
        return Err("NDI Runtime rejected this CPU or initialization failed".into());
    }
    Ok(Arc::new(NdiRuntimeInner {
        library,
        initialize,
        path,
    }))
}

/// A lightweight handle to the process-wide, pinned Core NDI runtime.
pub struct NdiRuntime {
    inner: Arc<NdiRuntimeInner>,
}

impl NdiRuntime {
    pub fn load() -> Result<Self, String> {
        let inner = NDI_RUNTIME
            .get_or_init(load_ndi_runtime_once)
            .as_ref()
            .map(Arc::clone)
            .map_err(Clone::clone)?;
        Ok(Self { inner })
    }

    fn library_path(&self) -> &std::path::Path {
        &self.inner.path
    }

    /// Starts a real Core NDI sender which accepts final BGRA frames.
    pub fn create_sender(
        &self,
        settings: &NdiOutputSettings,
        fps: u32,
    ) -> Result<NdiSender, String> {
        if settings.stream_name.trim().is_empty() {
            return Err("NDI stream name is required".into());
        }
        if fps == 0 {
            return Err("NDI frame rate must be non-zero".into());
        }
        let create =
            unsafe { ndi_symbol::<NdiSendCreateFn>(&self.inner.library, b"NDIlib_send_create\0")? };
        let destroy = unsafe {
            ndi_symbol::<NdiSendDestroyFn>(&self.inner.library, b"NDIlib_send_destroy\0")?
        };
        let send_video = unsafe {
            ndi_symbol::<NdiSendVideoFn>(&self.inner.library, b"NDIlib_send_send_video_v2\0")?
        };
        let send_audio = unsafe {
            ndi_symbol::<NdiSendAudioFn>(&self.inner.library, b"NDIlib_send_send_audio_v2\0")?
        };
        let connections = unsafe {
            ndi_symbol::<NdiSendConnectionsFn>(
                &self.inner.library,
                b"NDIlib_send_get_no_connections\0",
            )?
        };
        let name = CString::new(settings.stream_name.as_str())
            .map_err(|_| "NDI stream name contains an unsupported NUL byte".to_string())?;
        let group = (!settings.group.trim().is_empty())
            .then(|| {
                CString::new(settings.group.as_str())
                    .map_err(|_| "NDI group contains an unsupported NUL byte".to_string())
            })
            .transpose()?;
        let instance = unsafe {
            create(&NdiSendCreate {
                name: name.as_ptr(),
                groups: group.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                clock_video: false,
                clock_audio: false,
            })
        };
        if instance.is_null() {
            return Err("NDI Runtime could not create a sender".into());
        }
        Ok(NdiSender {
            _runtime: Arc::clone(&self.inner),
            instance,
            destroy,
            send_video,
            send_audio,
            connections,
            fps,
            quality: settings.quality,
            pacer: FramePacer::new(fps),
            _name: name,
            _group: group,
            submitted_frames: 0,
        })
    }

    /// Starts a Core NDI receiver for an explicitly chosen source name.
    pub fn create_receiver(
        &self,
        source_name: &str,
        quality: NdiQuality,
    ) -> Result<NdiReceiver, String> {
        if source_name.trim().is_empty() {
            return Err("NDI source name is required".into());
        }
        let create = unsafe {
            ndi_symbol::<NdiRecvCreateFn>(&self.inner.library, b"NDIlib_recv_create_v3\0")?
        };
        let destroy = unsafe {
            ndi_symbol::<NdiRecvDestroyFn>(&self.inner.library, b"NDIlib_recv_destroy\0")?
        };
        let capture = unsafe {
            ndi_symbol::<NdiRecvCaptureFn>(&self.inner.library, b"NDIlib_recv_capture_v2\0")?
        };
        let free_video = unsafe {
            ndi_symbol::<NdiRecvFreeVideoFn>(&self.inner.library, b"NDIlib_recv_free_video_v2\0")?
        };
        let free_audio = unsafe {
            ndi_symbol::<NdiRecvFreeAudioFn>(&self.inner.library, b"NDIlib_recv_free_audio_v2\0")?
        };
        let source_name = CString::new(source_name)
            .map_err(|_| "NDI source name contains an unsupported NUL byte".to_string())?;
        let create_settings = NdiRecvCreateV3 {
            source: NdiSource {
                name: source_name.as_ptr(),
                url_address: ptr::null(),
            },
            color_format: NDI_RECV_BGRX_BGRA,
            bandwidth: ndi_receiver_bandwidth(quality),
            allow_video_fields: false,
            name: ptr::null(),
        };
        let instance = unsafe { create(&create_settings) };
        if instance.is_null() {
            return Err("NDI Runtime could not create a receiver".into());
        }
        Ok(NdiReceiver {
            _runtime: Arc::clone(&self.inner),
            instance,
            destroy,
            capture,
            free_video,
            free_audio,
            _source_name: source_name,
        })
    }

    /// Enumerate sources currently visible to the NDI runtime.  The returned
    /// names are copied before the discovery instance is destroyed, so they
    /// are safe to persist in a cue configuration.
    pub fn list_sources(&self, timeout_ms: u32) -> Result<Vec<NdiSourceInfo>, String> {
        let create = unsafe {
            ndi_symbol::<NdiFindCreateFn>(&self.inner.library, b"NDIlib_find_create_v2\0")?
        };
        let destroy = unsafe {
            ndi_symbol::<NdiFindDestroyFn>(&self.inner.library, b"NDIlib_find_destroy\0")?
        };
        let wait = unsafe {
            ndi_symbol::<NdiFindWaitForSourcesFn>(
                &self.inner.library,
                b"NDIlib_find_wait_for_sources\0",
            )?
        };
        let current = unsafe {
            ndi_symbol::<NdiFindGetCurrentSourcesFn>(
                &self.inner.library,
                b"NDIlib_find_get_current_sources\0",
            )?
        };
        let instance = unsafe {
            create(&NdiFindCreate {
                show_local_sources: true,
                groups: ptr::null(),
                extra_ips: ptr::null(),
            })
        };
        if instance.is_null() {
            return Err("NDI Runtime could not create source discovery".into());
        }
        let finder = NdiFinder {
            _runtime: Arc::clone(&self.inner),
            instance,
            destroy,
        };
        // NDI discovery is asynchronous. A one-shot 1-second wait routinely
        // misses a sender which was already live (vMix especially), because
        // the finder has not yet joined the local discovery group. Keep this
        // bounded but re-read after short changes until the caller's deadline.
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1) as u64);
        let mut result = Vec::new();
        loop {
            let mut count = 0_u32;
            let sources = unsafe { current(finder.instance, &mut count) };
            if !sources.is_null() && count > 0 {
                let sources = unsafe { std::slice::from_raw_parts(sources, count as usize) };
                result = sources
                    .iter()
                    .filter_map(|source| {
                        if source.name.is_null() {
                            return None;
                        }
                        let name = unsafe { std::ffi::CStr::from_ptr(source.name) }
                            .to_string_lossy()
                            .into_owned();
                        if name.trim().is_empty() {
                            return None;
                        }
                        let url_address = (!source.url_address.is_null()).then(|| unsafe {
                            std::ffi::CStr::from_ptr(source.url_address)
                                .to_string_lossy()
                                .into_owned()
                        });
                        Some(NdiSourceInfo { name, url_address })
                    })
                    .collect();
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline.saturating_duration_since(now);
            let wait_ms = remaining.min(Duration::from_millis(250)).as_millis() as u32;
            let _ = unsafe { wait(finder.instance, wait_ms.max(1)) };
        }
        Ok(result)
    }

    /// Kept to make the initialization function observable in a debugger and
    /// to document that an `NdiRuntime` owns initialized SDK state.
    pub fn is_initialized(&self) -> bool {
        let _ = self.inner.initialize;
        true
    }
}

/// RAII wrapper for NDI discovery. Source strings are copied before this is
/// dropped; the native finder is then destroyed while the pinned runtime is
/// still guaranteed to be available.
struct NdiFinder {
    _runtime: Arc<NdiRuntimeInner>,
    instance: *mut c_void,
    destroy: NdiFindDestroyFn,
}

impl Drop for NdiFinder {
    fn drop(&mut self) {
        if !self.instance.is_null() {
            unsafe {
                (self.destroy)(self.instance);
            }
        }
    }
}

/// Synchronous NDI sender. The SDK has copied/consumed the image when `send`
/// returns; unlike the async API, no caller buffer must be pinned afterwards.
pub struct NdiSender {
    _runtime: Arc<NdiRuntimeInner>,
    instance: *mut c_void,
    destroy: NdiSendDestroyFn,
    send_video: NdiSendVideoFn,
    send_audio: NdiSendAudioFn,
    connections: NdiSendConnectionsFn,
    fps: u32,
    quality: NdiQuality,
    pacer: FramePacer,
    _name: CString,
    _group: Option<CString>,
    submitted_frames: u64,
}

unsafe impl Send for NdiSender {}

impl NdiSender {
    /// Returns true only when this fresh frame reaches Core NDI. Deferred
    /// frames are dropped instead of making the declared 30fps stream burst.
    pub fn send(&mut self, frame: &BgraFrame) -> Result<bool, String> {
        frame.validate()?;
        if !self.pacer.is_due(Instant::now()) {
            return Ok(false);
        }
        let reduced;
        let frame = if self.quality == NdiQuality::LowBandwidth {
            reduced = frame.downscaled_to_fit(1280, 720)?;
            &reduced
        } else {
            frame
        };
        let width =
            i32::try_from(frame.width).map_err(|_| "NDI frame width is too large".to_string())?;
        let height =
            i32::try_from(frame.height).map_err(|_| "NDI frame height is too large".to_string())?;
        let stride =
            i32::try_from(frame.stride).map_err(|_| "NDI frame stride is too large".to_string())?;
        let ndi_frame = NdiVideoFrameV2 {
            xres: width,
            yres: height,
            fourcc: NDI_FOURCC_BGRA,
            frame_rate_n: self.fps as i32,
            frame_rate_d: 1,
            picture_aspect_ratio: frame.width as f32 / frame.height as f32,
            frame_format: NDI_FRAME_PROGRESSIVE,
            timecode: NDI_SEND_TIMECODE_SYNTHESIZE,
            data: frame.data.as_ptr() as *mut u8,
            line_stride: stride,
            metadata: ptr::null(),
            timestamp: 0,
        };
        unsafe {
            (self.send_video)(self.instance, &ndi_frame);
        }
        self.pacer.mark_submitted(Instant::now());
        self.submitted_frames += 1;
        Ok(true)
    }

    pub fn receiver_count(&self) -> i32 {
        unsafe { (self.connections)(self.instance, 0) }
    }
    pub fn submitted_frames(&self) -> u64 {
        self.submitted_frames
    }

    /// Send one interleaved program-audio block.  NDI v2 transports planar
    /// `f32`; conversion happens on the network worker, never in the audio
    /// callback that produced this block.
    pub fn send_audio_interleaved(
        &mut self,
        sample_rate: u32,
        channels: u32,
        interleaved: &[f32],
    ) -> Result<(), String> {
        if sample_rate == 0 || channels == 0 {
            return Err("NDI audio sample rate and channels must be non-zero".into());
        }
        let channels_usize = channels as usize;
        if interleaved.len() % channels_usize != 0 {
            return Err("NDI audio buffer is not interleaved by channel".into());
        }
        let samples = interleaved.len() / channels_usize;
        if samples == 0 {
            return Ok(());
        }
        let sample_rate = i32::try_from(sample_rate)
            .map_err(|_| "NDI audio sample rate is too large".to_string())?;
        let channels_i32 = i32::try_from(channels)
            .map_err(|_| "NDI audio channel count is too large".to_string())?;
        let samples_i32 =
            i32::try_from(samples).map_err(|_| "NDI audio packet is too large".to_string())?;
        let mut planar = vec![0.0_f32; interleaved.len()];
        for channel in 0..channels_usize {
            for sample in 0..samples {
                planar[channel * samples + sample] = interleaved[sample * channels_usize + channel];
            }
        }
        let frame = NdiAudioFrameV2 {
            sample_rate,
            no_channels: channels_i32,
            no_samples: samples_i32,
            timecode: NDI_SEND_TIMECODE_SYNTHESIZE,
            data: planar.as_mut_ptr(),
            channel_stride_in_bytes: i32::try_from(samples * std::mem::size_of::<f32>())
                .map_err(|_| "NDI audio stride is too large".to_string())?,
            metadata: ptr::null(),
            timestamp: 0,
        };
        // `NDIlib_send_send_audio_v2` is synchronous: Core NDI has consumed
        // the buffer before it returns, so `planar` can safely be dropped.
        unsafe {
            (self.send_audio)(self.instance, &frame);
        }
        Ok(())
    }
}

impl Drop for NdiSender {
    fn drop(&mut self) {
        unsafe {
            (self.destroy)(self.instance);
        }
    }
}

/// A selected NDI source receiver. `capture_bgra` copies the SDK-owned frame
/// before freeing it, so callers receive an ordinary owned `BgraFrame`.
pub struct NdiReceiver {
    _runtime: Arc<NdiRuntimeInner>,
    instance: *mut c_void,
    destroy: NdiRecvDestroyFn,
    capture: NdiRecvCaptureFn,
    free_video: NdiRecvFreeVideoFn,
    free_audio: NdiRecvFreeAudioFn,
    _source_name: CString,
}

unsafe impl Send for NdiReceiver {}

/// One copied NDI audio block.  Samples are ordinary interleaved `f32` so the
/// rest of Qlisa never needs to know Core NDI's planar wire representation.
#[derive(Debug, Clone, PartialEq)]
pub struct PcmAudioBlock {
    pub sample_rate: u32,
    pub channels: u32,
    pub samples: Vec<f32>,
}

enum NdiCapture {
    Video(BgraFrame),
    Audio(PcmAudioBlock),
}

/// Frees a video frame returned by `NDIlib_recv_capture_v2` on every return
/// path, including validation and size-overflow failures before its bytes are
/// copied into Qlisa-owned memory.
struct NdiVideoFreeGuard<'a> {
    instance: *mut c_void,
    free_video: NdiRecvFreeVideoFn,
    frame: &'a NdiVideoFrameV2,
}

impl Drop for NdiVideoFreeGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.free_video)(self.instance, self.frame);
        }
    }
}

impl NdiReceiver {
    fn capture(&mut self, timeout_ms: u32) -> Result<Option<NdiCapture>, String> {
        let mut video = NdiVideoFrameV2::default();
        let mut audio = NdiAudioFrameV2::default();
        let frame_type = unsafe {
            (self.capture)(
                self.instance,
                &mut video,
                (&mut audio as *mut NdiAudioFrameV2).cast(),
                ptr::null_mut(),
                timeout_ms,
            )
        };
        if frame_type == 0 {
            return Ok(None);
        }
        if frame_type != NDI_FRAME_VIDEO {
            if frame_type == 2 {
                return self.copy_audio(&audio).map(Some);
            }
            return if frame_type == 4 {
                Err("NDI receiver connection failed".into())
            } else {
                Ok(None)
            };
        }
        // Core NDI owns this buffer until the matching free call. Install the
        // guard before even validating its fields, so a malformed SDK frame
        // cannot leak on any early `?` or `return` below.
        let _free = NdiVideoFreeGuard {
            instance: self.instance,
            free_video: self.free_video,
            frame: &video,
        };
        if video.xres <= 0 || video.yres <= 0 || video.line_stride == 0 || video.data.is_null() {
            return Err("NDI returned an invalid video frame".into());
        }
        let width = video.xres as u32;
        let height = video.yres as u32;
        let stride = video.line_stride.unsigned_abs();
        let total = (stride as usize)
            .checked_mul(height as usize)
            .ok_or("NDI frame size overflow")?;
        // NDI may provide a negative stride for flipped Windows frames. Its
        // pointer still denotes logical row zero; walking by the signed stride
        // produces conventional top-to-bottom rows in our owned buffer.
        let mut data = vec![0; total];
        unsafe {
            if video.line_stride > 0 {
                ptr::copy_nonoverlapping(video.data, data.as_mut_ptr(), total);
            } else {
                for row in 0..height as usize {
                    let src = video
                        .data
                        .offset((row as isize) * video.line_stride as isize);
                    let dst = data.as_mut_ptr().add(row * stride as usize);
                    ptr::copy_nonoverlapping(src, dst, stride as usize);
                }
            }
        }
        Ok(Some(NdiCapture::Video(BgraFrame {
            width,
            height,
            stride,
            data,
        })))
    }

    fn copy_audio(&self, audio: &NdiAudioFrameV2) -> Result<NdiCapture, String> {
        struct AudioFreeGuard<'a> {
            receiver: &'a NdiReceiver,
            frame: &'a NdiAudioFrameV2,
        }
        impl Drop for AudioFreeGuard<'_> {
            fn drop(&mut self) {
                unsafe {
                    (self.receiver.free_audio)(self.receiver.instance, self.frame);
                }
            }
        }
        let _free = AudioFreeGuard {
            receiver: self,
            frame: audio,
        };
        if audio.sample_rate <= 0
            || audio.no_channels <= 0
            || audio.no_samples < 0
            || audio.data.is_null()
        {
            return Err("NDI returned an invalid audio frame".into());
        }
        let channels = audio.no_channels as usize;
        let samples = audio.no_samples as usize;
        let stride = if audio.channel_stride_in_bytes == 0 {
            samples
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or("NDI audio stride overflow")?
        } else {
            audio.channel_stride_in_bytes.unsigned_abs() as usize
        };
        if stride < samples.saturating_mul(std::mem::size_of::<f32>()) {
            return Err("NDI audio channel stride is too small".into());
        }
        let total = channels
            .checked_mul(samples)
            .ok_or("NDI audio frame size overflow")?;
        let mut interleaved = vec![0.0_f32; total];
        unsafe {
            for channel in 0..channels {
                let source = (audio.data as *const u8).add(channel * stride) as *const f32;
                for sample in 0..samples {
                    interleaved[sample * channels + channel] = *source.add(sample);
                }
            }
        }
        Ok(NdiCapture::Audio(PcmAudioBlock {
            sample_rate: audio.sample_rate as u32,
            channels: audio.no_channels as u32,
            samples: interleaved,
        }))
    }
}

/// Truthful state for a selected NDI input. `Receiving` means at least one
/// decoded BGRA frame has been copied into its latest-frame mailbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NdiInputRuntimeStatus {
    pub source_name: String,
    pub receiving: bool,
    pub received_frames: u64,
    pub superseded_frames: u64,
    #[serde(default)]
    pub received_audio_blocks: u64,
    #[serde(default)]
    pub dropped_audio_samples: u64,
    pub last_error: Option<String>,
}

/// Audio bridge supplied by a Camera Cue.  Its producer is owned by the NDI
/// receive thread and feeds the existing synthetic-input route in AudioEngine.
/// This keeps network I/O off the real-time output callback.
pub struct NdiAudioInputSink {
    producer: SyntheticFeedProducer,
    resampler: NdiStereoResampler,
}

/// Streaming linear resampler for the first discrete NDI channel pair.
///
/// `phase_numerator` is expressed in units of `target_sample_rate`, so it can
/// be carried across arbitrary NDI packet boundaries without accumulating the
/// per-packet rounding error of `floor(packet_frames * dst / src)`.  Linear
/// interpolation is causal: at most one source sample remains pending until
/// the next packet supplies the right-hand endpoint.
struct NdiStereoResampler {
    target_sample_rate: u32,
    source_sample_rate: Option<u32>,
    source_channels: Option<u32>,
    previous: Option<[f32; 2]>,
    phase_numerator: u64,
}

impl NdiStereoResampler {
    fn new(target_sample_rate: u32) -> Self {
        Self {
            target_sample_rate: target_sample_rate.max(1),
            source_sample_rate: None,
            source_channels: None,
            previous: None,
            phase_numerator: 0,
        }
    }

    fn reset_for_format(&mut self, sample_rate: u32, channels: u32) {
        self.source_sample_rate = Some(sample_rate);
        self.source_channels = Some(channels);
        self.previous = None;
        self.phase_numerator = 0;
    }

    /// Select the first discrete pair. Mono is duplicated; channels beyond
    /// the first pair are deliberately ignored because NDI does not describe a
    /// speaker layout from which a universal surround downmix could be inferred.
    fn process_interleaved(
        &mut self,
        sample_rate: u32,
        channels: u32,
        samples: &[f32],
        mut emit: impl FnMut([f32; 2]),
    ) -> bool {
        if sample_rate == 0 || channels == 0 || samples.is_empty() {
            return false;
        }
        let channels_usize = channels as usize;
        if samples.len() % channels_usize != 0 {
            return false;
        }
        if self.source_sample_rate != Some(sample_rate) || self.source_channels != Some(channels) {
            self.reset_for_format(sample_rate, channels);
        }

        for frame in samples.chunks_exact(channels_usize) {
            let left = finite_ndi_sample(frame[0]);
            let right = if channels_usize > 1 {
                finite_ndi_sample(frame[1])
            } else {
                left
            };
            self.process_frame([left, right], &mut emit);
        }
        true
    }

    fn process_frame(&mut self, current: [f32; 2], emit: &mut impl FnMut([f32; 2])) {
        let Some(previous) = self.previous else {
            emit(current);
            self.previous = Some(current);
            self.phase_numerator = self.source_sample_rate.unwrap_or(1) as u64;
            return;
        };
        let source_rate = self.source_sample_rate.unwrap_or(1) as u64;
        let target_rate = self.target_sample_rate as u64;
        while self.phase_numerator <= target_rate {
            let fraction = self.phase_numerator as f64 / target_rate as f64;
            emit([
                interpolate_ndi_sample(previous[0], current[0], fraction),
                interpolate_ndi_sample(previous[1], current[1], fraction),
            ]);
            self.phase_numerator += source_rate;
        }
        self.phase_numerator -= target_rate;
        self.previous = Some(current);
    }
}

fn finite_ndi_sample(sample: f32) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        0.0
    }
}

fn interpolate_ndi_sample(left: f32, right: f32, fraction: f64) -> f32 {
    finite_ndi_sample((left as f64 + (right as f64 - left as f64) * fraction) as f32)
}

impl NdiAudioInputSink {
    pub fn new(producer: SyntheticFeedProducer, target_sample_rate: u32) -> Self {
        Self {
            producer,
            resampler: NdiStereoResampler::new(target_sample_rate),
        }
    }

    /// Select the first discrete NDI channel pair (duplicating mono) and
    /// resample it to the AudioEngine's current rate. It runs on the receiver
    /// thread; overflow drops a whole stereo frame and asks the callback to
    /// flush old queued samples, which keeps the contribution at the live edge.
    fn push(&mut self, block: &PcmAudioBlock) -> u64 {
        let mut dropped = 0_u64;
        let producer = &mut self.producer;
        let valid = self.resampler.process_interleaved(
            block.sample_rate,
            block.channels,
            &block.samples,
            |frame| {
                dropped += producer.try_push_frame(&frame).err().unwrap_or(0) as u64;
            },
        );
        if !valid && block.sample_rate != 0 && block.channels != 0 && !block.samples.is_empty() {
            return block.samples.len() as u64;
        }
        dropped
    }
}

/// Dedicated NDI receive loop. It owns the SDK receiver and publishes only
/// the latest decoded frame, which makes it safe to attach to a GL texture
/// without giving a network thread any OpenGL access.
pub struct NdiInputWorker {
    mailbox: Arc<BgraFrameMailbox>,
    running: Arc<AtomicBool>,
    status: Arc<Mutex<NdiInputRuntimeStatus>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl NdiInputWorker {
    /// Create the DLL-backed receiver before starting its capture thread, so
    /// Camera GO fails rather than running black when NDI is unavailable.
    pub fn start(source_name: impl Into<String>, quality: NdiQuality) -> Result<Self, String> {
        Self::start_with_audio(source_name, quality, None)
    }

    pub fn start_with_audio(
        source_name: impl Into<String>,
        quality: NdiQuality,
        mut audio_sink: Option<NdiAudioInputSink>,
    ) -> Result<Self, String> {
        let source_name = source_name.into();
        // NDI receiver construction only registers/connects the source; the
        // actual network wait happens in `capture(100)` below. Doing it on
        // this owner thread removes the former startup-timeout race where an
        // error returned while a detached worker was still creating native NDI
        // state. From here onward every NDI capture thread is retained by the
        // returned worker and stopped/joined by `stop`/`Drop`.
        let receiver = NdiRuntime::load()?.create_receiver(&source_name, quality)?;
        let mailbox = Arc::new(BgraFrameMailbox::default());
        let running = Arc::new(AtomicBool::new(true));
        let status = Arc::new(Mutex::new(NdiInputRuntimeStatus {
            source_name: source_name.clone(),
            receiving: false,
            received_frames: 0,
            superseded_frames: 0,
            received_audio_blocks: 0,
            dropped_audio_samples: 0,
            last_error: None,
        }));
        let thread_mailbox = Arc::clone(&mailbox);
        let thread_running = Arc::clone(&running);
        let thread_status = Arc::clone(&status);
        let thread = thread::Builder::new()
            .name(format!("qlisa-ndi-input-{source_name}"))
            .spawn(move || {
                let mut receiver = receiver;
                let result = (|| -> Result<(), String> {
                    while thread_running.load(Ordering::Relaxed) {
                        match receiver.capture(100)? {
                            Some(NdiCapture::Video(frame)) => {
                                let replaced = thread_mailbox.publish(frame).is_some();
                                if let Ok(mut state) = thread_status.lock() {
                                    state.receiving = true;
                                    state.received_frames += 1;
                                    if replaced {
                                        state.superseded_frames += 1;
                                    }
                                }
                            }
                            Some(NdiCapture::Audio(audio)) => {
                                let dropped = audio_sink
                                    .as_mut()
                                    .map(|sink| sink.push(&audio))
                                    .unwrap_or(0);
                                if let Ok(mut state) = thread_status.lock() {
                                    state.received_audio_blocks += 1;
                                    state.dropped_audio_samples += dropped;
                                }
                            }
                            None => {}
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    if let Ok(mut state) = thread_status.lock() {
                        state.receiving = false;
                        state.last_error = Some(error);
                    }
                }
            })
            .map_err(|error| format!("Could not start NDI input worker: {error}"))?;
        Ok(Self {
            mailbox,
            running,
            status,
            thread: Mutex::new(Some(thread)),
        })
    }

    pub fn mailbox(&self) -> Arc<BgraFrameMailbox> {
        Arc::clone(&self.mailbox)
    }

    pub fn status(&self) -> NdiInputRuntimeStatus {
        self.status
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| NdiInputRuntimeStatus {
                source_name: String::new(),
                receiving: false,
                received_frames: 0,
                superseded_frames: 0,
                received_audio_blocks: 0,
                dropped_audio_samples: 0,
                last_error: Some("NDI input status lock poisoned".into()),
            })
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        if let Ok(mut thread) = self.thread.lock() {
            if let Some(thread) = thread.take() {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for NdiInputWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Drop for NdiReceiver {
    fn drop(&mut self) {
        unsafe {
            (self.destroy)(self.instance);
        }
    }
}

unsafe fn ndi_symbol<T: Copy>(
    library: &libloading::Library,
    name: &'static [u8],
) -> Result<T, String> {
    // SAFETY: `T` is always one of the exact `extern "C"` signatures declared
    // immediately above from the public NDI SDK headers. The copied pointer is
    // retained only while `library` is held by a sender/receiver/runtime.
    unsafe {
        library.get::<T>(name).map(|symbol| *symbol).map_err(|e| {
            format!(
                "NDI Runtime lacks {}: {e}",
                String::from_utf8_lossy(&name[..name.len() - 1])
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use ringbuf::traits::Split;

    #[test]
    fn ndi_runtime_search_ignores_app_local_dlls() {
        let executable_directory = std::env::current_exe()
            .expect("test executable path")
            .parent()
            .expect("test executable directory")
            .to_path_buf();
        let candidates = ndi_candidates();
        for dll in ["Processing.NDI.Lib.x64.dll", "Processing.NDI.Lib.x86.dll"] {
            assert!(!candidates.contains(&executable_directory.join(dll)));
            assert!(!candidates.contains(
                &executable_directory.join("resources").join("ndi").join(dll)
            ));
        }
    }

    #[test]
    fn ffmpeg_job_policy_preserves_existing_limits_and_sets_kill_on_close() {
        let existing = 0x0000_0008;
        let flags = ffmpeg_job_limit_flags(existing);
        assert_eq!(flags & existing, existing);
        assert_ne!(flags & FFMPEG_JOB_KILL_ON_CLOSE, 0);
    }

    #[test]
    fn network_retry_backoff_caps_and_resets_after_sustained_streaming() {
        let mut retry = NetworkRetryBackoff::default();
        let expected = [1, 2, 4, 8, 15, 30, 30, 30];
        for seconds in expected {
            assert_eq!(retry.failed(), Duration::from_secs(seconds));
        }

        let stable_start = Instant::now();
        retry.note_streaming(stable_start);
        retry.note_streaming(stable_start + NETWORK_RETRY_RESET_AFTER - Duration::from_secs(1));
        assert_eq!(retry.failed(), Duration::from_secs(30));

        let recovered_at = stable_start + NETWORK_RETRY_RESET_AFTER + Duration::from_secs(1);
        retry.note_streaming(recovered_at);
        retry.note_streaming(recovered_at + NETWORK_RETRY_RESET_AFTER);
        assert_eq!(retry.failed(), Duration::from_secs(1));
    }

    #[test]
    fn srt_url_preserves_modes_and_encodes_secrets() {
        let srt = SrtSettings {
            mode: SrtMode::Caller,
            host: "10.0.0.5".into(),
            port: 4200,
            latency_ms: 80,
            passphrase: Some("correct horse".into()),
            stream_id: Some("show/a".into()),
            payload_size: Some(1316),
            ..Default::default()
        };
        let url = srt.url().unwrap();
        assert!(url.starts_with("srt://10.0.0.5:4200?mode=caller&latency=80000"));
        assert!(url.contains("passphrase=correct%20horse"));
        assert!(url.contains("streamid=show%2Fa"));
    }

    #[test]
    fn srt_url_brackets_ipv6_and_rejects_uri_injection() {
        let srt = SrtSettings {
            mode: SrtMode::Caller,
            host: "2001:db8::1".into(),
            ..Default::default()
        };
        assert!(srt.url().unwrap().starts_with("srt://[2001:db8::1]:"));
        assert!(SrtSettings {
            mode: SrtMode::Caller,
            host: "host/?mode=listener".into(),
            ..Default::default()
        }
        .validate()
        .is_err());
    }

    fn srt_audio_packet(sample_rate: u32, marker: f32, frames: usize) -> SrtAudioPacket {
        SrtAudioPacket {
            sample_rate,
            channels: 2,
            samples: (0..frames).flat_map(|_| [marker, -marker]).collect(),
        }
    }

    #[test]
    fn srt_audio_queue_preserves_fifo_order_without_overflow() {
        let mut queue = SrtAudioQueue::with_capacity_frames(48_000, 4);
        assert_eq!(queue.push(srt_audio_packet(48_000, 1.0, 2)), Ok(0));
        assert_eq!(queue.push(srt_audio_packet(48_000, 2.0, 2)), Ok(0));
        assert_eq!(queue.queued_frames, 4);
        assert_eq!(queue.packets.len(), 2);

        assert_eq!(queue.pop().unwrap().samples, [1.0, -1.0, 1.0, -1.0]);
        assert_eq!(queue.pop().unwrap().samples, [2.0, -2.0, 2.0, -2.0]);
        assert!(queue.pop().is_none());
        assert_eq!(queue.queued_frames, 0);
    }

    #[test]
    fn srt_audio_queue_overwrites_oldest_frames_when_full() {
        let mut queue = SrtAudioQueue::with_capacity_frames(48_000, 4);
        assert_eq!(queue.push(srt_audio_packet(48_000, 1.0, 3)), Ok(0));
        assert_eq!(queue.push(srt_audio_packet(48_000, 2.0, 3)), Ok(2));

        assert_eq!(queue.queued_frames, 4);
        assert_eq!(queue.packets.len(), 2);
        assert_eq!(queue.pop().unwrap().samples, [1.0, -1.0]);
        assert_eq!(
            queue.pop().unwrap().samples,
            [2.0, -2.0, 2.0, -2.0, 2.0, -2.0]
        );
        assert!(queue.pop().is_none());
        assert_eq!(queue.queued_frames, 0);
    }

    #[test]
    fn srt_audio_queue_oversized_packet_keeps_newest_tail() {
        let mut queue = SrtAudioQueue::with_capacity_frames(48_000, 4);
        let packet = SrtAudioPacket {
            sample_rate: 48_000,
            channels: 2,
            samples: (0..12).map(|sample| sample as f32).collect(),
        };

        assert_eq!(queue.push(packet), Ok(2));
        assert_eq!(queue.queued_frames, 4);
        assert_eq!(
            queue.pop().unwrap().samples,
            [4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0]
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn srt_audio_queue_requires_stereo_frames_at_the_negotiated_rate() {
        let mut queue = SrtAudioQueue::with_capacity_frames(48_000, 4);
        assert!(queue
            .push(SrtAudioPacket {
                sample_rate: 44_100,
                channels: 2,
                samples: vec![0.0, 0.0],
            })
            .is_err());
        assert!(queue
            .push(SrtAudioPacket {
                sample_rate: 48_000,
                channels: 2,
                samples: vec![0.0],
            })
            .is_err());
        assert!(queue
            .push(SrtAudioPacket {
                sample_rate: 0,
                channels: 2,
                samples: vec![0.0, 0.0],
            })
            .is_err());
        assert!(queue.is_empty());
        assert_eq!(queue.queued_frames, 0);
    }

    #[test]
    fn frame_pacer_never_bursts_after_a_late_frame() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(25);
        assert!(pacer.is_due(start));
        pacer.mark_submitted(start);
        assert!(!pacer.is_due(start + Duration::from_millis(39)));
        let late = start + Duration::from_millis(400);
        assert!(pacer.is_due(late));
        pacer.mark_submitted(late);
        assert!(!pacer.is_due(late + Duration::from_millis(39)));
    }

    #[test]
    fn frame_pacer_does_not_accumulate_frame_write_time() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(25);
        pacer.mark_submitted(start);
        let first_deadline = pacer.next_frame_at.expect("first deadline");

        // The second frame took 5 ms to pack/write.  Its deadline remains on
        // the original 25-fps cadence instead of moving 5 ms later.
        pacer.mark_submitted(start + Duration::from_millis(5));
        assert_eq!(
            pacer.next_frame_at,
            Some(first_deadline + pacer.interval),
            "frame write time must not accumulate into the cadence"
        );
    }

    #[test]
    fn ffmpeg_stderr_redacts_split_srt_secret() {
        let mut diagnostic = FfmpegDiagnostics::default();
        diagnostic.append("failed srt://host:9000?pass");
        diagnostic.append("phrase=do-not-show&streamid=show\n");
        let status = diagnostic.snapshot();
        assert!(!status.contains("srt://"));
        assert!(!status.contains("do-not-show"));
        assert!(status.contains("[SRT endpoint redacted]"));
    }

    #[test]
    fn diagnostics_redact_quoted_srt_passphrases() {
        let status = redact_srt_diagnostic(
            "mpv loadfile \"srt://host:9000?passphrase='camera secret'\" \
             passphrase=\"other secret\"",
        );
        assert!(!status.contains("srt://"));
        assert!(!status.contains("camera secret"));
        assert!(!status.contains("other secret"));
        assert_eq!(status.matches("passphrase=[redacted]").count(), 1);
        assert!(status.contains("[SRT endpoint redacted]"));
    }

    fn bmp_frame(width: u32, height: i32, stored_rows: &[u8]) -> Vec<u8> {
        let file_size = BMP_FILE_HEADER_BYTES + BMP_INFO_HEADER_BYTES + stored_rows.len();
        let mut bmp = vec![0_u8; file_size];
        bmp[..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&(file_size as u32).to_le_bytes());
        bmp[10..14].copy_from_slice(
            &((BMP_FILE_HEADER_BYTES + BMP_INFO_HEADER_BYTES) as u32).to_le_bytes(),
        );
        bmp[14..18].copy_from_slice(&(BMP_INFO_HEADER_BYTES as u32).to_le_bytes());
        bmp[18..22].copy_from_slice(&(width as i32).to_le_bytes());
        bmp[22..26].copy_from_slice(&height.to_le_bytes());
        bmp[26..28].copy_from_slice(&1_u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&32_u16.to_le_bytes());
        bmp[30..34].copy_from_slice(&0_u32.to_le_bytes()); // BI_RGB
        bmp[34..38].copy_from_slice(&(stored_rows.len() as u32).to_le_bytes());
        bmp[BMP_FILE_HEADER_BYTES + BMP_INFO_HEADER_BYTES..].copy_from_slice(stored_rows);
        bmp
    }

    #[test]
    fn bmp_pipe_parser_handles_partial_reads_and_bottom_up_rows() {
        // BMP stores this 2x2 image bottom-up. The parser must hand the GL
        // renderer conventional top-down BGRA rows without retaining padding.
        let top = [1, 2, 3, 255, 4, 5, 6, 255];
        let bottom = [7, 8, 9, 255, 10, 11, 12, 255];
        let mut stored = bottom.to_vec();
        stored.extend_from_slice(&top);
        let bmp = bmp_frame(2, 2, &stored);
        let mut parser = BmpPipeParser::default();
        assert!(parser.push(&bmp[..3]).unwrap().is_empty());
        assert!(parser.push(&bmp[3..37]).unwrap().is_empty());
        let parsed = parser.push(&bmp[37..]).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            (parsed[0].width, parsed[0].height, parsed[0].stride),
            (2, 2, 8)
        );
        let mut expected = top.to_vec();
        expected.extend_from_slice(&bottom);
        assert_eq!(parsed[0].data, expected);
    }

    #[test]
    fn bmp_pipe_parser_drains_concatenated_frames_from_its_borrowed_buffer() {
        let first = bmp_frame(1, 1, &[1, 2, 3, 255]);
        let second = bmp_frame(1, -1, &[4, 5, 6, 255]);
        let mut joined = first;
        joined.extend_from_slice(&second);
        let mut parser = BmpPipeParser::default();
        let frames = parser.push(&joined).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data, [1, 2, 3, 255]);
        assert_eq!(frames[1].data, [4, 5, 6, 255]);
        assert!(parser.pending.is_empty());
        assert!(parser.pending.capacity() <= SRT_INPUT_MAX_BMP_BYTES);
    }

    #[test]
    fn bmp_pipe_parser_rejects_malformed_and_oversized_frames() {
        let mut malformed = bmp_frame(1, 1, &[0, 0, 0, 255]);
        malformed[28..30].copy_from_slice(&24_u16.to_le_bytes());
        assert!(parse_bgra_bmp(&malformed).is_err());

        let mut oversized_header = b"BM".to_vec();
        oversized_header.extend_from_slice(&((SRT_INPUT_MAX_BMP_BYTES + 1) as u32).to_le_bytes());
        assert!(BmpPipeParser::default().push(&oversized_header).is_err());

        let too_wide = bmp_frame(SRT_INPUT_MAX_DIMENSION + 1, 1, &[0, 0, 0, 255]);
        assert!(parse_bgra_bmp(&too_wide).is_err());
    }

    #[test]
    fn unified_srt_input_command_has_one_srt_input_and_optional_audio_tee() {
        let url = "srt://receiver.example:9000?mode=caller&passphrase=not-for-logs";
        let args = srt_input_ffmpeg_args(url, 48_000, r"\\.\pipe\video", r"\\.\pipe\audio");
        assert_eq!(
            args.iter().filter(|value| value.as_str() == "-i").count(),
            1
        );
        assert_eq!(
            args.windows(2)
                .filter(|pair| pair[0] == "-i" && pair[1] == url)
                .count(),
            1
        );
        assert!(args.windows(2).any(|pair| pair == ["-map", "0:a:0?"]));
        assert!(args
            .windows(2)
            .any(|pair| { pair == ["-af", "aresample=48000:async=1000:first_pts=0"] }));
        assert!(args.windows(2).any(|pair| pair == ["-ar", "48000"]));
        let tee = args.last().expect("tee target");
        assert!(tee.contains("select=v:f=image2pipe"));
        assert!(tee.contains("onfail=ignore:select=a:f=f32le"));
    }

    #[test]
    fn tee_escapes_windows_named_pipe_separators() {
        assert_eq!(
            ffmpeg_tee_escape_target(r"\\.\pipe\qlisa-srt-input-video"),
            r"\\\\.\\pipe\\qlisa-srt-input-video"
        );
        assert_eq!(ffmpeg_tee_escape_target("a:b|c"), r"a\:b\|c");
    }

    #[test]
    fn missing_optional_audio_is_reported_without_marking_video_dead() {
        let diagnostic = "[f32le] No streams to mux were specified\n[tee] Slave muxer #1 failed, continuing with 1/2 slaves.";
        assert!(ffmpeg_reports_no_audio(diagnostic));
        let mut status = SrtInputRuntimeStatus {
            receiving_video: true,
            received_frames: 42,
            ..Default::default()
        };
        if !status.receiving_audio && ffmpeg_reports_no_audio(diagnostic) {
            status.audio_eof = true;
        }
        assert!(status.receiving_video);
        assert_eq!(status.received_frames, 42);
        assert!(status.audio_eof);
        assert!(status.last_error.is_none());
    }

    #[test]
    fn srt_input_terminal_status_keeps_audio_optional_but_requires_video() {
        let video_only = SrtInputRuntimeStatus {
            receiving_video: true,
            audio_eof: true,
            ..Default::default()
        };
        assert!(!video_only.feeds_unavailable());
        assert!(video_only.terminal_diagnostic().is_none());

        let video_ended_while_audio_drains = SrtInputRuntimeStatus {
            receiving_audio: true,
            video_eof: true,
            ..Default::default()
        };
        assert_eq!(
            video_ended_while_audio_drains
                .terminal_diagnostic()
                .as_deref(),
            Some("FFmpeg SRT video stream ended")
        );

        let failed = SrtInputRuntimeStatus {
            audio_eof: true,
            video_eof: true,
            last_error: Some("srt://secret.example?passphrase=do-not-show failed".into()),
            ..Default::default()
        };
        assert!(failed.feeds_unavailable());
        let diagnostic = failed.terminal_diagnostic().unwrap();
        assert!(!diagnostic.contains("srt://"));
        assert!(!diagnostic.contains("do-not-show"));
    }

    #[cfg(windows)]
    fn unused_loopback_port() -> u16 {
        let socket = std::net::UdpSocket::bind(("127.0.0.1", 0)).expect("reserve UDP port");
        socket.local_addr().expect("local UDP address").port()
    }

    #[cfg(windows)]
    fn listener_settings(port: u16) -> SrtSettings {
        SrtSettings {
            enabled: true,
            mode: SrtMode::Listener,
            host: "127.0.0.1".into(),
            port,
            ..Default::default()
        }
    }

    /// Simulate a failure after FFmpeg spawned but before all reader threads
    /// were installed. The start cleanup must cancel, kill, wait, and retain
    /// no detached child/worker handle.
    #[cfg(windows)]
    #[test]
    fn failed_srt_input_start_cleanup_reaps_child_and_worker() {
        use std::os::windows::process::CommandExt;

        let cancellation = WindowsPipeCancellation::new().expect("cancellation event");
        let running = Arc::new(AtomicBool::new(true));
        let status = Arc::new(Mutex::new(SrtInputRuntimeStatus::default()));
        let mut command = Command::new("powershell.exe");
        command
            .creation_flags(0x0800_0000)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = Arc::new(Mutex::new(Some(
            spawn_managed_ffmpeg(&mut command).expect("test child"),
        )));
        let worker_cancellation = Arc::clone(&cancellation);
        let mut video = Some(
            ManagedThread::spawn("qlisa-srt-input-test-worker".into(), move || {
                while !worker_cancellation.is_cancelled() {
                    thread::sleep(Duration::from_millis(1));
                }
            })
            .expect("test worker"),
        );
        let mut audio = None;
        let mut stderr = None;

        cleanup_failed_srt_input_start(
            &running,
            &cancellation,
            &child,
            &status,
            &mut video,
            &mut audio,
            &mut stderr,
        );

        assert!(!running.load(Ordering::Acquire));
        assert!(child.lock().unwrap().is_none(), "child must be reaped");
        assert!(video.as_ref().is_some_and(ManagedThread::is_finished));
        assert!(status.lock().unwrap().child_exit.is_some());
    }

    /// Dropping one managed child closes only its private Job Object and
    /// promptly kills that process; this bounded test uses no FFmpeg/SRT.
    #[cfg(windows)]
    #[test]
    fn dropping_managed_child_kills_its_job_members() {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, WaitForSingleObject,
        };

        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x0000_1000;
        const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
        const WAIT_OBJECT_0: u32 = 0;
        const WAIT_TIMEOUT: u32 = 258;
        const STILL_ACTIVE: u32 = 259;

        struct TestProcessHandle(windows_sys::Win32::Foundation::HANDLE);
        impl Drop for TestProcessHandle {
            fn drop(&mut self) {
                unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
            }
        }

        let mut command = Command::new("powershell.exe");
        command
            .creation_flags(0x0800_0000)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = spawn_managed_ffmpeg(&mut command).expect("managed sleep child");
        let process_handle = TestProcessHandle(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE_ACCESS,
                0,
                child.id(),
            )
        });
        assert_ne!(process_handle.0, 0, "open process handle for bounded wait");
        assert!(
            child
                .try_wait()
                .expect("check child is still active")
                .is_none(),
            "sleep process must still be active before dropping its Job Object"
        );
        assert_eq!(
            unsafe { WaitForSingleObject(process_handle.0, 0) },
            WAIT_TIMEOUT,
            "process handle must be unsignaled before job close"
        );

        drop(child);
        let wait = unsafe { WaitForSingleObject(process_handle.0, 3_000) };
        assert_eq!(wait, WAIT_OBJECT_0, "job close should terminate its child");
        let mut exit_code = 0;
        assert_ne!(
            unsafe { GetExitCodeProcess(process_handle.0, &mut exit_code) },
            0,
            "query terminated process state"
        );
        assert_ne!(
            exit_code, STILL_ACTIVE,
            "no matching process remains active"
        );
    }

    /// Exercise the critical stop-before-connect path with a real child: no
    /// contribution connects, so both FFmpeg output pipes remain in their
    /// cancellable `ConnectNamedPipe` waits until `stop` wakes and joins them.
    #[cfg(windows)]
    #[test]
    fn srt_input_stop_reaps_child_and_waiting_pipe_readers() {
        let (producer, _consumer) = ringbuf::HeapRb::<f32>::new(1024).split();
        let worker = SrtInputWorker::start(
            &listener_settings(unused_loopback_port()),
            48_000,
            producer.into(),
        )
        .expect("start SRT listener worker");
        worker.stop();
        assert!(
            worker.child.lock().unwrap().is_none(),
            "child must be reaped on stop"
        );
        assert!(worker
            .video_reader
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(ManagedThread::is_finished));
        assert!(worker
            .audio_reader
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(ManagedThread::is_finished));
        assert!(worker
            .stderr_drain
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(ManagedThread::is_finished));
    }

    /// Manual end-to-end smoke test for the exact single-session topology. It
    /// needs the bundled FFmpeg binary, an available local UDP port, and the
    /// Windows SRT stack, so CI keeps it opt-in and bounded.
    #[cfg(windows)]
    #[test]
    #[ignore = "bounded local FFmpeg testsrc+sine SRT smoke test"]
    fn local_ffmpeg_srt_testsrc_and_sine_reaches_one_unified_worker() {
        let port = unused_loopback_port();
        let listener = listener_settings(port);
        let caller = SrtSettings {
            enabled: true,
            mode: SrtMode::Caller,
            host: "127.0.0.1".into(),
            port,
            ..Default::default()
        };
        let (producer, _consumer) = ringbuf::HeapRb::<f32>::new(48_000).split();
        let worker =
            SrtInputWorker::start(&listener, 48_000, producer.into()).expect("start listener");
        let ffmpeg = find_ffmpeg_runtime().expect("bundled FFmpeg");
        let mut source = Command::new(ffmpeg);
        use std::os::windows::process::CommandExt;
        source
            .creation_flags(0x0800_0000)
            .args([
                "-hide_banner",
                "-loglevel",
                "warning",
                "-nostdin",
                // Without pacing, two seconds of lavfi media are encoded and
                // the caller closes before the SRT listener's latency buffer
                // can deliver its first packet. This is a transport smoke
                // test, so emit it as a live sender would.
                "-re",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x48:rate=10",
                "-re",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:sample_rate=48000",
                "-t",
                "2",
                "-c:v",
                "mpeg4",
                "-c:a",
                "aac",
                "-f",
                "mpegts",
                &caller.url().expect("caller URL"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut source = spawn_managed_ffmpeg(&mut source).expect("start local SRT sender");
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut received_both = false;
        let mut last_status = worker.status();
        while Instant::now() < deadline {
            let status = worker.status();
            last_status = status.clone();
            if status.received_frames > 0 && status.received_audio_samples > 0 {
                received_both = true;
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let source_exit = source.try_wait().ok().flatten();
        worker.stop();
        if source.try_wait().ok().flatten().is_none() {
            let _ = source.kill();
        }
        let _ = source.wait();
        assert!(
            received_both,
            "unified SRT worker did not receive both test streams; source_exit={source_exit:?}; \
             redacted_worker_status={last_status:?}"
        );
    }

    #[test]
    fn srt_provider_status_discloses_platform_audio_capability() {
        let detail = srt_provider_detail();
        #[cfg(windows)]
        assert!(detail.contains("video and audio"));
        #[cfg(not(windows))]
        assert!(detail.contains("video-only"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_named_pipe_stop_before_connect_is_persistent() {
        let cancellation = WindowsPipeCancellation::new().unwrap();
        let mut pipe = WindowsPipeServer::create(
            "stop-before-connect",
            WindowsPipeDirection::Outbound,
            Arc::clone(&cancellation),
        )
        .unwrap();

        // A stop that wins before the worker reaches ConnectNamedPipe must
        // remain visible; there is no one-shot cancellation window.
        cancellation.cancel();
        assert_eq!(
            pipe.connect().unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_named_pipe_child_failure_unblocks_output_and_input_connects() {
        for (kind, direction) in [
            ("child-failure-output", WindowsPipeDirection::Outbound),
            ("child-failure-input", WindowsPipeDirection::Inbound),
        ] {
            let cancellation = WindowsPipeCancellation::new().unwrap();
            let pipe =
                WindowsPipeServer::create(kind, direction, Arc::clone(&cancellation)).unwrap();
            let (result_tx, result_rx) = std::sync::mpsc::channel();
            let worker = thread::spawn(move || {
                let mut pipe = pipe;
                let _ = result_tx.send(pipe.connect().map_err(|error| error.kind()));
            });

            // No client opens the pipe. This is the same state a worker is in
            // when FFmpeg dies before opening its endpoint; cancelling from
            // the owner must finish both program output and SRT-input reader.
            thread::sleep(Duration::from_millis(20));
            cancellation.cancel();
            assert_eq!(
                result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                Err(std::io::ErrorKind::Interrupted)
            );
            worker.join().unwrap();
        }
    }

    #[test]
    fn srt_settings_reject_unsafe_or_invalid_values() {
        assert!(SrtSettings {
            mode: SrtMode::Caller,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(SrtSettings {
            host: "host".into(),
            passphrase: Some("short".into()),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(SrtSettings {
            host: "host".into(),
            payload_size: Some(1457),
            ..Default::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn network_output_validates_only_enabled_transports() {
        let settings = NetworkOutputSettings {
            ndi: NdiOutputSettings {
                enabled: true,
                stream_name: " ".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn padded_bgra_frame_is_repacked_for_ffmpeg() {
        let frame = BgraFrame {
            width: 2,
            height: 2,
            stride: 12,
            data: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 99, 99, 99, 99, 9, 10, 11, 12, 13, 14, 15, 16, 88, 88, 88,
                88,
            ],
        };
        assert_eq!(
            frame.packed_bgra().unwrap(),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
    }

    #[test]
    fn low_bandwidth_downscale_fits_bounds_and_samples_deterministically() {
        let frame = BgraFrame {
            width: 4,
            height: 2,
            stride: 20,
            data: vec![
                1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255, 99, 99, 99, 99, 5, 0, 0,
                255, 6, 0, 0, 255, 7, 0, 0, 255, 8, 0, 0, 255, 88, 88, 88, 88,
            ],
        };
        let reduced = frame.downscaled_to_fit(2, 2).unwrap();
        assert_eq!((reduced.width, reduced.height, reduced.stride), (2, 1, 8));
        assert_eq!(reduced.data, vec![1, 0, 0, 255, 3, 0, 0, 255]);
        assert_eq!(frame.downscaled_to_fit(4, 2).unwrap(), frame);
    }

    #[test]
    fn low_bandwidth_uses_a_720p_cap_without_upscaling() {
        let frame = BgraFrame {
            width: 1920,
            height: 1080,
            stride: 1920 * 4,
            data: vec![0; 1920 * 1080 * 4],
        };
        let reduced = frame.downscaled_to_fit(1280, 720).unwrap();
        assert_eq!((reduced.width, reduced.height), (1280, 720));
    }

    #[test]
    fn ndi_receiver_quality_uses_the_sdk_bandwidth_values() {
        assert_eq!(ndi_receiver_bandwidth(NdiQuality::Highest), 100);
        assert_eq!(ndi_receiver_bandwidth(NdiQuality::LowBandwidth), 0);
    }

    fn collect_ndi_resampled(
        resampler: &mut NdiStereoResampler,
        sample_rate: u32,
        channels: u32,
        samples: &[f32],
    ) -> Vec<[f32; 2]> {
        let mut output = Vec::new();
        assert!(
            resampler.process_interleaved(sample_rate, channels, samples, |frame| {
                output.push(frame);
            })
        );
        output
    }

    #[test]
    fn ndi_stereo_resampler_is_split_invariant_across_packet_boundaries() {
        let source_rate = 48_000;
        let target_rate = 44_100;
        let frames = 10_007_usize;
        let samples: Vec<f32> = (0..frames)
            .flat_map(|frame| {
                let value = frame as f32 / frames as f32;
                [value, -value]
            })
            .collect();

        let mut whole_resampler = NdiStereoResampler::new(target_rate);
        let whole = collect_ndi_resampled(&mut whole_resampler, source_rate, 2, &samples);

        let mut split_resampler = NdiStereoResampler::new(target_rate);
        let mut split = Vec::new();
        let chunk_frames = [1_usize, 7, 1_024, 3, 511, 29, 2_047];
        let mut first_frame = 0;
        let mut chunk_index = 0;
        while first_frame < frames {
            let end_frame =
                (first_frame + chunk_frames[chunk_index % chunk_frames.len()]).min(frames);
            assert!(split_resampler.process_interleaved(
                source_rate,
                2,
                &samples[first_frame * 2..end_frame * 2],
                |frame| split.push(frame),
            ));
            first_frame = end_frame;
            chunk_index += 1;
        }

        assert_eq!(split, whole);
        assert!(split.windows(2).all(|pair| {
            pair[1][0] >= pair[0][0] && (pair[1][0] + pair[1][1]).abs() < 0.000_001
        }));
    }

    #[test]
    fn ndi_stereo_resampler_has_exact_long_run_counts_at_48k_and_44k1() {
        fn count_after(
            resampler: &mut NdiStereoResampler,
            source_rate: u32,
            frames: usize,
        ) -> usize {
            let samples = vec![0.0_f32; frames * 2];
            let mut count = 0;
            assert!(resampler.process_interleaved(source_rate, 2, &samples, |_| count += 1));
            count
        }

        let mut down = NdiStereoResampler::new(44_100);
        let first_down_second = count_after(&mut down, 48_000, 48_000);
        let second_down_second = count_after(&mut down, 48_000, 48_000);
        assert_eq!(first_down_second, 44_100);
        assert_eq!(second_down_second, 44_100);

        let mut up = NdiStereoResampler::new(48_000);
        let first_up_second = count_after(&mut up, 44_100, 44_100);
        let second_up_second = count_after(&mut up, 44_100, 44_100);
        // Linear interpolation is causal, so the very first second retains one
        // source-sample lookahead. The deficit is constant rather than growing.
        assert_eq!(first_up_second, 47_999);
        assert_eq!(second_up_second, 48_000);
    }

    #[test]
    fn ndi_stereo_resampler_uses_first_pair_duplicates_mono_and_sanitizes() {
        let mut mono = NdiStereoResampler::new(48_000);
        assert_eq!(
            collect_ndi_resampled(&mut mono, 48_000, 1, &[1.0, f32::NAN, f32::INFINITY, -1.0],),
            vec![[1.0, 1.0], [0.0, 0.0], [0.0, 0.0], [-1.0, -1.0]]
        );

        let mut discrete = NdiStereoResampler::new(48_000);
        assert_eq!(
            collect_ndi_resampled(
                &mut discrete,
                48_000,
                4,
                &[1.0, -1.0, 99.0, 98.0, 2.0, -2.0, 97.0, 96.0],
            ),
            vec![[1.0, -1.0], [2.0, -2.0]]
        );
    }

    #[test]
    fn ndi_stereo_resampler_resets_history_when_source_rate_changes() {
        let mut resampler = NdiStereoResampler::new(48_000);
        assert_eq!(
            collect_ndi_resampled(&mut resampler, 48_000, 2, &[1.0, -1.0, 2.0, -2.0]),
            vec![[1.0, -1.0], [2.0, -2.0]]
        );
        assert_eq!(
            collect_ndi_resampled(&mut resampler, 24_000, 2, &[10.0, 10.0, 20.0, 20.0],),
            vec![[10.0, 10.0], [15.0, 15.0], [20.0, 20.0]]
        );
    }

    #[test]
    fn ndi_video_free_guard_releases_frames_on_early_exit() {
        static FREE_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

        unsafe extern "C" fn count_free(_instance: *mut c_void, _frame: *const NdiVideoFrameV2) {
            FREE_CALLS.fetch_add(1, Ordering::Relaxed);
        }

        FREE_CALLS.store(0, Ordering::Relaxed);
        let frame = NdiVideoFrameV2::default();
        {
            let _free = NdiVideoFreeGuard {
                instance: ptr::null_mut(),
                free_video: count_free,
                frame: &frame,
            };
            // Returning an invalid-frame error here drops the guard first.
        }
        assert_eq!(FREE_CALLS.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn external_frame_mailbox_broadcasts_the_latest_frame_to_each_cursor() {
        let mailbox = BgraFrameMailbox::default();
        let first = BgraFrame {
            width: 1,
            height: 1,
            stride: 4,
            data: vec![1, 2, 3, 4],
        };
        let second = BgraFrame {
            width: 1,
            height: 1,
            stride: 4,
            data: vec![5, 6, 7, 8],
        };

        assert!(mailbox.publish(first).is_none());
        let (first_sequence, first_for_a) = mailbox.latest_after(0).unwrap();
        let (first_for_b_sequence, first_for_b) = mailbox.latest_after(0).unwrap();
        assert_eq!(first_sequence, first_for_b_sequence);
        assert_eq!(first_for_a.data, first_for_b.data);

        assert!(mailbox.publish(second).is_some());
        let (second_for_a, frame_for_a) = mailbox.latest_after(first_sequence).unwrap();
        let (second_for_b, frame_for_b) = mailbox.latest_after(first_for_b_sequence).unwrap();
        assert_eq!(second_for_a, second_for_b);
        assert!(second_for_a > first_sequence);
        assert_eq!(frame_for_a.data, vec![5, 6, 7, 8]);
        assert_eq!(frame_for_b.data, frame_for_a.data);
    }

    /// Requires the optional NDI Runtime installed on a Windows test
    /// machine; it is intentionally excluded from portable CI.
    #[test]
    #[ignore = "requires the optional NDI Runtime"]
    fn ndi_runtime_can_create_sender_and_accept_a_bgra_frame() {
        let runtime = NdiRuntime::load().unwrap();
        let mut sender = runtime
            .create_sender(
                &NdiOutputSettings {
                    stream_name: "Qlisa test".into(),
                    ..Default::default()
                },
                25,
            )
            .unwrap();
        sender
            .send(&BgraFrame {
                width: 2,
                height: 2,
                stride: 8,
                data: vec![0; 16],
            })
            .unwrap();
        sender
            .send_audio_interleaved(48_000, 2, &[0.0, 0.0, 0.1, -0.1])
            .unwrap();
        assert_eq!(sender.submitted_frames(), 1);
    }

    /// Confirms that a finder created after a local sender has had time to
    /// join discovery still observes it. This guards the vMix-facing retry
    /// loop without requiring a particular external sender to be installed.
    #[test]
    #[ignore = "requires the optional NDI Runtime and local multicast discovery"]
    fn ndi_finder_discovers_a_live_local_sender() {
        let runtime = NdiRuntime::load().unwrap();
        let name = format!("Qlisa discovery test {}", std::process::id());
        let mut sender = runtime
            .create_sender(
                &NdiOutputSettings {
                    stream_name: name.clone(),
                    ..Default::default()
                },
                25,
            )
            .unwrap();
        sender
            .send(&BgraFrame {
                width: 2,
                height: 2,
                stride: 8,
                data: vec![0; 16],
            })
            .unwrap();
        let sources = runtime.list_sources(5_000).unwrap();
        // Core NDI prefixes local source names with the computer name, so the
        // configured cue must use the exact discovered string rather than the
        // sender's unqualified display name.
        assert!(
            sources
                .iter()
                .any(|source| source.name.ends_with(&format!("({name})"))),
            "local sender was not discovered: {sources:?}"
        );
    }

    /// Optional live-input probe. Set `QLISA_NDI_TEST_SOURCE` to one of the
    /// exact names returned by `list_sources` (including Core NDI's computer
    /// name prefix) to verify receiver creation and actual decoded video.
    #[test]
    #[ignore = "requires a selected live NDI sender"]
    fn ndi_receiver_captures_from_named_live_source() {
        let source_name = std::env::var("QLISA_NDI_TEST_SOURCE")
            .expect("set QLISA_NDI_TEST_SOURCE to an exact discovered source name");
        let runtime = NdiRuntime::load().unwrap();
        let mut receiver = runtime
            .create_receiver(&source_name, NdiQuality::Highest)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(NdiCapture::Video(frame)) = receiver.capture(250).unwrap() {
                assert!(frame.width > 0 && frame.height > 0);
                return;
            }
        }
        panic!("NDI receiver connected to '{source_name}' but did not receive a video frame");
    }

    /// Optional companion to the video probe above. It proves that Core NDI
    /// exposes actual PCM blocks from the selected live source before Qlisa's
    /// synthetic-input bridge downmixes/resamples them.
    #[test]
    #[ignore = "requires a selected live NDI sender with audio"]
    fn ndi_receiver_captures_audio_from_named_live_source() {
        let source_name = std::env::var("QLISA_NDI_TEST_SOURCE")
            .expect("set QLISA_NDI_TEST_SOURCE to an exact discovered source name");
        let runtime = NdiRuntime::load().unwrap();
        let mut receiver = runtime
            .create_receiver(&source_name, NdiQuality::Highest)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(NdiCapture::Audio(audio)) = receiver.capture(250).unwrap() {
                assert!(audio.sample_rate > 0 && audio.channels > 0 && !audio.samples.is_empty());
                return;
            }
        }
        panic!("NDI receiver connected to '{source_name}' but did not receive an audio block");
    }

    /// Exercises the bundled FFmpeg command line without requiring a second
    /// machine to connect. Full SRT delivery is an integration test with a
    /// caller/listener pair and is not suitable for portable CI.
    #[test]
    #[ignore = "requires the optional bundled FFmpeg runtime"]
    fn bundled_ffmpeg_can_start_an_srt_listener() {
        let sender = SrtFfmpegSender::start(
            &SrtSettings {
                enabled: true,
                port: 19_371,
                ..Default::default()
            },
            2,
            2,
            25,
            48_000,
        )
        .unwrap();
        sender.stop().unwrap();
    }

    #[test]
    fn network_manager_is_bounded_and_reports_worker_lifecycle() {
        let manager = NetworkOutputManager::default();
        manager
            .configure(&[NetworkOutputConfig {
                id: "srt-program".into(),
                ndi: None,
                srt: Some(SrtSettings {
                    enabled: true,
                    ..Default::default()
                }),
            }])
            .unwrap();
        assert_eq!(
            manager.statuses()[0].state,
            NetworkOutputState::WaitingForFrame
        );
        manager.stop_all();
        assert!(manager.statuses().is_empty());
    }

    fn test_ndi_config(id: &str, stream_name: &str) -> NetworkOutputConfig {
        NetworkOutputConfig {
            id: id.into(),
            ndi: Some(NdiOutputSettings {
                enabled: true,
                stream_name: stream_name.into(),
                ..Default::default()
            }),
            srt: None,
        }
    }

    #[test]
    fn network_configuration_match_is_order_independent_but_rate_sensitive() {
        let first = test_ndi_config("first", "First");
        let second = test_ndi_config("second", "Second");
        let mut current = std::collections::HashMap::new();
        current.insert("first".into(), (first.clone(), 48_000));
        current.insert("second".into(), (second.clone(), 48_000));

        assert!(network_configurations_match(
            &[second.clone(), first.clone()],
            48_000,
            &current
        ));
        assert!(!network_configurations_match(
            &[second.clone(), first.clone()],
            44_100,
            &current
        ));
        assert!(!network_configurations_match(
            &[test_ndi_config("first", "Changed"), second],
            48_000,
            &current
        ));
    }

    #[test]
    fn network_manager_reuses_only_a_live_identical_endpoint() {
        let manager = NetworkOutputManager::default();
        let config = test_ndi_config("reuse", "Reuse");
        manager.configure(std::slice::from_ref(&config)).unwrap();
        let first = {
            let endpoints = manager.endpoints.lock().unwrap();
            Arc::clone(endpoints.get("reuse").unwrap())
        };
        assert!(manager.is_current_configuration(std::slice::from_ref(&config), 48_000));

        // Error is a retrying worker state, not a dead worker.  Reapplying
        // identical Preferences must keep the endpoint (and its receiver
        // identity) alive while the transport retries.
        manager
            .endpoints
            .lock()
            .unwrap()
            .get("reuse")
            .unwrap()
            .status
            .lock()
            .unwrap()
            .state = NetworkOutputState::Error;
        assert!(manager.is_current_configuration(std::slice::from_ref(&config), 48_000));

        manager.configure(std::slice::from_ref(&config)).unwrap();
        let second = {
            let endpoints = manager.endpoints.lock().unwrap();
            Arc::clone(endpoints.get("reuse").unwrap())
        };
        assert_eq!(Arc::as_ptr(&first), Arc::as_ptr(&second));

        first.running.store(false, Ordering::Release);
        assert!(!manager.is_current_configuration(std::slice::from_ref(&config), 48_000));
        manager.stop_all();
    }

    #[test]
    fn network_transition_stops_old_workers_before_publishing_new_map() {
        let previous = std::collections::HashMap::from([("old".into(), Arc::new(()))]);
        let next = std::collections::HashMap::from([("new".into(), Arc::new(()))]);
        let events = std::cell::RefCell::new(Vec::new());
        stop_endpoints_before_publish(
            previous,
            next,
            |_, _| events.borrow_mut().push("stop"),
            |next| {
                assert_eq!(&*events.borrow(), &["stop"]);
                assert!(next.contains_key("new"));
                events.borrow_mut().push("publish");
                Ok::<_, ()>(())
            },
        )
        .unwrap();
        assert_eq!(&*events.borrow(), &["stop", "publish"]);
    }

    #[test]
    fn network_manager_rejects_unbounded_destination_fanout() {
        let configs: Vec<_> = (0..=MAX_NETWORK_OUTPUT_DESTINATIONS)
            .map(|index| NetworkOutputConfig {
                id: format!("output-{index}"),
                ndi: Some(NdiOutputSettings {
                    enabled: true,
                    stream_name: format!("Qlisa {index}"),
                    ..Default::default()
                }),
                srt: None,
            })
            .collect();
        let error = NetworkOutputManager::default()
            .configure(&configs)
            .unwrap_err();
        assert!(error.contains("at most"));
    }
}
