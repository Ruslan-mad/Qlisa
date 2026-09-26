//! [`AudioCue`] — plays an audio file through the audio engine.
//!
//! This is the primary cue type for Inkue.  It decodes an audio file using
//! symphonia and submits a [`Voice`](crate::engine::voice::Voice) to the
//! [`AudioEngine`](crate::engine::AudioEngine) when triggered.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::engine::{
    ring_command::{FadeCurve as EngineFadeCurve, VoiceId},
    voice::{FadeDirection, FadeState, LevelMatrix, Voice, MATRIX_INPUTS, MATRIX_OUTPUTS},
};

use super::{
    context::{CueContext, CueEvent},
    traits::{
        action_position_from_file_ms, media_position_from_action_ms, Cue, CueFactory,
        media_bounds_ms,
    },
    types::{
        combine_fade_specs, eof_fade_remaining_ms, ContinueMode, CueColor, CueId, CueState,
        CueType, FadeCurve, FadeSpec,
    },
};

#[derive(Clone)]
struct SlicePcmCache {
    ranges: Vec<(u64, u64)>,
    samples: Arc<Vec<f32>>,
    offsets: Vec<u64>,
    _reservation: Arc<SlicePcmCacheReservation>,
}

struct SlicePcmCacheState {
    ready: bool,
    cache: Option<SlicePcmCache>,
    fallback_reason: Option<String>,
}

const SLICE_PCM_CACHE_GLOBAL_MAX_BYTES: usize = 128 * 1024 * 1024;
const SLICE_PCM_PRELOAD_WORKER_MAX: usize = 4;
const SLICE_PCM_PRELOAD_QUEUE_MAX: usize = 64;
static SLICE_PCM_CACHE_BYTES: AtomicUsize = AtomicUsize::new(0);
type SlicePcmPreloadJob = Box<dyn FnOnce() + Send + 'static>;
static SLICE_PCM_PRELOAD_QUEUE: std::sync::OnceLock<Option<crossbeam_channel::Sender<SlicePcmPreloadJob>>> = std::sync::OnceLock::new();

struct SlicePcmCacheReservation(usize);
impl Drop for SlicePcmCacheReservation {
    fn drop(&mut self) { SLICE_PCM_CACHE_BYTES.fetch_sub(self.0, Ordering::AcqRel); }
}

fn reserve_slice_pcm_bytes(bytes: usize) -> Option<Arc<SlicePcmCacheReservation>> {
    let mut current = SLICE_PCM_CACHE_BYTES.load(Ordering::Acquire);
    loop {
        let next = current.checked_add(bytes)?;
        if next > SLICE_PCM_CACHE_GLOBAL_MAX_BYTES { return None; }
        match SLICE_PCM_CACHE_BYTES.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(Arc::new(SlicePcmCacheReservation(bytes))),
            Err(actual) => current = actual,
        }
    }
}

fn enqueue_slice_pcm_preload(job: SlicePcmPreloadJob) -> Result<(), String> {
    let queue = SLICE_PCM_PRELOAD_QUEUE.get_or_init(|| {
        let (sender, receiver) = crossbeam_channel::bounded::<SlicePcmPreloadJob>(SLICE_PCM_PRELOAD_QUEUE_MAX);
        let mut workers = 0;
        for index in 0..SLICE_PCM_PRELOAD_WORKER_MAX {
            let receiver = receiver.clone();
            if std::thread::Builder::new().name(format!("qlisa-slice-preload-{index}")).spawn(move || {
                while let Ok(job) = receiver.recv() { job(); }
            }).is_ok() { workers += 1; }
        }
        (workers > 0).then_some(sender)
    }).as_ref().ok_or_else(|| "slice PCM preload workers could not start".to_string())?;
    queue.try_send(job).map_err(|error| match error {
        crossbeam_channel::TrySendError::Full(_) => "slice PCM preload queue is full".to_string(),
        crossbeam_channel::TrySendError::Disconnected(_) => "slice PCM preload workers stopped".to_string(),
    })
}

// ---------------------------------------------------------------------------
// AudioCue
// ---------------------------------------------------------------------------

/// A cue that plays an audio file through the audio engine.
pub struct AudioCue {
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

    // --- Audio-specific ---
    /// Path to the audio file, relative to the workspace directory.
    pub file_path: Option<PathBuf>,
    /// Volume in dB (-60 to +12).
    pub volume_db: f64,
    /// Stereo pan (-1.0 to +1.0).
    pub pan: f32,
    /// Optional fade in specification.
    pub fade_in: Option<FadeSpec>,
    /// Optional fade out specification (also used on soft stop).
    pub fade_out: Option<FadeSpec>,
    /// Start playback at this offset into the file.
    pub start_time: Option<Duration>,
    /// Stop playback at this offset into the file.
    pub end_time: Option<Duration>,
    /// Number of extra loop repetitions (0 = play once, u32::MAX = infinite).
    pub loop_count: u32,
    /// Output patch to route through.
    pub output_patch_id: Option<uuid::Uuid>,
    /// Playback rate multiplier (1.0 = normal speed).
    pub rate: f64,
    /// QLab-style slices (markers + per-segment play counts).  Empty = plain
    /// playback.  When present, `loop_count` is ignored (the vamp segments
    /// own the looping).
    pub slices: crate::cue::types::SliceList,
    /// QLab-style crosspoint levels in dB, `[input channel][patch channel]`.
    ///
    /// `None` — the default — keeps the cue on the plain pan + Output Patch
    /// routing. When set it **replaces** pan: a crosspoint says how much of
    /// input channel *i* reaches the patch's *j*-th channel, which is what a
    /// theatre mix actually needs (and what QLab's `AudioLevelMatrix` carries).
    /// The cue's `volume_db` still applies on top, so a Fade Cue keeps working.
    /// Columns address the **Output Patch's** channels, not raw device
    /// channels, so a matrix survives repatching.
    pub level_matrix: Option<Vec<Vec<f64>>>,

    is_disabled: bool,

    // --- Runtime ---
    /// Pre-decoded samples, loaded by `load()`.
    decoded_samples: Option<Arc<Vec<f32>>>,
    /// Probed source for bounded streaming playback. `decoded_samples` is
    /// retained only for legacy/test injection.
    stream_path: Option<PathBuf>,
    stream_source: Option<Arc<crate::cue::media_decode::StreamingAudioSource>>,
    slice_pcm_cache: Arc<(Mutex<SlicePcmCacheState>, Condvar)>,
    decoded_channels: u16,
    decoded_sample_rate: u32,
    /// The voice ID currently in use, if any.
    active_voice_id: Option<VoiceId>,
    /// Cached duration computed from the decoded samples.
    cached_duration: Option<Duration>,
    /// `true` between `go()` and the moment the audio action actually starts
    /// (i.e. while waiting for `pre_wait` to expire).
    in_pre_wait: bool,
    /// Incremented on every `go()` call.  Kept for diagnostics / future use.
    play_generation: u64,
    /// Set to `true` by [`Transport::go`] immediately after firing the
    /// Auto-Continue chain, so the event loop never double-fires it.
    auto_continue_fired: bool,
    /// Elapsed time accumulated before the most recent pause (mirrors WaitCue pattern).
    elapsed_before_pause: Duration,
    /// Action-elapsed time accumulated before the most recent pause.
    action_elapsed_before_pause: Duration,
    /// Monotonic action clock used by Auto-Continue (seeking must not change it).
    continue_started_at: Option<Instant>,
    continue_elapsed_before_pause: Duration,
    /// `true` once the natural-end fade-out has been handed to the engine for
    /// the current play, so it fires exactly once.
    eof_fade_started: bool,
}

impl AudioCue {
    fn load_legacy_audio(&mut self, path: &Path) -> Result<()> {
        let (samples, channels, sample_rate) = crate::cue::media_decode::decode_audio_track_legacy(path)?
            .ok_or_else(|| anyhow!("No audio track in file: {}", path.display()))?;
        let duration = Duration::from_secs_f64(
            samples.len() as f64 / channels.max(1) as f64 / sample_rate.max(1) as f64,
        );
        self.accept_preloaded_audio(Arc::new(samples), channels, sample_rate, duration);
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
        Ok(Voice::new_stream(stream_result, gain, self.pan))
    }

    fn legacy_voice(&self, gain: f32) -> Result<Voice> {
        let samples = self
            .decoded_samples
            .as_ref()
            .ok_or_else(|| anyhow!("legacy audio fallback produced no PCM"))?;
        Ok(Voice::new(Arc::clone(samples), self.decoded_channels, self.decoded_sample_rate, gain, self.pan))
    }

    /// Create a new, empty Audio Cue with a fresh UUID.
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::from("Audio Cue"),
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
            pan: 0.0,
            fade_in: None,
            fade_out: None,
            start_time: None,
            end_time: None,
            loop_count: 0,
            output_patch_id: None,
            rate: 1.0,
            slices: crate::cue::types::SliceList::default(),
            level_matrix: None,
            is_disabled: false,
            decoded_samples: None,
            stream_path: None,
            stream_source: None,
            slice_pcm_cache: Arc::new((Mutex::new(SlicePcmCacheState { ready: true, cache: None, fallback_reason: None }), Condvar::new())),
            decoded_channels: 2,
            decoded_sample_rate: 44100,
            active_voice_id: None,
            cached_duration: None,
            in_pre_wait: false,
            play_generation: 0,
            auto_continue_fired: false,
            elapsed_before_pause: Duration::ZERO,
            action_elapsed_before_pause: Duration::ZERO,
            continue_started_at: None,
            continue_elapsed_before_pause: Duration::ZERO,
            eof_fade_started: false,
        }
    }

    /// Return the active voice ID if the cue is currently playing.
    pub fn voice_id(&self) -> Option<VoiceId> {
        self.active_voice_id
    }

    /// Build the frame-resolved slice program for the current clip window,
    /// or `None` when the cue has no slice markers inside it.
    fn build_slice_program(&self) -> Option<crate::engine::voice::SliceProgram> {
        if self.slices.is_empty() {
            return None;
        }
        let sr = self.decoded_sample_rate as u64;
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
        let segments: Vec<crate::engine::voice::SliceSegment> = self
            .slices
            .segments(clip_start, clip_end)
            .into_iter()
            .map(|(s, e, count)| crate::engine::voice::SliceSegment {
                start_frame: s * sr / 1000,
                end_frame: e * sr / 1000,
                play_count: count,
                pcm_offset_frames: None,
            })
            .collect();
        crate::engine::voice::SliceProgram::new(segments)
    }

    /// Convert a [`FadeCurve`] from the cue layer to the engine layer.
    fn engine_curve(c: FadeCurve) -> EngineFadeCurve {
        match c {
            FadeCurve::Linear => EngineFadeCurve::Linear,
            FadeCurve::SCurve => EngineFadeCurve::SCurve,
            FadeCurve::Exponential => EngineFadeCurve::Exponential,
        }
    }

    /// Trigger the fade-out that lands on the cue's natural end, once the
    /// remaining action time drops inside the fade-out window.
    ///
    /// Without this, `fade_out` only ever applied to *manual* stops — a cue
    /// left to reach EOF hard-cut.  Skipped for infinite loops and sliced
    /// playback, which have no fixed natural end (`duration()` is `None`, or
    /// ignores the slice program).
    fn tick_eof_fade(&mut self, context: &CueContext) {
        if self.eof_fade_started || self.in_pre_wait || !self.slices.is_empty() {
            return;
        }
        let (Some(voice_id), Some(fade), Some(total)) =
            (self.active_voice_id, self.fade_out.as_ref(), self.duration())
        else {
            return;
        };
        let (fade_ms, curve) = (fade.duration_ms, Self::engine_curve(fade.curve));
        let Some(remaining_ms) = eof_fade_remaining_ms(self.action_elapsed(), total, fade_ms)
        else {
            return;
        };
        // Fire exactly once per play, whether or not the engine still knows
        // the voice (it may already have completed between two ticks).
        self.eof_fade_started = true;
        let _ = context.audio_engine.stop_voice(voice_id, remaining_ms, curve);
    }

    /// Start the audio action (submit a voice to the engine).
    ///
    /// Called either directly from `go()` when `pre_wait` is zero, or from
    /// `tick()` once the pre-wait timer has expired.
    fn start_audio_action(&mut self, context: &CueContext) -> Result<()> {
        let gain = crate::cue::types::db_to_linear(self.volume_db) as f32;
        let slice_program = self.build_slice_program();
        let cached_slice_pcm = if slice_program.is_some() && self.decoded_samples.is_none() {
            let (lock, ready) = &*self.slice_pcm_cache;
            let mut state = lock.lock().map_err(|_| anyhow!("slice PCM preload state is unavailable"))?;
            let deadline = Instant::now() + Duration::from_secs(3);
            while !state.ready {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(anyhow!("AudioCue '{}': slice media is still being prepared; retry GO", self.name));
                }
                let (next_state, timeout) = ready.wait_timeout(state, remaining)
                    .map_err(|_| anyhow!("slice PCM preload worker failed"))?;
                state = next_state;
                if timeout.timed_out() && !state.ready {
                    return Err(anyhow!("AudioCue '{}': slice media is still being prepared; retry GO", self.name));
                }
            }
            let expected_ranges = slice_program.as_ref().map(|program| program.segments.iter()
                .map(|segment| (segment.start_frame, segment.end_frame)).collect::<Vec<_>>()).unwrap_or_default();
            let matching_cache = state.cache.clone().filter(|cache| cache.ranges == expected_ranges);
            if matching_cache.is_none() {
                let reason = if state.cache.is_some() { "slice ranges changed after preload" }
                    else { state.fallback_reason.as_deref().unwrap_or("slice PCM cache is unavailable") };
                log::warn!("AudioCue '{}' ({}) uses streaming slice seek fallback: {reason}", self.name, self.id);
            }
            matching_cache
        } else { None };
        let mut slice_pcm_offsets = None;
        let mut voice = if let Some(samples) = &self.decoded_samples {
            Voice::new(Arc::clone(samples), self.decoded_channels, self.decoded_sample_rate, gain, self.pan)
        } else if let Some(cache) = cached_slice_pcm {
            slice_pcm_offsets = Some(cache.offsets);
            let mut voice = Voice::new(cache.samples, self.decoded_channels, self.decoded_sample_rate, gain, self.pan);
            let reservation: Arc<dyn std::any::Any + Send + Sync> = cache._reservation;
            voice.pcm_keepalive = Some(reservation);
            voice
        } else {
            let path = self
                .stream_path
                .clone()
                .or_else(|| self.file_path.clone())
                .ok_or_else(|| anyhow!("AudioCue '{}': audio not loaded — assign a file and try again", self.name))?;
            let initial_frame = self.start_time.map(|d| (d.as_secs_f64() * self.decoded_sample_rate as f64) as u64)
                .or_else(|| self.build_slice_program().map(|program| program.segments[0].start_frame))
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
                Ok(None) => {
                    return Err(anyhow!("No audio track in file: {}", path.display()));
                }
                Err(_) => {
                    self.load_legacy_audio(&path)?;
                    self.legacy_voice(gain)?
                }
            }
        };
        if voice.stream.is_none() {
            if let Some(source) = self.stream_source.take() { source.cancel(); }
        }
        if self.loop_count > 0 || !self.slices.is_empty() {
            if let Some(stream) = &voice.stream {
                if self.loop_count > 0 && self.slices.is_empty() {
                    if self.start_time.is_none() && self.end_time.is_none() {
                        stream.enable_seamless_loop();
                    } else {
                        let start = self.start_time
                            .map(|time| (time.as_secs_f64() * self.decoded_sample_rate as f64) as u64)
                            .unwrap_or(0);
                        let end = self.end_time
                            .map(|time| (time.as_secs_f64() * self.decoded_sample_rate as f64) as u64)
                            .unwrap_or_else(|| stream.total_frames());
                        let configured = if stream.is_trimmed_looping()
                            && stream.trim_loop_start() == start
                            && stream.trim_loop_end() == end
                        {
                            stream.restart_trimmed_loop(start, end)
                        } else {
                            stream.enable_trimmed_loop(start, end)
                        };
                        if !configured {
                            stream.keep_worker_for_loop();
                        }
                    }
                } else {
                    stream.keep_worker_for_loop();
                }
            }
        }

        voice.inner.loops_remaining.store(self.loop_count, std::sync::atomic::Ordering::Relaxed);
        voice.inner.set_rate(self.rate as f32);

        // Apply start/end time markers (written before play_voice; no RT thread yet).
        if let Some(end) = self.end_time {
            let end_frame = (end.as_secs_f64() * self.decoded_sample_rate as f64) as u64;
            // SAFETY: written once before play_voice(); RT thread has not started yet.
            unsafe { *voice.inner.end_frame.get() = Some(end_frame); }
        } else if self.loop_count > 0 && self.slices.is_empty() && self.start_time.is_some() {
            if let Some(stream) = &voice.stream {
                let end_frame = stream.total_frames();
                if end_frame > 0 {
                    unsafe { *voice.inner.end_frame.get() = Some(end_frame); }
                }
            }
        }
        if let Some(start) = self.start_time {
            let start_frame = (start.as_secs_f64() * self.decoded_sample_rate as f64) as u64;
            voice.frame_pos.store(start_frame, std::sync::atomic::Ordering::Relaxed);
        }

        // Slice program (QLab slices): resolved against the clip window in
        // frames.  When active it owns all boundaries — loop_count/end_frame
        // are ignored by the callback.
        if let Some(mut program) = self.build_slice_program() {
            if let Some(offsets) = slice_pcm_offsets {
                for (segment, offset) in program.segments.iter_mut().zip(offsets) {
                    segment.pcm_offset_frames = Some(offset);
                }
            }
            voice.frame_pos.store(
                program.segments[0].start_frame,
                std::sync::atomic::Ordering::Relaxed,
            );
            // SAFETY: written once before play_voice(); RT thread not started.
            unsafe { *voice.inner.slices.get() = Some(program) };
        }

        // Apply fade-in if configured (written before play_voice; RT thread not running yet).
        if let Some(ref fi) = self.fade_in {
            let total = (fi.duration_ms * self.decoded_sample_rate as u64) / 1000;
            // SAFETY: same as above — single writer before submission.
            unsafe {
                *voice.inner.fade.get() = Some(FadeState {
                    direction: FadeDirection::In,
                    total_samples: total,
                    elapsed_samples: 0,
                    curve: Self::engine_curve(fi.curve),
                });
            }
        }

        // Apply Output Patch routing.  Look up the cue's assigned patch
        // (falling back to the workspace default); if found, map its first two
        // channel indices to the voice's L/R output slots and target its
        // device (the engine opens an aux stream for non-main devices).
        let mut patch_device: Option<String> = None;
        if let Some(patch) = context.resolve_patch_checked(self.output_patch_id)? {
            if let Some(&ch_l) = patch.channels.first() {
                voice.out_l = ch_l as usize;
            }
            if let Some(&ch_r) = patch.channels.get(1) {
                voice.out_r = ch_r as usize;
            } else if let Some(&ch_l) = patch.channels.first() {
                // Mono patch — route both L and R to the same channel.
                voice.out_r = ch_l as usize;
            }
            // Main is a logical bus on the already-open program stream. Keep
            // it unpatched at the engine boundary so the configured ASIO
            // program pair is applied exactly once; Aux uses its physical
            // binding and a dedicated stream when needed.
            voice.patched = !patch.is_main();
            voice.patch_id = Some(patch.id);
            voice.patch_slot = context
                .output_patches
                .iter()
                .position(|p| p.id == patch.id)
                .map(|i| i as u8);
            voice.inner.set_patch_gain(crate::cue::types::db_to_linear(patch.gain_db as f64) as f32);
            if !patch.is_main() && !patch.device_id.is_empty() {
                patch_device = Some(patch.device_id.clone());
            }
        }

        // Crosspoint levels, resolved against the patch that was just applied
        // so a matrix column addresses the patch's channel, not a raw device
        // one.  Written before submission — the RT callback only reads it.
        if let Some(spec) = &self.level_matrix {
            let patch_channels: Vec<u16> = context
                .resolve_patch_checked(self.output_patch_id)?
                .map(|p| p.channels.clone())
                .unwrap_or_default();
            if let Some(matrix) = build_level_matrix(spec, &patch_channels) {
                // SAFETY: written once, before play_voice_routed hands the
                // voice to the RT thread.
                unsafe { *voice.inner.level_matrix.get() = Some(matrix) };
            }
        }

        let voice_id = context.audio_engine.play_voice_routed(voice, patch_device.as_deref())?;
        self.active_voice_id = Some(voice_id);
        self.action_started_at = Some(Instant::now());
        self.continue_started_at = self.action_started_at;
        self.continue_elapsed_before_pause = Duration::ZERO;
        self.in_pre_wait = false;
        self.eof_fade_started = false;

        context.emit(CueEvent::ActionStarted { cue_id: self.id });
        Ok(())
    }

    /// Decode an audio file to interleaved f32 samples.
    ///
    /// This is a pure function (no `self` mutation) and must be called on a
    /// non-RT thread without holding any workspace locks.  The result is
    /// pushed back into the cue via [`accept_preloaded_audio`]. This explicit
    /// legacy API is reserved for waveform/preview/editing paths that require
    /// a complete PCM copy; normal playback uses the bounded stream source.
    pub fn decode_file(path: &Path) -> Result<(Vec<f32>, u16, u32)> {
        crate::cue::media_decode::decode_audio_track_legacy(path)?
            .ok_or_else(|| anyhow!("No audio track in file: {}", path.display()))
    }
}

impl Default for AudioCue {
    fn default() -> Self {
        Self::new()
    }
}

/// Turn a cue's dB crosspoints into the engine's linear matrix, mapping each
/// column through `patch_channels` so column *j* lands on the patch's *j*-th
/// device channel.  Falls back to column index when the cue has no patch.
///
/// Returns `None` when nothing would be routed — the voice then stays on the
/// original pan path rather than being silently muted by an empty matrix.
pub(crate) fn build_level_matrix(
    spec: &[Vec<f64>],
    patch_channels: &[u16],
) -> Option<LevelMatrix> {
    // "Configured or not" is a property of the *spec*, not of the padded array
    // below — an empty spec means the cue has no matrix at all.
    if spec.iter().all(|row| row.is_empty()) {
        return None;
    }
    let mut routed = vec![vec![0.0_f32; MATRIX_OUTPUTS]; MATRIX_INPUTS];
    for (input, row) in spec.iter().take(MATRIX_INPUTS).enumerate() {
        for (column, &db) in row.iter().enumerate() {
            let device = patch_channels
                .get(column)
                .map(|&c| c as usize)
                .unwrap_or(column);
            if device < MATRIX_OUTPUTS {
                routed[input][device] = crate::cue::types::db_to_linear(db) as f32;
            }
        }
    }
    LevelMatrix::new(&routed)
}

impl AudioCue {
    fn start_stream_source(
        &mut self,
        path: PathBuf,
        info: crate::cue::media_decode::AudioStreamInfo,
    ) -> Result<()> {
        let source = crate::cue::media_decode::StreamingAudioSource::start(path, info)?;
        if self.loop_count > 0 || !self.slices.is_empty() {
            if self.loop_count > 0
                && self.slices.is_empty()
                && self.start_time.is_none()
                && self.end_time.is_none()
            {
                source.enable_seamless_loop();
            } else {
                source.keep_worker_for_loop();
            }
        }
        self.stream_source = Some(source);
        Ok(())
    }
}

impl Cue for AudioCue {
    fn id(&self) -> CueId { self.id }
    fn cue_type(&self) -> CueType { CueType::Audio }
    fn output_patch_id(&self) -> Option<uuid::Uuid> { self.output_patch_id }
    fn name(&self) -> &str { &self.name }
    fn set_name(&mut self, name: String) { self.name = name; }
    fn number(&self) -> Option<&str> { self.number.as_deref() }
    fn set_number(&mut self, number: Option<String>) { self.number = number; }
    fn notes(&self) -> &str { &self.notes }
    fn set_notes(&mut self, notes: String) { self.notes = notes; }
    fn color(&self) -> CueColor { self.color }
    fn set_color(&mut self, color: CueColor) { self.color = color; }
    fn is_disabled(&self) -> bool { self.is_disabled }
    fn set_disabled(&mut self, d: bool) { self.is_disabled = d; }
    fn state(&self) -> CueState { self.state }

    fn load(&mut self, _context: &CueContext) -> Result<()> {
        let path = match &self.file_path {
            Some(p) => p.clone(),
            None => return Ok(()), // No file assigned; nothing to load.
        };

        let probe = crate::cue::media_decode::probe_audio_track(&path);
        if crate::cue::media_decode::should_use_legacy_fallback(&probe) {
            return self.load_legacy_audio(&path);
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
                Ok(())
            }
            Ok(None) => Err(anyhow!("No audio track in file: {}", path.display())),
            Err(_) => unreachable!("probe error handled by fallback policy"),
        }
    }

    fn accept_preloaded_audio(
        &mut self,
        samples: std::sync::Arc<Vec<f32>>,
        channels: u16,
        sample_rate: u32,
        duration: std::time::Duration,
    ) {
        self.decoded_samples = Some(samples);
        self.slice_pcm_cache = Arc::new((Mutex::new(SlicePcmCacheState { ready: true, cache: None, fallback_reason: None }), Condvar::new()));
        self.stream_path = None;
        if let Some(stream) = self.stream_source.take() { stream.cancel(); }
        self.decoded_channels = channels;
        self.decoded_sample_rate = sample_rate;
        self.cached_duration = Some(duration);
    }

    fn accept_preloaded_stream(
        &mut self,
        path: PathBuf,
        channels: u16,
        sample_rate: u32,
        duration: Option<Duration>,
    ) {
        self.stream_path = Some(path.clone());
        self.decoded_samples = None;
        self.decoded_channels = channels.max(1);
        self.decoded_sample_rate = sample_rate.max(1);
        self.cached_duration = duration.filter(|d| !d.is_zero());
        let ranges = self.build_slice_program().map(|program| program.segments.iter()
            .map(|segment| (segment.start_frame, segment.end_frame)).collect::<Vec<_>>());
        self.slice_pcm_cache = Arc::new((Mutex::new(SlicePcmCacheState { ready: ranges.is_none(), cache: None, fallback_reason: None }), Condvar::new()));
        if let Some(ranges) = ranges {
            let frames = ranges.iter().try_fold(0_u64, |sum, (start, end)| sum.checked_add(end.saturating_sub(*start)));
            let requested_bytes = frames.and_then(|frames| frames.checked_mul(channels as u64 * std::mem::size_of::<f32>() as u64))
                .and_then(|bytes| usize::try_from(bytes).ok());
            let preload = requested_bytes.filter(|bytes| *bytes <= crate::cue::media_decode::SLICE_PCM_CACHE_MAX_BYTES)
                .and_then(|bytes| reserve_slice_pcm_bytes(bytes).map(|reservation| (bytes, reservation)));
            let reason = if requested_bytes.is_none() {
                Some("requested slice cache size overflowed".to_string())
            } else if requested_bytes.is_some_and(|bytes| bytes > crate::cue::media_decode::SLICE_PCM_CACHE_MAX_BYTES) {
                Some("unique slice PCM exceeds the 64 MiB per-cue cache limit".to_string())
            } else if preload.is_none() {
                Some(format!("global slice PCM cache budget of {} MiB is full", SLICE_PCM_CACHE_GLOBAL_MAX_BYTES / (1024 * 1024)))
            } else { None };
            if let Some((_bytes, reservation)) = preload {
                let cache_slot = Arc::clone(&self.slice_pcm_cache);
                let path = path.clone();
                let job: SlicePcmPreloadJob = Box::new(move || {
                match crate::cue::media_decode::decode_slice_ranges(&path, crate::cue::media_decode::AudioStreamInfo {
                    channels, sample_rate, total_frames: duration.map(|d| (d.as_secs_f64() * sample_rate as f64) as u64),
                }, &ranges) {
                    Ok(Some((samples, offsets))) => {
                        if let Ok(mut state) = cache_slot.0.lock() {
                            state.cache = Some(SlicePcmCache { ranges, samples: Arc::new(samples), offsets, _reservation: reservation });
                            state.fallback_reason = None;
                            state.ready = true;
                            cache_slot.1.notify_all();
                        }
                    }
                    Ok(None) => {
                        if let Ok(mut state) = cache_slot.0.lock() {
                            let frames = ranges.iter().try_fold(0_u64, |sum, (start, end)| sum.checked_add(end.saturating_sub(*start)));
                            let bytes = frames.and_then(|frames| frames.checked_mul(channels as u64 * std::mem::size_of::<f32>() as u64));
                            state.fallback_reason = Some(if bytes.is_some_and(|bytes| bytes > crate::cue::media_decode::SLICE_PCM_CACHE_MAX_BYTES as u64) {
                                format!("unique slice PCM exceeds {} MiB cache limit", crate::cue::media_decode::SLICE_PCM_CACHE_MAX_BYTES / (1024 * 1024))
                            } else { "decoder could not provide all requested slice frames".into() });
                            state.ready = true; cache_slot.1.notify_all();
                        }
                    }
                    Err(error) => {
                        if let Ok(mut state) = cache_slot.0.lock() {
                            state.fallback_reason = Some(format!("slice PCM decode failed: {error:#}"));
                            state.ready = true; cache_slot.1.notify_all();
                        }
                    }
                }
                });
                if let Err(reason) = enqueue_slice_pcm_preload(job) {
                    log::warn!("Could not queue AudioCue slice PCM preload: {reason}");
                    if let Ok(mut state) = self.slice_pcm_cache.0.lock() { state.fallback_reason = Some(reason); state.ready = true; self.slice_pcm_cache.1.notify_all(); }
                }
            } else if let Some(reason) = reason {
                if let Ok(mut state) = self.slice_pcm_cache.0.lock() { state.fallback_reason = Some(reason); state.ready = true; self.slice_pcm_cache.1.notify_all(); }
            }
        }
        if let Some(stream) = self.stream_source.take() { stream.cancel(); }
    }

    fn go(&mut self, context: &CueContext) -> Result<()> {
        if self.state == CueState::Running {
            return Ok(()); // Already playing; ignore duplicate GO.
        }

        // New play: bump generation and clear the auto-continue flag so the
        // transport can fire the chain again for this play.
        self.play_generation = self.play_generation.wrapping_add(1);
        self.auto_continue_fired = false;
        self.continue_started_at = None;
        self.continue_elapsed_before_pause = Duration::ZERO;

        self.state = CueState::Running;
        self.started_at = Some(Instant::now());

        if !self.pre_wait.is_zero() {
            // Pre-wait active: record the start time and defer the action.
            // tick() will call start_audio_action() once the timer expires.
            self.in_pre_wait = true;
            return Ok(());
        }

        // No pre-wait: start the audio action immediately.
        // On failure, roll back to Standby so callers (e.g. GroupCue) don't
        // see a permanently-Running cue that will never complete.
        if let Err(e) = self.start_audio_action(context) {
            self.state = CueState::Standby;
            self.started_at = None;
            self.in_pre_wait = false;
            return Err(e);
        }
        Ok(())
    }

    fn stop(&mut self, context: &CueContext) -> Result<()> {
        self.in_pre_wait = false; // Cancel any pending pre-wait.
        if let Some(vid) = self.active_voice_id.take() {
            let (fade_ms, fade_curve) = self.fade_out
                .as_ref()
                .map(|f| (f.duration_ms as u32, Self::engine_curve(f.curve)))
                .unwrap_or((context.stop_fade_ms, EngineFadeCurve::SCurve));
            context.audio_engine.stop_voice(vid, fade_ms, fade_curve)?;
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
        context.emit(CueEvent::Stopped { cue_id: self.id });
        Ok(())
    }

    fn go_with_fade(&mut self, context: &CueContext, shared_fade: Option<&FadeSpec>) -> Result<()> {
        if shared_fade.is_none() {
            return self.go(context);
        }
        // Number children have their pre-wait owned by the Number scheduler.
        // Keep the temporary envelope only for this start; authored child
        // settings remain unchanged in the workspace and inspector.
        let authored = self.fade_in.clone();
        self.fade_in = combine_fade_specs(authored.as_ref(), shared_fade);
        let result = self.go(context);
        self.fade_in = authored;
        result
    }

    fn stop_with_fade(&mut self, context: &CueContext, shared_fade: Option<&FadeSpec>) -> Result<()> {
        if shared_fade.is_none() {
            return self.stop(context);
        }
        let authored = self.fade_out.clone();
        self.fade_out = combine_fade_specs(authored.as_ref(), shared_fade);
        let result = self.stop(context);
        self.fade_out = authored;
        result
    }

    fn pause(&mut self, context: &CueContext) -> Result<()> {
        if self.state != CueState::Running {
            return Ok(());
        }
        if !self.in_pre_wait {
            if let Some(vid) = self.active_voice_id {
                context.audio_engine.pause_voice(vid)?;
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
                context.audio_engine.resume_voice(vid)?;
            }
        }
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
        // Allow seek when running or paused; block during pre-wait and standby.
        if self.action_started_at.is_none() && self.state != CueState::Paused {
            return;
        }
        let max_action_ms = self.duration().map(|d| d.as_millis() as u64);
        let action_ms = max_action_ms.map_or(position_ms, |max| position_ms.min(max));
        let file_ms = media_position_from_action_ms(
            Duration::from_millis(action_ms),
            self.cached_duration,
            self.start_time,
            self.end_time,
            self.rate,
            self.loop_count,
        );
        if let Some(vid) = self.active_voice_id {
            let frame_pos = file_ms * self.decoded_sample_rate as u64 / 1000;
            let _ = ctx.audio_engine.seek_voice(vid, frame_pos);
        }
        if self.state == CueState::Paused {
            // Update the frozen accumulators directly — elapsed() reads these
            // when paused, so the display updates on the next event-loop tick.
            self.action_elapsed_before_pause = Duration::from_millis(action_ms);
            self.elapsed_before_pause = self.pre_wait + Duration::from_millis(action_ms);
        } else {
            self.action_started_at =
                Some(Instant::now() - Duration::from_millis(action_ms));
        }
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
            self.rate,
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
            self.rate,
            self.loop_count,
        ))
    }

    fn media_position_ms(
        &self,
        audio_engine: &crate::engine::AudioEngine,
        _output_engine: &crate::engine::OutputEngine,
    ) -> Option<u64> {
        let fallback = self.media_position_for_action_ms(self.action_elapsed());
        let raw = self
            .active_voice_id
            .and_then(|voice_id| audio_engine.voice_position_ms(voice_id))
            .or(fallback)?;
        let (start_ms, end_ms) = media_bounds_ms(self.cached_duration, self.start_time, self.end_time);
        Some(raw.clamp(start_ms, end_ms))
    }

    fn hard_stop(&mut self, context: &CueContext) -> Result<()> {
        self.in_pre_wait = false;
        if let Some(vid) = self.active_voice_id.take() {
            context.audio_engine.stop_voice(vid, 0, EngineFadeCurve::Linear)?;
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
        context.emit(CueEvent::Stopped { cue_id: self.id });
        Ok(())
    }

    fn reset(&mut self) -> Result<()> {
        // Note: does not stop playback — call stop() first if needed.
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
        Ok(())
    }

    fn tick(&mut self, context: &CueContext) -> Result<()> {
        if self.in_pre_wait && self.elapsed() >= self.pre_wait {
            if let Err(error) = self.start_audio_action(context) {
                self.state = CueState::Standby;
                self.started_at = None;
                self.action_started_at = None;
                self.in_pre_wait = false;
                return Err(error);
            }
        }
        self.tick_eof_fade(context);
        Ok(())
    }

    fn is_action_started(&self) -> bool {
        !self.in_pre_wait
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

    fn pre_wait(&self) -> Duration { self.pre_wait }
    fn set_pre_wait(&mut self, d: Duration) { self.pre_wait = d; }
    fn post_wait(&self) -> Duration { self.post_wait }
    fn set_post_wait(&mut self, d: Duration) { self.post_wait = d; }

    fn duration(&self) -> Option<Duration> {
        // Infinite loop — no fixed duration; rely on voice_done for completion.
        if self.loop_count == u32::MAX {
            return None;
        }
        self.cached_duration.map(|d| {
            // Adjust for start/end markers.
            let start = self.start_time.unwrap_or(Duration::ZERO);
            let end = self.end_time.unwrap_or(d);
            let base = end.saturating_sub(start);
            // Adjust for playback rate: rate > 1.0 shortens effective duration.
            let adjusted = if self.rate > 0.0 && (self.rate - 1.0).abs() > f64::EPSILON {
                Duration::from_secs_f64(base.as_secs_f64() / self.rate)
            } else {
                base
            };
            // Multiply by total number of plays: initial play + loop_count repeats.
            adjusted * (self.loop_count + 1)
        })
    }

    fn elapsed(&self) -> Duration {
        if self.state == CueState::Paused {
            return self.elapsed_before_pause;
        }
        self.started_at.map(|t| t.elapsed()).unwrap_or(Duration::ZERO)
    }

    fn action_elapsed(&self) -> Duration {
        if self.state == CueState::Paused {
            return self.action_elapsed_before_pause;
        }
        self.action_started_at.map(|t| t.elapsed()).unwrap_or(Duration::ZERO)
    }

    fn auto_continue_elapsed(&self) -> Duration {
        if self.state == CueState::Paused {
            return self.continue_elapsed_before_pause;
        }
        self.continue_started_at
            .map(|t| t.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    fn continue_mode(&self) -> ContinueMode { self.continue_mode }
    fn set_continue_mode(&mut self, mode: ContinueMode) { self.continue_mode = mode; }

    fn validate(
        &self,
        ctx: &crate::cue::validation::ValidationContext,
    ) -> Vec<crate::cue::validation::CueIssue> {
        use crate::cue::validation::CueIssue;
        // `None` routes to the default patch — only a *set* patch that no longer
        // exists is worth flagging (the cue silently falls back otherwise).
        match self.output_patch_id {
            Some(id) if !ctx.output_patch_ids.contains(&id) => {
                vec![CueIssue::warning("Output Patch not found (using default patch)")]
            }
            _ => Vec::new(),
        }
    }

    fn playing_voice_id(&self) -> Option<CueId> {
        self.active_voice_id
    }

    fn uses_sliced_playback(&self) -> bool {
        !self.slices.is_empty()
    }

    fn file_duration(&self) -> Option<Duration> {
        self.cached_duration
    }

    fn media_file_path(&self) -> Option<&std::path::Path> {
        self.file_path.as_deref()
    }

    fn extract_decoded_audio(
        &self,
    ) -> Option<(std::sync::Arc<Vec<f32>>, u16, u32, Duration)> {
        let samples = self.decoded_samples.as_ref()?;
        let duration = self.cached_duration?;
        Some((Arc::clone(samples), self.decoded_channels, self.decoded_sample_rate, duration))
    }

    fn extract_preloaded_stream(
        &self,
    ) -> Option<(PathBuf, u16, u32, Option<Duration>)> {
        Some((self.stream_path.clone()?, self.decoded_channels, self.decoded_sample_rate, self.cached_duration))
    }

    fn waveform_peaks(&self, bins: usize) -> Option<Vec<f32>> {
        let samples = self.decoded_samples.as_ref()?;
        let channels = self.decoded_channels as usize;
        if bins == 0 || channels == 0 { return Some(vec![]); }
        let total_frames = samples.len() / channels;
        if total_frames == 0 { return Some(vec![]); }

        let mut peaks = Vec::with_capacity(bins);
        for i in 0..bins {
            let start_frame = (i * total_frames) / bins;
            let end_frame = (((i + 1) * total_frames) / bins).max(start_frame + 1);
            let mut peak = 0.0f32;
            for frame in start_frame..end_frame.min(total_frames) {
                for ch in 0..channels {
                    let v = samples[frame * channels + ch].abs();
                    if v > peak { peak = v; }
                }
            }
            peaks.push(peak);
        }
        Some(peaks)
    }

    fn runtime_state(&self) -> crate::cue::traits::RuntimeState {
        crate::cue::traits::RuntimeState {
            state: self.state,
            voice_id: self.active_voice_id,
            started_at: self.started_at,
            action_started_at: self.action_started_at,
        }
    }

    fn restore_runtime_state(&mut self, snap: crate::cue::traits::RuntimeState) {
        self.state = snap.state;
        self.active_voice_id = snap.voice_id;
        self.started_at = snap.started_at;
        self.action_started_at = snap.action_started_at;
        // Infer pre-wait: Running but action not yet started.
        self.in_pre_wait = snap.state == CueState::Running && snap.action_started_at.is_none();
    }

    fn live_audio_params(&self) -> Option<crate::cue::traits::LiveAudioParams> {
        let voice_id = self.active_voice_id?;
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

    fn serialize(&self) -> Value {
        json!({
            "type": "audio",
            "cue_type": "audio",
            "id": self.id,
            "number": self.number,
            "name": self.name,
            "notes": self.notes,
            "color": self.color,
            "pre_wait_ms": self.pre_wait.as_millis() as u64,
            "post_wait_ms": self.post_wait.as_millis() as u64,
            "continue_mode": self.continue_mode,
            "file_path": self.file_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            "cached_duration_ms": self.cached_duration.map(|d| d.as_millis() as u64),
            "volume_db": self.volume_db,
            "pan": self.pan,
            "fade_in_ms": self.fade_in.as_ref().map(|f| f.duration_ms),
            "fade_in_curve": self.fade_in.as_ref().map(|f| f.curve),
            "fade_out_ms": self.fade_out.as_ref().map(|f| f.duration_ms),
            "fade_out_curve": self.fade_out.as_ref().map(|f| f.curve),
            "start_time_ms": self.start_time.map(|d| d.as_millis() as u64),
            "end_time_ms": self.end_time.map(|d| d.as_millis() as u64),
            "loop_count": self.loop_count,
            "output_patch_id": self.output_patch_id,
            "rate": self.rate,
            "slices": self.slices,
            "level_matrix": self.level_matrix,
            "is_disabled": self.is_disabled,
        })
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Factory for [`AudioCue`].
pub struct AudioCueFactory;

impl CueFactory for AudioCueFactory {
    fn create(&self) -> Box<dyn Cue> {
        Box::new(AudioCue::new())
    }

    fn from_json(&self, value: Value) -> Result<Box<dyn Cue>> {
        let mut cue = AudioCue::new();

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
        if let Some(ms) = value.get("cached_duration_ms").and_then(|v| v.as_u64()) {
            cue.cached_duration = Some(Duration::from_millis(ms));
        }
        if let Some(db) = value.get("volume_db").and_then(|v| v.as_f64()) {
            cue.volume_db = db;
        }
        if let Some(pan) = value.get("pan").and_then(|v| v.as_f64()) {
            cue.pan = pan as f32;
        }
        if let Some(ms) = value.get("fade_in_ms").and_then(|v| v.as_u64()) {
            let curve = value.get("fade_in_curve")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(FadeCurve::SCurve);
            cue.fade_in = Some(FadeSpec { duration_ms: ms, curve });
        }
        if let Some(ms) = value.get("fade_out_ms").and_then(|v| v.as_u64()) {
            let curve = value.get("fade_out_curve")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(FadeCurve::SCurve);
            cue.fade_out = Some(FadeSpec { duration_ms: ms, curve });
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
        if let Some(patch_str) = value.get("output_patch_id").and_then(|v| v.as_str()) {
            cue.output_patch_id = patch_str.parse().ok();
        }
        if let Some(rate) = value.get("rate").and_then(|v| v.as_f64()) {
            cue.rate = rate;
        }
        if let Some(s) = value.get("slices") {
            if let Ok(mut slices) = serde_json::from_value::<crate::cue::types::SliceList>(s.clone()) {
                slices.normalize();
                cue.slices = slices;
            }
        }
        // Absent (every workspace written before matrices existed) stays None,
        // which is the untouched pan path.
        if let Some(m) = value.get("level_matrix") {
            if let Ok(rows) = serde_json::from_value::<Option<Vec<Vec<f64>>>>(m.clone()) {
                cue.level_matrix = rows.filter(|r| !r.is_empty());
            }
        }
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
mod level_matrix_tests {
    use super::*;

    #[test]
    fn a_cue_has_no_matrix_by_default() {
        // The guarantee this whole feature rests on: existing cues keep the
        // original pan + patch routing, untouched.
        assert!(AudioCue::new().level_matrix.is_none());
    }

    #[test]
    fn columns_map_through_the_output_patch_channels() {
        // Patch feeds device channels 4 and 5; column 0 must land on 4.
        let spec = vec![vec![0.0, -60.0], vec![-60.0, 0.0]];
        let matrix = build_level_matrix(&spec, &[4, 5]).expect("routed");

        assert!((matrix.gains[0][4] - 1.0).abs() < 1e-6, "input L → patch channel 0 (device 4)");
        assert_eq!(matrix.gains[0][5], 0.0, "and nothing to the other one");
        assert!((matrix.gains[1][5] - 1.0).abs() < 1e-6, "input R → patch channel 1 (device 5)");
        assert_eq!(matrix.width, 6, "width covers up to the highest used device channel");
    }

    #[test]
    fn without_a_patch_columns_are_device_channels() {
        let spec = vec![vec![0.0], vec![0.0]];
        let matrix = build_level_matrix(&spec, &[]).expect("routed");
        assert!((matrix.gains[0][0] - 1.0).abs() < 1e-6);
        assert_eq!(matrix.width, 1);
    }

    #[test]
    fn an_empty_spec_yields_no_matrix() {
        // Nothing configured — the voice stays on the ordinary pan path.
        assert!(build_level_matrix(&[], &[]).is_none());
        assert!(build_level_matrix(&[vec![], vec![]], &[]).is_none());
    }

    #[test]
    fn an_all_silent_matrix_is_kept_and_mutes_the_cue() {
        // The operator pulled every crosspoint down; honouring that means
        // silence. Falling back to pan routing here would make the cue audible
        // against an explicit instruction.
        let matrix = build_level_matrix(&[vec![-60.0, -60.0]], &[]).expect("kept");
        assert_eq!(matrix.width, 0, "nothing is routed anywhere");
    }

    #[test]
    fn live_matrix_edits_reach_the_engine_one_crosspoint_at_a_time() {
        // The whole matrix is 136 bytes; sending it as a single command would
        // bloat every slot of the RT ring buffer, so it goes cell by cell.
        let matrix = build_level_matrix(&[vec![0.0], vec![0.0]], &[]).expect("routed");
        assert_eq!(matrix.gains.len(), MATRIX_INPUTS);
        assert_eq!(matrix.gains[0].len(), MATRIX_OUTPUTS);
    }

    #[test]
    fn crosspoints_past_the_fixed_capacity_are_dropped_not_wrapped() {
        let wide: Vec<f64> = (0..MATRIX_OUTPUTS + 4).map(|_| 0.0).collect();
        let matrix = build_level_matrix(&[wide], &[]).expect("routed");
        assert_eq!(matrix.width, MATRIX_OUTPUTS, "clamped to the array, never out of bounds");
    }

    #[test]
    fn serialize_roundtrip_preserves_the_matrix() {
        let factory = AudioCueFactory;
        let mut cue = AudioCue::new();
        cue.level_matrix = Some(vec![vec![0.0, -6.0], vec![-6.0, 0.0]]);

        let rebuilt = factory.from_json(cue.serialize()).unwrap();
        let json = rebuilt.serialize();

        assert_eq!(json["level_matrix"][0][1], -6.0);
        assert_eq!(json["level_matrix"][1][0], -6.0);
    }

    #[test]
    fn a_workspace_written_before_matrices_existed_loads_without_one() {
        let factory = AudioCueFactory;
        let mut json = AudioCue::new().serialize();
        json.as_object_mut().unwrap().remove("level_matrix");

        let rebuilt = factory.from_json(json).unwrap();

        assert_eq!(rebuilt.serialize()["level_matrix"], serde_json::Value::Null);
    }
}

#[cfg(test)]
mod sliced_cache_go_tests {
    use super::*;
    use crate::cue::traits::Cue;
    use crate::engine::{
        dmx_engine::DmxEngine,
        engine_traits::OutputEngineApi,
        output_engine::ContentRequest,
        ring_command::VoiceId,
    };
    use anyhow::Result;
    use crossbeam_channel::unbounded;

    struct NullOutput;
    impl OutputEngineApi for NullOutput {
        fn show_content(&self, _req: ContentRequest<'_>) -> Result<VoiceId> { anyhow::bail!("unused") }
        fn stop_content(&self, _voice_id: VoiceId, _visual_fade_ms: u32, _audio_fade_ms: u32) {}
        fn hard_stop_current(&self) {}
        fn panic_stop(&self) {}
        fn video_audio_voice(&self, _voice_id: VoiceId) -> Option<VoiceId> { None }
        fn resync_audio_to_video(&self, _voice_id: VoiceId) {}
        fn get_voice_opacity(&self, _voice_id: VoiceId) -> f32 { 1.0 }
        fn set_voice_opacity(&self, _voice_id: VoiceId, _opacity: f32) {}
        fn stop_voice(&self, _voice_id: VoiceId, _fade_ms: u32) -> Result<()> { Ok(()) }
        fn pause_voice(&self, _voice_id: VoiceId) -> Result<()> { Ok(()) }
        fn resume_voice(&self, _voice_id: VoiceId) -> Result<()> { Ok(()) }
        fn seek_voice_ms(&self, _voice_id: VoiceId, _position_ms: u64) {}
        fn show_text_overlay(&self, _ass_text: &str, _screen_index: Option<u32>) {}
        fn clear_text_overlay(&self) {}
        fn begin_eof_fade_out(&self, _voice_id: VoiceId, _fade_ms: u32) -> bool { false }
        fn devamp_voice(&self, _voice_id: VoiceId, _stop_at_end: bool) {}
        fn start_preloaded(&self, _voice_id: VoiceId) -> bool { false }
    }

    #[test]
    fn ordinary_go_uses_preloaded_slice_cache() {
        use crate::cue::types::SliceList;
        use std::io::Write;
        let path = std::env::temp_dir().join(format!("qlisa-slice-go-{}.wav", Uuid::new_v4()));
        let sr = 48_000_u32;
        let frames = sr as usize;
        let bytes = (frames * 4) as u32;
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"RIFF").unwrap(); file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap(); file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap(); file.write_all(&2_u16.to_le_bytes()).unwrap();
        file.write_all(&sr.to_le_bytes()).unwrap(); file.write_all(&(sr * 4).to_le_bytes()).unwrap();
        file.write_all(&4_u16.to_le_bytes()).unwrap(); file.write_all(&16_u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap(); file.write_all(&bytes.to_le_bytes()).unwrap();
        for frame in 0..frames {
            let value = ((frame as f64 * 440.0 * std::f64::consts::TAU / sr as f64).sin() * 12_000.0) as i16;
            file.write_all(&value.to_le_bytes()).unwrap(); file.write_all(&value.to_le_bytes()).unwrap();
        }
        drop(file);

        let mut cue = AudioCue::new();
        cue.file_path = Some(path.clone());
        cue.slices = SliceList { markers: vec![25, 50], play_counts: vec![1, 101, 1] };
        cue.accept_preloaded_stream(path.clone(), 2, sr, Some(Duration::from_secs(1)));
        let audio = crate::engine::audio_engine::AudioEngine::new_silent(&crate::preferences::MachineAudioConfig::default());
        let (events, _receiver) = unbounded();
        let context = CueContext::new(
            audio, Arc::new(NullOutput), events, 0, Vec::new(), None, None, Vec::new(),
            Arc::new(DmxEngine::new()), Vec::new(), Vec::new(), Vec::new(), 256,
        );
        cue.go(&context).expect("normal AudioCue GO");
        assert!(cue.voice_id().is_some(), "GO submitted an audio voice");
        assert!(cue.stream_source.is_none(), "GO selected the preloaded PCM cache and retired its stream");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn go_times_out_instead_of_silently_using_seek_loop_while_cache_is_pending() {
        use crate::cue::types::SliceList;
        let mut cue = AudioCue::new();
        cue.stream_path = Some(PathBuf::from("not-opened-before-cache-read.wav"));
        cue.decoded_sample_rate = 48_000;
        cue.cached_duration = Some(Duration::from_secs(1));
        cue.slices = SliceList { markers: vec![25, 50], play_counts: vec![1, 101, 1] };
        cue.slice_pcm_cache = Arc::new((Mutex::new(SlicePcmCacheState { ready: false, cache: None, fallback_reason: None }), Condvar::new()));
        let audio = crate::engine::audio_engine::AudioEngine::new_silent(&crate::preferences::MachineAudioConfig::default());
        let (events, _receiver) = unbounded();
        let context = CueContext::new(
            audio, Arc::new(NullOutput), events, 0, Vec::new(), None, None, Vec::new(),
            Arc::new(DmxEngine::new()), Vec::new(), Vec::new(), Vec::new(), 256,
        );
        let error = cue.go(&context).expect_err("GO must not enter the broken seek fallback while cache is pending");
        assert!(error.to_string().contains("still being prepared"));
        assert!(cue.voice_id().is_none());
    }

    #[test]
    fn pre_wait_cache_timeout_rolls_cue_back_to_standby() {
        use crate::cue::types::SliceList;
        let mut cue = AudioCue::new();
        cue.stream_path = Some(PathBuf::from("not-opened-before-cache-read.wav"));
        cue.decoded_sample_rate = 48_000;
        cue.cached_duration = Some(Duration::from_secs(1));
        cue.slices = SliceList { markers: vec![25, 50], play_counts: vec![1, 101, 1] };
        cue.pre_wait = Duration::from_millis(1);
        cue.slice_pcm_cache = Arc::new((Mutex::new(SlicePcmCacheState { ready: false, cache: None, fallback_reason: None }), Condvar::new()));
        let audio = crate::engine::audio_engine::AudioEngine::new_silent(&crate::preferences::MachineAudioConfig::default());
        let (events, _receiver) = unbounded();
        let context = CueContext::new(
            audio, Arc::new(NullOutput), events, 0, Vec::new(), None, None, Vec::new(),
            Arc::new(DmxEngine::new()), Vec::new(), Vec::new(), Vec::new(), 256,
        );
        cue.go(&context).unwrap();
        assert_eq!(cue.state(), CueState::Running);
        assert!(cue.in_pre_wait);
        std::thread::sleep(Duration::from_millis(2));
        let error = cue.tick(&context).expect_err("expired pre-wait must report pending media");
        assert!(error.to_string().contains("still being prepared"));
        assert_eq!(cue.state(), CueState::Standby);
        assert!(!cue.in_pre_wait);
        assert!(cue.started_at.is_none());
    }

    #[test]
    fn active_voice_keeps_slice_cache_reservation_alive_after_cache_drop() {
        let reservation = reserve_slice_pcm_bytes(256).expect("small reservation should fit");
        let weak_reservation = Arc::downgrade(&reservation);
        let samples = Arc::new(vec![0.0_f32; 128]);
        let cache = SlicePcmCache {
            ranges: vec![(0, 64)],
            samples: Arc::clone(&samples),
            offsets: vec![0],
            _reservation: Arc::clone(&reservation),
        };
        let mut voice = Voice::new(cache.samples.clone(), 2, 48_000, 1.0, 0.0);
        voice.pcm_keepalive = Some(cache._reservation.clone());

        drop(cache);
        drop(reservation);
        assert!(weak_reservation.upgrade().is_some(), "active voice must retain the memory lease");

        drop(voice);
        assert!(weak_reservation.upgrade().is_none(), "lease must release after the active voice drops");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::traits::Cue;

    fn make_cue() -> AudioCue {
        let mut c = AudioCue::new();
        c.set_name("Test Audio".to_string());
        c.set_number(Some("1".to_string()));
        c.volume_db = -6.0;
        c.pan = 0.5;
        c.fade_in = Some(FadeSpec::new(500));
        c.pre_wait = Duration::from_millis(1000);
        c.post_wait = Duration::from_millis(200);
        c.continue_mode = ContinueMode::AutoContinue;
        c
    }

    #[test]
    fn serialize_roundtrip() {
        let cue = make_cue();
        let json = cue.serialize();

        let factory = AudioCueFactory;
        let restored = factory.from_json(json).expect("should deserialize");

        assert_eq!(restored.name(), "Test Audio");
        assert_eq!(restored.number(), Some("1"));
        assert_eq!(restored.continue_mode(), ContinueMode::AutoContinue);
    }

    #[test]
    fn serialize_roundtrip_preserves_cached_duration() {
        let mut cue = make_cue();
        cue.cached_duration = Some(Duration::from_millis(1234));
        let json = cue.serialize();
        assert_eq!(json["cached_duration_ms"], 1234);
        let restored = AudioCueFactory.from_json(json).expect("should deserialize");
        assert_eq!(restored.serialize()["cached_duration_ms"], 1234);
    }

    #[test]
    fn serialize_roundtrip_preserves_slices() {
        use crate::cue::types::{SliceList, PLAY_COUNT_INFINITE};
        let mut cue = make_cue();
        cue.slices = SliceList {
            markers: vec![2000, 8000],
            play_counts: vec![1, PLAY_COUNT_INFINITE, 2],
        };
        let json = cue.serialize();
        let restored = AudioCueFactory.from_json(json).expect("deserialize");
        let restored_json = restored.serialize();
        let slices: SliceList =
            serde_json::from_value(restored_json.get("slices").unwrap().clone()).unwrap();
        assert_eq!(slices.markers, vec![2000, 8000]);
        assert_eq!(slices.play_counts, vec![1, PLAY_COUNT_INFINITE, 2]);
    }

    #[test]
    fn legacy_json_without_slices_loads_empty() {
        let cue = make_cue();
        let mut json = cue.serialize();
        json.as_object_mut().unwrap().remove("slices");
        let restored = AudioCueFactory.from_json(json).expect("deserialize");
        let restored_json = restored.serialize();
        let slices: crate::cue::types::SliceList =
            serde_json::from_value(restored_json.get("slices").unwrap().clone()).unwrap();
        assert!(slices.is_empty());
    }

    #[test]
    fn initial_state_is_standby() {
        let cue = AudioCue::new();
        assert_eq!(cue.state(), CueState::Standby);
        assert!(!cue.is_running());
        assert!(!cue.is_paused());
    }

    #[test]
    fn metadata_preload_does_not_start_decoder_worker() {
        let mut cue = AudioCue::new();
        cue.accept_preloaded_stream(
            PathBuf::from("metadata-only.wav"),
            2,
            48_000,
            Some(Duration::from_secs(2)),
        );
        assert!(cue.stream_source.is_none());
        assert_eq!(cue.stream_path.as_deref(), Some(Path::new("metadata-only.wav")));
        assert_eq!(cue.cached_duration, Some(Duration::from_secs(2)));
    }

    #[test]
    fn elapsed_zero_before_go() {
        let cue = AudioCue::new();
        assert_eq!(cue.elapsed(), Duration::ZERO);
        assert_eq!(cue.action_elapsed(), Duration::ZERO);
    }

    #[test]
    fn cue_number_is_string() {
        let mut cue = AudioCue::new();
        cue.set_number(Some("1.5.1".to_string()));
        assert_eq!(cue.number(), Some("1.5.1"));
        cue.set_number(Some("Intro".to_string()));
        assert_eq!(cue.number(), Some("Intro"));
        cue.set_number(None);
        assert_eq!(cue.number(), None);
    }
}
