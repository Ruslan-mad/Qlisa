//! [`AudioEngine`] — the top-level audio subsystem.
//!
//! **Real-time safety:** the audio callback (`fill_buffer`) must never
//! allocate, block, or do I/O.  All state mutations happen via the command ring
//! buffer; all outgoing data goes through the status ring buffer.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use arc_swap::ArcSwap;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::HeapRb;
use uuid::Uuid;

use crate::preferences::MachineAudioConfig;

use super::{
    audio_input::{open_input, InputCapture},
    device_manager::DeviceManager,
    ring_command::{AudioCommand, AudioStatus, FadeCurve, VoiceId},
    voice::{FadeDirection, FadeState, LiveSource, Voice, VoiceState},
};

const _MAX_VOICES: usize = 64;
const RING_CAPACITY: usize = 256;
pub const DEFAULT_FADE_OUT_MS: u32 = 500;

/// Circular staging buffer per input feed, in frames.  Large enough to absorb
/// two full input-callback periods at maximum buffer size (2 × 2048 @ 48 kHz ≈
/// 85 ms) while staying small so the target-lag calculation stays well inside.
const STAGING_FRAMES: usize = 8192;

/// Synthetic network/LTC feeds are fed by non-real-time decoder threads. Keep
/// their pre-callback backlog short: retaining more than this turns a brief
/// callback stall into visibly late camera audio. The staging history remains
/// larger for interpolation, but an overloaded synthetic producer explicitly
/// flushes that history to the live edge before it can be replayed.
pub const SYNTHETIC_LIVE_RING_MAX_MS: u32 = 100;

/// Network decoders deliver PCM in transport-sized bursts rather than at the
/// device callback cadence. Keep enough decoded audio queued to bridge a
/// normal NDI/SRT packet interval before starting (or after an underrun).
const NETWORK_LIVE_JITTER_MS: u32 = 40;
/// Network PCM has already been converted to the engine rate. A very small
/// correction still absorbs independent-clock drift without the audible pitch
/// modulation of the microphone path's deliberately aggressive +/-2% servo.
const NETWORK_LIVE_MAX_RATE_CORRECTION: f64 = 0.001;

fn synthetic_live_ring_frames(sample_rate: u32) -> usize {
    let frames = (sample_rate.max(1) as usize)
        .saturating_mul(SYNTHETIC_LIVE_RING_MAX_MS as usize)
        .div_ceil(1000);
    frames.max(1).min(STAGING_FRAMES)
}

/// Producer half of a bounded synthetic live feed.
///
/// Unlike an ordinary SPSC producer, overflow requests a callback-side flush
/// of the old queue. The producer never touches the consumer (which would
/// break SPSC ownership); the callback observes the atomic request, discards
/// stale queued samples, and advances the live source to its current edge.
/// All producer operations are non-blocking and allocation-free.
pub struct SyntheticFeedProducer {
    producer: ringbuf::HeapProd<f32>,
    flush_requested: Option<Arc<AtomicBool>>,
    dropped_samples: Arc<AtomicU64>,
}

impl SyntheticFeedProducer {
    fn attached(
        producer: ringbuf::HeapProd<f32>,
        flush_requested: Arc<AtomicBool>,
        dropped_samples: Arc<AtomicU64>,
    ) -> Self {
        Self {
            producer,
            flush_requested: Some(flush_requested),
            dropped_samples,
        }
    }

    fn record_overflow(&self, count: usize) {
        if let Some(flush_requested) = &self.flush_requested {
            flush_requested.store(true, Ordering::Release);
        }
        self.dropped_samples
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    /// Push one sample. For interleaved network audio prefer
    /// [`Self::try_push_frame`] so a stereo pair is admitted or dropped whole.
    pub fn try_push(&mut self, sample: f32) -> Result<(), f32> {
        match self.producer.try_push(sample) {
            Ok(()) => Ok(()),
            Err(sample) => {
                self.record_overflow(1);
                Err(sample)
            }
        }
    }

    /// Push one complete interleaved frame. A full queue drops the whole
    /// frame and schedules a live-edge flush, so channels cannot be skewed.
    pub fn try_push_frame(&mut self, samples: &[f32]) -> Result<(), usize> {
        if samples.is_empty() {
            return Ok(());
        }
        if self.producer.vacant_len() < samples.len() {
            self.record_overflow(samples.len());
            return Err(samples.len());
        }
        // `push_slice` publishes all written samples with one write-index
        // advance. In particular, a stereo L/R pair never becomes visible to
        // the callback as a one-sample frame between two `try_push` calls.
        // The prior vacancy check reserves the entire frame; a consumer can
        // only create more vacancy while this is in progress.
        if self.producer.push_slice(samples) != samples.len() {
            self.record_overflow(samples.len());
            return Err(samples.len());
        }
        Ok(())
    }

    pub fn capacity(&self) -> std::num::NonZeroUsize {
        self.producer.capacity()
    }

    pub fn occupied_len(&self) -> usize {
        self.producer.occupied_len()
    }

    pub fn vacant_len(&self) -> usize {
        self.producer.vacant_len()
    }

    pub fn dropped_samples(&self) -> u64 {
        self.dropped_samples.load(Ordering::Relaxed)
    }
}

/// A detached producer is useful for lightweight API test doubles. It keeps
/// ordinary bounded-ring semantics because no callback owns a flush flag.
impl From<ringbuf::HeapProd<f32>> for SyntheticFeedProducer {
    fn from(producer: ringbuf::HeapProd<f32>) -> Self {
        Self {
            producer,
            flush_requested: None,
            dropped_samples: Arc::new(AtomicU64::new(0)),
        }
    }
}

// ---------------------------------------------------------------------------
// Live input feed — one per captured device, drained by the output callback
// ---------------------------------------------------------------------------

/// One live input device's capture: its ring consumer plus a circular staging
/// buffer the output callback keeps current and live voices resample from.
///
/// Drained every output block (`drain`) so the input stays "warm" and bounded
/// even when no Mic Cue is playing — a GO is then instant with no cold-start.
struct InputFeed {
    /// Stable id referenced by a [`LiveSource`].
    id: Uuid,
    /// OS device id this feed captures from (one feed per device).
    device_id: String,
    /// Interleaved channel count of the staging frames.
    in_channels: usize,
    /// Input device sample rate (Hz).
    sample_rate: u32,
    /// Ring consumer fed by the cpal input callback.
    cons: ringbuf::HeapCons<f32>,
    /// Circular interleaved staging, `STAGING_FRAMES * in_channels` long.
    staging: Box<[f32]>,
    /// Monotonic count of frames written into `staging`.
    write_frame: u64,
    /// First absolute frame in the current valid staging generation. Overflow
    /// advances this boundary so modulo addressing cannot replay old samples.
    valid_from_frame: u64,
    /// Fixed prebuffer/target lag for burst-fed network audio, in source frames.
    /// Device and ordinary synthetic feeds retain the callback-period policy.
    network_jitter_frames: Option<usize>,
    /// Set by a synthetic producer after overflow. It is consumed only by the
    /// output callback, preserving SPSC ownership of `cons`.
    flush_requested: Option<Arc<AtomicBool>>,
    /// Keeps the cpal input stream alive (`None` only in unit tests).
    _capture: Option<InputCapture>,
}

impl InputFeed {
    /// Discard queued old input and make live readers resynchronise when fresh
    /// frames arrive. Skipping a full staging generation prevents an existing
    /// live cursor from replaying pre-overflow samples still held in `staging`.
    fn flush_to_live_edge(&mut self) {
        while self.cons.occupied_len() >= self.in_channels {
            for _ in 0..self.in_channels {
                let _ = self.cons.try_pop();
            }
        }
        // A synthetic stereo producer commits frames atomically, but discard
        // any inherited partial frame as well so a flush can never make the
        // next L/R pair start with stale audio from before the overflow.
        while self.cons.try_pop().is_some() {}
        self.write_frame = self.write_frame.saturating_add(STAGING_FRAMES as u64);
        self.valid_from_frame = self.write_frame;
    }

    /// Pop every available frame from the ring into the circular staging buffer.
    /// RT-safe: bounded by what the input callback produced, no allocation.
    fn drain(&mut self) {
        if self
            .flush_requested
            .as_ref()
            .is_some_and(|flush| flush.swap(false, Ordering::AcqRel))
        {
            self.flush_to_live_edge();
        }
        let ch = self.in_channels;
        while self.cons.occupied_len() >= ch {
            let slot = (self.write_frame as usize % STAGING_FRAMES) * ch;
            for c in 0..ch {
                if let Some(s) = self.cons.try_pop() {
                    self.staging[slot + c] = s;
                }
            }
            self.write_frame += 1;
        }
    }

    /// Linear-interpolated sample for input channel `ch` at fractional frame `pos`.
    fn sample(&self, ch: usize, pos: f64) -> f32 {
        let i0 = pos.floor() as u64;
        let frac = (pos - i0 as f64) as f32;
        let a = self.staging[(i0 as usize % STAGING_FRAMES) * self.in_channels + ch];
        let b = self.staging[((i0 + 1) as usize % STAGING_FRAMES) * self.in_channels + ch];
        a + (b - a) * frac
    }
}

/// Number of retired voice-list generations kept alive on the non-RT side so
/// the real-time callback never holds the last reference to one — and therefore
/// never runs a `Vec`/`Voice`/samples destructor.  A single output callback
/// (a few ms) can never outlive this many publishes (each driven by a GO / GC),
/// so 3 is a comfortable margin.
const VOICE_POOL_RETAIN: usize = 3;

/// The outgoing NDI/SRT bus is a copy of Qlisa's post-master stereo program
/// mix.  Each destination retains at most half a second at 48 kHz.  Unlike a
/// normal SPSC producer, the real-time callback *overwrites the oldest frame*
/// when full: retaining half a second of stale program audio is much worse for
/// a live output than losing the oldest few samples.
const NETWORK_AUDIO_TAP_FRAMES: usize = 48_000 / 2;
/// Bound callback fan-out as well as the memory held by program-audio taps.
/// More than this should be split across render nodes instead of making one
/// device callback service an unbounded number of encoders.
pub const MAX_NETWORK_AUDIO_DESTINATIONS: usize = 8;

/// Atomically published output format.  A restart is represented by an odd
/// generation while the rate is being changed, then the next even generation.
/// Consumers therefore never pair a new generation with an old rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramAudioFormatSnapshot {
    pub sample_rate: u32,
    pub generation: u64,
}

pub struct ProgramAudioFormat {
    sample_rate: AtomicU32,
    generation: AtomicU64,
}

impl ProgramAudioFormat {
    fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: AtomicU32::new(sample_rate),
            // Even values are stable; start at a non-zero generation so a
            // default/zero snapshot cannot accidentally look current.
            generation: AtomicU64::new(2),
        }
    }

    fn update_for_restart(&self, sample_rate: u32) {
        // Make every existing sender fail its generation check before the new
        // cpal stream is allowed to emit samples.  The second increment marks
        // the (rate, generation) pair stable again.
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.sample_rate.store(sample_rate, Ordering::Release);
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn snapshot(&self) -> ProgramAudioFormatSnapshot {
        loop {
            let before = self.generation.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let sample_rate = self.sample_rate.load(Ordering::Acquire);
            let after = self.generation.load(Ordering::Acquire);
            if before == after {
                return ProgramAudioFormatSnapshot {
                    sample_rate,
                    generation: after,
                };
            }
        }
    }
}

struct ProgramAudioTap {
    /// Preallocated stereo frames, packed as `(left_bits << 32) | right_bits`.
    /// Atomic cells make the producer's oldest-frame eviction safe when the
    /// consumer races a full queue; no mutex, allocation, or I/O is possible
    /// from the device callback.
    frames: Box<[AtomicU64]>,
    read_frame: AtomicU64,
    write_frame: AtomicU64,
    dropped_samples: std::sync::atomic::AtomicU64,
    format: Arc<ProgramAudioFormat>,
}

impl ProgramAudioTap {
    fn new(format: Arc<ProgramAudioFormat>) -> (Arc<Self>, ProgramAudioReceiver) {
        let tap = Arc::new(Self {
            frames: (0..NETWORK_AUDIO_TAP_FRAMES)
                .map(|_| AtomicU64::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            read_frame: AtomicU64::new(0),
            write_frame: AtomicU64::new(0),
            dropped_samples: std::sync::atomic::AtomicU64::new(0),
            format,
        });
        (
            Arc::clone(&tap),
            ProgramAudioReceiver {
                tap: Arc::clone(&tap),
            },
        )
    }

    /// Called exclusively by the device callback after master gain.
    fn push_stereo(&self, samples: &[f32], channels: usize) {
        if channels == 0 {
            return;
        }
        for frame in samples.chunks_exact(channels) {
            let packed = (u64::from(frame[0].to_bits()) << 32)
                | u64::from(frame.get(1).copied().unwrap_or(frame[0]).to_bits());
            loop {
                let write = self.write_frame.load(Ordering::Relaxed);
                let read = self.read_frame.load(Ordering::Acquire);
                if write.wrapping_sub(read) >= self.frames.len() as u64 {
                    // The consumer may be reading this frame.  It verifies its
                    // read-index CAS after loading, so a winning eviction makes
                    // it retry instead of returning a torn/stale frame.
                    if self
                        .read_frame
                        .compare_exchange_weak(
                            read,
                            read.wrapping_add(1),
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        self.dropped_samples.fetch_add(2, Ordering::Relaxed);
                    }
                    continue;
                }
                let index = write as usize % self.frames.len();
                self.frames[index].store(packed, Ordering::Relaxed);
                self.write_frame
                    .store(write.wrapping_add(1), Ordering::Release);
                break;
            }
        }
    }

    fn pop_stereo(&self) -> Option<(f32, f32)> {
        loop {
            let read = self.read_frame.load(Ordering::Acquire);
            let write = self.write_frame.load(Ordering::Acquire);
            if read == write {
                return None;
            }
            let packed = self.frames[read as usize % self.frames.len()].load(Ordering::Acquire);
            if self
                .read_frame
                .compare_exchange_weak(
                    read,
                    read.wrapping_add(1),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Some(((packed >> 32) as u32, packed as u32))
                    .map(|(left, right)| (f32::from_bits(left), f32::from_bits(right)));
            }
        }
    }
}

/// Consumer half of one post-master program-audio tap. It is owned by exactly
/// one network worker and never touched by the real-time callback.
pub struct ProgramAudioReceiver {
    tap: Arc<ProgramAudioTap>,
}

impl ProgramAudioReceiver {
    pub fn drain_into(&mut self, target: &mut Vec<f32>, max_samples: usize) {
        target.clear();
        // `target` is preallocated by the network worker.  Do not reserve
        // here: callers that violate that contract should fail visibly rather
        // than hide an allocation behind a supposedly bounded drain.
        let max_frames = max_samples / 2;
        for _ in 0..max_frames {
            let Some((left, right)) = self.tap.pop_stereo() else {
                break;
            };
            target.push(left);
            target.push(right);
        }
    }

    pub fn dropped_samples(&self) -> u64 {
        self.tap.dropped_samples.load(Ordering::Relaxed)
    }

    /// Move this network consumer to the producer's current live edge without
    /// touching the real-time callback. A transport calls this immediately
    /// after (re)starting so audio accumulated while FFmpeg/NDI was absent is
    /// never sent as a delayed burst.
    pub fn discard_queued(&mut self) -> u64 {
        loop {
            let read = self.tap.read_frame.load(Ordering::Acquire);
            let write = self.tap.write_frame.load(Ordering::Acquire);
            if read == write {
                return 0;
            }
            if self
                .tap
                .read_frame
                .compare_exchange_weak(read, write, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return write.wrapping_sub(read).saturating_mul(2);
            }
        }
    }

    pub fn format(&self) -> Arc<ProgramAudioFormat> {
        Arc::clone(&self.tap.format)
    }
}

/// Snapshot registry mirroring VoicePool's retention rule: old ArcSwap lists
/// are dropped by the settings thread, never by the real-time audio callback.
struct ProgramAudioTapRegistry {
    rt: Arc<ArcSwap<Vec<Arc<ProgramAudioTap>>>>,
    retained: Mutex<VecDeque<Arc<Vec<Arc<ProgramAudioTap>>>>>,
    format: Arc<ProgramAudioFormat>,
}

impl ProgramAudioTapRegistry {
    fn new(format: Arc<ProgramAudioFormat>) -> Self {
        Self {
            rt: Arc::new(ArcSwap::from_pointee(Vec::new())),
            retained: Mutex::new(VecDeque::new()),
            format,
        }
    }

    fn rt_handle(&self) -> Arc<ArcSwap<Vec<Arc<ProgramAudioTap>>>> {
        Arc::clone(&self.rt)
    }

    fn replace(&self, count: usize) -> Result<Vec<ProgramAudioReceiver>> {
        if count > MAX_NETWORK_AUDIO_DESTINATIONS {
            return Err(anyhow!(
                "at most {MAX_NETWORK_AUDIO_DESTINATIONS} network program-audio destinations are supported"
            ));
        }
        let mut receivers = Vec::with_capacity(count);
        let mut taps = Vec::with_capacity(count);
        for _ in 0..count {
            let (tap, receiver) = ProgramAudioTap::new(Arc::clone(&self.format));
            taps.push(tap);
            receivers.push(receiver);
        }
        let snapshot = Arc::new(taps);
        self.rt.store(Arc::clone(&snapshot));
        if let Ok(mut retained) = self.retained.lock() {
            retained.push_back(snapshot);
            while retained.len() > VOICE_POOL_RETAIN {
                retained.pop_front();
            }
        }
        Ok(receivers)
    }
}

/// A voice pool published **wait-free** to the real-time audio callback.
///
/// The RT callback reads a snapshot via [`ArcSwap::load`] — no lock, so it can
/// never be starved into emitting a block of silence (the old `try_lock`
/// failure mode).  Non-RT threads mutate `master` (a plain `Mutex`, only ever
/// contended among themselves) and then [`publish`](VoicePool::publish) a fresh
/// immutable snapshot.  Retired snapshots are held in `retained` for a few
/// generations so their destructors always run on a non-RT thread — the RT
/// thread allocates and frees nothing.
struct VoicePool {
    /// Editable master list — locked only by non-RT threads.
    master: Mutex<Vec<Arc<Voice>>>,
    /// Immutable snapshot the RT callback loads.
    rt: Arc<ArcSwap<Vec<Arc<Voice>>>>,
    /// Keep-alive ring of recent snapshots so the RT thread never deallocs one.
    retained: Mutex<VecDeque<Arc<Vec<Arc<Voice>>>>>,
}

impl VoicePool {
    fn new() -> Self {
        Self {
            master: Mutex::new(Vec::new()),
            rt: Arc::new(ArcSwap::from_pointee(Vec::new())),
            retained: Mutex::new(VecDeque::new()),
        }
    }

    /// The handle the RT callback reads from (`load()` each block).
    fn rt_handle(&self) -> Arc<ArcSwap<Vec<Arc<Voice>>>> {
        Arc::clone(&self.rt)
    }

    /// Publish the current master list as a new immutable snapshot, retaining a
    /// few prior generations so the RT thread never drops the last reference.
    fn publish(&self, master: &[Arc<Voice>]) {
        let snap = Arc::new(master.to_vec());
        self.rt.store(Arc::clone(&snap));
        if let Ok(mut ret) = self.retained.lock() {
            ret.push_back(snap);
            while ret.len() > VOICE_POOL_RETAIN {
                ret.pop_front(); // dropped here, on this non-RT thread
            }
        }
    }

    /// Append a voice and publish.  On a poisoned lock the voice is handed back
    /// (`Err`) so a caller can recover it — e.g. fall the cue back to the main
    /// output instead of losing it.
    fn push(&self, voice: Arc<Voice>) -> std::result::Result<(), Arc<Voice>> {
        match self.master.lock() {
            Ok(mut m) => {
                m.push(voice);
                self.publish(&m);
                Ok(())
            }
            Err(_) => Err(voice),
        }
    }

    /// Run `f` over the current voices (non-RT read; locks `master`).
    fn with<T>(&self, f: impl FnOnce(&[Arc<Voice>]) -> T) -> Option<T> {
        self.master.lock().ok().map(|m| f(&m))
    }

    /// Retain voices matching `keep`, then publish.  Removed `Arc<Voice>`s are
    /// dropped on this (non-RT) thread.
    fn retain(&self, keep: impl Fn(&Arc<Voice>) -> bool) {
        if let Ok(mut m) = self.master.lock() {
            m.retain(|v| keep(v));
            self.publish(&m);
        }
    }
}

/// One additional cpal output stream opened for an Output Patch that targets
/// a different device than the main stream.
///
/// Owns its own voice pool (mixed only by its own callback — per-stream pools
/// keep RT `try_lock` contention identical to the single-stream design) and
/// its own command/status rings.  Live (Mic) voices never land here: input
/// feeds are drained exclusively by the main stream's callback, so aux
/// streams get a permanently empty feed list.
struct AuxStream {
    /// OS device id this stream is open on.
    device_id: String,
    /// Output channel count of this stream (for channel-bounds alerts).
    channels: u32,
    voices: VoicePool,
    cmd_prod: ringbuf::HeapProd<AudioCommand>,
    status_cons: ringbuf::HeapCons<AudioStatus>,
    /// Set by the cpal error callback on DeviceNotAvailable.
    failed: Arc<std::sync::atomic::AtomicBool>,
    _stream: Stream,
}

/// The audio engine.
pub struct AudioEngine {
    pub device_manager: Mutex<DeviceManager>,
    cmd_prod: Mutex<ringbuf::HeapProd<AudioCommand>>,
    status_cons: Mutex<ringbuf::HeapCons<AudioStatus>>,
    voices: VoicePool,
    /// Additional per-device streams for Output Patches targeting a device
    /// other than the main one.  Opened lazily at the first GO that needs the
    /// device, kept open afterwards so later GOs start with zero device-open
    /// latency.  Never contains the main device.
    aux_streams: Mutex<Vec<AuxStream>>,
    /// Live input feeds (one per captured device), shared with the output
    /// callback which drains them each block.
    input_feeds: Arc<Mutex<Vec<InputFeed>>>,
    /// Non-RT mirror used by 30 Hz GC to avoid taking `input_feeds` while the
    /// live-feed set already exactly matches the referenced feed ids.
    input_feed_count: AtomicUsize,
    program_audio_taps: ProgramAudioTapRegistry,
    program_audio_format: Arc<ProgramAudioFormat>,
    _stream: Mutex<Option<Stream>>,
    sample_rate: std::sync::atomic::AtomicU32,
    /// Actual output callback period in frames (updated on the first callback).
    /// Used by `mix_live` to set a tight `target_lag` instead of a fixed 25 ms.
    output_period: Arc<std::sync::atomic::AtomicU32>,
    /// Monotonic count of output callbacks fired.  Shared across restarts; the
    /// device watchdog treats a count that stops advancing for ~2 s as a dead
    /// stream (kind-agnostic device-loss detection, even if cpal reports no error).
    output_callbacks: Arc<std::sync::atomic::AtomicU64>,
    /// Total output channel count of the current stream (updated on restart).
    output_channels: std::sync::atomic::AtomicU32,
    /// Output-channel offset applied to unpatched voices at submission — the
    /// selected ASIO output pair (0 on other backends).
    default_out_offset: std::sync::atomic::AtomicU32,
    master_gain: Arc<std::sync::atomic::AtomicU32>,
    /// Failure flag of the **current** stream (replaced on every restart).  Set by
    /// the cpal error callback after repeated `DeviceNotAvailable`; read by the
    /// device watchdog to detect a lost output device mid-show.
    stream_failed: Mutex<Arc<std::sync::atomic::AtomicBool>>,
    /// The config the operator actually selected — kept across an automatic
    /// fallback so the watchdog can detect the desired device's return.
    desired_config: Mutex<MachineAudioConfig>,
    /// Device id the current stream is open on (`None` = system default).
    current_device_id: Mutex<Option<String>>,
    /// `true` while running on the default device after the desired one was lost.
    in_fallback: std::sync::atomic::AtomicBool,
}

/// Lightweight non-RT view of stream-backed voices for the diagnostics page.
#[derive(Debug, Clone, Copy)]
pub struct StreamingVoiceDiagnostics {
    pub voice_id: Uuid,
    pub source_id: Uuid,
    pub state: VoiceState,
    pub current_frame: u64,
    pub preview_only: bool,
}

/// Snapshot of the output device's health, read by the device watchdog.
#[derive(Debug, Clone)]
pub struct AudioHealth {
    /// The current stream is erroring (device pulled / grabbed exclusively).
    pub failed: bool,
    /// Running on the default device after an automatic fallback.
    pub in_fallback: bool,
    /// The operator-selected device's display label — friendly name when known,
    /// else its id (`None` = system default).
    pub desired_device: Option<String>,
    /// Whether the desired device is currently present among enumerated devices.
    pub desired_present: bool,
}

/// The only permitted destination for a cue preview.  Kept separate from the
/// normal Output Patch decision because normal patches deliberately fall back
/// to the main PA on failure; preview must never do that.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PreviewRoute {
    Disabled,
    MainOutputNotPinned,
    MainOutputInFallback,
    SameAsMainDevice,
    AsioPair(u32),
    Aux(String),
}

fn preview_route(preview_device_id: Option<&str>, main_device_id: Option<&str>) -> PreviewRoute {
    let Some(device_id) = preview_device_id.filter(|id| !id.is_empty()) else {
        return PreviewRoute::Disabled;
    };
    // A system-default main stream has no stable PA identity.  Even a preview
    // device with a different current id could become the default after a
    // Windows device change, so headphones must remain silent until the PA is
    // explicitly pinned in machine settings.
    let Some(main_device_id) = main_device_id.filter(|id| !id.is_empty()) else {
        return PreviewRoute::MainOutputNotPinned;
    };
    if device_id == main_device_id {
        PreviewRoute::SameAsMainDevice
    } else {
        PreviewRoute::Aux(device_id.to_owned())
    }
}

fn validate_asio_preview_pair(pair: u32, main_pair: u32, output_channels: u32) -> Result<()> {
    let pairs = (output_channels / 2).max(1);
    if pair >= pairs {
        anyhow::bail!(
            "ASIO preview pair Out {}-{} is unavailable; the current stream has only {} stereo pairs",
            pair * 2 + 1,
            pair * 2 + 2,
            pairs
        );
    }
    if pair == main_pair {
        anyhow::bail!(
            "ASIO preview pair Out {}-{} conflicts with the main program pair",
            pair * 2 + 1,
            pair * 2 + 2
        );
    }
    Ok(())
}

/// Return whether a normal voice would write to either channel reserved for
/// the ASIO preview pair. Matrix entries are already expanded to device
/// channel indices before a voice reaches the engine.
fn voice_routes_to_preview_pair(voice: &Voice, pair: u32) -> bool {
    if voice.preview_only {
        return false;
    }
    let reserved = [pair as usize * 2, pair as usize * 2 + 1];
    // SAFETY: level_matrix is written before submission and only changed by
    // the audio command callback after submission.
    if let Some(matrix) = unsafe { &*voice.inner.level_matrix.get() } {
        return reserved.iter().any(|&channel| {
            channel < matrix.width
                && matrix.gains.iter().any(|row| row[channel] != 0.0)
        });
    }
    reserved.contains(&voice.out_l) || reserved.contains(&voice.out_r)
}

/// Output Patch routing owns this alert.  A successful operator preview is
/// intentionally unrelated, so it must never make a failing program patch
/// look healthy again.
fn clear_patch_device_health_if_requested(is_output_patch_route: bool) {
    if is_output_patch_route {
        crate::health::clear("output-patch-device");
    }
}

// SAFETY: cpal::Stream is not Send on Windows when using WASAPI.
unsafe impl Send for AudioEngine {}
unsafe impl Sync for AudioEngine {}

/// The stream-independent half of an [`AudioEngine`], built before any device
/// is opened and shared with every stream the engine goes on to open.
///
/// Exists so the normal and the silent constructor assemble exactly the same
/// engine — the only difference being whether a [`StreamResult`] is available.
struct EngineCore {
    voices: VoicePool,
    input_feeds: Arc<Mutex<Vec<InputFeed>>>,
    program_audio_taps: ProgramAudioTapRegistry,
    program_audio_format: Arc<ProgramAudioFormat>,
    master_gain: Arc<std::sync::atomic::AtomicU32>,
    output_period: Arc<std::sync::atomic::AtomicU32>,
    output_callbacks: Arc<std::sync::atomic::AtomicU64>,
}

impl EngineCore {
    fn new() -> Self {
        let program_audio_format = Arc::new(ProgramAudioFormat::new(48_000));
        Self {
            voices: VoicePool::new(),
            input_feeds: Arc::new(Mutex::new(Vec::new())),
            program_audio_taps: ProgramAudioTapRegistry::new(Arc::clone(&program_audio_format)),
            program_audio_format,
            master_gain: Arc::new(std::sync::atomic::AtomicU32::new(f32::to_bits(1.0_f32))),
            output_period: Arc::new(std::sync::atomic::AtomicU32::new(256)),
            output_callbacks: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    fn open(
        &self,
        config: &MachineAudioConfig,
        stream_failed: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<StreamResult> {
        open_stream_inner(
            config,
            self.voices.rt_handle(),
            Arc::clone(&self.input_feeds),
            self.program_audio_taps.rt_handle(),
            Some(&self.program_audio_format),
            Arc::clone(&self.master_gain),
            Arc::clone(&self.output_period),
            stream_failed,
            Arc::clone(&self.output_callbacks),
        )
    }

    /// Build the engine around an open stream, or around none (silent mode).
    fn assemble(
        self,
        config: &MachineAudioConfig,
        stream: Option<StreamResult>,
        stream_failed: Arc<std::sync::atomic::AtomicBool>,
        started_in_fallback: bool,
    ) -> Arc<AudioEngine> {
        // In silent mode the ring buffers have no consumer: `send_command`
        // fills the queue and then reports "ring buffer full", which callers
        // already treat as a non-fatal playback error.  A later `restart()`
        // replaces both halves with the new stream's.
        let (cmd_prod, status_cons, stream, sample_rate, channels, out_offset) = match stream {
            Some(sr) => (
                sr.cmd_prod,
                sr.status_cons,
                Some(sr.stream),
                sr.sample_rate,
                sr.channels,
                sr.default_out_offset,
            ),
            None => {
                let (cmd_prod, _) = HeapRb::<AudioCommand>::new(RING_CAPACITY).split();
                let (_, status_cons) = HeapRb::<AudioStatus>::new(RING_CAPACITY).split();
                (cmd_prod, status_cons, None, 48_000, 2, 0)
            }
        };

        let engine = Arc::new(AudioEngine {
            device_manager: Mutex::new(DeviceManager::new()),
            cmd_prod: Mutex::new(cmd_prod),
            status_cons: Mutex::new(status_cons),
            voices: self.voices,
            aux_streams: Mutex::new(Vec::new()),
            input_feeds: self.input_feeds,
            input_feed_count: AtomicUsize::new(0),
            program_audio_taps: self.program_audio_taps,
            program_audio_format: self.program_audio_format,
            _stream: Mutex::new(stream),
            sample_rate: std::sync::atomic::AtomicU32::new(sample_rate),
            output_channels: std::sync::atomic::AtomicU32::new(channels),
            default_out_offset: std::sync::atomic::AtomicU32::new(out_offset),
            master_gain: self.master_gain,
            output_period: self.output_period,
            output_callbacks: self.output_callbacks,
            stream_failed: Mutex::new(stream_failed),
            // `desired_config` keeps the operator's choice even when we started on
            // the fallback, so the watchdog can offer a restore when it returns.
            desired_config: Mutex::new(config.clone()),
            current_device_id: Mutex::new(if started_in_fallback {
                None
            } else {
                config.device_id.clone()
            }),
            in_fallback: std::sync::atomic::AtomicBool::new(started_in_fallback),
        });

        // A broken configured device (HDMI with no display, unplugged interface)
        // is handled continuously by the device watchdog (lib.rs), which falls
        // back to the default device and surfaces a banner — no one-shot startup
        // watchdog needed.

        // Warm the device cache off the main thread.  `DeviceManager::new()` no
        // longer enumerates (that blocked app startup on a slow/hung Windows
        // audio driver, cpal #867); a one-shot bounded refresh fills it here
        // without stalling `.setup()`.
        {
            let engine_bg = Arc::clone(&engine);
            let _ = std::thread::Builder::new()
                .name("inkue-device-warmup".to_string())
                .spawn(move || {
                    let devices = crate::engine::device_manager::run_bounded(
                        crate::engine::device_manager::ENUM_TIMEOUT,
                        crate::engine::device_manager::enumerate_output_devices,
                    )
                    .unwrap_or_default();
                    if let Ok(mut mgr) = engine_bg.device_manager.lock() {
                        mgr.replace_cache(devices);
                    }
                });
        }

        engine
    }
}

impl AudioEngine {
    /// Snapshot stream-backed voices without touching the realtime callback.
    pub fn streaming_voice_diagnostics(&self) -> Vec<StreamingVoiceDiagnostics> {
        let mut snapshots: Vec<StreamingVoiceDiagnostics> = self.voices
            .with(|voices| {
                voices
                    .iter()
                    .filter_map(|voice| {
                        let source = voice.stream.as_ref()?;
                        Some(StreamingVoiceDiagnostics {
                            voice_id: voice.id,
                            source_id: source.id(),
                            state: voice.voice_state(),
                            current_frame: voice.current_frame(),
                            preview_only: voice.preview_only,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Ok(aux_streams) = self.aux_streams.lock() {
            for aux in aux_streams.iter() {
                if let Some(aux_snapshots) = aux.voices.with(|voices| {
                    voices
                        .iter()
                        .filter_map(|voice| {
                            let source = voice.stream.as_ref()?;
                            Some(StreamingVoiceDiagnostics {
                                voice_id: voice.id,
                                source_id: source.id(),
                                state: voice.voice_state(),
                                current_frame: voice.current_frame(),
                                preview_only: voice.preview_only,
                            })
                        })
                        .collect::<Vec<_>>()
                }) {
                    snapshots.extend(aux_snapshots);
                }
            }
        }
        snapshots
    }

    /// Open an output device according to `config` and start the audio callback.
    pub fn new(config: &MachineAudioConfig) -> Result<Arc<Self>> {
        let core = EngineCore::new();
        let stream_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Resilient startup: if the configured device is absent (unplugged since
        // it was saved), fall back to the system default rather than crashing.
        // The device watchdog then raises the banner and offers a restore when it
        // returns.  `desired_config` keeps the operator's original choice.
        let (sr, started_in_fallback) = match core.open(config, Arc::clone(&stream_failed)) {
            Ok(sr) => (sr, false),
            Err(e) if config.device_id.is_some() => {
                log::warn!(
                    "Configured audio device unavailable at startup ({e}); \
                     falling back to the system default"
                );
                let fallback = MachineAudioConfig {
                    device_id: None,
                    ..config.clone()
                };
                (core.open(&fallback, Arc::clone(&stream_failed))?, true)
            }
            Err(e) => return Err(e),
        };

        Ok(core.assemble(config, Some(sr), stream_failed, started_in_fallback))
    }

    /// Construct a **silent** engine: no device opened, no output callback.
    ///
    /// The fallback when no audio device can be opened at all — no sound card,
    /// or no ALSA/PipeWire server running.  The workspace still loads and
    /// video, MIDI, OSC, timecode and lighting cues still run, so the operator
    /// gets a working app plus a health banner instead of a process that
    /// vanishes at startup.  The device watchdog keeps retrying and brings
    /// audio up as soon as a device appears.
    pub fn new_silent(config: &MachineAudioConfig) -> Arc<Self> {
        // Pre-failed so `audio_health()` reports the fault immediately instead
        // of waiting for the watchdog's callback-stall heuristic.
        let stream_failed = Arc::new(std::sync::atomic::AtomicBool::new(true));
        EngineCore::new().assemble(config, None, stream_failed, false)
    }

    // ── Device resilience ─────────────────────────────────────────────────────

    /// Monotonic output-callback count.  The watchdog flags a dead stream when
    /// this stops advancing — a device-loss signal that does not depend on cpal
    /// surfacing an error of a particular kind.
    pub fn callback_count(&self) -> u64 {
        self.output_callbacks
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Snapshot of the output device's current health for the watchdog.
    pub fn audio_health(&self) -> AudioHealth {
        let failed = self
            .stream_failed
            .lock()
            .map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(false);
        let in_fallback = self.in_fallback.load(std::sync::atomic::Ordering::Relaxed);
        let (desired_id, desired_label) = self
            .desired_config
            .lock()
            .ok()
            .map(|c| {
                // Presence is checked by id; the banner shows the friendly name
                // (falling back to the id for devices saved before names existed).
                (
                    c.device_id.clone(),
                    c.device_name.clone().or_else(|| c.device_id.clone()),
                )
            })
            .unwrap_or((None, None));
        // Only enumerate devices while in fallback — that is the sole moment we
        // need to detect the desired device's return.  In the healthy steady
        // state the watchdog stays free (just an atomic read of `failed`).
        let desired_present = match (&in_fallback, &desired_id) {
            (true, Some(id)) => {
                // Enumerate off the manager lock (bounded) so a slow WASAPI query
                // never stalls a concurrent main-thread reader of the cache.
                let devices = crate::engine::device_manager::run_bounded(
                    crate::engine::device_manager::ENUM_TIMEOUT,
                    crate::engine::device_manager::enumerate_output_devices,
                )
                .unwrap_or_default();
                let present = devices.iter().any(|d| &d.id == id);
                if let Ok(mut mgr) = self.device_manager.lock() {
                    mgr.replace_cache(devices);
                }
                present
            }
            _ => true,
        };
        AudioHealth {
            failed,
            in_fallback,
            desired_device: desired_label,
            desired_present,
        }
    }

    /// Open `config` as the operator's chosen device (an explicit settings change).
    /// Records it as the desired device and clears any fallback state.
    pub fn apply_user_config(&self, config: &MachineAudioConfig) -> Result<()> {
        if let Ok(mut d) = self.desired_config.lock() {
            *d = config.clone();
        }
        self.in_fallback
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.restart(config)
    }

    /// Automatically fall back to the system default after the desired device was
    /// lost, keeping the show audible.
    ///
    /// Returns the lost device id when the default device **actually took
    /// over**, `None` when there was nothing to fall back to either — the
    /// engine then stays silent and `in_fallback` stays false, so the watchdog
    /// keeps retrying instead of reporting a fallback that never happened.
    pub fn fall_back_to_default(&self) -> Option<String> {
        let desired = self.desired_config.lock().ok().map(|c| c.clone())?;
        let lost = desired.device_id.clone();
        let fallback = MachineAudioConfig {
            device_id: None,
            ..desired
        };
        match self.restart(&fallback) {
            Ok(()) => {
                self.in_fallback
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                lost
            }
            Err(e) => {
                log::error!("Audio fallback restart failed: {e}");
                None
            }
        }
    }

    /// Retry the configured device after a *total* failure — no device could be
    /// opened at startup, or the machine has none at all.
    ///
    /// Unlike [`Self::fall_back_to_default`] there is nothing to fall back to,
    /// so this is a plain retry: `true` means audio is live again and the
    /// watchdog clears the banner on its next tick.  A failed attempt is silent
    /// (it runs on a timer and must not spam the log).
    pub fn retry_output_stream(&self) -> bool {
        let Ok(desired) = self.desired_config.lock().map(|c| c.clone()) else {
            return false;
        };
        match self.restart(&desired) {
            Ok(()) => {
                log::info!("Audio output device opened on retry");
                true
            }
            Err(_) => false,
        }
    }

    /// Re-open the operator's chosen device after it returned (manual restore).
    pub fn restore_desired(&self) -> Result<()> {
        let desired = self
            .desired_config
            .lock()
            .map(|c| c.clone())
            .map_err(|_| anyhow!("desired_config poisoned"))?;
        self.in_fallback
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.restart(&desired)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Replace the network program-audio subscriptions atomically.  Callers
    /// receive one bounded stereo consumer per active NDI/SRT destination;
    /// disabled or removed destinations get no callback work at all.
    pub fn replace_network_audio_taps(&self, count: usize) -> Vec<ProgramAudioReceiver> {
        self.program_audio_taps
            .replace(count)
            .unwrap_or_else(|error| {
                log::error!("Could not replace network program-audio taps: {error}");
                Vec::new()
            })
    }

    /// Total output channel count of the currently open stream.
    pub fn output_channels(&self) -> u32 {
        self.output_channels
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Set the master output gain (real-time safe via atomic).
    pub fn set_master_gain(&self, gain: f32) {
        self.master_gain
            .store(f32::to_bits(gain), std::sync::atomic::Ordering::Relaxed);
    }

    /// Shift an unpatched voice's L/R outputs to the default output offset
    /// (the selected ASIO pair).  Patched voices route exactly where their
    /// Output Patch says.
    fn apply_default_offset(&self, voice: &mut Voice) {
        if voice.patched {
            return;
        }
        let offset = self
            .default_out_offset
            .load(std::sync::atomic::Ordering::Relaxed) as usize;
        voice.out_l += offset;
        voice.out_r += offset;
    }

    fn configured_preview_pair(&self) -> Option<u32> {
        self.desired_config.lock().ok().and_then(|config| {
            (matches!(config.backend, crate::preferences::AudioBackend::Asio))
                .then_some(config.preview_asio_pair)
                .flatten()
        })
    }

    fn reject_preview_channel_overlap(&self, voice: &Voice) -> Result<()> {
        let Some(pair) = self.configured_preview_pair() else {
            return Ok(());
        };
        if !voice_routes_to_preview_pair(voice, pair) {
            return Ok(());
        }
        let message = format!(
            "Audio voice rejected: output channels {}-{} are reserved for ASIO preview pair Out {}-{}",
            voice.out_l.min(voice.out_r) + 1,
            voice.out_l.max(voice.out_r) + 1,
            pair * 2 + 1,
            pair * 2 + 2
        );
        crate::health::set(crate::health::HealthAlert::new(
            "preview-output-reserved",
            crate::health::HealthLevel::Error,
            message.clone(),
        ));
        anyhow::bail!(message)
    }

    fn reject_reserved_matrix_channel(&self, voice_id: VoiceId, output: u8) -> Result<()> {
        let Some(pair) = self.configured_preview_pair() else {
            return Ok(());
        };
        let reserved = [pair as usize * 2, pair as usize * 2 + 1];
        if !reserved.contains(&(output as usize)) {
            return Ok(());
        }
        // Aux streams have their own device/channel namespace. Only guard a
        // normal voice in the main pool; preview voices are explicitly exempt.
        let is_main_program_voice = self
            .voices
            .with(|voices| {
                voices
                    .iter()
                    .any(|voice| voice.id == voice_id && !voice.preview_only)
            })
            .unwrap_or(false);
        if !is_main_program_voice {
            return Ok(());
        }
        let message = format!(
            "Audio matrix route rejected: output channel {} is reserved for ASIO preview pair Out {}-{}",
            output as usize + 1,
            pair * 2 + 1,
            pair * 2 + 2
        );
        crate::health::set(crate::health::HealthAlert::new(
            "preview-output-reserved",
            crate::health::HealthLevel::Error,
            message.clone(),
        ));
        anyhow::bail!(message)
    }

    /// Add a pre-decoded voice to the pool and issue a Play command.
    pub fn play_voice(&self, mut voice: Voice) -> Result<VoiceId> {
        self.apply_default_offset(&mut voice);
        self.reject_preview_channel_overlap(&voice)?;
        self.check_channel_bounds(&voice, self.output_channels());
        let id = voice.id;
        let arc = Arc::new(voice);
        arc.set_playing();

        self.voices
            .push(Arc::clone(&arc))
            .map_err(|_| anyhow!("voices mutex poisoned"))?;

        self.send_command(AudioCommand::Play { voice_id: id })?;
        Ok(id)
    }

    /// Add a pre-decoded voice to the pool in the **paused** state, returning
    /// its id without starting playback.
    ///
    /// Used to pair a video's audio track with its muted mpv video: the voice
    /// is submitted paused at GO, then resumed (via [`resume_voice`]) the moment
    /// the video's first frame is presented, so audio and video start together
    /// with no A/V offset.
    pub fn play_voice_paused(&self, mut voice: Voice) -> Result<VoiceId> {
        self.apply_default_offset(&mut voice);
        self.reject_preview_channel_overlap(&voice)?;
        self.check_channel_bounds(&voice, self.output_channels());
        let id = voice.id;
        let arc = Arc::new(voice);
        arc.set_paused();

        self.voices
            .push(Arc::clone(&arc))
            .map_err(|_| anyhow!("voices mutex poisoned"))?;

        // No Play command — the callback only mixes Playing/FadingOut voices, so
        // this stays silent until resume_voice() is called.
        Ok(id)
    }

    // ── Output Patch device routing ───────────────────────────────────────────

    /// `true` when `device_id` should play on the main stream: no device
    /// requested, or it is the device the main stream is already open on
    /// (including when "main" is the system default and the patch names that
    /// same device explicitly — no duplicate stream on one device).
    fn routes_to_main(&self, device_id: Option<&str>) -> bool {
        let Some(dev) = device_id.filter(|d| !d.is_empty()) else {
            return true;
        };
        let main = self.current_device_id.lock().ok().and_then(|c| c.clone());
        let default_dev = match main {
            Some(_) => None,
            None => self
                .device_manager
                .lock()
                .ok()
                .and_then(|m| m.default_device().map(|d| d.id.clone())),
        };
        resolves_to_main_device(dev, main.as_deref(), default_dev.as_deref())
    }

    /// Resolve the machine-configured preview target.  A preview output is
    /// intentionally *not* allowed to be the main device: doing so would make
    /// an audition audible on PA.  There is no system-default fallback here.
    fn preview_route_for_config(&self, config: &MachineAudioConfig) -> Result<PreviewRoute> {
        let configured_route = preview_route(
            config.preview_device_id.as_deref(),
            config.device_id.as_deref(),
        );
        if matches!(config.backend, crate::preferences::AudioBackend::Asio) {
            if let Some(pair) = config.preview_asio_pair {
                validate_asio_preview_pair(
                    pair,
                    config.asio_out_pair,
                    self.output_channels(),
                )?;
                if self.in_fallback.load(Ordering::Relaxed) {
                    anyhow::bail!("ASIO main output is unavailable; preview remains silent");
                }
                return Ok(PreviewRoute::AsioPair(pair));
            }
        }
        if matches!(
            configured_route,
            PreviewRoute::Disabled | PreviewRoute::MainOutputNotPinned
        ) {
            return Ok(configured_route);
        }
        let main_is_unavailable = self.in_fallback.load(Ordering::Relaxed)
            || self
                .current_device_id
                .lock()
                .map(|device_id| device_id.is_none())
                .unwrap_or(true);
        if main_is_unavailable {
            return Ok(PreviewRoute::MainOutputInFallback);
        }
        Ok(configured_route)
    }

    fn preview_route(&self) -> Result<PreviewRoute> {
        let config = self
            .desired_config
            .lock()
            .map_err(|_| anyhow!("preview configuration is unavailable"))?
            .clone();
        self.preview_route_for_config(&config)
    }

    /// Submit a non-program preview voice to the configured headphone output.
    ///
    /// The preview always uses an aux stream and therefore cannot enter the
    /// main mix or its program-audio taps.  A missing device, a failed aux
    /// open, an unconfigured output, or selecting the main PA all return an
    /// error; callers must present silence rather than falling back to PA.
    pub fn play_preview_voice(&self, voice: Voice) -> Result<VoiceId> {
        let mut voice = voice;
        voice.preview_only = true;
        self.apply_preview_gain(&mut voice);
        self.play_preview_voice_on_route(voice, self.preview_route()?)
    }

    /// Play a one-shot test voice using a draft preview route. This keeps the
    /// test useful before Apply while preserving the same isolation guarantees
    /// as cue preview voices.
    pub fn play_test_preview_voice(
        &self,
        voice: Voice,
        preview_device_id: Option<String>,
        preview_asio_pair: Option<u32>,
        backend: crate::preferences::AudioBackend,
    ) -> Result<VoiceId> {
        let mut config = self
            .desired_config
            .lock()
            .map_err(|_| anyhow!("preview configuration is unavailable"))?
            .clone();
        config.preview_device_id = preview_device_id;
        config.preview_asio_pair = preview_asio_pair;
        config.backend = backend;
        let mut voice = voice;
        voice.preview_only = true;
        self.apply_preview_gain_for_db(&mut voice, config.preview_gain_db);
        self.play_preview_voice_on_route(voice, self.preview_route_for_config(&config)?)
    }

    fn apply_preview_gain(&self, voice: &mut Voice) {
        let gain_db = self
            .desired_config
            .lock()
            .map(|config| config.preview_gain_db)
            .unwrap_or(0.0);
        self.apply_preview_gain_for_db(voice, gain_db);
    }

    fn apply_preview_gain_for_db(&self, voice: &mut Voice, gain_db: f32) {
        let multiplier = crate::cue::types::db_to_linear(gain_db as f64) as f32;
        voice.inner.set_patch_gain(multiplier);
    }

    /// Update the machine-local Preview fader and hot-apply it to every
    /// currently playing preview voice. This only changes an atomic gain
    /// field, so it is safe while the callback is running and never touches
    /// the program bus.
    pub fn set_preview_gain_db(&self, gain_db: f32) -> Result<()> {
        let gain_db = gain_db.clamp(-60.0, 12.0);
        if let Ok(mut config) = self.desired_config.lock() {
            config.preview_gain_db = gain_db;
        }
        let multiplier = crate::cue::types::db_to_linear(gain_db as f64) as f32;
        let _ = self.voices.with(|voices| {
            for voice in voices.iter().filter(|voice| voice.preview_only) {
                voice.inner.set_patch_gain(multiplier);
            }
        });
        if let Ok(mut streams) = self.aux_streams.lock() {
            for stream in streams.iter_mut() {
                let _ = stream.voices.with(|voices| {
                    for voice in voices.iter().filter(|voice| voice.preview_only) { voice.inner.set_patch_gain(multiplier); }
                });
            }
        }
        Ok(())
    }

    /// Play a one-shot test voice on the current main pair without entering
    /// the program mix or its network audio taps.
    pub fn play_main_test_voice(&self, mut voice: Voice) -> Result<VoiceId> {
        self.apply_default_offset(&mut voice);
        voice.patched = true;
        voice.preview_only = true;
        let id = voice.id;
        let arc = Arc::new(voice);
        arc.set_playing();
        self.voices
            .push(Arc::clone(&arc))
            .map_err(|_| anyhow!("test voice pool poisoned"))?;
        self.send_command(AudioCommand::Play { voice_id: id })?;
        Ok(id)
    }

    /// Play an Output Patch test tone on the patch's physical device and
    /// selected stereo pair. A patch that names the current main device uses
    /// the main stream; every other device gets its own aux stream.
    pub fn play_test_output_patch_voice(
        &self,
        mut voice: Voice,
        device_id: &str,
        channels: &[u16],
    ) -> Result<VoiceId> {
        if device_id.is_empty() {
            anyhow::bail!("Output Patch has no device assigned");
        }
        if channels.len() != 2 || channels[1] != channels[0].saturating_add(1) {
            anyhow::bail!("Output Patch must select one adjacent stereo pair");
        }
        voice.out_l = channels[0] as usize;
        voice.out_r = channels[1] as usize;
        voice.patched = true;
        voice.preview_only = true;
        let id = voice.id;
        if self.routes_to_main(Some(device_id)) {
            let arc = Arc::new(voice);
            arc.set_playing();
            self.voices
                .push(Arc::clone(&arc))
                .map_err(|_| anyhow!("test voice pool poisoned"))?;
            self.send_command(AudioCommand::Play { voice_id: id })?;
            Ok(id)
        } else {
            self.submit_to_aux(voice, device_id, true, true, false)
                .map_err(|(_, error)| anyhow!("Output Patch test could not open '{device_id}': {error}"))
        }
    }

    fn play_preview_voice_on_route(&self, voice: Voice, route: PreviewRoute) -> Result<VoiceId> {
        match route {
            PreviewRoute::Disabled => anyhow::bail!(
                "Preview/headphones output is not configured. Choose one in Settings > Audio."
            ),
            PreviewRoute::MainOutputNotPinned => anyhow::bail!(
                "Main output must be an explicit device before headphone preview can be used. Choose the PA device in Settings > Audio; preview remains silent."
            ),
            PreviewRoute::MainOutputInFallback => anyhow::bail!(
                "Main output is unavailable or running on the system-default fallback. Restore the explicitly selected PA device before headphone preview can be used; preview remains silent."
            ),
            PreviewRoute::SameAsMainDevice => anyhow::bail!(
                "Preview/headphones output matches the main output. Choose a separate device to protect PA."
            ),
            PreviewRoute::AsioPair(pair) => {
                let mut voice = voice;
                let offset = pair as usize * 2;
                voice.out_l = offset;
                voice.out_r = offset + 1;
                voice.patched = true;
                voice.preview_only = true;
                let id = voice.id;
                let arc = Arc::new(voice);
                arc.set_playing();
                self.voices
                    .push(Arc::clone(&arc))
                    .map_err(|_| anyhow!("preview voice pool poisoned"))?;
                self.send_command(AudioCommand::Play { voice_id: id })?;
                Ok(id)
            }
            PreviewRoute::Aux(device_id) => self
                .submit_to_aux(voice, &device_id, true, false, false)
                .map_err(|(_, error)| anyhow!(
                    "Preview/headphones output '{device_id}' could not be opened; preview was kept silent: {error}"
                )),
        }
    }

    /// Immediately remove a preview voice from every pool.  This is stronger
    /// than a queued zero-length fade: it also cleans up reliably while the
    /// engine is in silent/no-device mode and prevents a stale preview from
    /// surviving a panel or cue change.
    pub fn stop_preview_voice(&self, voice_id: VoiceId) {
        self.voices.retain(|voice| {
            if voice.id == voice_id {
                voice.set_stopped();
                false
            } else {
                true
            }
        });
        if let Ok(mut aux) = self.aux_streams.lock() {
            for stream in aux.iter_mut() {
                stream.voices.retain(|voice| {
                    if voice.id == voice_id {
                        voice.set_stopped();
                        false
                    } else {
                        true
                    }
                });
            }
        }
    }

    /// Play `voice` on the device an Output Patch routes to.
    ///
    /// `None` / empty / the main device → the normal main-stream path. Any
    /// other device → a dedicated aux stream (opened on first use). If the
    /// explicit Aux device cannot be opened, return an error and keep the
    /// voice silent; never route a mis-patched cue to the Main PA.
    pub fn play_voice_routed(&self, voice: Voice, device_id: Option<&str>) -> Result<VoiceId> {
        if self.routes_to_main(device_id) {
            // Routing is healthy again — retire any stale device alert so the
            // banner always matches the latest GO.
            crate::health::clear("output-patch-device");
            return self.play_voice(voice);
        }
        // routes_to_main returned false, so device_id is Some(non-empty).
        let dev = device_id.unwrap_or_default();
        match self.submit_to_aux(voice, dev, true, true, true) {
            Ok(id) => Ok(id),
            Err((_voice, e)) => {
                log::warn!(
                    "[audio] explicit Aux patch device '{dev}' unavailable ({e}) — cue remains silent"
                );
                crate::health::set(crate::health::HealthAlert::new(
                    "output-patch-device",
                    crate::health::HealthLevel::Error,
                    format!(
                        "Explicit Aux patch output device unavailable — cue remains silent ({e})"
                    ),
                ));
                Err(e)
            }
        }
    }

    /// [`Self::play_voice_paused`] with Output Patch device routing.
    /// Used for a video's audio track when its patch targets another device.
    pub fn play_voice_paused_routed(
        &self,
        voice: Voice,
        device_id: Option<&str>,
    ) -> Result<VoiceId> {
        if self.routes_to_main(device_id) {
            crate::health::clear("output-patch-device");
            return self.play_voice_paused(voice);
        }
        let dev = device_id.unwrap_or_default();
        match self.submit_to_aux(voice, dev, false, true, true) {
            Ok(id) => Ok(id),
            Err((_voice, e)) => {
                log::warn!(
                    "[audio] explicit Aux patch device '{dev}' unavailable ({e}) — paused cue remains silent"
                );
                crate::health::set(crate::health::HealthAlert::new(
                    "output-patch-device",
                    crate::health::HealthLevel::Error,
                    format!(
                        "Explicit Aux patch output device unavailable — paused cue remains silent ({e})"
                    ),
                ));
                Err(e)
            }
        }
    }

    /// Push `voice` into the aux stream for `device_id` (opening it if
    /// needed). `start` issues the Play command; `false` submits paused.
    /// Both health flags are false for operator preview so it cannot clear or
    /// alter Output Patch health. On failure the voice is handed back to the
    /// caller, which must either report the error or keep it silent.
    fn submit_to_aux(
        &self,
        voice: Voice,
        device_id: &str,
        start: bool,
        report_patch_channel_bounds: bool,
        clear_patch_device_health: bool,
    ) -> std::result::Result<VoiceId, (Voice, anyhow::Error)> {
        let mut aux = match self.aux_streams.lock() {
            Ok(g) => g,
            Err(_) => return Err((voice, anyhow!("aux_streams mutex poisoned"))),
        };

        // Drop a dead stream for this device so it is reopened fresh below.
        aux.retain(|s| {
            !(s.device_id == device_id && s.failed.load(std::sync::atomic::Ordering::Relaxed))
        });

        if !aux.iter().any(|s| s.device_id == device_id) {
            let buffer_size = self
                .desired_config
                .lock()
                .map(|c| c.buffer_size)
                .unwrap_or(256);
            match open_aux_stream(device_id, buffer_size, Arc::clone(&self.master_gain)) {
                Ok(s) => {
                    log::info!("[audio] aux output stream opened on '{device_id}'");
                    clear_patch_device_health_if_requested(clear_patch_device_health);
                    aux.push(s);
                }
                Err(e) => return Err((voice, e)),
            }
        }

        // Both branches above guarantee the stream exists here.
        let Some(stream) = aux.iter_mut().find(|s| s.device_id == device_id) else {
            return Err((voice, anyhow!("aux stream vanished")));
        };

        if report_patch_channel_bounds {
            self.check_channel_bounds(&voice, stream.channels);
        }
        let id = voice.id;
        let arc = Arc::new(voice);
        if start {
            arc.set_playing();
        } else {
            arc.set_paused();
        }
        if let Err(arc) = stream.voices.push(arc) {
            // Poisoned aux pool — recover the (still sole-owned) Voice so the
            // caller can fall the cue back to the main output.
            let voice = Arc::try_unwrap(arc)
                .unwrap_or_else(|_| unreachable!("push failure keeps the only ref"));
            return Err((voice, anyhow!("aux voice pool poisoned")));
        }
        if start {
            let _ = stream
                .cmd_prod
                .try_push(AudioCommand::Play { voice_id: id });
        }
        Ok(id)
    }

    /// Send `cmd` to the main stream and every aux stream.  Streams that do
    /// not own the referenced voice ignore the command, so broadcasting keeps
    /// the public API device-agnostic.
    fn broadcast_command(&self, cmd: AudioCommand) -> Result<()> {
        let main = self.send_command(cmd.clone());
        if let Ok(mut aux) = self.aux_streams.lock() {
            for s in aux.iter_mut() {
                let _ = s.cmd_prod.try_push(cmd.clone());
            }
        }
        main
    }

    /// Ensure an input capture exists for `device_id` (or the default input when
    /// `None`/empty), returning the feed id to bind a Mic Cue to.
    ///
    /// Idempotent: a device is captured once and shared by all Mic Cues using it.
    /// The feed is released by [`gc_voices`](Self::gc_voices) once no live voice
    /// references it any more (so the OS mic indicator turns off after stop).
    /// Ensure a capture feed exists for `device_id`.
    ///
    /// `buffer_size` is passed to the cpal input stream (0 = OS default).
    /// Pass the same value as the output stream's configured buffer so both
    /// device clocks fire at the same period, minimising input ↔ output drift.
    pub fn ensure_input_feed(&self, device_id: Option<&str>, buffer_size: u32) -> Result<Uuid> {
        let key = device_id.unwrap_or_default().to_string();
        {
            let feeds = self
                .input_feeds
                .lock()
                .map_err(|_| anyhow!("input_feeds poisoned"))?;
            if let Some(f) = feeds.iter().find(|f| f.device_id == key) {
                return Ok(f.id);
            }
        }
        log::info!("ensure_input_feed: opening device={device_id:?} buf={buffer_size}");
        let (capture, cons) = open_input(device_id, buffer_size).map_err(|e| {
            log::error!("ensure_input_feed failed for {device_id:?}: {e}");
            e
        })?;
        let in_channels = capture.channels.max(1) as usize;
        let id = Uuid::new_v4();
        let feed = InputFeed {
            id,
            device_id: key,
            in_channels,
            sample_rate: capture.sample_rate,
            cons,
            staging: vec![0.0_f32; STAGING_FRAMES * in_channels].into_boxed_slice(),
            write_frame: 0,
            valid_from_frame: 0,
            network_jitter_frames: None,
            flush_requested: None,
            _capture: Some(capture),
        };
        self.input_feeds
            .lock()
            .map_err(|_| anyhow!("input_feeds poisoned"))?
            .push(feed);
        self.input_feed_count.fetch_add(1, Ordering::Release);
        Ok(id)
    }

    /// Register a synthetic input feed not backed by any capture device, and
    /// return its id plus a bounded producer to push samples into.
    ///
    /// Used by the Timecode Cue's LTC generator: a background thread fills the
    /// producer with encoded LTC audio, and a live voice reading this feed routes
    /// it to an Output Patch through the normal [`Self::play_mic_voice`] path.
    /// The feed is released by [`Self::gc_voices`] once its live voice stops.
    pub fn register_synthetic_feed(
        &self,
        channels: usize,
        sample_rate: u32,
    ) -> Result<(Uuid, SyntheticFeedProducer)> {
        self.register_synthetic_feed_with_jitter(channels, sample_rate, None)
    }

    /// Register decoder-fed network PCM. Unlike an LTC/ordinary synthetic
    /// feed it starts from a bounded jitter buffer and uses only a gentle clock
    /// servo in [`mix_live`].
    pub fn register_network_feed(
        &self,
        channels: usize,
        sample_rate: u32,
    ) -> Result<(Uuid, SyntheticFeedProducer)> {
        let jitter_frames = (sample_rate.max(1) as usize)
            .saturating_mul(NETWORK_LIVE_JITTER_MS as usize)
            .div_ceil(1000)
            .clamp(2, STAGING_FRAMES.saturating_sub(2));
        self.register_synthetic_feed_with_jitter(channels, sample_rate, Some(jitter_frames))
    }

    fn register_synthetic_feed_with_jitter(
        &self,
        channels: usize,
        sample_rate: u32,
        network_jitter_frames: Option<usize>,
    ) -> Result<(Uuid, SyntheticFeedProducer)> {
        let channels = channels.max(1);
        let capacity = synthetic_live_ring_frames(sample_rate).saturating_mul(channels);
        let (prod, cons) = HeapRb::<f32>::new(capacity).split();
        let flush_requested = Arc::new(AtomicBool::new(false));
        let dropped_samples = Arc::new(AtomicU64::new(0));
        let id = Uuid::new_v4();
        let feed = InputFeed {
            id,
            device_id: format!("synthetic:{id}"),
            in_channels: channels,
            sample_rate,
            cons,
            staging: vec![0.0_f32; STAGING_FRAMES * channels].into_boxed_slice(),
            write_frame: 0,
            valid_from_frame: 0,
            network_jitter_frames,
            flush_requested: Some(Arc::clone(&flush_requested)),
            _capture: None,
        };
        self.input_feeds
            .lock()
            .map_err(|_| anyhow!("input_feeds poisoned"))?
            .push(feed);
        self.input_feed_count.fetch_add(1, Ordering::Release);
        Ok((
            id,
            SyntheticFeedProducer::attached(prod, flush_requested, dropped_samples),
        ))
    }

    /// Channel count and sample rate of an existing input feed.
    fn feed_info(&self, feed_id: Uuid) -> Option<(usize, u32)> {
        self.input_feeds.lock().ok().and_then(|f| {
            f.iter()
                .find(|f| f.id == feed_id)
                .map(|f| (f.in_channels, f.sample_rate))
        })
    }

    /// Start a live (Mic Cue) voice reading input channels `in_l`/`in_r` (equal
    /// for mono) from `feed_id`, routed to output channels `out_l`/`out_r`, with
    /// optional fade-in.  Returns the voice id (use [`stop_voice`] to stop it).
    #[allow(clippy::too_many_arguments)]
    pub fn play_mic_voice(
        &self,
        feed_id: Uuid,
        in_l: usize,
        in_r: usize,
        out_l: usize,
        out_r: usize,
        gain: f32,
        pan: f32,
        fade_in_ms: u32,
        fade_curve: FadeCurve,
    ) -> Result<VoiceId> {
        self.play_mic_voice_routed(
            feed_id,
            in_l,
            in_r,
            out_l,
            out_r,
            gain,
            pan,
            fade_in_ms,
            fade_curve,
            None,
            None,
            1.0,
            None,
        )
    }

    /// Start a live input voice with explicit Output Patch routing. This is
    /// used by network Camera Cues, whose synthetic feed must follow the same
    /// selected bus, patch gain, and per-patch meter accounting as file cues.
    #[allow(clippy::too_many_arguments)]
    pub fn play_mic_voice_routed(
        &self,
        feed_id: Uuid,
        in_l: usize,
        in_r: usize,
        out_l: usize,
        out_r: usize,
        gain: f32,
        pan: f32,
        fade_in_ms: u32,
        fade_curve: FadeCurve,
        patch_id: Option<Uuid>,
        patch_slot: Option<u8>,
        patch_gain: f32,
        device_id: Option<&str>,
    ) -> Result<VoiceId> {
        let (in_ch, src_rate) = self
            .feed_info(feed_id)
            .ok_or_else(|| anyhow!("input feed {feed_id:?} not found"))?;
        // Clamp requested channels to what the device offers.
        let in_l = in_l.min(in_ch.saturating_sub(1));
        let in_r = in_r.min(in_ch.saturating_sub(1));

        let live = LiveSource::new(feed_id, in_l, in_r, src_rate);
        let mut voice = Voice::new_live(live, self.sample_rate(), gain, pan);
        voice.out_l = out_l;
        voice.out_r = out_r;
        // Mic routing is always explicit (Input Patch → Output Patch channels);
        // never shift it by the default ASIO pair offset.
        voice.patched = true;
        voice.patch_id = patch_id;
        voice.patch_slot = patch_slot;
        voice.inner.set_patch_gain(patch_gain);
        if fade_in_ms > 0 {
            let total = fade_in_ms as u64 * self.sample_rate() as u64 / 1000;
            // SAFETY: written once before the voice is shared with the callback.
            unsafe {
                *voice.inner.fade.get() = Some(FadeState {
                    direction: FadeDirection::In,
                    total_samples: total,
                    elapsed_samples: 0,
                    curve: fade_curve,
                });
            }
        }

        self.reject_preview_channel_overlap(&voice)?;

        if self.routes_to_main(device_id) {
            return self.play_voice(voice);
        }
        let dev = device_id.unwrap_or_default();
        match self.submit_to_aux(voice, dev, true, true, true) {
            Ok(id) => Ok(id),
            Err((_voice, error)) => {
                log::warn!(
                    "[audio] explicit Aux patch device '{dev}' unavailable ({error}) — live voice remains silent"
                );
                crate::health::set(crate::health::HealthAlert::new(
                    "output-patch-device",
                    crate::health::HealthLevel::Error,
                    format!("Explicit Aux patch output device unavailable — cue remains silent ({error})"),
                ));
                Err(error)
            }
        }
    }

    /// Set a single crosspoint on a playing voice (live matrix editing).
    pub fn set_voice_crosspoint(
        &self,
        voice_id: VoiceId,
        input: u8,
        output: u8,
        gain: f32,
    ) -> Result<()> {
        if gain != 0.0 {
            self.reject_reserved_matrix_channel(voice_id, output)?;
        }
        self.broadcast_command(AudioCommand::SetCrosspoint {
            voice_id,
            input,
            output,
            gain,
        })
    }

    /// Apply a whole level matrix to a playing voice, one crosspoint at a
    /// time.  `None` clears it, returning the voice to pan routing.
    pub fn set_voice_level_matrix(
        &self,
        voice_id: VoiceId,
        matrix: Option<&crate::engine::voice::LevelMatrix>,
    ) -> Result<()> {
        match matrix {
            None => self.broadcast_command(AudioCommand::ClearLevelMatrix { voice_id }),
            Some(m) => {
                for (input, row) in m.gains.iter().enumerate() {
                    for (output, &gain) in row.iter().enumerate() {
                        if gain != 0.0 {
                            self.reject_reserved_matrix_channel(voice_id, output as u8)?;
                        }
                        self.broadcast_command(AudioCommand::SetCrosspoint {
                            voice_id,
                            input: input as u8,
                            output: output as u8,
                            gain,
                        })?;
                    }
                }
                Ok(())
            }
        }
    }

    pub fn stop_voice(&self, voice_id: VoiceId, fade_ms: u32, fade_curve: FadeCurve) -> Result<()> {
        self.broadcast_command(AudioCommand::Stop {
            voice_id,
            fade_ms,
            fade_curve,
        })
    }

    /// Panic: silence every voice immediately, bypassing per-voice IDs.
    ///
    /// One ring-buffer command per stream stops its whole pool inside a
    /// single RT callback, so it works even for voices whose owning cue lost
    /// track of them (desynced bookkeeping) and cannot overflow the command
    /// ring the way N individual `Stop`s could.  Broadcast to every aux
    /// stream so patch-routed voices are silenced too.
    pub fn panic_stop_all(&self) -> Result<()> {
        self.broadcast_command(AudioCommand::StopAll)
    }

    pub fn pause_voice(&self, voice_id: VoiceId) -> Result<()> {
        self.send_command(AudioCommand::Pause { voice_id })
    }

    pub fn resume_voice(&self, voice_id: VoiceId) -> Result<()> {
        self.broadcast_command(AudioCommand::Resume { voice_id })
    }

    pub fn set_voice_gain(&self, voice_id: VoiceId, gain: f32) -> Result<()> {
        self.broadcast_command(AudioCommand::SetGain { voice_id, gain })
    }

    pub fn set_voice_pan(&self, voice_id: VoiceId, pan: f32) -> Result<()> {
        self.broadcast_command(AudioCommand::SetPan { voice_id, pan })
    }

    /// Seek a voice to the given decoded-audio frame position.
    pub fn seek_voice(&self, voice_id: VoiceId, frame_pos: u64) -> Result<()> {
        let _ = self.with_voice(voice_id, |voice| {
            if let Some(stream) = voice.stream.as_ref() {
                // Publish the generation now so the producer invalidates stale
                // PCM. The callback applies the ring reset with the Seek
                // command below; it owns the consumer index.
                stream.request_seek(frame_pos);
            }
        });
        self.broadcast_command(AudioCommand::Seek {
            voice_id,
            frame_pos,
        })
    }

    /// Live mixer fader: set the patch-gain multiplier on every playing voice
    /// routed through `patch_id` (all streams).  New voices pick the gain up
    /// from the patch table at GO.
    pub fn set_patch_gain(&self, patch_id: Uuid, gain: f32) -> Result<()> {
        self.broadcast_command(AudioCommand::SetPatchGain { patch_id, gain })
    }

    /// Devamp: release the voice's current slice loop.  The pass in progress
    /// finishes, then playback continues into the next slice — or stops at the
    /// slice boundary when `stop_at_end` is set.  No-op on unsliced voices.
    pub fn devamp_voice(&self, voice_id: VoiceId, stop_at_end: bool) -> Result<()> {
        self.broadcast_command(AudioCommand::Devamp {
            voice_id,
            stop_at_end,
        })
    }

    /// Current playback position of a voice in **file time** (ms).
    ///
    /// Reads the RT frame cursor, so it reflects loops and slice jumps —
    /// unlike a cue's wall-clock elapsed.  Used for UI time displays.
    pub fn voice_position_ms(&self, voice_id: VoiceId) -> Option<u64> {
        self.with_voice(voice_id, |v| {
            v.current_frame() * 1000 / v.sample_rate.max(1) as u64
        })
    }

    /// Close every aux stream and clear routing alerts.
    ///
    /// Called on any event that changes the routing universe — main device /
    /// backend change, Output Patch edits — so no stream keeps playing on an
    /// output the operator just re-configured away from, and no stale banner
    /// contradicts the new reality.  The next GO reopens what it needs.
    pub fn close_all_aux(&self) {
        if let Ok(mut aux) = self.aux_streams.lock() {
            if !aux.is_empty() {
                log::info!(
                    "[audio] closing {} aux output stream(s) (routing changed)",
                    aux.len()
                );
            }
            aux.clear();
        }
        crate::health::clear("output-patch-device");
        crate::health::clear("output-patch-channels");
    }

    /// Raise or clear the "patch routes to a channel the device does not
    /// have" alert for one submitted voice.  Reflects the **latest** GO, so
    /// the banner always matches what the operator just heard (or didn't).
    fn check_channel_bounds(&self, voice: &Voice, device_channels: u32) {
        let highest = voice.out_l.max(voice.out_r);
        if highest >= device_channels as usize {
            crate::health::set(crate::health::HealthAlert::new(
                "output-patch-channels",
                crate::health::HealthLevel::Warning,
                format!(
                    "Cue routed to output channel {} but the device has only {} — audio is dropped. Fix the Output Patch channels.",
                    highest + 1,
                    device_channels
                ),
            ));
        } else {
            crate::health::clear("output-patch-channels");
        }
    }

    /// Run `f` on the voice with `voice_id`, searching the main pool then
    /// every aux pool.  Returns `None` when the voice is not found.
    fn with_voice<T>(&self, voice_id: VoiceId, f: impl Fn(&Arc<Voice>) -> T) -> Option<T> {
        let find = |pool: &[Arc<Voice>]| pool.iter().find(|v| v.id == voice_id).map(&f);
        if let Some(Some(t)) = self.voices.with(find) {
            return Some(t);
        }
        let aux = self.aux_streams.lock().ok()?;
        for s in aux.iter() {
            if let Some(Some(t)) = s.voices.with(find) {
                return Some(t);
            }
        }
        None
    }

    /// Whether a preview voice still has an active transport state.  This lets
    /// UI/session owners self-heal after natural EOF even before the next GC.
    pub fn preview_voice_is_alive(&self, voice_id: VoiceId) -> bool {
        self.voice_is_alive(voice_id)
    }

    /// Whether a voice is still playing, paused, or fading. Completion logic
    /// must consult the engine before resetting a cue: a wall-clock duration
    /// can diverge from the live voice after a seek or loop.
    pub fn voice_is_alive(&self, voice_id: VoiceId) -> bool {
        self.with_voice(voice_id, |voice| {
            !matches!(voice.voice_state(), VoiceState::Stopped | VoiceState::Idle)
        })
        .unwrap_or(false)
    }

    /// Read the current linear gain of a voice.
    /// Returns 1.0 if the voice is not found.
    pub fn get_voice_gain(&self, voice_id: VoiceId) -> f32 {
        self.with_voice(voice_id, |v| v.inner.gain()).unwrap_or(1.0)
    }

    /// Read the current stereo pan of a voice (-1 = left, 0 = center, +1 = right).
    /// Returns 0.0 (center) if the voice is not found.
    pub fn get_voice_pan(&self, voice_id: VoiceId) -> f32 {
        self.with_voice(voice_id, |v| v.inner.pan()).unwrap_or(0.0)
    }

    /// Seek a voice to the given position in milliseconds.
    ///
    /// Looks up the voice's decoded sample rate to convert `position_ms` into a
    /// frame position, then sends a [`AudioCommand::Seek`] to the RT callback.
    /// Used by [`OutputEngine::seek`] which only knows wall-clock position.
    pub fn seek_voice_ms(&self, voice_id: VoiceId, position_ms: u64) -> Result<()> {
        let frame_pos = self.with_voice(voice_id, |v| position_ms * v.sample_rate as u64 / 1000);
        if let Some(fp) = frame_pos {
            self.seek_voice(voice_id, fp)?;
        }
        Ok(())
    }

    /// Drain the status ring buffers (main + aux) and return all pending
    /// status messages.
    pub fn drain_status(&self) -> Vec<AudioStatus> {
        let mut out = Vec::new();
        if let Ok(mut cons) = self.status_cons.lock() {
            while let Some(s) = cons.try_pop() {
                out.push(s);
            }
        }
        if let Ok(mut aux) = self.aux_streams.lock() {
            for s in aux.iter_mut() {
                while let Some(msg) = s.status_cons.try_pop() {
                    // Aux master-level meters would fight the main stream's on
                    // the VU display; only voice-scoped statuses pass through.
                    if !matches!(msg, AudioStatus::MasterLevels { .. }) {
                        out.push(msg);
                    }
                }
            }
        }
        out
    }

    /// Remove fully-stopped voices from the pool, and close any input capture
    /// device that no live voice still references (so the OS releases the mic
    /// once its Mic Cue stops — the device indicator turns off).
    ///
    /// Locks `voices` then `input_feeds`, matching the RT callback's order so the
    /// two never deadlock.
    pub fn gc_voices(&self) {
        self.voices
            .retain(|v| !matches!(v.voice_state(), VoiceState::Stopped | VoiceState::Idle));

        // Feed ids still referenced by a surviving live voice.
        let in_use: std::collections::HashSet<Uuid> = self
            .voices
            .with(|pool| {
                pool.iter()
                    .filter_map(|v| v.live.as_ref().map(|l| l.feed_id))
                    .collect()
            })
            .unwrap_or_default();

        // The show loop calls GC at 30 Hz. In the steady state this count check
        // keeps it entirely away from the callback's input-feed mutex. A newly
        // registered orphan or a stopped live voice makes the counts differ and
        // triggers the actual retain exactly when cleanup is needed.
        if self.input_feed_count.load(Ordering::Acquire) != in_use.len() {
            if let Ok(mut feeds) = self.input_feeds.lock() {
                feeds.retain(|f| in_use.contains(&f.id));
                self.input_feed_count.store(feeds.len(), Ordering::Release);
            }
        }

        // Aux pools: same sweep.  A healthy aux stream stays open even when
        // idle (zero device-open latency on the next GO); a failed one is
        // dropped once empty so the next GO retries a fresh open.
        if let Ok(mut aux) = self.aux_streams.lock() {
            for s in aux.iter() {
                s.voices
                    .retain(|v| !matches!(v.voice_state(), VoiceState::Stopped | VoiceState::Idle));
            }
            aux.retain(|s| {
                let empty = s.voices.with(|p| p.is_empty()).unwrap_or(true);
                let failed = s.failed.load(std::sync::atomic::Ordering::Relaxed);
                !(failed && empty)
            });
        }
    }

    /// Stop the current stream and re-open according to `config`.
    ///
    /// Playing voices are **preserved**: the voice pool is shared with the new
    /// stream's callback, so each voice resumes from its current `frame_pos` on
    /// the new device (the source-frame cursor is output-rate-independent, and
    /// channel routing is bounds-checked in `fill_buffer`).  A device switch
    /// therefore only drops audio for the brief teardown/re-open gap instead of
    /// killing the running cue — for both a planned change and an auto-fallback.
    pub fn restart(&self, config: &MachineAudioConfig) -> Result<()> {
        // Drop the old stream before opening the new one (exclusive backends
        // require the device to be released first).  Voices are intentionally
        // left intact so the new stream continues mixing them where they froze.
        {
            let mut sg = self
                ._stream
                .lock()
                .map_err(|_| anyhow!("stream mutex poisoned"))?;
            *sg = None;
        }

        // Close ALL aux streams: they belong to the previous device universe
        // (a WASAPI aux on the interface ASIO is about to grab exclusively
        // would make this restart fail), and stale ones would keep playing on
        // outputs the operator just re-configured away from.  The next GO
        // through a patch reopens exactly what the new universe needs.
        self.close_all_aux();

        let new_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sr = open_stream_inner(
            config,
            self.voices.rt_handle(),
            Arc::clone(&self.input_feeds),
            self.program_audio_taps.rt_handle(),
            Some(&self.program_audio_format),
            Arc::clone(&self.master_gain),
            Arc::clone(&self.output_period),
            Arc::clone(&new_failed),
            Arc::clone(&self.output_callbacks),
        )?;

        *self
            .cmd_prod
            .lock()
            .map_err(|_| anyhow!("cmd_prod poisoned"))? = sr.cmd_prod;
        *self
            .status_cons
            .lock()
            .map_err(|_| anyhow!("status_cons poisoned"))? = sr.status_cons;
        *self
            ._stream
            .lock()
            .map_err(|_| anyhow!("stream poisoned"))? = Some(sr.stream);
        self.sample_rate
            .store(sr.sample_rate, std::sync::atomic::Ordering::Relaxed);
        self.output_channels
            .store(sr.channels, std::sync::atomic::Ordering::Relaxed);
        self.default_out_offset
            .store(sr.default_out_offset, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut f) = self.stream_failed.lock() {
            *f = new_failed;
        }
        if let Ok(mut d) = self.current_device_id.lock() {
            *d = config.device_id.clone();
        }

        // The device set does not change on a restart (same hardware, different
        // selection), so we do not re-enumerate here — that would add a slow,
        // possibly hanging WASAPI query to this main-thread path.  The cache is
        // kept fresh by the startup warm-up and every Preferences enumeration.

        Ok(())
    }

    fn send_command(&self, cmd: AudioCommand) -> Result<()> {
        self.cmd_prod
            .lock()
            .map_err(|_| anyhow!("cmd_prod mutex poisoned"))?
            .try_push(cmd)
            .map_err(|_| anyhow!("Audio command ring buffer full"))
    }
}

// ---------------------------------------------------------------------------
// Stream builder — shared between new() and restart()
// ---------------------------------------------------------------------------

struct StreamResult {
    stream: Stream,
    sample_rate: u32,
    channels: u32,
    /// Default output-channel offset for unpatched voices (the selected ASIO
    /// pair × 2; always 0 on non-ASIO backends).
    default_out_offset: u32,
    cmd_prod: ringbuf::HeapProd<AudioCommand>,
    status_cons: ringbuf::HeapCons<AudioStatus>,
}

/// Select device, configure buffer, build and start the cpal stream.
///
/// Creates fresh ring buffers and returns them alongside the running stream so
/// the caller can wire them into the engine (either for initial construction or
/// after a restart).
fn open_stream_inner(
    config: &MachineAudioConfig,
    cb_voices: Arc<ArcSwap<Vec<Arc<Voice>>>>,
    cb_feeds: Arc<Mutex<Vec<InputFeed>>>,
    cb_program_audio_taps: Arc<ArcSwap<Vec<Arc<ProgramAudioTap>>>>,
    program_audio_format: Option<&ProgramAudioFormat>,
    cb_mg: Arc<std::sync::atomic::AtomicU32>,
    cb_period: Arc<std::sync::atomic::AtomicU32>,
    stream_failed: Arc<std::sync::atomic::AtomicBool>,
    cb_callbacks: Arc<std::sync::atomic::AtomicU64>,
) -> Result<StreamResult> {
    use crate::preferences::AudioBackend;

    let host = match config.backend {
        AudioBackend::WasapiShared
        | AudioBackend::WasapiExclusive
        | AudioBackend::SystemDefault => cpal::default_host(),
        AudioBackend::Asio => {
            // Fall back to the default host when ASIO is not compiled in
            // (e.g. `pnpm tauri dev` without `--features asio-support`).
            // This lets developers iterate without ASIO while keeping their
            // saved machine config pointing at ASIO for production builds.
            match open_asio_host() {
                Ok(h) => h,
                Err(e) => {
                    log::warn!("ASIO unavailable ({e}), falling back to default host");
                    cpal::default_host()
                }
            }
        }
    };

    let device_name = config.device_id.as_deref();

    // On Linux, `pw:<node_name>` IDs route through the `pipewire` ALSA device
    // with PIPEWIRE_NODE set.  The guard keeps the env var live until the
    // device is fully opened below.
    #[cfg(target_os = "linux")]
    let (_pw_guard, effective_name): (
        Option<crate::engine::device_manager::PwNodeGuard>,
        Option<&str>,
    ) = {
        use crate::engine::device_manager::pipewire_node_of;
        match device_name
            .filter(|s| !s.is_empty())
            .and_then(pipewire_node_of)
        {
            Some(node) => {
                let guard = crate::engine::device_manager::acquire_pw_node(node);
                (Some(guard), Some("pipewire"))
            }
            None => (None, device_name.filter(|s| !s.is_empty())),
        }
    };
    #[cfg(not(target_os = "linux"))]
    let effective_name = device_name.filter(|s| !s.is_empty());

    let device = if matches!(config.backend, AudioBackend::Asio) {
        let found = effective_name.and_then(|name| {
            host.output_devices().ok().and_then(|mut it| {
                it.find(|d| d.id().ok().map(|id| id.id() == name).unwrap_or(false))
            })
        });
        found
            .or_else(|| host.default_output_device())
            .ok_or_else(|| anyhow!(
                "No ASIO device found. Make sure the driver is not already in use by another application."
            ))?
    } else if let Some(name) = effective_name {
        host.output_devices()
            .map_err(|e| anyhow!("Failed to enumerate devices: {e}"))?
            .find(|d| d.id().ok().map(|id| id.id() == name).unwrap_or(false))
            .ok_or_else(|| anyhow!("Audio device '{}' not found", name))?
    } else {
        host.default_output_device()
            .ok_or_else(|| anyhow!("No default audio output device found"))?
    };

    let default_config = device
        .default_output_config()
        .map_err(|e| anyhow!("Device config error: {e}"))?;
    let sample_rate = default_config.sample_rate();
    let channels = default_config.channels();
    let total_ch = channels as usize;

    // Buffer size: apply Fixed on all backends except ASIO (which uses its own
    // control panel) and WASAPI Shared (where Windows owns the engine period and
    // ignores the hint).  This makes the user-configured buffer_size effective on
    // macOS (CoreAudio) and Linux (ALSA/PipeWire) — previously they always got
    // the OS default (typically 256-1024 samples), causing high Mic Cue latency
    // even when the operator had set 64 samples in Preferences.
    let buf_size = match config.backend {
        AudioBackend::Asio | AudioBackend::WasapiShared => cpal::BufferSize::Default,
        _ => cpal::BufferSize::Fixed(config.buffer_size),
    };

    let stream_cfg = StreamConfig {
        channels,
        sample_rate,
        buffer_size: buf_size,
    };

    // ASIO: default output pair for voices that have no Output Patch.
    // Applied at voice submission (play_voice), not in the callback.
    let pair_offset = if matches!(config.backend, AudioBackend::Asio) {
        (config.asio_out_pair as usize * 2).min(total_ch.saturating_sub(2))
    } else {
        0
    };

    let (cmd_prod, mut cmd_cons) = HeapRb::<AudioCommand>::new(RING_CAPACITY).split();
    let (mut status_prod, status_cons) = HeapRb::<AudioStatus>::new(RING_CAPACITY).split();

    // Throttled error callback: log at most once per second, signal stream_failed
    // on the first DeviceNotAvailable so the watchdog can switch devices quickly.
    let make_err_fn = {
        let last_log = Arc::new(std::sync::atomic::AtomicU64::new(0));
        move || {
            let last_log2 = Arc::clone(&last_log);
            let failed2 = Arc::clone(&stream_failed);
            move |err: cpal::Error| {
                // Only DeviceNotAvailable is truly fatal (device pulled or
                // exclusively grabbed).  Other kinds cover recoverable ALSA
                // errors (e.g. Xrun, POLLERR) that should not trigger the
                // watchdog restart.
                if matches!(err.kind(), cpal::ErrorKind::DeviceNotAvailable) {
                    failed2.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let prev = last_log2.swap(now, std::sync::atomic::Ordering::Relaxed);
                if now > prev {
                    log::error!("cpal stream error ({:?}): {err}", err.kind());
                }
            }
        }
    };

    // Both formats mix **full-width** (all device channels) so Output Patch
    // channel routing reaches every physical output — essential on ASIO where
    // one driver exposes all of the interface's outs.  Unpatched voices are
    // shifted to `pair_offset` at submission (see `play_voice`), not here.
    let callback_scratch_frames = (config.buffer_size as usize).max(4096);
    let stream = match default_config.sample_format() {
        cpal::SampleFormat::F32 => {
            // The canonical program bus is built beside the physical mix, not
            // recovered from device channels. Keep its storage outside the RT
            // callback and chunk an unexpectedly large device block so no
            // frame is dropped and the callback never allocates.
            let mut program_scratch = vec![0.0_f32; callback_scratch_frames * 2].into_boxed_slice();
            device.build_output_stream(
                stream_cfg,
                move |data: &mut [f32], _| {
                    cb_callbacks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let frames = data.len() / total_ch;
                    cb_period.store(frames as u32, std::sync::atomic::Ordering::Relaxed);
                    for output in data.chunks_mut(callback_scratch_frames * total_ch) {
                        let chunk_frames = output.len() / total_ch;
                        fill_buffer(
                            output,
                            &mut program_scratch[..chunk_frames * 2],
                            total_ch,
                            sample_rate,
                            &cb_voices,
                            &cb_feeds,
                            &cb_program_audio_taps,
                            &mut cmd_cons,
                            &mut status_prod,
                            &cb_mg,
                            &cb_period,
                        );
                    }
                },
                make_err_fn(),
                None,
            )?
        }
        cpal::SampleFormat::I32 => {
            // Pre-allocate both physical and canonical-program scratch. Large
            // callbacks are processed in bounded chunks rather than truncated.
            let scratch_len = callback_scratch_frames * total_ch;
            let mut scratch = vec![0.0f32; scratch_len].into_boxed_slice();
            let mut program_scratch = vec![0.0_f32; callback_scratch_frames * 2].into_boxed_slice();
            device.build_output_stream(
                stream_cfg,
                move |data: &mut [i32], _| {
                    cb_callbacks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let frames = data.len() / total_ch;
                    cb_period.store(frames as u32, std::sync::atomic::Ordering::Relaxed);
                    for destination in data.chunks_mut(scratch_len) {
                        let n = destination.len();
                        let chunk_frames = n / total_ch;
                        fill_buffer(
                            &mut scratch[..n],
                            &mut program_scratch[..chunk_frames * 2],
                            total_ch,
                            sample_rate,
                            &cb_voices,
                            &cb_feeds,
                            &cb_program_audio_taps,
                            &mut cmd_cons,
                            &mut status_prod,
                            &cb_mg,
                            &cb_period,
                        );
                        for (dst, src) in destination.iter_mut().zip(scratch[..n].iter()) {
                            *dst = (src.clamp(-1.0, 1.0) * i32::MAX as f32) as i32;
                        }
                    }
                },
                make_err_fn(),
                None,
            )?
        }
        fmt => return Err(anyhow!("Unsupported sample format: {fmt:?}")),
    };

    // Mark the old program-audio format stale *before* this new stream can
    // invoke its callback. Network workers compare this generation before
    // touching NDI/FFmpeg, so an output-device restart cannot feed samples at
    // the previous rate into a sender configured for the new one (or vice
    // versa).
    if let Some(format) = program_audio_format {
        format.update_for_restart(sample_rate);
    }
    stream.play()?;
    log::info!(
        "Audio stream opened — backend={:?} device={:?} rate={}Hz channels={} buf={:?}",
        config.backend,
        config.device_id,
        sample_rate,
        channels,
        buf_size,
    );

    Ok(StreamResult {
        stream,
        sample_rate,
        channels: channels as u32,
        default_out_offset: pair_offset as u32,
        cmd_prod,
        status_cons,
    })
}

/// Pure routing decision: does an explicitly-requested patch device match the
/// device the main stream is open on?  `main` is the main stream's device id
/// (`None` = system default, compared via `default_dev`).
fn resolves_to_main_device(requested: &str, main: Option<&str>, default_dev: Option<&str>) -> bool {
    match main {
        Some(cur) => cur == requested,
        None => default_dev == Some(requested),
    }
}

/// Open an additional output stream on `device_id` for Output Patch routing.
///
/// Reuses [`open_stream_inner`] with a synthetic config: generic default host
/// (WASAPI shared on Windows, CoreAudio on macOS, ALSA/PipeWire on Linux — no
/// ASIO, which is exclusive and single-device by design), fresh voice pool,
/// permanently empty input-feed list (live voices stay on the main stream),
/// and the shared master gain so the master fader applies everywhere.
fn open_aux_stream(
    device_id: &str,
    buffer_size: u32,
    master_gain: Arc<std::sync::atomic::AtomicU32>,
) -> Result<AuxStream> {
    use crate::preferences::AudioBackend;

    // WasapiShared on Windows selects BufferSize::Default (WASAPI shared mode
    // owns its engine period); SystemDefault elsewhere applies the configured
    // buffer like the main stream does on CoreAudio/ALSA.
    #[cfg(windows)]
    let backend = AudioBackend::WasapiShared;
    #[cfg(not(windows))]
    let backend = AudioBackend::SystemDefault;

    let config = MachineAudioConfig {
        backend,
        device_id: Some(device_id.to_string()),
        device_name: None,
        preview_device_id: None,
        preview_device_name: None,
        preview_asio_pair: None,
        preview_gain_db: 0.0,
        input_device_id: None,
        buffer_size,
        asio_out_pair: 0,
        aux_buses: Vec::new(),
    };

    let voices = VoicePool::new();
    let feeds: Arc<Mutex<Vec<InputFeed>>> = Arc::new(Mutex::new(Vec::new()));
    let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let sr = open_stream_inner(
        &config,
        voices.rt_handle(),
        feeds,
        Arc::new(ArcSwap::from_pointee(Vec::new())),
        None,
        master_gain,
        Arc::new(std::sync::atomic::AtomicU32::new(256)),
        Arc::clone(&failed),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )?;

    Ok(AuxStream {
        device_id: device_id.to_string(),
        channels: sr.channels,
        voices,
        cmd_prod: sr.cmd_prod,
        status_cons: sr.status_cons,
        failed,
        _stream: sr.stream,
    })
}

// ---------------------------------------------------------------------------
// Audio callback — real-time safe
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn fill_buffer(
    output: &mut [f32],
    program_stereo: &mut [f32],
    channels: usize,
    output_sample_rate: u32,
    voices: &ArcSwap<Vec<Arc<Voice>>>,
    input_feeds: &Arc<Mutex<Vec<InputFeed>>>,
    program_audio_taps: &ArcSwap<Vec<Arc<ProgramAudioTap>>>,
    cmd_cons: &mut ringbuf::HeapCons<AudioCommand>,
    status_prod: &mut ringbuf::HeapProd<AudioStatus>,
    master_gain: &Arc<std::sync::atomic::AtomicU32>,
    output_period: &Arc<std::sync::atomic::AtomicU32>,
) {
    output.fill(0.0);
    program_stereo.fill(0.0);

    let frames = output.len() / channels;
    debug_assert_eq!(program_stereo.len(), frames * 2);

    // A main-device callback is the single clock domain for this canonical
    // stereo bus. Output-Patch voices rendered by an aux callback and operator
    // preview are intentionally excluded here: combining independent device
    // clocks requires a separate resampling/fan-out stage, not concurrent
    // writes into these taps.
    let taps = program_audio_taps.load();
    let capture_program = !taps.is_empty();

    // Wait-free snapshot of the voice list — no lock, so the callback can never
    // be starved into emitting a block of silence (the old `try_lock` failure
    // mode).  The snapshot's producer (VoicePool) retains recent generations so
    // dropping this guard never runs a destructor on the RT thread.
    let voices_guard = voices.load();

    // Process incoming commands first.
    while let Some(cmd) = cmd_cons.try_pop() {
        apply_command(&voices_guard, cmd, status_prod);
    }

    // Keep live input feeds current: drain each device's ring into its staging
    // buffer (cheap, bounded) so live voices have fresh audio and the input
    // stays "warm" even with no Mic Cue playing.  Held for the voice loop.
    let mut feeds_guard = input_feeds.try_lock().ok();
    if let Some(feeds) = feeds_guard.as_deref_mut() {
        for feed in feeds.iter_mut() {
            feed.drain();
        }
    }

    let master = f32::from_bits(master_gain.load(std::sync::atomic::Ordering::Relaxed));

    let mut peak_l = 0.0_f32;
    let mut peak_r = 0.0_f32;
    // Per-Output-Patch peaks, indexed by Voice::patch_slot (fixed-size — no
    // allocation in the RT callback).
    let mut patch_peaks = [0.0_f32; PATCH_VU_SLOTS * 2];

    for voice in voices_guard.iter() {
        let state = voice.voice_state();
        if state != VoiceState::Playing && state != VoiceState::FadingOut {
            continue;
        }

        let mut voice_peak_l = 0.0_f32;
        let mut voice_peak_r = 0.0_f32;

        // Live (Mic Cue) voice — resample from its input feed instead of samples.
        if voice.live.is_some() {
            if let Some(feeds) = feeds_guard.as_deref() {
                mix_live(
                    output,
                    (!voice.preview_only && capture_program).then_some(&mut *program_stereo),
                    channels,
                    output_sample_rate,
                    voice,
                    feeds,
                    status_prod,
                    &mut voice_peak_l,
                    &mut voice_peak_r,
                    output_period,
                );
            }
            if !voice.preview_only {
                accumulate_peaks(
                    voice,
                    voice_peak_l,
                    voice_peak_r,
                    &mut peak_l,
                    &mut peak_r,
                    &mut patch_peaks,
                );
            }
            continue;
        }

        // File/video audio normally arrives through a bounded decoder ring.
        // The callback only performs atomic reads and ring pops here; decoder
        // I/O, allocation, and seek work stay on the worker pool.
        if voice.stream.is_some() {
            mix_stream(
                output,
                (!voice.preview_only && capture_program).then_some(&mut *program_stereo),
                channels,
                output_sample_rate,
                voice,
                status_prod,
                &mut voice_peak_l,
                &mut voice_peak_r,
            );
            if !voice.preview_only {
                accumulate_peaks(voice, voice_peak_l, voice_peak_r, &mut peak_l, &mut peak_r, &mut patch_peaks);
            }
            continue;
        }

        let patch_gain = voice.inner.patch_gain();
        let (gain_l, gain_r) = voice.pan_gains();
        let (gain_l, gain_r) = (gain_l * patch_gain, gain_r * patch_gain);
        // A level matrix carries its own routing, so it multiplies the voice
        // gain directly instead of the panned pair.
        let matrix_gain = voice.inner.gain() * patch_gain;
        // SAFETY: written once before the voice was submitted; read-only here.
        let matrix_ptr = voice.inner.level_matrix.get();
        let voice_channels = voice.channels as usize;
        let total_frames = voice.total_frames();
        // Frame advance step: user rate × (source SR / output SR).
        // voice.inner.rate() is the pure user multiplier (1.0 = normal speed).
        let rate =
            voice.inner.rate() as f64 * (voice.sample_rate as f64 / output_sample_rate as f64);

        // Maintain frame position as f64 for accurate sub-frame interpolation.
        // The fractional part is lost at callback boundaries (≤ 1 sample / ~22 µs
        // at 44 100 Hz), which is inaudible.
        let mut frame_pos_f = voice.frame_pos.load(std::sync::atomic::Ordering::Relaxed) as f64;

        // SAFETY: `fade` is only mutated from this callback; VoiceInner docs
        // establish the single-writer invariant.
        let fade_ptr = voice.inner.fade.get();

        // SAFETY: `end_frame` is written once before the voice is submitted.
        let end_frame_val: Option<u64> = unsafe { *voice.inner.end_frame.get() };
        let end = end_frame_val.unwrap_or(u64::MAX);

        // SAFETY: `slices` is written once before submission; `current` /
        // `remaining` are mutated only from this callback (single-writer).
        let slices_ptr = voice.inner.slices.get();

        let mut voice_stopped = false;

        for frame in 0..frames {
            // --- Per-frame fade gain -------------------------------------------
            let fade_gain: f32 = if let Some(fade) = unsafe { &mut *fade_ptr } {
                let g = fade.gain();
                let done = fade.advance(1);
                if done {
                    if fade.direction == FadeDirection::Out {
                        voice.set_stopped();
                        let _ = status_prod.try_push(AudioStatus::Completed { voice_id: voice.id });
                        unsafe { *fade_ptr = None };
                        voice_stopped = true;
                    } else {
                        // Fade-in complete — clear state, continue playing.
                        unsafe { *fade_ptr = None };
                    }
                }
                g
            } else {
                1.0_f32
            };

            if voice_stopped {
                break;
            }

            // --- Boundary / loop check ----------------------------------------
            let int_pos = frame_pos_f as u64;
            if let Some(prog) = unsafe { &mut *slices_ptr } {
                // Sliced playback: the program owns loop/advance decisions.
                let seg = prog.segments[prog.current];
                if int_pos >= seg.end_frame.min(total_frames) {
                    let req = voice
                        .inner
                        .devamp_request
                        .load(std::sync::atomic::Ordering::Relaxed);
                    if req == crate::engine::voice::DEVAMP_STOP {
                        voice.inner.devamp_request.store(
                            crate::engine::voice::DEVAMP_NONE,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        voice.set_stopped();
                        let _ = status_prod.try_push(AudioStatus::Completed { voice_id: voice.id });
                        break;
                    }
                    if req == crate::engine::voice::DEVAMP_CONTINUE {
                        voice.inner.devamp_request.store(
                            crate::engine::voice::DEVAMP_NONE,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        prog.remaining = 0;
                    }
                    if prog.remaining > 0 {
                        if prog.remaining != u32::MAX {
                            prog.remaining -= 1;
                        }
                        frame_pos_f = seg.start_frame as f64;
                    } else if prog.current + 1 < prog.segments.len() {
                        prog.current += 1;
                        let next = prog.segments[prog.current];
                        prog.remaining = if next.play_count == u32::MAX {
                            u32::MAX
                        } else {
                            next.play_count.saturating_sub(1)
                        };
                        frame_pos_f = next.start_frame as f64;
                    } else {
                        voice.set_stopped();
                        let _ = status_prod.try_push(AudioStatus::Completed { voice_id: voice.id });
                        break;
                    }
                }
            } else if int_pos >= end || int_pos >= total_frames {
                let loops = voice
                    .inner
                    .loops_remaining
                    .load(std::sync::atomic::Ordering::Relaxed);
                if loops > 0 {
                    if loops != u32::MAX {
                        voice
                            .inner
                            .loops_remaining
                            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    frame_pos_f = 0.0;
                } else {
                    voice.set_stopped();
                    let _ = status_prod.try_push(AudioStatus::Completed { voice_id: voice.id });
                    break;
                }
            }

            // --- Sample with linear interpolation (handles rate != 1.0) --------
            let int_pos = frame_pos_f as u64;
            let frac = (frame_pos_f - int_pos as f64) as f32;
            let base = int_pos as usize * voice_channels;
            // Clamp next frame to last valid frame for interpolation at end.
            let next = (int_pos + 1).min(total_frames.saturating_sub(1)) as usize * voice_channels;

            let sample_l = voice.samples[base] + (voice.samples[next] - voice.samples[base]) * frac;
            let sample_r = if voice_channels > 1 {
                voice.samples[base + 1] + (voice.samples[next + 1] - voice.samples[base + 1]) * frac
            } else {
                sample_l
            };

            let out_base = frame * channels;
            // The canonical bus is deliberately upstream of hardware routing:
            // pan/cue gain, patch fader and fade are artistic levels; ASIO
            // offset, physical out_l/out_r and LevelMatrix are device routing.
            // There is no unique stereo inverse for a physical matrix.
            let program_l = sample_l * gain_l * fade_gain;
            let program_r = sample_r * gain_r * fade_gain;
            if capture_program && !voice.preview_only {
                program_stereo[frame * 2] += program_l;
                program_stereo[frame * 2 + 1] += program_r;
            }

            // SAFETY: `level_matrix` is written once before submission and only
            // read here.
            if let Some(matrix) = unsafe { &*matrix_ptr } {
                // Crosspoint routing replaces pan and the out_l/out_r pair —
                // those are this same mix, restricted to two channels.
                let level = matrix_gain * fade_gain;
                for out in 0..matrix.width.min(channels) {
                    let mixed =
                        (sample_l * matrix.gains[0][out] + sample_r * matrix.gains[1][out]) * level;
                    output[out_base + out] += mixed;
                    // Even outputs feed the left meter, odd the right, so the
                    // VU stays meaningful whatever the channel count.
                    if out % 2 == 0 {
                        voice_peak_l = voice_peak_l.max(mixed.abs());
                    } else {
                        voice_peak_r = voice_peak_r.max(mixed.abs());
                    }
                }
            } else {
                // Route to the per-voice output channels (from Output Patch).
                // Bounds-check at the sample level keeps the RT callback safe even
                // if the patch references a channel the device does not have.
                if voice.out_l == voice.out_r {
                    if voice.out_l < channels {
                        output[out_base + voice.out_l] +=
                            (program_l + program_r) * std::f32::consts::FRAC_1_SQRT_2;
                    }
                } else {
                    if voice.out_l < channels {
                        output[out_base + voice.out_l] += program_l;
                    }
                    if voice.out_r < channels {
                        output[out_base + voice.out_r] += program_r;
                    }
                }

                voice_peak_l = voice_peak_l.max(program_l.abs());
                voice_peak_r = voice_peak_r.max(program_r.abs());
            }

            frame_pos_f += rate;
        }

        // Store integer floor; sub-frame precision is re-established each callback.
        voice
            .frame_pos
            .store(frame_pos_f as u64, std::sync::atomic::Ordering::Relaxed);

        if !voice.preview_only {
            accumulate_peaks(
                voice,
                voice_peak_l,
                voice_peak_r,
                &mut peak_l,
                &mut peak_r,
                &mut patch_peaks,
            );
        }
    }

    // Apply master gain.
    for s in output.iter_mut() {
        *s *= master;
    }
    if capture_program {
        for sample in program_stereo.iter_mut() {
            *sample *= master;
        }
    }

    // Each encoder gets the same canonical post-master stereo bus and an
    // independent bounded ring, so transport backpressure cannot delay audio.
    for tap in taps.iter() {
        tap.push_stereo(program_stereo, 2);
    }

    let _ = status_prod.try_push(AudioStatus::MasterLevels {
        peak_l: peak_l * master,
        peak_r: peak_r * master,
    });
    for slot in 0..PATCH_VU_SLOTS {
        let (l, r) = (patch_peaks[slot * 2], patch_peaks[slot * 2 + 1]);
        if l > 0.0 || r > 0.0 {
            let _ = status_prod.try_push(AudioStatus::PatchLevels {
                slot: slot as u8,
                peak_l: l * master,
                peak_r: r * master,
            });
        }
    }
}

/// Mix one bounded streaming source.  Missing PCM is rendered as silence and
/// reported through the bounded status ring; no callback operation waits for a
/// decoder worker to refill the source.
fn complete_stream_at_eof(
    source: &crate::cue::media_decode::StreamingAudioSource,
    voice: &Arc<Voice>,
    status_prod: &mut ringbuf::HeapProd<AudioStatus>,
) -> bool {
    // A seek publishes a new generation and clears EOF before the callback
    // resets the cursor.  Never turn that transient empty ring into a stale
    // completion event.
    if !source.is_eof() || source.is_cancelled() || source.seek_pending() {
        return false;
    }
    if voice.complete_once() {
        let _ = status_prod.try_push(AudioStatus::Completed { voice_id: voice.id });
        true
    } else {
        false
    }
}

fn mix_stream(
    output: &mut [f32],
    mut program_stereo: Option<&mut [f32]>,
    channels: usize,
    output_sample_rate: u32,
    voice: &Arc<Voice>,
    status_prod: &mut ringbuf::HeapProd<AudioStatus>,
    peak_l: &mut f32,
    peak_r: &mut f32,
) {
    let Some(source) = voice.stream.as_ref() else { return; };
    // A control-path seek publishes a new generation before its ring command
    // is observed. Apply it here as a wait-free safety net so stale PCM can
    // never leak for an extra callback if command delivery is delayed.
    if source.seek_pending() {
        source.apply_seek_rt();
        voice.reset_stream_cursor();
        voice
            .frame_pos
            .store(source.requested_frame(), Ordering::Relaxed);
    }
    // Do not start consuming until the decoder has built the 750 ms ready
    // watermark. This makes GO deterministic without ever waiting in RT.
    // A short file may reach EOF before it can fill the normal watermark;
    // once EOF is known, drain whatever PCM remains instead of waiting for a
    // watermark that can never be reached.
    if !source.is_ready() && !(source.is_eof() && source.buffered_samples() > 0) {
        if source.is_eof() && source.buffered_samples() == 0 {
            complete_stream_at_eof(source, voice, status_prod);
            return;
        }
        source.note_underrun_frames(output.len() / channels.max(1));
        return;
    }
    let frames = output.len() / channels;
    let patch_gain = voice.inner.patch_gain();
    let (gain_l, gain_r) = voice.pan_gains();
    let gain_l = gain_l * patch_gain;
    let gain_r = gain_r * patch_gain;
    let matrix_gain = voice.inner.gain() * patch_gain;
    let matrix_ptr = voice.inner.level_matrix.get();
    let fade_ptr = voice.inner.fade.get();
    let end = unsafe { *voice.inner.end_frame.get() }.unwrap_or(u64::MAX);
    let mut frame_pos = voice.current_frame();
    let ratio = (voice.inner.rate() as f64
        * source.sample_rate as f64
        / output_sample_rate.max(1) as f64)
        .max(0.001);
    let cursor_ptr = voice.inner.stream_cursor.get();
    let underruns_before = source.underruns();

    for frame in 0..frames {
        let fade_gain = if let Some(fade) = unsafe { &mut *fade_ptr } {
            let g = fade.gain();
            if fade.advance(1) {
                if fade.direction == FadeDirection::Out {
                    voice.set_stopped();
                    let _ = status_prod.try_push(AudioStatus::Completed { voice_id: voice.id });
                    break;
                }
                unsafe { *fade_ptr = None };
            }
            g
        } else { 1.0 };
        if voice.voice_state() == VoiceState::Stopped { break; }

        if let Some(program) = unsafe { &mut *voice.inner.slices.get() } {
            let segment = program.segments[program.current];
            if frame_pos >= segment.end_frame {
                let request = voice.inner.devamp_request.load(Ordering::Relaxed);
                if request == crate::engine::voice::DEVAMP_STOP {
                    voice.inner.devamp_request.store(crate::engine::voice::DEVAMP_NONE, Ordering::Relaxed);
                    voice.set_stopped();
                    let _ = status_prod.try_push(AudioStatus::Completed { voice_id: voice.id });
                    break;
                }
                if request == crate::engine::voice::DEVAMP_CONTINUE {
                    voice.inner.devamp_request.store(crate::engine::voice::DEVAMP_NONE, Ordering::Relaxed);
                    program.remaining = 0;
                }
                if program.remaining > 0 {
                    if program.remaining != u32::MAX { program.remaining -= 1; }
                    source.request_seek_rt(segment.start_frame);
                    voice.reset_stream_cursor();
                    frame_pos = segment.start_frame;
                    continue;
                }
                if program.current + 1 < program.segments.len() {
                    program.current += 1;
                    let next = program.segments[program.current];
                    program.remaining = if next.play_count == u32::MAX { u32::MAX } else { next.play_count.saturating_sub(1) };
                    source.request_seek_rt(next.start_frame);
                    voice.reset_stream_cursor();
                    frame_pos = next.start_frame;
                    continue;
                }
                voice.set_stopped();
                let _ = status_prod.try_push(AudioStatus::Completed { voice_id: voice.id });
                break;
            }
        } else if frame_pos >= end
            && !(voice.stream.is_some()
                && voice.inner.loops_remaining.load(Ordering::Relaxed) == 0
                && unsafe { (*cursor_ptr).initialized && (*cursor_ptr).current_is_tail })
        {
            let loops = voice.inner.loops_remaining.load(Ordering::Relaxed);
            if loops > 0 {
                if loops != u32::MAX { voice.inner.loops_remaining.fetch_sub(1, Ordering::Relaxed); }
                source.request_seek_rt(0);
                voice.reset_stream_cursor();
                frame_pos = 0;
            } else {
                voice.set_stopped();
                let _ = status_prod.try_push(AudioStatus::Completed { voice_id: voice.id });
                break;
            }
        }

        // Keep a two-frame lookahead across callback blocks. This preserves the
        // old Voice linear interpolation and source/output-rate + user-rate
        // semantics without touching locks or allocating in the callback.
        let cursor = unsafe { &mut *cursor_ptr };
        if !cursor.initialized {
            if !source.pop_frame(&mut cursor.current) {
                if source.is_eof() && source.buffered_samples() == 0 {
                    complete_stream_at_eof(source, voice, status_prod);
                    break;
                }
                continue;
            }
            cursor.next_valid = source.pop_frame(&mut cursor.next);
            cursor.current_is_tail = !cursor.next_valid
                && source.is_eof()
                && source.buffered_samples() == 0;
            if !cursor.next_valid { cursor.next = cursor.current; }
            cursor.phase = 0.0;
            cursor.initialized = true;
        }
        let frac = cursor.phase as f32;
        let sample = [
            cursor.current[0] + (cursor.next[0] - cursor.current[0]) * frac,
            cursor.current[1] + (cursor.next[1] - cursor.current[1]) * frac,
        ];
        cursor.phase += ratio;
        // If a high output rate crosses more than one source frame in a
        // single callback frame, reaching EOF can move `current` to the last
        // frame before that frame has actually been rendered.  Defer terminal
        // completion until a later iteration has emitted this held tail.
        let mut tail_consumed = false;
        let mut tail_started_this_output = false;
        while cursor.phase >= 1.0 {
            cursor.current = cursor.next;
            if cursor.next_valid {
                cursor.next_valid = source.pop_frame(&mut cursor.next);
                if !cursor.next_valid {
                    cursor.next = cursor.current;
                    cursor.current_is_tail = source.is_eof()
                        && source.buffered_samples() == 0;
                    tail_started_this_output |= cursor.current_is_tail;
                } else {
                    cursor.current_is_tail = false;
                }
            } else if source.pop_frame(&mut cursor.next) {
                cursor.next_valid = true;
                cursor.current_is_tail = false;
            } else {
                cursor.next = cursor.current;
                if cursor.current_is_tail && !tail_started_this_output {
                    tail_consumed = true;
                }
            }
            cursor.phase -= 1.0;
            frame_pos = frame_pos.saturating_add(1);
        }
        let out_base = frame * channels;
        let program_l = sample[0] * gain_l * fade_gain;
        let program_r = sample[1] * gain_r * fade_gain;
        if let Some(program) = program_stereo.as_deref_mut() {
            program[frame * 2] += program_l;
            program[frame * 2 + 1] += program_r;
        }
        if let Some(matrix) = unsafe { &*matrix_ptr } {
            let level = matrix_gain * fade_gain;
            for out in 0..matrix.width.min(channels) {
                let mixed = (sample[0] * matrix.gains[0][out] + sample[1] * matrix.gains[1][out]) * level;
                output[out_base + out] += mixed;
                if out % 2 == 0 { *peak_l = (*peak_l).max(mixed.abs()); } else { *peak_r = (*peak_r).max(mixed.abs()); }
            }
        } else if voice.out_l == voice.out_r {
            if voice.out_l < channels { output[out_base + voice.out_l] += (program_l + program_r) * std::f32::consts::FRAC_1_SQRT_2; }
        } else {
            if voice.out_l < channels { output[out_base + voice.out_l] += program_l; }
            if voice.out_r < channels { output[out_base + voice.out_r] += program_r; }
        }
        *peak_l = (*peak_l).max(program_l.abs());
        *peak_r = (*peak_r).max(program_r.abs());

        // `current_is_tail` is set only after the decoder has returned EOF and
        // no lookahead frame remains.  The output above has now emitted that
        // final held frame, so this is the first safe point to complete.  An
        // EOF with a non-empty ring never reaches this branch prematurely.
        if tail_consumed {
            complete_stream_at_eof(source, voice, status_prod);
            break;
        }
    }
    voice.frame_pos.store(frame_pos, Ordering::Relaxed);
    let underruns_after = source.underruns();
    if underruns_after > underruns_before && source.should_report_underrun(underruns_after) {
        let _ = status_prod.try_push(AudioStatus::Underrun { voice_id: voice.id, count: underruns_after });
    }
}

/// Number of Output Patches metered by the per-patch VU (fixed so the RT
/// callback can accumulate into a stack array — patches beyond this many are
/// simply unmetered in the mixer).
pub const PATCH_VU_SLOTS: usize = 16;

/// Fold one voice's block peaks into the master meters and, when the voice is
/// routed through a metered Output Patch, into that patch's VU slot.
fn accumulate_peaks(
    voice: &Arc<Voice>,
    voice_peak_l: f32,
    voice_peak_r: f32,
    peak_l: &mut f32,
    peak_r: &mut f32,
    patch_peaks: &mut [f32; PATCH_VU_SLOTS * 2],
) {
    *peak_l = (*peak_l).max(voice_peak_l);
    *peak_r = (*peak_r).max(voice_peak_r);
    if let Some(slot) = voice.patch_slot {
        let slot = slot as usize;
        if slot < PATCH_VU_SLOTS {
            patch_peaks[slot * 2] = patch_peaks[slot * 2].max(voice_peak_l);
            patch_peaks[slot * 2 + 1] = patch_peaks[slot * 2 + 1].max(voice_peak_r);
        }
    }
}

/// Mix one live (Mic Cue) voice from its input feed into `output`.
///
/// Resamples the input device clock to the output clock with adaptive drift
/// compensation: the read cursor is kept ~25 ms behind the feed's write head,
/// and the resample ratio is nudged ±2 % to hold that lag, so slow clock drift
/// between the input and output devices never under/overruns.  Applies the
/// voice's pan/gain, soft fade, and Output-Patch channel routing exactly like
/// the file path.
#[allow(clippy::too_many_arguments)]
fn mix_live(
    output: &mut [f32],
    mut program_stereo: Option<&mut [f32]>,
    channels: usize,
    output_sample_rate: u32,
    voice: &Arc<Voice>,
    feeds: &[InputFeed],
    status_prod: &mut ringbuf::HeapProd<AudioStatus>,
    peak_l: &mut f32,
    peak_r: &mut f32,
    output_period: &Arc<std::sync::atomic::AtomicU32>,
) {
    let Some(live) = voice.live.as_ref() else {
        return;
    };
    let Some(feed) = feeds.iter().find(|f| f.id == live.feed_id) else {
        return;
    };
    let frames = output.len() / channels;
    // Keep the read cursor 3 output periods behind the write head.  This gives
    // enough headroom to absorb one missed input callback without underrunning,
    // while minimising the imposed latency.  With 64-sample buffers at 48kHz
    // that is 3 × 64 / 48000 ≈ 4 ms — vs. the old fixed 1200 samples (25 ms).
    let period = output_period
        .load(std::sync::atomic::Ordering::Relaxed)
        .max(64) as f64;
    let target_lag = feed
        .network_jitter_frames
        .map(|frames| frames as f64)
        .unwrap_or_else(|| (period * 3.0).max(192.0));

    let valid_from = feed.valid_from_frame as f64;
    if live.read_frame() < valid_from {
        live.mark_waiting();
        live.set_read_frame(valid_from);
    }

    if !live.is_started() {
        let anchor = live.read_frame().max(valid_from);
        let available = feed.write_frame as f64 - anchor;
        let required = if feed.network_jitter_frames.is_some() {
            target_lag
        } else {
            2.0
        };
        if available < required {
            return;
        }
        live.set_read_frame((feed.write_frame as f64 - target_lag).max(valid_from));
        live.mark_started();
    }

    let base_ratio = live.src_rate as f64 / output_sample_rate as f64;
    let mut read = live.read_frame();
    // Resync on a gross lag (resume after pause, glitch, or runaway drift):
    // jump the cursor back to `target_lag` behind the write head.
    if !(0.0..=STAGING_FRAMES as f64).contains(&(feed.write_frame as f64 - read)) {
        read = (feed.write_frame as f64 - target_lag).max(0.0);
    }
    // Adaptive: nudge the ratio to hold the read cursor near `target_lag`.
    let lag = feed.write_frame as f64 - read;
    let max_correction = if feed.network_jitter_frames.is_some() {
        NETWORK_LIVE_MAX_RATE_CORRECTION
    } else {
        0.02
    };
    let correction = ((lag - target_lag) / target_lag).clamp(-max_correction, max_correction);
    let ratio = base_ratio * (1.0 + correction);

    let patch_gain = voice.inner.patch_gain();
    let (gain_l, gain_r) = voice.pan_gains();
    let (gain_l, gain_r) = (gain_l * patch_gain, gain_r * patch_gain);
    // SAFETY: `fade` is only mutated from this callback thread.
    let fade_ptr = voice.inner.fade.get();
    // Oldest frame still resident in the circular staging buffer.
    let oldest = (feed.write_frame as f64 - (STAGING_FRAMES as f64 - 2.0)).max(valid_from);

    for frame in 0..frames {
        let fade_gain: f32 = if let Some(fade) = unsafe { &mut *fade_ptr } {
            let g = fade.gain();
            let done = fade.advance(1);
            if done {
                if fade.direction == FadeDirection::Out {
                    voice.set_stopped();
                    let _ = status_prod.try_push(AudioStatus::Completed { voice_id: voice.id });
                    unsafe { *fade_ptr = None };
                    break;
                } else {
                    unsafe { *fade_ptr = None };
                }
            }
            g
        } else {
            1.0
        };

        if read < oldest {
            read = oldest.max(0.0);
        }
        // Underrun: not enough fresh audio yet — stop here, resume next block.
        if read + 1.0 >= feed.write_frame as f64 {
            if feed.network_jitter_frames.is_some() {
                live.set_read_frame(read);
                live.mark_waiting();
            }
            break;
        }

        let s_l = feed.sample(live.in_l, read);
        let s_r = if live.in_r == live.in_l {
            s_l
        } else {
            feed.sample(live.in_r, read)
        };
        let out_l = s_l * gain_l * fade_gain;
        let out_r = s_r * gain_r * fade_gain;

        if let Some(program) = program_stereo.as_deref_mut() {
            program[frame * 2] += out_l;
            program[frame * 2 + 1] += out_r;
        }

        let base = frame * channels;
        if voice.out_l == voice.out_r {
            if voice.out_l < channels {
                output[base + voice.out_l] += (out_l + out_r) * std::f32::consts::FRAC_1_SQRT_2;
            }
        } else {
            if voice.out_l < channels {
                output[base + voice.out_l] += out_l;
            }
            if voice.out_r < channels {
                output[base + voice.out_r] += out_r;
            }
        }

        *peak_l = (*peak_l).max(out_l.abs());
        *peak_r = (*peak_r).max(out_r.abs());

        read += ratio;
    }

    live.set_read_frame(read);
}

fn apply_command(
    voices: &[Arc<Voice>],
    cmd: AudioCommand,
    _status_prod: &mut ringbuf::HeapProd<AudioStatus>,
) {
    match cmd {
        AudioCommand::Play { voice_id } => {
            if let Some(v) = voices.iter().find(|v| v.id == voice_id) {
                v.set_playing();
            }
        }
        AudioCommand::Stop {
            voice_id,
            fade_ms,
            fade_curve,
        } => {
            if let Some(v) = voices.iter().find(|v| v.id == voice_id) {
                // A paused voice (e.g. a video's audio that was never resumed
                // because the video was replaced before its first frame) must
                // hard-stop: fading it would set it Playing and make it audible.
                if fade_ms == 0 || v.voice_state() == VoiceState::Paused {
                    v.set_stopped();
                } else {
                    let total = (fade_ms as u64 * v.sample_rate as u64) / 1000;
                    // SAFETY: Only written from this callback.
                    unsafe {
                        *v.inner.fade.get() = Some(FadeState {
                            direction: FadeDirection::Out,
                            total_samples: total,
                            elapsed_samples: 0,
                            curve: fade_curve,
                        });
                    }
                    v.state.store(
                        VoiceState::FadingOut as u8,
                        std::sync::atomic::Ordering::Release,
                    );
                }
            }
        }
        AudioCommand::SetCrosspoint {
            voice_id,
            input,
            output,
            gain,
        } => {
            if let Some(v) = voices.iter().find(|v| v.id == voice_id) {
                let (i, o) = (input as usize, output as usize);
                if i < crate::engine::voice::MATRIX_INPUTS
                    && o < crate::engine::voice::MATRIX_OUTPUTS
                {
                    // SAFETY: `level_matrix` is only mutated here, from the
                    // single callback thread.
                    unsafe {
                        let slot = &mut *v.inner.level_matrix.get();
                        let matrix =
                            slot.get_or_insert_with(crate::engine::voice::LevelMatrix::silent);
                        matrix.gains[i][o] = gain;
                        matrix.recompute_width();
                    }
                }
            }
        }
        AudioCommand::ClearLevelMatrix { voice_id } => {
            if let Some(v) = voices.iter().find(|v| v.id == voice_id) {
                // SAFETY: as above — single-writer, inside the callback.
                unsafe { *v.inner.level_matrix.get() = None };
            }
        }
        AudioCommand::Pause { voice_id } => {
            if let Some(v) = voices.iter().find(|v| v.id == voice_id) {
                v.set_paused();
            }
        }
        AudioCommand::Resume { voice_id } => {
            if let Some(v) = voices.iter().find(|v| v.id == voice_id) {
                v.set_playing();
            }
        }
        AudioCommand::SetGain { voice_id, gain } => {
            if let Some(v) = voices.iter().find(|v| v.id == voice_id) {
                v.inner.set_gain(gain);
            }
        }
        AudioCommand::SetPan { voice_id, pan } => {
            if let Some(v) = voices.iter().find(|v| v.id == voice_id) {
                v.inner.set_pan(pan);
            }
        }
        AudioCommand::SetMasterGain { .. } => {}
        AudioCommand::SetPatchGain { patch_id, gain } => {
            for v in voices.iter().filter(|v| v.patch_id == Some(patch_id)) {
                v.inner.set_patch_gain(gain);
            }
        }
        AudioCommand::StopAll => {
            for v in voices {
                v.set_stopped();
            }
        }
        AudioCommand::Seek {
            voice_id,
            frame_pos,
        } => {
            if let Some(v) = voices.iter().find(|v| v.id == voice_id) {
                if let Some(stream) = v.stream.as_ref() {
                    stream.apply_seek_rt();
                }
                v.reset_stream_cursor();
                v.frame_pos
                    .store(frame_pos, std::sync::atomic::Ordering::Relaxed);
            }
        }
        AudioCommand::Devamp {
            voice_id,
            stop_at_end,
        } => {
            if let Some(v) = voices.iter().find(|v| v.id == voice_id) {
                let req = if stop_at_end {
                    crate::engine::voice::DEVAMP_STOP
                } else {
                    crate::engine::voice::DEVAMP_CONTINUE
                };
                v.inner
                    .devamp_request
                    .store(req, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

/// Open and return the ASIO cpal host.
///
/// Requires the `asio-support` Cargo feature. Returns an error when the
/// feature is absent or no ASIO host is detected at runtime.
fn open_asio_host() -> Result<cpal::Host> {
    #[cfg(all(windows, feature = "asio-support"))]
    {
        let asio = cpal::available_hosts()
            .into_iter()
            .filter(|id| *id != cpal::default_host().id())
            .find_map(|id| cpal::host_from_id(id).ok());
        asio.ok_or_else(|| anyhow!("No ASIO host found. Ensure your ASIO drivers are installed."))
    }
    #[cfg(not(all(windows, feature = "asio-support")))]
    {
        Err(anyhow!(
            "ASIO support is not compiled in. \
             Install the Steinberg ASIO SDK, set CPAL_ASIO_DIR, \
             then build with: pnpm tauri build -- --features asio-support"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::media_decode::StreamingAudioSource;
    use ringbuf::traits::{Producer, Split};
    use ringbuf::HeapRb;

    /// Build a minimal Voice with `n_frames` of silence at the given sample rate.
    fn make_voice(n_frames: usize, channels: u16, sample_rate: u32, rate: f32) -> Arc<Voice> {
        let samples = Arc::new(vec![0.0f32; n_frames * channels as usize]);
        let v = Voice::new(samples, channels, sample_rate, 1.0, 0.0);
        v.inner.set_rate(rate);
        v.set_playing();
        Arc::new(v)
    }

    /// An RT voice-list handle (the ArcSwap the callback reads) holding `voices`.
    fn rt_pool(voices: Vec<Arc<Voice>>) -> Arc<ArcSwap<Vec<Arc<Voice>>>> {
        Arc::new(ArcSwap::from_pointee(voices))
    }

    fn empty_program_audio_taps() -> Arc<ArcSwap<Vec<Arc<ProgramAudioTap>>>> {
        Arc::new(ArcSwap::from_pointee(Vec::new()))
    }

    /// Call fill_buffer for `output_frames` output frames and return the
    /// resulting frame_pos stored in the voice.
    fn run_fill(voice: Arc<Voice>, output_frames: usize, output_sr: u32) -> u64 {
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        let feeds: Arc<Mutex<Vec<InputFeed>>> = Arc::new(Mutex::new(Vec::new()));
        let (_, mut cmd_cons) = HeapRb::<AudioCommand>::new(16).split();
        let (mut status_prod, _) = HeapRb::<AudioStatus>::new(16).split();
        let master = Arc::new(std::sync::atomic::AtomicU32::new(f32::to_bits(1.0)));
        let period = Arc::new(std::sync::atomic::AtomicU32::new(output_frames as u32));
        let mut output = vec![0.0f32; output_frames * 2];
        let mut program = vec![0.0f32; output_frames * 2];
        let taps = empty_program_audio_taps();
        fill_buffer(
            &mut output,
            &mut program,
            2,
            output_sr,
            &pool,
            &feeds,
            &taps,
            &mut cmd_cons,
            &mut status_prod,
            &master,
            &period,
        );
        voice.frame_pos.load(std::sync::atomic::Ordering::Relaxed)
    }

    // The SR ratio (e.g. 44100/48000) is not exactly representable in f64, so
    // frame_pos after N output frames may be off by ±1 source frame.  All
    // assertions below allow a tolerance of 1 frame.
    fn assert_frame_pos(actual: u64, expected: u64, msg: &str) {
        let diff = (actual as i64 - expected as i64).unsigned_abs();
        assert!(diff <= 1, "{msg}: expected {expected} ± 1, got {actual}");
    }

    #[test]
    fn routing_no_device_goes_to_main() {
        // Decision covered by routes_to_main's early return; the pure helper
        // only sees explicitly-requested devices.
        assert!(resolves_to_main_device("dev-a", Some("dev-a"), None));
    }

    #[test]
    fn routing_matching_main_device() {
        assert!(resolves_to_main_device(
            "focusrite",
            Some("focusrite"),
            None
        ));
        assert!(!resolves_to_main_device(
            "hdmi-out",
            Some("focusrite"),
            None
        ));
    }

    #[test]
    fn asio_preview_pair_rejects_main_and_out_of_range_pairs() {
        let conflict = validate_asio_preview_pair(1, 1, 8).unwrap_err().to_string();
        assert!(conflict.contains("conflicts with the main program pair"));
        let unavailable = validate_asio_preview_pair(4, 0, 8).unwrap_err().to_string();
        assert!(unavailable.contains("unavailable"));
        assert!(validate_asio_preview_pair(2, 0, 8).is_ok());
    }

    #[test]
    fn preview_gain_updates_only_preview_voices_and_preserves_cue_gain() {
        let engine = AudioEngine::new_silent(&MachineAudioConfig::default());
        let program = Arc::new(Voice::new(
            Arc::new(vec![0.0; 2]),
            2,
            48_000,
            0.7,
            0.0,
        ));
        let mut preview_voice = Voice::new(
            Arc::new(vec![0.0; 2]),
            2,
            48_000,
            0.7,
            0.0,
        );
        preview_voice.preview_only = true;
        let preview = Arc::new(preview_voice);
        assert!(engine.voices.push(Arc::clone(&program)).is_ok());
        assert!(engine.voices.push(Arc::clone(&preview)).is_ok());

        engine.set_preview_gain_db(-6.0).unwrap();

        assert_eq!(program.inner.gain(), 0.7);
        assert_eq!(preview.inner.gain(), 0.7);
        assert_eq!(program.inner.patch_gain(), 1.0);
        let expected = crate::cue::types::db_to_linear(-6.0);
        assert!((preview.inner.patch_gain() as f64 - expected).abs() < 0.000_001);
    }

    #[test]
    fn asio_preview_voice_uses_only_its_selected_pair() {
        let mut voice = Voice::new(Arc::new(vec![1.0; 8]), 2, 48_000, 1.0, 0.0);
        voice.out_l = 2;
        voice.out_r = 3;
        voice.patched = true;
        voice.preview_only = true;
        voice.set_playing();
        let voice = Arc::new(voice);
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        let feeds: Arc<Mutex<Vec<InputFeed>>> = Arc::new(Mutex::new(Vec::new()));
        let (_, mut cmd_cons) = HeapRb::<AudioCommand>::new(16).split();
        let (mut status_prod, _) = HeapRb::<AudioStatus>::new(16).split();
        let master = Arc::new(std::sync::atomic::AtomicU32::new(f32::to_bits(1.0)));
        let period = Arc::new(std::sync::atomic::AtomicU32::new(2));
        let mut output = vec![0.0f32; 8];
        let mut program = vec![0.0f32; 4];
        let taps = empty_program_audio_taps();
        fill_buffer(
            &mut output,
            &mut program,
            4,
            48_000,
            &pool,
            &feeds,
            &taps,
            &mut cmd_cons,
            &mut status_prod,
            &master,
            &period,
        );
        assert_eq!(&output[..2], &[0.0, 0.0]);
        assert!(output[2] > 0.0 && output[3] > 0.0);
        assert_eq!(program, vec![0.0; 4]);
    }

    #[test]
    fn preview_pair_guard_covers_stereo_and_level_matrix_routes() {
        let mut stereo = Voice::new(Arc::new(vec![0.0; 2]), 2, 48_000, 1.0, 0.0);
        stereo.out_l = 4;
        stereo.out_r = 5;
        stereo.patched = true;
        assert!(voice_routes_to_preview_pair(&stereo, 2));

        let matrix = Voice::new(Arc::new(vec![0.0; 2]), 2, 48_000, 1.0, 0.0);
        let mut gains = vec![vec![0.0; 6]; 2];
        gains[0][5] = 1.0;
        // The matrix stores device channel indices, not patch-column indices.
        unsafe { *matrix.inner.level_matrix.get() = crate::engine::voice::LevelMatrix::new(&gains) };
        assert!(voice_routes_to_preview_pair(&matrix, 2));
    }

    #[test]
    fn routing_matching_system_default() {
        // Main stream on the system default: a patch naming that same device
        // explicitly must not open a duplicate aux stream.
        assert!(resolves_to_main_device("speakers", None, Some("speakers")));
        assert!(!resolves_to_main_device("hdmi-out", None, Some("speakers")));
        // Default device unknown — be conservative and open the aux stream.
        assert!(!resolves_to_main_device("hdmi-out", None, None));
    }

    #[test]
    fn configured_preview_selects_an_aux_route() {
        assert_eq!(
            preview_route(Some("headphones"), Some("pa")),
            PreviewRoute::Aux("headphones".to_owned()),
        );
    }

    #[test]
    fn preview_requires_an_explicit_main_device_and_never_selects_pa() {
        // A configured-but-currently-missing device still resolves to an aux
        // attempt; its open error must be returned by play_preview_voice, not
        // redirected to the PA path.
        assert_eq!(
            preview_route(Some("unplugged-headphones"), Some("pa")),
            PreviewRoute::Aux("unplugged-headphones".to_owned()),
        );
        assert_eq!(
            preview_route(Some("pa"), Some("pa")),
            PreviewRoute::SameAsMainDevice,
        );
        assert_eq!(preview_route(None, Some("pa")), PreviewRoute::Disabled);
        assert_eq!(
            preview_route(Some("headphones"), None),
            PreviewRoute::MainOutputNotPinned,
        );
    }

    #[test]
    fn preview_route_does_not_clear_output_patch_device_health() {
        crate::health::set(crate::health::HealthAlert::new(
            "output-patch-device",
            crate::health::HealthLevel::Warning,
            "patch missing",
        ));

        clear_patch_device_health_if_requested(false);
        assert!(crate::health::snapshot()
            .iter()
            .any(|alert| alert.key == "output-patch-device"));

        clear_patch_device_health_if_requested(true);
        assert!(!crate::health::snapshot()
            .iter()
            .any(|alert| alert.key == "output-patch-device"));
    }

    #[test]
    fn program_audio_taps_receive_the_same_post_master_stereo_mix() {
        let samples = Arc::new(vec![0.8_f32, -0.4, 0.6, -0.2]);
        let voice = Voice::new(samples, 2, 48_000, 1.0, 0.0);
        voice.set_playing();
        let pool = rt_pool(vec![Arc::new(voice)]);
        let feeds: Arc<Mutex<Vec<InputFeed>>> = Arc::new(Mutex::new(Vec::new()));
        let format = Arc::new(ProgramAudioFormat::new(48_000));
        let (tap_a, mut receiver_a) = ProgramAudioTap::new(Arc::clone(&format));
        let (tap_b, mut receiver_b) = ProgramAudioTap::new(format);
        let taps = Arc::new(ArcSwap::from_pointee(vec![tap_a, tap_b]));
        let (_, mut cmd_cons) = HeapRb::<AudioCommand>::new(16).split();
        let (mut status_prod, _) = HeapRb::<AudioStatus>::new(16).split();
        let master = Arc::new(std::sync::atomic::AtomicU32::new(f32::to_bits(0.5)));
        let period = Arc::new(std::sync::atomic::AtomicU32::new(2));
        let mut output = vec![0.0_f32; 4];
        let mut program = vec![0.0_f32; 4];

        fill_buffer(
            &mut output,
            &mut program,
            2,
            48_000,
            &pool,
            &feeds,
            &taps,
            &mut cmd_cons,
            &mut status_prod,
            &master,
            &period,
        );

        let mut received_a = Vec::new();
        let mut received_b = Vec::new();
        receiver_a.drain_into(&mut received_a, 4);
        receiver_b.drain_into(&mut received_b, 4);
        // The voice is centre-panned (equal-power pan) and master is 0.5;
        // checking one sample verifies that this is a post-master tap rather
        // than merely another copy of the source voice.
        let expected_left = 0.8 * 0.5 * std::f32::consts::FRAC_1_SQRT_2;
        assert!((output[0] - expected_left).abs() < 0.000_001);
        assert_eq!(received_a, output);
        assert_eq!(received_b, output);
    }

    fn render_file_voice_with_program_tap(
        voice: Voice,
        channels: usize,
        master_gain: f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let frames = voice.total_frames() as usize;
        voice.set_playing();
        let pool = rt_pool(vec![Arc::new(voice)]);
        let feeds: Arc<Mutex<Vec<InputFeed>>> = Arc::new(Mutex::new(Vec::new()));
        let format = Arc::new(ProgramAudioFormat::new(48_000));
        let (tap, mut receiver) = ProgramAudioTap::new(format);
        let taps = Arc::new(ArcSwap::from_pointee(vec![tap]));
        let (_, mut cmd_cons) = HeapRb::<AudioCommand>::new(16).split();
        let (mut status_prod, _) = HeapRb::<AudioStatus>::new(16).split();
        let master = Arc::new(AtomicU32::new(f32::to_bits(master_gain)));
        let period = Arc::new(AtomicU32::new(frames as u32));
        let mut output = vec![0.0_f32; frames * channels];
        let mut program = vec![0.0_f32; frames * 2];

        fill_buffer(
            &mut output,
            &mut program,
            channels,
            48_000,
            &pool,
            &feeds,
            &taps,
            &mut cmd_cons,
            &mut status_prod,
            &master,
            &period,
        );
        let mut received = Vec::new();
        receiver.drain_into(&mut received, frames * 2);
        (output, received)
    }

    #[test]
    fn canonical_program_bus_ignores_asio_pair_output_patch_and_level_matrix() {
        let samples = Arc::new(vec![0.8_f32, -0.4, 0.6, -0.2]);

        let baseline = Voice::new(Arc::clone(&samples), 2, 48_000, 1.0, 0.0);
        let (_, baseline_program) = render_file_voice_with_program_tap(baseline, 8, 1.0);

        // This is the same channel shift apply_default_offset performs for a
        // non-zero ASIO pair, and the same physical pair an Output Patch uses.
        let mut shifted = Voice::new(Arc::clone(&samples), 2, 48_000, 1.0, 0.0);
        shifted.out_l = 4;
        shifted.out_r = 5;
        shifted.patched = true;
        let (shifted_output, shifted_program) = render_file_voice_with_program_tap(shifted, 8, 1.0);
        assert_eq!(shifted_program, baseline_program);
        assert!(shifted_output[..4].iter().all(|sample| *sample == 0.0));
        assert!(shifted_output[4].abs() > 0.0 && shifted_output[5].abs() > 0.0);

        let matrix_voice = Voice::new(samples, 2, 48_000, 1.0, 0.0);
        let mut gains = vec![vec![0.0_f32; 8]; 2];
        gains[0][6] = 1.0;
        gains[1][7] = 1.0;
        unsafe {
            *matrix_voice.inner.level_matrix.get() = crate::engine::voice::LevelMatrix::new(&gains);
        }
        let (matrix_output, matrix_program) =
            render_file_voice_with_program_tap(matrix_voice, 8, 1.0);
        assert_eq!(matrix_program, baseline_program);
        assert!(matrix_output[..6].iter().all(|sample| *sample == 0.0));
        assert!(matrix_output[6].abs() > 0.0 && matrix_output[7].abs() > 0.0);
    }

    #[test]
    fn canonical_program_bus_applies_cue_patch_and_master_gain_once() {
        let voice = Voice::new(Arc::new(vec![1.0_f32, -0.5]), 2, 48_000, 0.8, 0.0);
        voice.inner.set_patch_gain(0.5);
        let (_, program) = render_file_voice_with_program_tap(voice, 2, 0.25);
        let pan = std::f32::consts::FRAC_1_SQRT_2;
        let expected_l = 1.0 * pan * 0.8 * 0.5 * 0.25;
        let expected_r = -0.5 * pan * 0.8 * 0.5 * 0.25;
        assert!((program[0] - expected_l).abs() < 0.000_001);
        assert!((program[1] - expected_r).abs() < 0.000_001);
    }

    #[test]
    fn equal_physical_outputs_fold_file_stereo_once_without_changing_program_bus() {
        let mut voice = Voice::new(Arc::new(vec![1.0_f32, 1.0]), 2, 48_000, 1.0, 0.0);
        voice.out_l = 3;
        voice.out_r = 3;
        let (output, program) = render_file_voice_with_program_tap(voice, 4, 1.0);
        assert!(output[..3].iter().all(|sample| *sample == 0.0));
        assert!((output[3] - 1.0).abs() < 0.000_001);
        assert!((program[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.000_001);
        assert!((program[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.000_001);
    }

    #[test]
    fn program_audio_tap_evicts_oldest_frames_under_backpressure() {
        let format = Arc::new(ProgramAudioFormat::new(48_000));
        let (tap, mut receiver) = ProgramAudioTap::new(format);
        for frame in 0..(NETWORK_AUDIO_TAP_FRAMES as u32 + 3) {
            tap.push_stereo(&[frame as f32, -(frame as f32)], 2);
        }

        let mut received = Vec::with_capacity(6);
        receiver.drain_into(&mut received, 6);
        assert_eq!(received, vec![3.0, -3.0, 4.0, -4.0, 5.0, -5.0]);
        assert_eq!(receiver.dropped_samples(), 6);
    }

    #[test]
    fn program_audio_format_generation_changes_on_restart() {
        let format = ProgramAudioFormat::new(48_000);
        let before = format.snapshot();
        format.update_for_restart(44_100);
        let after = format.snapshot();
        assert_eq!(after.sample_rate, 44_100);
        assert!(after.generation > before.generation);
        assert_eq!(after.generation & 1, 0, "stable snapshots are even");
    }

    #[test]
    fn program_audio_receiver_flushes_stale_transport_backlog() {
        let format = Arc::new(ProgramAudioFormat::new(48_000));
        let (tap, mut receiver) = ProgramAudioTap::new(format);
        tap.push_stereo(&[1.0, -1.0, 2.0, -2.0], 2);
        assert_eq!(receiver.discard_queued(), 4);

        let mut drained = Vec::with_capacity(4);
        receiver.drain_into(&mut drained, 4);
        assert!(drained.is_empty());

        tap.push_stereo(&[3.0, -3.0], 2);
        receiver.drain_into(&mut drained, 4);
        assert_eq!(drained, vec![3.0, -3.0]);
    }

    #[test]
    fn stop_all_command_silences_every_voice() {
        let v1 = make_voice(1000, 2, 48000, 1.0);
        let v2 = make_voice(1000, 2, 48000, 1.0);
        let pool = rt_pool(vec![Arc::clone(&v1), Arc::clone(&v2)]);
        let feeds: Arc<Mutex<Vec<InputFeed>>> = Arc::new(Mutex::new(Vec::new()));
        let (mut cmd_prod, mut cmd_cons) = HeapRb::<AudioCommand>::new(16).split();
        let (mut status_prod, _) = HeapRb::<AudioStatus>::new(16).split();
        let master = Arc::new(std::sync::atomic::AtomicU32::new(f32::to_bits(1.0)));
        let period = Arc::new(std::sync::atomic::AtomicU32::new(64));
        cmd_prod.try_push(AudioCommand::StopAll).unwrap();

        let mut output = vec![0.0f32; 64 * 2];
        let mut program = vec![0.0f32; 64 * 2];
        let taps = empty_program_audio_taps();
        fill_buffer(
            &mut output,
            &mut program,
            2,
            48000,
            &pool,
            &feeds,
            &taps,
            &mut cmd_cons,
            &mut status_prod,
            &master,
            &period,
        );

        for v in [&v1, &v2] {
            assert_eq!(v.voice_state(), VoiceState::Stopped);
        }
    }

    #[test]
    fn sr_ratio_44100_on_48000() {
        // 1 s of 48 kHz output = 48 000 frames → should consume 44 100 source frames.
        let voice = make_voice(220_500, 2, 44_100, 1.0);
        let pos = run_fill(voice, 48_000, 48_000);
        assert_frame_pos(pos, 44_100, "44.1 kHz file on 48 kHz output");
    }

    #[test]
    fn sr_ratio_48000_on_48000() {
        // Same SR: 1 output frame = 1 source frame exactly.
        let voice = make_voice(96_000, 2, 48_000, 1.0);
        let pos = run_fill(voice, 48_000, 48_000);
        assert_frame_pos(pos, 48_000, "48 kHz file on 48 kHz output");
    }

    #[test]
    fn sr_ratio_48000_on_44100() {
        // 1 s of 44.1 kHz output = 44 100 frames → should consume 48 000 source frames.
        let voice = make_voice(96_000, 2, 48_000, 1.0);
        let pos = run_fill(voice, 44_100, 44_100);
        assert_frame_pos(pos, 48_000, "48 kHz file on 44.1 kHz output");
    }

    #[test]
    fn user_rate_2x_on_matching_sr() {
        // rate=2.0 on matching SR: 2 source frames per output frame.
        let voice = make_voice(96_000, 2, 48_000, 2.0);
        let pos = run_fill(voice, 48_000, 48_000);
        assert_frame_pos(pos, 96_000, "rate=2.0 should consume 96 000 frames in 1 s");
    }

    #[test]
    fn sr_ratio_96000_on_48000() {
        // 96 kHz file on 48 kHz output: rate step = 2.0, correct duration.
        // Note: no anti-aliasing filter — content above 24 kHz may alias, but
        // in practice 96 kHz files are already band-limited below 20 kHz.
        let voice = make_voice(192_000, 2, 96_000, 1.0);
        let pos = run_fill(voice, 48_000, 48_000);
        assert_frame_pos(
            pos,
            96_000,
            "96 kHz file on 48 kHz output: 1 s = 96 000 source frames",
        );
    }

    // ── Live input feed / resampler ────────────────────────────────────────

    /// Build a test feed (no real device) plus its ring producer.
    fn make_feed(in_channels: usize) -> (InputFeed, ringbuf::HeapProd<f32>) {
        let (prod, cons) = HeapRb::<f32>::new(STAGING_FRAMES * in_channels).split();
        let feed = InputFeed {
            id: Uuid::new_v4(),
            device_id: "test".into(),
            in_channels,
            sample_rate: 48_000,
            cons,
            staging: vec![0.0_f32; STAGING_FRAMES * in_channels].into_boxed_slice(),
            write_frame: 0,
            valid_from_frame: 0,
            network_jitter_frames: None,
            flush_requested: None,
            _capture: None,
        };
        (feed, prod)
    }

    fn make_synthetic_feed(
        in_channels: usize,
        sample_rate: u32,
    ) -> (InputFeed, SyntheticFeedProducer) {
        let capacity = synthetic_live_ring_frames(sample_rate) * in_channels;
        let (prod, cons) = HeapRb::<f32>::new(capacity).split();
        let flush_requested = Arc::new(AtomicBool::new(false));
        let dropped_samples = Arc::new(AtomicU64::new(0));
        let feed = InputFeed {
            id: Uuid::new_v4(),
            device_id: "synthetic:test".into(),
            in_channels,
            sample_rate,
            cons,
            staging: vec![0.0_f32; STAGING_FRAMES * in_channels].into_boxed_slice(),
            write_frame: 0,
            valid_from_frame: 0,
            network_jitter_frames: None,
            flush_requested: Some(Arc::clone(&flush_requested)),
            _capture: None,
        };
        (
            feed,
            SyntheticFeedProducer::attached(prod, flush_requested, dropped_samples),
        )
    }

    fn make_network_feed(
        in_channels: usize,
        sample_rate: u32,
    ) -> (InputFeed, SyntheticFeedProducer) {
        let (mut feed, producer) = make_synthetic_feed(in_channels, sample_rate);
        feed.network_jitter_frames = Some(
            (sample_rate.max(1) as usize)
                .saturating_mul(NETWORK_LIVE_JITTER_MS as usize)
                .div_ceil(1000)
                .clamp(2, STAGING_FRAMES.saturating_sub(2)),
        );
        (feed, producer)
    }

    #[test]
    fn feed_drain_advances_write_and_interpolates() {
        let (mut feed, mut prod) = make_feed(1);
        for i in 0..4 {
            let _ = prod.try_push(i as f32); // ramp 0,1,2,3
        }
        feed.drain();
        assert_eq!(feed.write_frame, 4);
        assert_eq!(feed.sample(0, 0.0), 0.0);
        assert_eq!(feed.sample(0, 2.0), 2.0);
        assert!(
            (feed.sample(0, 1.5) - 1.5).abs() < 1e-6,
            "linear interp between frames"
        );
    }

    #[test]
    fn synthetic_feed_overflow_flushes_stale_backlog_to_live_edge() {
        let sample_rate = 48_000;
        let (mut feed, mut producer) = make_synthetic_feed(2, sample_rate);
        let max_frames = synthetic_live_ring_frames(sample_rate);
        assert!(max_frames as u64 * 1000 / sample_rate as u64 <= SYNTHETIC_LIVE_RING_MAX_MS as u64);
        assert_eq!(producer.capacity().get(), max_frames * 2);

        for _ in 0..max_frames {
            producer.try_push_frame(&[0.25, -0.25]).unwrap();
        }
        assert_eq!(producer.occupied_len(), max_frames * 2);
        assert_eq!(producer.try_push_frame(&[1.0, -1.0]), Err(2));
        assert_eq!(producer.dropped_samples(), 2);

        // The callback owns the consumer. On the next block it rejects every
        // queued old sample and moves existing LiveSource cursors beyond the
        // stale staging generation, rather than replaying up to 100 ms late.
        feed.drain();
        assert_eq!(feed.cons.occupied_len(), 0);
        assert!(feed.write_frame >= STAGING_FRAMES as u64);
    }

    #[test]
    fn synthetic_stereo_overflow_flushes_an_odd_edge_without_skewing_the_next_frame() {
        let (mut feed, mut producer) = make_synthetic_feed(2, 48_000);
        let capacity = producer.capacity().get();

        // Model the one-sample edge left by an interrupted legacy producer.
        // A new stereo frame cannot fit, must request a flush, and must not
        // publish only its left channel.
        for _ in 0..capacity - 1 {
            producer.try_push(0.25).unwrap();
        }
        assert_eq!(producer.vacant_len(), 1);
        assert_eq!(producer.try_push_frame(&[0.75, -0.75]), Err(2));
        assert_eq!(producer.occupied_len(), capacity - 1);

        feed.drain();
        assert_eq!(feed.cons.occupied_len(), 0);
        assert_eq!(feed.write_frame, STAGING_FRAMES as u64);

        producer.try_push_frame(&[0.75, -0.75]).unwrap();
        feed.drain();
        let frame = ((feed.write_frame - 1) as usize % STAGING_FRAMES) * 2;
        assert_eq!(&feed.staging[frame..frame + 2], &[0.75, -0.75]);
    }

    #[test]
    fn network_feed_prebuffers_bursts_and_limits_clock_correction() {
        let (mut feed, mut producer) = make_network_feed(2, 48_000);
        let jitter_frames = feed.network_jitter_frames.unwrap();
        let first_burst = jitter_frames / 2;
        for _ in 0..first_burst {
            producer.try_push_frame(&[0.5, -0.25]).unwrap();
        }
        feed.drain();

        let live = LiveSource::new(feed.id, 0, 1, 48_000);
        let voice = Arc::new(Voice::new_live(live, 48_000, 1.0, 0.0));
        let (mut status_prod, _) = HeapRb::<AudioStatus>::new(64).split();
        let period = Arc::new(AtomicU32::new(256));
        let mut output = vec![0.0_f32; 256 * 2];
        let (mut peak_l, mut peak_r) = (0.0, 0.0);

        mix_live(
            &mut output,
            None,
            2,
            48_000,
            &voice,
            std::slice::from_ref(&feed),
            &mut status_prod,
            &mut peak_l,
            &mut peak_r,
            &period,
        );
        assert!(output.iter().all(|sample| *sample == 0.0));
        assert!(!voice.live.as_ref().unwrap().is_started());

        for _ in first_burst..jitter_frames {
            producer.try_push_frame(&[0.5, -0.25]).unwrap();
        }
        feed.drain();
        output.fill(0.0);
        mix_live(
            &mut output,
            None,
            2,
            48_000,
            &voice,
            std::slice::from_ref(&feed),
            &mut status_prod,
            &mut peak_l,
            &mut peak_r,
            &period,
        );
        assert!(output.iter().any(|sample| sample.abs() > 0.1));

        let before = voice.live.as_ref().unwrap().read_frame();
        output.fill(0.0);
        mix_live(
            &mut output,
            None,
            2,
            48_000,
            &voice,
            std::slice::from_ref(&feed),
            &mut status_prod,
            &mut peak_l,
            &mut peak_r,
            &period,
        );
        let advanced = voice.live.as_ref().unwrap().read_frame() - before;
        assert!(
            (255.7..=256.1).contains(&advanced),
            "network clock correction advanced {advanced} frames"
        );
    }

    #[test]
    fn network_overflow_never_replays_stale_staging_samples() {
        let (mut feed, mut producer) = make_network_feed(2, 48_000);
        let jitter_frames = feed.network_jitter_frames.unwrap();
        for _ in 0..jitter_frames {
            producer.try_push_frame(&[0.25, 0.25]).unwrap();
        }
        feed.drain();

        let live = LiveSource::new(feed.id, 0, 1, 48_000);
        let voice = Arc::new(Voice::new_live(live, 48_000, 1.0, 0.0));
        let (mut status_prod, _) = HeapRb::<AudioStatus>::new(64).split();
        let period = Arc::new(AtomicU32::new(256));
        let mut output = vec![0.0_f32; 256 * 2];
        let (mut peak_l, mut peak_r) = (0.0, 0.0);
        mix_live(
            &mut output,
            None,
            2,
            48_000,
            &voice,
            std::slice::from_ref(&feed),
            &mut status_prod,
            &mut peak_l,
            &mut peak_r,
            &period,
        );
        assert!(output.iter().any(|sample| sample.abs() > 0.1));

        let capacity_frames = producer.capacity().get() / 2;
        for _ in 0..capacity_frames {
            producer.try_push_frame(&[0.25, 0.25]).unwrap();
        }
        assert_eq!(producer.try_push_frame(&[0.25, 0.25]), Err(2));
        feed.drain();

        output.fill(0.0);
        mix_live(
            &mut output,
            None,
            2,
            48_000,
            &voice,
            std::slice::from_ref(&feed),
            &mut status_prod,
            &mut peak_l,
            &mut peak_r,
            &period,
        );
        assert!(
            output.iter().all(|sample| *sample == 0.0),
            "invalidated staging history leaked after overflow"
        );

        for _ in 0..jitter_frames {
            producer.try_push_frame(&[0.75, 0.75]).unwrap();
        }
        feed.drain();
        output.fill(0.0);
        mix_live(
            &mut output,
            None,
            2,
            48_000,
            &voice,
            std::slice::from_ref(&feed),
            &mut status_prod,
            &mut peak_l,
            &mut peak_r,
            &period,
        );
        assert!(output.iter().all(|sample| *sample > 0.5));
    }

    #[test]
    fn steady_state_gc_does_not_wait_for_the_active_feed_lock() {
        let engine = AudioEngine::new_silent(&MachineAudioConfig::default());
        let (feed_id, _producer) = engine.register_network_feed(2, 48_000).unwrap();
        engine
            .play_mic_voice(feed_id, 0, 1, 0, 1, 1.0, 0.0, 0, FadeCurve::Linear)
            .unwrap();

        let guard = engine.input_feeds.lock().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker_engine = Arc::clone(&engine);
        let worker = std::thread::spawn(move || {
            worker_engine.gc_voices();
            let _ = done_tx.send(());
        });
        let completed_without_feed_lock = done_rx
            .recv_timeout(std::time::Duration::from_millis(250))
            .is_ok();
        drop(guard);
        worker.join().unwrap();
        assert!(completed_without_feed_lock);
    }

    #[test]
    fn mix_live_unity_ratio_routes_input() {
        let (mut feed, mut prod) = make_feed(2);
        for _ in 0..2000 {
            let _ = prod.try_push(0.5); // L
            let _ = prod.try_push(0.25); // R
        }
        feed.drain();
        let feeds = vec![feed];

        let live = LiveSource::new(feeds[0].id, 0, 1, 48_000);
        let voice = Arc::new(Voice::new_live(live, 48_000, 1.0, 0.0)); // unity gain, center pan
        let (mut status_prod, _) = HeapRb::<AudioStatus>::new(64).split();
        let mut out = vec![0.0_f32; 256 * 2];
        let (mut pl, mut pr) = (0.0_f32, 0.0_f32);
        let period = Arc::new(std::sync::atomic::AtomicU32::new(256));

        mix_live(
            &mut out,
            None,
            2,
            48_000,
            &voice,
            &feeds,
            &mut status_prod,
            &mut pl,
            &mut pr,
            &period,
        );

        // Center pan → both gains = sqrt(0.5) ≈ 0.707; L = 0.5·0.707 ≈ 0.354.
        assert!(out[0] > 0.34 && out[0] < 0.37, "L sample was {}", out[0]);
        assert!(out[1] > 0.16 && out[1] < 0.19, "R sample was {}", out[1]);

        // in_sr == out_sr, target_lag = 3 × period = 768, read advances 256.
        let read = voice.live.as_ref().unwrap().read_frame();
        let target_lag = 3.0 * 256.0_f64;
        let expected = (feeds[0].write_frame as f64 - target_lag) + 256.0;
        assert!(
            (read - expected).abs() < 5.0,
            "read {read} vs expected {expected}"
        );
    }

    #[test]
    fn live_program_tap_does_not_advance_twice_and_equal_outputs_fold_once() {
        let (mut feed, mut producer) = make_feed(2);
        for _ in 0..2000 {
            producer.try_push(1.0).unwrap();
            producer.try_push(1.0).unwrap();
        }
        feed.drain();

        let live = LiveSource::new(feed.id, 0, 1, 48_000);
        live.set_read_frame(100.0);
        live.mark_started();
        let mut live_voice = Voice::new_live(live, 48_000, 1.0, 0.0);
        live_voice.out_l = 1;
        live_voice.out_r = 1;
        live_voice.set_playing();
        let voice = Arc::new(live_voice);
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        let feeds = Arc::new(Mutex::new(vec![feed]));
        let format = Arc::new(ProgramAudioFormat::new(48_000));
        let (tap, mut receiver) = ProgramAudioTap::new(format);
        let taps = Arc::new(ArcSwap::from_pointee(vec![tap]));
        let (_, mut cmd_cons) = HeapRb::<AudioCommand>::new(16).split();
        let (mut status_prod, _) = HeapRb::<AudioStatus>::new(32).split();
        let master = Arc::new(AtomicU32::new(f32::to_bits(1.0)));
        let period = Arc::new(AtomicU32::new(16));
        let mut output = vec![0.0_f32; 16 * 2];
        let mut program = vec![0.0_f32; 16 * 2];

        fill_buffer(
            &mut output,
            &mut program,
            2,
            48_000,
            &pool,
            &feeds,
            &taps,
            &mut cmd_cons,
            &mut status_prod,
            &master,
            &period,
        );

        let advanced = voice.live.as_ref().unwrap().read_frame() - 100.0;
        assert!(
            (16.0..17.0).contains(&advanced),
            "live cursor advanced {advanced} frames"
        );
        for frame in output.chunks_exact(2) {
            assert_eq!(frame[0], 0.0);
            assert!((frame[1] - 1.0).abs() < 0.000_001);
        }
        let mut received = Vec::new();
        receiver.drain_into(&mut received, program.len());
        assert_eq!(received, program);
        for frame in received.chunks_exact(2) {
            assert!((frame[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.000_001);
            assert!((frame[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.000_001);
        }
    }

    // ── End-of-file / loop / simultaneous / seek (RT completion logic) ──────

    /// A voice whose PCM is a steady 0.5 tone (non-silent) for contrast tests.
    fn make_tone_voice(n_frames: usize, sr: u32) -> Arc<Voice> {
        let samples = Arc::new(vec![0.5f32; n_frames * 2]);
        let v = Voice::new(samples, 2, sr, 1.0, 0.0);
        v.set_playing();
        Arc::new(v)
    }

    /// Run one 256-frame callback over `pool`; return the (drained) statuses.
    fn run_block(
        pool: &Arc<ArcSwap<Vec<Arc<Voice>>>>,
        cmd: Option<AudioCommand>,
    ) -> Vec<AudioStatus> {
        run_block_with_output(pool, cmd).1
    }

    fn run_block_with_output(
        pool: &Arc<ArcSwap<Vec<Arc<Voice>>>>,
        cmd: Option<AudioCommand>,
    ) -> (Vec<f32>, Vec<AudioStatus>) {
        use ringbuf::traits::Consumer;
        let feeds: Arc<Mutex<Vec<InputFeed>>> = Arc::new(Mutex::new(Vec::new()));
        let (mut cmd_prod, mut cmd_cons) = HeapRb::<AudioCommand>::new(16).split();
        if let Some(c) = cmd {
            cmd_prod.try_push(c).unwrap();
        }
        let (mut status_prod, mut status_cons) = HeapRb::<AudioStatus>::new(64).split();
        let master = Arc::new(std::sync::atomic::AtomicU32::new(f32::to_bits(1.0)));
        let period = Arc::new(std::sync::atomic::AtomicU32::new(256));
        let mut out = vec![0.0f32; 256 * 2];
        let mut program = vec![0.0f32; 256 * 2];
        let taps = empty_program_audio_taps();
        fill_buffer(
            &mut out,
            &mut program,
            2,
            48_000,
            pool,
            &feeds,
            &taps,
            &mut cmd_cons,
            &mut status_prod,
            &master,
            &period,
        );
        let mut statuses = Vec::new();
        while let Some(s) = status_cons.try_pop() {
            statuses.push(s);
        }
        (out, statuses)
    }

    fn make_stream_voice(
        frames: &[[f32; 2]],
        ready: bool,
        eof: bool,
    ) -> (Arc<Voice>, Arc<StreamingAudioSource>) {
        let source = StreamingAudioSource::test_from_pcm(frames, ready, eof);
        let voice = Arc::new(Voice::new_stream(Arc::clone(&source), 1.0, 0.0));
        voice.set_playing();
        (voice, source)
    }

    fn completed_count(statuses: &[AudioStatus], voice_id: VoiceId) -> usize {
        statuses
            .iter()
            .filter(|status| {
                matches!(status, AudioStatus::Completed { voice_id: id } if *id == voice_id)
            })
            .count()
    }

    #[test]
    fn streaming_eof_initialized_cursor_emits_tail_and_completes_once() {
        let frames = [[0.10, -0.10], [0.20, -0.20], [0.90, -0.90]];
        let (voice, source) = make_stream_voice(&frames, true, true);
        let id = voice.id;
        let (output, statuses) = run_block_with_output(&rt_pool(vec![Arc::clone(&voice)]), None);

        assert_eq!(voice.voice_state(), VoiceState::Stopped);
        assert_eq!(completed_count(&statuses, id), 1);
        assert_eq!(source.underruns(), 0, "terminal EOF is not starvation");
        let diagnostics = source.diagnostics();
        assert!(diagnostics.eof);
        assert_eq!(diagnostics.buffered_frames, 0);
        assert!(!diagnostics.refill_requested);
        assert!(!diagnostics.job_requested);
        assert!(!diagnostics.job_running);
        assert!(output.chunks_exact(2).any(|frame| {
            (frame[0] - frames[2][0]).abs() < 1e-6
                && (frame[1] - frames[2][1]).abs() < 1e-6
        }), "the final PCM frame must reach the output");
    }

    #[test]
    fn streaming_eof_with_ring_tail_waits_until_tail_is_consumed() {
        let frames = vec![[0.25, -0.25]; 300];
        let (voice, source) = make_stream_voice(&frames, true, true);
        let id = voice.id;
        let pool = rt_pool(vec![Arc::clone(&voice)]);

        let first = run_block(&pool, None);
        assert_eq!(completed_count(&first, id), 0);
        assert_eq!(voice.voice_state(), VoiceState::Playing);
        assert!(source.buffered_samples() > 0);

        let second = run_block(&pool, None);
        assert_eq!(completed_count(&second, id), 1);
        assert_eq!(voice.voice_state(), VoiceState::Stopped);
        assert_eq!(source.underruns(), 0);
    }

    #[test]
    fn streaming_empty_non_eof_ring_is_a_real_underrun() {
        let (voice, source) = make_stream_voice(&[], true, false);
        let id = voice.id;
        let statuses = run_block(&rt_pool(vec![Arc::clone(&voice)]), None);

        assert_eq!(voice.voice_state(), VoiceState::Playing);
        assert_eq!(completed_count(&statuses, id), 0);
        assert!(source.underruns() > 0);
    }

    #[test]
    fn streaming_seek_reset_does_not_emit_stale_completed() {
        let frames = vec![[0.5, -0.5]; 300];
        let (voice, source) = make_stream_voice(&frames, true, true);
        let id = voice.id;
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        let _ = run_block(&pool, None);

        source.request_seek(0);
        let statuses = run_block(
            &pool,
            Some(AudioCommand::Seek {
                voice_id: id,
                frame_pos: 0,
            }),
        );
        assert_eq!(completed_count(&statuses, id), 0);
        assert_eq!(voice.voice_state(), VoiceState::Playing);
        assert!(!source.is_eof());
    }

    #[test]
    fn streaming_repeated_callbacks_after_completion_are_quiet_and_idempotent() {
        let frames = [[0.75, -0.75]];
        let (voice, source) = make_stream_voice(&frames, true, true);
        let id = voice.id;
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        let first = run_block(&pool, None);
        let underruns = source.underruns();
        let second = run_block(&pool, None);
        let third = run_block(&pool, None);

        assert_eq!(completed_count(&first, id), 1);
        assert_eq!(completed_count(&second, id), 0);
        assert_eq!(completed_count(&third, id), 0);
        assert_eq!(source.underruns(), underruns);
        assert_eq!(voice.voice_state(), VoiceState::Stopped);
    }

    #[test]
    fn eof_stops_voice_and_emits_completed() {
        // 100 source frames, 256-frame block at matched SR → runs past the end.
        let voice = make_voice(100, 2, 48_000, 1.0);
        let id = voice.id;
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        let statuses = run_block(&pool, None);

        assert_eq!(
            voice.voice_state(),
            VoiceState::Stopped,
            "voice must stop at EOF"
        );
        assert!(
            statuses
                .iter()
                .any(|s| matches!(s, AudioStatus::Completed { voice_id } if *voice_id == id)),
            "EOF must emit exactly one Completed for the voice"
        );
    }

    /// Install a slice program on a not-yet-running voice (test-side stand-in
    /// for the AudioCue GO path).
    fn set_slices(voice: &Arc<Voice>, segs: &[(u64, u64, u32)]) {
        use crate::engine::voice::{SliceProgram, SliceSegment};
        let program = SliceProgram::new(
            segs.iter()
                .map(|&(s, e, c)| SliceSegment {
                    start_frame: s,
                    end_frame: e,
                    play_count: c,
                })
                .collect(),
        )
        .expect("non-empty slice program");
        // SAFETY: the voice has not been submitted to the RT pool yet.
        unsafe { *voice.inner.slices.get() = Some(program) };
        voice
            .frame_pos
            .store(segs[0].0, std::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn sliced_voice_vamps_on_infinite_segment() {
        // 300 frames, slices: [0,100)×1 → [100,200)×∞ → [200,300)×1.
        let voice = make_voice(300, 2, 48_000, 1.0);
        set_slices(&voice, &[(0, 100, 1), (100, 200, u32::MAX), (200, 300, 1)]);
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        let statuses = run_block(&pool, None);

        assert_eq!(
            voice.voice_state(),
            VoiceState::Playing,
            "vamp must keep playing"
        );
        assert!(
            !statuses
                .iter()
                .any(|s| matches!(s, AudioStatus::Completed { .. })),
            "vamping must not complete"
        );
        let pos = voice.frame_pos.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            (100..200).contains(&pos),
            "position must stay inside the vamp, got {pos}"
        );
    }

    #[test]
    fn devamp_continue_releases_the_vamp_and_plays_through() {
        let voice = make_voice(300, 2, 48_000, 1.0);
        set_slices(&voice, &[(100, 200, u32::MAX), (200, 300, 1)]);
        let id = voice.id;
        let pool = rt_pool(vec![Arc::clone(&voice)]);

        // Block 1: establish the vamp.
        run_block(&pool, None);
        assert_eq!(voice.voice_state(), VoiceState::Playing);

        // Block 2: devamp → finish the pass, play the last slice, complete.
        let statuses = run_block(
            &pool,
            Some(AudioCommand::Devamp {
                voice_id: id,
                stop_at_end: false,
            }),
        );
        assert_eq!(
            voice.voice_state(),
            VoiceState::Stopped,
            "must play through to the end"
        );
        assert!(
            statuses
                .iter()
                .any(|s| matches!(s, AudioStatus::Completed { voice_id } if *voice_id == id)),
            "devamped voice must complete at file end"
        );
    }

    #[test]
    fn devamp_stop_ends_at_the_slice_boundary() {
        let voice = make_voice(300, 2, 48_000, 1.0);
        set_slices(&voice, &[(100, 200, u32::MAX), (200, 300, 1)]);
        let id = voice.id;
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        run_block(&pool, None);

        let statuses = run_block(
            &pool,
            Some(AudioCommand::Devamp {
                voice_id: id,
                stop_at_end: true,
            }),
        );
        assert_eq!(
            voice.voice_state(),
            VoiceState::Stopped,
            "stop-at-end must stop the voice"
        );
        assert!(
            statuses
                .iter()
                .any(|s| matches!(s, AudioStatus::Completed { voice_id } if *voice_id == id)),
            "stop-at-end must emit Completed"
        );
        let pos = voice.frame_pos.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            pos <= 200,
            "must not play past the slice boundary, got {pos}"
        );
    }

    #[test]
    fn sliced_finite_counts_replay_then_advance() {
        // [0,100)×2 → [100,200)×1: one 256-frame block plays 100+100 frames of
        // segment 0, then enters segment 1 (~56 frames in).
        let voice = make_voice(300, 2, 48_000, 1.0);
        set_slices(&voice, &[(0, 100, 2), (100, 200, 1)]);
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        let statuses = run_block(&pool, None);

        assert_eq!(voice.voice_state(), VoiceState::Playing);
        assert!(!statuses
            .iter()
            .any(|s| matches!(s, AudioStatus::Completed { .. })));
        let pos = voice.frame_pos.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            (100..200).contains(&pos),
            "expected ~156 inside segment 1, got {pos}"
        );
    }

    #[test]
    fn finite_loop_wraps_and_decrements_without_stopping() {
        // 200 frames, loop once (loops_remaining=1); 256-frame block wraps once
        // (dec to 0) then keeps playing from the top — must NOT stop yet.
        let voice = make_voice(200, 2, 48_000, 1.0);
        voice
            .inner
            .loops_remaining
            .store(1, std::sync::atomic::Ordering::Relaxed);
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        let statuses = run_block(&pool, None);

        assert_eq!(
            voice
                .inner
                .loops_remaining
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "one wrap must decrement the finite loop counter"
        );
        assert_eq!(
            voice.voice_state(),
            VoiceState::Playing,
            "still playing mid-loop"
        );
        assert!(
            !statuses
                .iter()
                .any(|s| matches!(s, AudioStatus::Completed { .. })),
            "a wrap must not emit Completed"
        );
        let pos = voice.frame_pos.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            pos < 100,
            "after wrapping 200-frame file within 256 frames, pos≈56, got {pos}"
        );
    }

    #[test]
    fn infinite_loop_never_completes() {
        // loops_remaining = u32::MAX; play 4000 frames (20× past a 200-frame file).
        let voice = make_voice(200, 2, 48_000, 1.0);
        voice
            .inner
            .loops_remaining
            .store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        for _ in 0..16 {
            let statuses = run_block(&pool, None);
            assert!(
                !statuses
                    .iter()
                    .any(|s| matches!(s, AudioStatus::Completed { .. })),
                "infinite loop must never emit Completed"
            );
        }
        assert_eq!(
            voice
                .inner
                .loops_remaining
                .load(std::sync::atomic::Ordering::Relaxed),
            u32::MAX,
            "infinite loop counter must not decrement"
        );
        assert_eq!(voice.voice_state(), VoiceState::Playing);
    }

    #[test]
    fn simultaneous_voices_all_advance() {
        let v1 = make_voice(100_000, 2, 48_000, 1.0);
        let v2 = make_voice(100_000, 2, 48_000, 1.0);
        let pool = rt_pool(vec![Arc::clone(&v1), Arc::clone(&v2)]);
        run_block(&pool, None);
        for v in [&v1, &v2] {
            let pos = v.frame_pos.load(std::sync::atomic::Ordering::Relaxed);
            assert!(
                (pos as i64 - 256).abs() <= 1,
                "each simultaneous voice advances ~256, got {pos}"
            );
        }
    }

    #[test]
    fn seek_command_repositions_before_mixing() {
        // Seek is applied before the voice loop, so pos = seek_target + block.
        let voice = make_voice(100_000, 2, 48_000, 1.0);
        let id = voice.id;
        let pool = rt_pool(vec![Arc::clone(&voice)]);
        run_block(
            &pool,
            Some(AudioCommand::Seek {
                voice_id: id,
                frame_pos: 5_000,
            }),
        );
        let pos = voice.frame_pos.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            (pos as i64 - 5_256).abs() <= 1,
            "seek(5000)+256 frames → ~5256, got {pos}"
        );
    }

    #[test]
    fn voice_pool_publishes_snapshots_and_bounds_retention() {
        // FINDING A1 (fixed): the RT callback reads a wait-free ArcSwap snapshot
        // instead of try_lock'ing the pool.  A publish is immediately visible to
        // the RT handle, and the keep-alive ring stays bounded so retired
        // generations are dropped on the non-RT side (never the RT thread).
        let pool = VoicePool::new();
        assert!(pool.rt.load().is_empty());

        assert!(pool.push(make_tone_voice(1000, 48_000)).is_ok(), "push");
        assert_eq!(
            pool.rt.load().len(),
            1,
            "a published voice is visible to the RT handle"
        );

        // Several publishes; the retained ring never exceeds its cap.
        for _ in 0..10 {
            assert!(pool.push(make_tone_voice(1000, 48_000)).is_ok(), "push");
        }
        assert!(pool.retained.lock().unwrap().len() <= VOICE_POOL_RETAIN);

        pool.retain(|_| false);
        assert!(
            pool.rt.load().is_empty(),
            "retain(none) republishes an empty snapshot"
        );
    }

    #[test]
    fn callback_mixes_the_published_snapshot_with_no_lock() {
        // Replaces the old A1 proof: there is no longer a lock the callback can be
        // starved on, so a freshly published tone always reaches the output —
        // the "block of silence on contention" failure mode is gone.
        let pool = VoicePool::new();
        assert!(pool.push(make_tone_voice(100_000, 48_000)).is_ok(), "push");

        let feeds: Arc<Mutex<Vec<InputFeed>>> = Arc::new(Mutex::new(Vec::new()));
        let (_p, mut cmd_cons) = HeapRb::<AudioCommand>::new(16).split();
        let (mut status_prod, _sc) = HeapRb::<AudioStatus>::new(16).split();
        let master = Arc::new(std::sync::atomic::AtomicU32::new(f32::to_bits(1.0)));
        let period = Arc::new(std::sync::atomic::AtomicU32::new(256));
        let mut out = vec![0.0f32; 256 * 2];
        let mut program = vec![0.0f32; 256 * 2];
        let taps = empty_program_audio_taps();
        fill_buffer(
            &mut out,
            &mut program,
            2,
            48_000,
            &pool.rt,
            &feeds,
            &taps,
            &mut cmd_cons,
            &mut status_prod,
            &master,
            &period,
        );
        assert!(
            out.iter().any(|s| s.abs() > 0.1),
            "the published tone must reach the output"
        );
    }

    // ── Silent mode (no audio device at all) ─────────────────────────────────

    #[test]
    fn silent_engine_reports_a_failed_stream() {
        let engine = AudioEngine::new_silent(&MachineAudioConfig::default());

        let health = engine.audio_health();
        assert!(
            health.failed,
            "silent mode must report the fault immediately"
        );
        assert!(!health.in_fallback, "there was no device to fall back from");
        assert_eq!(
            engine.callback_count(),
            0,
            "no callback can fire without a stream"
        );
    }

    #[test]
    fn silent_engine_keeps_a_usable_format() {
        // The rest of the app (decode, resampling, patch routing) reads these;
        // they must be sane defaults rather than zeroes.
        let engine = AudioEngine::new_silent(&MachineAudioConfig::default());
        assert_eq!(engine.sample_rate(), 48_000);
        assert_eq!(engine.output_channels(), 2);
    }

    #[test]
    fn main_test_voice_is_marked_preview_only() {
        let engine = AudioEngine::new_silent(&MachineAudioConfig::default());
        let voice = Voice::new(Arc::new(vec![0.0; 8]), 2, 48_000, 1.0, 0.0);
        let id = engine.play_main_test_voice(voice).expect("test voice");
        let isolated = engine.voices.with(|voices| {
            voices
                .iter()
                .find(|voice| voice.id == id)
                .map(|voice| voice.preview_only && voice.patched)
        }).flatten();
        assert_eq!(isolated, Some(true));
    }

    #[test]
    fn silent_engine_preserves_the_operator_device_choice() {
        // `desired_config` drives the watchdog banner and the manual restore —
        // starting silent must not silently rewrite the operator's selection.
        let config = MachineAudioConfig {
            device_id: Some("Focusrite Scarlett".into()),
            ..MachineAudioConfig::default()
        };
        let engine = AudioEngine::new_silent(&config);
        assert_eq!(
            engine.audio_health().desired_device.as_deref(),
            Some("Focusrite Scarlett"),
        );
    }

    #[test]
    fn stopping_preview_removes_its_voice_even_without_an_audio_device() {
        let engine = AudioEngine::new_silent(&MachineAudioConfig::default());
        let voice = Voice::new(Arc::new(vec![0.0; 96]), 2, 48_000, 1.0, 0.0);
        let id = voice.id;
        let voice = Arc::new(voice);
        assert!(engine.voices.push(Arc::clone(&voice)).is_ok());
        assert_eq!(engine.voices.with(|voices| voices.len()), Some(1));

        engine.stop_preview_voice(id);

        assert_eq!(voice.voice_state(), VoiceState::Stopped);
        assert_eq!(engine.voices.with(|voices| voices.len()), Some(0));
    }

    #[test]
    fn unavailable_preview_device_never_inserts_a_main_voice() {
        let config = MachineAudioConfig {
            device_id: Some("test-main-pa".to_owned()),
            preview_device_id: Some("__qlisa_missing_preview_device__".to_owned()),
            ..MachineAudioConfig::default()
        };
        let engine = AudioEngine::new_silent(&config);
        let voice = Voice::new(Arc::new(vec![0.0; 96]), 2, 48_000, 1.0, 0.0);

        assert!(engine.play_preview_voice(voice).is_err());
        assert_eq!(engine.voices.with(|voices| voices.len()), Some(0));
    }

    #[test]
    fn unavailable_explicit_aux_never_falls_back_to_main() {
        let config = MachineAudioConfig {
            device_id: Some("test-main-pa".to_owned()),
            ..MachineAudioConfig::default()
        };
        let engine = AudioEngine::new_silent(&config);
        let voice = Voice::new(Arc::new(vec![0.0; 96]), 2, 48_000, 1.0, 0.0);

        let error = engine
            .play_voice_routed(voice, Some("__qlisa_missing_aux_device__"))
            .unwrap_err()
            .to_string();

        assert!(!error.is_empty());
        assert_eq!(engine.voices.with(|voices| voices.len()), Some(0));
        assert_eq!(engine.aux_streams.lock().unwrap().len(), 0);
    }

    #[test]
    fn unavailable_explicit_aux_paused_voice_never_falls_back_to_main() {
        let config = MachineAudioConfig {
            device_id: Some("test-main-pa".to_owned()),
            ..MachineAudioConfig::default()
        };
        let engine = AudioEngine::new_silent(&config);
        let voice = Voice::new(Arc::new(vec![0.0; 96]), 2, 48_000, 1.0, 0.0);

        assert!(engine
            .play_voice_paused_routed(voice, Some("__qlisa_missing_aux_video_device__"))
            .is_err());
        assert_eq!(engine.voices.with(|voices| voices.len()), Some(0));
        assert_eq!(engine.aux_streams.lock().unwrap().len(), 0);
    }

    #[test]
    fn output_patch_test_tone_never_falls_back_to_main() {
        let config = MachineAudioConfig {
            device_id: Some("test-main-pa".to_owned()),
            ..MachineAudioConfig::default()
        };
        let engine = AudioEngine::new_silent(&config);
        let voice = Voice::new(Arc::new(vec![0.0; 96]), 2, 48_000, 1.0, 0.0);

        assert!(engine
            .play_test_output_patch_voice(
                voice,
                "__qlisa_missing_aux_test_device__",
                &[0, 1],
            )
            .is_err());
        assert_eq!(engine.voices.with(|voices| voices.len()), Some(0));
        assert_eq!(engine.aux_streams.lock().unwrap().len(), 0);
    }

    #[test]
    fn system_default_main_rejects_preview_before_any_output_is_opened() {
        let config = MachineAudioConfig {
            preview_device_id: Some("headphones".to_owned()),
            ..MachineAudioConfig::default()
        };
        let engine = AudioEngine::new_silent(&config);
        let voice = Voice::new(Arc::new(vec![0.0; 96]), 2, 48_000, 1.0, 0.0);

        let error = engine.play_preview_voice(voice).unwrap_err().to_string();
        assert!(error.contains("Main output must be an explicit device"));
        assert_eq!(engine.voices.with(|voices| voices.len()), Some(0));
    }

    #[test]
    fn fallback_main_rejects_preview_targeting_the_actual_default_output() {
        let config = MachineAudioConfig {
            device_id: Some("desired-pa-a".to_owned()),
            preview_device_id: Some("fallback-default-b".to_owned()),
            ..MachineAudioConfig::default()
        };
        let engine = AudioEngine::new_silent(&config);
        // Model a failed explicit PA A whose show-safe main route was restarted
        // on system default B. `desired_config` must remain A for recovery, but
        // it cannot authorize a preview on B because B is now potentially PA.
        engine.in_fallback.store(true, Ordering::Relaxed);
        *engine.current_device_id.lock().unwrap() = None;

        let voice = Voice::new(Arc::new(vec![0.0; 96]), 2, 48_000, 1.0, 0.0);
        let error = engine.play_preview_voice(voice).unwrap_err().to_string();

        assert!(error.contains("system-default fallback"));
        assert_eq!(engine.voices.with(|voices| voices.len()), Some(0));
        assert_eq!(engine.aux_streams.lock().unwrap().len(), 0);
    }
}
