//! [`VideoCue`] — plays a video file on the unified [`OutputEngine`] window.
//!
//! The cue delegates actual playback to the [`OutputEngine`], which manages
//! the persistent Win32 + libmpv output window.
//! The lifecycle (go / stop / pause / resume / pre-wait) mirrors [`AudioCue`]
//! exactly, so the Transport and event loop need no special-casing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::engine::output_engine::{ContentRequest, LayerStyle, SurfaceId, VideoGeometry, VoiceId};
use crate::engine::ring_command::FadeCurve as EngineFadeCurve;
use crate::engine::voice::{FadeDirection, FadeState, Voice};

use super::{
    context::{CueContext, CueEvent},
    traits::{
        action_position_from_file_ms, media_position_from_action_ms, Cue, CueFactory,
        RuntimeState, media_bounds_ms,
    },
    types::{
        combine_fade_specs, db_to_linear, eof_fade_remaining_ms, ContinueMode, CueColor, CueId,
        CueState, CueType, FadeCurve, FadeSpec,
    },
};

// ---------------------------------------------------------------------------
// VideoCue
// ---------------------------------------------------------------------------

/// A cue that plays a video file on the unified [`OutputEngine`] output window.
pub struct VideoCue {
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

    // --- Video-specific ---
    /// Path to the video file (relative to the workspace directory).
    pub file_path: Option<PathBuf>,
    /// Playback volume in dB (−60 to +12).
    pub volume_db: f64,
    /// Audio fade-in applied to the decoded audio voice.
    pub fade_in: Option<FadeSpec>,
    /// Audio fade-out applied to the decoded audio voice on stop.
    pub fade_out: Option<FadeSpec>,
    /// Visual (GL overlay) fade-in — independent from audio.
    pub video_fade_in: Option<FadeSpec>,
    /// Visual (GL overlay) fade-out — independent from audio.
    pub video_fade_out: Option<FadeSpec>,
    /// Start playback at this offset into the file.
    pub start_time: Option<Duration>,
    /// Stop playback at this offset into the file.
    pub end_time: Option<Duration>,
    /// Extra loop repetitions (0 = play once, `u32::MAX` = infinite).
    pub loop_count: u32,
    /// Stable named output destination. `None` uses the configured default.
    pub output_id: Option<String>,
    pub output_ids: Vec<String>,
    /// Legacy UUID surface key retained solely for old workspace round trips.
    /// Named routing never converts an id into a UUID.
    pub output_surface_id: Option<SurfaceId>,
    /// Output Patch to route video audio through.  `None` uses the workspace
    /// default patch (or system default if none is configured).
    pub output_patch_id: Option<uuid::Uuid>,
    /// Freeze on the last frame at natural EOF instead of cutting to black.
    pub hold_last_frame: bool,
    /// Visual geometry (fit / position / scale / rotation / crop).
    pub geometry: VideoGeometry,
    /// Compositing (stacking layer, base opacity, blend mode).
    pub layer_style: LayerStyle,
    /// QLab-style slices (markers + per-segment play counts).  Empty = plain
    /// playback.  When present, `loop_count` is ignored.
    pub slices: crate::cue::types::SliceList,

    is_disabled: bool,

    // --- Runtime ---
    /// The video voice ID currently in use, if any.
    active_voice_id: Option<VoiceId>,
    /// The video's audio track, decoded to interleaved f32 by `load()` /
    /// background preload.  `None` when the file has no audio track.
    decoded_samples: Option<Arc<Vec<f32>>>,
    /// Probed audio source used by the bounded streaming Voice path.
    stream_path: Option<PathBuf>,
    stream_source: Option<Arc<crate::cue::media_decode::StreamingAudioSource>>,
    decoded_channels: u16,
    decoded_sample_rate: u32,
    /// Total media duration — set by [`Cue::set_runtime_duration`] when the
    /// surface reports its `loadedmetadata` event.
    cached_duration: Option<Duration>,
    /// `true` between `go()` and the moment the action starts after pre-wait.
    in_pre_wait: bool,
    /// Incremented on every `go()` call.
    play_generation: u64,
    /// Prevents double-firing of Auto-Continue.
    auto_continue_fired: bool,
    /// Elapsed time accumulated before the most recent pause.
    elapsed_before_pause: Duration,
    /// Action-elapsed time accumulated before the most recent pause.
    action_elapsed_before_pause: Duration,
    /// Monotonic action clock used by Auto-Continue (seeking must not change it).
    continue_started_at: Option<Instant>,
    continue_elapsed_before_pause: Duration,
    /// `true` once the natural-end visual fade-out has been triggered for the
    /// current play, so `tick()` fires it exactly once.
    eof_fade_started: bool,
    /// Same, for the natural-end fade-out of the paired audio voice — the two
    /// fades have their own spec, so they arm independently.
    eof_audio_fade_started: bool,
    /// QLab-style crosspoint levels in dB for the video's audio track,
    /// `[input channel][patch channel]`.  Same model as
    /// [`AudioCue::level_matrix`](crate::cue::audio_cue::AudioCue::level_matrix).
    pub level_matrix: Option<Vec<Vec<f64>>>,
    /// Set while `preload()` builds the content request, so it asks the output
    /// engine for a dark, held load instead of a normal one.
    preloading: bool,
    /// `true` between a Load Cue and the Start that releases it: the content
    /// is decoded and paused off-screen, so starting it is a reveal, not a
    /// fresh load.
    preloaded: bool,
    /// One-shot action-time seek supplied by a Number scrub before GO.
    initial_seek_action_ms: Option<u64>,
}

impl VideoCue {
    fn load_legacy_audio(&mut self, path: &Path) -> Result<bool> {
        let Some((samples, channels, sample_rate)) = crate::cue::media_decode::decode_audio_track_legacy(path)? else {
            if let Some(stream) = self.stream_source.take() {
                stream.cancel();
            }
            self.decoded_samples = None;
            self.stream_path = None;
            return Ok(false);
        };
        self.accept_preloaded_audio(Arc::new(samples), channels, sample_rate, Duration::ZERO);
        Ok(true)
    }

    /// Create a new, empty Video Cue with a fresh UUID.
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::from("Video Cue"),
            number: None,
            notes: String::new(),
            color: CueColor::None,
            state: CueState::Standby,
            pre_wait: Duration::ZERO,
            post_wait: Duration::ZERO,
            started_at: None,
            action_started_at: None,
            continue_mode: ContinueMode::DoNotContinue,
            file_path: None,
            volume_db: 0.0,
            fade_in: None,
            fade_out: None,
            video_fade_in: None,
            video_fade_out: None,
            start_time: None,
            end_time: None,
            loop_count: 0,
            output_id: None,
            output_ids: Vec::new(),
            output_surface_id: None,
            output_patch_id: None,
            hold_last_frame: false,
            geometry: VideoGeometry::default(),
            layer_style: LayerStyle::default(),
            slices: crate::cue::types::SliceList::default(),
            is_disabled: false,
            active_voice_id: None,
            decoded_samples: None,
            stream_path: None,
            stream_source: None,
            decoded_channels: 2,
            decoded_sample_rate: 44100,
            cached_duration: None,
            in_pre_wait: false,
            play_generation: 0,
            auto_continue_fired: false,
            elapsed_before_pause: Duration::ZERO,
            action_elapsed_before_pause: Duration::ZERO,
            continue_started_at: None,
            continue_elapsed_before_pause: Duration::ZERO,
            eof_fade_started: false,
            eof_audio_fade_started: false,
            level_matrix: None,
            preloading: false,
            preloaded: false,
            initial_seek_action_ms: None,
        }
    }

    /// Convert a [`FadeCurve`] from the cue layer to the engine layer.
    fn engine_curve(c: FadeCurve) -> EngineFadeCurve {
        match c {
            FadeCurve::Linear => EngineFadeCurve::Linear,
            FadeCurve::SCurve => EngineFadeCurve::SCurve,
            FadeCurve::Exponential => EngineFadeCurve::Exponential,
        }
    }

    /// Resolve the slice segments within the clip window as
    /// `(start_ms, end_ms, play_count)`.  Empty = no slicing.
    fn slice_segments_ms(&self) -> Vec<(u64, u64, u32)> {
        if self.slices.is_empty() {
            return Vec::new();
        }
        let file_ms = self
            .cached_duration
            .map(|d| d.as_millis() as u64)
            .unwrap_or(u64::MAX);
        let clip_start = self.start_time.map(|d| d.as_millis() as u64).unwrap_or(0);
        let clip_end = self
            .end_time
            .map(|d| d.as_millis() as u64)
            .unwrap_or(file_ms)
            .min(file_ms);
        self.slices.segments(clip_start, clip_end)
    }

    /// Trigger the fade-outs that land on the cue's natural end, once the
    /// remaining action time drops inside each fade-out window.
    ///
    /// Without this, `video_fade_out` / `fade_out` only ever applied to
    /// *manual* stops — a video reaching EOF hard-cut to black
    /// (`mpv_events` forces the overlay opaque on END_FILE) and its sound cut
    /// off abruptly.  Picture and sound have their own spec and their own
    /// window, so they are armed independently.  Skipped for infinite loops
    /// (no natural end).
    fn tick_eof_fade(&mut self, context: &CueContext) {
        if self.in_pre_wait || self.loop_count == u32::MAX {
            return;
        }
        let (Some(voice_id), Some(total)) = (self.active_voice_id, self.duration()) else {
            return;
        };
        let action_elapsed = self.action_elapsed();

        // Picture — skipped for hold-last-frame (nothing to fade to).
        if !self.eof_fade_started && !self.hold_last_frame {
            let fade_ms = self
                .video_fade_out
                .as_ref()
                .map(|f| f.duration_ms)
                .unwrap_or(0);
            if let Some(remaining_ms) = eof_fade_remaining_ms(action_elapsed, total, fade_ms) {
                // Fire exactly once per play, whether or not the engine accepted
                // it (a `false` return means another cue took over the output).
                self.eof_fade_started = true;
                context
                    .output_engine
                    .begin_eof_fade_out(voice_id, remaining_ms);
            }
        }

        // Sound — the video's audio track is a normal AudioEngine voice, so it
        // fades exactly as an Audio Cue would.  Held last frames still fade:
        // the sound does reach its end even when the picture freezes.
        if !self.eof_audio_fade_started {
            let Some(fade) = self.fade_out.as_ref() else {
                return;
            };
            let (fade_ms, curve) = (fade.duration_ms, Self::engine_curve(fade.curve));
            let Some(remaining_ms) = eof_fade_remaining_ms(action_elapsed, total, fade_ms) else {
                return;
            };
            self.eof_audio_fade_started = true;
            if let Some(audio_voice) = context.output_engine.video_audio_voice(voice_id) {
                let _ = context
                    .audio_engine
                    .stop_voice(audio_voice, remaining_ms, curve);
            }
        }
    }

    /// Build the audio voice for this video's audio track and submit it to the
    /// AudioEngine in the **paused** state, returning its id.
    ///
    /// The voice carries the cue's volume, fade-in, loop, start/end markers and
    /// Output Patch routing — exactly like an Audio Cue — so video audio gets
    /// the full professional signal path (routing, master volume, VU, fades).
    /// Returns `Ok(None)` when the video has no audio track.
    fn submit_paused_audio(&mut self, context: &CueContext) -> Result<Option<VoiceId>> {
        let gain = db_to_linear(self.volume_db) as f32;
        let mut voice = if let Some(samples) = &self.decoded_samples {
            Voice::new(Arc::clone(samples), self.decoded_channels, self.decoded_sample_rate, gain, 0.0)
        } else if let Some(path) = self.stream_path.clone().or_else(|| self.file_path.clone()) {
            let initial_frame = self.start_time.map(|d| (d.as_secs_f64() * self.decoded_sample_rate as f64) as u64)
                .or_else(|| self.slice_segments_ms().first().map(|(start, _, _)| start * self.decoded_sample_rate as u64 / 1000))
                .unwrap_or(0);
            let probe = if self.stream_path.is_some() {
                Ok(Some(crate::cue::media_decode::AudioStreamInfo {
                    channels: self.decoded_channels,
                    sample_rate: self.decoded_sample_rate,
                    total_frames: self.cached_duration.map(|d| (d.as_secs_f64() * self.decoded_sample_rate as f64) as u64),
                }))
            } else {
                crate::cue::media_decode::probe_audio_track(&path)
            };
            match probe {
                Ok(Some(info)) => self.start_stream_voice(path.clone(), info, initial_frame, gain)?,
                Ok(None) => return Ok(None),
                Err(_) => {
                    if self.load_legacy_audio(&path)? { self.legacy_voice(gain)? } else { return Ok(None); }
                }
            }
        } else {
            return Ok(None);
        };
        if self.loop_count > 0 || !self.slices.is_empty() {
            if let Some(stream) = &voice.stream { stream.keep_worker_for_loop(); }
        }

        voice
            .inner
            .loops_remaining
            .store(self.loop_count, std::sync::atomic::Ordering::Relaxed);

        // Rate defaults to 1.0; SR mismatch is corrected in fill_buffer.

        if let Some(end) = self.end_time {
            let end_frame = (end.as_secs_f64() * self.decoded_sample_rate as f64) as u64;
            // SAFETY: written once before submission; the RT thread never sees
            // this voice until play_voice_paused pushes it.
            unsafe {
                *voice.inner.end_frame.get() = Some(end_frame);
            }
        }
        if let Some(start) = self.start_time {
            let start_frame = (start.as_secs_f64() * self.decoded_sample_rate as f64) as u64;
            voice
                .frame_pos
                .store(start_frame, std::sync::atomic::Ordering::Relaxed);
        }

        // Slice program: the paired audio follows the same segments as the
        // mpv side (which drives them via ab-loop), sample-resolved here.
        {
            let sr = self.decoded_sample_rate as u64;
            let segments: Vec<crate::engine::voice::SliceSegment> = self
                .slice_segments_ms()
                .into_iter()
                .map(|(s, e, count)| crate::engine::voice::SliceSegment {
                    start_frame: s * sr / 1000,
                    end_frame: e * sr / 1000,
                    play_count: count,
                })
                .collect();
            if let Some(program) = crate::engine::voice::SliceProgram::new(segments) {
                voice.frame_pos.store(
                    program.segments[0].start_frame,
                    std::sync::atomic::Ordering::Relaxed,
                );
                // SAFETY: written once before submission.
                unsafe { *voice.inner.slices.get() = Some(program) };
            }
        }

        if let Some(ref fi) = self.fade_in {
            let total = (fi.duration_ms * self.decoded_sample_rate as u64) / 1000;
            // SAFETY: single writer before submission.
            unsafe {
                *voice.inner.fade.get() = Some(FadeState {
                    direction: FadeDirection::In,
                    total_samples: total,
                    elapsed_samples: 0,
                    curve: Self::engine_curve(fi.curve),
                });
            }
        }

        let mut patch_device: Option<String> = None;
        if let Some(patch) = context.resolve_patch_checked(self.output_patch_id)? {
            if let Some(&ch_l) = patch.channels.first() {
                voice.out_l = ch_l as usize;
            }
            if let Some(&ch_r) = patch.channels.get(1) {
                voice.out_r = ch_r as usize;
            } else if let Some(&ch_l) = patch.channels.first() {
                voice.out_r = ch_l as usize;
            }
            voice.patched = !patch.is_main();
            voice.patch_id = Some(patch.id);
            voice.patch_slot = context
                .output_patches
                .iter()
                .position(|p| p.id == patch.id)
                .map(|i| i as u8);
            voice
                .inner
                .set_patch_gain(crate::cue::types::db_to_linear(patch.gain_db as f64) as f32);
            if !patch.is_main() && !patch.device_id.is_empty() {
                patch_device = Some(patch.device_id.clone());
            }
        }

        if let Some(spec) = &self.level_matrix {
            let patch_channels: Vec<u16> = context
                .resolve_patch_checked(self.output_patch_id)?
                .map(|p| p.channels.clone())
                .unwrap_or_default();
            if let Some(matrix) = crate::cue::audio_cue::build_level_matrix(spec, &patch_channels) {
                // SAFETY: written once, before the voice reaches the RT thread.
                unsafe { *voice.inner.level_matrix.get() = Some(matrix) };
            }
        }

        Ok(Some(context.audio_engine.play_voice_paused_routed(
            voice,
            patch_device.as_deref(),
        )?))
    }

    /// Kick off video playback.  Called directly from `go()` when there is no
    /// pre-wait, or from `tick()` once the pre-wait timer has elapsed.
    /// [`Self::start_video_action`], returning the cue to Standby when it fails.
    ///
    /// A cue whose action never started must not stay at `Running` with no
    /// voice: the UI only leaves Running on a state change it is told about, so
    /// it would freeze on the cue forever (the classic symptom when the output
    /// engine is headless and refuses every `show_content`).
    fn start_action_or_reset(&mut self, context: &CueContext) -> Result<()> {
        let result = self.start_video_action(context);
        if result.is_err() {
            self.state = CueState::Standby;
            self.started_at = None;
            self.in_pre_wait = false;
        }
        result
    }

    fn start_video_action(&mut self, context: &CueContext) -> Result<()> {
        let start_ms = self.start_time.map(|d| d.as_millis() as u64);
        let end_ms = self.end_time.map(|d| d.as_millis() as u64);
        let fade_in_ms = self
            .video_fade_in
            .as_ref()
            .map(|f| f.duration_ms as u32)
            .unwrap_or(0);

        let path = self.file_path.clone().ok_or_else(|| {
            anyhow!(
                "VideoCue '{}': no file assigned — set a file in the inspector",
                self.name
            )
        })?;

        // Submit the audio voice (paused) first so it is ready to resume the
        // instant the video's first frame is presented.  The path check above
        // intentionally precedes this allocation so a malformed cue cannot
        // leave an audio voice behind.
        let audio_voice_id = self.submit_paused_audio(context)?;

        let slices: Vec<(f64, f64, u32)> = self
            .slice_segments_ms()
            .into_iter()
            .map(|(s, e, count)| (s as f64 / 1000.0, e as f64 / 1000.0, count))
            .collect();

        let voice_id = match context.output_engine.show_content_multi(ContentRequest {
            file_path: &path,
            is_image: false,
            fade_in_ms,
            loop_count: self.loop_count,
            initial_seek_action_ms: self.initial_seek_action_ms.take(),
            start_ms,
            end_ms,
            screen_index: if self.output_id.is_some() {
                None
            } else {
                context.output_screen
            },
            output_id: self.output_id.as_deref(),
            audio_voice_id,
            display_duration_ms: None,
            hold_last_frame: self.hold_last_frame,
            geometry: self.geometry,
            live_source: false,
            layer_style: self.layer_style,
            slices,
            preload: self.preloading,
        }, &self.output_ids) {
            Ok(voice_id) => voice_id,
            Err(error) => {
                // The audio voice is submitted before the visual pipeline so
                // it can start in lockstep with the first frame.  If output
                // routing fails (for example a disconnected named monitor),
                // do not leave that paused voice orphaned in AudioEngine.
                if let Some(audio_id) = audio_voice_id {
                    let _ = context.audio_engine.stop_voice(
                        audio_id,
                        0,
                        EngineFadeCurve::Linear,
                    );
                }
                return Err(error);
            }
        };

        self.active_voice_id = Some(voice_id);
        self.action_started_at = Some(Instant::now());
        self.continue_started_at = self.action_started_at;
        self.continue_elapsed_before_pause = Duration::ZERO;
        self.in_pre_wait = false;
        self.eof_fade_started = false;
        self.eof_audio_fade_started = false;
        self.preloaded = false;

        context.emit(CueEvent::ActionStarted { cue_id: self.id });
        Ok(())
    }
}

impl Default for VideoCue {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoCue {
    fn start_stream_source(
        &mut self,
        path: PathBuf,
        info: crate::cue::media_decode::AudioStreamInfo,
    ) -> Result<()> {
        let source = crate::cue::media_decode::StreamingAudioSource::start(path, info)?;
        if self.loop_count > 0 || !self.slices.is_empty() {
            source.keep_worker_for_loop();
        }
        self.stream_source = Some(source);
        Ok(())
    }

    fn start_stream_voice(
        &mut self,
        path: PathBuf,
        info: crate::cue::media_decode::AudioStreamInfo,
        initial_frame: u64,
        gain: f32,
    ) -> Result<Voice> {
        let stream_result = match self.stream_source.clone().filter(|stream| {
            !stream.is_cancelled() && (!stream.is_eof() || stream.buffered_samples() > 0 || self.loop_count > 0)
        }) {
            Some(stream) => {
                if initial_frame > 0 { stream.prepare_seek(initial_frame).map(|_| stream) } else { Ok(stream) }
            }
            None => crate::cue::media_decode::StreamingAudioSource::start_at(path, info, initial_frame),
        }?;
        self.stream_source = Some(Arc::clone(&stream_result));
        Ok(Voice::new_stream(stream_result, gain, 0.0))
    }

    fn legacy_voice(&self, gain: f32) -> Result<Voice> {
        let samples = self
            .decoded_samples
            .as_ref()
            .ok_or_else(|| anyhow!("legacy video audio fallback produced no PCM"))?;
        Ok(Voice::new(Arc::clone(samples), self.decoded_channels, self.decoded_sample_rate, gain, 0.0))
    }
}

impl Cue for VideoCue {
    // -----------------------------------------------------------------------
    // Identity
    // -----------------------------------------------------------------------

    fn id(&self) -> CueId {
        self.id
    }
    fn cue_type(&self) -> CueType {
        CueType::Video
    }
    fn output_patch_id(&self) -> Option<uuid::Uuid> {
        self.output_patch_id
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

    fn is_preloaded_or_loading(&self) -> bool {
        self.preloading || self.preloaded
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    fn load(&mut self, _context: &CueContext) -> Result<()> {
        // The video frames stream directly from disk via the OutputEngine, but
        // the audio track must be decoded so it can play as an AudioEngine voice
        // in sync with the (muted) video.
        let path = match &self.file_path {
            Some(p) => p.clone(),
            None => return Ok(()),
        };
        let probe = crate::cue::media_decode::probe_audio_track(&path);
        if crate::cue::media_decode::should_use_legacy_fallback(&probe) {
            self.load_legacy_audio(&path)?;
            return Ok(());
        }
        match probe {
            Ok(Some(info)) => {
                self.accept_preloaded_stream(
                    path.clone(),
                    info.channels,
                    info.sample_rate,
                    info.total_frames.map(|frames| Duration::from_secs_f64(frames as f64 / info.sample_rate as f64)),
                );
                self.start_stream_source(path.clone(), info)?;
            }
            Ok(None) => {}
            Err(_) => unreachable!("probe error handled by fallback policy"),
        }
        Ok(())
    }

    fn accept_preloaded_audio(
        &mut self,
        samples: Arc<Vec<f32>>,
        channels: u16,
        sample_rate: u32,
        _duration: Duration,
    ) {
        // Store the decoded audio track.  The video's own duration comes from
        // the mpv probe (set_runtime_duration), so the decoded length is ignored.
        self.decoded_channels = channels;
        self.decoded_sample_rate = sample_rate;
        self.decoded_samples = Some(samples);
        self.stream_path = None;
        if let Some(stream) = self.stream_source.take() { stream.cancel(); }
    }

    fn accept_preloaded_stream(
        &mut self,
        path: PathBuf,
        channels: u16,
        sample_rate: u32,
        _duration: Option<Duration>,
    ) {
        self.stream_path = Some(path);
        if let Some(stream) = self.stream_source.take() { stream.cancel(); }
        self.decoded_samples = None;
        self.decoded_channels = channels.max(1);
        self.decoded_sample_rate = sample_rate.max(1);
    }

    fn go(&mut self, context: &CueContext) -> Result<()> {
        if self.state == CueState::Running {
            return Ok(()); // Ignore duplicate GO.
        }

        let has_file = self
            .file_path
            .as_ref()
            .is_some_and(|p| !p.as_os_str().is_empty());
        if !has_file {
            // No file assigned — nothing to play. Complete instantly (same
            // pattern as MemoCue) so Auto-Continue/Auto-Follow can advance
            // past it instead of getting stuck "running" an empty cue.
            self.state = CueState::Running;
            self.started_at = Some(Instant::now());
            context.emit(CueEvent::ActionStarted { cue_id: self.id });
            self.state = CueState::Completed;
            context.emit(CueEvent::ActionCompleted { cue_id: self.id });
            return Ok(());
        }

        // Already preloaded by a Load Cue: the file is open and frame 0 is
        // decoded off-screen, so starting it is a reveal — reloading would
        // throw away exactly the work the Load was for.
        if self.preloaded {
            return self.resume(context);
        }

        self.play_generation = self.play_generation.wrapping_add(1);
        self.auto_continue_fired = false;
        self.continue_started_at = None;
        self.continue_elapsed_before_pause = Duration::ZERO;
        self.state = CueState::Running;
        self.started_at = Some(Instant::now());

        if !self.pre_wait.is_zero() {
            self.in_pre_wait = true;
            return Ok(());
        }

        self.start_action_or_reset(context)
    }

    fn preload(&mut self, context: &CueContext) -> Result<()> {
        if self.state == CueState::Running || self.preloaded {
            return Ok(());
        }
        let has_file = self
            .file_path
            .as_ref()
            .is_some_and(|p| !p.as_os_str().is_empty());
        if !has_file {
            return Ok(());
        }

        self.play_generation = self.play_generation.wrapping_add(1);
        self.auto_continue_fired = false;

        self.preloading = true;
        let result = self.start_video_action(context);
        self.preloading = false;
        result?;

        // The cue is standing by, not playing: its clocks must not run and the
        // picture is held off-screen until a Start releases it.
        self.state = CueState::Paused;
        self.started_at = None;
        self.action_started_at = None;
        self.elapsed_before_pause = Duration::ZERO;
        self.action_elapsed_before_pause = Duration::ZERO;
        self.continue_started_at = None;
        self.continue_elapsed_before_pause = Duration::ZERO;
        self.preloaded = true;
        Ok(())
    }

    fn stop(&mut self, context: &CueContext) -> Result<()> {
        self.in_pre_wait = false;

        if let Some(vid) = self.active_voice_id.take() {
            let visual_fade_ms = self
                .video_fade_out
                .as_ref()
                .map(|f| f.duration_ms as u32)
                .unwrap_or(0);
            let audio_fade_ms = self
                .fade_out
                .as_ref()
                .map(|f| f.duration_ms as u32)
                .unwrap_or(0);
            context
                .output_engine
                .stop_content(vid, visual_fade_ms, audio_fade_ms);
        }

        self.state = CueState::Standby;
        self.started_at = None;
        self.action_started_at = None;
        self.elapsed_before_pause = Duration::ZERO;
        self.action_elapsed_before_pause = Duration::ZERO;
        self.continue_started_at = None;
        self.continue_elapsed_before_pause = Duration::ZERO;
        self.auto_continue_fired = false;
        self.eof_fade_started = false;
        self.eof_audio_fade_started = false;
        self.preloaded = false;
        context.emit(CueEvent::Stopped { cue_id: self.id });
        Ok(())
    }

    fn go_with_fade(&mut self, context: &CueContext, shared_fade: Option<&FadeSpec>) -> Result<()> {
        if shared_fade.is_none() {
            return self.go(context);
        }
        let authored_audio = self.fade_in.clone();
        let authored_video = self.video_fade_in.clone();
        // A Number envelope is shared by picture and the video's audio track.
        self.fade_in = combine_fade_specs(authored_audio.as_ref(), shared_fade);
        self.video_fade_in = combine_fade_specs(authored_video.as_ref(), shared_fade);
        let result = self.go(context);
        self.fade_in = authored_audio;
        self.video_fade_in = authored_video;
        result
    }

    fn stop_with_fade(&mut self, context: &CueContext, shared_fade: Option<&FadeSpec>) -> Result<()> {
        if shared_fade.is_none() {
            return self.stop(context);
        }
        let authored_audio = self.fade_out.clone();
        let authored_video = self.video_fade_out.clone();
        self.fade_out = combine_fade_specs(authored_audio.as_ref(), shared_fade);
        self.video_fade_out = combine_fade_specs(authored_video.as_ref(), shared_fade);
        let result = self.stop(context);
        self.fade_out = authored_audio;
        self.video_fade_out = authored_video;
        result
    }

    fn pause(&mut self, context: &CueContext) -> Result<()> {
        if self.state != CueState::Running {
            return Ok(());
        }
        if !self.in_pre_wait {
            if let Some(vid) = self.active_voice_id {
                context.output_engine.pause_voice(vid)?;
            }
        }
        if let Some(t) = self.started_at.take() {
            self.elapsed_before_pause = t.elapsed();
        }
        if let Some(t) = self.action_started_at.take() {
            self.action_elapsed_before_pause = t.elapsed();
        }
        if let Some(t) = self.continue_started_at.take() {
            self.continue_elapsed_before_pause = t.elapsed();
        }
        self.state = CueState::Paused;
        Ok(())
    }

    fn resume(&mut self, context: &CueContext) -> Result<()> {
        if self.state != CueState::Paused {
            return Ok(());
        }
        if !self.in_pre_wait {
            if let Some(vid) = self.active_voice_id {
            // Releasing a preload reveals and unpauses in one step; an
            // ordinary pause just unpauses.
                let revealed = self.preloaded && context.output_engine.start_preloaded(vid);
                if !revealed {
                    context.output_engine.resume_voice(vid)?;
                }
            }
        }
        self.preloaded = false;
        let now = Instant::now();
        self.started_at = Some(now - self.elapsed_before_pause);
        if !self.in_pre_wait {
            self.action_started_at = Some(now - self.action_elapsed_before_pause);
            self.continue_started_at = Some(now - self.continue_elapsed_before_pause);
        }
        self.state = CueState::Running;
        Ok(())
    }

    fn seek(&mut self, position_ms: u64, ctx: &CueContext) {
        if self.action_started_at.is_none() && self.state != CueState::Paused {
            return;
        }
        let Some(voice_id) = self.active_voice_id else {
            return;
        };
        let max_action_ms = self.duration().map(|d| d.as_millis() as u64);
        let action_ms = max_action_ms.map_or(position_ms, |max| position_ms.min(max));
        // Keep the seek in action-time coordinates. The output slot may not
        // know the source duration yet (especially immediately after GO), so
        // it must perform trim/loop conversion after FILE_LOADED.
        ctx.output_engine.seek_voice_action_ms(voice_id, action_ms);
        if self.state == CueState::Paused {
            self.action_elapsed_before_pause = Duration::from_millis(action_ms);
            self.elapsed_before_pause = self.pre_wait + Duration::from_millis(action_ms);
        } else {
            self.action_started_at = Some(Instant::now() - Duration::from_millis(action_ms));
        }
    }

    fn set_initial_seek_action_ms(&mut self, position_ms: u64) {
        self.initial_seek_action_ms = Some(position_ms);
    }

    fn seek_file_position(&mut self, file_position_ms: u64, ctx: &CueContext) {
        if self.action_started_at.is_none() && self.state != CueState::Paused {
            return;
        }
        let action_ms = action_position_from_file_ms(
            file_position_ms,
            self.cached_duration,
            self.start_time,
            self.end_time,
            1.0,
        );
        self.seek(action_ms, ctx);
    }

    fn media_position_for_action_ms(&self, action_elapsed: Duration) -> Option<u64> {
        if self.state != CueState::Running && self.state != CueState::Paused {
            return None;
        }
        Some(media_position_from_action_ms(
            action_elapsed,
            self.cached_duration,
            self.start_time,
            self.end_time,
            1.0,
            self.loop_count,
        ))
    }

    fn media_position_ms(
        &self,
        _audio_engine: &crate::engine::AudioEngine,
        output_engine: &crate::engine::OutputEngine,
    ) -> Option<u64> {
        let fallback = self.media_position_for_action_ms(self.action_elapsed());
        let raw = self
            .active_voice_id
            .and_then(|voice_id| output_engine.voice_position_ms(voice_id))
            .or(fallback)?;
        let (start_ms, end_ms) = media_bounds_ms(self.cached_duration, self.start_time, self.end_time);
        Some(raw.clamp(start_ms, end_ms))
    }

    fn hard_stop(&mut self, context: &CueContext) -> Result<()> {
        self.in_pre_wait = false;

        if let Some(vid) = self.active_voice_id.take() {
            let _ = context.output_engine.stop_voice(vid, 0);
        }

        self.state = CueState::Standby;
        self.started_at = None;
        self.action_started_at = None;
        self.elapsed_before_pause = Duration::ZERO;
        self.action_elapsed_before_pause = Duration::ZERO;
        self.continue_started_at = None;
        self.continue_elapsed_before_pause = Duration::ZERO;
        self.auto_continue_fired = false;
        self.eof_fade_started = false;
        self.eof_audio_fade_started = false;
        self.preloaded = false;
        context.emit(CueEvent::Stopped { cue_id: self.id });
        Ok(())
    }

    fn reset(&mut self) -> Result<()> {
        self.state = CueState::Standby;
        self.active_voice_id = None;
        self.started_at = None;
        self.action_started_at = None;
        self.elapsed_before_pause = Duration::ZERO;
        self.action_elapsed_before_pause = Duration::ZERO;
        self.continue_started_at = None;
        self.continue_elapsed_before_pause = Duration::ZERO;
        self.in_pre_wait = false;
        self.auto_continue_fired = false;
        self.eof_fade_started = false;
        self.eof_audio_fade_started = false;
        self.preloaded = false;
        Ok(())
    }

    fn tick(&mut self, context: &CueContext) -> Result<()> {
        // Once the pre-wait timer expires, start the video action.
        if self.in_pre_wait && self.elapsed() >= self.pre_wait {
            if let Err(e) = self.start_action_or_reset(context) {
                log::warn!("VideoCue '{}' failed to start action: {e}", self.name);
            }
        }
        self.tick_eof_fade(context);
        Ok(())
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
        if !self.slices.is_empty() {
            // Sliced playback: a vamp has no fixed duration; finite counts sum.
            let segments = self.slice_segments_ms();
            if segments.is_empty() {
                // Markers all fall outside the clip window — plain playback.
            } else if segments.iter().any(|&(_, _, c)| c == u32::MAX) {
                return None;
            } else {
                let total_ms: u64 = segments.iter().map(|&(s, e, c)| (e - s) * c as u64).sum();
                return Some(Duration::from_millis(total_ms));
            }
        }
        if self.loop_count == u32::MAX {
            return None; // Infinite loop — no fixed duration.
        }
        self.cached_duration.map(|d| {
            let start = self.start_time.unwrap_or(Duration::ZERO);
            let end = self.end_time.unwrap_or(d);
            let base = end.saturating_sub(start);
            base * (self.loop_count + 1)
        })
    }

    fn elapsed(&self) -> Duration {
        if self.state == CueState::Paused {
            return self.elapsed_before_pause;
        }
        self.started_at
            .map(|t| t.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    fn action_elapsed(&self) -> Duration {
        if self.state == CueState::Paused {
            return self.action_elapsed_before_pause;
        }
        self.action_started_at
            .map(|t| t.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    fn auto_continue_elapsed(&self) -> Duration {
        if self.state == CueState::Paused {
            return self.continue_elapsed_before_pause;
        }
        self.continue_started_at
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

    fn extract_decoded_audio(&self) -> Option<(Arc<Vec<f32>>, u16, u32, Duration)> {
        let samples = self.decoded_samples.as_ref()?;
        let duration = self.cached_duration?;
        Some((
            Arc::clone(samples),
            self.decoded_channels,
            self.decoded_sample_rate,
            duration,
        ))
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

    fn media_file_path(&self) -> Option<&std::path::Path> {
        self.file_path.as_deref()
    }

    fn set_runtime_duration(&mut self, duration: Duration) {
        self.cached_duration = Some(duration);
    }

    fn file_duration(&self) -> Option<Duration> {
        self.cached_duration
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

    fn live_audio_params(&self) -> Option<crate::cue::traits::LiveAudioParams> {
        // This is the *visual* voice id; `update_cue` maps it to the paired
        // audio voice before touching the AudioEngine. Sending it as-is is
        // what made inspector volume edits silently do nothing on a playing
        // Video Cue — the AudioEngine had no voice by that id.
        let voice_id = self.active_voice_id?;
        Some(crate::cue::traits::LiveAudioParams {
            voice_id,
            gain: db_to_linear(self.volume_db) as f32,
            pan: 0.0,
            level_matrix: self.level_matrix.clone(),
        })
    }

    fn apply_live_audio_patch(&mut self, patch: crate::cue::traits::LiveAudioPatch) {
        if let Some(volume_db) = patch.volume_db {
            self.volume_db = volume_db;
        }
        if let Some(level_matrix) = patch.level_matrix {
            self.level_matrix = level_matrix;
        }
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

    fn uses_sliced_playback(&self) -> bool {
        !self.slices.is_empty()
    }

    fn is_visual(&self) -> bool {
        true
    }

    // -----------------------------------------------------------------------
    // Serialisation
    // -----------------------------------------------------------------------

    fn serialize(&self) -> Value {
        json!({
            "type": "video",
            "cue_type": "video",
            "id": self.id,
            "number": self.number,
            "name": self.name,
            "notes": self.notes,
            "color": self.color,
            "pre_wait_ms": self.pre_wait.as_millis() as u64,
            "post_wait_ms": self.post_wait.as_millis() as u64,
            "continue_mode": self.continue_mode,
            "file_path": self.file_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            "volume_db": self.volume_db,
            "fade_in_ms": self.fade_in.as_ref().map(|f| f.duration_ms),
            "fade_in_curve": self.fade_in.as_ref().map(|f| f.curve),
            "fade_out_ms": self.fade_out.as_ref().map(|f| f.duration_ms),
            "fade_out_curve": self.fade_out.as_ref().map(|f| f.curve),
            "video_fade_in_ms": self.video_fade_in.as_ref().map(|f| f.duration_ms),
            "video_fade_in_curve": self.video_fade_in.as_ref().map(|f| f.curve),
            "video_fade_out_ms": self.video_fade_out.as_ref().map(|f| f.duration_ms),
            "video_fade_out_curve": self.video_fade_out.as_ref().map(|f| f.curve),
            "start_time_ms": self.start_time.map(|d| d.as_millis() as u64),
            "end_time_ms": self.end_time.map(|d| d.as_millis() as u64),
            "loop_count": self.loop_count,
            "output_id": self.output_id,
            "output_ids": self.output_ids,
            "output_surface_id": self.output_surface_id,
            "output_patch_id": self.output_patch_id,
            "hold_last_frame": self.hold_last_frame,
            "geometry": self.geometry,
            "layer_style": self.layer_style,
            "slices": self.slices,
            "level_matrix": self.level_matrix,
            "is_disabled": self.is_disabled,
            "cached_duration_ms": self.cached_duration.map(|d| d.as_millis() as u64),
        })
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Factory for [`VideoCue`].
pub struct VideoCueFactory;

impl CueFactory for VideoCueFactory {
    fn create(&self) -> Box<dyn Cue> {
        Box::new(VideoCue::new())
    }

    fn from_json(&self, value: Value) -> Result<Box<dyn Cue>> {
        let mut cue = VideoCue::new();

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
        if let Some(path) = value.get("file_path").and_then(|v| v.as_str()) {
            cue.file_path = Some(PathBuf::from(path));
        }
        if let Some(db) = value.get("volume_db").and_then(|v| v.as_f64()) {
            cue.volume_db = db;
        }
        if let Some(ms) = value.get("fade_in_ms").and_then(|v| v.as_u64()) {
            let curve = value
                .get("fade_in_curve")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(FadeCurve::SCurve);
            cue.fade_in = Some(FadeSpec {
                duration_ms: ms,
                curve,
            });
        }
        if let Some(ms) = value.get("fade_out_ms").and_then(|v| v.as_u64()) {
            let curve = value
                .get("fade_out_curve")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(FadeCurve::SCurve);
            cue.fade_out = Some(FadeSpec {
                duration_ms: ms,
                curve,
            });
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
        if let Some(ms) = value.get("start_time_ms").and_then(|v| v.as_u64()) {
            cue.start_time = Some(Duration::from_millis(ms));
        }
        if let Some(ms) = value.get("end_time_ms").and_then(|v| v.as_u64()) {
            cue.end_time = Some(Duration::from_millis(ms));
        }
        if let Some(lc) = value.get("loop_count").and_then(|v| v.as_u64()) {
            cue.loop_count = lc as u32;
        }
        cue.output_id = value
            .get("output_id")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        cue.output_ids = value.get("output_ids").and_then(|v| v.as_array()).map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        if cue.output_ids.is_empty() { if let Some(id) = cue.output_id.clone() { cue.output_ids.push(id); } }
        if let Some(sid_str) = value.get("output_surface_id").and_then(|v| v.as_str()) {
            cue.output_surface_id = sid_str.parse().ok();
        }
        // "screen_index" was a per-cue field in older workspaces; it is now a
        // global preference (DisplayPreferences::output_screen) and is ignored here.
        if let Some(pid_str) = value.get("output_patch_id").and_then(|v| v.as_str()) {
            cue.output_patch_id = pid_str.parse().ok();
        }
        if let Some(b) = value.get("hold_last_frame").and_then(|v| v.as_bool()) {
            cue.hold_last_frame = b;
        }
        if let Some(g) = value.get("geometry") {
            if let Ok(geometry) = serde_json::from_value::<VideoGeometry>(g.clone()) {
                cue.geometry = geometry;
            }
        }
        if let Some(ls) = value.get("layer_style") {
            if let Ok(style) = serde_json::from_value::<LayerStyle>(ls.clone()) {
                cue.layer_style = style;
            }
        }
        if let Some(m) = value.get("level_matrix") {
            if let Ok(rows) = serde_json::from_value::<Option<Vec<Vec<f64>>>>(m.clone()) {
                cue.level_matrix = rows.filter(|r| !r.is_empty());
            }
        }
        if let Some(s) = value.get("slices") {
            if let Ok(mut slices) =
                serde_json::from_value::<crate::cue::types::SliceList>(s.clone())
            {
                slices.normalize();
                cue.slices = slices;
            }
        }
        if let Some(b) = value.get("is_disabled").and_then(|v| v.as_bool()) {
            cue.is_disabled = b;
        }
        if let Some(ms) = value.get("cached_duration_ms").and_then(|v| v.as_u64()) {
            cue.cached_duration = Some(Duration::from_millis(ms));
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
    use crate::engine::output_engine::FitMode;

    #[test]
    fn serialize_roundtrip_geometry_and_hold() {
        let mut cue = VideoCue::new();
        cue.hold_last_frame = true;
        cue.geometry = VideoGeometry {
            fit_mode: FitMode::Fill,
            pan_x: 0.2,
            pan_y: -0.1,
            scale: 1.25,
            rotation: 180,
            crop_left: 0.05,
            crop_right: 0.0,
            crop_top: 0.1,
            crop_bottom: 0.0,
        };

        let json = cue.serialize();
        assert_eq!(json["hold_last_frame"], true);
        assert_eq!(json["geometry"]["fit_mode"], "fill");

        let rebuilt = VideoCueFactory.from_json(json).expect("roundtrip");
        assert_eq!(rebuilt.visual_geometry().unwrap(), cue.geometry);
        let rebuilt_json = rebuilt.serialize();
        assert_eq!(rebuilt_json["hold_last_frame"], true);
    }

    #[test]
    fn from_json_without_geometry_uses_defaults() {
        let json = serde_json::json!({ "type": "video", "name": "Legacy" });
        let cue = VideoCueFactory.from_json(json).expect("legacy load");
        assert!(cue.visual_geometry().unwrap().is_default());
        let json = cue.serialize();
        assert_eq!(json["hold_last_frame"], false);
    }

    #[test]
    fn layer_style_roundtrip() {
        use crate::engine::output_engine::BlendMode;
        let mut cue = VideoCue::new();
        assert!(cue.layer_style().unwrap().is_default());
        // Visual cues never auto-stop each other: launching a visual cue
        // stacks it as a new layer, only Stop/Fade cues remove one.
        assert!(!cue.stop_on_next_go());

        cue.layer_style = LayerStyle {
            layer: Some(3),
            opacity: 0.5,
            blend_mode: BlendMode::Multiply,
        };

        let json = cue.serialize();
        assert_eq!(json["layer_style"]["blend_mode"], "multiply");

        let rebuilt = VideoCueFactory.from_json(json).expect("roundtrip");
        assert_eq!(rebuilt.layer_style().unwrap(), cue.layer_style);
    }

    #[test]
    fn legacy_json_loads_with_default_layer_style() {
        // Pre-1.3 workspaces have no layer_style (and may carry the removed
        // stop_on_next_visual flag, which must be ignored).
        let json =
            serde_json::json!({ "type": "video", "name": "Old", "stop_on_next_visual": true });
        let cue = VideoCueFactory.from_json(json).expect("legacy load");
        assert!(!cue.stop_on_next_go());
        assert!(cue.layer_style().unwrap().is_default());
    }

    #[test]
    fn legacy_single_output_migrates_to_multi_output_list() {
        let cue = VideoCueFactory
            .from_json(serde_json::json!({ "type": "video", "output_id": "main" }))
            .expect("legacy output route");
        let json = cue.serialize();
        assert_eq!(json["output_id"], "main");
        assert_eq!(json["output_ids"], serde_json::json!(["main"]));
    }
}
