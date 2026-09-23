//! Shared media decoding: extract an audio track to interleaved f32 samples.
//!
//! Used by both [`AudioCue`](super::audio_cue::AudioCue) (audio files) and
//! [`VideoCue`](super::video_cue::VideoCue) (the audio track of a video
//! container).
//!
//! Decode chain:
//! 1. Symphonia with gapless enabled  (handles most MP3/WAV/FLAC/OGG/AAC)
//! 2. Symphonia with gapless disabled (handles MP3s with malformed Xing/LAME headers)
//! 3. libmpv → temp WAV → symphonia   (handles anything ffmpeg can read: MP2,
//!    unusual MPEG encodings, edge-case containers)

use std::cell::UnsafeCell;
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BinaryHeap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use uuid::Uuid;

/// Streaming audio is deliberately bounded.  The worker aims for four seconds
/// of queued PCM, while the ten second ring is a hard memory/backpressure cap.
pub const STREAM_TARGET_SECONDS: usize = 4;
pub const STREAM_READY_MILLIS: usize = 750;
pub const STREAM_MAX_SECONDS: usize = 10;
/// Four workers keep several simultaneous cues from starving each other while
/// the queue remains bounded. A source still owns at most one active job.
const STREAM_WORKERS: usize = 4;
const STREAM_JOB_QUEUE: usize = 32;
const STREAM_LOW_WATERMARK_NUMERATOR: usize = 1;
const STREAM_LOW_WATERMARK_DENOMINATOR: usize = 2;
const STREAM_REFILL_POLL_MILLIS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioStreamInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub total_frames: Option<u64>,
}

/// Probe the first audio track without decoding PCM. Workspace preload uses
/// this cheap metadata pass, then starts a bounded source that keeps only its
/// small ready/target window in memory.
pub fn probe_audio_track(path: &Path) -> Result<Option<AudioStreamInfo>> {
    let Some(decoder) = stream_decoder(path, true).or_else(|_| stream_decoder(path, false))? else {
        return Ok(None);
    };
    Ok(Some(AudioStreamInfo {
        channels: decoder.channels,
        sample_rate: decoder.sample_rate,
        total_frames: decoder.total_frames,
    }))
}

/// Legacy PCM fallback is allowed only when probing/decoding support fails.
/// A successful probe with no audio track must remain a no-audio result.
pub(crate) fn should_use_legacy_fallback(probe: &Result<Option<AudioStreamInfo>>) -> bool {
    probe.is_err()
}

/// A bounded, single-consumer PCM source.  The cpal callback owns the consumer
/// exclusively; decoder workers own the producer.  No callback path takes a
/// lock, allocates, performs I/O, or waits for the decoder.
pub struct StreamingAudioSource {
    source_id: Uuid,
    path: PathBuf,
    pub channels: u16,
    pub sample_rate: u32,
    ring: Arc<PcmRing>,
    buffered_samples: AtomicUsize,
    cancel: AtomicBool,
    seek_generation: AtomicU64,
    applied_generation: AtomicU64,
    requested_frame: AtomicU64,
    decoded_frames: AtomicU64,
    total_frames: AtomicU64,
    eof: AtomicBool,
    ready: AtomicBool,
    underruns: AtomicU64,
    decode_failures: AtomicU64,
    reusable: AtomicBool,
    underrun_reported: AtomicU64,
    refill_requested: AtomicBool,
    playback_state: AtomicU8,
    decoder_session: Mutex<Option<DecoderSession>>,
    job_requested: AtomicBool,
    job_running: AtomicBool,
    pool_registered: AtomicBool,
    playing_accounted: AtomicBool,
    silent_frames: AtomicU64,
    initial_underruns: AtomicU64,
    regular_underruns: AtomicU64,
    min_buffered_frames: AtomicUsize,
    max_buffered_frames: AtomicUsize,
    requested_at_ms: AtomicU64,
    last_refill_wait_us: AtomicU64,
    max_refill_wait_us: AtomicU64,
    last_decode_us: AtomicU64,
    max_decode_us: AtomicU64,
}

struct PcmRing {
    slots: Box<[UnsafeCell<f32>]>,
    slot_generations: Box<[AtomicU64]>,
    channels: usize,
    capacity_frames: usize,
    read: AtomicUsize,
    write: AtomicUsize,
    generation: AtomicU64,
}

// SAFETY: one decoder producer and one cpal consumer access disjoint slots;
// indices use acquire/release ordering and are reset only by the consumer.
unsafe impl Send for PcmRing {}
unsafe impl Sync for PcmRing {}

impl PcmRing {
    fn new(capacity_samples: usize, channels: usize) -> Arc<Self> {
        let capacity_frames = (capacity_samples / channels.max(1)).max(2);
        let slot_count = capacity_frames * channels.max(1);
        let mut slots = Vec::with_capacity(slot_count);
        slots.resize_with(slot_count, || UnsafeCell::new(0.0));
        let mut slot_generations = Vec::with_capacity(capacity_frames);
        slot_generations.resize_with(capacity_frames, || AtomicU64::new(0));
        Arc::new(Self {
            slots: slots.into_boxed_slice(),
            slot_generations: slot_generations.into_boxed_slice(),
            channels: channels.max(1),
            capacity_frames,
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            generation: AtomicU64::new(1),
        })
    }
    fn reset(&self, generation: u64) {
        let write = self.write.load(Ordering::Acquire);
        self.read.store(write, Ordering::Release);
        self.generation.store(generation, Ordering::Release);
    }
    fn push_frame(&self, values: &[f32], generation: u64) -> Result<(), ()> {
        if self.generation.load(Ordering::Acquire) != generation {
            return Err(());
        }
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if values.len() != self.channels || write.wrapping_sub(read) >= self.capacity_frames {
            return Err(());
        }
        let base = (write % self.capacity_frames) * self.channels;
        for (index, value) in values.iter().enumerate() {
            unsafe {
                *self.slots[base + index].get() = *value;
            }
        }
        self.slot_generations[write % self.capacity_frames].store(generation, Ordering::Release);
        if self.generation.load(Ordering::Acquire) != generation {
            return Err(());
        }
        self.write.store(write.wrapping_add(1), Ordering::Release);
        Ok(())
    }
    fn pop_frame(&self, out: &mut [f32; 2]) -> bool {
        let read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        if read == write {
            return false;
        }
        let slot = read % self.capacity_frames;
        let base = slot * self.channels;
        if self.slot_generations[slot].load(Ordering::Acquire)
            != self.generation.load(Ordering::Acquire)
        {
            self.read.store(read.wrapping_add(1), Ordering::Release);
            return false;
        }
        out[0] = unsafe { *self.slots[base].get() };
        out[1] = if self.channels > 1 {
            unsafe { *self.slots[base + 1].get() }
        } else {
            out[0]
        };
        self.read.store(read.wrapping_add(1), Ordering::Release);
        true
    }
}

// SAFETY: only the audio callback touches `consumer`; the producer half is
// moved into exactly one decoder worker. All cross-thread control is atomic.
unsafe impl Send for StreamingAudioSource {}
unsafe impl Sync for StreamingAudioSource {}

impl StreamingAudioSource {
    pub fn start(path: PathBuf, info: AudioStreamInfo) -> Result<Arc<Self>> {
        Self::start_at(path, info, 0)
    }

    pub fn start_at(path: PathBuf, info: AudioStreamInfo, initial_frame: u64) -> Result<Arc<Self>> {
        let channels = info.channels.max(1) as usize;
        let capacity = (info.sample_rate.max(8_000) as usize)
            .saturating_mul(channels)
            .saturating_mul(STREAM_MAX_SECONDS)
            .max(1024);
        let ring = PcmRing::new(capacity, channels);
        let capacity_frames = capacity / channels.max(1);
        let source = Arc::new(Self {
            source_id: Uuid::new_v4(),
            path,
            channels: info.channels.max(1),
            sample_rate: info.sample_rate.max(1),
            ring: Arc::clone(&ring),
            buffered_samples: AtomicUsize::new(0),
            cancel: AtomicBool::new(false),
            seek_generation: AtomicU64::new(1),
            applied_generation: AtomicU64::new(1),
            requested_frame: AtomicU64::new(initial_frame),
            decoded_frames: AtomicU64::new(0),
            total_frames: AtomicU64::new(info.total_frames.unwrap_or(0)),
            eof: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            decode_failures: AtomicU64::new(0),
            reusable: AtomicBool::new(false),
            underrun_reported: AtomicU64::new(0),
            refill_requested: AtomicBool::new(true),
            // 0 = paused, 1 = preload/idle, 2 = playing normal.
            playback_state: AtomicU8::new(1),
            decoder_session: Mutex::new(None),
            job_requested: AtomicBool::new(false),
            job_running: AtomicBool::new(false),
            pool_registered: AtomicBool::new(true),
            playing_accounted: AtomicBool::new(false),
            silent_frames: AtomicU64::new(0),
            initial_underruns: AtomicU64::new(0),
            regular_underruns: AtomicU64::new(0),
            min_buffered_frames: AtomicUsize::new(capacity_frames),
            max_buffered_frames: AtomicUsize::new(0),
            requested_at_ms: AtomicU64::new(0),
            last_refill_wait_us: AtomicU64::new(0),
            max_refill_wait_us: AtomicU64::new(0),
            last_decode_us: AtomicU64::new(0),
            max_decode_us: AtomicU64::new(0),
        });
        stream_pool().register(&source);
        source.request_refill();
        Ok(source)
    }

    /// Build a source without a decoder for RT lifecycle tests.  The helper
    /// deliberately bypasses the global worker pool: tests can place exact
    /// PCM and terminal state into the bounded ring, then exercise the real
    /// audio callback deterministically.
    #[cfg(test)]
    pub(crate) fn test_from_pcm(
        frames: &[[f32; 2]],
        ready: bool,
        eof: bool,
    ) -> Arc<Self> {
        let capacity_frames = frames.len().max(2) + 1;
        let ring = PcmRing::new(capacity_frames * 2, 2);
        for frame in frames {
            ring.push_frame(frame, 1)
                .expect("test PCM must fit in the source ring");
        }
        Arc::new(Self {
            source_id: Uuid::new_v4(),
            path: PathBuf::from("test-stream.wav"),
            channels: 2,
            sample_rate: 48_000,
            ring,
            buffered_samples: AtomicUsize::new(frames.len() * 2),
            cancel: AtomicBool::new(false),
            seek_generation: AtomicU64::new(1),
            applied_generation: AtomicU64::new(1),
            requested_frame: AtomicU64::new(0),
            decoded_frames: AtomicU64::new(frames.len() as u64),
            total_frames: AtomicU64::new(frames.len() as u64),
            eof: AtomicBool::new(eof),
            ready: AtomicBool::new(ready),
            underruns: AtomicU64::new(0),
            decode_failures: AtomicU64::new(0),
            reusable: AtomicBool::new(false),
            underrun_reported: AtomicU64::new(0),
            refill_requested: AtomicBool::new(!eof),
            playback_state: AtomicU8::new(StreamPlaybackState::Playing as u8),
            decoder_session: Mutex::new(None),
            job_requested: AtomicBool::new(false),
            job_running: AtomicBool::new(false),
            pool_registered: AtomicBool::new(false),
            playing_accounted: AtomicBool::new(false),
            silent_frames: AtomicU64::new(0),
            initial_underruns: AtomicU64::new(0),
            regular_underruns: AtomicU64::new(0),
            min_buffered_frames: AtomicUsize::new(frames.len()),
            max_buffered_frames: AtomicUsize::new(frames.len()),
            requested_at_ms: AtomicU64::new(0),
            last_refill_wait_us: AtomicU64::new(0),
            max_refill_wait_us: AtomicU64::new(0),
            last_decode_us: AtomicU64::new(0),
            max_decode_us: AtomicU64::new(0),
        })
    }

    pub fn total_frames(&self) -> u64 {
        self.total_frames.load(Ordering::Acquire)
    }

    pub fn id(&self) -> Uuid { self.source_id }

    pub fn path(&self) -> &Path { &self.path }

    pub fn capacity_frames(&self) -> usize { self.ring.capacity_frames }

    pub fn capacity_mib(&self) -> f64 {
        (self.ring.capacity_frames * self.channels.max(1) as usize * std::mem::size_of::<f32>()) as f64
            / (1024.0 * 1024.0)
    }

    pub fn capacity_bytes(&self) -> u64 {
        (self.ring.capacity_frames * self.channels.max(1) as usize * std::mem::size_of::<f32>()) as u64
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
    pub fn requested_frame(&self) -> u64 {
        self.requested_frame.load(Ordering::Acquire)
    }
    pub fn seek_pending(&self) -> bool {
        self.seek_generation.load(Ordering::Acquire)
            != self.applied_generation.load(Ordering::Acquire)
    }
    pub fn is_eof(&self) -> bool {
        self.eof.load(Ordering::Acquire)
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
    pub fn decode_failures(&self) -> u64 {
        self.decode_failures.load(Ordering::Relaxed)
    }
    pub fn buffered_samples(&self) -> usize {
        self.buffered_samples.load(Ordering::Acquire)
    }
    pub fn note_underrun(&self) {
        self.note_underrun_frames(1);
    }

    pub fn note_underrun_frames(&self, frames: usize) {
        self.refill_requested.store(true, Ordering::Release);
        self.underruns.fetch_add(1, Ordering::Relaxed);
        self.silent_frames.fetch_add(frames as u64, Ordering::Relaxed);
        if self.is_ready() {
            self.regular_underruns.fetch_add(1, Ordering::Relaxed);
        } else {
            self.initial_underruns.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn keep_worker_for_loop(self: &Arc<Self>) {
        self.reusable.store(true, Ordering::Release);
        // A short non-loop source can hit EOF before the cue finishes setting
        // its loop policy. Restart only after the ring is empty; otherwise a
        // second decode would duplicate still-buffered PCM.
        if self.is_eof() && self.buffered_samples() == 0 {
            // The source may have reached EOF before the cue's loop policy
            // was published. Re-arm a fresh generation and reset the ring on
            // this control path; `request_refill` intentionally rejects EOF.
            let _ = self.prepare_seek(0);
        }
    }
    pub fn should_report_underrun(&self, count: u64) -> bool {
        let threshold = (count / 4096) * 4096;
        threshold > 0
            && self
                .underrun_reported
                .compare_exchange(
                    0.max(threshold.saturating_sub(4096)),
                    threshold,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok()
    }

    /// Publish a new source generation. This is safe from a control thread and
    /// does not touch the callback-owned ring consumer index.
    pub fn request_seek(self: &Arc<Self>, frame: u64) {
        self.requested_frame.store(frame, Ordering::Release);
        self.seek_generation.fetch_add(1, Ordering::AcqRel);
        self.eof.store(false, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        self.decoded_frames.store(0, Ordering::Release);
        self.refill_requested.store(true, Ordering::Release);
    }

    /// Apply the already-published generation on the RT side. Ring indices are
    /// reset only by the callback (or before a voice is submitted), so a
    /// control-thread seek cannot race the consumer's read index.
    pub fn apply_seek_rt(self: &Arc<Self>) {
        let generation = self.seek_generation.load(Ordering::Acquire);
        if self.applied_generation.load(Ordering::Acquire) != generation {
            self.buffered_samples.store(0, Ordering::Release);
            self.ring.reset(generation);
            self.applied_generation.store(generation, Ordering::Release);
        }
    }

    /// Seek used by loop/slice boundaries that are already executing on RT.
    pub fn request_seek_rt(self: &Arc<Self>, frame: u64) {
        self.request_seek(frame);
        self.apply_seek_rt();
    }

    /// Prepare a seek from a non-real-time control path. A seek after EOF gets
    /// a fresh bounded worker job; submission is try-send and never blocks the
    /// UI or the audio callback.
    pub fn prepare_seek(self: &Arc<Self>, frame: u64) -> Result<()> {
        self.request_seek(frame);
        let generation = self.seek_generation.load(Ordering::Acquire);
        self.buffered_samples.store(0, Ordering::Release);
        self.ring.reset(generation);
        self.applied_generation.store(generation, Ordering::Release);
        self.request_refill();
        Ok(())
    }

    pub fn cancel(&self) {
        if !self.cancel.swap(true, Ordering::Release) {
            self.account_playing(false);
        }
        let generation = self.seek_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.buffered_samples.store(0, Ordering::Release);
        self.ring.reset(generation);
        self.applied_generation.store(generation, Ordering::Release);
    }

    /// Pop one interleaved frame.  Missing data is intentional silence and is
    /// counted for diagnostics; the callback never waits for the worker.
    pub fn pop_frame(&self, out: &mut [f32; 2]) -> bool {
        let channels = self.channels.max(1) as usize;
        let ok = self.ring.pop_frame(out);
        if ok {
            for _ in 0..channels {
                self.decrement_buffered();
            }
            if channels == 1 {
                out[1] = out[0];
            }
        } else {
            *out = [0.0; 2];
            // EOF is a terminal, expected empty state.  Do not wake the
            // scheduler or count silence after the decoder has said that no
            // more frames can arrive.  A seek clears EOF before requesting a
            // new generation, so a post-seek empty ring remains refillable.
            if !self.is_eof() && !self.is_cancelled() {
                self.refill_requested.store(true, Ordering::Release);
                self.underruns.fetch_add(1, Ordering::Relaxed);
                if self.is_ready() {
                    self.regular_underruns.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.initial_underruns.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        ok
    }

    fn ready_threshold(&self) -> usize {
        self.sample_rate as usize * self.channels.max(1) as usize * STREAM_READY_MILLIS / 1000
    }

    fn target_threshold(&self) -> usize {
        self.sample_rate as usize * self.channels.max(1) as usize * STREAM_TARGET_SECONDS
    }

    fn low_watermark(&self) -> usize {
        self.target_threshold()
            .saturating_mul(STREAM_LOW_WATERMARK_NUMERATOR)
            / STREAM_LOW_WATERMARK_DENOMINATOR
    }

    fn needs_refill(&self) -> bool {
        !self.cancel.load(Ordering::Acquire)
            && !self.eof.load(Ordering::Acquire)
            && !(self.playback_state.load(Ordering::Acquire) == StreamPlaybackState::Paused as u8
                && self.is_ready())
            && (self.refill_requested.load(Ordering::Acquire)
                || self.buffered_samples() <= self.low_watermark())
    }

    fn request_refill(self: &Arc<Self>) {
        if self.cancel.load(Ordering::Acquire) || self.eof.load(Ordering::Acquire) {
            return;
        }
        self.refill_requested.store(true, Ordering::Release);
        if self
            .job_requested
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.requested_at_ms.store(now_millis(), Ordering::Release);
            stream_pool().enqueue(Arc::clone(self));
        }
    }

    pub fn set_playback_state(&self, state: StreamPlaybackState) {
        let previous = self.playback_state.swap(state as u8, Ordering::AcqRel);
        if previous != StreamPlaybackState::Playing as u8
            && state == StreamPlaybackState::Playing
        {
            self.account_playing(true);
        } else if previous == StreamPlaybackState::Playing as u8
            && state != StreamPlaybackState::Playing
        {
            self.account_playing(false);
        }
        if state != StreamPlaybackState::Paused && !self.is_eof() {
            self.refill_requested.store(true, Ordering::Release);
        }
    }

    pub fn is_worker_active(&self) -> bool {
        self.job_running.load(Ordering::Acquire)
    }

    pub fn diagnostics(&self) -> StreamSourceDiagnostics {
        let buffered_frames = self.buffered_samples() / self.channels.max(1) as usize;
        update_min(&self.min_buffered_frames, buffered_frames);
        update_max(&self.max_buffered_frames, buffered_frames);
        StreamSourceDiagnostics {
            source_id: self.source_id,
            path: self.path.to_string_lossy().into_owned(),
            channels: self.channels,
            sample_rate: self.sample_rate,
            capacity_frames: self.capacity_frames(),
            capacity_mib: self.capacity_mib(),
            state: match self.playback_state.load(Ordering::Acquire) {
                2 => StreamPlaybackState::Playing,
                0 => StreamPlaybackState::Paused,
                _ => StreamPlaybackState::Preload,
            },
            buffered_frames,
            buffered_seconds: buffered_frames as f64 / self.sample_rate.max(1) as f64,
            buffered_mib: (self.buffered_samples() * std::mem::size_of::<f32>()) as f64
                / (1024.0 * 1024.0),
            ready: self.is_ready(),
            eof: self.is_eof(),
            cancelled: self.is_cancelled(),
            job_requested: self.job_requested.load(Ordering::Acquire),
            job_running: self.job_running.load(Ordering::Acquire),
            refill_requested: self.refill_requested.load(Ordering::Acquire),
            underruns: self.underruns(),
            decode_failures: self.decode_failures(),
            silent_frames: self.silent_frames.load(Ordering::Relaxed),
            initial_underruns: self.initial_underruns.load(Ordering::Relaxed),
            regular_underruns: self.regular_underruns.load(Ordering::Relaxed),
            min_buffered_frames: self.min_buffered_frames.load(Ordering::Relaxed),
            max_buffered_frames: self.max_buffered_frames.load(Ordering::Relaxed),
            last_refill_wait_us: self.last_refill_wait_us.load(Ordering::Relaxed),
            max_refill_wait_us: self.max_refill_wait_us.load(Ordering::Relaxed),
            last_decode_us: self.last_decode_us.load(Ordering::Relaxed),
            max_decode_us: self.max_decode_us.load(Ordering::Relaxed),
        }
    }

    fn reset_diagnostics(&self) {
        self.underruns.store(0, Ordering::Release);
        self.decode_failures.store(0, Ordering::Release);
        self.silent_frames.store(0, Ordering::Release);
        self.initial_underruns.store(0, Ordering::Release);
        self.regular_underruns.store(0, Ordering::Release);
        let buffered = self.buffered_samples() / self.channels.max(1) as usize;
        self.min_buffered_frames.store(buffered, Ordering::Release);
        self.max_buffered_frames.store(buffered, Ordering::Release);
        self.last_refill_wait_us.store(0, Ordering::Release);
        self.max_refill_wait_us.store(0, Ordering::Release);
        self.last_decode_us.store(0, Ordering::Release);
        self.max_decode_us.store(0, Ordering::Release);
    }

    fn priority(&self) -> u8 {
        match (
            self.playback_state.load(Ordering::Acquire),
            self.is_ready(),
            self.needs_refill(),
        ) {
            (2, false, _) => 5,    // playing and not ready
            (2, true, true) => 4,  // playing with a low buffer
            (2, true, false) => 3, // playing normal
            (1, false, _) => 2,    // GO/preload source that is not ready
            (1, true, _) => 1,     // ordinary preload
            _ => 0,                // paused
        }
    }

    fn decrement_buffered(&self) {
        let _ = self
            .buffered_samples
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(1))
            });
    }

    fn account_playing(&self, playing: bool) {
        if !self.pool_registered.load(Ordering::Acquire) {
            return;
        }
        if playing {
            if self
                .playing_accounted
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                stream_pool().account_playing_delta(1);
            }
        } else if self
            .playing_accounted
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            stream_pool().account_playing_delta(-1);
        }
    }
}

impl Drop for StreamingAudioSource {
    fn drop(&mut self) {
        if self.pool_registered.load(Ordering::Acquire) {
            stream_pool().release_source(self);
        }
    }
}

struct StreamJob {
    source: Arc<StreamingAudioSource>,
    ring: Arc<PcmRing>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPlaybackState {
    Paused = 0,
    Preload = 1,
    Playing = 2,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamSourceDiagnostics {
    pub source_id: Uuid,
    pub path: String,
    pub channels: u16,
    pub sample_rate: u32,
    pub capacity_frames: usize,
    pub capacity_mib: f64,
    pub state: StreamPlaybackState,
    pub buffered_frames: usize,
    pub buffered_seconds: f64,
    pub buffered_mib: f64,
    pub ready: bool,
    pub eof: bool,
    pub cancelled: bool,
    pub job_requested: bool,
    pub job_running: bool,
    pub refill_requested: bool,
    pub underruns: u64,
    pub decode_failures: u64,
    pub silent_frames: u64,
    pub initial_underruns: u64,
    pub regular_underruns: u64,
    pub min_buffered_frames: usize,
    pub max_buffered_frames: usize,
    pub last_refill_wait_us: u64,
    pub max_refill_wait_us: u64,
    pub last_decode_us: u64,
    pub max_decode_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPoolDiagnostics {
    pub registered_sources: usize,
    pub live_sources: usize,
    pub pending_jobs: usize,
    pub active_workers: usize,
    pub worker_count: usize,
    pub peak_active_workers: usize,
    pub peak_pending_jobs: usize,
    pub completed_jobs: u64,
    pub priority_urgent: usize,
    pub priority_playback: usize,
    pub priority_preload: usize,
    pub priority_paused: usize,
    pub peak_playing_sources: usize,
    pub peak_streaming_memory_bytes: u64,
    pub current_sources: usize,
    pub current_streaming_memory_bytes: u64,
    pub current_playing_sources: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamDiagnosticEvent {
    pub at_ms: u64,
    pub source_id: Uuid,
    pub source_label: String,
    pub kind: String,
    pub detail: String,
}

struct PendingStreamJob {
    priority: u8,
    sequence: u64,
    source: Arc<StreamingAudioSource>,
}

impl PartialEq for PendingStreamJob {
    fn eq(&self, other: &Self) -> bool {
        self.sequence == other.sequence
    }
}
impl Eq for PendingStreamJob {}
impl PartialOrd for PendingStreamJob {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl Ord for PendingStreamJob {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

struct StreamWorkerPool {
    request_tx: mpsc::Sender<Arc<StreamingAudioSource>>,
    registry: Arc<Mutex<Vec<Weak<StreamingAudioSource>>>>,
    pending_jobs: Arc<AtomicUsize>,
    active_workers: Arc<AtomicUsize>,
    peak_active_workers: Arc<AtomicUsize>,
    peak_pending_jobs: Arc<AtomicUsize>,
    completed_jobs: Arc<AtomicU64>,
    events: Arc<Mutex<VecDeque<StreamDiagnosticEvent>>>,
    event_cursors: Arc<Mutex<std::collections::HashMap<Uuid, (u64, u64)>>>,
    priority_counts: Arc<[AtomicUsize; 4]>,
    peak_playing_sources: Arc<AtomicUsize>,
    peak_streaming_memory_bytes: Arc<AtomicU64>,
    current_sources: Arc<AtomicUsize>,
    current_streaming_memory_bytes: Arc<AtomicU64>,
    current_playing_sources: Arc<AtomicUsize>,
    completed_sources: Arc<Mutex<VecDeque<StreamSourceDiagnostics>>>,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

fn update_max(target: &AtomicUsize, value: usize) {
    let mut current = target.load(Ordering::Relaxed);
    while value > current {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(next) => current = next,
        }
    }
}

fn update_max_u64(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value > current {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(next) => current = next,
        }
    }
}

fn saturating_sub(target: &AtomicUsize, amount: usize) -> usize {
    let previous = target
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            Some(value.saturating_sub(amount))
        })
        .unwrap_or(0);
    previous.saturating_sub(amount)
}

fn saturating_sub_u64(target: &AtomicU64, amount: u64) -> u64 {
    let previous = target
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            Some(value.saturating_sub(amount))
        })
        .unwrap_or(0);
    previous.saturating_sub(amount)
}

fn update_min(target: &AtomicUsize, value: usize) {
    let mut current = target.load(Ordering::Relaxed);
    while value < current {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(next) => current = next,
        }
    }
}

fn stream_pool() -> &'static StreamWorkerPool {
    static POOL: OnceLock<StreamWorkerPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let (request_tx, request_rx) = mpsc::channel::<Arc<StreamingAudioSource>>();
        let (tx, rx) = mpsc::sync_channel::<StreamJob>(STREAM_JOB_QUEUE);
        let rx = Arc::new(std::sync::Mutex::new(rx));
        let active_workers = Arc::new(AtomicUsize::new(0));
        let pending_jobs = Arc::new(AtomicUsize::new(0));
        let peak_active_workers = Arc::new(AtomicUsize::new(0));
        let peak_pending_jobs = Arc::new(AtomicUsize::new(0));
        let completed_jobs = Arc::new(AtomicU64::new(0));
        let events = Arc::new(Mutex::new(VecDeque::with_capacity(256)));
        let event_cursors = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let priority_counts = Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
        let peak_playing_sources = Arc::new(AtomicUsize::new(0));
        let peak_streaming_memory_bytes = Arc::new(AtomicU64::new(0));
        let current_sources = Arc::new(AtomicUsize::new(0));
        let current_streaming_memory_bytes = Arc::new(AtomicU64::new(0));
        let current_playing_sources = Arc::new(AtomicUsize::new(0));
        let completed_sources = Arc::new(Mutex::new(VecDeque::with_capacity(256)));
        for index in 0..STREAM_WORKERS {
            let rx = Arc::clone(&rx);
            let active_workers = Arc::clone(&active_workers);
            let peak_active_workers = Arc::clone(&peak_active_workers);
            let completed_jobs = Arc::clone(&completed_jobs);
            let _ = thread::Builder::new()
                .name(format!("qlisa-audio-decoder-{index}"))
                .spawn(move || loop {
                    let job = rx.lock().ok().and_then(|r| r.recv().ok());
                    let Some(job) = job else {
                        break;
                    };
                    let source = Arc::clone(&job.source);
                    source.job_running.store(true, Ordering::Release);
                    let active = active_workers.fetch_add(1, Ordering::AcqRel) + 1;
                    update_max(&peak_active_workers, active);
                    let requested_at = source.requested_at_ms.swap(0, Ordering::AcqRel);
                    if requested_at > 0 {
                        let wait_us = now_millis().saturating_sub(requested_at).saturating_mul(1000);
                        source.last_refill_wait_us.store(wait_us, Ordering::Release);
                        update_max_u64(&source.max_refill_wait_us, wait_us);
                    }
                    let started_at = Instant::now();
                    run_stream_job(job);
                    let decode_us = started_at.elapsed().as_micros() as u64;
                    source.last_decode_us.store(decode_us, Ordering::Release);
                    update_max_u64(&source.max_decode_us, decode_us);
                    active_workers.fetch_sub(1, Ordering::AcqRel);
                    completed_jobs.fetch_add(1, Ordering::Relaxed);
                    source.job_running.store(false, Ordering::Release);
                    source.job_requested.store(false, Ordering::Release);
                    if source.needs_refill() {
                        source.request_refill();
                    }
                });
        }
        let registry = Arc::new(Mutex::new(Vec::<Weak<StreamingAudioSource>>::new()));
        let scheduler_registry = Arc::clone(&registry);
        let scheduler_pending_jobs = Arc::clone(&pending_jobs);
        let scheduler_peak_pending_jobs = Arc::clone(&peak_pending_jobs);
        let scheduler_priority_counts = Arc::clone(&priority_counts);
        thread::Builder::new()
            .name("qlisa-audio-decoder-scheduler".into())
            .spawn(move || {
                let mut pending = BinaryHeap::<PendingStreamJob>::new();
                let mut sequence = 0_u64;
                loop {
                    while let Ok(source) = request_rx.try_recv() {
                        pending.push(PendingStreamJob {
                            priority: source.priority(),
                            sequence,
                            source,
                        });
                        sequence = sequence.wrapping_add(1);
                    }
                    if let Ok(mut registry) = scheduler_registry.lock() {
                        registry.retain(|weak| weak.strong_count() > 0);
                        for weak in registry.iter() {
                            if let Some(source) = weak.upgrade() {
                                if source.needs_refill()
                                    && source
                                        .job_requested
                                        .compare_exchange(
                                            false,
                                            true,
                                            Ordering::AcqRel,
                                            Ordering::Acquire,
                                        )
                                        .is_ok()
                                {
                                    pending.push(PendingStreamJob {
                                        priority: source.priority(),
                                        sequence,
                                        source,
                                    });
                                    sequence = sequence.wrapping_add(1);
                                }
                            }
                        }
                    }
                    if !pending.is_empty() {
                        let mut refreshed = BinaryHeap::new();
                        while let Some(mut job) = pending.pop() {
                            job.priority = job.source.priority();
                            refreshed.push(job);
                        }
                        pending = refreshed;
                    }
                    scheduler_pending_jobs.store(pending.len(), Ordering::Release);
                    update_max(&scheduler_peak_pending_jobs, pending.len());
                    let mut priority = [0usize; 4];
                    for job in pending.iter() {
                        match job.priority {
                            5 => priority[0] += 1,
                            3 | 4 => priority[1] += 1,
                            1 | 2 => priority[2] += 1,
                            _ => priority[3] += 1,
                        }
                    }
                    for (index, value) in priority.into_iter().enumerate() {
                        scheduler_priority_counts[index].store(value, Ordering::Release);
                    }
                    if !dispatch_pending_job(&tx, &mut pending) {
                        thread::sleep(Duration::from_millis(STREAM_REFILL_POLL_MILLIS));
                    }
                }
            })
            .expect("audio decoder scheduler thread");
        StreamWorkerPool {
            request_tx,
            registry,
            pending_jobs,
            active_workers,
            peak_active_workers,
            peak_pending_jobs,
            completed_jobs,
            events,
            event_cursors,
            priority_counts,
            peak_playing_sources,
            peak_streaming_memory_bytes,
            current_sources,
            current_streaming_memory_bytes,
            current_playing_sources,
            completed_sources,
        }
    })
}

fn dispatch_pending_job(
    tx: &mpsc::SyncSender<StreamJob>,
    pending: &mut BinaryHeap<PendingStreamJob>,
) -> bool {
    let Some(job) = pending.pop() else { return false; };
    if job.source.cancel.load(Ordering::Acquire) || !job.source.needs_refill() {
        job.source.job_requested.store(false, Ordering::Release);
        return true;
    }
    match tx.try_send(StreamJob {
        source: Arc::clone(&job.source),
        ring: Arc::clone(&job.source.ring),
    }) {
        Ok(()) => true,
        Err(_) => {
            // Keep the source's dedup flag set and retain the logical job.
            // The scheduler will retry after a worker consumes queue space.
            pending.push(job);
            false
        }
    }
}

impl StreamWorkerPool {
    fn register(&self, source: &Arc<StreamingAudioSource>) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.push(Arc::downgrade(source));
        }
        self.current_sources.fetch_add(1, Ordering::AcqRel);
        let capacity = source.capacity_bytes();
        let current_memory = self
            .current_streaming_memory_bytes
            .fetch_add(capacity, Ordering::AcqRel)
            .saturating_add(capacity);
        update_max_u64(&self.peak_streaming_memory_bytes, current_memory);
        let current_playing = self.current_playing_sources.load(Ordering::Acquire);
        update_max(&self.peak_playing_sources, current_playing);
    }
    fn enqueue(&self, source: Arc<StreamingAudioSource>) {
        let _ = self.request_tx.send(source);
    }

    fn diagnostics(&self) -> StreamPoolDiagnostics {
        let (registered_sources, live_sources) = self
            .registry
            .lock()
            .map(|registry| {
                (
                    registry.len(),
                    registry.iter().filter(|weak| weak.strong_count() > 0).count(),
                )
            })
            .unwrap_or_default();
        StreamPoolDiagnostics {
            registered_sources,
            live_sources,
            pending_jobs: self.pending_jobs.load(Ordering::Acquire),
            active_workers: self.active_workers.load(Ordering::Acquire),
            worker_count: STREAM_WORKERS,
            peak_active_workers: self.peak_active_workers.load(Ordering::Acquire),
            peak_pending_jobs: self.peak_pending_jobs.load(Ordering::Acquire),
            completed_jobs: self.completed_jobs.load(Ordering::Acquire),
            priority_urgent: self.priority_counts[0].load(Ordering::Acquire),
            priority_playback: self.priority_counts[1].load(Ordering::Acquire),
            priority_preload: self.priority_counts[2].load(Ordering::Acquire),
            priority_paused: self.priority_counts[3].load(Ordering::Acquire),
            peak_playing_sources: self.peak_playing_sources.load(Ordering::Acquire),
            peak_streaming_memory_bytes: self.peak_streaming_memory_bytes.load(Ordering::Acquire),
            current_sources: self.current_sources.load(Ordering::Acquire),
            current_streaming_memory_bytes: self.current_streaming_memory_bytes.load(Ordering::Acquire),
            current_playing_sources: self.current_playing_sources.load(Ordering::Acquire),
        }
    }

    fn source_snapshots(&self) -> Vec<StreamSourceDiagnostics> {
        let mut sources: Vec<StreamSourceDiagnostics> = self.registry
            .lock()
            .map(|mut registry| {
                registry.retain(|weak| weak.strong_count() > 0);
                registry
                    .iter()
                    .filter_map(Weak::upgrade)
                    .map(|source| source.diagnostics())
                    .collect()
            })
            .unwrap_or_default();
        if let Ok(mut completed) = self.completed_sources.lock() {
            for source in &sources {
                if source.cancelled && source.eof && source.buffered_frames == 0 {
                    if let Some(existing) = completed.iter_mut().find(|item| item.source_id == source.source_id) {
                        *existing = source.clone();
                    } else {
                        completed.push_back(source.clone());
                    }
                }
            }
            while completed.len() > 256 { completed.pop_front(); }
            let live_ids: std::collections::HashSet<_> = sources.iter().map(|source| source.source_id).collect();
            for record in completed.iter() {
                if !live_ids.contains(&record.source_id) {
                    sources.push(record.clone());
                }
            }
        }
        sources
    }

    fn account_playing_delta(&self, delta: i8) {
        let current = if delta > 0 {
            self.current_playing_sources.fetch_add(delta as usize, Ordering::AcqRel) + delta as usize
        } else {
            saturating_sub(&self.current_playing_sources, delta.unsigned_abs() as usize)
        };
        update_max(&self.peak_playing_sources, current);
    }

    fn release_source(&self, source: &StreamingAudioSource) {
        saturating_sub(&self.current_sources, 1);
        saturating_sub_u64(&self.current_streaming_memory_bytes, source.capacity_bytes());
        if source.playing_accounted.swap(false, Ordering::AcqRel) {
            saturating_sub(&self.current_playing_sources, 1);
        }
    }

    fn event_snapshots(&self) -> Vec<StreamDiagnosticEvent> {
        self.events
            .lock()
            .map(|events| events.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn refresh_events(&self, sources: &[StreamSourceDiagnostics]) {
        let Ok(mut cursors) = self.event_cursors.lock() else { return; };
        let Ok(mut events) = self.events.lock() else { return; };
        for source in sources {
            let cursor = cursors.entry(source.source_id).or_insert((0, 0));
            if source.underruns > cursor.0 {
                events.push_back(StreamDiagnosticEvent {
                    at_ms: now_millis(),
                    source_id: source.source_id,
                    source_label: std::path::Path::new(&source.path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(&source.path)
                        .to_string(),
                    kind: "underrun".into(),
                    detail: format!("Пропуск звука: {}", source.underruns - cursor.0),
                });
            }
            if source.decode_failures > cursor.1 {
                events.push_back(StreamDiagnosticEvent {
                    at_ms: now_millis(),
                    source_id: source.source_id,
                    source_label: std::path::Path::new(&source.path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(&source.path)
                        .to_string(),
                    kind: "decodeError".into(),
                    detail: format!("Ошибка декодирования: {}", source.decode_failures - cursor.1),
                });
            }
            *cursor = (source.underruns, source.decode_failures);
        }
        while events.len() > 256 {
            events.pop_front();
        }
    }

    fn reset_diagnostics(&self) {
        if let Ok(registry) = self.registry.lock() {
            for source in registry.iter().filter_map(Weak::upgrade) {
                source.reset_diagnostics();
            }
        }
        self.peak_active_workers.store(self.active_workers.load(Ordering::Acquire), Ordering::Release);
        self.peak_pending_jobs.store(self.pending_jobs.load(Ordering::Acquire), Ordering::Release);
        self.completed_jobs.store(0, Ordering::Release);
        for counter in self.priority_counts.iter() {
            counter.store(0, Ordering::Release);
        }
        self.peak_playing_sources
            .store(self.current_playing_sources.load(Ordering::Acquire), Ordering::Release);
        self.peak_streaming_memory_bytes.store(
            self.current_streaming_memory_bytes.load(Ordering::Acquire),
            Ordering::Release,
        );
        if let Ok(mut completed) = self.completed_sources.lock() {
            completed.clear();
        }
        if let Ok(mut events) = self.events.lock() {
            events.clear();
        }
        if let Ok(mut cursors) = self.event_cursors.lock() {
            cursors.clear();
        }
    }
}

pub fn stream_pool_diagnostics() -> StreamPoolDiagnostics {
    stream_pool().diagnostics()
}

pub fn stream_source_diagnostics() -> Vec<StreamSourceDiagnostics> {
    let pool = stream_pool();
    let sources = pool.source_snapshots();
    pool.refresh_events(&sources);
    sources
}

pub fn stream_diagnostic_events() -> Vec<StreamDiagnosticEvent> {
    stream_pool().event_snapshots()
}

pub fn reset_stream_diagnostics() {
    stream_pool().reset_diagnostics();
}

struct StreamDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    channels: u16,
    sample_rate: u32,
    total_frames: Option<u64>,
}

struct DecoderSession {
    decoder: StreamDecoder,
    frame: u64,
    generation: u64,
}

fn stream_decoder(path: &Path, gapless: bool) -> Result<Option<StreamDecoder>> {
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let file = std::fs::File::open(path)
        .with_context(|| format!("Cannot open media file: {}", path.display()))?;
    let probed = symphonia::default::get_probe().format(
        &hint,
        MediaSourceStream::new(Box::new(file), Default::default()),
        &FormatOptions {
            enable_gapless: gapless,
            ..Default::default()
        },
        &MetadataOptions::default(),
    )?;
    let format = probed.format;
    let track = match format
        .tracks()
        .iter()
        .find(|t| t.codec_params.sample_rate.is_some())
    {
        Some(t) => t,
        None => return Ok(None),
    };
    let track_id = track.id;
    let params = track.codec_params.clone();
    let channels = params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2)
        .max(1);
    let sample_rate = params.sample_rate.unwrap_or(44_100).max(1);
    let decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default())?;
    Ok(Some(StreamDecoder {
        format,
        decoder,
        track_id,
        channels,
        sample_rate,
        total_frames: params.n_frames,
    }))
}

fn interleaved(decoded: AudioBufferRef<'_>) -> Vec<f32> {
    let frames = decoded.frames();
    let channels = decoded.spec().channels.count();
    let mut out = Vec::with_capacity(frames * channels);
    match decoded {
        AudioBufferRef::F32(buf) => {
            for frame in 0..frames {
                for ch in 0..channels {
                    out.push(buf.chan(ch)[frame]);
                }
            }
        }
        AudioBufferRef::S16(buf) => {
            for frame in 0..frames {
                for ch in 0..channels {
                    out.push(buf.chan(ch)[frame] as f32 / i16::MAX as f32);
                }
            }
        }
        AudioBufferRef::S32(buf) => {
            for frame in 0..frames {
                for ch in 0..channels {
                    out.push(buf.chan(ch)[frame] as f32 / i32::MAX as f32);
                }
            }
        }
        AudioBufferRef::U8(buf) => {
            for frame in 0..frames {
                for ch in 0..channels {
                    out.push(buf.chan(ch)[frame] as f32 / 128.0 - 1.0);
                }
            }
        }
        other => {
            let mut f32_buf = other.make_equivalent::<f32>();
            other.convert(&mut f32_buf);
            for frame in 0..f32_buf.frames() {
                for ch in 0..f32_buf.spec().channels.count() {
                    out.push(f32_buf.chan(ch)[frame]);
                }
            }
        }
    }
    out
}

fn push_stream_frame(
    source: &StreamingAudioSource,
    ring: &PcmRing,
    values: &[f32],
    generation: u64,
) -> bool {
    loop {
        if source.cancel.load(Ordering::Acquire)
            || source.seek_generation.load(Ordering::Acquire) != generation
        {
            return false;
        }
        if ring.push_frame(values, generation).is_ok() {
            source
                .buffered_samples
                .fetch_add(values.len(), Ordering::Relaxed);
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn run_stream_job(job: StreamJob) {
    let source = job.source;
    let ring = job.ring;
    if source.cancel.load(Ordering::Acquire) {
        return;
    }
    let generation = source.seek_generation.load(Ordering::Acquire);
    let mut session = source
        .decoder_session
        .lock()
        .ok()
        .and_then(|mut value| value.take());
    if session
        .as_ref()
        .is_some_and(|value| value.generation != generation)
    {
        session = None;
    }
    let mut session = match session {
        Some(value) => value,
        None => match stream_decoder(&source.path, true)
            .or_else(|_| stream_decoder(&source.path, false))
        {
            Ok(Some(decoder)) => DecoderSession {
                decoder,
                frame: 0,
                generation,
            },
            Ok(None) => {
                source.eof.store(true, Ordering::Release);
                source.refill_requested.store(false, Ordering::Release);
                return;
            }
            Err(_) => {
                source.decode_failures.fetch_add(1, Ordering::Relaxed);
                source.ready.store(true, Ordering::Release);
                source.eof.store(true, Ordering::Release);
                source.refill_requested.store(false, Ordering::Release);
                return;
            }
        },
    };
    source.eof.store(false, Ordering::Release);
    source
        .total_frames
        .store(session.decoder.total_frames.unwrap_or(0), Ordering::Release);
    let target = source.target_threshold();
    while source.buffered_samples() < target {
        if source.cancel.load(Ordering::Acquire)
            || source.seek_generation.load(Ordering::Acquire) != generation
        {
            return;
        }
        match session.decoder.format.next_packet() {
            Ok(packet) if packet.track_id() == session.decoder.track_id => {
                let decoded = match session.decoder.decoder.decode(&packet) {
                    Ok(value) => value,
                    Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                    Err(_) => {
                        source.decode_failures.fetch_add(1, Ordering::Relaxed);
                        source.eof.store(true, Ordering::Release);
                        break;
                    }
                };
                let channels = decoded.spec().channels.count().max(1);
                if channels != source.channels.max(1) as usize {
                    source.decode_failures.fetch_add(1, Ordering::Relaxed);
                    source.ready.store(true, Ordering::Release);
                    source.eof.store(true, Ordering::Release);
                    break;
                }
                let pcm = interleaved(decoded);
                for sample_frame in pcm.chunks(channels) {
                    if source.cancel.load(Ordering::Acquire)
                        || source.seek_generation.load(Ordering::Acquire) != generation
                    {
                        return;
                    }
                    if session.frame < source.requested_frame.load(Ordering::Acquire) {
                        session.frame += 1;
                        source
                            .decoded_frames
                            .store(session.frame, Ordering::Release);
                        continue;
                    }
                    if !push_stream_frame(&source, &ring, sample_frame, generation) {
                        return;
                    }
                    session.frame += 1;
                    source
                        .decoded_frames
                        .store(session.frame, Ordering::Release);
                    if source.buffered_samples() >= source.ready_threshold() {
                        source.ready.store(true, Ordering::Release);
                    }
                    if source.buffered_samples() >= target {
                        break;
                    }
                }
            }
            Ok(_) => {}
            Err(symphonia::core::errors::Error::IoError(_)) => {
                if source.buffered_samples() > 0 {
                    source.ready.store(true, Ordering::Release);
                }
                source.eof.store(true, Ordering::Release);
                break;
            }
            Err(symphonia::core::errors::Error::ResetRequired) => session.decoder.decoder.reset(),
            Err(_) => {
                source.decode_failures.fetch_add(1, Ordering::Relaxed);
                source.ready.store(true, Ordering::Release);
                source.eof.store(true, Ordering::Release);
                break;
            }
        }
    }
    source.refill_requested.store(
        source.buffered_samples() < source.low_watermark() && !source.is_eof(),
        Ordering::Release,
    );
    if let Ok(mut stored) = source.decoder_session.lock() {
        *stored = Some(session);
    };
}

/// Keep the format attached to the PCM buffer in sync with what the decoder
/// actually produced.  Some AAC tracks (notably phone HEVC/MOV files) can
/// advertise one rate in container codec parameters and expose another rate
/// from the decoder after the AAC configuration is read.  Returning the
/// container value in that case makes AudioEngine advance the source cursor at
/// the wrong speed.
fn observe_decoded_audio_format(
    current: &mut Option<(u16, u32)>,
    rate: u32,
    channels: usize,
) -> Result<()> {
    let actual = (channels as u16, rate);
    if actual.0 == 0 || actual.1 == 0 {
        return Err(anyhow!("Decoder returned an invalid audio format"));
    }
    if let Some(previous) = *current {
        if previous != actual {
            return Err(anyhow!(
                "Audio decoder changed format from {}ch/{}Hz to {}ch/{}Hz",
                previous.0,
                previous.1,
                actual.0,
                actual.1
            ));
        }
    } else {
        *current = Some(actual);
    }
    Ok(())
}

/// Decode the first audio track of `path` to interleaved f32 samples.
///
/// Returns:
/// - `Ok(Some((samples, channels, sample_rate)))` when an audio track is found
///   and decoded,
/// - `Ok(None)` when the container has **no** audio track (e.g. a silent video),
/// - `Err(..)` on an I/O or decode failure after all fallbacks are exhausted.
/// Compatibility name for callers that explicitly need a complete PCM copy
/// (waveform/preview/tests). Playback and workspace preload must use
/// [`probe_audio_track`] plus [`StreamingAudioSource`].
pub fn decode_audio_track(path: &Path) -> Result<Option<(Vec<f32>, u16, u32)>> {
    decode_audio_track_legacy(path)
}

/// Legacy whole-file decode retained for explicit compatibility paths only
/// (pre-existing waveform/preflight commands and unsupported streaming codecs).
/// Playback and workspace preload use [`StreamingAudioSource`] instead.
pub fn decode_audio_track_legacy(path: &Path) -> Result<Option<(Vec<f32>, u16, u32)>> {
    // Try symphonia first (two attempts: gapless on, then off).
    match decode_with_symphonia(path) {
        Ok(r) => return Ok(r),
        Err(e) => {
            log::warn!(
                "Symphonia could not decode '{}': {e}. Trying libmpv fallback.",
                path.display()
            );
        }
    }

    // Fallback: transcode via libmpv (ffmpeg) → temp WAV → re-read with symphonia.
    decode_via_mpv(path)
}

// ---------------------------------------------------------------------------
// Symphonia decoder (steps 1 & 2)
// ---------------------------------------------------------------------------

/// Probe and decode `path` using symphonia only — no libmpv fallback.
///
/// Separating this from [`decode_audio_track`] ensures that [`decode_via_mpv`]
/// can call it on the temp WAV without risking infinite recursion.
fn decode_with_symphonia(path: &Path) -> Result<Option<(Vec<f32>, u16, u32)>> {
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Cannot open media file: {}", path.display()))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        match symphonia::default::get_probe().format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        ) {
            Ok(p) => p,
            Err(_) => {
                // Some MP3s have malformed Xing/LAME gapless headers — retry without.
                let file2 = std::fs::File::open(path)
                    .with_context(|| format!("Cannot open media file: {}", path.display()))?;
                let mss2 = MediaSourceStream::new(Box::new(file2), Default::default());
                let mut hint2 = Hint::new();
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    hint2.with_extension(ext);
                }
                symphonia::default::get_probe()
                    .format(
                        &hint2,
                        mss2,
                        &FormatOptions { enable_gapless: false, ..Default::default() },
                        &MetadataOptions::default(),
                    )
                    .with_context(|| format!("Unsupported media format: {}", path.display()))?
            }
        }
    };

    let mut format = probed.format;

    // Pick the first track that actually reports a sample rate (audio tracks do;
    // video/subtitle tracks do not).
    let track = match format
        .tracks()
        .iter()
        .find(|t| t.codec_params.sample_rate.is_some())
    {
        Some(t) => t,
        None => return Ok(None),
    };

    let track_id = track.id;
    let codec_params = track.codec_params.clone();

    let channels = codec_params.channels.map(|c| c.count() as u16).unwrap_or(2);
    let sample_rate = codec_params.sample_rate.unwrap_or(44100);

    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .with_context(|| "Failed to create audio decoder")?;

    let estimated_samples = codec_params
        .n_frames
        .map(|n| n as usize * channels as usize)
        .unwrap_or(44_100 * 2 * 60);
    let mut samples: Vec<f32> = Vec::with_capacity(estimated_samples);
    // Use the decoded SignalSpec below, not only codec_params.  The latter is
    // the container's declaration and is not authoritative for AAC SBR/HE-AAC.
    let mut decoded_format: Option<(u16, u32)> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(_)) => break,
            Err(symphonia::core::errors::Error::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => return Err(anyhow!("Decode error: {e}")),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // Malformed frame — skip and continue rather than aborting.
            Err(symphonia::core::errors::Error::DecodeError(e)) => {
                log::warn!("Skipping malformed audio frame in {}: {e}", path.display());
                continue;
            }
            Err(e) => return Err(anyhow!("Decode error: {e}")),
        };

        let n_frames = decoded.frames();
        let n_ch = decoded.spec().channels.count();
        observe_decoded_audio_format(&mut decoded_format, decoded.spec().rate, n_ch)?;
        samples.reserve(n_frames * n_ch);

        match decoded {
            AudioBufferRef::F32(buf) => {
                for frame in 0..n_frames {
                    for ch in 0..n_ch {
                        samples.push(buf.chan(ch)[frame]);
                    }
                }
            }
            AudioBufferRef::S16(buf) => {
                for frame in 0..n_frames {
                    for ch in 0..n_ch {
                        samples.push(buf.chan(ch)[frame] as f32 / i16::MAX as f32);
                    }
                }
            }
            AudioBufferRef::S32(buf) => {
                for frame in 0..n_frames {
                    for ch in 0..n_ch {
                        samples.push(buf.chan(ch)[frame] as f32 / i32::MAX as f32);
                    }
                }
            }
            AudioBufferRef::U8(buf) => {
                for frame in 0..n_frames {
                    for ch in 0..n_ch {
                        samples.push(buf.chan(ch)[frame] as f32 / 128.0 - 1.0);
                    }
                }
            }
            other => {
                let mut f32_buf = other.make_equivalent::<f32>();
                other.convert(&mut f32_buf);
                let n_frames = f32_buf.frames();
                let n_ch = f32_buf.spec().channels.count();
                for frame in 0..n_frames {
                    for ch in 0..n_ch {
                        samples.push(f32_buf.chan(ch)[frame]);
                    }
                }
            }
        }
    }

    samples.shrink_to_fit();
    let (decoded_channels, decoded_rate) = decoded_format.unwrap_or((channels, sample_rate));
    Ok(Some((samples, decoded_channels, decoded_rate)))
}

// ---------------------------------------------------------------------------
// libmpv fallback decoder (step 3)
// ---------------------------------------------------------------------------

/// Decode `path` by having libmpv transcode it to a temp WAV file, then reading
/// that WAV with symphonia.
///
/// libmpv delegates to ffmpeg internally and can handle formats symphonia cannot
/// (MP2, unusual MPEG variants, some AAC-in-MP3 wrappers, etc.).
fn decode_via_mpv(path: &Path) -> Result<Option<(Vec<f32>, u16, u32)>> {
    use crate::engine::mpv_sys::{MpvLib, MPV_EVENT_END_FILE, MPV_EVENT_SHUTDOWN};
    use std::ffi::CString;

    let mpv = MpvLib::load().context("libmpv not available for audio fallback")?;

    let tmp_path = std::env::temp_dir()
        .join(format!("inkue_audio_{}.wav", uuid::Uuid::new_v4().simple()));

    let decode_result: Result<()> = (|| {
        unsafe {
            #[cfg(not(target_os = "windows"))]
            libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());

            let ctx = (mpv.mpv_create)();
            if ctx.is_null() {
                return Err(anyhow!("mpv_create returned null"));
            }

            // Helper: set a string option before initialize.
            let set = |key: &str, val: &str| {
                if let (Ok(k), Ok(v)) = (CString::new(key), CString::new(val)) {
                    (mpv.mpv_set_option_string)(ctx, k.as_ptr(), v.as_ptr());
                }
            };

            set("video", "no");
            set("vo", "null");
            set("ao", "pcm");
            set("audio-channels", "stereo");

            // Forward slashes required — libmpv on Windows does not always accept backslashes.
            let tmp_str = tmp_path
                .to_str()
                .ok_or_else(|| anyhow!("temp path is not valid UTF-8"))?
                .replace('\\', "/");
            set("ao-pcm-file", &tmp_str);

            if (mpv.mpv_initialize)(ctx) < 0 {
                (mpv.mpv_terminate_destroy)(ctx);
                return Err(anyhow!("mpv_initialize failed"));
            }

            let path_str = path
                .to_str()
                .ok_or_else(|| anyhow!("file path is not valid UTF-8"))?;
            let cmd = CString::new("loadfile").unwrap();
            let arg = CString::new(path_str)?;
            let null: *const std::ffi::c_char = std::ptr::null();
            let argv = [cmd.as_ptr(), arg.as_ptr(), null];
            (mpv.mpv_command)(ctx, argv.as_ptr());

            // Block until playback ends (up to 5 minutes).
            loop {
                let ev = (mpv.mpv_wait_event)(ctx, 300.0);
                if ev.is_null() {
                    break;
                }
                match (*ev).event_id {
                    MPV_EVENT_END_FILE | MPV_EVENT_SHUTDOWN => break,
                    _ => {}
                }
            }

            (mpv.mpv_terminate_destroy)(ctx);
        }
        Ok(())
    })();

    if let Err(e) = decode_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    // Read the WAV that mpv wrote, using the pure-symphonia path (no recursion).
    let wav_result = decode_with_symphonia(&tmp_path);
    let _ = std::fs::remove_file(&tmp_path);

    wav_result.with_context(|| {
        format!(
            "libmpv transcoded '{}' but the resulting WAV could not be read",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::observe_decoded_audio_format;

    #[test]
    fn decoded_format_prefers_actual_decoder_spec() {
        // Container metadata may say 44.1 kHz while the AAC decoder exposes
        // 48 kHz after parsing its configuration.  The observed format is the
        // one that must travel with the PCM buffer.
        let mut format = None;
        observe_decoded_audio_format(&mut format, 48_000, 2).unwrap();
        assert_eq!(format, Some((2, 48_000)));
    }

    #[test]
    fn decoded_format_rejects_midstream_format_change() {
        let mut format = None;
        observe_decoded_audio_format(&mut format, 48_000, 2).unwrap();
        let error = observe_decoded_audio_format(&mut format, 44_100, 2)
            .expect_err("a single PCM buffer cannot carry two sample rates");
        assert!(error.to_string().contains("changed format"));
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;

    fn test_source(
        state: StreamPlaybackState,
        ready: bool,
        buffered_samples: usize,
    ) -> Arc<StreamingAudioSource> {
        Arc::new(StreamingAudioSource {
            source_id: Uuid::new_v4(),
            path: PathBuf::from("test.wav"),
            channels: 2,
            sample_rate: 48_000,
            ring: PcmRing::new(48_000 * 2 * STREAM_MAX_SECONDS, 2),
            buffered_samples: AtomicUsize::new(buffered_samples),
            cancel: AtomicBool::new(false),
            seek_generation: AtomicU64::new(1),
            applied_generation: AtomicU64::new(1),
            requested_frame: AtomicU64::new(0),
            decoded_frames: AtomicU64::new(0),
            total_frames: AtomicU64::new(0),
            eof: AtomicBool::new(false),
            ready: AtomicBool::new(ready),
            underruns: AtomicU64::new(0),
            decode_failures: AtomicU64::new(0),
            reusable: AtomicBool::new(false),
            underrun_reported: AtomicU64::new(0),
            refill_requested: AtomicBool::new(true),
            playback_state: AtomicU8::new(state as u8),
            decoder_session: Mutex::new(None),
            job_requested: AtomicBool::new(false),
            job_running: AtomicBool::new(false),
            pool_registered: AtomicBool::new(false),
            playing_accounted: AtomicBool::new(false),
            silent_frames: AtomicU64::new(0),
            initial_underruns: AtomicU64::new(0),
            regular_underruns: AtomicU64::new(0),
            min_buffered_frames: AtomicUsize::new(buffered_samples / 2),
            max_buffered_frames: AtomicUsize::new(buffered_samples / 2),
            requested_at_ms: AtomicU64::new(0),
            last_refill_wait_us: AtomicU64::new(0),
            max_refill_wait_us: AtomicU64::new(0),
            last_decode_us: AtomicU64::new(0),
            max_decode_us: AtomicU64::new(0),
        })
    }

    #[test]
    fn pcm_ring_is_bounded_and_generation_reset_is_constant_time() {
        let ring = PcmRing::new(4, 2);
        assert!(ring.push_frame(&[1.0, 10.0], 1).is_ok());
        assert!(ring.push_frame(&[2.0, 20.0], 1).is_ok());
        assert!(ring.push_frame(&[3.0, 30.0], 1).is_err());
        ring.reset(2);
        let mut frame = [0.0; 2];
        assert!(
            !ring.pop_frame(&mut frame),
            "reset drops the old session without draining PCM"
        );
        assert!(ring.push_frame(&[4.0, 40.0], 2).is_ok());
        assert!(
            ring.push_frame(&[4.0, 40.0], 1).is_err(),
            "stale producer generation cannot write"
        );
        assert!(ring.pop_frame(&mut frame));
        assert_eq!(frame, [4.0, 40.0]);
    }

    #[test]
    fn stream_limits_are_explicit_and_safe_for_memory_budget() {
        assert_eq!(STREAM_TARGET_SECONDS, 4);
        assert_eq!(STREAM_READY_MILLIS, 750);
        assert!(STREAM_MAX_SECONDS >= STREAM_TARGET_SECONDS);
    }

    #[test]
    fn legacy_fallback_is_selected_only_for_probe_errors() {
        let unsupported: Result<Option<AudioStreamInfo>> = Err(anyhow!("unsupported codec"));
        assert!(should_use_legacy_fallback(&unsupported));
        assert!(!should_use_legacy_fallback(&Ok(None)));
        assert!(!should_use_legacy_fallback(&Ok(Some(AudioStreamInfo {
            channels: 2,
            sample_rate: 48_000,
            total_frames: Some(1),
        }))));
    }

    #[test]
    fn priority_keeps_playing_sources_ahead_of_paused_sources() {
        let paused = test_source(StreamPlaybackState::Paused, true, 48_000 * 2);
        let playing = test_source(StreamPlaybackState::Playing, true, 48_000 * 2);
        let critical = test_source(StreamPlaybackState::Playing, false, 0);
        let mut jobs = BinaryHeap::new();
        for (sequence, source) in [paused, playing, critical].into_iter().enumerate() {
            jobs.push(PendingStreamJob {
                priority: source.priority(),
                sequence: sequence as u64,
                source,
            });
        }
        assert!(
            !jobs.pop().unwrap().source.is_ready(),
            "not-ready playing source is critical"
        );
        assert!(
            jobs.pop().unwrap().source.priority() > 0,
            "normal playing source precedes paused source"
        );
    }

    #[test]
    fn ten_sources_have_one_logical_job_each_without_duplicates() {
        let sources: Vec<_> = (0..10)
            .map(|_| test_source(StreamPlaybackState::Preload, false, 0))
            .collect();
        let mut jobs = BinaryHeap::new();
        for (sequence, source) in sources.iter().cloned().enumerate() {
            jobs.push(PendingStreamJob {
                priority: source.priority(),
                sequence: sequence as u64,
                source,
            });
        }
        let mut seen = std::collections::HashSet::new();
        while let Some(job) = jobs.pop() {
            assert!(seen.insert(Arc::as_ptr(&job.source) as usize));
        }
        assert_eq!(seen.len(), 10);
    }

    #[test]
    fn initial_starvation_sets_refill_and_underrun_without_waiting() {
        let source = test_source(StreamPlaybackState::Playing, false, 0);
        let mut frame = [1.0; 2];
        assert!(!source.pop_frame(&mut frame));
        assert_eq!(source.underruns(), 1);
        assert!(source.needs_refill());
    }

    #[test]
    fn eof_empty_pop_is_terminal_not_an_underrun_or_refill_request() {
        let source = test_source(StreamPlaybackState::Playing, true, 0);
        source.eof.store(true, Ordering::Release);
        source.refill_requested.store(false, Ordering::Release);
        let mut frame = [1.0; 2];

        assert!(!source.pop_frame(&mut frame));
        assert_eq!(source.underruns(), 0);
        assert!(!source.refill_requested.load(Ordering::Acquire));
        assert!(!source.needs_refill());
    }

    #[test]
    fn diagnostics_reset_does_not_clear_playback_state_or_pcm() {
        let source = StreamingAudioSource::test_from_pcm(&[[0.25, 0.5]], true, false);
        source.note_underrun_frames(128);
        source.decode_failures.fetch_add(2, Ordering::Relaxed);
        source.reset_diagnostics();
        let diagnostics = source.diagnostics();
        assert_eq!(diagnostics.buffered_frames, 1);
        assert!(diagnostics.ready);
        assert_eq!(diagnostics.underruns, 0);
        assert_eq!(diagnostics.silent_frames, 0);
        assert_eq!(diagnostics.decode_failures, 0);
    }

    #[test]
    fn diagnostics_accounting_never_underflows() {
        let counter = AtomicUsize::new(0);
        assert_eq!(saturating_sub(&counter, 1), 0);
        assert_eq!(counter.load(Ordering::Acquire), 0);
        counter.store(3, Ordering::Release);
        assert_eq!(saturating_sub(&counter, 2), 1);
        assert_eq!(counter.load(Ordering::Acquire), 1);
    }

    #[test]
    fn paused_ready_source_does_not_requeue_below_low_watermark() {
        let source = test_source(StreamPlaybackState::Paused, true, 1);
        source.refill_requested.store(false, Ordering::Release);
        assert!(!source.needs_refill());
        let playing = test_source(StreamPlaybackState::Playing, false, 0);
        assert!(playing.priority() > source.priority());
    }

    #[test]
    fn rt_seek_only_changes_atomics_and_does_not_enqueue() {
        let source = test_source(StreamPlaybackState::Playing, true, 48_000 * 2);
        source.request_seek_rt(123);
        assert_eq!(source.requested_frame(), 123);
        assert!(!source.job_requested.load(Ordering::Acquire));
        assert!(source.seek_pending() == false);
    }

    #[test]
    fn bounded_dispatch_retry_keeps_all_pending_jobs() {
        let sources: Vec<_> = (0..33)
            .map(|_| test_source(StreamPlaybackState::Preload, false, 0))
            .collect();
        let mut pending = BinaryHeap::new();
        for (sequence, source) in sources.iter().cloned().enumerate() {
            pending.push(PendingStreamJob { priority: source.priority(), sequence: sequence as u64, source });
        }
        assert_eq!(pending.len(), 33, "scheduler retains a job when the worker queue is full");
    }

    #[test]
    fn full_worker_queue_keeps_pending_job_and_dedup_flag() {
        let source = test_source(StreamPlaybackState::Playing, false, 0);
        source.job_requested.store(true, Ordering::Release);
        let mut pending = BinaryHeap::new();
        pending.push(PendingStreamJob { priority: source.priority(), sequence: 0, source: Arc::clone(&source) });
        let (tx, _rx) = mpsc::sync_channel(0);
        assert!(!dispatch_pending_job(&tx, &mut pending));
        assert_eq!(pending.len(), 1);
        assert!(source.job_requested.load(Ordering::Acquire));
    }
}
