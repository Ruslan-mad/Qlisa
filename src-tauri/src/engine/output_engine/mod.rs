//! [`OutputEngine`] — unified output for both video and image cues.
//!
//! A native window hosts libmpv via the OpenGL Render API (`vo=libmpv`, see
//! `render.rs`); the dip-to-black fade is a GL quad drawn in the same surface.
//! The render loop and fade are identical on every OS — only native window
//! creation differs (winit on Windows/Linux, AppKit/objc2 on macOS, see
//! `macos_window.rs`).
//!
//! On every OS the floating cue timer is a Tauri WebView window (`float-timer`),
//! and the on-output timer is mpv's OSD (`osd-msg1`).

mod blend;
mod browser_surface;
mod fade;
/// macOS-only: AppKit NSWindow creation + control for the GL output path.
#[cfg(target_os = "macos")]
mod macos_window;
mod mpv_events;
mod render;
mod runtime;
mod slot;
mod types;
mod warp;

pub use blend::BlendMode;
pub(crate) use runtime::OutputRegistry;
use runtime::OutputPipelineConfig;
use types::{compose_display_props, FadeAnimState, MpvCtx, OutputVoice, PendingVideoStart};
use render::RenderRuntime;
pub use types::{
    ContentRequest, FitMode, LayerStyle, OutputStatus, OutputSurface, OutputTransform, ScreenInfo,
    SurfaceId, TestPattern, VideoGeometry, VoiceId,
};
pub use browser_surface::{validate_browser_url, BrowserSurfaceManager, BrowserSurfaceState};

use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossbeam_channel::{Receiver, Sender};
use serde::Serialize;
use uuid::Uuid;

use crate::cue::types::{db_to_linear, FadeSpec};
use crate::engine::AudioEngine;
use crate::engine::network_io::{BgraFrame, NetworkFrameSink, NetworkOutputConfig, NetworkOutputManager};

use super::mpv_sys::{
    MpvLib, MpvNode, MpvNodeList, MpvNodeUnion, MPV_FORMAT_INT64, MPV_FORMAT_NODE_MAP,
    MPV_FORMAT_NONE, MPV_FORMAT_STRING,
};

/// Metadata available from a bounded headless libmpv probe.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MediaProbe {
    pub duration: Option<Duration>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputMonitorFrameRead {
    Frame { sequence: u64, frame: BgraFrame },
    Unchanged { sequence: u64 },
    NoFrame,
}

/// A quiescence gate for one native output's overlay mpv client.
///
/// An [`Arc<NativeOutputPipeline>`] alone does not keep its raw `mpv_handle*`
/// valid: another caller can retain that Arc while a destination is retired.
/// Every non-owner mpv call therefore holds a short permit. Retirement first
/// closes the gate (rejecting future permits), then waits for the active ones
/// to leave. The render and event owner threads are joined afterwards, before
/// the client is destroyed, rather than holding permits across their loops.
#[derive(Default)]
pub(super) struct MpvLifecycleGate {
    state: Mutex<MpvLifecycleState>,
    idle: std::sync::Condvar,
}

#[derive(Default)]
struct MpvLifecycleState {
    closing: bool,
    active_calls: usize,
}

pub(super) struct MpvCallPermit<'a> {
    gate: &'a MpvLifecycleGate,
}

impl MpvLifecycleGate {
    fn enter(&self) -> Option<MpvCallPermit<'_>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closing {
            return None;
        }
        state.active_calls += 1;
        Some(MpvCallPermit { gate: self })
    }

    /// Make the gate permanently unavailable, then wait until all callers
    /// which obtained a permit before the transition have returned.
    fn close_and_wait(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closing = true;
        while state.active_calls != 0 {
            state = self
                .idle
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    #[cfg(test)]
    fn is_closing(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .closing
    }
}

impl Drop for MpvCallPermit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        debug_assert!(state.active_calls > 0, "mpv lifecycle permit underflow");
        state.active_calls = state.active_calls.saturating_sub(1);
        if state.active_calls == 0 {
            self.gate.idle.notify_all();
        }
    }
}

// Per-output native pipeline state. Kept behind one Arc so render, mpv event,
// and command threads always address the same output instance.
pub(super) struct PipelineState {
    lifecycle: MpvLifecycleGate,
    pub fade: Mutex<FadeAnimState>,
    /// Dedicated volatile operator FTB layer. It is composited after all cue
    /// and overlay content and therefore cannot be cleared by cue fades.
    pub operator_ftb: Mutex<FadeAnimState>,
    pub mpv_lib: Mutex<Option<Arc<MpvLib>>>,
    pub mpv_ctx: Mutex<Option<Arc<MpvCtx>>>,
    pub status_tx: Sender<OutputStatus>,
    pub current_voice: Mutex<Option<Uuid>>,
    pub current_fade_out_ms: Mutex<u32>,
    pub pending_video_start: Mutex<Option<PendingVideoStart>>,
    pub current_audio_voice: Mutex<Option<Uuid>>,
    pub pending_crop: Mutex<Option<VideoGeometry>>,
    pub transform: Mutex<OutputTransform>,
    pub last_cue_geometry: Mutex<VideoGeometry>,
    pub overlay_has_dummy: AtomicBool,
    pub timer_osd_active: AtomicBool,
    /// Whether the Text Cue overlay is currently populated.  This lives in
    /// the pipeline (rather than the render runtime) because `fade` and the
    /// native OSD helpers also need to know whether the overlay has pixels.
    pub text_overlay_active: AtomicBool,
    pub test_pattern_active: AtomicBool,
}

impl PipelineState {
    fn new(status_tx: Sender<OutputStatus>) -> Arc<Self> { Arc::new(Self {
        lifecycle: MpvLifecycleGate::default(), fade: Mutex::new(FadeAnimState::idle()), operator_ftb: Mutex::new(FadeAnimState::operator_blackout_idle()), mpv_lib: Mutex::new(None), mpv_ctx: Mutex::new(None),
        status_tx, current_voice: Mutex::new(None), current_fade_out_ms: Mutex::new(0),
        pending_video_start: Mutex::new(None), current_audio_voice: Mutex::new(None),
        pending_crop: Mutex::new(None), transform: Mutex::new(OutputTransform::default()),
        last_cue_geometry: Mutex::new(VideoGeometry::default()), overlay_has_dummy: AtomicBool::new(false),
        timer_osd_active: AtomicBool::new(false), text_overlay_active: AtomicBool::new(false),
        test_pattern_active: AtomicBool::new(false),
    }) }

    pub(super) fn enter_mpv_call(&self) -> Option<MpvCallPermit<'_>> {
        self.lifecycle.enter()
    }

    fn close_mpv_calls_and_wait(&self) {
        self.lifecycle.close_and_wait();
    }
}

// Timer preview/font/text are WebView coordination caches, shared by the app
// rather than native per-output state.
fn init_timer_globals() {
    TIMER_PREVIEW.get_or_init(|| Mutex::new(None));
    FLOAT_TIMER_TEXT.get_or_init(|| Mutex::new(String::new()));
    FLOAT_TIMER_FONT.get_or_init(|| Mutex::new(String::new()));
}
/// When `Some`, the timer refresh loop shows this text instead of live cue time.
pub(crate) static TIMER_PREVIEW: OnceLock<Mutex<Option<String>>> = OnceLock::new();
/// Deduplication cache for the floating timer text (avoids redundant Tauri events).
pub(super) static FLOAT_TIMER_TEXT: OnceLock<Mutex<String>> = OnceLock::new();
/// Font family mirrored from OSD settings → emitted to the float-timer window.
pub(super) static FLOAT_TIMER_FONT: OnceLock<Mutex<String>> = OnceLock::new();
/// `true` while the overlay context has the transparent lavfi dummy loaded.
///
/// mpv needs a decoded video surface to composite OSD/text at all — **idle**
/// renders ignore the OSD *and* clear the target to opaque black on some
/// libmpv builds (0.41-dev on Windows honours neither `background=none` nor
/// OSD in idle; measured 2026-07-11).  With a fully transparent RGBA source
/// loaded, mpv honours the source alpha and composites the OSD with correct
/// per-pixel alpha, so timer/text float over the video layers below.
/// `true` while the on-output timer (`osd-msg1`) shows text.
/// `true` while a test pattern occupies the overlay context.

/// `true` when the overlay context currently shows something (timer OSD, Text
/// Cue, test pattern) and must be composited on top of the video layers.
///
/// **Load-bearing for the compositor**: the overlay context's idle render is
/// opaque black on some libmpv builds, so compositing it unconditionally
/// blacks out every video layer below — it must only be composited while one
/// of these is actually active.
pub(super) fn overlay_active(p: &PipelineState) -> bool {
    p.timer_osd_active.load(Ordering::Relaxed)
        || p.text_overlay_active.load(Ordering::Relaxed)
        || p.test_pattern_active.load(Ordering::Relaxed)
}

/// mpv `osd-overlay` ID reserved for the Text Cue surface.  Distinct from the
/// timer (which uses `osd-msg1`, a separate OSD channel).
const TEXT_OSD_OVERLAY_ID: i64 = 47;

/// Load the transparent lavfi dummy into the overlay context (idempotent).
///
/// Called whenever timer/text OSD content appears.  No-op while a test
/// pattern is showing — the pattern is the overlay surface then.
fn ensure_overlay_surface(p: &PipelineState) {
    if p.test_pattern_active.load(Ordering::Relaxed)
        || p.overlay_has_dummy.swap(true, Ordering::Relaxed)
    {
        return;
    }
    let Some(_permit) = p.enter_mpv_call() else {
        return;
    };
    if let (Some(lib), Some(ctx)) = (p.mpv_lib.lock().ok().and_then(|x| x.clone()), p.mpv_ctx.lock().ok().and_then(|x| x.clone())) {
        unsafe {
            let cmd = cs("loadfile");
            // Tiny + fully transparent: `format=rgba` keeps the alpha plane,
            // 10 fps keeps the OSD recomposited without measurable cost.
            let url = cs("av://lavfi:color=c=black@0.0:s=64x64:r=10,format=rgba");
            let flags = cs("replace");
            let idx = cs("0");
            let opts = cs("audio=no,loop-file=inf");
            let args: [*const c_char; 6] = [
                cmd.as_ptr(),
                url.as_ptr(),
                flags.as_ptr(),
                idx.as_ptr(),
                opts.as_ptr(),
                std::ptr::null(),
            ];
            let ret = (lib.mpv_command)(ctx.0, args.as_ptr());
            if ret < 0 {
                log::warn!("[output] overlay dummy loadfile failed: {ret}");
            }
        }
    }
    // Runtime-specific wake is issued by the owning OutputEngine.
}

/// Unload the overlay dummy once neither the timer nor a Text Cue needs it.
fn release_overlay_surface_if_idle(p: &PipelineState) {
    if overlay_active(p) || !p.overlay_has_dummy.swap(false, Ordering::Relaxed) {
        return;
    }
    let Some(_permit) = p.enter_mpv_call() else {
        return;
    };
    if let (Some(lib), Some(ctx)) = (p.mpv_lib.lock().ok().and_then(|x| x.clone()), p.mpv_ctx.lock().ok().and_then(|x| x.clone())) {
        unsafe {
            let stop = cs("stop");
            let args: [*const c_char; 2] = [stop.as_ptr(), std::ptr::null()];
            (lib.mpv_command)(ctx.0, args.as_ptr());
        }
    }
    // Runtime-specific wake is issued by the owning OutputEngine.
}

/// Initialise every global that does not depend on libmpv.
///
/// Shared by both constructors so a headless engine behaves exactly like a
/// live one minus the video output — the timer, transform and geometry state
/// all keep working (and keep serialising) with no output window attached.
// ---------------------------------------------------------------------------
// OutputEngine
// ---------------------------------------------------------------------------

/// Error surfaced by every visual operation attempted in headless mode.
pub const NO_VIDEO_OUTPUT: &str =
    "Video output unavailable — libmpv is not loaded (see the startup log)";

/// Shared visibility state for physical output windows. Native window events
/// run on the winit thread, so they cannot call back into `OutputEngine` while
/// it holds the output graph gate. This small state object computes the
/// aggregate event without touching any NDI/SRT pipeline.
struct DisplayVisibilityState {
    app_handle: tauri::AppHandle,
    outputs: Mutex<HashMap<String, bool>>,
}

impl DisplayVisibilityState {
    fn new(app_handle: tauri::AppHandle) -> Arc<Self> {
        Arc::new(Self {
            app_handle,
            outputs: Mutex::new(HashMap::new()),
        })
    }

    fn set(&self, output_id: &str, visible: bool) {
        let aggregate = self
            .outputs
            .lock()
            .map(|mut outputs| {
                outputs.insert(output_id.to_owned(), visible);
                outputs.values().any(|state| *state)
            })
            .unwrap_or(visible);
        #[cfg(not(test))]
        {
            use tauri::Emitter;
            let _ = self.app_handle.emit("output-window-visible", aggregate);
        }
    }

    fn remove(&self, output_id: &str) {
        let aggregate = self
            .outputs
            .lock()
            .map(|mut outputs| {
                outputs.remove(output_id);
                outputs.values().any(|state| *state)
            })
            .unwrap_or(false);
        #[cfg(not(test))]
        {
            use tauri::Emitter;
            let _ = self.app_handle.emit("output-window-visible", aggregate);
        }
    }
}

/// The overlay context and all native state belonging to one configured
/// output destination.  There is deliberately no process-global window,
/// render runtime, slot pool, fade state, or "current voice": every output
/// owns its complete visual pipeline.
struct NativeOutputPipeline {
    config: Mutex<OutputPipelineConfig>,
    mpv: MpvHandles,
    current_voice: Arc<Mutex<Option<VoiceId>>>,
    go_sent_at: Arc<Mutex<Option<Instant>>>,
    visible: Arc<AtomicBool>,
    pipeline: Arc<PipelineState>,
    render_runtime: Arc<RenderRuntime>,
    visibility_state: Option<Arc<DisplayVisibilityState>>,
    /// `true` for an NDI/SRT destination.  Its native handle exists solely as
    /// an OpenGL context host and is never shown to the operator.
    network_output: AtomicBool,
    slot_registry: Arc<slot::SlotRegistry>,
    /// The GL render owner.  It is joined before any mpv context is asked to
    /// quit, so libmpv cannot invalidate a context under update/render/report.
    render_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// The overlay mpv client owner.  This is stopped only after the render
    /// thread has unregistered and freed all Render API contexts.
    event_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Serialises the one-way render → mpv shutdown sequence.  `Drop` and a
    /// configuration retirement can both reach it, so the ordering must stay
    /// intact even if they race through different `Arc`s.
    shutdown_lock: Mutex<()>,
    mpv_destroyed: AtomicBool,
}

struct MpvHandles {
    lib: Arc<MpvLib>,
    ctx: Arc<MpvCtx>,
}

impl NativeOutputPipeline {
    /// Acquire a permit for a non-owner call into this pipeline's mpv clients.
    /// A retired pipeline returns `None`, even if another caller still holds
    /// an `Arc` to it.
    fn enter_mpv_call(&self) -> Option<MpvCallPermit<'_>> {
        self.pipeline.enter_mpv_call()
    }

    fn create_live(
        config: OutputPipelineConfig,
        lib: Arc<MpvLib>,
        audio_engine: Arc<AudioEngine>,
        app_handle: &tauri::AppHandle,
        status_tx: &Sender<OutputStatus>,
        network_frame_sink: Option<NetworkFrameSink>,
        visibility_state: Arc<DisplayVisibilityState>,
    ) -> Result<Arc<Self>> {
        let ctx = create_overlay_context(&lib)?;
        let (current_voice, go_sent_at) = (
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
        );
        let pipeline = PipelineState::new(status_tx.clone());
        *pipeline.mpv_lib.lock().unwrap() = Some(Arc::clone(&lib));
        *pipeline.mpv_ctx.lock().unwrap() = Some(Arc::clone(&ctx));
        *pipeline.transform.lock().unwrap() = config.transform;

        let render_runtime = RenderRuntime::new();
        let is_network_output = network_frame_sink.is_some();
        let is_display_output = !is_network_output
            && matches!(
                &config.sink_kind,
                crate::preferences::OutputSinkKind::Display
            );
        let visible = Arc::new(AtomicBool::new(false));
        if is_display_output {
            let visible_callback = Arc::clone(&visible);
            let state_callback = Arc::clone(&visibility_state);
            let output_id = config.id.clone();
            render_runtime.set_visibility_callback(Arc::new(move |shown| {
                visible_callback.store(shown, Ordering::Release);
                state_callback.set(&output_id, shown);
            }));
        }
        render_runtime.set_network_frame_sink(network_frame_sink);
        render_runtime.configure_output_window(
            &config.id,
            config.monitor.is_none(),
            config.floating_window.clone(),
            config.always_on_top,
            config.hide_cursor,
        );
        render_runtime.set_warp(warp::warp_matrix(&config.transform));
        let wake_runtime = Arc::clone(&render_runtime);
        let slot_registry = slot::SlotRegistry::new(
            status_tx.clone(),
            Arc::new(move || wake_runtime.wake()),
        );

        // Start the event loop before publishing the render thread.  Apart
        // from avoiding a usable pipeline with no event consumer, this makes
        // thread-spawn failure recoverable: at this point no detached GL
        // thread owns the context yet, so it is safe to terminate mpv.
        let event_thread = {
            let lib2 = Arc::clone(&lib);
            let ctx2 = Arc::clone(&ctx);
            let voice2 = Arc::clone(&current_voice);
            let tx2 = status_tx.clone();
            let go2 = Arc::clone(&go_sent_at);
            let ae = Arc::clone(&audio_engine);
            let slots2 = Arc::clone(&slot_registry);
            let pipeline2 = Arc::clone(&pipeline);
            let name = format!("qlisa-output-{}-events", config.id);
            std::thread::Builder::new()
                .name(name)
                .spawn(move || {
                    mpv_events::mpv_event_loop(
                        lib2, ctx2, voice2, tx2, go2, ae, slots2, pipeline2,
                    )
                })
                .map_err(|e| anyhow!("output event thread: {e}"))?
        };

        // `render::init` owns the native window and blocks on Windows/Linux
        // until the GL/mpv render context is ready.  If it fails, stop and
        // join the event loop before releasing the context.  The render
        // thread is not published as ready in this failure path.
        let render_thread = match render::init(
            Arc::clone(&render_runtime),
            app_handle,
            Arc::clone(&lib),
            Arc::clone(&ctx),
            Arc::clone(&slot_registry),
            Arc::clone(&pipeline),
        ) {
            Ok(handle) => handle,
            Err(e) => {
                // `render::init` joins a failed worker before returning, so
                // no Render API call can overlap this final client shutdown.
                let quit = cs("quit");
                let args: [*const c_char; 2] = [quit.as_ptr(), std::ptr::null()];
                unsafe { (lib.mpv_command)(ctx.0, args.as_ptr()) };
                let _ = event_thread.join();
                unsafe { (lib.mpv_terminate_destroy)(ctx.0) };
                return Err(e);
            }
        };

        Ok(Arc::new(Self {
            config: Mutex::new(config),
            mpv: MpvHandles { lib, ctx },
            current_voice,
            go_sent_at,
            visible,
            pipeline,
            render_runtime,
            visibility_state: is_display_output.then_some(visibility_state),
            network_output: AtomicBool::new(is_network_output),
            slot_registry,
            render_thread: Mutex::new(Some(render_thread)),
            event_thread: Mutex::new(Some(event_thread)),
            shutdown_lock: Mutex::new(()),
            mpv_destroyed: AtomicBool::new(false),
        }))
    }

    fn config(&self) -> OutputPipelineConfig {
        self.config.lock().map(|c| c.clone()).unwrap_or_else(|_| {
            OutputPipelineConfig {
                id: String::new(),
                name: String::new(),
                sink_kind: crate::preferences::OutputSinkKind::Display,
                monitor: None,
                floating_window: None,
                enabled: false,
                always_on_top: false,
                hide_cursor: false,
                transform: OutputTransform::default(),
                fullscreen_locked: true,
            }
        })
    }

    fn update_config(&self, config: OutputPipelineConfig) {
        if let Ok(mut c) = self.config.lock() {
            *c = config.clone();
        }
        if let Ok(mut t) = self.pipeline.transform.lock() {
            *t = config.transform;
        }
        self.render_runtime.set_warp(warp::warp_matrix(&config.transform));
        self.render_runtime.configure_output_window(
            &config.id,
            config.monitor.is_none(),
            config.floating_window.clone(),
            config.always_on_top,
            config.hide_cursor,
        );
    }

    fn set_network_frame_sink(&self, sink: Option<NetworkFrameSink>) {
        let is_network_output = sink.is_some();
        self.network_output.store(is_network_output, Ordering::Relaxed);
        if is_network_output {
            self.visible.store(false, Ordering::Relaxed);
            self.render_runtime.hide();
            self.render_runtime.set_monitor_capture(false);
        }
        self.render_runtime.set_network_frame_sink(sink);
    }

    /// Position the native window without ever falling back to another
    /// monitor.  `None` means a floating window and therefore does not need a
    /// connected-monitor check.
    fn position(
        &self,
        screens: &[ScreenInfo],
        monitor: Option<u32>,
        health_key: &str,
        show: bool,
    ) -> bool {
        if self.network_output.load(Ordering::Relaxed) {
            // Do not create or commit a visible desktop surface for a network
            // output. The render runtime continues through its hidden FBO.
            self.render_runtime.wake();
            return true;
        }
        let (target, missing) = resolve_output_screen(screens, monitor);
        if missing {
            crate::health::set(crate::health::HealthAlert::new(
                health_key,
                crate::health::HealthLevel::Warning,
                format!(
                    "Output monitor {} is not connected — this output stays hidden. Check Preferences → Display.",
                    monitor.map(|i| i + 1).unwrap_or(0),
                ),
            ));
            self.visible.store(false, Ordering::Relaxed);
            self.render_runtime.hide();
            return false;
        }
        crate::health::clear(health_key);

        match target {
            Some(s) => {
                #[cfg(not(target_os = "macos"))]
                render::set_fullscreen_on_rect(
                    &self.render_runtime,
                    s.x,
                    s.y,
                    s.width,
                    s.height,
                );
                #[cfg(target_os = "macos")]
                render::position_on_screen(&self.render_runtime, s.index);
            }
            None => self.render_runtime.set_windowed_floating(),
        }
        if show {
            self.visible.store(true, Ordering::Relaxed);
            self.render_runtime.show();
        }
        true
    }

    fn hide(&self) {
        self.visible.store(false, Ordering::Relaxed);
        self.render_runtime.hide();
    }

    fn show_for_config(&self, screens: &[ScreenInfo]) -> bool {
        self.position(
            screens,
            self.config().monitor,
            &output_health_key(&self.config().id),
            true,
        )
    }

    fn has_visual_content(&self) -> bool {
        self.slot_registry.all_slots().iter().any(|s| {
            s.state
                .lock()
                .map(|st| st.voice_id.is_some())
                .unwrap_or(false)
        })
    }

    /// Stop all slots and overlay state.  `slot::hard_unload` stops each
    /// paired audio voice before clearing its slot, so this is safe to use
    /// while retiring an output during a preferences update.
    fn hard_stop(&self) {
        if self.mpv_destroyed.load(Ordering::Acquire) {
            return;
        }
        let Some(_permit) = self.enter_mpv_call() else {
            return;
        };
        *self.current_voice.lock().unwrap() = None;
        *self.pipeline.current_voice.lock().unwrap() = None;
        *self.go_sent_at.lock().unwrap() = None;
        *self.pipeline.current_fade_out_ms.lock().unwrap() = 0;
        *self.pipeline.pending_video_start.lock().unwrap() = None;
        *self.pipeline.pending_crop.lock().unwrap() = None;
        *self.pipeline.current_audio_voice.lock().unwrap() = None;

        for slot in self.slot_registry.all_slots() {
            slot::hard_unload(&self.slot_registry, &slot, false);
        }

        unsafe {
            let stop = cs("stop");
            let args: [*const c_char; 2] = [stop.as_ptr(), std::ptr::null()];
            (self.mpv.lib.mpv_command)(self.mpv.ctx.0, args.as_ptr());
        }
        self.pipeline.timer_osd_active.store(false, Ordering::Relaxed);
        self.pipeline.text_overlay_active.store(false, Ordering::Relaxed);
        self.pipeline.test_pattern_active.store(false, Ordering::Relaxed);
        self.pipeline.overlay_has_dummy.store(false, Ordering::Relaxed);
        self.render_runtime.set_text_overlay_active(false);
        self.render_runtime.clear_external_sources();
        self.render_runtime.mark_overlay_dirty();
        fade::set_overlay_alpha(&self.pipeline, 255);
    }

    /// Ask the GL owner to leave and wait until every Render API context has
    /// been freed on that thread.  The runtime wake makes its idle wait at most
    /// one short bounded iteration rather than leaving shutdown to frame flow.
    fn shutdown_render_thread(&self) {
        self.render_runtime.request_shutdown();
        let handle = self.render_thread.lock().ok().and_then(|mut h| h.take());
        if let Some(handle) = handle {
            if handle.join().is_err() {
                log::error!("[output] render thread panicked during shutdown");
            }
        }
    }

    /// Ask the overlay event owner to leave.  Call only after
    /// [`Self::shutdown_render_thread`], because `quit` may invalidate the
    /// mpv client behind the Render API contexts.
    fn shutdown_event_thread(&self) {
        let handle = self.event_thread.lock().ok().and_then(|mut h| h.take());
        if handle.is_none() {
            return;
        }
        unsafe {
            let quit = cs("quit");
            let args: [*const c_char; 2] = [quit.as_ptr(), std::ptr::null()];
            (self.mpv.lib.mpv_command)(self.mpv.ctx.0, args.as_ptr());
            (self.mpv.lib.mpv_wakeup)(self.mpv.ctx.0);
        }
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    /// Retire native resources in the only safe order: close public mpv
    /// callers, stop rendering and free every Render API context on the GL
    /// thread, stop the overlay event owner (which can otherwise operate on
    /// slot clients), then stop/join/destroy each slot client and finally
    /// destroy the overlay client. `self.mpv.lib` remains owned for the whole
    /// sequence, so the function-table backing library cannot unload.
    fn shutdown_native_pipeline(&self) {
        let _shutdown = self
            .shutdown_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // This is the linearisation point for public callers holding an Arc
        // to the pipeline.  Do it before signalling either owner thread: a
        // caller which began earlier drains here; one beginning later is
        // rejected and can never observe the raw context after destruction.
        self.pipeline.close_mpv_calls_and_wait();
        self.shutdown_render_thread();
        // The event owner deliberately does not hold a gate permit across its
        // blocking `mpv_wait_event` loop. Its join below is the owner-side
        // quiescence guarantee, and completes before `terminate_destroy`.
        self.shutdown_event_thread();
        // The overlay event owner can apply hwdec fallbacks across all slot
        // clients, so it must be gone before their shutdown starts. Render
        // cleanup above has already freed every slot render context from the
        // GL thread; each slot method then quit+wakes, joins and destroys its
        // own mpv client without holding registry or slot-state locks.
        self.slot_registry.shutdown_all();
        if !self.mpv_destroyed.swap(true, Ordering::AcqRel) {
            unsafe { (self.mpv.lib.mpv_terminate_destroy)(self.mpv.ctx.0) };
            if let Ok(mut ctx) = self.pipeline.mpv_ctx.lock() {
                *ctx = None;
            }
            if let Ok(mut lib) = self.pipeline.mpv_lib.lock() {
                *lib = None;
            }
        }
    }

    fn retire(&self) {
        self.hard_stop();
        self.hide();
        if let Some(state) = &self.visibility_state {
            state.remove(&self.config().id);
        }
        self.shutdown_native_pipeline();
        if let Ok(mut cfg) = self.config.lock() {
            cfg.enabled = false;
        }
    }
}

impl Drop for NativeOutputPipeline {
    fn drop(&mut self) {
        self.shutdown_native_pipeline();
    }
}

/// Volatile operator state for one configured video destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputControlStatus {
    pub output_id: String,
    pub name: String,
    pub available: bool,
    pub healthy: bool,
    pub ftb: bool,
    /// Whether the destination has a live native/network pipeline.
    pub active: bool,
    /// Whether this native display window is currently shown. Network
    /// destinations always report false and are controlled by their sender.
    pub visible: bool,
    /// The configured physical monitor, when this is a display destination.
    pub monitor: Option<u32>,
    pub network: bool,
    pub detail: Option<String>,
}

/// Read-only facts about one live video slot.  The snapshot is assembled on
/// the diagnostics command thread and never participates in playback.
#[derive(Debug, Clone)]
pub struct VideoRuntimeDiagnostic {
    pub cue_voice_id: VoiceId,
    pub slot_voice_id: VoiceId,
    pub output_id: String,
    pub output_name: String,
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

fn network_output_status_detail(
    status: Option<&crate::engine::network_io::NetworkOutputRuntimeStatus>,
) -> Option<String> {
    match status {
        Some(status) => match status.state {
            crate::engine::network_io::NetworkOutputState::WaitingForFrame
            | crate::engine::network_io::NetworkOutputState::Streaming => None,
            crate::engine::network_io::NetworkOutputState::Error
            | crate::engine::network_io::NetworkOutputState::Stopped
            | crate::engine::network_io::NetworkOutputState::Disabled => status
                .last_error
                .clone()
                .or_else(|| Some(format!("Network output is {:?}", status.state))),
        },
        None => Some("Network worker is not configured".to_owned()),
    }
}

/// Manages the native pipeline for every enabled display output.
pub struct OutputEngine {
    /// Shared dynamic library; every destination has its own mpv context.
    mpv_lib: Option<Arc<MpvLib>>,
    outputs: Mutex<HashMap<String, Arc<NativeOutputPipeline>>>,
    /// Retired pipelines stay alive until the engine is dropped so stale
    /// command references have a stable, already-shut-down owner while output
    /// preference changes settle.
    retired_outputs: Mutex<Vec<Arc<NativeOutputPipeline>>>,
    voices: Mutex<HashMap<VoiceId, OutputVoice>>,
    status_tx: Sender<OutputStatus>,
    status_rx: Mutex<Receiver<OutputStatus>>,
    default_surface_id: SurfaceId,
    audio_engine: Arc<AudioEngine>,
    app_handle: tauri::AppHandle,
    output_registry: Mutex<OutputRegistry>,
    voice_outputs: Mutex<HashMap<VoiceId, String>>,
    /// Audio voices created by live NDI/SRT Camera Cues. Unlike normal video
    /// sound they do not belong to an mpv slot, so output reconfiguration
    /// needs an explicit association in order to stop them.
    cue_audio_voices: Mutex<HashMap<VoiceId, VoiceId>>,
    voice_groups: Mutex<HashMap<VoiceId, Vec<VoiceId>>>,
    voice_group_completed: Mutex<HashMap<VoiceId, HashSet<VoiceId>>>,
    voice_group_completion_emitted: Mutex<HashSet<VoiceId>>,
    /// Serialises full output graph transitions without sharing the network
    /// publisher's short endpoint-map lock.
    network_configure_gate: Mutex<()>,
    display_visibility: Arc<DisplayVisibilityState>,
    network_outputs: Arc<NetworkOutputManager>,
    browser_surface: BrowserSurfaceManager,
}

impl OutputEngine {
    fn resolve_browser_output_id(
        registry: &OutputRegistry,
        requested: Option<&str>,
    ) -> Result<String> {
        registry
            .resolve(requested)
            .ok_or_else(|| anyhow!("Browser output destination is unavailable"))
    }

    fn browser_monitor_for_output(
        statuses: &[OutputControlStatus],
        output_id: Option<&str>,
    ) -> Result<Option<u32>> {
        let Some(output_id) = output_id else {
            return Ok(None);
        };
        let status = statuses
            .iter()
            .find(|status| status.output_id == output_id)
            .ok_or_else(|| anyhow!("Browser output destination is unavailable"))?;
        if status.network {
            anyhow::bail!("Browser cues require a physical display output");
        }
        Ok(status.monitor)
    }

    /// Return the live worker state for every configured NDI/SRT destination.
    /// This is a read-only snapshot intended for Preferences/diagnostics.
    pub fn network_output_statuses(&self) -> Vec<crate::engine::network_io::NetworkOutputRuntimeStatus> {
        self.network_outputs.statuses()
    }

    /// Read-only network diagnostics assembled from existing destination
    /// workers and configured transport settings.
    pub fn network_output_diagnostics(
        &self,
    ) -> Vec<crate::engine::network_io::NetworkOutputDiagnostics> {
        self.network_outputs.diagnostics()
    }

    /// Return current slot/mpv properties for the diagnostics page. Unsupported
    /// mpv properties are represented as `None`; no command or render call is
    /// issued while collecting this snapshot; only read-only property queries
    /// are performed under the existing lifecycle permit.
    pub fn video_runtime_diagnostics(&self) -> Vec<VideoRuntimeDiagnostic> {
        let canonical = self
            .voice_groups
            .lock()
            .map(|groups| {
                groups
                    .iter()
                    .flat_map(|(primary, members)| members.iter().map(move |member| (*member, *primary)))
                    .collect::<HashMap<VoiceId, VoiceId>>()
            })
            .unwrap_or_default();
        let mut result = Vec::new();
        for pipeline in self.active_output_pipelines() {
            let Some(_permit) = pipeline.enter_mpv_call() else {
                continue;
            };
            let config = pipeline.config();
            for slot in pipeline.slot_registry.all_slots() {
                let Some(snapshot) = slot::diagnostics(&slot) else {
                    continue;
                };
                let slot_voice_id = snapshot.voice_id;
                result.push(VideoRuntimeDiagnostic {
                    cue_voice_id: canonical.get(&slot_voice_id).copied().unwrap_or(slot_voice_id),
                    slot_voice_id,
                    output_id: config.id.clone(),
                    output_name: config.name.clone(),
                    mpv_context: snapshot.mpv_context,
                    render_context: snapshot.render_context,
                    file_loaded: snapshot.file_loaded,
                    preloaded: snapshot.preloaded,
                    pending_unload: snapshot.pending_unload,
                    hold_last_frame: snapshot.hold_last_frame,
                    paused: snapshot.paused,
                    eof: snapshot.eof,
                    time_pos_ms: snapshot.time_pos_ms,
                    width: snapshot.width,
                    height: snapshot.height,
                    fps: snapshot.fps,
                    hwdec_backend: snapshot.hwdec_backend,
                    decoder_format: snapshot.decoder_format,
                    dropped_frames: snapshot.dropped_frames,
                    delayed_frames: snapshot.delayed_frames,
                });
            }
        }
        result
    }

    /// Return operator-facing state keyed by stable destination id. FTB is
    /// runtime-only, so a frontend reload cannot clear an active blackout.
    pub fn output_control_statuses(&self) -> Vec<OutputControlStatus> {
        let Ok(_configure_guard) = self.network_configure_gate.lock() else {
            return Vec::new();
        };
        let network_statuses = self.network_outputs.statuses();
        let active = self.outputs.lock().ok();
        let registry = self.output_registry.lock().ok();
        let (Some(active), Some(registry)) = (active, registry) else { return Vec::new() };
        let statuses = registry
            .ids()
            .filter_map(|id| {
                let registered = registry.pipeline(id)?;
                let pipeline = active.get(id);
                let (available, healthy, ftb, active, visible, monitor, network, detail) = if let Some(pipeline) = pipeline {
                    let network = pipeline.network_output.load(Ordering::Relaxed);
                    let ftb = pipeline
                        .pipeline
                        .operator_ftb
                        .lock()
                        .map(|state| state.target_alpha > 0 || state.current_alpha > 0)
                        .unwrap_or(false);
                    let network_detail = if network {
                        network_output_status_detail(
                            network_statuses.iter().find(|status| status.output_id == *id),
                        )
                    } else {
                        None
                    };
                    let destroyed = pipeline.mpv_destroyed.load(Ordering::Acquire);
                    let visible = pipeline.visible.load(Ordering::Acquire);
                    let detail = network_detail.or_else(|| {
                        (!network && !visible).then(|| "Output is hidden".to_owned())
                    });
                    let available = !destroyed && ((network && detail.is_none()) || (!network && visible));
                    let healthy = available && detail.is_none();
                    (available, healthy, ftb, true, visible, pipeline.config().monitor, network, detail)
                } else {
                    (
                        false,
                        false,
                        false,
                        false,
                        false,
                        registered.config.monitor,
                        matches!(
                            &registered.config.sink_kind,
                            crate::preferences::OutputSinkKind::Ndi
                                | crate::preferences::OutputSinkKind::Srt
                        ),
                        Some("Output is not active".to_owned()),
                    )
                };
                Some(OutputControlStatus {
                    output_id: id.clone(),
                    name: registered.config.name.clone(),
                    available,
                    healthy,
                    ftb,
                    active,
                    visible,
                    monitor,
                    network,
                    detail,
                })
            })
            .collect::<Vec<_>>();
        statuses
    }

    /// Toggle the dedicated operator FTB layer for one active destination.
    /// Cue state, media position, and audio routes remain untouched.
    pub fn toggle_output_ftb(&self, output_id: &str) -> Result<bool> {
        let pipeline = self
            .outputs
            .lock()
            .map_err(|_| anyhow!("output map poisoned"))?
            .get(output_id)
            .cloned()
            .ok_or_else(|| anyhow!("Output '{output_id}' is unavailable"))?;
        if pipeline.mpv_destroyed.load(Ordering::Acquire) {
            return Err(anyhow!("Output '{output_id}' is unavailable"));
        }
        let enabled = pipeline
            .pipeline
            .operator_ftb
            .lock()
            .map(|state| state.target_alpha == 0)
            .map_err(|_| anyhow!("output FTB state poisoned"))?;
        fade::set_operator_blackout(&pipeline.pipeline, enabled, 250);
        pipeline.render_runtime.wake();
        Ok(enabled)
    }

    /// Construct the live engine with one legacy-compatible default output.
    pub fn new(audio_engine: Arc<AudioEngine>, app_handle: tauri::AppHandle) -> Result<Self> {
        let lib = Arc::new(MpvLib::load()?);
        #[cfg(not(target_os = "windows"))]
        unsafe {
            libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
        }
        let (status_tx, status_rx) = crossbeam_channel::unbounded();
        init_timer_globals();
        let display_visibility = DisplayVisibilityState::new(app_handle.clone());

        let default_config = OutputPipelineConfig {
            id: "default".into(),
            name: "Main".into(),
            sink_kind: crate::preferences::OutputSinkKind::Display,
            monitor: None,
            floating_window: None,
            enabled: true,
            always_on_top: false,
            hide_cursor: false,
            transform: OutputTransform::default(),
            fullscreen_locked: true,
        };
        let default_pipeline = NativeOutputPipeline::create_live(
            default_config,
            Arc::clone(&lib),
            Arc::clone(&audio_engine),
            &app_handle,
            &status_tx,
            None,
            Arc::clone(&display_visibility),
        )?;
        let mut outputs = HashMap::new();
        outputs.insert("default".into(), default_pipeline);

        Ok(Self {
            mpv_lib: Some(lib),
            outputs: Mutex::new(outputs),
            retired_outputs: Mutex::new(Vec::new()),
            voices: Mutex::new(HashMap::new()),
            status_tx,
            status_rx: Mutex::new(status_rx),
            default_surface_id: Uuid::new_v4(),
            audio_engine,
            app_handle: app_handle.clone(),
            output_registry: Mutex::new(OutputRegistry::legacy_default()),
            voice_outputs: Mutex::new(HashMap::new()),
            cue_audio_voices: Mutex::new(HashMap::new()),
            voice_groups: Mutex::new(HashMap::new()),
            voice_group_completed: Mutex::new(HashMap::new()),
            voice_group_completion_emitted: Mutex::new(HashSet::new()),
            network_configure_gate: Mutex::new(()),
            display_visibility,
            network_outputs: Arc::new(NetworkOutputManager::default()),
            browser_surface: BrowserSurfaceManager::new(app_handle.clone()),
        })
    }

    /// Construct a **headless** engine: no libmpv and no native output
    /// windows.  Destination preferences still reconcile in the pure
    /// registry, so opening/editing a workspace remains possible.
    pub fn new_headless(audio_engine: Arc<AudioEngine>, app_handle: tauri::AppHandle) -> Self {
        let (status_tx, status_rx) = crossbeam_channel::unbounded();
        init_timer_globals();
        let display_visibility = DisplayVisibilityState::new(app_handle.clone());
        Self {
            mpv_lib: None,
            outputs: Mutex::new(HashMap::new()),
            retired_outputs: Mutex::new(Vec::new()),
            voices: Mutex::new(HashMap::new()),
            status_tx,
            status_rx: Mutex::new(status_rx),
            default_surface_id: Uuid::new_v4(),
            audio_engine,
            app_handle: app_handle.clone(),
            output_registry: Mutex::new(OutputRegistry::legacy_default()),
            voice_outputs: Mutex::new(HashMap::new()),
            cue_audio_voices: Mutex::new(HashMap::new()),
            voice_groups: Mutex::new(HashMap::new()),
            voice_group_completed: Mutex::new(HashMap::new()),
            voice_group_completion_emitted: Mutex::new(HashSet::new()),
            network_configure_gate: Mutex::new(()),
            display_visibility,
            network_outputs: Arc::new(NetworkOutputManager::default()),
            browser_surface: BrowserSurfaceManager::new(app_handle.clone()),
        }
    }

    pub fn is_available(&self) -> bool {
        self.mpv_lib.is_some() && !self.outputs.lock().map(|o| o.is_empty()).unwrap_or(true)
    }

    /// Start the single shared Browser WebView. Browser cues are an exclusive
    /// fullscreen visual source in the MVP and do not enter the mpv compositor.
    pub fn start_browser_surface(
        &self,
        cue_id: Uuid,
        url: &str,
        reload_on_go: bool,
        zoom: f64,
        output_id: Option<&str>,
    ) -> Result<()> {
        // Browser cues use the configured default physical destination when
        // they do not carry an explicit route. Resolve that destination
        // before taking the status snapshot so the shared WebView is placed
        // on the selected output monitor instead of the primary desktop.
        let resolved_output_id = {
            let output_registry = self
                .output_registry
                .lock()
                .map_err(|_| anyhow!("output registry poisoned"))?;
            Self::resolve_browser_output_id(&output_registry, output_id)?
        };
        let statuses = self.output_control_statuses();
        let monitor_index =
            Self::browser_monitor_for_output(&statuses, Some(&resolved_output_id))?;
        let monitor = monitor_index
            .map(|index| {
                self.list_screens()
                    .into_iter()
                    .find(|screen| screen.index == index)
                    .ok_or_else(|| anyhow!("Browser output monitor is unavailable"))
            })
            .transpose()?;
        self.browser_surface
            .start(
                cue_id,
                url,
                reload_on_go,
                zoom,
                Some(&resolved_output_id),
                monitor,
            )
    }

    pub fn stop_browser_surface(&self, cue_id: Uuid, hard: bool) -> Result<()> {
        self.browser_surface.stop(cue_id, hard)
    }

    pub fn clear_browser_surface(&self, hard: bool) -> Result<()> {
        if let Some(cue_id) = self.browser_surface_state().cue_id {
            self.browser_surface.stop(cue_id, hard)?;
        }
        Ok(())
    }

    pub fn browser_surface_state(&self) -> BrowserSurfaceState {
        self.browser_surface.state()
    }

    pub fn try_mpv_lib(&self) -> Option<&MpvLib> {
        self.mpv_lib.as_deref()
    }

    pub fn try_mpv_lib_arc(&self) -> Option<Arc<MpvLib>> {
        self.mpv_lib.as_ref().map(Arc::clone)
    }

    fn default_pipeline(&self) -> Option<Arc<NativeOutputPipeline>> {
        let id = self.output_registry.lock().ok()?.default_id()?;
        self.outputs.lock().ok()?.get(&id).cloned()
    }

    fn pipeline_for_output(&self, requested: Option<&str>) -> Option<(String, Arc<NativeOutputPipeline>)> {
        let id = self.output_registry.lock().ok()?.resolve(requested)?;
        let pipeline = self.outputs.lock().ok()?.get(&id).cloned()?;
        Some((id, pipeline))
    }

    fn pipeline_for_voice(&self, voice_id: VoiceId) -> Option<Arc<NativeOutputPipeline>> {
        let id = self.voice_outputs.lock().ok()?.get(&voice_id).cloned()?;
        self.outputs.lock().ok()?.get(&id).cloned()
    }

    fn all_pipelines(&self) -> Vec<Arc<NativeOutputPipeline>> {
        let mut out = self
            .outputs
            .lock()
            .map(|m| m.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        if let Ok(retired) = self.retired_outputs.lock() {
            out.extend(retired.iter().cloned());
        }
        out
    }

    /// Probe media metadata without displaying the file. This runs on a
    /// background worker; creating a throwaway mpv context must never happen
    /// in cue-list summary construction.
    pub fn probe_media_info(lib: &MpvLib, path: &Path) -> Option<MediaProbe> {
        Self::probe_media_info_inner(lib, path, true)
    }

    fn probe_media_info_inner(
        lib: &MpvLib,
        path: &Path,
        wait_for_dimensions: bool,
    ) -> Option<MediaProbe> {
        unsafe {
            let ctx = (lib.mpv_create)();
            if ctx.is_null() {
                return None;
            }

            opt_str(lib, ctx, "vo", "null");
            opt_str(lib, ctx, "ao", "null");
            opt_str(lib, ctx, "pause", "yes");
            opt_str(lib, ctx, "hwdec", "no");

            if (lib.mpv_initialize)(ctx) < 0 {
                (lib.mpv_terminate_destroy)(ctx);
                return None;
            }

            let path_str = path.to_string_lossy().replace('\\', "/");
            let path_cstr = match CString::new(path_str.as_str()) {
                Ok(c) => c,
                Err(_) => {
                    (lib.mpv_terminate_destroy)(ctx);
                    return None;
                }
            };
            let cmd_cstr = cs("loadfile");
            let replace_cstr = cs("replace");
            let args: [*const std::ffi::c_char; 4] = [
                cmd_cstr.as_ptr(),
                path_cstr.as_ptr(),
                replace_cstr.as_ptr(),
                std::ptr::null(),
            ];
            (lib.mpv_command)(ctx, args.as_ptr());

            use super::mpv_sys::{
                MPV_EVENT_END_FILE, MPV_EVENT_FILE_LOADED, MPV_EVENT_PLAYBACK_RESTART,
                MPV_EVENT_SHUTDOWN, MPV_EVENT_VIDEO_RECONFIG, MPV_FORMAT_DOUBLE,
            };
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut duration_secs: Option<f64> = None;
            let mut width: Option<u32> = None;
            let mut height: Option<u32> = None;
            let mut file_loaded = false;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let timeout = remaining.as_secs_f64().max(0.01);
                let event = (lib.mpv_wait_event)(ctx, timeout);
                if event.is_null() {
                    break;
                }
                let event_id = (*event).event_id;
                if matches!(
                    event_id,
                    MPV_EVENT_FILE_LOADED
                        | MPV_EVENT_VIDEO_RECONFIG
                        | MPV_EVENT_PLAYBACK_RESTART
                ) {
                    if event_id == MPV_EVENT_FILE_LOADED {
                        file_loaded = true;
                        let mut val: f64 = 0.0;
                        let name = cs("duration");
                        let ret = (lib.mpv_get_property)(
                            ctx,
                            name.as_ptr(),
                            MPV_FORMAT_DOUBLE,
                            &mut val as *mut f64 as *mut c_void,
                        );
                        if ret == 0 && val > 0.0 {
                            duration_secs = Some(val);
                        }
                    }

                    width = get_prop_i64(lib, ctx, "video-params/w")
                        .filter(|value| *value > 0)
                        .and_then(|value| u32::try_from(value).ok())
                        .or(width);
                    height = get_prop_i64(lib, ctx, "video-params/h")
                        .filter(|value| *value > 0)
                        .and_then(|value| u32::try_from(value).ok())
                        .or(height);

                    if file_loaded
                        && (!wait_for_dimensions || (width.is_some() && height.is_some()))
                    {
                        break;
                    }
                }
                if event_id == MPV_EVENT_SHUTDOWN || event_id == MPV_EVENT_END_FILE {
                    break;
                }
                if Instant::now() >= deadline {
                    break;
                }
            }

            (lib.mpv_terminate_destroy)(ctx);
            let duration = duration_secs.map(|s| Duration::from_millis((s * 1000.0) as u64));
            (duration.is_some() || width.is_some() || height.is_some()).then_some(MediaProbe {
                duration,
                width,
                height,
            })
        }
    }

    /// Backwards-compatible duration-only wrapper used by the existing video
    /// preload and filmstrip paths. It returns at FILE_LOADED exactly as the
    /// old implementation did; it does not wait for dimensions.
    pub fn probe_duration(lib: &MpvLib, path: &Path) -> Option<Duration> {
        Self::probe_media_info_inner(lib, path, false).and_then(|probe| probe.duration)
    }

    /// Enumerate all connected monitors.  Index 0 is always the primary.
    pub fn list_screens(&self) -> Vec<ScreenInfo> {
        #[cfg(target_os = "windows")]
        {
            let mut screens: Vec<ScreenInfo> = Vec::new();
            unsafe {
                use windows_sys::Win32::Graphics::Gdi::{
                    EnumDisplayMonitors, GetMonitorInfoW, MONITORINFO,
                };
                extern "system" fn cb(
                    hmon: windows_sys::Win32::Graphics::Gdi::HMONITOR,
                    _hdc: windows_sys::Win32::Graphics::Gdi::HDC,
                    _rect: *mut windows_sys::Win32::Foundation::RECT,
                    data: windows_sys::Win32::Foundation::LPARAM,
                ) -> windows_sys::Win32::Foundation::BOOL {
                    unsafe {
                        let list = &mut *(data as *mut Vec<ScreenInfo>);
                        let mut mi: MONITORINFO = std::mem::zeroed();
                        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                        if GetMonitorInfoW(hmon, &mut mi) != 0 {
                            let r = mi.rcMonitor;
                            let is_primary = (mi.dwFlags & 1) != 0;
                            list.push(ScreenInfo {
                                index: list.len() as u32,
                                width: (r.right - r.left) as u32,
                                height: (r.bottom - r.top) as u32,
                                x: r.left,
                                y: r.top,
                                is_primary,
                            });
                        }
                        1
                    }
                }
                EnumDisplayMonitors(
                    0,
                    std::ptr::null(),
                    Some(cb),
                    &mut screens as *mut Vec<ScreenInfo> as isize,
                );
            }
            screens.sort_by(|a, b| b.is_primary.cmp(&a.is_primary).then(a.x.cmp(&b.x)));
            for (i, s) in screens.iter_mut().enumerate() {
                s.index = i as u32;
            }
            screens
        }

        #[cfg(not(target_os = "windows"))]
        {
            use tauri::Manager;
            // Enumerate via the main Tauri window — available on the calling thread.
            let win = self.app_handle.get_webview_window("main");
            let Some(win) = win else {
                return Vec::new();
            };

            let all = win.available_monitors().unwrap_or_default();
            let primary_pos = win.primary_monitor().ok().flatten().map(|p| *p.position());

            let mut screens: Vec<ScreenInfo> = all
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let pos = m.position();
                    let sz = m.size();
                    let is_primary = primary_pos
                        .map(|pp| pp.x == pos.x && pp.y == pos.y)
                        .unwrap_or(i == 0);
                    ScreenInfo {
                        index: i as u32,
                        width: sz.width,
                        height: sz.height,
                        x: pos.x,
                        y: pos.y,
                        is_primary,
                    }
                })
                .collect();
            screens.sort_by(|a, b| b.is_primary.cmp(&a.is_primary).then(a.x.cmp(&b.x)));
            for (i, s) in screens.iter_mut().enumerate() {
                s.index = i as u32;
            }
            screens
        }
    }

    /// The ID of the default "Screen 1" surface.
    pub fn default_surface_id(&self) -> SurfaceId {
        self.default_surface_id
    }

    /// Snapshot of all configured output surfaces.  The pure registry keeps
    /// disabled entries visible to Preferences, while the native map contains
    /// only enabled display pipelines.
    pub fn surfaces(&self) -> Vec<OutputSurface> {
        let reg = self.output_registry.lock().unwrap();
        let mut surfaces: Vec<_> = reg
            .ids()
            .filter_map(|id| {
                reg.pipeline(id).map(|p| OutputSurface {
                    id: stable_surface_id(id),
                    name: p.config.name.clone(),
                    label: id.clone(),
                })
            })
            .collect();
        if surfaces.is_empty() {
            surfaces.push(OutputSurface {
                id: self.default_surface_id,
                name: "Screen 1".into(),
                label: String::new(),
            });
        }
        surfaces
    }

    /// Validate and reconcile named destinations.  New native contexts are
    /// created before the active map is replaced and never while its mutex is
    /// held; a failed creation leaves the old routing untouched.
    pub fn configure_outputs(
        &self,
        destinations: &[crate::preferences::OutputDestination],
        default_id: &str,
    ) -> Result<()> {
        let _configure_guard = self
            .network_configure_gate
            .lock()
            .map_err(|_| anyhow!("output configure gate poisoned"))?;
        validate_output_destinations(destinations, default_id)?;

        // Build workers before any compositor can publish a frame.  Each
        // named NDI/SRT destination owns exactly one worker and one hidden
        // compositor pipeline; display destinations deliberately stay out of
        // this list.
        let network_configs: Vec<_> = destinations
            .iter()
            .filter(|destination| destination.enabled)
            .map(network_output_config)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        let audio_sample_rate = self.audio_engine.sample_rate();
        let active_before = self.outputs.lock().map(|m| m.clone()).unwrap_or_default();
        let active_configs: HashMap<_, _> = active_before
            .iter()
            .map(|(id, pipeline)| (id.clone(), pipeline.config()))
            .collect();
        let network_configuration_current = self
            .network_outputs
            .is_current_configuration(&network_configs, audio_sample_rate);

        // An Apply with an identical graph must not touch any live resource.
        // In particular, replacing audio taps or assigning a fresh frame sink
        // can create a transient no-frame interval visible to vMix even when
        // the destination settings did not change.  Registry metadata is still
        // refreshed because disabled destinations/default routing are pure
        // state and do not affect the native graph.
        if output_graph_is_unchanged(
            destinations,
            &active_configs,
            network_configuration_current,
        ) {
            self.output_registry
                .lock()
                .map_err(|_| anyhow!("output registry poisoned"))?
                .update(destinations, default_id);
            return Ok(());
        }

        if !network_configuration_current {
            let program_audio = self
                .audio_engine
                .replace_network_audio_taps(network_configs.len());
            self.network_outputs
                .configure_with_program_audio(&network_configs, program_audio, audio_sample_rate)
                .map_err(|error| anyhow!("network output configuration: {error}"))?;
        }

        let mut created = HashMap::new();
        if let Some(lib) = &self.mpv_lib {
            for destination in destinations.iter().filter(|d| {
                d.enabled
            }) {
                if active_before.contains_key(&destination.id) {
                    continue;
                }
                let config = OutputPipelineConfig::from_destination(destination);
                let pipeline = NativeOutputPipeline::create_live(
                    config,
                    Arc::clone(lib),
                    Arc::clone(&self.audio_engine),
                    &self.app_handle,
                    &self.status_tx,
                    network_output_config(destination)?
                        .map(|_| self.network_outputs.frame_sink(destination.id.clone())),
                    Arc::clone(&self.display_visibility),
                )?;
                created.insert(destination.id.clone(), pipeline);
            }
        }

        let wanted_ids: HashSet<String> = destinations
            .iter()
            .filter(|d| d.enabled)
            .map(|d| d.id.clone())
            .collect();
        let mut next = HashMap::new();
        for destination in destinations.iter().filter(|d| d.enabled) {
            if let Some(pipeline) = active_before.get(&destination.id) {
                pipeline.update_config(OutputPipelineConfig::from_destination(destination));
                pipeline.set_network_frame_sink(
                    network_output_config(destination)?
                        .map(|_| self.network_outputs.frame_sink(destination.id.clone())),
                );
                next.insert(destination.id.clone(), Arc::clone(pipeline));
            } else if let Some(pipeline) = created.remove(&destination.id) {
                next.insert(destination.id.clone(), pipeline);
            }
        }

        let retired: Vec<_> = {
            let mut active = self.outputs.lock().map_err(|_| anyhow!("output map poisoned"))?;
            let old = std::mem::replace(&mut *active, next);
            old.into_iter()
                .filter_map(|(id, pipeline)| (!wanted_ids.contains(&id)).then_some(pipeline))
                .collect()
        };

        // Update pure routing only after all required native contexts exist.
        // A valid default is guaranteed by validation; disabled/non-display
        // entries remain in the registry for the preferences UI.
        self.output_registry
            .lock()
            .map_err(|_| anyhow!("output registry poisoned"))?
            .update(destinations, default_id);

        let retired_ids: HashSet<String> = retired.iter().map(|p| p.config().id).collect();
        let mut affected_voices = HashSet::new();
        for pipeline in &retired {
            for slot in pipeline.slot_registry.all_slots() {
                if let Ok(state) = slot.state.lock() {
                    if let Some(voice) = state.voice_id {
                        affected_voices.insert(voice);
                    }
                }
            }
            pipeline.retire();
        }

        self.output_registry
            .lock()
            .map_err(|_| anyhow!("output registry poisoned"))?
            .release_outputs(&retired_ids);
        {
            let owners = self.voice_outputs.lock().map_err(|_| anyhow!("voice map poisoned"))?;
            affected_voices.extend(
                owners
                    .iter()
                    .filter(|(_, id)| retired_ids.contains(*id))
                    .map(|(voice, _)| *voice),
            );
        }

        // A grouped cue is indivisible: if one destination disappears, stop
        // surviving members too and dissolve the group bookkeeping.
        let retired_voice_ids = affected_voices.clone();
        let groups_to_terminate: Vec<(VoiceId, Vec<VoiceId>)> = self
            .voice_groups
            .lock()
            .map_err(|_| anyhow!("voice groups poisoned"))?
            .iter()
            .filter(|(_, members)| members.iter().any(|voice| retired_voice_ids.contains(voice)))
            .map(|(primary, members)| (*primary, members.clone()))
            .collect();
        for (primary, members) in groups_to_terminate {
            self.voice_groups.lock().map_err(|_| anyhow!("voice groups poisoned"))?.remove(&primary);
            self.voice_group_completed.lock().map_err(|_| anyhow!("voice completion state poisoned"))?.remove(&primary);
            self.voice_group_completion_emitted.lock().map_err(|_| anyhow!("voice completion state poisoned"))?.remove(&primary);
            affected_voices.extend(members.iter().copied());
            for member in members {
                if retired_voice_ids.contains(&member) {
                    self.release_voice_bookkeeping_single(member, true);
                } else {
                    self.stop_content_single(member, 0, 0);
                }
            }
        }
        self.voices
            .lock()
            .map_err(|_| anyhow!("voice map poisoned"))?
            .retain(|voice, _| !affected_voices.contains(voice));
        self.voice_outputs
            .lock()
            .map_err(|_| anyhow!("voice map poisoned"))?
            .retain(|voice, id| !affected_voices.contains(voice) && wanted_ids.contains(id));

        if !retired.is_empty() {
            self.retired_outputs
                .lock()
                .map_err(|_| anyhow!("retired output map poisoned"))?
                .extend(retired);
        }

        let screens = self.list_screens();
        for pipeline in self.active_output_pipelines() {
            // Reassert placement after an update.  Existing visible outputs
            // move immediately; new outputs remain hidden until GO/F9.
            let visible = pipeline.visible.load(Ordering::Relaxed);
            let cfg = pipeline.config();
            pipeline.position(&screens, cfg.monitor, &output_health_key(&cfg.id), visible);
        }
        Ok(())
    }

    /// Backwards-compatible command name used by Preferences.
    pub fn sync_output_destinations(
        &self,
        destinations: &[crate::preferences::OutputDestination],
        default_id: &str,
    ) -> Result<()> {
        self.configure_outputs(destinations, default_id)
    }

    /// Move one enabled physical display output to a connected monitor.
    ///
    /// This is deliberately separate from [`Self::configure_outputs`]. A
    /// monitor picker is a placement operation, not an output-graph apply:
    /// NDI/SRT workers and their frame sinks must remain completely untouched.
    pub fn set_display_output_monitor(
        &self,
        output_id: &str,
        monitor: Option<u32>,
    ) -> Result<()> {
        self.set_display_output_monitors(&[(output_id.to_owned(), monitor)])
    }

    /// Move several physical display outputs atomically. This is used for a
    /// monitor assignment swap, where both live windows and the pure registry
    /// must change under one output-graph gate.
    pub fn set_display_output_monitors(
        &self,
        assignments: &[(String, Option<u32>)],
    ) -> Result<()> {
        self.set_display_output_monitors_inner(assignments, true)
    }

    /// Restore previous monitor assignments after a failed Preferences write.
    /// The old monitor may no longer be connected, so this path intentionally
    /// skips connected-monitor validation.
    pub(crate) fn restore_display_output_monitors(
        &self,
        assignments: &[(String, Option<u32>)],
    ) -> Result<()> {
        self.set_display_output_monitors_inner(assignments, false)
    }

    fn set_display_output_monitors_inner(
        &self,
        assignments: &[(String, Option<u32>)],
        require_connected: bool,
    ) -> Result<()> {
        let _configure_guard = self
            .network_configure_gate
            .lock()
            .map_err(|_| anyhow!("output configure gate poisoned"))?;
        if assignments.is_empty() {
            return Err(anyhow!("At least one display output assignment is required"));
        }
        let screens = self.list_screens();
        if require_connected {
            for (_, monitor) in assignments {
                let Some(index) = monitor else {
                    continue;
                };
                if !screens.iter().any(|screen| screen.index == *index) {
                    return Err(anyhow!("Monitor {} is not connected", index + 1));
                }
            }
        }

        let active = self
            .outputs
            .lock()
            .map_err(|_| anyhow!("output map poisoned"))?;
        let mut pipelines = Vec::with_capacity(assignments.len());
        for (output_id, monitor) in assignments {
            let pipeline = active
                .get(output_id)
                .cloned()
                .ok_or_else(|| anyhow!("Output '{output_id}' is unavailable"))?;
            let old_config = pipeline.config();
            if !old_config.enabled
                || !matches!(
                    &old_config.sink_kind,
                    crate::preferences::OutputSinkKind::Display
                )
                || pipeline.network_output.load(Ordering::Acquire)
            {
                return Err(anyhow!("Output '{output_id}' is not a physical display"));
            }
            let was_visible = pipeline.visible.load(Ordering::Acquire);
            pipelines.push((output_id.clone(), pipeline, old_config, *monitor, was_visible));
        }
        drop(active);

        for (_, pipeline, _, monitor, _) in &pipelines {
            let mut next_config = pipeline.config();
            next_config.monitor = *monitor;
            pipeline.update_config(next_config);
        }

        let rollback_runtime = || {
            let mut restore_errors = Vec::new();
            for (_, pipeline, old_config, _, was_visible) in &pipelines {
                pipeline.update_config(old_config.clone());
                if !pipeline.position(
                    &screens,
                    old_config.monitor,
                    &output_health_key(&old_config.id),
                    *was_visible,
                ) {
                    restore_errors.push(old_config.id.clone());
                }
            }
            restore_errors
        };

        let mut positioned = true;
        for (output_id, pipeline, _, monitor, was_visible) in &pipelines {
            if !pipeline.position(
                &screens,
                *monitor,
                &output_health_key(output_id),
                *was_visible,
            ) {
                positioned = false;
                break;
            }
        }
        if require_connected && !positioned {
            let restore_errors = rollback_runtime();
            return Err(if restore_errors.is_empty() {
                anyhow!("Monitor assignment failed")
            } else {
                anyhow!(
                    "Monitor assignment failed; runtime rollback failed for: {}",
                    restore_errors.join(", ")
                )
            });
        }

        let registry_updated = match self.output_registry.lock() {
            Ok(mut registry) => registry.set_monitors(assignments),
            Err(_) => {
                let restore_errors = rollback_runtime();
                return Err(if restore_errors.is_empty() {
                    anyhow!("output registry poisoned")
                } else {
                    anyhow!(
                        "output registry poisoned; runtime rollback failed for: {}",
                        restore_errors.join(", ")
                    )
                });
            }
        };
        if !registry_updated {
            let restore_errors = rollback_runtime();
            return Err(if restore_errors.is_empty() {
                anyhow!("One of the display outputs is not registered")
            } else {
                anyhow!(
                    "One of the display outputs is not registered; runtime rollback failed for: {}",
                    restore_errors.join(", ")
                )
            });
        }
        Ok(())
    }

    pub fn output_id_for_voice(&self, voice_id: VoiceId) -> Option<String> {
        self.voice_outputs.lock().ok()?.get(&voice_id).cloned()
    }

    /// Select the only output whose final compositor image is tapped by the
    /// operator monitor. `None` disables every tap immediately.
    pub fn set_output_monitor_source(&self, source_id: Option<&str>) -> Result<()> {
        let pipelines = self.active_output_pipelines();
        if let Some(source_id) = source_id {
            let selected = pipelines
                .iter()
                .find(|pipeline| pipeline.config().id == source_id)
                .ok_or_else(|| anyhow!("Output monitor source '{source_id}' is unavailable"))?;
            if selected.network_output.load(Ordering::Relaxed) {
                return Err(anyhow!("Output monitor supports display destinations only"));
            }
        }
        for pipeline in pipelines {
            let active = source_id
                .map(|source_id| pipeline.config().id == source_id)
                .unwrap_or(false);
            pipeline.render_runtime.set_monitor_capture(active);
        }
        Ok(())
    }

    pub fn output_monitor_frame(
        &self,
        source_id: &str,
        after_sequence: u64,
    ) -> Result<OutputMonitorFrameRead> {
        let pipeline = self
            .outputs
            .lock()
            .map_err(|_| anyhow!("output map poisoned"))?
            .get(source_id)
            .cloned()
            .ok_or_else(|| anyhow!("Output monitor source '{source_id}' is unavailable"))?;
        if pipeline.network_output.load(Ordering::Relaxed)
            || !pipeline.render_runtime.monitor_capture_enabled()
        {
            return Err(anyhow!("Output monitor source '{source_id}' is not active"));
        }
        Ok(match pipeline.render_runtime.monitor_frame_after(after_sequence) {
            Some((sequence, Some(frame))) => OutputMonitorFrameRead::Frame { sequence, frame },
            Some((sequence, None)) => OutputMonitorFrameRead::Unchanged { sequence },
            None => OutputMonitorFrameRead::NoFrame,
        })
    }

    // ── Unified content display ──────────────────────────────────────────────

    /// Display video, image, or a live feed on a named output.  A voice is
    /// claimed before native work begins and all failures roll the claim back.
    pub fn show_content(&self, req: ContentRequest<'_>) -> Result<VoiceId> {
        let Some((output_id, output)) = self.pipeline_for_output(req.output_id) else {
            return Err(match req.output_id {
                Some(id) => anyhow!("Output destination '{id}' is unavailable"),
                None => anyhow!("{NO_VIDEO_OUTPUT}"),
            });
        };
        let voice_id = Uuid::new_v4();

        if !self
            .output_registry
            .lock()
            .map_err(|_| anyhow!("output registry poisoned"))?
            .claim_voice(voice_id, &output_id)
        {
            return Err(anyhow!("Output destination '{output_id}' is unavailable"));
        }
        self.voice_outputs
            .lock()
            .map_err(|_| anyhow!("voice map poisoned"))?
            .insert(voice_id, output_id.clone());
        self.voices
            .lock()
            .map_err(|_| anyhow!("voice map poisoned"))?
            .insert(
                voice_id,
                OutputVoice {
                    id: voice_id,
                    started_at: Instant::now(),
                    duration: None,
                },
            );

        // A named output owns its monitor.  Legacy screen_index is consulted
        // only for callers that omitted output_id (old workspace/cue path).
        let cfg = output.config();
        let monitor = if req.output_id.is_some() {
            cfg.monitor
        } else {
            req.screen_index.or(cfg.monitor)
        };
        let screens = self.list_screens();
        if !output.position(&screens, monitor, &output_health_key(&cfg.id), true) {
            // A configured monitor that disappeared must never cause content
            // to be loaded onto another screen.  Roll back the ownership claim
            // before returning so the cue can be retried after reconnection.
            self.release_voice_bookkeeping(voice_id, true);
            return Err(anyhow!(
                "Output destination '{}' has no connected monitor",
                cfg.id
            ));
        }
        // Hold one permit across slot allocation and load.  Those helpers
        // make several raw mpv calls, and an output can otherwise be retired
        // after we obtained its Arc but before the load reaches libmpv.
        let Some(_permit) = output.enter_mpv_call() else {
            self.release_voice_bookkeeping(voice_id, true);
            return Err(anyhow!("Output destination '{output_id}' is retiring"));
        };
        fade::set_overlay_alpha(&output.pipeline, 0);

        let lib = Arc::clone(&output.mpv.lib);
        let slot = match slot::acquire_slot(&output.slot_registry, &lib, &self.audio_engine) {
            Ok(slot) => slot,
            Err(e) => {
                self.release_voice_bookkeeping(voice_id, true);
                return Err(e);
            }
        };
        slot::load_into_slot(
            &slot,
            slot::SlotLoad {
                voice_id,
                audio_voice_id: req.audio_voice_id,
                url: req.file_path.to_string_lossy().replace('\\', "/"),
                is_image: req.is_image,
                fade_in_ms: req.fade_in_ms,
                loop_count: req.loop_count,
                initial_seek_action_ms: req.initial_seek_action_ms,
                start_ms: req.start_ms,
                end_ms: req.end_ms,
                display_duration_ms: req.display_duration_ms,
                hold_last_frame: req.hold_last_frame,
                live_source: req.live_source,
                geometry: req.geometry,
                layer_style: req.layer_style,
                slices: req.slices,
                preload: req.preload,
            },
        );
        Ok(voice_id)
    }

    /// Display one cue on several destinations. The first voice is canonical;
    /// lifecycle operations on it are fanned out by the engine.
    pub fn show_content_multi(&self, req: ContentRequest<'_>, output_ids: &[String]) -> Result<VoiceId> {
        let ids: Vec<&str> = output_ids.iter().map(String::as_str).filter(|id| !id.trim().is_empty()).fold(Vec::new(), |mut ids, id| { if !ids.contains(&id) { ids.push(id); } ids });
        if ids.is_empty() { return self.show_content(req); }
        let mut voices = Vec::with_capacity(ids.len());
        for id in ids {
            let audio_voice_id = if voices.is_empty() { req.audio_voice_id } else { None };
            let voice = match self.show_content(ContentRequest {
                file_path: req.file_path, is_image: req.is_image, fade_in_ms: req.fade_in_ms,
                loop_count: req.loop_count, start_ms: req.start_ms, end_ms: req.end_ms,
                initial_seek_action_ms: req.initial_seek_action_ms,
                screen_index: None, output_id: Some(id), audio_voice_id,
                display_duration_ms: req.display_duration_ms, hold_last_frame: req.hold_last_frame,
                geometry: req.geometry, live_source: req.live_source, layer_style: req.layer_style,
                slices: req.slices.clone(), preload: req.preload,
            }) {
                Ok(voice) => voice,
                Err(error) => {
                    for previous in voices { self.stop_content(previous, 0, 0); }
                    return Err(error);
                }
            };
            voices.push(voice);
        }
        let primary = voices[0];
        self.voice_groups.lock().map_err(|_| anyhow!("voice groups poisoned"))?.insert(primary, voices);
        self.voice_group_completed.lock().map_err(|_| anyhow!("voice completion state poisoned"))?.insert(primary, HashSet::new());
        Ok(primary)
    }

    /// Display a live external BGRA source (currently the NDI receiver) on a
    /// named destination.  The receiver remains outside the output engine;
    /// only its bounded mailbox crosses into the render thread.
    pub fn show_external_bgra_source(
        &self,
        output_id: Option<&str>,
        source: Arc<crate::engine::network_io::BgraFrameMailbox>,
        geometry: VideoGeometry,
        layer_style: LayerStyle,
        fade_in_ms: u32,
    ) -> Result<VoiceId> {
        let Some((resolved_id, output)) = self.pipeline_for_output(output_id) else {
            return Err(match output_id {
                Some(id) => anyhow!("Output destination '{id}' is unavailable"),
                None => anyhow!("{NO_VIDEO_OUTPUT}"),
            });
        };
        let voice_id = Uuid::new_v4();
        if !self.output_registry.lock().map_err(|_| anyhow!("output registry poisoned"))?
            .claim_voice(voice_id, &resolved_id)
        {
            return Err(anyhow!("Output destination '{resolved_id}' is unavailable"));
        }
        self.voice_outputs.lock().map_err(|_| anyhow!("voice map poisoned"))?
            .insert(voice_id, resolved_id.clone());
        self.voices.lock().map_err(|_| anyhow!("voice map poisoned"))?
            .insert(voice_id, OutputVoice { id: voice_id, started_at: Instant::now(), duration: None });

        let config = output.config();
        let screens = self.list_screens();
        if !output.position(&screens, config.monitor, &output_health_key(&config.id), true) {
            self.release_voice_bookkeeping(voice_id, true);
            return Err(anyhow!("Output destination '{}' has no connected monitor", config.id));
        }
        fade::set_overlay_alpha(&output.pipeline, 0);
        let layer_key = slot::resolve_layer_key(layer_style.layer, output.slot_registry.next_layer_seq());
        output.render_runtime.add_external_source(voice_id, source, geometry, layer_style, layer_key, fade_in_ms);
        Ok(voice_id)
    }

    pub fn show_external_bgra_source_multi(
        &self, output_id: Option<&str>, output_ids: &[String], source: Arc<crate::engine::network_io::BgraFrameMailbox>,
        geometry: VideoGeometry, layer_style: LayerStyle, fade_in_ms: u32,
    ) -> Result<VoiceId> {
        let ids: Vec<&str> = output_ids.iter().map(String::as_str).filter(|id| !id.trim().is_empty()).fold(Vec::new(), |mut ids, id| { if !ids.contains(&id) { ids.push(id); } ids });
        if ids.is_empty() { return self.show_external_bgra_source(output_id, source, geometry, layer_style, fade_in_ms); }
        let mut voices = Vec::with_capacity(ids.len());
        for id in ids {
            match self.show_external_bgra_source(Some(id), Arc::clone(&source), geometry, layer_style, fade_in_ms) {
                Ok(voice) => voices.push(voice),
                Err(error) => {
                    for previous in voices { self.stop_content(previous, 0, 0); }
                    return Err(error);
                }
            }
        }
        let primary = voices[0];
        self.voice_groups.lock().map_err(|_| anyhow!("voice groups poisoned"))?.insert(primary, voices);
        self.voice_group_completed.lock().map_err(|_| anyhow!("voice completion state poisoned"))?.insert(primary, HashSet::new());
        Ok(primary)
    }

    /// Pair a live cue's synthetic-input audio voice with its visual voice.
    /// This allows output reconfiguration to stop the pair together even
    /// though the audio was not created by an mpv slot.
    pub fn attach_cue_audio_voice(&self, visual_voice_id: VoiceId, audio_voice_id: VoiceId) {
        if self.is_current_voice(visual_voice_id) {
            if let Ok(mut voices) = self.cue_audio_voices.lock() {
                voices.insert(visual_voice_id, audio_voice_id);
            }
        } else {
            let _ = self.audio_engine.stop_voice(
                audio_voice_id,
                0,
                crate::engine::ring_command::FadeCurve::Linear,
            );
        }
    }

    pub fn devamp_voice(&self, voice_id: VoiceId, stop_at_end: bool) {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|m| m.get(&voice_id).cloned()) {
            for member in members { self.devamp_voice_single(member, stop_at_end); }
            return;
        }
        self.devamp_voice_single(voice_id, stop_at_end);
    }

    fn devamp_voice_single(&self, voice_id: VoiceId, stop_at_end: bool) {
        if let Some(output) = self.pipeline_for_voice(voice_id) {
            let Some(_permit) = output.enter_mpv_call() else { return };
            if let Some(slot) = slot::slot_for_voice(&output.slot_registry, voice_id) {
                slot::devamp_slot(&slot, stop_at_end);
            }
        }
    }

    pub fn voice_position_ms(&self, voice_id: VoiceId) -> Option<u64> {
        let output = self.pipeline_for_voice(voice_id)?;
        let _permit = output.enter_mpv_call()?;
        let slot = slot::slot_for_voice(&output.slot_registry, voice_id)?;
        slot::position_ms(&slot)
    }

    /// Resolve the owner and slot before dropping ownership.  This ordering
    /// matters when a stop arrives concurrently with a destination update.
    pub fn stop_content(&self, voice_id: VoiceId, visual_fade_ms: u32, audio_fade_ms: u32) {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|mut m| m.remove(&voice_id)) {
            self.voice_group_completed.lock().ok().map(|mut m| { m.remove(&voice_id); });
            self.voice_group_completion_emitted.lock().ok().map(|mut m| { m.remove(&voice_id); });
            for member in members { self.stop_content_single(member, visual_fade_ms, audio_fade_ms); }
            return;
        }
        self.stop_content_single(voice_id, visual_fade_ms, audio_fade_ms);
    }

    fn stop_content_single(&self, voice_id: VoiceId, visual_fade_ms: u32, audio_fade_ms: u32) {
        let output = self.pipeline_for_voice(voice_id);
        let is_external = output.as_ref().is_some_and(|p| p.render_runtime.stop_external_source(voice_id, visual_fade_ms));
        let slot = output
            .as_ref()
            .and_then(|p| slot::slot_for_voice(&p.slot_registry, voice_id));
        let audio_id = slot
            .as_ref()
            .and_then(|s| s.state.lock().ok().and_then(|st| st.audio_voice_id));

        if let Some(slot) = slot {
            slot::begin_stop(&slot, visual_fade_ms);
            if let Some(audio_id) = audio_id {
                let _ = self.audio_engine.stop_voice(
                    audio_id,
                    audio_fade_ms,
                    crate::engine::ring_command::FadeCurve::SCurve,
                );
            }
        }
        self.stop_attached_cue_audio(voice_id, audio_fade_ms);
        self.release_voice_bookkeeping(voice_id, !is_external);
    }

    pub fn hard_stop_current(&self) {
        self.stop_all_attached_cue_audio();
        self.voices.lock().unwrap().clear();
        self.voice_outputs.lock().unwrap().clear();
        self.voice_groups.lock().unwrap().clear();
        self.voice_group_completed.lock().unwrap().clear();
        self.voice_group_completion_emitted.lock().unwrap().clear();
        self.output_registry.lock().unwrap().clear_voices();
        for output in self.all_pipelines() {
            output.hard_stop();
        }
    }

    pub fn panic_stop(&self) {
        self.stop_all_attached_cue_audio();
        self.voices.lock().unwrap().clear();
        self.voice_outputs.lock().unwrap().clear();
        self.voice_groups.lock().unwrap().clear();
        self.voice_group_completed.lock().unwrap().clear();
        self.voice_group_completion_emitted.lock().unwrap().clear();
        self.output_registry.lock().unwrap().clear_voices();
        for output in self.all_pipelines() {
            output.hard_stop();
        }
        // Transport normally cuts AudioEngine first; keeping this backstop
        // here also handles a direct panic command with stale cue bookkeeping.
        let _ = self.audio_engine.panic_stop_all();
    }

    pub fn is_current_voice(&self, voice_id: VoiceId) -> bool {
        self.pipeline_for_voice(voice_id).is_some_and(|pipeline| {
            slot::slot_for_voice(&pipeline.slot_registry, voice_id).is_some()
                || pipeline.render_runtime.has_external_source(voice_id)
        })
    }

    /// `true` while mpv is actively playing the requested voice. Unlike
    /// `is_current_voice`, this becomes false after natural EOF even if slot
    /// bookkeeping has not yet been collected.
    pub fn is_voice_playing(&self, voice_id: VoiceId) -> bool {
        let Some(pipeline) = self.pipeline_for_voice(voice_id) else {
            return false;
        };
        let Some(_permit) = pipeline.enter_mpv_call() else {
            return false;
        };
        let Some(slot) = slot::slot_for_voice(&pipeline.slot_registry, voice_id) else {
            return pipeline.render_runtime.has_external_source(voice_id);
        };
        slot::is_playing(&slot)
    }

    pub fn apply_geometry(&self, voice_id: VoiceId, geometry: &VideoGeometry) {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|m| m.get(&voice_id).cloned()) {
            for member in members { self.apply_geometry_single(member, geometry); }
            return;
        }
        self.apply_geometry_single(voice_id, geometry);
    }

    fn apply_geometry_single(&self, voice_id: VoiceId, geometry: &VideoGeometry) {
        let Some(output) = self.pipeline_for_voice(voice_id) else { return };
        if output.render_runtime.apply_external_geometry(voice_id, geometry) { return; }
        let Some(_permit) = output.enter_mpv_call() else { return };
        let Some(slot) = slot::slot_for_voice(&output.slot_registry, voice_id) else { return };
        apply_scalar_geometry(&slot.lib, slot.mpv_ctx.0, geometry);
        let applied = try_apply_crop(&slot.lib, slot.mpv_ctx.0, geometry);
        if let Ok(mut state) = slot.state.lock() {
            state.geometry = *geometry;
            state.crop_applied = applied || !geometry.has_crop();
        }
        output.render_runtime.wake();
    }

    pub fn set_layer_props(&self, voice_id: VoiceId, style: &LayerStyle) {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|m| m.get(&voice_id).cloned()) {
            for member in members { self.set_layer_props_single(member, style); }
            return;
        }
        self.set_layer_props_single(voice_id, style);
    }

    fn set_layer_props_single(&self, voice_id: VoiceId, style: &LayerStyle) {
        if let Some(output) = self.pipeline_for_voice(voice_id) {
            if output.render_runtime.set_external_layer_style(voice_id, style) { return; }
            if let Some(slot) = slot::slot_for_voice(&output.slot_registry, voice_id) {
                slot::set_layer_style(&slot, style);
            }
        }
    }

    pub fn get_voice_opacity(&self, voice_id: VoiceId) -> f32 {
        self.pipeline_for_voice(voice_id)
            .and_then(|p| p.render_runtime.external_opacity(voice_id).or_else(|| slot::slot_for_voice(&p.slot_registry, voice_id).map(|s| slot::opacity_of(&s))))
            .unwrap_or(0.0)
    }

    pub fn set_voice_opacity(&self, voice_id: VoiceId, opacity: f32) {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|m| m.get(&voice_id).cloned()) {
            for member in members { self.set_voice_opacity_single(member, opacity); }
            return;
        }
        self.set_voice_opacity_single(voice_id, opacity);
    }

    fn set_voice_opacity_single(&self, voice_id: VoiceId, opacity: f32) {
        if let Some(output) = self.pipeline_for_voice(voice_id) {
            if output.render_runtime.set_external_opacity(voice_id, opacity) { return; }
            if let Some(slot) = slot::slot_for_voice(&output.slot_registry, voice_id) {
                slot::set_opacity_direct(&slot, opacity);
            }
        }
    }

    pub fn begin_eof_fade_out(&self, voice_id: VoiceId, fade_ms: u32) -> bool {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|m| m.get(&voice_id).cloned()) {
            let mut ok = false;
            for member in members { ok |= self.begin_eof_fade_out_single(member, fade_ms); }
            return ok;
        }
        self.begin_eof_fade_out_single(voice_id, fade_ms)
    }

    fn begin_eof_fade_out_single(&self, voice_id: VoiceId, fade_ms: u32) -> bool {
        let Some(output) = self.pipeline_for_voice(voice_id) else { return false };
        if output.render_runtime.fade_external_opacity(voice_id, 0.0, fade_ms) { return true; }
        let Some(slot) = slot::slot_for_voice(&output.slot_registry, voice_id) else { return false };
        slot::animate_opacity(&slot, 0.0, fade_ms);
        true
    }

    pub fn start_preloaded(&self, voice_id: VoiceId) -> bool {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|m| m.get(&voice_id).cloned()) {
            let mut ok = false;
            for member in members { ok |= self.start_preloaded_single(member); }
            return ok;
        }
        self.start_preloaded_single(voice_id)
    }

    fn start_preloaded_single(&self, voice_id: VoiceId) -> bool {
        let Some(output) = self.pipeline_for_voice(voice_id) else { return false };
        let Some(_permit) = output.enter_mpv_call() else { return false };
        let Some(slot) = slot::slot_for_voice(&output.slot_registry, voice_id) else { return false };
        slot::start_preloaded(&slot)
    }

    /// Return the current overlay alpha (0 = transparent, 255 = black).
    pub fn get_overlay_alpha(&self) -> u8 {
        self.default_pipeline()
            .and_then(|p| p.pipeline.fade.lock().ok().map(|s| s.current_alpha))
            .unwrap_or(0)
    }

    /// Directly set the overlay alpha — called from FadeCue.tick() at ~30 fps.
    pub fn set_overlay_alpha_direct(&self, alpha: u8) {
        if let Some(output) = self.default_pipeline() {
            fade::set_overlay_alpha(&output.pipeline, alpha);
        }
    }

    /// Return the AudioEngine voice carrying a video voice's audio track.
    pub fn video_audio_voice(&self, voice_id: VoiceId) -> Option<VoiceId> {
        let output = self.pipeline_for_voice(voice_id)?;
        let slot = slot::slot_for_voice(&output.slot_registry, voice_id)?;
        let audio = slot.state.lock().ok().and_then(|st| st.audio_voice_id);
        audio
    }

    /// Current playback position of a voice's video (mpv `time-pos`), in ms.
    pub fn current_video_position_ms(&self, voice_id: VoiceId) -> Option<u64> {
        let output = self.pipeline_for_voice(voice_id)?;
        let _permit = output.enter_mpv_call()?;
        let slot = slot::slot_for_voice(&output.slot_registry, voice_id)?;
        slot::position_ms(&slot)
    }

    /// Re-anchor the paired audio voice to the video's **actual** position
    /// (mpv `time-pos`), without moving mpv.  Corrects the A/V drift that builds
    /// up when the picture keeps advancing while the audio voice is frozen
    /// during an output-device outage.
    pub fn resync_audio_to_video(&self, voice_id: VoiceId) {
        if let (Some(ms), Some(av)) = (
            self.current_video_position_ms(voice_id),
            self.video_audio_voice(voice_id),
        ) {
            let _ = self.audio_engine.seek_voice_ms(av, ms);
        }
    }

    // ── Legacy API kept for VideoCue ─────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn play_voice(
        &self,
        file_path: &Path,
        _surface_id: Option<SurfaceId>,
        _volume_db: f64,
        loop_count: u32,
        start_ms: Option<u64>,
        end_ms: Option<u64>,
        _fade_in: Option<&FadeSpec>,
        screen_index: Option<u32>,
    ) -> Result<VoiceId> {
        self.show_content(ContentRequest {
            preload: false,
            file_path,
            is_image: false,
            fade_in_ms: 0,
            loop_count,
            initial_seek_action_ms: None,
            start_ms,
            end_ms,
            screen_index,
            output_id: None,
            audio_voice_id: None,
            display_duration_ms: None,
            hold_last_frame: false,
            geometry: VideoGeometry::default(),
            live_source: false,
            layer_style: LayerStyle::default(),
            slices: Vec::new(),
        })
    }

    pub fn stop_voice(&self, voice_id: VoiceId, fade_ms: u32) -> Result<()> {
        self.stop_content(voice_id, fade_ms, fade_ms);
        Ok(())
    }

    pub fn stop_current_voice(&self, _fade_ms: u32) {
        self.hard_stop_current();
    }

    pub fn pause_voice(&self, voice_id: VoiceId) -> Result<()> {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|m| m.get(&voice_id).cloned()) {
            for member in members { self.pause_voice_single(member)?; }
            return Ok(());
        }
        self.pause_voice_single(voice_id)
    }

    fn pause_voice_single(&self, voice_id: VoiceId) -> Result<()> {
        let output = self.pipeline_for_voice(voice_id);
        if let Some(output) = output {
            let Some(_permit) = output.enter_mpv_call() else { return Ok(()) };
            let Some(ctx) = slot::slot_for_voice(&output.slot_registry, voice_id)
                .map(|slot| slot.mpv_ctx.0)
            else { return Ok(()) };
            unsafe {
                (output.mpv.lib.mpv_set_property_string)(ctx, cs("pause").as_ptr(), cs("yes").as_ptr());
            }
        }
        if let Some(aid) = self.video_audio_voice(voice_id) {
            let _ = self.audio_engine.pause_voice(aid);
        }
        Ok(())
    }

    pub fn resume_voice(&self, voice_id: VoiceId) -> Result<()> {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|m| m.get(&voice_id).cloned()) {
            for member in members { self.resume_voice_single(member)?; }
            return Ok(());
        }
        self.resume_voice_single(voice_id)
    }

    fn resume_voice_single(&self, voice_id: VoiceId) -> Result<()> {
        let output = self.pipeline_for_voice(voice_id);
        if let Some(output) = output {
            let Some(_permit) = output.enter_mpv_call() else { return Ok(()) };
            let Some(ctx) = slot::slot_for_voice(&output.slot_registry, voice_id)
                .map(|slot| slot.mpv_ctx.0)
            else { return Ok(()) };
            unsafe {
                (output.mpv.lib.mpv_set_property_string)(ctx, cs("pause").as_ptr(), cs("no").as_ptr());
            }
        }
        if let Some(aid) = self.video_audio_voice(voice_id) {
            let _ = self.audio_engine.resume_voice(aid);
        }
        Ok(())
    }

    pub fn set_voice_volume(&self, voice_id: VoiceId, volume_db: f64) -> Result<()> {
        if let Some(aid) = self.video_audio_voice(voice_id) {
            let _ = self
                .audio_engine
                .set_voice_gain(aid, db_to_linear(volume_db) as f32);
        }
        Ok(())
    }

    /// Seek a voice's video (and re-anchor its paired audio voice).
    pub fn seek_voice_ms(&self, voice_id: VoiceId, position_ms: u64) {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|m| m.get(&voice_id).cloned()) {
            for member in members { self.seek_voice_ms_single(member, position_ms); }
            return;
        }
        self.seek_voice_ms_single(voice_id, position_ms);
    }

    /// Seek a video in action-time coordinates. The slot converts this to a
    /// source position using its trim/loop settings once the source duration
    /// is known, including when the request arrives immediately after GO.
    pub fn seek_voice_action_ms(&self, voice_id: VoiceId, position_ms: u64) {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|m| m.get(&voice_id).cloned()) {
            for member in members { self.seek_voice_action_ms_single(member, position_ms); }
            return;
        }
        self.seek_voice_action_ms_single(voice_id, position_ms);
    }

    fn seek_voice_action_ms_single(&self, voice_id: VoiceId, position_ms: u64) {
        let Some(output) = self.pipeline_for_voice(voice_id) else { return };
        let Some(_permit) = output.enter_mpv_call() else { return };
        let Some(slot) = slot::slot_for_voice(&output.slot_registry, voice_id) else { return };
        if slot::queue_seek_action_if_loading(&slot, position_ms) {
            return;
        }
        if let Some(duration_ms) = slot::duration_ms(&slot).or_else(|| slot::loaded_duration_ms(&slot)) {
            let file_ms = slot::action_seek_file_ms(&slot, position_ms, duration_ms);
            // A seek issued after FILE_LOADED but before the first
            // PLAYBACK_RESTART used to be queued for an event that had
            // already passed. Keep the slot dark until this exact seek lands.
            slot::wait_to_reveal_after_seek(&slot, file_ms);
            slot::apply_seek(&slot, file_ms);
        } else {
            // A slot without a duration is still loading. Keep the action
            // coordinate until FILE_LOADED supplies the source duration.
            if let Ok(mut state) = slot.state.lock() {
                state.pending_seek_action_ms = Some(position_ms);
            }
        }
    }

    fn seek_voice_ms_single(&self, voice_id: VoiceId, position_ms: u64) {
        let Some(output) = self.pipeline_for_voice(voice_id) else { return };
        let Some(_permit) = output.enter_mpv_call() else { return };
        let Some(slot) = slot::slot_for_voice(&output.slot_registry, voice_id) else { return };
        if slot::queue_seek_if_loading(&slot, position_ms) {
            // FILE_LOADED will issue the seek after mpv has accepted the new
            // file. The paired audio voice is re-anchored there as well.
            return;
        }
        slot::wait_to_reveal_after_seek(&slot, position_ms);
        slot::apply_seek(&slot, position_ms);
    }

    // ── Window visibility ─────────────────────────────────────────────────────

    /// Toggle the output window visibility (F9 / View menu).
    pub fn toggle_visibility(&self) {
        let Ok(_configure_guard) = self.network_configure_gate.lock() else {
            return;
        };
        if self.is_visible_locked() {
            self.hide_output_locked();
        } else {
            self.show_output_locked();
        }
    }

    /// Make the output window visible.
    pub fn show_output(&self) {
        let Ok(_configure_guard) = self.network_configure_gate.lock() else {
            return;
        };
        self.show_output_locked();
    }

    fn show_output_locked(&self) {
        let screens = self.list_screens();
        for output in self.active_display_pipelines() {
            output.show_for_config(&screens);
        }
        use tauri::Emitter;
        let _ = self
            .app_handle
            .emit("output-window-visible", self.is_visible_locked());
    }

    /// Hide the output window.
    pub fn hide_output(&self) {
        let Ok(_configure_guard) = self.network_configure_gate.lock() else {
            return;
        };
        self.hide_output_locked();
    }

    fn hide_output_locked(&self) {
        for output in self.active_display_pipelines() {
            output.hide();
        }
        use tauri::Emitter;
        let _ = self.app_handle.emit("output-window-visible", false);
    }

    /// Return whether the output window is currently visible.
    pub fn is_visible(&self) -> bool {
        let Ok(_configure_guard) = self.network_configure_gate.lock() else {
            return false;
        };
        self.is_visible_locked()
    }

    fn is_visible_locked(&self) -> bool {
        self.active_display_pipelines().into_iter().any(|p| {
            p.config().enabled && p.visible.load(Ordering::Relaxed)
        })
    }

    // ── OSD / timer ──────────────────────────────────────────────────────────

    /// Update the countdown text shown on the output window timer (mpv OSD).
    ///
    /// Pass `None` (or an empty string) to hide the timer.
    pub fn set_output_timer(&self, text: Option<&str>) {
        let text = text.unwrap_or("");
        for output in self.active_output_pipelines() {
            let Some(_permit) = output.enter_mpv_call() else { continue };
            // The OSD only renders over a decoded surface (see
            // OVERLAY_HAS_DUMMY), and the overlay is only composited while
            // flagged active.
            if text.is_empty() {
                output.pipeline.timer_osd_active.store(false, Ordering::Relaxed);
            } else {
                output.pipeline.timer_osd_active.store(true, Ordering::Relaxed);
                ensure_overlay_surface(&output.pipeline);
            }
            unsafe {
                prop_str(&output.mpv.lib, output.mpv.ctx.0, "osd-msg1", text);
            }
            if text.is_empty() {
                release_overlay_surface_if_idle(&output.pipeline);
                output.render_runtime.mark_overlay_dirty();
            }
        }
    }

    /// Apply font, size, position and margin settings for the OSD timer overlay.
    pub fn set_timer_style(
        &self,
        font: &str,
        font_size: u32,
        position: crate::preferences::TimerPosition,
        margin: u32,
    ) {
        use crate::preferences::TimerPosition;
        let font_changed = FLOAT_TIMER_FONT
            .get()
            .and_then(|m| m.lock().ok())
            .map(|mut g| {
                if *g != font {
                    *g = font.to_owned();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if font_changed {
            use tauri::Emitter;
            let _ = self.app_handle.emit("float-timer-font", font);
        }
        let (align_x, align_y) = match position {
            TimerPosition::Center => ("center", "center"),
            TimerPosition::TopLeft => ("left", "top"),
            TimerPosition::TopRight => ("right", "top"),
            TimerPosition::BottomLeft => ("left", "bottom"),
            TimerPosition::BottomRight => ("right", "bottom"),
        };
        let margin_str = match position {
            TimerPosition::Center => "0".to_string(),
            _ => margin.to_string(),
        };
        for output in self.active_output_pipelines() {
            let Some(_permit) = output.enter_mpv_call() else { continue };
            unsafe {
                prop_str(&output.mpv.lib, output.mpv.ctx.0, "osd-font", font);
                prop_str(
                    &output.mpv.lib,
                    output.mpv.ctx.0,
                    "osd-font-size",
                    &font_size.to_string(),
                );
                prop_str(&output.mpv.lib, output.mpv.ctx.0, "osd-align-x", align_x);
                prop_str(&output.mpv.lib, output.mpv.ctx.0, "osd-align-y", align_y);
                prop_str(&output.mpv.lib, output.mpv.ctx.0, "osd-margin-x", &margin_str);
                prop_str(&output.mpv.lib, output.mpv.ctx.0, "osd-margin-y", &margin_str);
            }
        }
    }

    // ── Floating timer (Tauri WebView window) ─────────────────────────────────

    /// Show or hide the standalone floating timer window (Tauri WebView).
    ///
    /// GTK (Linux) and AppKit (macOS) require window show/hide on the main
    /// thread, but Tauri command handlers run on a worker thread.  Marshalling
    /// onto the main thread makes this safe on all three OS — the same
    /// cross-platform discipline the winit output window follows.
    pub fn set_floating_timer_visible(&self, visible: bool) {
        let app = self.app_handle.clone();
        let _ = self.app_handle.run_on_main_thread(move || {
            use tauri::Manager;
            if let Some(win) = app.get_webview_window("float-timer") {
                let _ = if visible { win.show() } else { win.hide() };
            }
        });
    }

    /// Write the current timer text to the floating window.
    /// Only emits a Tauri event when the text actually changed.
    pub fn update_floating_timer(&self, text: Option<&str>) {
        let new_text = text.unwrap_or("");
        let changed = FLOAT_TIMER_TEXT
            .get()
            .and_then(|m| m.lock().ok())
            .map(|mut g| {
                if *g != new_text {
                    *g = new_text.to_owned();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if changed {
            use tauri::Emitter;
            let _ = self.app_handle.emit("float-timer-text", new_text);
        }
    }

    /// Set or clear the preview text shown on the OSD timer.
    pub fn set_timer_preview(&self, text: Option<String>) {
        if let Some(m) = TIMER_PREVIEW.get() {
            if let Ok(mut g) = m.lock() {
                *g = text;
            }
        }
    }

    /// Return the current preview text, if any.
    pub fn get_timer_preview(&self) -> Option<String> {
        TIMER_PREVIEW.get()?.lock().ok()?.clone()
    }

    // ── Text overlay (sub-text / ASS) ────────────────────────────────────────

    /// Display an ASS-tagged text string on the output surface.
    ///
    /// Uses mpv's `osd-overlay` command (`format=ass-events`), the API-supported
    /// way to draw client-supplied ASS: it honours full override tags (`\an`,
    /// `\fn`, `\fs`, `\c` …), is independent of `osd-level`, persists across file
    /// loads, and composites over whatever the VO shows.  (`sub-text` is read-only
    /// and `osd-msg2`/`osd-msg3` only render at `osd-level >= 2`, which is reserved
    /// for the cue timer on `osd-msg1`.)
    ///
    /// When nothing is playing, a black lavfi source is loaded so the OSD has a
    /// surface to composite onto and the output shows black rather than the desktop.
    pub fn show_text_overlay(&self, ass_text: &str, screen_index: Option<u32>) {
        self.show_text_overlay_on_output(ass_text, None, screen_index);
    }

    /// Display Text Cue content on one named output. Each native pipeline has
    /// its own mpv OSD context, so overlays on two outputs remain independent.
    pub fn show_text_overlay_on_output(
        &self,
        ass_text: &str,
        output_id: Option<&str>,
        screen_index: Option<u32>,
    ) {
        let Some((resolved_id, output)) = self.pipeline_for_output(output_id) else {
            match output_id {
                Some(id) => log::warn!(
                    "[output] text overlay ignored: output destination '{id}' is unavailable"
                ),
                None => log::warn!("[output] text overlay ignored: {NO_VIDEO_OUTPUT}"),
            }
            return;
        };
        let cfg = output.config();
        let monitor = if output_id.is_some() { cfg.monitor } else { screen_index.or(cfg.monitor) };
        let screens = self.list_screens();
        if !output.position(&screens, monitor, &output_health_key(&resolved_id), true) {
            // Keep a missing-monitor output hidden.  In particular, do not
            // create an OSD surface that could later appear on the primary
            // display after a monitor hot-plug or window-manager change.
            log::warn!(
                "[output] text overlay skipped: monitor for output '{}' is unavailable",
                resolved_id
            );
            return;
        }
        let Some(_permit) = output.enter_mpv_call() else {
            return;
        };
        output.pipeline.text_overlay_active.store(true, Ordering::Relaxed);
        output.render_runtime.set_text_overlay_active(true);
        ensure_overlay_surface(&output.pipeline);
        unsafe { osd_overlay_set(&output.mpv.lib, output.mpv.ctx.0, ass_text); }
        fade::set_overlay_alpha(&output.pipeline, 0);
    }

    /// Clear the text set via [`show_text_overlay`].
    ///
    /// Restores the opaque-black idle state (alpha=255) when no video or image
    /// content is currently playing.
    pub fn clear_text_overlay(&self) {
        self.clear_text_overlay_on_output(None);
    }

    pub fn clear_text_overlay_on_output(&self, output_id: Option<&str>) {
        let Some((_, output)) = self.pipeline_for_output(output_id) else { return };
        let Some(_permit) = output.enter_mpv_call() else { return };
        output.pipeline.text_overlay_active.store(false, Ordering::Relaxed);
        output.render_runtime.set_text_overlay_active(false);
        unsafe { osd_overlay_remove(&output.mpv.lib, output.mpv.ctx.0); }
        release_overlay_surface_if_idle(&output.pipeline);
        output.render_runtime.mark_overlay_dirty();
        if !output.has_visual_content()
            && !output.pipeline.timer_osd_active.load(Ordering::Relaxed)
        {
            fade::set_overlay_alpha(&output.pipeline, 255);
        }
    }

    /// Whether the output window is currently user-visible.
    pub fn is_output_visible(&self) -> bool {
        self.is_visible()
    }

    /// Update the global projector-alignment transform and apply it
    /// immediately, so the operator sees the effect live while dragging in
    /// the alignment editor.  No-op when the transform is unchanged (the
    /// event loop re-asserts it every tick to stay in sync with the loaded
    /// workspace).
    ///
    /// The transform (incl. fractional rotation + corner pin) is a dedicated
    /// warp render pass — mpv properties are not touched.
    pub fn set_output_transform(&self, transform: OutputTransform) {
        if let Some(output) = self.default_pipeline() {
            let cfg = output.config();
            output.update_config(OutputPipelineConfig { transform, ..cfg });
        }
    }

    // ── Test patterns (projector calibration) ────────────────────────────────

    /// Show a calibration pattern (grid, colour bars, custom image, …) on the
    /// output window, replacing whatever is playing.
    ///
    /// The current content is hard-stopped first (its owning cue completes
    /// through the normal `OutputStatus::Completed` path), the window is
    /// positioned like a GO would (fallback + banner included), and the
    /// pattern is shown with **neutral cue geometry** — only the global
    /// [`OutputTransform`] applies, which is exactly what alignment and
    /// colorimetry need.
    pub fn show_test_pattern(&self, pattern: &TestPattern, screen_index: Option<u32>) {
        let Some(output) = self.default_pipeline() else {
            log::warn!("[output] test pattern ignored: {NO_VIDEO_OUTPUT}");
            return;
        };
        output.hard_stop();
        let cfg = output.config();
        let monitor = screen_index.or(cfg.monitor);
        let screens = self.list_screens();
        output.position(&screens, monitor, &output_health_key(&cfg.id), true);
        let Some(_permit) = output.enter_mpv_call() else { return };

        // The pattern replaces whatever the overlay context held (incl. the
        // transparent OSD dummy) and makes the overlay composite opaque.
        output.pipeline.test_pattern_active.store(true, Ordering::Relaxed);
        output.pipeline.text_overlay_active.store(false, Ordering::Relaxed);
        output.pipeline.timer_osd_active.store(false, Ordering::Relaxed);
        output.pipeline.overlay_has_dummy.store(false, Ordering::Relaxed);
        output.render_runtime.set_text_overlay_active(false);

        // Pattern resolution: match the target screen so the grid is 1:1.
        let (w, h) = resolve_output_screen(&screens, monitor)
            .0
            .map(|s| (s.width, s.height))
            .unwrap_or((1920, 1080));
        let url = pattern.mpv_url(w, h);

        apply_geometry_props(
            &output.pipeline,
            &output.mpv.lib,
            output.mpv.ctx.0,
            &VideoGeometry::default(),
        );

        unsafe {
            // Patterns behave like images: play immediately, no paused-load
            // handshake, and no keep-open (a previous held video may have set it).
            (output.mpv.lib.mpv_set_property_string)(output.mpv.ctx.0, cs("pause").as_ptr(), cs("no").as_ptr());
            (output.mpv.lib.mpv_set_property_string)(output.mpv.ctx.0, cs("keep-open").as_ptr(), cs("no").as_ptr());
            if let Ok(mut p) = output.pipeline.pending_video_start.lock() { *p = None; }

            let opts = if pattern.is_file() {
                // A custom image needs image-display-duration to hold on screen.
                cs("audio=no,image-display-duration=inf")
            } else {
                cs("audio=no")
            };
            let path_cstr = match CString::new(url.as_str()) {
                Ok(c) => c,
                Err(_) => {
                    log::warn!("[output] test pattern path contains NUL byte");
                    return;
                }
            };
            let cmd = cs("loadfile");
            let flags = cs("replace");
            let idx = cs("0");
            // loadfile signature: <url> <flags> <index> <options> (see fade.rs).
            let args: [*const std::ffi::c_char; 6] = [
                cmd.as_ptr(),
                path_cstr.as_ptr(),
                flags.as_ptr(),
                idx.as_ptr(),
                opts.as_ptr(),
                std::ptr::null(),
            ];
            let ret = (output.mpv.lib.mpv_command)(output.mpv.ctx.0, args.as_ptr());
            if ret < 0 {
                log::warn!("[output] test pattern loadfile failed: {ret} ({url})");
            }
        }

        fade::set_overlay_alpha(&output.pipeline, 0);
    }

    /// Clear the test pattern: stop playback and return to opaque black.
    pub fn clear_test_pattern(&self) {
        let Some(output) = self.default_pipeline() else { return };
        let Some(_permit) = output.enter_mpv_call() else { return };
        unsafe {
            let stop = cs("stop");
            let args: [*const c_char; 2] = [stop.as_ptr(), std::ptr::null()];
            (output.mpv.lib.mpv_command)(output.mpv.ctx.0, args.as_ptr());
        }
        output.pipeline.test_pattern_active.store(false, Ordering::Relaxed);
        // Timer/Text OSD may still be live — give them their surface back.
        if overlay_active(&output.pipeline) {
            ensure_overlay_surface(&output.pipeline);
        }
        output.render_runtime.mark_overlay_dirty();
        if !output.has_visual_content() {
            fade::set_overlay_alpha(&output.pipeline, 255);
        }
    }

    /// Immediately apply the output-screen preference to the live window.
    ///
    /// `Some(idx)` shows the window fullscreen on that screen right away (same
    /// missing-screen fallback + health banner as a GO), so the operator sees
    /// the effect of the Preferences selection without waiting for the next
    /// visual cue.  `None` (floating) restores the windowed floating rect.
    pub fn apply_output_screen(&self, screen_index: Option<u32>) {
        let Some(output) = self.default_pipeline() else { return };
        let cfg = output.config();
        let screens = self.list_screens();
        output.position(&screens, screen_index, &output_health_key(&cfg.id), true);
    }

    /// Apply the output-screen preference when a workspace is (re)loaded, so a
    /// configured screen goes live as a black fullscreen surface immediately —
    /// not only on the first visual GO.
    ///
    /// Unlike [`Self::apply_output_screen`], a configured-but-missing screen
    /// only raises the health banner and keeps the window hidden: falling back
    /// to fullscreen-on-primary here would black out the operator's main
    /// display the moment they open a show file without the projector attached.
    pub fn apply_output_screen_on_load(&self, screen_index: Option<u32>) {
        let Some(output) = self.default_pipeline() else { return };
        let cfg = output.config();
        let screens = self.list_screens();
        output.position(&screens, screen_index, &output_health_key(&cfg.id), true);
    }

    // ── Fullscreen ────────────────────────────────────────────────────────────

    /// Toggle the output window between windowed and true fullscreen.
    pub fn toggle_fullscreen(&self) {
        if let Some(output) = self.default_pipeline() {
            output.render_runtime.toggle_fullscreen();
        }
    }

    // ── Status / GC ──────────────────────────────────────────────────────────

    pub fn push_status(&self, _status: OutputStatus) {}

    /// Drain all pending status events.  Called by the 30 fps event loop.
    pub fn drain_status(&self) -> Vec<OutputStatus> {
        let rx = self.status_rx.lock().unwrap();
        let mut out = Vec::new();
        let mut completed_groups = HashSet::new();
        let mut duration_groups = HashSet::new();
        while let Ok(s) = rx.try_recv() {
            out.push(match s {
                OutputStatus::Completed { voice_id } => {
                    if let Some(primary) = self.group_primary_for_voice(voice_id) {
                        let members = self.voice_groups.lock().ok().and_then(|groups| groups.get(&primary).cloned()).unwrap_or_default();
                        let complete = self.voice_group_completed.lock().ok().map(|mut done| {
                            group_completion_ready(done.entry(primary).or_default(), &members, voice_id)
                        }).unwrap_or(false);
                        if !complete { continue; }
                        if !self.voice_group_completion_emitted.lock().ok().map(|mut emitted| emitted.insert(primary)).unwrap_or(false) { continue; }
                        OutputStatus::Completed { voice_id: primary }
                    } else {
                        let canonical = self.canonical_voice_id(voice_id);
                        if !completed_groups.insert(canonical) { continue; }
                        OutputStatus::Completed { voice_id: canonical }
                    }
                },
                OutputStatus::Duration { voice_id, duration_ms } => {
                    let canonical = self.canonical_voice_id(voice_id);
                    if !duration_groups.insert(canonical) { continue; }
                    OutputStatus::Duration { voice_id: canonical, duration_ms }
                },
                OutputStatus::Error { voice_id, message } => {
                    let canonical = self.canonical_voice_id(voice_id);
                    // One failed destination makes the fan-out cue unsafe to
                    // continue. Stop every surviving member and dissolve the
                    // group so no hidden voice remains orphaned.
                    if self.group_primary_for_voice(voice_id).is_some() {
                        self.stop_content(canonical, 0, 0);
                    }
                    OutputStatus::Error { voice_id: canonical, message }
                },
            });
        }
        out
    }

    /// Remove a completed voice.
    pub fn gc_voice(&self, voice_id: VoiceId) {
        self.release_voice_bookkeeping(voice_id, true);
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    fn position_window(&self, screen_index: Option<u32>) {
        self.apply_output_screen(screen_index);
        use tauri::Emitter;
        let _ = self.app_handle.emit("output-window-visible", self.is_visible());
    }

    fn active_output_pipelines(&self) -> Vec<Arc<NativeOutputPipeline>> {
        self.outputs
            .lock()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Active native windows that belong to physical display destinations.
    /// Network destinations keep a hidden compositor context, but the global
    /// show/hide control must never wake, reposition, or hide those workers.
    fn active_display_pipelines(&self) -> Vec<Arc<NativeOutputPipeline>> {
        self.active_output_pipelines()
            .into_iter()
            .filter(|pipeline| {
                matches!(
                    &pipeline.config().sink_kind,
                    crate::preferences::OutputSinkKind::Display
                ) && !pipeline.network_output.load(Ordering::Acquire)
            })
            .collect()
    }

    fn release_voice_bookkeeping(&self, voice_id: VoiceId, remove_external: bool) {
        if let Some(members) = self.voice_groups.lock().ok().and_then(|mut m| m.remove(&voice_id)) {
            self.voice_group_completed.lock().ok().map(|mut m| { m.remove(&voice_id); });
            self.voice_group_completion_emitted.lock().ok().map(|mut m| { m.remove(&voice_id); });
            for member in members { self.release_voice_bookkeeping_single(member, remove_external); }
            return;
        }
        self.release_voice_bookkeeping_single(voice_id, remove_external);
    }

    fn release_voice_bookkeeping_single(&self, voice_id: VoiceId, remove_external: bool) {
        if remove_external { if let Some(output) = self.pipeline_for_voice(voice_id) {
            output.render_runtime.remove_external_source(voice_id);
        } }
        self.voices.lock().unwrap().remove(&voice_id);
        self.voice_outputs.lock().unwrap().remove(&voice_id);
        self.output_registry.lock().unwrap().release_voice(voice_id);
        // `configure_outputs` reaches this path after a retired native
        // pipeline has disappeared. Stop an NDI/SRT synthetic feed as well,
        // otherwise its audio can remain audible without any picture.
        self.stop_attached_cue_audio(voice_id, 0);
    }

    fn stop_attached_cue_audio(&self, visual_voice_id: VoiceId, fade_ms: u32) {
        let audio_voice_id = self
            .cue_audio_voices
            .lock()
            .ok()
            .and_then(|mut voices| voices.remove(&visual_voice_id));
        if let Some(audio_voice_id) = audio_voice_id {
            let _ = self.audio_engine.stop_voice(
                audio_voice_id,
                fade_ms,
                crate::engine::ring_command::FadeCurve::Linear,
            );
        }
    }

    fn stop_all_attached_cue_audio(&self) {
        let audio_voice_ids = self
            .cue_audio_voices
            .lock()
            .map(|mut voices| voices.drain().map(|(_, voice)| voice).collect::<Vec<_>>())
            .unwrap_or_default();
        for audio_voice_id in audio_voice_ids {
            let _ = self.audio_engine.stop_voice(
                audio_voice_id,
                0,
                crate::engine::ring_command::FadeCurve::Linear,
            );
        }
    }

    fn canonical_voice_id(&self, voice_id: VoiceId) -> VoiceId {
        self.voice_groups.lock().ok().and_then(|groups| groups.iter().find_map(|(primary, members)| members.contains(&voice_id).then_some(*primary))).unwrap_or(voice_id)
    }

    fn group_primary_for_voice(&self, voice_id: VoiceId) -> Option<VoiceId> {
        self.voice_groups.lock().ok().and_then(|groups| groups.iter().find_map(|(primary, members)| members.contains(&voice_id).then_some(*primary)))
    }
}

fn stable_surface_id(id: &str) -> SurfaceId {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    Uuid::from_u128(h.finish() as u128)
}

/// Create one transparent overlay mpv context for a native output.  Video
/// slots create their own contexts in `slot.rs`; this context is reserved for
/// the timer, Text Cue, and projector test pattern.
fn create_overlay_context(lib: &Arc<MpvLib>) -> Result<Arc<MpvCtx>> {
    let ctx = unsafe { (lib.mpv_create)() };
    if ctx.is_null() {
        return Err(anyhow!("mpv_create() returned null"));
    }
    unsafe {
        opt_str(lib, ctx, "vo", "libmpv");
        opt_str(lib, ctx, "background", "none");
        opt_str(lib, ctx, "alpha", "yes");
        opt_str(lib, ctx, "hwdec", "no");
        opt_str(lib, ctx, "osc", "no");
        opt_str(lib, ctx, "osd-level", "1");
        opt_str(lib, ctx, "input-default-bindings", "no");
        opt_str(lib, ctx, "input-vo-keyboard", "no");
        opt_str(lib, ctx, "input-cursor", "no");
        opt_str(lib, ctx, "keep-open", "no");
        opt_str(lib, ctx, "idle", "yes");
        opt_str(lib, ctx, "ao", "null");
        opt_str(lib, ctx, "audio", "no");
        opt_str(lib, ctx, "video-sync", "desync");
        (lib.mpv_request_log_messages)(ctx, cs("v").as_ptr());
        let ret = (lib.mpv_initialize)(ctx);
        if ret < 0 {
            (lib.mpv_terminate_destroy)(ctx);
            return Err(anyhow!("mpv_initialize() failed with code {ret}"));
        }
        prop_str(lib, ctx, "osd-font-size", "120");
        prop_str(lib, ctx, "osd-color", "#FFFFFF");
        prop_str(lib, ctx, "osd-border-color", "#000000");
        prop_str(lib, ctx, "osd-border-size", "3");
        prop_str(lib, ctx, "osd-align-x", "center");
        prop_str(lib, ctx, "osd-align-y", "center");
        prop_str(lib, ctx, "osd-margin-x", "0");
        prop_str(lib, ctx, "osd-margin-y", "0");
    }
    Ok(Arc::new(MpvCtx(ctx)))
}

impl Drop for OutputEngine {
    fn drop(&mut self) {
        // Every native pipeline joins its GL owner before quitting mpv, so the
        // raw contexts can be destroyed without racing Render API calls.
        let mut all = self
            .outputs
            .get_mut()
            .map(|m| m.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        if let Ok(retired) = self.retired_outputs.get_mut() {
            all.extend(retired.iter().cloned());
        }
        for output in all {
            output.hard_stop();
            output.hide();
            output.shutdown_native_pipeline();
        }
    }
}

// ---------------------------------------------------------------------------
// Private utility functions
// ---------------------------------------------------------------------------

pub(super) fn cs(s: &str) -> CString {
    CString::new(s).expect("cs(): interior NUL byte in literal")
}

/// Resolve the configured output screen index against the connected screens.
///
/// Returns `(target, missing)`:
/// - `target` — the screen to go fullscreen on (`None` = floating window);
///   when the configured index is absent, the target is `None`.
/// - `missing` — `true` when a screen was configured but is not connected.
pub(super) fn resolve_output_screen(
    screens: &[ScreenInfo],
    screen_index: Option<u32>,
) -> (Option<ScreenInfo>, bool) {
    match screen_index {
        None => (None, false),
        Some(idx) => match screens.iter().find(|s| s.index == idx) {
            Some(s) => (Some(s.clone()), false),
            None => (None, true),
        },
    }
}

fn output_health_key(id: &str) -> String {
    format!("output-screen-{id}")
}

fn group_completion_ready(done: &mut HashSet<VoiceId>, members: &[VoiceId], voice_id: VoiceId) -> bool {
    if !members.contains(&voice_id) { return false; }
    done.insert(voice_id);
    done.len() == members.len()
}

#[cfg(test)]
mod grouped_output_tests {
    use super::*;

    #[test]
    fn grouped_completion_waits_for_every_member_and_deduplicates() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut done = HashSet::new();
        assert!(!group_completion_ready(&mut done, &[a, b], a));
        assert!(!group_completion_ready(&mut done, &[a, b], a));
        assert!(group_completion_ready(&mut done, &[a, b], b));
        assert!(group_completion_ready(&mut done, &[a, b], b));
    }

    #[test]
    fn completion_from_unknown_member_never_completes_group() {
        let a = Uuid::new_v4();
        let unknown = Uuid::new_v4();
        let mut done = HashSet::new();
        assert!(!group_completion_ready(&mut done, &[a], unknown));
        assert!(done.is_empty());
    }
}

fn validate_output_destinations(
    destinations: &[crate::preferences::OutputDestination],
    default_id: &str,
) -> Result<()> {
    if destinations.is_empty() {
        return Err(anyhow!("At least one output destination is required"));
    }
    let mut ids = HashSet::new();
    for destination in destinations {
        if destination.id.trim().is_empty() || destination.name.trim().is_empty() {
            return Err(anyhow!("Output ids and names must be non-empty"));
        }
        if !ids.insert(destination.id.as_str()) {
            return Err(anyhow!("Output ids must be unique"));
        }
    }
    let is_display = |d: &crate::preferences::OutputDestination| {
        matches!(&d.sink_kind, crate::preferences::OutputSinkKind::Display)
    };
    if !destinations.iter().any(|d| d.enabled && is_display(d)) {
        return Err(anyhow!("At least one enabled display output is required"));
    }
    let Some(default) = destinations.iter().find(|d| d.id == default_id) else {
        return Err(anyhow!("Default output destination is not configured"));
    };
    if !default.enabled || !is_display(default) {
        return Err(anyhow!("Default output destination must be enabled"));
    }
    Ok(())
}

/// Whether applying `destinations` can leave every native resource untouched.
/// The caller supplies the current native pipeline configs and the network
/// manager's endpoint identity check; this pure seam keeps the no-op contract
/// testable without creating libmpv contexts or network sockets.
fn output_graph_is_unchanged(
    destinations: &[crate::preferences::OutputDestination],
    active_configs: &HashMap<String, OutputPipelineConfig>,
    network_configuration_current: bool,
) -> bool {
    if !network_configuration_current {
        return false;
    }
    let enabled_count = destinations.iter().filter(|destination| destination.enabled).count();
    enabled_count == active_configs.len()
        && destinations
            .iter()
            .filter(|destination| destination.enabled)
            .all(|destination| {
                active_configs
                    .get(&destination.id)
                    .is_some_and(|current| current == &OutputPipelineConfig::from_destination(destination))
            })
}

/// Translate one persisted destination into its isolated transport worker
/// contract.  Keeping this mapping here means `network_io` stays usable in
/// headless tests and never depends on preferences/window types.
fn network_output_config(
    destination: &crate::preferences::OutputDestination,
) -> Result<Option<NetworkOutputConfig>> {
    use crate::preferences::OutputSinkKind;
    let config = match destination.sink_kind {
        OutputSinkKind::Display => return Ok(None),
        OutputSinkKind::Ndi => NetworkOutputConfig {
            id: destination.id.clone(),
            ndi: Some(destination.network.ndi.clone()),
            srt: None,
        },
        OutputSinkKind::Srt => NetworkOutputConfig {
            id: destination.id.clone(),
            ndi: None,
            srt: Some(destination.network.srt.clone()),
        },
    };
    config.validate().map_err(|error| anyhow!("{}: {error}", destination.name))?;
    Ok(Some(config))
}

/// Read an int64 mpv property, or `None` when unavailable.
pub(super) unsafe fn get_prop_i64(lib: &MpvLib, ctx: *mut c_void, name: &str) -> Option<i64> {
    let mut val: i64 = 0;
    let n = cs(name);
    let ret = (lib.mpv_get_property)(
        ctx,
        n.as_ptr(),
        MPV_FORMAT_INT64,
        &mut val as *mut i64 as *mut c_void,
    );
    (ret == 0).then_some(val)
}

/// Apply the pixel `video-crop` derived from `geometry` — possible only once
/// the source dimensions (`video-params/w|h`) are known.  Returns `false`
/// when they are not yet available (caller keeps the crop pending).
pub(super) fn try_apply_crop(lib: &MpvLib, ctx: *mut c_void, geometry: &VideoGeometry) -> bool {
    unsafe {
        let w = get_prop_i64(lib, ctx, "video-params/w").unwrap_or(0);
        let h = get_prop_i64(lib, ctx, "video-params/h").unwrap_or(0);
        if w <= 0 || h <= 0 {
            return false;
        }
        match geometry.crop_rect_px(w as u32, h as u32) {
            Some((cw, ch, cx, cy)) => {
                prop_str(lib, ctx, "video-crop", &format!("{cw}x{ch}+{cx}+{cy}"));
            }
            None => prop_str(lib, ctx, "video-crop", ""),
        }
        true
    }
}

/// Push a cue's [`VideoGeometry`] to mpv.
///
/// The cue geometry is applied **pure** — the global [`OutputTransform`]
/// lives in the warp render pass instead, so composing it here would
/// double-apply it.
///
/// The scalar properties (`keepaspect`, `panscan`, `video-zoom`,
/// `video-pan-x/y`, `video-rotate`) are global mpv properties that persist
/// across `loadfile`, so every load — and every live edit — sets **all** of
/// them (a cue without geometry resets the previous cue's values).  The crop
/// is pixel-based: applied immediately when the source dimensions are known,
/// otherwise parked in [`PENDING_CROP`] for the `VIDEO_RECONFIG` handler.
pub(super) fn apply_geometry_props(pipeline: &PipelineState, lib: &MpvLib, ctx: *mut c_void, geometry: &VideoGeometry) {
    let Some(_permit) = pipeline.enter_mpv_call() else {
        return;
    };
    // Remember the cue geometry most recently pushed to mpv.
    if let Ok(mut g) = pipeline.last_cue_geometry.lock() { *g = *geometry; }

    apply_scalar_geometry(lib, ctx, geometry);

    let pending_crop = if geometry.has_crop() {
        // Pixel crop needs the source dimensions; park it when unknown.
        if try_apply_crop(lib, ctx, geometry) {
            None
        } else {
            Some(*geometry)
        }
    } else {
        // No crop on this cue: clear any crop left by the previous cue.
        // (An empty string needs no dimensions.)
        unsafe { prop_str(lib, ctx, "video-crop", "") };
        None
    };
    if let Ok(mut p) = pipeline.pending_crop.lock() { *p = pending_crop; }
}

/// Push a geometry's scalar mpv properties (everything except the pixel
/// crop, which needs the source dimensions).  Per-context — used by both the
/// overlay context and each video slot.
///
/// The cue geometry is applied **pure** (the global OutputTransform lives in
/// the warp render pass).
pub(super) fn apply_scalar_geometry(lib: &MpvLib, ctx: *mut c_void, geometry: &VideoGeometry) {
    let props = compose_display_props(geometry, &OutputTransform::default());
    unsafe {
        let (keepaspect, panscan) = geometry.fit_props();
        prop_str(lib, ctx, "keepaspect", keepaspect);
        prop_str(lib, ctx, "panscan", panscan);
        prop_str(lib, ctx, "video-zoom", &format!("{:.6}", props.zoom_log2));
        prop_str(lib, ctx, "video-pan-x", &format!("{:.6}", props.pan_x));
        prop_str(lib, ctx, "video-pan-y", &format!("{:.6}", props.pan_y));
        prop_str(lib, ctx, "video-rotate", &props.rotation.to_string());
    }
}

pub(super) unsafe fn opt_str(lib: &MpvLib, ctx: *mut c_void, name: &str, value: &str) {
    let n = cs(name);
    let v = cs(value);
    (lib.mpv_set_option_string)(ctx, n.as_ptr(), v.as_ptr());
}

/// Set an mpv *property* (after `mpv_initialize`).
pub(super) unsafe fn prop_str(lib: &MpvLib, ctx: *mut c_void, name: &str, value: &str) {
    let n = cs(name);
    let v = cs(value);
    (lib.mpv_set_property_string)(ctx, n.as_ptr(), v.as_ptr());
}

/// Show the Text Cue ASS string via mpv's `osd-overlay` command.
///
/// `res_y=720` is the ASS script reference height, so `\fs` sizes stay
/// proportional to the output regardless of its actual resolution.
pub(super) unsafe fn osd_overlay_set(lib: &MpvLib, ctx: *mut c_void, ass_text: &str) {
    let Ok(data_v) = CString::new(ass_text) else {
        log::warn!("[output] osd-overlay text contains an interior NUL — ignored");
        return;
    };
    let (name, id, format, data_k, res_y) =
        (cs("name"), cs("id"), cs("format"), cs("data"), cs("res_y"));
    let (name_v, format_v) = (cs("osd-overlay"), cs("ass-events"));

    let mut keys: [*const c_char; 5] = [
        name.as_ptr(),
        id.as_ptr(),
        format.as_ptr(),
        data_k.as_ptr(),
        res_y.as_ptr(),
    ];
    let mut values: [MpvNode; 5] = [
        MpvNode {
            u: MpvNodeUnion {
                string: name_v.as_ptr(),
            },
            format: MPV_FORMAT_STRING,
        },
        MpvNode {
            u: MpvNodeUnion {
                int64: TEXT_OSD_OVERLAY_ID,
            },
            format: MPV_FORMAT_INT64,
        },
        MpvNode {
            u: MpvNodeUnion {
                string: format_v.as_ptr(),
            },
            format: MPV_FORMAT_STRING,
        },
        MpvNode {
            u: MpvNodeUnion {
                string: data_v.as_ptr(),
            },
            format: MPV_FORMAT_STRING,
        },
        MpvNode {
            u: MpvNodeUnion { int64: 720 },
            format: MPV_FORMAT_INT64,
        },
    ];
    command_node_map(lib, ctx, &mut keys, &mut values);
}

/// Remove the Text Cue `osd-overlay` (`format=none`).
pub(super) unsafe fn osd_overlay_remove(lib: &MpvLib, ctx: *mut c_void) {
    let (name, id, format) = (cs("name"), cs("id"), cs("format"));
    let (name_v, format_v) = (cs("osd-overlay"), cs("none"));

    let mut keys: [*const c_char; 3] = [name.as_ptr(), id.as_ptr(), format.as_ptr()];
    let mut values: [MpvNode; 3] = [
        MpvNode {
            u: MpvNodeUnion {
                string: name_v.as_ptr(),
            },
            format: MPV_FORMAT_STRING,
        },
        MpvNode {
            u: MpvNodeUnion {
                int64: TEXT_OSD_OVERLAY_ID,
            },
            format: MPV_FORMAT_INT64,
        },
        MpvNode {
            u: MpvNodeUnion {
                string: format_v.as_ptr(),
            },
            format: MPV_FORMAT_STRING,
        },
    ];
    command_node_map(lib, ctx, &mut keys, &mut values);
}

/// Run `mpv_command_node` with a `MPV_FORMAT_NODE_MAP` built from parallel
/// `keys`/`values` slices, freeing any memory mpv allocates for the result.
unsafe fn command_node_map(
    lib: &MpvLib,
    ctx: *mut c_void,
    keys: &mut [*const c_char],
    values: &mut [MpvNode],
) {
    debug_assert_eq!(keys.len(), values.len());
    let mut list = MpvNodeList {
        num: keys.len() as i32,
        values: values.as_mut_ptr(),
        keys: keys.as_mut_ptr(),
    };
    let arg = MpvNode {
        u: MpvNodeUnion { list: &mut list },
        format: MPV_FORMAT_NODE_MAP,
    };
    let mut result = MpvNode {
        u: MpvNodeUnion { int64: 0 },
        format: MPV_FORMAT_NONE,
    };
    let ret = (lib.mpv_command_node)(ctx, &arg, &mut result);
    (lib.mpv_free_node_contents)(&mut result);
    if ret < 0 {
        log::warn!("[output] mpv_command_node(osd-overlay) failed: {ret}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screens(n: u32) -> Vec<ScreenInfo> {
        (0..n)
            .map(|i| ScreenInfo {
                index: i,
                width: 1920,
                height: 1080,
                x: (i as i32) * 1920,
                y: 0,
                is_primary: i == 0,
            })
            .collect()
    }

    #[test]
    fn resolve_screen_none_is_floating() {
        assert_eq!(resolve_output_screen(&screens(2), None), (None, false));
    }

    #[test]
    fn resolve_screen_found() {
        let (target, missing) = resolve_output_screen(&screens(2), Some(1));
        assert!(!missing);
        assert_eq!(target.unwrap().index, 1);
    }

    #[test]
    fn resolve_screen_missing_has_no_fallback_target() {
        let (target, missing) = resolve_output_screen(&screens(2), Some(4));
        assert!(missing);
        assert!(target.is_none());
    }

    #[test]
    fn resolve_screen_missing_with_no_screens() {
        let (target, missing) = resolve_output_screen(&[], Some(1));
        assert!(missing);
        assert!(target.is_none());
    }

    #[test]
    fn headless_error_names_the_missing_piece() {
        // The operator reads this on a cue that will not fire; it has to say
        // what is wrong, not just that something is.
        assert!(NO_VIDEO_OUTPUT.contains("libmpv"));
    }

    #[test]
    fn pipeline_state_instances_are_independent() {
        let (tx, _) = crossbeam_channel::unbounded();
        let a = PipelineState::new(tx.clone());
        let b = PipelineState::new(tx);
        a.fade.lock().unwrap().current_alpha = 17;
        *a.current_voice.lock().unwrap() = Some(Uuid::new_v4());
        a.transform.lock().unwrap().rotation = 90.0;
        assert_eq!(b.fade.lock().unwrap().current_alpha, 255);
        fade::set_operator_blackout(&a, true, 0);
        assert_eq!(a.operator_ftb.lock().unwrap().current_alpha, 255);
        assert_eq!(b.operator_ftb.lock().unwrap().current_alpha, 0);
        assert_eq!(a.fade.lock().unwrap().current_alpha, 17);
        assert!(b.current_voice.lock().unwrap().is_none());
        assert_eq!(b.transform.lock().unwrap().rotation, 0.0);
    }

    #[test]
    fn missing_network_worker_is_not_healthy() {
        use crate::engine::network_io::{NetworkOutputRuntimeStatus, NetworkOutputState};

        assert!(network_output_status_detail(None).is_some());
        let waiting = NetworkOutputRuntimeStatus {
            output_id: "net".into(),
            state: NetworkOutputState::WaitingForFrame,
            submitted_frames: 0,
            superseded_frames: 0,
            last_error: None,
        };
        assert!(network_output_status_detail(Some(&waiting)).is_none());
    }

    #[test]
    fn lifecycle_close_rejects_new_calls_and_waits_for_existing_permits() {
        use std::sync::Arc;
        use std::sync::mpsc;
        use std::time::Duration;

        let gate = Arc::new(MpvLifecycleGate::default());
        let permit = gate.enter().expect("open gate grants a permit");
        let (closed_tx, closed_rx) = mpsc::channel();
        let closer = {
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                gate.close_and_wait();
                closed_tx.send(()).unwrap();
            })
        };

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !gate.is_closing() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(gate.is_closing(), "closer must publish the closed state");
        assert!(gate.enter().is_none(), "closing rejects a new raw mpv call");
        assert!(closed_rx.try_recv().is_err(), "existing permit keeps closer waiting");

        drop(permit);
        closed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        closer.join().unwrap();
    }

    #[test]
    fn identical_active_graph_is_a_native_noop() {
        let destination = crate::preferences::DisplayPreferences::default()
            .output_destinations
            .into_iter()
            .next()
            .unwrap();
        let current = HashMap::from([(
            destination.id.clone(),
            OutputPipelineConfig::from_destination(&destination),
        )]);

        assert!(output_graph_is_unchanged(
            std::slice::from_ref(&destination),
            &current,
            true,
        ));
    }

    #[test]
    fn changed_endpoint_or_network_identity_requires_transition() {
        let destination = crate::preferences::DisplayPreferences::default()
            .output_destinations
            .into_iter()
            .next()
            .unwrap();
        let current = HashMap::from([(
            destination.id.clone(),
            OutputPipelineConfig::from_destination(&destination),
        )]);

        let mut changed = destination.clone();
        changed.name = "Changed".into();
        assert!(!output_graph_is_unchanged(
            std::slice::from_ref(&changed),
            &current,
            true,
        ));
        assert!(!output_graph_is_unchanged(
            std::slice::from_ref(&destination),
            &current,
            false,
        ));
    }

    #[test]
    fn identical_enabled_ndi_graph_is_a_native_noop_when_network_identity_is_live() {
        let mut destination = crate::preferences::DisplayPreferences::default()
            .output_destinations
            .into_iter()
            .next()
            .unwrap();
        destination.id = "program-ndi".into();
        destination.name = "Program NDI".into();
        destination.sink_kind = crate::preferences::OutputSinkKind::Ndi;
        destination.network.ndi.enabled = true;
        destination.network.ndi.stream_name = "Qlisa Program".into();

        let current = HashMap::from([(
            destination.id.clone(),
            OutputPipelineConfig::from_destination(&destination),
        )]);

        assert!(output_graph_is_unchanged(
            std::slice::from_ref(&destination),
            &current,
            true,
        ));
        // A matching native pipeline is insufficient if the sender identity
        // is gone; that case must rebuild the transport graph.
        assert!(!output_graph_is_unchanged(
            std::slice::from_ref(&destination),
            &current,
            false,
        ));
    }

    #[test]
    fn browser_output_selection_resolves_physical_monitor_only() {
        let display = OutputControlStatus {
            output_id: "projector".into(),
            name: "Projector".into(),
            available: true,
            healthy: true,
            ftb: false,
            active: true,
            visible: true,
            monitor: Some(2),
            network: false,
            detail: None,
        };
        assert_eq!(
            OutputEngine::browser_monitor_for_output(&[display], Some("projector")).unwrap(),
            Some(2)
        );
    }

    #[test]
    fn browser_output_selection_uses_configured_default_when_implicit() {
        let mut destination = crate::preferences::DisplayPreferences::default()
            .output_destinations
            .into_iter()
            .next()
            .expect("default display destination");
        destination.id = "projector".into();
        let registry = OutputRegistry::new(std::slice::from_ref(&destination), "projector");

        assert_eq!(
            OutputEngine::resolve_browser_output_id(&registry, None).unwrap(),
            "projector"
        );
    }

    #[test]
    fn browser_output_selection_rejects_network_destination() {
        let network = OutputControlStatus {
            output_id: "ndi".into(),
            name: "NDI".into(),
            available: true,
            healthy: true,
            ftb: false,
            active: true,
            visible: false,
            monitor: None,
            network: true,
            detail: None,
        };
        assert!(OutputEngine::browser_monitor_for_output(&[network], Some("ndi")).is_err());
    }
}
