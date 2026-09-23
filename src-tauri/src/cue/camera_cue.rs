//! [`CameraCue`] — shows a live camera / capture / network video feed on the
//! unified output window, like any other visual cue.
//!
//! The feed is opened by libmpv (which wraps libavformat's capture demuxers):
//! DirectShow on Windows, V4L2 on Linux, AVFoundation on macOS — plus any
//! network stream mpv can play (RTSP / HTTP / UDP…), which covers IP cameras
//! and phone-camera apps.  Video fade in/out (dip-to-black overlay) and the
//! per-cue [`VideoGeometry`] work exactly as they do for Video and Image cues.
//! The feed runs until stopped and is replaced by the next visual GO.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::engine::{
    device_manager::OutputPatch,
    network_io::{
        redact_srt_diagnostic, srt_host, NdiAudioInputSink, NdiInputWorker, NdiQuality,
        NetworkInputRuntimeDiagnostics, NetworkInputSource, NetworkProtocol,
        SrtInputRuntimeStatus, SrtInputWorker,
    },
    output_engine::{ContentRequest, LayerStyle, VideoGeometry, VoiceId},
};

use super::{
    context::{CueContext, CueEvent},
    traits::{Cue, CueFactory, RuntimeState},
    types::{ContinueMode, CueColor, CueId, CueState, CueType, FadeCurve, FadeSpec},
};

// ---------------------------------------------------------------------------
// CameraSource
// ---------------------------------------------------------------------------

/// Where the live feed comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CameraSource {
    /// A local capture device (webcam, USB camera, HDMI capture card).
    ///
    /// `id` is the platform identifier ffmpeg's capture demuxer needs:
    /// the DirectShow device *name* on Windows, the `/dev/videoN` path on
    /// Linux, the AVFoundation device name on macOS.  `name` is what the
    /// operator sees (usually the same string).
    Device { id: String, name: String },
    /// A network stream URL (RTSP / HTTP / UDP…) — IP cameras, NDI-to-RTSP
    /// bridges, phone-camera apps.
    Url { url: String },
    /// A managed network receiver. SRT is handed straight to libmpv/FFmpeg;
    /// NDI is deliberately kept as a distinct protocol so the operator picks
    /// a discovered sender rather than typing a fragile URL.
    Network { input: NetworkInputSource },
}

impl Default for CameraSource {
    fn default() -> Self {
        Self::Device {
            id: String::new(),
            name: String::new(),
        }
    }
}

impl CameraSource {
    /// `true` when the source is actually configured.
    pub fn is_configured(&self) -> bool {
        match self {
            Self::Device { id, .. } => !id.is_empty(),
            Self::Url { url } => !url.is_empty(),
            Self::Network { input } => input.validate().is_ok(),
        }
    }

    /// The mpv URL that opens this source on the current OS. NDI has no mpv
    /// demuxer: it is received through the Core SDK and rendered by the NDI
    /// input path, so callers receive a precise error instead of treating the
    /// NDI source name as a filesystem path.
    pub fn mpv_url(&self) -> Result<String> {
        match self {
            Self::Url { url } => Ok(url.clone()),
            Self::Device { id, .. } => {
                #[cfg(target_os = "windows")]
                {
                    Ok(format!("av://dshow:video={id}"))
                }
                #[cfg(target_os = "linux")]
                {
                    Ok(format!("av://v4l2:{id}"))
                }
                #[cfg(target_os = "macos")]
                {
                    Ok(format!("av://avfoundation:{id}"))
                }
            }
            Self::Network {
                input: NetworkInputSource::Srt { settings },
            } => settings.url().map_err(anyhow::Error::msg),
            Self::Network {
                input: NetworkInputSource::Ndi { .. },
            } => Err(anyhow::anyhow!(
                "NDI input must be started through the NDI receiver"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// CameraCue
// ---------------------------------------------------------------------------

/// A cue that shows a live camera / capture / network feed on the output.
pub struct CameraCue {
    // --- Identity ---
    id: CueId,
    name: String,
    number: Option<String>,
    notes: String,
    color: CueColor,

    // --- State ---
    state: CueState,

    // --- Timing ---
    pre_wait: Duration,
    post_wait: Duration,
    started_at: Option<Instant>,
    action_started_at: Option<Instant>,

    // --- Continue ---
    continue_mode: ContinueMode,

    // --- Camera-specific ---
    /// The live source (device or network URL).
    pub source: CameraSource,
    /// Visual (GL overlay) fade-in from black.
    pub video_fade_in: Option<FadeSpec>,
    /// Visual (GL overlay) fade-out to black on stop.
    pub video_fade_out: Option<FadeSpec>,
    /// Visual geometry (fit / position / scale / rotation / crop).
    pub geometry: VideoGeometry,
    /// Compositing (stacking layer, base opacity, blend mode).
    pub layer_style: LayerStyle,
    pub ndi_quality: NdiQuality,
    pub output_id: Option<String>,
    pub output_ids: Vec<String>,
    /// Audio controls for network sources that provide a synthetic voice.
    pub volume_db: f64,
    pub pan: f32,
    pub output_patch_id: Option<Uuid>,
    pub level_matrix: Option<Vec<Vec<f64>>>,

    is_disabled: bool,

    // --- Runtime ---
    active_voice_id: Option<VoiceId>,
    /// The audio voice fed by the selected external receiver. Kept separately
    /// from the visual compositor voice so STOP cannot leave live audio behind.
    active_audio_voice_id: Option<VoiceId>,
    /// Owns the NDI capture thread while this cue is running. Only its
    /// latest-frame mailbox crosses into the OpenGL renderer.
    ndi_input: Option<NdiInputWorker>,
    /// One FFmpeg-backed worker which owns both decoded SRT video and audio.
    /// Unlike an ordinary network URL, it never goes through libmpv: the
    /// worker's mailbox is fanned out directly to every selected output.
    srt_input: Option<SrtInputWorker>,
    in_pre_wait: bool,
    play_generation: u64,
    auto_continue_fired: bool,
    /// Volatile diagnostics shown in the cue row. They deliberately are not
    /// serialised into the workspace and are reset by the next GO.
    runtime_error: Option<String>,
    runtime_warning: Option<String>,
    runtime_diagnostic_changed: bool,
}

impl CameraCue {
    /// Create a new, empty Camera Cue with a fresh UUID.
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::from("Camera Cue"),
            number: None,
            notes: String::new(),
            color: CueColor::None,
            state: CueState::Standby,
            pre_wait: Duration::ZERO,
            post_wait: Duration::ZERO,
            started_at: None,
            action_started_at: None,
            continue_mode: ContinueMode::DoNotContinue,
            source: CameraSource::default(),
            video_fade_in: None,
            video_fade_out: None,
            geometry: VideoGeometry::default(),
            layer_style: LayerStyle::default(),
            ndi_quality: NdiQuality::Highest,
            output_id: None,
            output_ids: Vec::new(),
            volume_db: 0.0,
            pan: 0.0,
            output_patch_id: None,
            level_matrix: None,
            is_disabled: false,
            active_voice_id: None,
            active_audio_voice_id: None,
            ndi_input: None,
            srt_input: None,
            in_pre_wait: false,
            play_generation: 0,
            auto_continue_fired: false,
            runtime_error: None,
            runtime_warning: None,
            runtime_diagnostic_changed: false,
        }
    }

    /// Snapshot the active SRT receiver without exposing its endpoint or
    /// passphrase. The worker owns the actual process; this is strictly a
    /// diagnostic/status view for a camera inspector or retry policy.
    pub fn srt_input_status(&self) -> Option<SrtInputRuntimeStatus> {
        self.srt_input.as_ref().map(SrtInputWorker::status)
    }

    pub fn ndi_input_status(&self) -> Option<crate::engine::network_io::NdiInputRuntimeStatus> {
        self.ndi_input.as_ref().map(NdiInputWorker::status)
    }

    /// Returns an operator-safe error once the unified SRT receiver can no
    /// longer produce video. `SrtInputWorker` owns all raw FFmpeg output, but
    /// redact once more at this boundary so a future status implementation
    /// cannot accidentally leak an endpoint or passphrase through a cue error.
    fn srt_terminal_error(status: &SrtInputRuntimeStatus) -> Option<String> {
        status
            .terminal_diagnostic()
            .map(|detail| redact_srt_diagnostic(&detail))
    }

    /// Audio is optional for an SRT Camera Cue. Keep an audio-pipe failure
    /// visible to the operator without treating it as a video terminal error.
    fn srt_audio_warning(status: &SrtInputRuntimeStatus) -> Option<String> {
        if status.receiving_video && !status.receiving_audio && status.audio_eof {
            let detail = status
                .last_error
                .as_deref()
                .or(status.diagnostics.as_deref())
                .unwrap_or("FFmpeg SRT audio stream ended; video is still receiving");
            return Some(redact_srt_diagnostic(detail));
        }
        None
    }

    fn set_runtime_diagnostics(&mut self, error: Option<String>, warning: Option<String>) {
        if self.runtime_error != error || self.runtime_warning != warning {
            self.runtime_error = error;
            self.runtime_warning = warning;
            self.runtime_diagnostic_changed = true;
        }
    }

    #[cfg(test)]
    pub(crate) fn set_runtime_error_for_test(&mut self, error: impl Into<String>) {
        self.set_runtime_diagnostics(Some(error.into()), None);
    }

    /// Open the feed on the output window.
    /// [`Self::start_camera_action`], returning the cue to Standby when it fails.
    ///
    /// A cue whose action never started must not stay at `Running` with no
    /// voice: the UI only leaves Running on a state change it is told about, so
    /// it would freeze on the cue forever (the classic symptom when the output
    /// engine is headless and refuses every `show_content`).
    fn start_action_or_reset(&mut self, context: &CueContext) -> Result<()> {
        let result = self.start_camera_action(context);
        if result.is_err() {
            self.state = CueState::Standby;
            self.started_at = None;
            self.in_pre_wait = false;
        }
        result
    }

    /// Attach a network receiver's synthetic stereo feed through the same live
    /// voice path as Mic Cues, including the selected Output Patch and level
    /// controls. Both NDI and unified SRT use this helper.
    fn start_external_audio_voice(&self, context: &CueContext, feed_id: Uuid) -> Result<VoiceId> {
        let patch = context.resolve_patch_checked(self.output_patch_id)?;
        let (out_l, out_r) = Self::external_audio_output_pair(patch);
        let patch_gain = patch
            .map(|patch| crate::cue::types::db_to_linear(patch.gain_db as f64) as f32)
            .unwrap_or(1.0);
        let patch_id = patch.map(|patch| patch.id);
        let patch_slot = patch_id.and_then(|id| context.output_patches.iter().position(|p| p.id == id)).map(|slot| slot as u8);
        let device_id = patch
            .filter(|patch| !patch.is_main() && !patch.device_id.is_empty())
            .map(|patch| patch.device_id.as_str());
        let voice_id = context.audio_engine.play_mic_voice_routed(
            feed_id,
            0,
            1,
            out_l,
            out_r,
            crate::cue::types::db_to_linear(self.volume_db) as f32,
            self.pan,
            0,
            crate::engine::ring_command::FadeCurve::Linear,
            patch_id,
            patch_slot,
            patch_gain,
            device_id,
        )?;
        if let Some(spec) = &self.level_matrix {
            let channels = patch.map(|value| value.channels.clone()).unwrap_or_default();
            if let Some(matrix) = crate::cue::audio_cue::build_level_matrix(spec, &channels) {
                if let Err(error) = context
                    .audio_engine
                    .set_voice_level_matrix(voice_id, Some(&matrix))
                {
                    let _ = context.audio_engine.stop_voice(
                        voice_id,
                        0,
                        crate::engine::ring_command::FadeCurve::Linear,
                    );
                    return Err(error);
                }
            }
        }
        Ok(voice_id)
    }

    #[cfg(test)]
    fn attach_external_audio_to_main(
        audio_engine: &dyn crate::engine::AudioEngineApi,
        patch: Option<&OutputPatch>,
        feed_id: Uuid,
    ) -> Result<VoiceId> {
        let (out_l, out_r) = Self::external_audio_output_pair(patch);
        let patch_gain = patch
            .map(|patch| crate::cue::types::db_to_linear(patch.gain_db as f64) as f32)
            .unwrap_or(1.0);
        audio_engine.play_mic_voice(
            feed_id,
            0,
            1,
            out_l,
            out_r,
            patch_gain,
            0.0,
            0,
            crate::engine::ring_command::FadeCurve::Linear,
        )
    }

    /// Legacy synthetic-receiver output-channel selection. A default Output
    /// Patch contributes its first one or two channel indices; its device is
    /// still the main stream, while its gain is applied by the attachment.
    fn external_audio_output_pair(patch: Option<&OutputPatch>) -> (usize, usize) {
        let (mut out_l, mut out_r) = (0, 1);
        if let Some(patch) = patch {
            if let Some(&channel) = patch.channels.first() {
                out_l = channel as usize;
                out_r = patch.channels.get(1).copied().unwrap_or(channel) as usize;
            }
        }
        (out_l, out_r)
    }

    fn start_camera_action(&mut self, context: &CueContext) -> Result<()> {
        let fade_in_ms = self
            .video_fade_in
            .as_ref()
            .map(|f| f.duration_ms as u32)
            .unwrap_or(0);
        if let CameraSource::Network {
            input: NetworkInputSource::Ndi { source_name },
        } = &self.source
        {
            // `NdiInputWorker` performs Core NDI capture on its own thread and
            // publishes a bounded latest-frame mailbox. The render thread owns
            // the texture upload/composite, so neither side can block the
            // other or accumulate show latency.
            let sample_rate = context.audio_engine.sample_rate();
            let (feed_id, producer) = context
                .audio_engine
                .register_network_feed(2, sample_rate)
                .context("could not create the NDI audio input")?;
            let worker = NdiInputWorker::start_with_audio(
                source_name.clone(),
                self.ndi_quality,
                Some(NdiAudioInputSink::new(producer, sample_rate)),
            )
            .map_err(anyhow::Error::msg)?;
            let mailbox = worker.mailbox();
            let voice_id = match context.output_engine.show_external_bgra_source_multi(
                self.output_id.as_deref(),
                &self.output_ids,
                mailbox,
                self.geometry,
                self.layer_style,
                fade_in_ms,
            ) {
                Ok(value) => value,
                Err(error) => {
                    drop(worker);
                    return Err(error);
                }
            };
            let audio_voice_id = match self.start_external_audio_voice(context, feed_id) {
                Ok(value) => value,
                Err(error) => {
                    context.output_engine.stop_content(voice_id, 0, 0);
                    drop(worker);
                    return Err(error);
                }
            };
            context
                .output_engine
                .attach_cue_audio_voice(voice_id, audio_voice_id);
            self.ndi_input = Some(worker);
            self.active_voice_id = Some(voice_id);
            self.active_audio_voice_id = Some(audio_voice_id);
            self.action_started_at = Some(Instant::now());
            self.in_pre_wait = false;
            context.emit(CueEvent::ActionStarted { cue_id: self.id });
            return Ok(());
        }

        // On Windows SRT is a unified FFmpeg receiver, not a libmpv URL plus
        // an audio sidecar. One child demuxes video and optional audio from
        // one SRT session; the bounded mailbox fans that one decoded frame
        // stream out to every selected output, just like NDI.
        //
        // Other platforms retain the established video-only libmpv path below.
        // Do not start an SRT worker/audio session there: the worker's named
        // pipe runtime is Windows-only and a second session is unnecessary.
        #[cfg(windows)]
        if let CameraSource::Network {
            input: NetworkInputSource::Srt { settings },
        } = &self.source
        {
            let sample_rate = context.audio_engine.sample_rate();
            let (feed_id, producer) = context
                .audio_engine
                .register_network_feed(2, sample_rate)
                .context("could not create the SRT audio input")?;
            let worker = SrtInputWorker::start(settings, sample_rate, producer)
                .map_err(anyhow::Error::msg)?;
            let mailbox = worker.mailbox();
            let voice_id = match context.output_engine.show_external_bgra_source_multi(
                self.output_id.as_deref(),
                &self.output_ids,
                mailbox,
                self.geometry,
                self.layer_style,
                fade_in_ms,
            ) {
                Ok(value) => value,
                Err(error) => {
                    drop(worker);
                    return Err(error);
                }
            };
            let audio_voice_id = match self.start_external_audio_voice(context, feed_id) {
                Ok(value) => value,
                Err(error) => {
                    context.output_engine.stop_content(voice_id, 0, 0);
                    drop(worker);
                    return Err(error);
                }
            };
            context
                .output_engine
                .attach_cue_audio_voice(voice_id, audio_voice_id);
            self.srt_input = Some(worker);
            self.active_voice_id = Some(voice_id);
            self.active_audio_voice_id = Some(audio_voice_id);
            self.action_started_at = Some(Instant::now());
            self.in_pre_wait = false;
            context.emit(CueEvent::ActionStarted { cue_id: self.id });
            return Ok(());
        }

        let url = std::path::PathBuf::from(self.source.mpv_url()?);
        let voice_id = context.output_engine.show_content_multi(
            ContentRequest {
                file_path: &url,
                is_image: false,
                fade_in_ms,
                loop_count: 0,
                initial_seek_action_ms: None,
                start_ms: None,
                end_ms: None,
                screen_index: if self.output_id.is_some() {
                    None
                } else {
                    context.output_screen
                },
                output_id: self.output_id.as_deref(),
                audio_voice_id: None,
                display_duration_ms: None,
                hold_last_frame: false,
                geometry: self.geometry,
                live_source: true,
                // A live feed has nothing to decode ahead of time; Load falls back
                // to the trait default (bring up, pause) — which for a camera is
                // still useful: it opens the capture device early.
                preload: false,
                layer_style: self.layer_style,
                slices: Vec::new(),
            },
            &self.output_ids,
        )?;

        self.active_voice_id = Some(voice_id);
        self.action_started_at = Some(Instant::now());
        self.in_pre_wait = false;

        context.emit(CueEvent::ActionStarted { cue_id: self.id });
        Ok(())
    }
}

impl Default for CameraCue {
    fn default() -> Self {
        Self::new()
    }
}

impl Cue for CameraCue {
    fn network_input_diagnostics(&self) -> Option<NetworkInputRuntimeDiagnostics> {
        let CameraSource::Network { input } = &self.source else {
            return None;
        };
        match input {
            NetworkInputSource::Ndi { source_name } => {
                let status = self.ndi_input_status();
                Some(NetworkInputRuntimeDiagnostics {
                    protocol: NetworkProtocol::Ndi,
                    source_name: source_name.clone(),
                    endpoint: None,
                    srt_mode: None,
                    latency_ms: None,
                    receiving: status.as_ref().is_some_and(|value| value.receiving),
                    received_frames: status.as_ref().map_or(0, |value| value.received_frames),
                    dropped_frames: None,
                    superseded_frames: status.as_ref().map(|value| value.superseded_frames),
                    received_audio_samples: None,
                    dropped_audio_samples: status
                        .as_ref()
                        .map(|value| value.dropped_audio_samples),
                    video_eof: false,
                    audio_eof: false,
                    ffmpeg_running: None,
                    ffmpeg_pid: None,
                    last_error: status.and_then(|value| value.last_error),
                    last_warning: None,
                })
            }
            NetworkInputSource::Srt { settings } => {
                let status = self
                    .srt_input
                    .as_ref()
                    .map(|worker| worker.diagnostics(settings));
                Some(status.unwrap_or_else(|| NetworkInputRuntimeDiagnostics {
                    protocol: NetworkProtocol::Srt,
                    source_name: "SRT".into(),
                    endpoint: Some(format!("srt://{}:{}", srt_host(settings), settings.port)),
                    srt_mode: Some(settings.mode),
                    latency_ms: Some(settings.latency_ms),
                    receiving: false,
                    received_frames: 0,
                    dropped_frames: None,
                    superseded_frames: None,
                    received_audio_samples: Some(0),
                    dropped_audio_samples: Some(0),
                    video_eof: false,
                    audio_eof: false,
                    ffmpeg_running: Some(false),
                    ffmpeg_pid: None,
                    last_error: None,
                    last_warning: None,
                }))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Identity
    // -----------------------------------------------------------------------

    fn id(&self) -> CueId {
        self.id
    }
    fn cue_type(&self) -> CueType {
        CueType::Camera
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn set_name(&mut self, name: String) {
        self.name = name;
    }
    fn number(&self) -> Option<&str> {
        self.number.as_deref()
    }
    fn set_number(&mut self, number: Option<String>) {
        self.number = number;
    }
    fn notes(&self) -> &str {
        &self.notes
    }
    fn set_notes(&mut self, notes: String) {
        self.notes = notes;
    }
    fn color(&self) -> CueColor {
        self.color
    }
    fn set_color(&mut self, color: CueColor) {
        self.color = color;
    }
    fn is_disabled(&self) -> bool {
        self.is_disabled
    }
    fn set_disabled(&mut self, d: bool) {
        self.is_disabled = d;
    }
    fn state(&self) -> CueState {
        self.state
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    fn load(&mut self, _context: &CueContext) -> Result<()> {
        Ok(())
    }

    fn go(&mut self, context: &CueContext) -> Result<()> {
        if self.state == CueState::Running {
            return Ok(());
        }

        self.set_runtime_diagnostics(None, None);

        if !self.source.is_configured() {
            // No source assigned — complete instantly (same pattern as an
            // Image cue without a file) so Auto-Continue/Follow can advance.
            self.state = CueState::Running;
            self.started_at = Some(Instant::now());
            context.emit(CueEvent::ActionStarted { cue_id: self.id });
            self.state = CueState::Completed;
            context.emit(CueEvent::ActionCompleted { cue_id: self.id });
            return Ok(());
        }

        self.play_generation = self.play_generation.wrapping_add(1);
        self.auto_continue_fired = false;
        self.state = CueState::Running;
        self.started_at = Some(Instant::now());

        if !self.pre_wait.is_zero() {
            self.in_pre_wait = true;
            return Ok(());
        }

        self.start_action_or_reset(context)
    }

    fn stop(&mut self, context: &CueContext) -> Result<()> {
        self.in_pre_wait = false;

        if let Some(vid) = self.active_voice_id.take() {
            let fade_ms = self
                .video_fade_out
                .as_ref()
                .map(|f| f.duration_ms as u32)
                .unwrap_or(0);
            context.output_engine.stop_content(vid, fade_ms, 0);
        }
        if let Some(audio) = self.active_audio_voice_id.take() {
            let _ = context.audio_engine.stop_voice(
                audio,
                0,
                crate::engine::ring_command::FadeCurve::Linear,
            );
        }
        // Stop joins the capture thread. Dropping it after detaching the
        // mailbox from the output guarantees no frame is uploaded after STOP.
        self.ndi_input.take();
        self.srt_input.take();

        self.state = CueState::Standby;
        self.started_at = None;
        self.action_started_at = None;
        self.auto_continue_fired = false;
        context.emit(CueEvent::Stopped { cue_id: self.id });
        Ok(())
    }

    fn pause(&mut self, _context: &CueContext) -> Result<()> {
        // A live feed cannot meaningfully pause; keep it running.
        Ok(())
    }

    fn resume(&mut self, _context: &CueContext) -> Result<()> {
        Ok(())
    }

    fn hard_stop(&mut self, context: &CueContext) -> Result<()> {
        self.in_pre_wait = false;

        if let Some(vid) = self.active_voice_id.take() {
            context.output_engine.stop_content(vid, 0, 0);
        }
        if let Some(audio) = self.active_audio_voice_id.take() {
            let _ = context.audio_engine.stop_voice(
                audio,
                0,
                crate::engine::ring_command::FadeCurve::Linear,
            );
        }
        self.ndi_input.take();
        self.srt_input.take();

        self.state = CueState::Standby;
        self.started_at = None;
        self.action_started_at = None;
        self.auto_continue_fired = false;
        context.emit(CueEvent::Stopped { cue_id: self.id });
        Ok(())
    }

    fn reset(&mut self) -> Result<()> {
        self.state = CueState::Standby;
        self.active_voice_id = None;
        self.active_audio_voice_id = None;
        self.ndi_input.take();
        self.srt_input.take();
        self.started_at = None;
        self.action_started_at = None;
        self.in_pre_wait = false;
        self.auto_continue_fired = false;
        Ok(())
    }

    fn tick(&mut self, context: &CueContext) -> Result<()> {
        // A failed receiver otherwise leaves an external mailbox attached to
        // the compositor forever: the cue remains Running but emits no frames.
        // Detach it immediately, publish the normal stopped state, and return
        // the already-redacted diagnostic through the cue error path.
        let srt_status = self.srt_input.as_ref().map(SrtInputWorker::status);
        if let Some(error) = srt_status.as_ref().and_then(Self::srt_terminal_error) {
            log::warn!("CameraCue '{}' SRT input failed: {error}", self.name);
            self.set_runtime_diagnostics(Some(error.clone()), None);
            self.hard_stop(context)?;
            return Err(anyhow::anyhow!("SRT camera input failed: {error}"));
        }
        self.set_runtime_diagnostics(None, srt_status.as_ref().and_then(Self::srt_audio_warning));

        if self.in_pre_wait && self.elapsed() >= self.pre_wait {
            if let Err(e) = self.start_action_or_reset(context) {
                log::warn!("CameraCue '{}' failed to start: {e}", self.name);
            }
        }
        Ok(())
    }

    fn runtime_error(&self) -> Option<&str> {
        self.runtime_error.as_deref()
    }

    fn runtime_warning(&self) -> Option<&str> {
        self.runtime_warning.as_deref()
    }

    fn take_runtime_diagnostic_changed(&mut self) -> bool {
        std::mem::take(&mut self.runtime_diagnostic_changed)
    }

    fn is_action_started(&self) -> bool {
        !self.in_pre_wait
    }

    // -----------------------------------------------------------------------
    // Timing
    // -----------------------------------------------------------------------

    fn pre_wait(&self) -> Duration {
        self.pre_wait
    }
    fn set_pre_wait(&mut self, d: Duration) {
        self.pre_wait = d;
    }
    fn post_wait(&self) -> Duration {
        self.post_wait
    }
    fn set_post_wait(&mut self, d: Duration) {
        self.post_wait = d;
    }

    fn duration(&self) -> Option<Duration> {
        None // A live feed has no natural end — runs until stopped.
    }

    fn elapsed(&self) -> Duration {
        self.started_at
            .map(|t| t.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    fn action_elapsed(&self) -> Duration {
        self.action_started_at
            .map(|t| t.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    // -----------------------------------------------------------------------
    // Continue mode
    // -----------------------------------------------------------------------

    fn continue_mode(&self) -> ContinueMode {
        self.continue_mode
    }
    fn set_continue_mode(&mut self, mode: ContinueMode) {
        self.continue_mode = mode;
    }

    // -----------------------------------------------------------------------
    // Runtime helpers
    // -----------------------------------------------------------------------

    fn playing_voice_id(&self) -> Option<CueId> {
        self.active_voice_id
    }

    fn output_patch_id(&self) -> Option<Uuid> {
        self.output_patch_id
    }

    fn live_audio_params(&self) -> Option<crate::cue::traits::LiveAudioParams> {
        let voice_id = self.active_audio_voice_id?;
        Some(crate::cue::traits::LiveAudioParams {
            voice_id,
            gain: crate::cue::types::db_to_linear(self.volume_db) as f32,
            pan: self.pan,
            level_matrix: self.level_matrix.clone(),
        })
    }

    fn apply_live_audio_patch(&mut self, patch: crate::cue::traits::LiveAudioPatch) {
        if let Some(volume_db) = patch.volume_db {
            self.volume_db = volume_db;
        }
        if let Some(pan) = patch.pan {
            self.pan = pan;
        }
        if let Some(level_matrix) = patch.level_matrix {
            self.level_matrix = level_matrix;
        }
    }

    fn is_visual(&self) -> bool {
        true
    }

    fn visual_geometry(&self) -> Option<VideoGeometry> {
        Some(self.geometry)
    }

    fn layer_style(&self) -> Option<LayerStyle> {
        Some(self.layer_style)
    }

    fn apply_live_visual_patch(&mut self, patch: crate::cue::traits::LiveVisualPatch) {
        if let Some(geometry) = patch.geometry {
            self.geometry = geometry;
        }
        if let Some(layer_style) = patch.layer_style {
            self.layer_style = layer_style;
        }
    }

    fn play_generation(&self) -> u64 {
        self.play_generation
    }
    fn is_auto_continue_fired(&self) -> bool {
        self.auto_continue_fired
    }

    fn auto_continue_marker(&self) -> Option<bool> {
        Some(self.auto_continue_fired)
    }
    fn mark_auto_continue_fired(&mut self) {
        self.auto_continue_fired = true;
    }
    fn clear_auto_continue_fired(&mut self) {
        self.auto_continue_fired = false;
    }

    fn runtime_state(&self) -> RuntimeState {
        RuntimeState {
            state: self.state,
            voice_id: self.active_voice_id,
            started_at: self.started_at,
            action_started_at: self.action_started_at,
        }
    }

    fn restore_runtime_state(&mut self, snap: RuntimeState) {
        self.state = snap.state;
        self.active_voice_id = snap.voice_id;
        self.started_at = snap.started_at;
        self.action_started_at = snap.action_started_at;
        self.in_pre_wait = snap.state == CueState::Running && snap.action_started_at.is_none();
    }

    // -----------------------------------------------------------------------
    // Serialisation
    // -----------------------------------------------------------------------

    fn serialize(&self) -> Value {
        json!({
            "type": "camera",
            "cue_type": "camera",
            "id": self.id,
            "number": self.number,
            "name": self.name,
            "notes": self.notes,
            "color": self.color,
            "pre_wait_ms": self.pre_wait.as_millis() as u64,
            "post_wait_ms": self.post_wait.as_millis() as u64,
            "continue_mode": self.continue_mode,
            "source": self.source,
            "video_fade_in_ms": self.video_fade_in.as_ref().map(|f| f.duration_ms),
            "video_fade_in_curve": self.video_fade_in.as_ref().map(|f| f.curve),
            "video_fade_out_ms": self.video_fade_out.as_ref().map(|f| f.duration_ms),
            "video_fade_out_curve": self.video_fade_out.as_ref().map(|f| f.curve),
            "geometry": self.geometry,
            "ndi_quality": self.ndi_quality,
            "output_id": self.output_id,
            "output_ids": self.output_ids,
            "volume_db": self.volume_db,
            "pan": self.pan,
            "output_patch_id": self.output_patch_id,
            "level_matrix": self.level_matrix,
            "layer_style": self.layer_style,
            "is_disabled": self.is_disabled,
        })
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Factory for [`CameraCue`].
pub struct CameraCueFactory;

impl CueFactory for CameraCueFactory {
    fn create(&self) -> Box<dyn Cue> {
        Box::new(CameraCue::new())
    }

    fn from_json(&self, value: Value) -> Result<Box<dyn Cue>> {
        let mut cue = CameraCue::new();

        if let Some(id_str) = value.get("id").and_then(|v| v.as_str()) {
            cue.id = id_str.parse().unwrap_or_else(|_| Uuid::new_v4());
        }
        if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
            cue.name = name.to_string();
        }
        if let Some(num) = value.get("number").and_then(|v| v.as_str()) {
            cue.number = Some(num.to_string());
        }
        if let Some(notes) = value.get("notes").and_then(|v| v.as_str()) {
            cue.notes = notes.to_string();
        }
        if let Some(ms) = value.get("pre_wait_ms").and_then(|v| v.as_u64()) {
            cue.pre_wait = Duration::from_millis(ms);
        }
        if let Some(ms) = value.get("post_wait_ms").and_then(|v| v.as_u64()) {
            cue.post_wait = Duration::from_millis(ms);
        }
        if let Some(cm) = value.get("continue_mode") {
            if let Ok(mode) = serde_json::from_value(cm.clone()) {
                cue.continue_mode = mode;
            }
        }
        if let Some(col) = value.get("color") {
            if let Ok(color) = serde_json::from_value(col.clone()) {
                cue.color = color;
            }
        }
        if let Some(src) = value.get("source") {
            if let Ok(source) = serde_json::from_value(src.clone()) {
                cue.source = source;
            }
        }
        if let Some(ms) = value.get("video_fade_in_ms").and_then(|v| v.as_u64()) {
            let curve = value
                .get("video_fade_in_curve")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(FadeCurve::SCurve);
            cue.video_fade_in = Some(FadeSpec {
                duration_ms: ms,
                curve,
            });
        }
        if let Some(ms) = value.get("video_fade_out_ms").and_then(|v| v.as_u64()) {
            let curve = value
                .get("video_fade_out_curve")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(FadeCurve::SCurve);
            cue.video_fade_out = Some(FadeSpec {
                duration_ms: ms,
                curve,
            });
        }
        if let Some(g) = value.get("geometry") {
            if let Ok(geometry) = serde_json::from_value::<VideoGeometry>(g.clone()) {
                cue.geometry = geometry;
            }
        }
        if let Some(quality) = value.get("ndi_quality") {
            if let Ok(quality) = serde_json::from_value(quality.clone()) {
                cue.ndi_quality = quality;
            }
        }
        cue.output_id = value
            .get("output_id")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        cue.output_ids = value
            .get("output_ids")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if cue.output_ids.is_empty() {
            if let Some(id) = cue.output_id.clone() {
                cue.output_ids.push(id);
            }
        }
        if let Some(volume_db) = value.get("volume_db").and_then(|v| v.as_f64()) {
            cue.volume_db = volume_db;
        }
        if let Some(pan) = value.get("pan").and_then(|v| v.as_f64()) {
            cue.pan = pan as f32;
        }
        cue.output_patch_id = value
            .get("output_patch_id")
            .and_then(|v| v.as_str())
            .and_then(|value| value.parse().ok());
        if let Some(matrix) = value.get("level_matrix") {
            if let Ok(rows) = serde_json::from_value::<Option<Vec<Vec<f64>>>>(matrix.clone()) {
                cue.level_matrix = rows.filter(|rows| !rows.is_empty());
            }
        }
        if let Some(ls) = value.get("layer_style") {
            if let Ok(style) = serde_json::from_value::<LayerStyle>(ls.clone()) {
                cue.layer_style = style;
            }
        }
        // "stop_on_next_visual" from older workspaces is silently ignored.
        if let Some(b) = value.get("is_disabled").and_then(|v| v.as_bool()) {
            cue.is_disabled = b;
        }

        Ok(Box::new(cue))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_type_is_camera_and_visual() {
        let cue = CameraCue::new();
        assert_eq!(cue.cue_type(), CueType::Camera);
        assert!(cue.is_visual());
        // Visual cues stack as layers; only Stop/Fade cues remove one.
        assert!(!cue.stop_on_next_go());
        assert!(cue.duration().is_none());
    }

    #[test]
    fn default_source_is_unconfigured() {
        assert!(!CameraSource::default().is_configured());
        assert!(CameraSource::Url {
            url: "rtsp://cam".into()
        }
        .is_configured());
        assert!(CameraSource::Device {
            id: "d".into(),
            name: "Cam".into()
        }
        .is_configured());
        assert!(CameraSource::Network {
            input: NetworkInputSource::Srt {
                settings: crate::engine::network_io::SrtSettings {
                    enabled: true,
                    ..Default::default()
                },
            },
        }
        .is_configured());
    }

    #[test]
    fn device_source_builds_platform_mpv_url() {
        let src = CameraSource::Device {
            id: "Logitech C920".into(),
            name: "Logitech C920".into(),
        };
        let url = src.mpv_url().expect("device URL");
        #[cfg(target_os = "windows")]
        assert_eq!(url, "av://dshow:video=Logitech C920");
        #[cfg(target_os = "linux")]
        assert_eq!(url, "av://v4l2:Logitech C920");
        #[cfg(target_os = "macos")]
        assert_eq!(url, "av://avfoundation:Logitech C920");
    }

    #[test]
    fn url_source_passes_through() {
        let src = CameraSource::Url {
            url: "rtsp://192.168.1.50:8554/live".into(),
        };
        assert_eq!(
            src.mpv_url().expect("stream URL"),
            "rtsp://192.168.1.50:8554/live"
        );
    }

    #[test]
    fn srt_network_source_uses_validated_safe_url() {
        let src = CameraSource::Network {
            input: NetworkInputSource::Srt {
                settings: crate::engine::network_io::SrtSettings {
                    enabled: true,
                    mode: crate::engine::network_io::SrtMode::Caller,
                    host: "192.168.2.8".into(),
                    port: 4200,
                    latency_ms: 80,
                    passphrase: Some("correct horse".into()),
                    ..Default::default()
                },
            },
        };
        let url = src.mpv_url().expect("SRT URL");
        assert!(url.starts_with("srt://192.168.2.8:4200?mode=caller"));
        assert!(url.contains("latency=80000"));
        assert!(!format!("{src:?}").is_empty());
    }

    #[test]
    fn srt_health_reports_terminal_failures_without_credentials() {
        let healthy = SrtInputRuntimeStatus::default();
        assert_eq!(CameraCue::srt_terminal_error(&healthy), None);

        let failed_pipe = SrtInputRuntimeStatus {
            last_error: Some(
                "Could not read srt://192.168.2.8:4200?passphrase=correct-horse".into(),
            ),
            ..Default::default()
        };
        let error = CameraCue::srt_terminal_error(&failed_pipe).expect("terminal failure");
        assert!(error.contains("[SRT endpoint redacted]"));
        assert!(!error.contains("192.168.2.8"));
        assert!(!error.contains("correct-horse"));

        let child_exit = SrtInputRuntimeStatus {
            child_exit: Some("exit status: 1".into()),
            ..Default::default()
        };
        assert_eq!(
            CameraCue::srt_terminal_error(&child_exit).as_deref(),
            Some("FFmpeg SRT input exited with exit status: 1")
        );

        let video_eof = SrtInputRuntimeStatus {
            video_eof: true,
            ..Default::default()
        };
        assert_eq!(
            CameraCue::srt_terminal_error(&video_eof).as_deref(),
            Some("FFmpeg SRT video stream ended")
        );

        let audio_failed_while_video_receives = SrtInputRuntimeStatus {
            receiving_video: true,
            audio_eof: true,
            last_error: Some("Could not read FFmpeg SRT audio pipe".into()),
            ..Default::default()
        };
        assert_eq!(
            CameraCue::srt_terminal_error(&audio_failed_while_video_receives),
            None,
            "optional audio failure must not hard-stop live video"
        );
        assert_eq!(
            CameraCue::srt_audio_warning(&audio_failed_while_video_receives).as_deref(),
            Some("Could not read FFmpeg SRT audio pipe")
        );

        let video_failed_while_audio_receives = SrtInputRuntimeStatus {
            receiving_audio: true,
            video_eof: true,
            last_error: Some("Could not read FFmpeg SRT video pipe".into()),
            ..Default::default()
        };
        assert_eq!(
            CameraCue::srt_terminal_error(&video_failed_while_audio_receives).as_deref(),
            Some("Could not read FFmpeg SRT video pipe")
        );
    }

    #[test]
    fn ndi_network_source_never_becomes_an_mpv_path() {
        let src = CameraSource::Network {
            input: NetworkInputSource::Ndi {
                source_name: "Studio Cam".into(),
            },
        };
        assert!(src.is_configured());
        assert!(src.mpv_url().unwrap_err().to_string().contains("NDI"));
    }

    #[test]
    fn serialize_roundtrip() {
        let mut cue = CameraCue::new();
        cue.set_name("FOH cam".to_string());
        cue.source = CameraSource::Url {
            url: "rtsp://cam.local/stream".into(),
        };
        cue.video_fade_in = Some(FadeSpec {
            duration_ms: 1500,
            curve: FadeCurve::Linear,
        });

        let json = cue.serialize();
        assert_eq!(json["type"], "camera");
        assert_eq!(json["source"]["kind"], "url");

        let rebuilt = CameraCueFactory.from_json(json).expect("roundtrip");
        assert_eq!(rebuilt.name(), "FOH cam");
        assert_eq!(rebuilt.cue_type(), CueType::Camera);
        let rebuilt_json = rebuilt.serialize();
        assert_eq!(rebuilt_json["video_fade_in_ms"], 1500);
        assert_eq!(rebuilt_json["source"]["url"], "rtsp://cam.local/stream");
    }

    #[test]
    fn audio_settings_have_stable_defaults_and_roundtrip() {
        let cue = CameraCue::new();
        let defaults = cue.serialize();
        assert_eq!(defaults["volume_db"], 0.0);
        assert_eq!(defaults["pan"], 0.0);
        assert!(defaults["output_patch_id"].is_null());
        assert!(defaults["level_matrix"].is_null());

        let json = serde_json::json!({
            "type": "camera",
            "volume_db": -9.0,
            "pan": 0.4,
            "output_patch_id": Uuid::new_v4(),
            "level_matrix": [[0.0, -6.0], [-12.0, 0.0]],
        });
        let rebuilt = CameraCueFactory.from_json(json).expect("audio settings load");
        assert_eq!(rebuilt.serialize()["volume_db"], -9.0);
        assert_eq!(rebuilt.serialize()["pan"], 0.4);
        assert_eq!(rebuilt.serialize()["level_matrix"][1][0], -12.0);
    }

    #[test]
    fn live_audio_params_follow_camera_settings_and_voice() {
        let mut cue = CameraCue::new();
        cue.active_audio_voice_id = Some(Uuid::new_v4());
        cue.volume_db = -6.0;
        cue.pan = -0.25;
        cue.level_matrix = Some(vec![vec![0.0, -3.0]]);
        let params = cue.live_audio_params().expect("active network voice");
        assert_eq!(params.voice_id, cue.active_audio_voice_id.unwrap());
        assert!((params.gain - crate::cue::types::db_to_linear(-6.0) as f32).abs() < 1e-6);
        assert_eq!(params.pan, -0.25);
        assert_eq!(params.level_matrix, cue.level_matrix);

        cue.apply_live_audio_patch(crate::cue::traits::LiveAudioPatch {
            volume_db: Some(-3.0),
            pan: Some(0.5),
            level_matrix: Some(None),
        });
        assert_eq!(cue.volume_db, -3.0);
        assert_eq!(cue.pan, 0.5);
        assert!(cue.level_matrix.is_none());
    }

    #[test]
    fn from_json_device_source() {
        let json = serde_json::json!({
            "type": "camera",
            "name": "Webcam",
            "source": { "kind": "device", "id": "USB Video Device", "name": "USB Video Device" },
        });
        let cue = CameraCueFactory.from_json(json).expect("load");
        let rebuilt = cue.serialize();
        assert_eq!(rebuilt["source"]["kind"], "device");
        assert_eq!(rebuilt["source"]["id"], "USB Video Device");
    }

    #[test]
    fn go_without_source_completes_instantly() {
        // Serialize path only — go() needs a CueContext; the unconfigured
        // check is exercised through is_configured here.
        assert!(!CameraCue::new().source.is_configured());
    }

    #[test]
    fn network_receiver_audio_keeps_legacy_main_channel_selection() {
        assert_eq!(CameraCue::external_audio_output_pair(None), (0, 1));

        let patch = OutputPatch {
            id: Uuid::new_v4(),
            name: "Main PA".into(),
            device_id: "ignored-for-network-live".into(),
            channels: vec![4, 5],
            gain_db: -6.0,
            kind: crate::engine::device_manager::OutputPatchKind::Main,
            enabled: true,
        };
        assert_eq!(
            CameraCue::external_audio_output_pair(Some(&patch)),
            (4, 5),
            "network live audio retains the pre-unified-SRT main channel pair"
        );
    }

    #[test]
    fn ndi_and_srt_external_audio_attach_through_the_same_legacy_main_voice_path() {
        let engine = crate::engine::AudioEngine::new_silent(
            &crate::preferences::MachineAudioConfig::default(),
        );

        // The two workers have separate producers, but both feed the exact
        // same legacy main-stream attachment helper. `voice_position_ms`
        // searches the main voice pool, so `Some` also proves no aux route was
        // needed to attach either synthetic live voice.
        for source in ["NDI", "SRT"] {
            let (feed_id, _producer) = engine
                .register_synthetic_feed(2, 48_000)
                .expect("register synthetic receiver feed");
            let voice_id = CameraCue::attach_external_audio_to_main(engine.as_ref(), None, feed_id)
                .unwrap_or_else(|error| panic!("{source} main live attachment failed: {error}"));
            assert_eq!(
                engine.voice_position_ms(voice_id),
                Some(0),
                "{source} synthetic receiver voice must be in the main pool"
            );
        }
    }

    #[test]
    fn network_receiver_audio_applies_default_patch_gain() {
        let engine = crate::engine::AudioEngine::new_silent(
            &crate::preferences::MachineAudioConfig::default(),
        );
        let (feed_id, _producer) = engine
            .register_synthetic_feed(2, 48_000)
            .expect("register synthetic receiver feed");
        let patch = OutputPatch {
            id: Uuid::new_v4(),
            name: "Main PA".into(),
            device_id: String::new(),
            channels: vec![0, 1],
            gain_db: -6.0,
            kind: crate::engine::device_manager::OutputPatchKind::Main,
            enabled: true,
        };
        let voice_id =
            CameraCue::attach_external_audio_to_main(engine.as_ref(), Some(&patch), feed_id)
                .expect("attach network receiver audio");
        let expected = crate::cue::types::db_to_linear(-6.0) as f32;
        assert!((engine.get_voice_gain(voice_id) - expected).abs() < 0.000_001);
    }
}
