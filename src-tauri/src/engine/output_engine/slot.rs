//! Video slot pool — one mpv context per simultaneously-visible visual cue.
//!
//! QLab-style layering: every Video / Image / Camera cue gets its own
//! [`VideoSlot`] (mpv context + event thread + FBO on the render thread), and
//! the compositor in `render.rs` stacks the slot textures in layer order with
//! per-slot opacity and blend mode.  Slots are created lazily up to
//! [`MAX_VIDEO_SLOTS`] and live for the lifetime of their output. During
//! output retirement every slot event owner is stopped and joined before its
//! mpv client is destroyed. When the pool is exhausted the oldest content is
//! stolen (hard stop, `Completed` status so the owning cue resets).
//!

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use crate::engine::mpv_sys::{
    MpvEventEndFile, MpvEventLogMessage, MpvEventProperty, MpvLib, MPV_END_FILE_REASON_EOF,
    MPV_END_FILE_REASON_ERROR, MPV_EVENT_END_FILE, MPV_EVENT_FILE_LOADED, MPV_EVENT_LOG_MESSAGE,
    MPV_EVENT_PLAYBACK_RESTART, MPV_EVENT_PROPERTY_CHANGE, MPV_EVENT_SEEK, MPV_EVENT_SHUTDOWN,
    MPV_EVENT_VIDEO_RECONFIG, MPV_FORMAT_DOUBLE,
};
use crate::engine::network_io::redact_srt_diagnostic;
use crate::engine::AudioEngine;

use super::blend::BlendMode;
use super::types::{LayerStyle, MpvCtx, OutputStatus, VideoGeometry, VoiceId};
use super::{cs, get_prop_i64, opt_str, try_apply_crop};
use crossbeam_channel::Sender;

/// Maximum simultaneously-open video slots (excluding the overlay context).
/// Each active slot is a full decode pipeline; 8 covers any realistic stage.
pub(super) const MAX_VIDEO_SLOTS: usize = 8;

/// Stacking sequence for automatic layering (newest on top) and steal order.

/// Set once a slot has seen libmpv fail to initialise hardware decoding.
/// From then on every slot decodes in software — see [`fall_back_to_software`].
static SOFTWARE_DECODE_ONLY: AtomicBool = AtomicBool::new(false);

/// Operator override for the `hwdec` mode, from the `INKUE_HWDEC` environment
/// variable (any value libmpv accepts: `no`, `auto-copy`, `d3d11va-copy`, …).
///
/// An explicit choice is **pinned**: it also disables the automatic fallback
/// below, so setting a backend on purpose — to reproduce a decoder bug, or to
/// work around one — is not silently undone. Read once, on first use.
static HWDEC_OVERRIDE: OnceLock<Option<String>> = OnceLock::new();

fn hwdec_override() -> Option<&'static str> {
    HWDEC_OVERRIDE
        .get_or_init(|| {
            let mode = std::env::var("INKUE_HWDEC")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
            if let Some(m) = &mode {
                log::info!("[slot] INKUE_HWDEC={m} — hwdec pinned, automatic fallback disabled");
            }
            mode
        })
        .as_deref()
}

/// The `hwdec` mode new slots are created with.
///
/// `auto-copy` reads decoded frames back into system memory, which is the only
/// hwdec family that composites correctly through our own GL render context.
/// Once [`SOFTWARE_DECODE_ONLY`] is latched, slots stop asking for hardware
/// decoding altogether.
fn hwdec_mode() -> &'static str {
    resolve_hwdec_mode(
        hwdec_override(),
        SOFTWARE_DECODE_ONLY.load(Ordering::Relaxed),
    )
}

/// Precedence, as an executable spec: an operator's pin wins over everything,
/// then a latched failure, then the default.
fn resolve_hwdec_mode(pinned: Option<&'static str>, software_only: bool) -> &'static str {
    match (pinned, software_only) {
        (Some(mode), _) => mode,
        (None, true) => "no",
        (None, false) => "auto-copy",
    }
}

/// `true` when an mpv log line reports that hardware decoding could not be set
/// up — the GPU/driver refuses the codec profile, or the hwaccel device cannot
/// be created at all.  mpv words it the same way for every backend (`d3d11` on
/// Windows, `vulkan`/`cuda`/`vaapi` elsewhere), so one pattern covers them all;
/// the reason string is matched too in case the wording of the first half ever
/// changes.
///
/// mpv retries the failure per frame and can hand the renderer partially
/// decoded frames, which is what shows up as a green/torn picture; the cure is
/// to stop asking for hardware decoding (issue #5).
pub(super) fn reports_hwdec_failure(text: &str) -> bool {
    text.contains("Failed setup for format")
        || text.contains("hwaccel initialisation returned error")
}

/// Instance-scoped slot pool.  The notifier is deliberately injected so a
/// slot cannot accidentally wake or report status to another output runtime.
pub(super) struct SlotRegistry {
    slots: RwLock<Vec<Arc<VideoSlot>>>,
    layer_seq: AtomicU64,
    status_tx: Sender<OutputStatus>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl SlotRegistry {
    pub(super) fn new(
        status_tx: Sender<OutputStatus>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self {
            slots: RwLock::new(Vec::new()),
            layer_seq: AtomicU64::new(1),
            status_tx,
            wake,
        })
    }
    pub(super) fn all_slots(&self) -> Vec<Arc<VideoSlot>> {
        self.slots.read().map(|v| v.clone()).unwrap_or_default()
    }
    pub(super) fn slot_for_voice(&self, voice: VoiceId) -> Option<Arc<VideoSlot>> {
        self.all_slots().into_iter().find(|s| {
            s.state
                .lock()
                .map(|st| st.voice_id == Some(voice))
                .unwrap_or(false)
        })
    }
    pub(super) fn acquire(
        self: &Arc<Self>,
        lib: &Arc<MpvLib>,
        audio_engine: &Arc<AudioEngine>,
    ) -> Result<Arc<VideoSlot>> {
        acquire_slot(self, lib, audio_engine)
    }
    fn wake(&self) {
        (self.wake)();
    }
    pub(super) fn next_layer_seq(&self) -> u64 {
        self.layer_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Stop every slot client owned by this output. The caller must already
    /// have closed the output lifecycle gate and joined the GL owner; it also
    /// joins the overlay event owner before calling here, because that owner
    /// can otherwise apply a hwdec fallback to slot clients.
    ///
    /// Request every worker first so no slot waits behind another slot's
    /// teardown. A worker observes the flag immediately after its bounded
    /// `mpv_wait_event` call; `mpv_wakeup` makes the normal path immediate.
    /// No mutex is held while joining, so an event handler that is unwinding a
    /// slot-state lock cannot deadlock shutdown.
    pub(super) fn shutdown_all(&self) {
        let slots = self.all_slots();
        for slot in &slots {
            slot.request_event_shutdown();
        }
        for slot in &slots {
            slot.join_event_thread();
        }
        for slot in &slots {
            slot.destroy_mpv_context();
        }
    }
}

// ---------------------------------------------------------------------------
// Opacity animation
// ---------------------------------------------------------------------------

/// Per-slot opacity animation (fade in/out, EOF fade, Fade Cue).
#[derive(Debug, Clone)]
pub(super) struct OpacityAnim {
    pub current: f32,
    pub start: f32,
    pub target: f32,
    pub duration_ms: u32,
    pub started_at: Instant,
}

impl OpacityAnim {
    pub(super) fn resting(v: f32) -> Self {
        Self {
            current: v,
            start: v,
            target: v,
            duration_ms: 0,
            started_at: Instant::now(),
        }
    }

    pub(super) fn animate_to(&mut self, target: f32, duration_ms: u32) {
        self.start = self.current;
        self.target = target.clamp(0.0, 1.0);
        self.duration_ms = duration_ms;
        self.started_at = Instant::now();
    }

    pub(super) fn set(&mut self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        self.current = v;
        self.start = v;
        self.target = v;
        self.duration_ms = 0;
    }

    /// Advance one frame.  Returns `(current, just_completed)`.
    pub(super) fn tick(&mut self) -> (f32, bool) {
        if (self.current - self.target).abs() < f32::EPSILON {
            return (self.current, false);
        }
        let t = if self.duration_ms == 0 {
            1.0
        } else {
            (self.started_at.elapsed().as_millis() as f32 / self.duration_ms as f32).min(1.0)
        };
        self.current = self.start + (self.target - self.start) * t;
        let done = t >= 1.0;
        if done {
            self.current = self.target;
        }
        (self.current, done)
    }

    pub(super) fn is_animating(&self) -> bool {
        (self.current - self.target).abs() > f32::EPSILON
    }
}

// ---------------------------------------------------------------------------
// SlotState
// ---------------------------------------------------------------------------

/// Everything about the content currently in a slot.  Locked briefly by the
/// engine (GO/stop/live edits), the slot's event thread, and the render
/// thread's per-frame tick — never held across blocking calls.
pub(super) struct SlotState {
    /// The output voice occupying this slot (`None` = idle).
    pub voice_id: Option<VoiceId>,
    /// The AudioEngine voice carrying this content's audio track, if any.
    pub audio_voice_id: Option<VoiceId>,
    /// Cue geometry, kept for the pixel-crop at `VIDEO_RECONFIG`.
    pub geometry: VideoGeometry,
    /// `true` once the crop has been resolved for the current load.
    pub crop_applied: bool,
    /// Compositor sort key: explicit layers order below automatic ones,
    /// sequence breaks ties (newest on top).  See [`resolve_layer_key`].
    pub layer_key: u64,
    pub blend_mode: BlendMode,
    /// The cue's base opacity; the animation multiplies against it.
    pub base_opacity: f32,
    /// Runtime opacity animation (0 → base on reveal, → 0 on stop).
    pub anim: OpacityAnim,
    /// Set while a paused video load waits for its first frame
    /// (`PLAYBACK_RESTART`); carries the fade-in duration.
    pub pending_reveal: Option<u32>,
    /// Failsafe deadline for the reveal (mpv event missing/late).
    pub reveal_deadline: Option<Instant>,
    /// Seek requested immediately after GO, before mpv has loaded the file.
    /// mpv applies a `seek` command issued in this window to the old load (or
    /// drops it), so replay it after `FILE_LOADED`.
    pub pending_seek_ms: Option<u64>,
    /// Action-time seek requested before source duration is known.
    pub pending_seek_action_ms: Option<u64>,
    /// `FILE_LOADED` has completed for this load. A seek that arrives after
    /// this point must go to mpv now; queuing it for an event that already
    /// happened loses the seek and leaves the first frame at zero.
    pub file_loaded: bool,
    /// Source duration captured at `FILE_LOADED`, for a seek immediately
    /// after that event when querying mpv still transiently returns no value.
    pub loaded_duration_ms: Option<u64>,
    /// Keep the slot dark until mpv has actually reached this initial seek.
    /// `PLAYBACK_RESTART` can describe frame zero from the paused load rather
    /// than the seek issued by the Number timeline.
    pub reveal_after_seek_ms: Option<u64>,
    pub source_start_ms: Option<u64>,
    pub source_end_ms: Option<u64>,
    pub loop_count: u32,
    /// Preloaded (Load Cue): the file is open and frame 0 is decoded, but the
    /// reveal is **held back** until the cue is actually started. Both the
    /// `PLAYBACK_RESTART` path and the reveal watchdog respect it, so nothing
    /// reaches the screen in the meantime.
    pub preloaded: bool,
    /// `true` when the fade-out completes and mpv should be stopped + the
    /// slot released.  Drained by the render thread tick.
    pub pending_unload: bool,
    /// Freeze on last frame at EOF (`keep-open=yes` was set for this load).
    pub hold_last_frame: bool,
    /// QLab-style slice plan (`None` = plain playback).  Segments loop via
    /// mpv's ab-loop; the event thread advances `current` when `time-pos`
    /// crosses a segment boundary.
    pub slice_plan: Option<SlicePlan>,
    /// Monotonic per-slot generation guard (a slow event for load N must not
    /// touch load N+1).
    pub generation: u64,
}

/// Slice program for one slot: `(start_s, end_s, play_count)` per segment;
/// `u32::MAX` = vamp.
pub(super) struct SlicePlan {
    pub segments: Vec<(f64, f64, u32)>,
    /// Index of the segment currently playing.
    pub current: usize,
    /// Devamp "stop at end of current slice" armed.
    pub stop_at_end: bool,
}

impl SlotState {
    fn idle() -> Self {
        Self {
            voice_id: None,
            audio_voice_id: None,
            geometry: VideoGeometry::default(),
            crop_applied: true,
            layer_key: 0,
            blend_mode: BlendMode::Normal,
            base_opacity: 1.0,
            anim: OpacityAnim::resting(0.0),
            pending_reveal: None,
            reveal_deadline: None,
            pending_seek_ms: None,
            pending_seek_action_ms: None,
            file_loaded: false,
            loaded_duration_ms: None,
            reveal_after_seek_ms: None,
            source_start_ms: None,
            source_end_ms: None,
            loop_count: 0,
            preloaded: false,
            pending_unload: false,
            hold_last_frame: false,
            slice_plan: None,
            generation: 0,
        }
    }
}

/// Sort key so explicit layers (1–1000) stack below automatic ones, newest
/// automatic content on top, ties broken by GO order.
pub(super) fn resolve_layer_key(layer: Option<u32>, seq: u64) -> u64 {
    match layer {
        // Explicit layer L → band L, ordered by seq inside the band.
        Some(l) => (l.clamp(1, 1000) as u64) << 40 | (seq & 0xFF_FFFF_FFFF),
        // Automatic → above every explicit band.
        None => (1001u64 << 40) | (seq & 0xFF_FFFF_FFFF),
    }
}

// ---------------------------------------------------------------------------
// VideoSlot
// ---------------------------------------------------------------------------

/// Completion notification for one slot event worker. It is intentionally a
/// small, native-free primitive so its wake-up contract can be tested without
/// loading libmpv.
#[derive(Default)]
struct SlotEventExit {
    finished: Mutex<bool>,
    changed: Condvar,
}

impl SlotEventExit {
    fn finish(&self) {
        let mut finished = self
            .finished
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *finished = true;
        self.changed.notify_all();
    }

    fn wait(&self, timeout: Duration) -> bool {
        let finished = self
            .finished
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *finished {
            return true;
        }
        let (finished, _) = self
            .changed
            .wait_timeout_while(finished, timeout, |finished| !*finished)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *finished
    }
}

/// Marks an event worker as finished even if its event loop panics. The
/// subsequent `JoinHandle::join` still records that panic, but teardown can
/// safely reclaim the client because the worker has actually exited.
struct SlotEventExitGuard(Arc<SlotEventExit>);

impl Drop for SlotEventExitGuard {
    fn drop(&mut self) {
        self.0.finish();
    }
}

/// One mpv context of the video pool.
pub(super) struct VideoSlot {
    pub index: usize,
    pub lib: Arc<MpvLib>,
    pub mpv_ctx: Arc<MpvCtx>,
    pub audio_engine: Arc<AudioEngine>,
    pub state: Mutex<SlotState>,
    /// mpv_render_context, created by the render thread (GL context needed);
    /// null until then.
    pub render_ctx: AtomicPtr<c_void>,
    /// Set at creation; cleared by the render thread once `render_ctx` is up.
    pub needs_render_init: AtomicBool,
    /// Set before `quit` + `wakeup`; the event loop checks it before handling
    /// an event so it cannot issue another raw mpv call during retirement.
    event_shutdown: AtomicBool,
    /// Retained rather than detached: the client remains valid until its only
    /// event owner has exited and this handle has been joined.
    event_thread: Mutex<Option<JoinHandle<()>>>,
    event_exit: Arc<SlotEventExit>,
    /// Guards repeated pipeline retirement / `Drop` calls from destroying the
    /// same libmpv client twice.
    mpv_destroyed: AtomicBool,
    status_tx: Sender<OutputStatus>,
    wake: Arc<dyn Fn() + Send + Sync>,
    registry: Weak<SlotRegistry>,
}

/// Read-only runtime facts for the diagnostics page.  This deliberately
/// contains no handles and does not change slot state or the render loop.
#[derive(Debug, Clone, Default)]
pub(super) struct VideoSlotDiagnostics {
    pub voice_id: VoiceId,
    pub mpv_context: bool,
    pub render_context: bool,
    pub file_loaded: bool,
    pub preloaded: bool,
    pub pending_unload: bool,
    pub hold_last_frame: bool,
    pub paused: Option<bool>,
    pub eof: Option<bool>,
    pub time_pos_ms: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub hwdec_backend: Option<String>,
    pub decoder_format: Option<String>,
    pub dropped_frames: Option<u64>,
    pub delayed_frames: Option<u64>,
}

// SAFETY: the raw mpv pointers are only used through the thread-safe libmpv
// client API; render_ctx is only touched by the render thread.
unsafe impl Send for VideoSlot {}
unsafe impl Sync for VideoSlot {}

impl VideoSlot {
    /// Send an OutputStatus to the show event loop.
    fn send_status(&self, status: OutputStatus) {
        let _ = self.status_tx.send(status);
    }
    fn wake(&self) {
        (self.wake)();
    }

    fn accepts_mpv_calls(&self) -> bool {
        !self.event_shutdown.load(Ordering::Acquire) && !self.mpv_destroyed.load(Ordering::Acquire)
    }

    /// Signal the one owner that may block in `mpv_wait_event`. The output
    /// gate has already excluded all normal callers, and the GL/overlay
    /// owners are joined before this runs, so these are the final raw calls
    /// allowed on this client until it is destroyed.
    fn request_event_shutdown(&self) {
        if self.mpv_destroyed.load(Ordering::Acquire) {
            return;
        }
        self.event_shutdown.store(true, Ordering::Release);
        unsafe {
            let quit = cs("quit");
            let args: [*const c_char; 2] = [quit.as_ptr(), std::ptr::null()];
            (self.lib.mpv_command)(self.mpv_ctx.0, args.as_ptr());
            (self.lib.mpv_wakeup)(self.mpv_ctx.0);
        }
    }

    /// Join the event owner without keeping any slot mutex locked. The
    /// worker's `mpv_wait_event` timeout is bounded and the shutdown wakeup
    /// above interrupts it, so this cannot wait on ordinary idle polling.
    fn join_event_thread(&self) {
        let handle = self
            .event_thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(handle) = handle else {
            return;
        };

        if handle.join().is_err() {
            log::error!(
                "[slot {}] event thread panicked during shutdown",
                self.index
            );
        }
        debug_assert!(
            self.event_exit.wait(Duration::ZERO),
            "joined slot event worker must have published completion"
        );
    }

    /// Called only after [`Self::join_event_thread`] and after render cleanup
    /// freed the slot's Render API context on the GL thread.
    fn destroy_mpv_context(&self) {
        if self.mpv_destroyed.swap(true, Ordering::AcqRel) {
            return;
        }
        unsafe { (self.lib.mpv_terminate_destroy)(self.mpv_ctx.0) };
    }
}

// ---------------------------------------------------------------------------
// Pool operations
// ---------------------------------------------------------------------------

/// Find the slot currently owning `voice`.
pub(super) fn slot_for_voice(
    registry: &Arc<SlotRegistry>,
    voice: VoiceId,
) -> Option<Arc<VideoSlot>> {
    registry.all_slots().into_iter().find(|s| {
        s.state
            .lock()
            .map(|st| st.voice_id == Some(voice))
            .unwrap_or(false)
    })
}

/// Acquire a slot for new content: reuse an idle one, create one below the
/// cap, or steal the oldest occupied slot (hard stop + `Completed`).
pub(super) fn acquire_slot(
    registry: &Arc<SlotRegistry>,
    lib: &Arc<MpvLib>,
    audio_engine: &Arc<AudioEngine>,
) -> Result<Arc<VideoSlot>> {
    // 1. Reuse an idle slot.
    for slot in registry.all_slots() {
        if let Ok(st) = slot.state.lock() {
            if st.voice_id.is_none() && !st.pending_unload {
                return Ok(Arc::clone(&slot));
            }
        }
    }

    // 2. Create a new one below the cap.
    let count = registry.slots.read().map(|v| v.len()).unwrap_or(0);
    if count < MAX_VIDEO_SLOTS {
        return create_slot(registry, lib, audio_engine);
    }

    // 3. Steal the oldest (lowest sequence) occupied slot.
    let victim = registry
        .all_slots()
        .into_iter()
        .min_by_key(|s| {
            s.state
                .lock()
                .map(|st| st.layer_key & 0xFF_FFFF_FFFF)
                .unwrap_or(u64::MAX)
        })
        .ok_or_else(|| anyhow!("video slot pool empty and at cap — cannot allocate"))?;
    log::warn!(
        "[slot] pool exhausted ({MAX_VIDEO_SLOTS} slots) — stealing slot {}",
        victim.index
    );
    hard_unload(registry, &victim, true);
    Ok(victim)
}

/// Create a new mpv context + event thread and register the slot.
fn create_slot(
    registry: &Arc<SlotRegistry>,
    lib: &Arc<MpvLib>,
    audio_engine: &Arc<AudioEngine>,
) -> Result<Arc<VideoSlot>> {
    let ctx = unsafe { (lib.mpv_create)() };
    if ctx.is_null() {
        return Err(anyhow!("mpv_create() returned null for video slot"));
    }

    unsafe {
        opt_str(lib, ctx, "vo", "libmpv");
        opt_str(lib, ctx, "hwdec", hwdec_mode());
        opt_str(lib, ctx, "osc", "no");
        opt_str(lib, ctx, "osd-level", "0");
        opt_str(lib, ctx, "input-default-bindings", "no");
        opt_str(lib, ctx, "input-vo-keyboard", "no");
        opt_str(lib, ctx, "input-cursor", "no");
        opt_str(lib, ctx, "keep-open", "no");
        opt_str(lib, ctx, "idle", "yes");
        opt_str(lib, ctx, "ao", "null");
        opt_str(lib, ctx, "audio", "no");
        opt_str(lib, ctx, "video-sync", "desync");
        // Transparent background: where the slot has no pixels (letterbox,
        // idle) the FBO alpha is 0 so lower layers show through.
        // `background=none` is mpv ≥ 0.38; `alpha=yes` covers older libmpv
        // (Ubuntu 22.04 ships 0.34) — setting both is harmless.
        opt_str(lib, ctx, "background", "none");
        opt_str(lib, ctx, "alpha", "yes");

        let ret = (lib.mpv_initialize)(ctx);
        if ret < 0 {
            (lib.mpv_terminate_destroy)(ctx);
            return Err(anyhow!("mpv_initialize() failed for video slot: {ret}"));
        }

        // Observe time-pos so the event thread can advance slice plans when
        // playback crosses a segment boundary (fires ~once per video frame;
        // the handler is a cheap lock + compare when no plan is active).
        let time_pos = cs("time-pos");
        (lib.mpv_observe_property)(ctx, 0, time_pos.as_ptr(), MPV_FORMAT_DOUBLE);

        // Decoder diagnostics reach the event thread as log messages — that is
        // how a failed hwdec init is detected and worked around.
        (lib.mpv_request_log_messages)(ctx, cs("warn").as_ptr());
    }

    let index = registry.slots.read().map(|v| v.len()).unwrap_or(0);
    let slot = Arc::new(VideoSlot {
        index,
        lib: Arc::clone(lib),
        mpv_ctx: Arc::new(MpvCtx(ctx)),
        audio_engine: Arc::clone(audio_engine),
        state: Mutex::new(SlotState::idle()),
        render_ctx: AtomicPtr::new(std::ptr::null_mut()),
        needs_render_init: AtomicBool::new(true),
        event_shutdown: AtomicBool::new(false),
        event_thread: Mutex::new(None),
        event_exit: Arc::new(SlotEventExit::default()),
        mpv_destroyed: AtomicBool::new(false),
        status_tx: registry.status_tx.clone(),
        wake: Arc::clone(&registry.wake),
        registry: Arc::downgrade(registry),
    });

    let event_slot = Arc::clone(&slot);
    let event_exit = Arc::clone(&slot.event_exit);
    let event_thread = match std::thread::Builder::new()
        .name(format!("inkue-slot-{index}-events"))
        .spawn(move || {
            let _exit = SlotEventExitGuard(event_exit);
            slot_event_loop(event_slot);
        }) {
        Ok(handle) => handle,
        Err(e) => {
            // No worker owns this client when spawning fails, so the normal
            // libmpv teardown is safe and avoids leaking a half-created slot.
            unsafe { (lib.mpv_terminate_destroy)(ctx) };
            return Err(anyhow!("spawn slot event thread: {e}"));
        }
    };
    *slot
        .event_thread
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(event_thread);

    if let Ok(mut slots) = registry.slots.write() {
        slots.push(Arc::clone(&slot));
    } else {
        // The worker has already started, but the slot was never published to
        // the renderer. Retire it synchronously instead of dropping its join
        // handle and leaving a hidden mpv client behind.
        slot.request_event_shutdown();
        slot.join_event_thread();
        slot.destroy_mpv_context();
        return Err(anyhow!("slot registry poisoned"));
    }
    // The render thread creates the mpv_render_context + FBO on next wake.
    registry.wake();

    // Block until the render context exists (normally a few ms).  With
    // `vo=libmpv`, a `loadfile` whose track selection runs before the render
    // context is attached fails with NOTHING_TO_PLAY (-16): mpv sees a video
    // track but no VO to put it on, and with `audio=no` nothing is left —
    // the cue errors out at GO (observed in the field on fast local files).
    let deadline = Instant::now() + Duration::from_secs(2);
    while slot.render_ctx.load(Ordering::Acquire).is_null() {
        if Instant::now() >= deadline {
            log::warn!("[slot {index}] render context not ready after 2 s — loading anyway");
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    log::info!("[slot] created video slot {index}");
    Ok(slot)
}

/// Immediately cut a slot's content: stop mpv, stop its audio voice, clear
/// the state.  `report_completed` sends `Completed` so the owning cue resets
/// (steal / panic paths).
pub(super) fn hard_unload(
    _registry: &Arc<SlotRegistry>,
    slot: &Arc<VideoSlot>,
    report_completed: bool,
) {
    let (voice, audio) = {
        let Ok(mut st) = slot.state.lock() else {
            return;
        };
        st.generation = st.generation.wrapping_add(1);
        let voice = st.voice_id.take();
        let audio = st.audio_voice_id.take();
        st.pending_reveal = None;
        st.reveal_deadline = None;
        st.pending_seek_ms = None;
        st.pending_seek_action_ms = None;
        st.file_loaded = false;
        st.loaded_duration_ms = None;
        st.reveal_after_seek_ms = None;
        st.preloaded = false;
        st.pending_unload = false;
        st.slice_plan = None;
        st.anim.set(0.0);
        (voice, audio)
    };
    if slot.accepts_mpv_calls() {
        unsafe {
            let stop = cs("stop");
            let args: [*const c_char; 2] = [stop.as_ptr(), std::ptr::null()];
            (slot.lib.mpv_command)(slot.mpv_ctx.0, args.as_ptr());
        }
    }
    if let Some(aid) = audio {
        let _ =
            slot.audio_engine
                .stop_voice(aid, 0, crate::engine::ring_command::FadeCurve::Linear);
    }
    if report_completed {
        if let Some(vid) = voice {
            slot.send_status(OutputStatus::Completed { voice_id: vid });
        }
    }
    slot.wake();
}

/// Parameters for [`load_into_slot`].
pub(super) struct SlotLoad {
    pub voice_id: VoiceId,
    pub audio_voice_id: Option<VoiceId>,
    pub url: String,
    pub is_image: bool,
    pub fade_in_ms: u32,
    pub loop_count: u32,
    pub initial_seek_action_ms: Option<u64>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub display_duration_ms: Option<u64>,
    pub hold_last_frame: bool,
    pub live_source: bool,
    pub geometry: VideoGeometry,
    pub layer_style: LayerStyle,
    /// QLab-style slice segments `(start_s, end_s, play_count)`; empty = none.
    pub slices: Vec<(f64, f64, u32)>,
    /// Decode into the slot but hold the reveal (Load Cue) — see
    /// [`SlotState::preloaded`].
    pub preload: bool,
}

/// Load content into an (idle) slot.
pub(super) fn load_into_slot(slot: &Arc<VideoSlot>, load: SlotLoad) {
    if !slot.accepts_mpv_calls() {
        return;
    }
    let lib = &slot.lib;
    let ctx = slot.mpv_ctx.0;
    let seq = slot
        .registry
        .upgrade()
        .map(|r| r.next_layer_seq())
        .unwrap_or(1);

    {
        let Ok(mut st) = slot.state.lock() else {
            return;
        };
        st.generation = st.generation.wrapping_add(1);
        st.voice_id = Some(load.voice_id);
        st.audio_voice_id = load.audio_voice_id;
        st.geometry = load.geometry;
        st.crop_applied = !load.geometry.has_crop();
        st.layer_key = resolve_layer_key(load.layer_style.layer, seq);
        st.blend_mode = load.layer_style.blend_mode;
        st.base_opacity = load.layer_style.opacity.clamp(0.0, 1.0) as f32;
        st.pending_unload = false;
        st.pending_seek_ms = None;
        st.pending_seek_action_ms = load.initial_seek_action_ms;
        st.file_loaded = false;
        st.loaded_duration_ms = None;
        st.reveal_after_seek_ms = None;
        st.source_start_ms = load.start_ms;
        st.source_end_ms = load.end_ms;
        st.loop_count = load.loop_count;
        st.hold_last_frame = load.hold_last_frame;
        st.slice_plan = if load.slices.is_empty() {
            None
        } else {
            Some(SlicePlan {
                segments: load.slices.clone(),
                current: 0,
                stop_at_end: false,
            })
        };
        st.preloaded = load.preload;
        if load.preload {
            // Preload: decode into the slot but keep it dark and paused. The
            // reveal is armed here and released by `start_preloaded`, so an
            // image gets the same treatment as a video — it must not appear
            // just because it decodes instantly.
            st.anim.set(0.0);
            st.pending_reveal = Some(load.fade_in_ms);
            st.reveal_deadline = None;
        } else if load.is_image {
            // Images decode near-instantly: start the reveal fade right away.
            st.pending_reveal = None;
            st.reveal_deadline = None;
            let base = st.base_opacity;
            if load.fade_in_ms > 0 {
                st.anim.set(0.0);
                st.anim.animate_to(base, load.fade_in_ms);
            } else {
                st.anim.set(base);
            }
        } else {
            // Videos load paused; PLAYBACK_RESTART reveals + unpauses.
            st.anim.set(0.0);
            st.pending_reveal = Some(load.fade_in_ms);
            st.reveal_deadline = None; // armed at FILE_LOADED
        }
    }

    // Per-slot scalar geometry (crop resolves at VIDEO_RECONFIG).
    super::apply_scalar_geometry(lib, ctx, &load.geometry);
    let _ = try_apply_crop(lib, ctx, &load.geometry);

    unsafe {
        let keep_open = if load.hold_last_frame && !load.is_image {
            "yes"
        } else {
            "no"
        };
        (lib.mpv_set_property_string)(ctx, cs("keep-open").as_ptr(), cs(keep_open).as_ptr());

        let mut opts: Vec<String> = vec!["audio=no".to_string()];
        if load.is_image {
            let duration_val = load
                .display_duration_ms
                .map(|ms| format!("{:.3}", ms as f64 / 1000.0))
                .unwrap_or_else(|| "inf".to_string());
            opts.push(format!("image-display-duration={duration_val}"));
            // A preloaded image stays paused like a video: its display
            // duration must start counting when the cue is started, not when
            // it was loaded.
            let pause = if load.preload { "yes" } else { "no" };
            (lib.mpv_set_property_string)(ctx, cs("pause").as_ptr(), cs(pause).as_ptr());
        } else {
            if let Some(start) = load.start_ms {
                opts.push(format!("start={:.3}", start as f64 / 1000.0));
            }
            if let Some(end) = load.end_ms {
                opts.push(format!("end={:.3}", end as f64 / 1000.0));
            }
            if load.slices.is_empty() {
                let loop_val = if load.loop_count == u32::MAX {
                    "inf".to_string()
                } else if load.loop_count == 0 {
                    "no".to_string()
                } else {
                    load.loop_count.to_string()
                };
                opts.push(format!("loop-file={loop_val}"));
            } else {
                // Sliced playback: the segments own all looping (via ab-loop);
                // program segment 0's loop as loadfile options so it is active
                // before the first frame plays.
                opts.push("loop-file=no".to_string());
                let (a, b, count) = load.slices[0];
                if count != 1 {
                    opts.push(format!("ab-loop-a={a:.3}"));
                    opts.push(format!("ab-loop-b={b:.3}"));
                    let count_val = if count == u32::MAX {
                        "inf".to_string()
                    } else {
                        count.saturating_sub(1).to_string()
                    };
                    opts.push(format!("ab-loop-count={count_val}"));
                }
            }
            // Live sources: playback is timestamp-paced, so any backlog
            // buffered during device-open + the paused-load window would
            // persist as a *constant* glass-to-glass delay — `untimed`
            // displays frames as soon as they decode (safe: audio=no),
            // draining that backlog and pinning the feed to the live edge.
            if load.live_source {
                opts.push("cache=no".to_string());
                opts.push("untimed=yes".to_string());
                opts.push("demuxer-readahead-secs=0".to_string());
                opts.push("demuxer-lavf-analyzeduration=0.1".to_string());
                // lavf options from mpv's built-in low-latency profile.
                opts.push("demuxer-lavf-o-add=fflags=+nobuffer".to_string());
                opts.push("demuxer-lavf-probe-info=nostreams".to_string());
                // Frame-threaded decode adds one frame of delay per thread
                // (~270 ms for an 8-thread MJPEG webcam at 30 fps).
                opts.push("vd-lavc-threads=1".to_string());
                opts.push("video-latency-hacks=yes".to_string());
            }
            // Paused load: frame 0 decoded → PLAYBACK_RESTART → reveal.
            (lib.mpv_set_property_string)(ctx, cs("pause").as_ptr(), cs("yes").as_ptr());
        }

        let Ok(path_cstr) = CString::new(load.url.as_str()) else {
            log::warn!("[slot] load path contains NUL byte");
            return;
        };
        let opts_cstr = cs(&opts.join(","));
        let cmd = cs("loadfile");
        let flags = cs("replace");
        let idx = cs("0");
        // loadfile signature: <url> <flags> <index> <options> (see fade.rs).
        let args: [*const c_char; 6] = [
            cmd.as_ptr(),
            path_cstr.as_ptr(),
            flags.as_ptr(),
            idx.as_ptr(),
            opts_cstr.as_ptr(),
            std::ptr::null(),
        ];
        let ret = (lib.mpv_command)(ctx, args.as_ptr());
        if ret < 0 {
            log::warn!(
                "[slot {}] loadfile failed: {ret} ({})",
                slot.index,
                redact_srt_diagnostic(&load.url)
            );
        } else {
            log::info!(
                "[slot {}] loadfile: {} opts=[{}]",
                slot.index,
                redact_srt_diagnostic(&load.url),
                opts.join(",")
            );
        }
    }
    slot.wake();
}

/// Queue a seek that arrived while a video is still loading.  The slot is
/// paused until its first frame, and mpv may discard an immediate `seek`
/// command while replacing the previous file.  Returning `true` tells the
/// caller not to issue that premature command.
pub(super) fn queue_seek_if_loading(slot: &Arc<VideoSlot>, position_ms: u64) -> bool {
    let Ok(mut state) = slot.state.lock() else {
        return false;
    };
    if seek_must_wait_for_file_loaded(&state) {
        state.pending_seek_ms = Some(position_ms);
        return true;
    }
    false
}

pub(super) fn take_pending_seek(slot: &Arc<VideoSlot>) -> Option<u64> {
    slot.state.lock().ok()?.pending_seek_ms.take()
}

pub(super) fn queue_seek_action_if_loading(slot: &Arc<VideoSlot>, position_ms: u64) -> bool {
    let Ok(mut state) = slot.state.lock() else {
        return false;
    };
    if seek_must_wait_for_file_loaded(&state) {
        state.pending_seek_action_ms = Some(position_ms);
        return true;
    }
    false
}

pub(super) fn take_pending_seek_action(slot: &Arc<VideoSlot>) -> Option<u64> {
    slot.state.lock().ok()?.pending_seek_action_ms.take()
}

/// Apply a seek on a loaded slot and re-anchor the paired audio voice.  This
/// is also used by the FILE_LOADED handler for a seek queued during GO.
pub(super) fn apply_seek(slot: &Arc<VideoSlot>, position_ms: u64) {
    if !slot.accepts_mpv_calls() {
        return;
    }
    let pos_str = format!("{:.3}", position_ms as f64 / 1000.0);
    let cmd_cstr = cs("seek");
    let pos_cstr = cs(&pos_str);
    let mode_cstr = cs("absolute");
    unsafe {
        let args = [
            cmd_cstr.as_ptr(),
            pos_cstr.as_ptr(),
            mode_cstr.as_ptr(),
            std::ptr::null(),
        ];
        (slot.lib.mpv_command)(slot.mpv_ctx.0, args.as_ptr());
    }
    if let Some(audio_id) = slot
        .state
        .lock()
        .ok()
        .and_then(|state| state.audio_voice_id)
    {
        let _ = slot.audio_engine.seek_voice_ms(audio_id, position_ms);
    }
}

pub(super) fn action_seek_file_ms(
    slot: &Arc<VideoSlot>,
    action_ms: u64,
    source_duration_ms: u64,
) -> u64 {
    let (start_ms, end_ms, loop_count) = slot
        .state
        .lock()
        .map(|state| (state.source_start_ms, state.source_end_ms, state.loop_count))
        .unwrap_or((None, None, 0));
    let file_ms = crate::cue::traits::media_position_from_action_ms(
        std::time::Duration::from_millis(action_ms),
        Some(std::time::Duration::from_millis(source_duration_ms)),
        start_ms.map(std::time::Duration::from_millis),
        end_ms.map(std::time::Duration::from_millis),
        1.0,
        loop_count,
    );
    file_ms
}

fn seek_must_wait_for_file_loaded(state: &SlotState) -> bool {
    state.pending_reveal.is_some() && !state.preloaded && !state.file_loaded
}

/// Mark an initial seek that must settle before the paused load is revealed.
/// This closes the `FILE_LOADED` → `PLAYBACK_RESTART` race for Number seeks.
pub(super) fn wait_to_reveal_after_seek(slot: &Arc<VideoSlot>, file_ms: u64) {
    if let Ok(mut state) = slot.state.lock() {
        if state.pending_reveal.is_some() && !state.preloaded {
            state.reveal_after_seek_ms = Some(file_ms);
        }
    }
}

fn seek_position_reached(actual_ms: Option<u64>, expected_ms: u64) -> bool {
    // mpv reports the decoded/keyframe position while paused. A short margin
    // accepts that normal granularity without allowing a visible frame-zero
    // reveal for a real timeline seek.
    actual_ms.is_some_and(|actual| actual.abs_diff(expected_ms) <= 250)
}

/// Reveal only when an initial seek has landed. Returns false while mpv is
/// still exposing the paused-load frame at zero.
fn reveal_if_seeked(slot: &Arc<VideoSlot>) -> bool {
    let expected = match slot.state.lock() {
        Ok(state) if state.pending_reveal.is_some() && !state.preloaded => {
            state.reveal_after_seek_ms
        }
        _ => return false,
    };
    if let Some(expected_ms) = expected {
        if !seek_position_reached(position_ms(slot), expected_ms) {
            return false;
        }
        if let Ok(mut state) = slot.state.lock() {
            if state.reveal_after_seek_ms == Some(expected_ms) {
                state.reveal_after_seek_ms = None;
            }
        }
    }
    reveal(slot);
    true
}

/// Begin the stop fade for a voice.  The render thread finishes the unload
/// once the opacity reaches 0.  Audio is stopped by the caller (engine).
pub(super) fn begin_stop(slot: &Arc<VideoSlot>, visual_fade_ms: u32) {
    if let Ok(mut st) = slot.state.lock() {
        st.pending_reveal = None;
        st.reveal_deadline = None;
        st.preloaded = false;
        st.pending_unload = true;
        st.audio_voice_id = None; // caller owns the audio stop
        if visual_fade_ms == 0 {
            st.anim.set(0.0);
        } else {
            st.anim.animate_to(0.0, visual_fade_ms);
        }
    }
    slot.wake();
}

/// Per-frame slot maintenance, called by the render thread: advance the
/// opacity animation and finish pending unloads.  Returns the current
/// opacity and whether the slot still needs animation frames.
pub(super) fn tick_slot(slot: &Arc<VideoSlot>) -> (f32, bool) {
    // Reveal watchdog (mpv never signalled the first frame).
    let force_reveal = {
        let Ok(mut st) = slot.state.lock() else {
            return (0.0, false);
        };
        match (st.pending_reveal, st.reveal_deadline) {
            // A preloaded slot is *meant* to sit there dark and paused, so the
            // watchdog must never drag it on screen.
            _ if st.preloaded => false,
            (Some(_), Some(deadline)) if Instant::now() >= deadline => {
                st.reveal_deadline = None;
                true
            }
            _ => false,
        }
    };
    if force_reveal {
        if !reveal_if_seeked(slot) {
            // A queued Number seek has not displaced frame zero yet. Retry
            // the command and keep the picture dark rather than exposing the
            // wrong start frame while its paired audio is already seeked.
            let expected = slot
                .state
                .lock()
                .ok()
                .and_then(|st| st.reveal_after_seek_ms);
            if let Some(expected_ms) = expected {
                log::warn!(
                    "[slot {}] initial seek has not settled; retrying",
                    slot.index
                );
                apply_seek(slot, expected_ms);
                if let Ok(mut st) = slot.state.lock() {
                    if st.reveal_after_seek_ms == Some(expected_ms) {
                        st.reveal_deadline = Some(Instant::now() + Duration::from_millis(2500));
                    }
                }
            }
        }
    }

    let Ok(mut st) = slot.state.lock() else {
        return (0.0, false);
    };
    let (opacity, _completed) = st.anim.tick();
    let animating = st.anim.is_animating();
    // Unload as soon as the stop fade has landed.  Do NOT require the tick to
    // report "just completed": a zero-fade stop (`begin_stop(0)` — hard cut,
    // the default for a Camera Cue) parks the animation at 0 already *resting*,
    // so `tick()` never completes — requiring it left the slot occupied and
    // mpv playing forever, which kept the capture device open (the next GO on
    // the same camera then failed with `dshow: device already in use`).
    let unload_now = st.pending_unload && opacity <= 0.0 && !animating;
    if unload_now {
        st.pending_unload = false;
        st.voice_id = None;
        st.generation = st.generation.wrapping_add(1);
        drop(st);
        if slot.accepts_mpv_calls() {
            unsafe {
                let stop = cs("stop");
                let args: [*const c_char; 2] = [stop.as_ptr(), std::ptr::null()];
                (slot.lib.mpv_command)(slot.mpv_ctx.0, args.as_ptr());
            }
        }
        return (0.0, false);
    }
    (opacity, animating)
}

/// Release a preloaded slot: the file is already open and decoded, so this
/// only lifts the hold and reveals it.  Returns `false` when the slot was not
/// preloaded (an ordinary resume then applies).
pub(super) fn start_preloaded(slot: &Arc<VideoSlot>) -> bool {
    {
        let Ok(mut st) = slot.state.lock() else {
            return false;
        };
        if !st.preloaded {
            return false;
        }
        st.preloaded = false;
    }
    reveal(slot);
    log::info!("[slot {}] preloaded content started", slot.index);
    true
}

/// Reveal a paused video load: resume its audio voice, unpause mpv, start the
/// opacity fade-in.  Called on `PLAYBACK_RESTART` (or the watchdog).
fn reveal(slot: &Arc<VideoSlot>) {
    if !slot.accepts_mpv_calls() {
        return;
    }
    let (fade_in_ms, base, audio) = {
        let Ok(mut st) = slot.state.lock() else {
            return;
        };
        let Some(fade) = st.pending_reveal.take() else {
            return;
        };
        st.reveal_deadline = None;
        (fade, st.base_opacity, st.audio_voice_id)
    };

    if let Some(aid) = audio {
        let _ = slot.audio_engine.resume_voice(aid);
    }
    unsafe {
        (slot.lib.mpv_set_property_string)(slot.mpv_ctx.0, cs("pause").as_ptr(), cs("no").as_ptr());
    }
    if let Ok(mut st) = slot.state.lock() {
        if fade_in_ms > 0 {
            st.anim.set(0.0);
            st.anim.animate_to(base, fade_in_ms);
        } else {
            st.anim.set(base);
        }
    }
    slot.wake();
}

// ---------------------------------------------------------------------------
// Live property updates
// ---------------------------------------------------------------------------

/// Live-apply a cue's LayerStyle edit (opacity edits retarget the animation
/// so a running fade is not fought).
pub(super) fn set_layer_style(slot: &Arc<VideoSlot>, style: &LayerStyle) {
    if let Ok(mut st) = slot.state.lock() {
        st.blend_mode = style.blend_mode;
        let new_base = style.opacity.clamp(0.0, 1.0) as f32;
        // Re-key only the explicit band; the sequence part is kept so the
        // stacking among same-layer content is stable.
        let seq = st.layer_key & 0xFF_FFFF_FFFF;
        st.layer_key = resolve_layer_key(style.layer, seq);
        if !st.anim.is_animating() && !st.pending_unload && st.pending_reveal.is_none() {
            st.anim.set(new_base);
        }
        st.base_opacity = new_base;
    }
    slot.wake();
}

/// Directly drive a slot's opacity (Fade Cue tick, ~30 fps).
pub(super) fn set_opacity_direct(slot: &Arc<VideoSlot>, opacity: f32) {
    if let Ok(mut st) = slot.state.lock() {
        st.anim.set(opacity);
    }
    slot.wake();
}

/// Current animated opacity of a voice's slot.
pub(super) fn opacity_of(slot: &Arc<VideoSlot>) -> f32 {
    slot.state.lock().map(|st| st.anim.current).unwrap_or(0.0)
}

/// Animate a slot's opacity to a target (EOF fade-out).
pub(super) fn animate_opacity(slot: &Arc<VideoSlot>, target: f32, duration_ms: u32) {
    if let Ok(mut st) = slot.state.lock() {
        st.anim.animate_to(target, duration_ms.max(1));
    }
    slot.wake();
}

// ---------------------------------------------------------------------------
// Slot event loop
// ---------------------------------------------------------------------------

fn slot_event_loop(slot: Arc<VideoSlot>) {
    let lib = Arc::clone(&slot.lib);
    let ctx = slot.mpv_ctx.0 as usize; // Send-safe copy for this thread only.

    loop {
        if slot.event_shutdown.load(Ordering::Acquire) {
            break;
        }
        let event = unsafe { (lib.mpv_wait_event)(ctx as *mut c_void, 1.0) };
        // `quit` and `wakeup` can return a queued non-shutdown event first.
        // Never process it after retirement has started: the teardown caller
        // is waiting for this owner before it destroys the client.
        if slot.event_shutdown.load(Ordering::Acquire) {
            break;
        }
        if event.is_null() {
            continue;
        }
        let event_id = unsafe { (*event).event_id };

        match event_id {
            MPV_EVENT_SHUTDOWN => break,

            MPV_EVENT_PLAYBACK_RESTART | MPV_EVENT_SEEK => {
                if reveal_if_seeked(&slot) {
                    log::info!("[slot {}] first seeked frame — revealed", slot.index);
                }
            }

            MPV_EVENT_FILE_LOADED => {
                // Report the media duration to the show event loop.
                let mut duration_secs: f64 = 0.0;
                let ret = unsafe {
                    let name = cs("duration");
                    (lib.mpv_get_property)(
                        ctx as *mut c_void,
                        name.as_ptr(),
                        MPV_FORMAT_DOUBLE,
                        &mut duration_secs as *mut f64 as *mut c_void,
                    )
                };
                let loaded_duration_ms =
                    (ret == 0 && duration_secs.is_finite() && duration_secs > 0.0)
                        .then_some((duration_secs * 1000.0) as u64);
                if let Ok(mut st) = slot.state.lock() {
                    st.file_loaded = true;
                    st.loaded_duration_ms = loaded_duration_ms;
                    if let Some(vid) = st.voice_id {
                        if ret == 0 {
                            slot.send_status(OutputStatus::Duration {
                                voice_id: vid,
                                duration_ms: (duration_secs * 1000.0) as u64,
                            });
                        }
                        // Arm the reveal watchdog for paused video loads — but
                        // never for a preload, which has no deadline to meet.
                        if st.pending_reveal.is_some() && !st.preloaded {
                            st.reveal_deadline = Some(Instant::now() + Duration::from_millis(2500));
                        }
                    }
                }
                if let (Some(action_ms), Some(duration_ms)) =
                    (take_pending_seek_action(&slot), loaded_duration_ms)
                {
                    let file_ms = action_seek_file_ms(&slot, action_ms, duration_ms);
                    wait_to_reveal_after_seek(&slot, file_ms);
                    apply_seek(&slot, file_ms);
                } else if let Some(position_ms) = take_pending_seek(&slot) {
                    wait_to_reveal_after_seek(&slot, position_ms);
                    apply_seek(&slot, position_ms);
                }
                slot.wake();
            }

            MPV_EVENT_VIDEO_RECONFIG => {
                // Source dimensions are now known — resolve the pixel crop.
                let geometry = slot.state.lock().ok().and_then(|mut st| {
                    if st.crop_applied {
                        None
                    } else {
                        st.crop_applied = true;
                        Some(st.geometry)
                    }
                });
                if let Some(g) = geometry {
                    let _ = try_apply_crop(&lib, ctx as *mut c_void, &g);
                }
                slot.wake();
            }

            MPV_EVENT_END_FILE => {
                let data_ptr = unsafe { (*event).data };
                let Some(end_data) = (unsafe { (data_ptr as *mut MpvEventEndFile).as_ref() })
                else {
                    continue;
                };
                match end_data.reason {
                    MPV_END_FILE_REASON_EOF => {
                        let (voice, audio) = {
                            let Ok(mut st) = slot.state.lock() else {
                                continue;
                            };
                            let voice = st.voice_id.take();
                            let audio = st.audio_voice_id.take();
                            st.pending_reveal = None;
                            st.reveal_deadline = None;
                            st.preloaded = false;
                            st.pending_unload = false;
                            st.slice_plan = None;
                            st.anim.set(0.0);
                            (voice, audio)
                        };
                        if let Some(aid) = audio {
                            let _ = slot.audio_engine.stop_voice(
                                aid,
                                0,
                                crate::engine::ring_command::FadeCurve::Linear,
                            );
                        }
                        if let Some(vid) = voice {
                            slot.send_status(OutputStatus::Completed { voice_id: vid });
                        }
                        slot.wake();
                    }
                    MPV_END_FILE_REASON_ERROR => {
                        let (voice, audio) = {
                            let Ok(mut st) = slot.state.lock() else {
                                continue;
                            };
                            let voice = st.voice_id.take();
                            let audio = st.audio_voice_id.take();
                            st.pending_reveal = None;
                            st.reveal_deadline = None;
                            st.preloaded = false;
                            st.slice_plan = None;
                            st.anim.set(0.0);
                            (voice, audio)
                        };
                        if let Some(aid) = audio {
                            let _ = slot.audio_engine.stop_voice(
                                aid,
                                0,
                                crate::engine::ring_command::FadeCurve::Linear,
                            );
                        }
                        if let Some(vid) = voice {
                            slot.send_status(OutputStatus::Error {
                                voice_id: vid,
                                message: format!("mpv error (code {})", end_data.error),
                            });
                        }
                        slot.wake();
                    }
                    _ => {}
                }
            }

            MPV_EVENT_PROPERTY_CHANGE => {
                // time-pos update — advance the slice plan when playback
                // crossed the current segment's end (ab-loop keeps time *below*
                // the boundary while a segment still loops, so crossing it
                // means the segment is done).
                let data = unsafe { ((*event).data as *const MpvEventProperty).as_ref() };
                let Some(prop) = data else { continue };
                if prop.format != MPV_FORMAT_DOUBLE || prop.data.is_null() {
                    continue;
                }
                let time = unsafe { *(prop.data as *const f64) };

                enum SliceAction {
                    Advance((f64, f64, u32)),
                    Stop,
                }
                let action = {
                    let Ok(mut st) = slot.state.lock() else {
                        continue;
                    };
                    let Some(plan) = st.slice_plan.as_mut() else {
                        continue;
                    };
                    let (_, end, _) = plan.segments[plan.current];
                    if time < end - 0.010 {
                        None
                    } else if plan.stop_at_end {
                        Some(SliceAction::Stop)
                    } else if plan.current + 1 < plan.segments.len() {
                        // Skip every boundary the clock already passed (a slow
                        // event must not re-program a stale segment).
                        while plan.current + 1 < plan.segments.len()
                            && time >= plan.segments[plan.current].1 - 0.010
                        {
                            plan.current += 1;
                        }
                        Some(SliceAction::Advance(plan.segments[plan.current]))
                    } else {
                        None // Last segment — natural EOF completes the cue.
                    }
                };
                match action {
                    Some(SliceAction::Advance(seg)) => {
                        apply_segment_loop(&lib, ctx as *mut c_void, seg);
                        log::info!(
                            "[slot {}] slice → [{:.3}s, {:.3}s) ×{}",
                            slot.index,
                            seg.0,
                            seg.1,
                            if seg.2 == u32::MAX {
                                "∞".into()
                            } else {
                                seg.2.to_string()
                            },
                        );
                    }
                    Some(SliceAction::Stop) => {
                        log::info!("[slot {}] devamp stop at slice boundary", slot.index);
                        if let Some(registry) = slot.registry.upgrade() {
                            hard_unload(&registry, &slot, true);
                        }
                    }
                    None => {}
                }
            }

            MPV_EVENT_LOG_MESSAGE => {
                let data = unsafe { (*event).data as *const MpvEventLogMessage };
                if data.is_null() {
                    continue;
                }
                let level = unsafe { std::ffi::CStr::from_ptr((*data).level) }.to_string_lossy();
                let text = unsafe { std::ffi::CStr::from_ptr((*data).text) }.to_string_lossy();
                let trimmed = text.trim_end_matches('\n');
                if trimmed.is_empty() {
                    continue;
                }
                let redacted = redact_srt_diagnostic(trimmed);
                if matches!(level.as_ref(), "fatal" | "error") {
                    log::error!("[slot {}] [mpv] {redacted}", slot.index);
                }
                // Belt and braces: libavcodec's messages (`h264: Failed setup
                // for format …`) reach the *first* mpv core created — the
                // overlay context, which is where the detection actually
                // fires (see `mpv_events`).  A slot only sees them if mpv ever
                // changes that routing, or if a slot happens to be first.
                if reports_hwdec_failure(trimmed) {
                    fall_back_to_software(&format!("slot {}", slot.index), slot.registry.upgrade());
                }
            }

            _ => {}
        }
    }
}

/// Drop every slot back to software decoding after a failed hwdec init.
///
/// libmpv does not reliably recover on its own: it retries the hardware
/// decoder frame after frame and can hand the compositor half-decoded frames,
/// which shows up as a green cast with a torn band across the picture (issue
/// #5).  Setting `hwdec=no` reinitialises the decoder in place, so the picture
/// corrects itself — usually before the first frame is even revealed, because
/// video loads start paused.  The switch is latched for the session and
/// applied to every existing slot: one GPU that refuses a codec profile
/// refuses it in every slot.
///
/// `origin` names the context that saw the message, for the log line only —
/// the failure is a property of the machine, not of one slot.  Skipped when
/// the operator pinned a mode with `INKUE_HWDEC`.
pub(super) fn fall_back_to_software(origin: &str, registry: Option<Arc<SlotRegistry>>) {
    if hwdec_override().is_some() {
        return;
    }
    if SOFTWARE_DECODE_ONLY.swap(true, Ordering::SeqCst) {
        return; // Already software-only — mpv repeats the message per frame.
    }
    log::warn!(
        "[{origin}] hardware decoding failed — switching every video slot to \
         software decoding for this session",
    );
    for slot in registry.as_ref().map(|r| r.all_slots()).unwrap_or_default() {
        if !slot.accepts_mpv_calls() {
            continue;
        }
        unsafe {
            (slot.lib.mpv_set_property_string)(
                slot.mpv_ctx.0,
                cs("hwdec").as_ptr(),
                cs("no").as_ptr(),
            );
        }
    }
    if let Some(registry) = registry {
        registry.wake();
    }
}

/// Program mpv's ab-loop for `seg` — or clear it when the segment plays once.
fn apply_segment_loop(lib: &Arc<MpvLib>, ctx: *mut c_void, seg: (f64, f64, u32)) {
    let (a, b, count) = seg;
    unsafe {
        if count == 1 {
            (lib.mpv_set_property_string)(ctx, cs("ab-loop-a").as_ptr(), cs("no").as_ptr());
            (lib.mpv_set_property_string)(ctx, cs("ab-loop-b").as_ptr(), cs("no").as_ptr());
        } else {
            let count_val = if count == u32::MAX {
                "inf".to_string()
            } else {
                count.saturating_sub(1).to_string()
            };
            (lib.mpv_set_property_string)(
                ctx,
                cs("ab-loop-count").as_ptr(),
                cs(&count_val).as_ptr(),
            );
            (lib.mpv_set_property_string)(
                ctx,
                cs("ab-loop-a").as_ptr(),
                cs(&format!("{a:.3}")).as_ptr(),
            );
            (lib.mpv_set_property_string)(
                ctx,
                cs("ab-loop-b").as_ptr(),
                cs(&format!("{b:.3}")).as_ptr(),
            );
        }
    }
}

/// Devamp: release the slot's current slice loop.  The pass in progress
/// finishes (ab-loop-count → 0 lets playback continue past B), then the plan
/// advances normally — or the slot stops at the boundary when `stop_at_end`.
/// No-op for unsliced content.
pub(super) fn devamp_slot(slot: &Arc<VideoSlot>, stop_at_end: bool) {
    if !slot.accepts_mpv_calls() {
        return;
    }
    {
        let Ok(mut st) = slot.state.lock() else {
            return;
        };
        let Some(plan) = st.slice_plan.as_mut() else {
            return;
        };
        if stop_at_end {
            plan.stop_at_end = true;
        }
    }
    unsafe {
        let key = cs("ab-loop-count");
        let val = cs("0");
        (slot.lib.mpv_set_property_string)(slot.mpv_ctx.0, key.as_ptr(), val.as_ptr());
    }
    log::info!("[slot {}] devamp (stop_at_end={stop_at_end})", slot.index);
}

// ---------------------------------------------------------------------------
// Pool-wide operations
// ---------------------------------------------------------------------------

/// Panic: hard-unload every slot (double-Escape backstop).
pub(super) fn panic_all() {
    // Kept as a compatibility shim for callers during the runtime migration.
    // Instance owners should call `hard_unload` over their own registry.
    for slot in Vec::<Arc<VideoSlot>>::new() {
        if let Some(registry) = slot.registry.upgrade() {
            hard_unload(&registry, &slot, false);
        }
    }
}

/// Current playback position of a voice's video, in ms.
pub(super) fn position_ms(slot: &Arc<VideoSlot>) -> Option<u64> {
    if !slot.accepts_mpv_calls() {
        return None;
    }
    let mut secs: f64 = 0.0;
    let name = cs("time-pos");
    let ret = unsafe {
        (slot.lib.mpv_get_property)(
            slot.mpv_ctx.0,
            name.as_ptr(),
            MPV_FORMAT_DOUBLE,
            &mut secs as *mut f64 as *mut c_void,
        )
    };
    (ret == 0 && secs >= 0.0).then_some((secs * 1000.0) as u64)
}

pub(super) fn duration_ms(slot: &Arc<VideoSlot>) -> Option<u64> {
    if !slot.accepts_mpv_calls() {
        return None;
    }
    let mut secs: f64 = 0.0;
    let name = cs("duration");
    let ret = unsafe {
        (slot.lib.mpv_get_property)(
            slot.mpv_ctx.0,
            name.as_ptr(),
            MPV_FORMAT_DOUBLE,
            &mut secs as *mut f64 as *mut c_void,
        )
    };
    (ret == 0 && secs.is_finite() && secs > 0.0).then_some((secs * 1000.0) as u64)
}

pub(super) fn loaded_duration_ms(slot: &Arc<VideoSlot>) -> Option<u64> {
    slot.state.lock().ok()?.loaded_duration_ms
}

/// Whether mpv still has an active playback session for this slot. A stale
/// `Completed` notification can arrive after a seek command; checking
/// `idle-active` lets the event loop ignore it while the same voice is playing.
pub(super) fn is_playing(slot: &Arc<VideoSlot>) -> bool {
    if !slot.accepts_mpv_calls() {
        return false;
    }
    let name = cs("idle-active");
    let mut idle_active: i32 = 1;
    let ret = unsafe {
        (slot.lib.mpv_get_property)(
            slot.mpv_ctx.0,
            name.as_ptr(),
            crate::engine::mpv_sys::MPV_FORMAT_FLAG,
            &mut idle_active as *mut i32 as *mut c_void,
        )
    };
    ret == 0 && idle_active == 0
}

/// Collect properties that mpv exposes without sending commands.  Properties
/// absent in a particular libmpv build remain `None`; diagnostics must never
/// turn an unsupported property into a guessed value.
pub(super) fn diagnostics(slot: &Arc<VideoSlot>) -> Option<VideoSlotDiagnostics> {
    let state = slot.state.lock().ok()?;
    let voice_id = state.voice_id?;
    let file_loaded = state.file_loaded;
    let preloaded = state.preloaded;
    let pending_unload = state.pending_unload;
    let hold_last_frame = state.hold_last_frame;
    drop(state);

    let mut snapshot = VideoSlotDiagnostics {
        voice_id,
        mpv_context: !slot.mpv_ctx.0.is_null() && !slot.mpv_destroyed.load(Ordering::Acquire),
        render_context: !slot.render_ctx.load(Ordering::Acquire).is_null(),
        file_loaded,
        preloaded,
        pending_unload,
        hold_last_frame,
        ..VideoSlotDiagnostics::default()
    };
    if !slot.accepts_mpv_calls() {
        return Some(snapshot);
    }

    unsafe {
        snapshot.paused = get_prop_flag(&slot.lib, slot.mpv_ctx.0, "pause");
        snapshot.eof = get_prop_flag(&slot.lib, slot.mpv_ctx.0, "eof-reached");
        snapshot.time_pos_ms = get_prop_double(&slot.lib, slot.mpv_ctx.0, "time-pos")
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| (value * 1000.0) as u64);
        snapshot.width = get_prop_i64(&slot.lib, slot.mpv_ctx.0, "video-params/w")
            .filter(|value| *value > 0)
            .map(|value| value as u32);
        snapshot.height = get_prop_i64(&slot.lib, slot.mpv_ctx.0, "video-params/h")
            .filter(|value| *value > 0)
            .map(|value| value as u32);
        snapshot.fps = get_prop_double(&slot.lib, slot.mpv_ctx.0, "container-fps")
            .filter(|value| value.is_finite() && *value > 0.0)
            .or_else(|| get_prop_double(&slot.lib, slot.mpv_ctx.0, "estimated-vf-fps").filter(|value| value.is_finite() && *value > 0.0));
        snapshot.hwdec_backend = get_prop_string(&slot.lib, slot.mpv_ctx.0, "hwdec-current");
        snapshot.decoder_format = get_prop_string(&slot.lib, slot.mpv_ctx.0, "video-format");
        snapshot.dropped_frames = get_prop_i64(&slot.lib, slot.mpv_ctx.0, "frame-drop-count")
            .or_else(|| get_prop_i64(&slot.lib, slot.mpv_ctx.0, "decoder-frame-drop-count"))
            .filter(|value| *value >= 0)
            .map(|value| value as u64);
        snapshot.delayed_frames = get_prop_i64(&slot.lib, slot.mpv_ctx.0, "vo-delayed-frame-count")
            .or_else(|| get_prop_i64(&slot.lib, slot.mpv_ctx.0, "mistimed-frame-count"))
            .filter(|value| *value >= 0)
            .map(|value| value as u64);
    }
    Some(snapshot)
}

unsafe fn get_prop_double(lib: &MpvLib, ctx: *mut c_void, name: &str) -> Option<f64> {
    let mut value = 0.0;
    let name = cs(name);
    let ret = (lib.mpv_get_property)(ctx, name.as_ptr(), MPV_FORMAT_DOUBLE, &mut value as *mut f64 as *mut c_void);
    (ret == 0).then_some(value)
}

unsafe fn get_prop_flag(lib: &MpvLib, ctx: *mut c_void, name: &str) -> Option<bool> {
    let mut value = 0;
    let name = cs(name);
    let ret = (lib.mpv_get_property)(ctx, name.as_ptr(), crate::engine::mpv_sys::MPV_FORMAT_FLAG, &mut value as *mut i32 as *mut c_void);
    (ret == 0).then_some(value != 0)
}

unsafe fn get_prop_string(lib: &MpvLib, ctx: *mut c_void, name: &str) -> Option<String> {
    let mut value: *mut c_char = std::ptr::null_mut();
    let name = cs(name);
    let ret = (lib.mpv_get_property)(ctx, name.as_ptr(), crate::engine::mpv_sys::MPV_FORMAT_STRING, &mut value as *mut *mut c_char as *mut c_void);
    if ret != 0 || value.is_null() {
        return None;
    }
    let result = CStr::from_ptr(value).to_str().ok().map(str::to_owned);
    (lib.mpv_free)(value as *mut c_void);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use uuid::Uuid;

    #[test]
    fn slot_event_exit_signal_wakes_a_blocked_waiter() {
        let exit = Arc::new(SlotEventExit::default());
        let waiter_exit = Arc::clone(&exit);
        let (started_tx, started_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            waiter_exit.wait(Duration::from_secs(1))
        });

        started_rx.recv().unwrap();
        exit.finish();
        assert!(waiter.join().unwrap());
        // Completion is idempotent and remains visible to a later shutdown
        // observer, just as it is after the owning JoinHandle is collected.
        exit.finish();
        assert!(exit.wait(Duration::ZERO));
    }

    #[test]
    fn registries_have_independent_voice_indexes() {
        let (tx_a, _) = crossbeam_channel::unbounded();
        let (tx_b, _) = crossbeam_channel::unbounded();
        let a = SlotRegistry::new(tx_a, Arc::new(|| {}));
        let b = SlotRegistry::new(tx_b, Arc::new(|| {}));
        assert!(a.all_slots().is_empty());
        assert!(b.all_slots().is_empty());
        assert!(a.slot_for_voice(Uuid::new_v4()).is_none());
        assert!(b.slot_for_voice(Uuid::new_v4()).is_none());
    }

    #[test]
    fn hwdec_failure_detected_from_the_real_mpv_log_lines() {
        // Verbatim from issue #5 (Windows 11, libmpv, H.264 in MP4).
        assert!(reports_hwdec_failure(
            "h264: Failed setup for format d3d11: hwaccel initialisation returned error."
        ));
        // Linux/macOS wording of the same failure.
        assert!(reports_hwdec_failure(
            "hevc: Failed setup for format vaapi: hwaccel initialisation returned error."
        ));
    }

    #[test]
    fn ordinary_mpv_warnings_do_not_trigger_the_software_fallback() {
        assert!(!reports_hwdec_failure(
            "mov,mp4,m4a,3gp,3g2,mj2: Detected creation time before 1970, parsing as unix timestamp."
        ));
        assert!(!reports_hwdec_failure("h264: no frame!"));
        assert!(!reports_hwdec_failure(
            "Using hardware decoding (d3d11va-copy)."
        ));
    }

    #[test]
    fn hwdec_mode_falls_back_to_software_once_latched() {
        assert_eq!(resolve_hwdec_mode(None, false), "auto-copy");
        assert_eq!(
            resolve_hwdec_mode(None, true),
            "no",
            "a latched failure disables hwdec for new slots",
        );
    }

    #[test]
    fn an_operator_pin_outranks_the_automatic_fallback() {
        // INKUE_HWDEC is set on purpose (bug repro, or a known-bad GPU path):
        // the automatic fallback must not silently undo it.
        assert_eq!(
            resolve_hwdec_mode(Some("d3d11va-copy"), true),
            "d3d11va-copy"
        );
        assert_eq!(resolve_hwdec_mode(Some("no"), false), "no");
    }

    #[test]
    fn layer_key_explicit_bands_order_below_automatic() {
        let explicit_low = resolve_layer_key(Some(1), 100);
        let explicit_high = resolve_layer_key(Some(1000), 1);
        let auto_old = resolve_layer_key(None, 2);
        let auto_new = resolve_layer_key(None, 3);
        assert!(explicit_low < explicit_high);
        assert!(
            explicit_high < auto_old,
            "automatic stacks above every explicit layer"
        );
        assert!(auto_old < auto_new, "newer automatic content stacks on top");
    }

    #[test]
    fn layer_key_same_band_ordered_by_sequence() {
        assert!(resolve_layer_key(Some(500), 1) < resolve_layer_key(Some(500), 2));
    }

    #[test]
    fn layer_key_clamps_out_of_range_layers() {
        assert_eq!(resolve_layer_key(Some(0), 7), resolve_layer_key(Some(1), 7));
        assert_eq!(
            resolve_layer_key(Some(5000), 7),
            resolve_layer_key(Some(1000), 7)
        );
    }

    #[test]
    fn opacity_anim_snaps_and_animates() {
        let mut anim = OpacityAnim::resting(0.0);
        assert!(!anim.is_animating());
        anim.set(0.7);
        assert_eq!(anim.current, 0.7);
        anim.animate_to(0.0, 200);
        assert!(anim.is_animating());
        // Zero-duration animation completes on the first tick.
        anim.animate_to(1.0, 0);
        let (v, done) = anim.tick();
        assert_eq!(v, 1.0);
        assert!(done);
        assert!(!anim.is_animating());
    }

    #[test]
    fn opacity_anim_clamps_targets() {
        let mut anim = OpacityAnim::resting(0.5);
        anim.animate_to(7.0, 0);
        let (v, _) = anim.tick();
        assert_eq!(v, 1.0);
    }

    #[test]
    fn resting_anim_never_reports_completed() {
        // begin_stop(fade=0) parks the animation at 0 already resting.  tick()
        // then never reports "just completed", so tick_slot's unload decision
        // must not require it — requiring it left the slot occupied and the
        // capture device open forever (camera relaunch failed with
        // "dshow: device already in use").
        let mut anim = OpacityAnim::resting(1.0);
        anim.set(0.0);
        let (v, completed) = anim.tick();
        assert_eq!(v, 0.0);
        assert!(!completed);
        assert!(!anim.is_animating());
    }

    #[test]
    fn seek_only_queues_before_file_loaded() {
        let mut state = SlotState::idle();
        state.pending_reveal = Some(0);

        assert!(seek_must_wait_for_file_loaded(&state));

        // `pending_reveal` remains set between FILE_LOADED and the first
        // PLAYBACK_RESTART. This was the Number seek race: queuing here
        // waited for an event that had already been consumed.
        state.file_loaded = true;
        assert!(!seek_must_wait_for_file_loaded(&state));
    }

    #[test]
    fn initial_reveal_rejects_frame_zero_until_seek_lands() {
        assert!(!seek_position_reached(Some(0), 2_000));
        assert!(seek_position_reached(Some(1_800), 2_000));
        assert!(seek_position_reached(Some(2_250), 2_000));
        assert!(!seek_position_reached(Some(2_251), 2_000));
        assert!(!seek_position_reached(None, 2_000));
    }
}
