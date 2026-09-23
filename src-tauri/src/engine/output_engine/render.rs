//! Unified OpenGL Render API output path.
//!
//! Drives mpv with `vo=libmpv` and renders each frame into the default
//! framebuffer of an OS window via `glutin` (OpenGL Core) + `mpv_render_context`.
//! A fullscreen black quad handles fade-to-black.  The render loop and the GL
//! fade are identical on every OS — only native window creation differs.
//!
//! ## Window creation
//!
//! - **Windows / Linux** — one process-wide `winit 0.30` event loop owns all
//!   output windows (stored as `Arc<Window>` per runtime). Windows rejects a
//!   second event loop, so new destinations are submitted through its proxy.
//! - **macOS** — winit cannot be used: its EventLoop demands the AppKit main thread,
//!   which Tauri's `NSApplication` already owns.  Instead `macos_window.rs` creates
//!   and drives an `NSWindow` directly via `objc2` (`super::macos_window`).
//!
//! In both cases creation yields a raw window/display handle pair, which the render
//! thread turns into a `glutin` GL context + `mpv_render_context`.
//!
//! ## Thread model
//!
//! | Thread                    | Role |
//! |---------------------------|------|
//! | `inkue-output-window`    | (Windows/Linux only) winit EventLoop + window events |
//! | `inkue-output-render`    | glutin context + mpv RenderContext + render loop |
//! | `inkue-output-mpv-events`| mpv_wait_event (PLAYBACK_RESTART, EOF, …) |

use std::ffi::{CStr, CString, c_void};
use std::num::NonZeroU32;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{anyhow, Result};
use glow::HasContext;
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, NotCurrentGlContext, Version};
use glutin::display::{Display, DisplayApiPreference, GlDisplay};
use glutin::surface::{GlSurface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

// winit-based window backend (Windows + Linux only).
#[cfg(not(target_os = "macos"))]
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
#[cfg(not(target_os = "macos"))]
use winit::application::ApplicationHandler;
#[cfg(not(target_os = "macos"))]
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
#[cfg(not(target_os = "macos"))]
use winit::event::{ElementState, WindowEvent};
#[cfg(not(target_os = "macos"))]
use winit::event_loop::{ActiveEventLoop, EventLoop};
#[cfg(not(target_os = "macos"))]
use winit::keyboard::{Key, ModifiersState, NamedKey};
#[cfg(not(target_os = "macos"))]
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId, WindowLevel};

use crate::engine::mpv_sys::{
    MpvLib, MpvOpenglFbo, MpvOpenglInitParams, MpvRenderParam,
    MPV_RENDER_PARAM_API_TYPE, MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
    MPV_RENDER_PARAM_FLIP_Y, MPV_RENDER_PARAM_OPENGL_FBO,
    MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, MPV_RENDER_UPDATE_FRAME,
};
use super::types::{LayerStyle, MpvCtx, VideoGeometry};
use super::fade;
use super::slot;
use super::PipelineState;
use crate::engine::network_io::{BgraFrame, BgraFrameMailbox, NetworkFrameSink};
use super::VoiceId;
use crate::preferences::FloatingWindowGeometry;

// ---------------------------------------------------------------------------
// Instance-scoped runtime state
// ---------------------------------------------------------------------------

pub(crate) struct RenderShared {
    pub signal: Arc<(Mutex<bool>, Condvar)>,
    /// One-way lifecycle transition.  Once set, the GL thread must not enter
    /// another libmpv render API call; the owner wakes and joins that thread
    /// before it asks mpv to shut its client contexts down.
    pub shutting_down: AtomicBool,
    pub text_overlay_active: AtomicBool,
    pub visible: AtomicBool,
    pub width: AtomicU32,
    pub height: AtomicU32,
    pub warp: Mutex<Option<[f32; 9]>>,
    pub warp_dirty: AtomicBool,
    pub overlay_dirty: AtomicBool,
    /// A network-only pipeline renders while its backing native window stays
    /// hidden.  This must not be conflated with `visible`: committing a frame
    /// to a hidden Wayland surface can map an operator-visible window.
    pub network_capture_active: AtomicBool,
    /// Operator monitor capture is opt-in and limited to one selected output.
    /// It reuses the final compositor texture; no media is decoded twice.
    pub monitor_capture_active: AtomicBool,
}

#[derive(Default)]
struct MonitorFrameMailbox {
    latest: Mutex<Option<(u64, BgraFrame)>>,
    next_sequence: AtomicU64,
}

impl MonitorFrameMailbox {
    fn publish(&self, frame: BgraFrame) {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some((sequence, frame));
        }
    }

    fn read_after(&self, sequence: u64) -> Option<(u64, Option<BgraFrame>)> {
        let latest = self.latest.lock().ok()?;
        let (current, frame) = latest.as_ref()?;
        Some((*current, (*current > sequence).then(|| frame.clone())))
    }

    fn clear(&self) {
        if let Ok(mut latest) = self.latest.lock() {
            *latest = None;
        }
    }
}

struct ExternalSource { mailbox: Arc<BgraFrameMailbox>, cursor: Arc<AtomicU64>, state: Mutex<ExternalState> }
struct ExternalState { geometry: VideoGeometry, layer_key: u64, blend_mode: i32, anim: slot::OpacityAnim, pending_remove: bool }
#[derive(Clone)]
struct ExternalDraw { voice: VoiceId, mailbox: Arc<BgraFrameMailbox>, cursor: Arc<AtomicU64>, geometry: VideoGeometry, layer_key: u64, blend_mode: i32, opacity: f32 }

pub(crate) struct RenderRuntime {
    pub shared: Arc<RenderShared>,
    #[cfg(not(target_os = "macos"))]
    pub window: Mutex<Option<Arc<winit::window::Window>>>,
    #[cfg(target_os = "macos")]
    pub mac_window: Mutex<Option<Arc<super::macos_window::MacOutputWindow>>>,
    /// Destination identity plus the last windowed geometry.  Native window
    /// events update this state; the event loop persists it to global settings.
    output_id: Mutex<String>,
    floating_output: AtomicBool,
    floating_geometry: Mutex<Option<FloatingWindowGeometry>>,
    floating_placement_dirty: AtomicBool,
    always_on_top: AtomicBool,
    hide_cursor: AtomicBool,
    /// Called when the native window changes visibility outside the output
    /// engine (for example the user closes a floating output window).
    visibility_callback: Mutex<Option<Arc<dyn Fn(bool) + Send + Sync>>>,
    monitor_frames: MonitorFrameMailbox,
    frame_sink: Mutex<Option<NetworkFrameSink>>,
    /// Latest-frame external video sources (currently NDI). They are uploaded
    /// by this render thread only, so receiver workers never touch GL.
    external_sources: Mutex<HashMap<VoiceId, ExternalSource>>,
    external_dirty: AtomicBool,
}

impl RenderRuntime {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            shared: Arc::new(RenderShared { signal: Arc::new((Mutex::new(false), Condvar::new())), shutting_down: AtomicBool::new(false), text_overlay_active: AtomicBool::new(false), visible: AtomicBool::new(false), width: AtomicU32::new(1920), height: AtomicU32::new(1080), warp: Mutex::new(None), warp_dirty: AtomicBool::new(false), overlay_dirty: AtomicBool::new(false), network_capture_active: AtomicBool::new(false), monitor_capture_active: AtomicBool::new(false) }),
            #[cfg(not(target_os = "macos"))] window: Mutex::new(None),
            #[cfg(target_os = "macos")] mac_window: Mutex::new(None),
            output_id: Mutex::new(String::new()),
            floating_output: AtomicBool::new(true),
            floating_geometry: Mutex::new(None),
            floating_placement_dirty: AtomicBool::new(true),
            always_on_top: AtomicBool::new(false),
            hide_cursor: AtomicBool::new(false),
            visibility_callback: Mutex::new(None),
            monitor_frames: MonitorFrameMailbox::default(),
            frame_sink: Mutex::new(None),
            external_sources: Mutex::new(HashMap::new()), external_dirty: AtomicBool::new(false),
        })
    }
    pub(super) fn wake(&self) { if let Ok(mut r) = self.shared.signal.0.lock() { *r = true; self.shared.signal.1.notify_one(); } }
    /// Stop the GL loop.  This is intentionally irreversible: a native output
    /// pipeline is retired rather than restarted after its mpv contexts are
    /// torn down.
    pub(super) fn request_shutdown(&self) {
        self.shared.shutting_down.store(true, Ordering::Release);
        if let Ok(mut ready) = self.shared.signal.0.lock() {
            *ready = true;
            self.shared.signal.1.notify_all();
        }
    }
    fn is_shutting_down(&self) -> bool {
        self.shared.shutting_down.load(Ordering::Acquire)
    }
    pub(super) fn set_visibility_callback(&self, callback: Arc<dyn Fn(bool) + Send + Sync>) {
        if let Ok(mut current) = self.visibility_callback.lock() {
            *current = Some(callback);
        }
    }
    fn notify_visibility(&self, visible: bool) {
        let callback = self
            .visibility_callback
            .lock()
            .ok()
            .and_then(|current| current.clone());
        if let Some(callback) = callback {
            callback(visible);
        }
    }
    pub(super) fn show(&self) { self.shared.visible.store(true, Ordering::Relaxed); self.notify_visibility(true); #[cfg(not(target_os = "macos"))] if let Ok(w) = self.window.lock() { if let Some(w) = w.as_ref() { w.set_visible(true); } } self.wake(); }
    pub(super) fn hide(&self) { self.shared.visible.store(false, Ordering::Relaxed); self.notify_visibility(false); #[cfg(not(target_os = "macos"))] if let Ok(w) = self.window.lock() { if let Some(w) = w.as_ref() { w.set_visible(false); } } }
    pub(super) fn mark_overlay_dirty(&self) { self.shared.overlay_dirty.store(true, Ordering::Relaxed); self.wake(); }
    pub(super) fn set_text_overlay_active(&self, v: bool) { self.shared.text_overlay_active.store(v, Ordering::Relaxed); self.wake(); }
    pub(super) fn set_warp(&self, m: Option<[f32; 9]>) { if let Ok(mut w) = self.shared.warp.lock() { *w = m; } self.shared.warp_dirty.store(true, Ordering::Relaxed); self.wake(); }
    pub(super) fn set_size(&self, w: u32, h: u32) { self.shared.width.store(w.max(1), Ordering::Relaxed); self.shared.height.store(h.max(1), Ordering::Relaxed); self.wake(); }
    pub(super) fn set_network_frame_sink(&self, sink: Option<NetworkFrameSink>) {
        let active = sink.is_some();
        if let Ok(mut current) = self.frame_sink.lock() { *current = sink; }
        self.shared.network_capture_active.store(active, Ordering::Relaxed);
        self.wake();
    }
    fn network_frame_sink(&self) -> Option<NetworkFrameSink> {
        self.frame_sink.lock().ok().and_then(|current| current.clone())
    }
    pub(super) fn set_monitor_capture(&self, active: bool) {
        let changed = self.shared.monitor_capture_active.swap(active, Ordering::AcqRel) != active;
        if changed {
            self.monitor_frames.clear();
        }
        if active { self.wake(); }
    }
    pub(super) fn monitor_capture_enabled(&self) -> bool {
        self.shared.monitor_capture_active.load(Ordering::Acquire)
    }
    pub(super) fn monitor_frame_after(&self, sequence: u64) -> Option<(u64, Option<BgraFrame>)> {
        self.monitor_frames.read_after(sequence)
    }
    pub(super) fn add_external_source(&self, voice: VoiceId, source: Arc<BgraFrameMailbox>, geometry: VideoGeometry, style: LayerStyle, layer_key: u64, fade_in_ms: u32) {
        let base = style.opacity.clamp(0.0, 1.0) as f32;
        let mut anim = slot::OpacityAnim::resting(if fade_in_ms == 0 { base } else { 0.0 });
        if fade_in_ms > 0 { anim.animate_to(base, fade_in_ms); }
        if let Ok(mut sources) = self.external_sources.lock() { sources.insert(voice, ExternalSource { mailbox: source, cursor: Arc::new(AtomicU64::new(0)), state: Mutex::new(ExternalState { geometry, layer_key, blend_mode: style.blend_mode.shader_id(), anim, pending_remove: false }) }); }
        self.external_dirty.store(true, Ordering::Release);
        self.wake();
    }
    pub(super) fn stop_external_source(&self, voice: VoiceId, fade_out_ms: u32) -> bool {
        let found = if let Ok(mut sources) = self.external_sources.lock() {
            if fade_out_ms == 0 { sources.remove(&voice).is_some() }
            else if let Some(source) = sources.get(&voice) { if let Ok(mut state) = source.state.lock() { state.pending_remove = true; state.anim.animate_to(0.0, fade_out_ms); true } else { false } } else { false }
        } else { false };
        if found { self.external_dirty.store(true, Ordering::Release); self.wake(); }
        found
    }
    pub(super) fn remove_external_source(&self, voice: VoiceId) {
        if let Ok(mut sources) = self.external_sources.lock() { sources.remove(&voice); }
        self.external_dirty.store(true, Ordering::Release);
        self.wake();
    }
    pub(super) fn clear_external_sources(&self) {
        if let Ok(mut sources) = self.external_sources.lock() { sources.clear(); }
        self.external_dirty.store(true, Ordering::Release);
        self.wake();
    }
    fn tick_external_sources(&self) -> (Vec<ExternalDraw>, bool, bool) {
        let mut draws = Vec::new(); let mut animating = false; let mut removed = false;
        if let Ok(mut sources) = self.external_sources.lock() { sources.retain(|voice, source| {
            let Ok(mut state) = source.state.lock() else { return true };
            let (opacity, _) = state.anim.tick(); let running = state.anim.is_animating(); animating |= running;
            if state.pending_remove && opacity <= 0.0 && !running { removed = true; return false; }
            draws.push(ExternalDraw { voice: *voice, mailbox: Arc::clone(&source.mailbox), cursor: Arc::clone(&source.cursor), geometry: state.geometry, layer_key: state.layer_key, blend_mode: state.blend_mode, opacity }); true
        }); }
        draws.sort_by_key(|draw| draw.layer_key);
        (draws, animating, self.external_dirty.swap(false, Ordering::AcqRel) || removed)
    }
    pub(super) fn has_external_source(&self, voice: VoiceId) -> bool {
        self.external_sources.lock().map(|sources| sources.contains_key(&voice)).unwrap_or(false)
    }
    fn external_animating(&self) -> bool { self.external_sources.lock().map(|sources| sources.values().any(|source| source.state.lock().map(|state| state.anim.is_animating()).unwrap_or(false))).unwrap_or(false) }
    pub(super) fn apply_external_geometry(&self, voice: VoiceId, geometry: &VideoGeometry) -> bool { let changed = self.external_sources.lock().ok().and_then(|sources| sources.get(&voice).and_then(|source| source.state.lock().ok().map(|mut state| state.geometry = *geometry))).is_some(); if changed { self.external_dirty.store(true, Ordering::Release); self.wake(); } changed }
    pub(super) fn set_external_layer_style(&self, voice: VoiceId, style: &LayerStyle) -> bool { let changed = self.external_sources.lock().ok().and_then(|sources| sources.get(&voice).and_then(|source| source.state.lock().ok().map(|mut state| { let seq = state.layer_key & 0xFF_FFFF_FFFF; state.layer_key = slot::resolve_layer_key(style.layer, seq); state.blend_mode = style.blend_mode.shader_id(); if !state.anim.is_animating() && !state.pending_remove { state.anim.set(style.opacity.clamp(0.0, 1.0) as f32); } }))).is_some(); if changed { self.external_dirty.store(true, Ordering::Release); self.wake(); } changed }
    pub(super) fn external_opacity(&self, voice: VoiceId) -> Option<f32> { self.external_sources.lock().ok()?.get(&voice)?.state.lock().ok().map(|state| state.anim.current) }
    pub(super) fn set_external_opacity(&self, voice: VoiceId, opacity: f32) -> bool { let changed = self.external_sources.lock().ok().and_then(|sources| sources.get(&voice).and_then(|source| source.state.lock().ok().map(|mut state| state.anim.set(opacity)))).is_some(); if changed { self.external_dirty.store(true, Ordering::Release); self.wake(); } changed }
    pub(super) fn fade_external_opacity(&self, voice: VoiceId, target: f32, ms: u32) -> bool { let changed = self.external_sources.lock().ok().and_then(|sources| sources.get(&voice).and_then(|source| source.state.lock().ok().map(|mut state| state.anim.animate_to(target, ms.max(1))))).is_some(); if changed { self.external_dirty.store(true, Ordering::Release); self.wake(); } changed }
    pub(super) fn toggle_fullscreen(&self) { super::render::toggle_fullscreen(self); }
    pub(super) fn set_windowed_floating(&self) { super::render::set_windowed_floating(self); }
    pub(super) fn configure_output_window(
        &self,
        output_id: &str,
        floating: bool,
        geometry: Option<FloatingWindowGeometry>,
        always_on_top: bool,
        hide_cursor: bool,
    ) {
        if let Ok(mut id) = self.output_id.lock() { *id = output_id.to_owned(); }
        self.floating_output.store(floating, Ordering::Relaxed);
        self.floating_placement_dirty.store(true, Ordering::Relaxed);
        if let Ok(mut current) = self.floating_geometry.lock() {
            // Preferences are authoritative when they include a saved rect.
            // For old workspaces `None` deliberately does not erase a rect
            // recorded earlier in this live session.
            if geometry.is_some() || current.is_none() { *current = geometry; }
        }
        self.apply_window_preferences(always_on_top, hide_cursor);
    }
    pub(super) fn window_preferences(&self) -> (bool, bool) {
        (
            self.always_on_top.load(Ordering::Relaxed),
            self.hide_cursor.load(Ordering::Relaxed),
        )
    }
    fn apply_window_preferences(&self, always_on_top: bool, hide_cursor: bool) {
        self.always_on_top.store(always_on_top, Ordering::Relaxed);
        self.hide_cursor.store(hide_cursor, Ordering::Relaxed);
        #[cfg(not(target_os = "macos"))]
        if let Ok(window) = self.window.lock() {
            if let Some(window) = window.as_ref() {
                window.set_window_level(if always_on_top {
                    WindowLevel::AlwaysOnTop
                } else {
                    WindowLevel::Normal
                });
                window.set_cursor_visible(!hide_cursor);
            }
        }
        #[cfg(target_os = "macos")]
        super::macos_window::apply_preferences(self, always_on_top, hide_cursor);
    }
    #[cfg(not(target_os = "macos"))]
    fn floating_geometry(&self) -> FloatingWindowGeometry {
        self.floating_geometry.lock().ok().and_then(|g| g.clone()).unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    fn record_floating_geometry(&self, window: &Window) -> Option<FloatingWindowGeometry> {
        if !self.floating_output.load(Ordering::Relaxed) { return None; }
        let maximized = window.is_maximized();
        let mut current = self.floating_geometry.lock().ok()?;
        let mut next = current.clone().unwrap_or_default();
        // While maximized, preserve the normal rect.  Windows restores this
        // rect after unmaximize and it is what we need on the next launch.
        if !maximized {
            if let Ok(position) = window.outer_position() {
                next.x = position.x;
                next.y = position.y;
            }
            let size = window.inner_size();
            next.width = size.width.max(MIN_FLOATING_WIDTH);
            next.height = size.height.max(MIN_FLOATING_HEIGHT);
        }
        next.maximized = maximized;
        if current.as_ref() == Some(&next) { return None; }
        *current = Some(next.clone());
        Some(next)
    }
    #[cfg(not(target_os = "macos"))]
    fn is_floating(&self) -> bool { self.floating_output.load(Ordering::Relaxed) }
    #[cfg(not(target_os = "macos"))]
    fn take_floating_placement_dirty(&self) -> bool {
        self.floating_placement_dirty.swap(false, Ordering::Relaxed)
    }
    #[cfg(not(target_os = "macos"))]
    fn output_id(&self) -> Option<String> {
        self.output_id.lock().ok().map(|id| id.clone()).filter(|id| !id.is_empty())
    }
}

#[cfg(not(target_os = "macos"))]
struct PendingGeometryPersist {
    app_handle: tauri::AppHandle,
    output_id: String,
    geometry: FloatingWindowGeometry,
    generation: u64,
}

/// One process-global bounded worker coalesces high-frequency native
/// move/resize events from every output. The latest geometry per output
/// replaces the previous one and is written only after a quiet interval, so
/// dragging a window never performs sync disk I/O per event or leaks a worker
/// when an output pipeline is retired.
#[cfg(not(target_os = "macos"))]
struct GeometryPersistQueue {
    pending: Mutex<Vec<PendingGeometryPersist>>,
    latest_generation: Mutex<Vec<(String, u64)>>,
    next_generation: AtomicU64,
    wake: Condvar,
}

#[cfg(not(target_os = "macos"))]
static GEOMETRY_PERSIST_QUEUE: OnceLock<Arc<GeometryPersistQueue>> = OnceLock::new();

#[cfg(not(target_os = "macos"))]
impl GeometryPersistQueue {
    fn start() -> Arc<Self> {
        let queue = Arc::new(Self {
            pending: Mutex::new(Vec::new()),
            latest_generation: Mutex::new(Vec::new()),
            next_generation: AtomicU64::new(0),
            wake: Condvar::new(),
        });
        let worker = Arc::clone(&queue);
        if let Err(error) = std::thread::Builder::new()
            .name("inkue-output-geometry".into())
            .spawn(move || worker.run())
        {
            log::warn!("Could not start output geometry persistence worker: {error}");
        }
        queue
    }

    fn submit(&self, mut pending: PendingGeometryPersist) {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let Ok(mut latest) = self.latest_generation.lock() else { return; };
        if let Some((_, current)) = latest.iter_mut().find(|(id, _)| id == &pending.output_id) {
            *current = generation;
        } else if latest.len() < 64 {
            latest.push((pending.output_id.clone(), generation));
        } else {
            // Do not evict a generation: an already-running worker could then
            // mistake an old event for the latest one and overwrite a newer
            // Close flush. New output IDs are ignored until a known slot is
            // reused; normal output graphs are far below this bound.
            return;
        }
        pending.generation = generation;
        drop(latest);

        let Ok(mut current) = self.pending.lock() else { return; };
        if let Some(existing) = current.iter_mut().find(|existing| existing.output_id == pending.output_id) {
            *existing = pending;
        } else if current.len() < 64 {
            current.push(pending);
        } else {
            // A finite queue cannot grow with untrusted output IDs. Keep the
            // newest geometry while the worker catches up.
            current[0] = pending;
        }
        self.wake.notify_one();
    }

    fn flush(&self) {
        let pending = self.pending.lock().ok().map(|mut current| std::mem::take(&mut *current)).unwrap_or_default();
        for pending in pending {
            self.persist_if_latest(pending);
        }
    }

    fn persist_if_latest(&self, pending: PendingGeometryPersist) {
        if !self.is_latest(&pending.output_id, pending.generation) {
            return;
        }
        let Ok(_write_guard) = crate::state::app_state::GLOBAL_PREFERENCES_WRITE_GATE.lock() else { return; };
        // Close flush and a newer move can race with a worker that already
        // removed an older item from `pending`. Recheck after taking the same
        // global write gate used by every Preferences writer.
        if self.is_latest(&pending.output_id, pending.generation) {
            persist_floating_geometry_now_locked(
                &pending.app_handle,
                &pending.output_id,
                pending.geometry,
            );
        }
    }

    fn is_latest(&self, output_id: &str, generation: u64) -> bool {
        self.latest_generation
            .lock()
            .ok()
            .and_then(|latest| latest.iter().find(|(id, _)| id == output_id).map(|(_, current)| *current == generation))
            .unwrap_or(false)
    }

    fn run(self: Arc<Self>) {
        const QUIET_INTERVAL: Duration = Duration::from_millis(400);
        loop {
            let pending = {
                let mut current = match self.pending.lock() {
                    Ok(current) => current,
                    Err(_) => return,
                };
                while current.is_empty() {
                    current = match self.wake.wait(current) {
                        Ok(current) => current,
                        Err(_) => return,
                    };
                }
                loop {
                    let (next, timeout) = match self.wake.wait_timeout(current, QUIET_INTERVAL) {
                        Ok(result) => result,
                        Err(_) => return,
                    };
                    current = next;
                    if timeout.timed_out() {
                        break Some(std::mem::take(&mut *current));
                    }
                }
            };
            if let Some(pending) = pending {
                for pending in pending {
                    self.persist_if_latest(pending);
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
const MIN_FLOATING_WIDTH: u32 = 320;
#[cfg(not(target_os = "macos"))]
const MIN_FLOATING_HEIGHT: u32 = 240;

/// Force one redraw after an overlay deactivation (timer cleared, Text Cue
/// ended, test pattern cleared).
// ---------------------------------------------------------------------------
// Public helpers called from OutputEngine
// ---------------------------------------------------------------------------

/// Wake the render thread immediately.
///
/// `tick_fade()` self-paces at 16 ms only while an animation is in progress
/// (`current_alpha != target_alpha`).  When a Fade Cue drives the overlay alpha
/// externally at 30 fps — setting `current == target` each step — the loop would
/// otherwise sleep up to 100 ms between redraws.  Calling this on each alpha
/// change keeps that fade smooth.
/// Store new physical window dimensions and wake the render thread so it resizes
/// the GL surface.  Called by the macOS window backend after a screen move /
/// fullscreen toggle (the winit path drives this from its own `Resized` event).
#[cfg(target_os = "macos")]
pub(super) fn set_surface_size(runtime: &RenderRuntime, width: u32, height: u32) {
    runtime.set_size(width, height);
}

pub(super) fn show(runtime: &RenderRuntime) {
    runtime.shared.visible.store(true, Ordering::Relaxed);
    runtime.notify_visibility(true);
    #[cfg(not(target_os = "macos"))]
    if let Ok(w) = runtime.window.lock() { if let Some(w) = w.as_ref() { w.set_visible(true); } }
    #[cfg(target_os = "macos")]
    super::macos_window::show(runtime);
    // Wake the render loop so it commits the first frame immediately.  On
    // Wayland the surface is only mapped once a buffer arrives; without this
    // wake the window would not appear until the next mpv signal (up to 100 ms).
    runtime.wake();
}

pub(super) fn hide(runtime: &RenderRuntime) {
    runtime.shared.visible.store(false, Ordering::Relaxed);
    runtime.notify_visibility(false);
    #[cfg(not(target_os = "macos"))]
    if let Ok(w) = runtime.window.lock() { if let Some(w) = w.as_ref() { w.set_visible(false); } }
    #[cfg(target_os = "macos")]
    super::macos_window::hide(runtime);
}

pub(super) fn toggle_fullscreen(runtime: &RenderRuntime) {
    #[cfg(not(target_os = "macos"))]
    if let Ok(w) = runtime.window.lock() { if let Some(w) = w.as_ref() {
        if w.fullscreen().is_some() {
            w.set_fullscreen(None);
        } else {
            w.set_fullscreen(Some(Fullscreen::Borderless(w.current_monitor())));
        }
    } }
    #[cfg(target_os = "macos")]
    super::macos_window::toggle_fullscreen(runtime);
}

/// Place the output window fullscreen on the monitor whose top-left corner is
/// `(x, y)` — **physical** virtual-screen coordinates from `list_screens()`.
///
/// Uses `Fullscreen::Borderless` on the matched `MonitorHandle` rather than a
/// manual move/resize: the old path passed the physical rect as a *logical*
/// position, which winit multiplies by the current monitor's DPI scale — with
/// any display above 100 % the window landed shifted and oversized (the
/// "output drifts on GO" report). Borderless fullscreen is DPI-proof, covers
/// the taskbar, pins the window to the monitor, and works on Wayland where
/// `set_outer_position` is a no-op.
#[cfg(not(target_os = "macos"))]
pub(super) fn set_fullscreen_on_rect(runtime: &RenderRuntime, x: i32, y: i32, width: u32, height: u32) {
    let Ok(wg) = runtime.window.lock() else { return };
    let Some(w) = wg.as_ref() else { return };
    // A monitor-bound output is deliberately not a normal desktop window.
    // Removing its chrome and resize affordances prevents minimize, drag, and
    // accidental fullscreen exit during a show.
    w.set_maximized(false);
    w.set_decorations(false);
    w.set_resizable(false);
    let monitor = w.available_monitors().find(|m| {
        let p = m.position();
        p.x == x && p.y == y
    });
    match monitor {
        Some(m) => w.set_fullscreen(Some(Fullscreen::Borderless(Some(m)))),
        None => {
            // The compositor reported different coordinates than list_screens()
            // (possible on Wayland). Land on the rect in physical pixels, then
            // fullscreen whatever monitor the window ended up on.
            w.set_fullscreen(None);
            w.set_outer_position(PhysicalPosition::new(x, y));
            let _ = w.request_inner_size(PhysicalSize::new(width, height));
            w.set_fullscreen(Some(Fullscreen::Borderless(None)));
        }
    }
}

/// Place the macOS NSWindow fullscreen onto the given screen index.
#[cfg(target_os = "macos")]
pub(super) fn position_on_screen(runtime: &RenderRuntime, screen_index: u32) {
    super::macos_window::position_on_screen(runtime, screen_index);
}

/// Install (or clear) the global output warp and wake the render thread so
/// the change shows immediately — even on a paused frame or a test pattern.
pub(super) fn set_output_warp(runtime: &RenderRuntime, matrix: Option<[f32; 9]>) {
    runtime.set_warp(matrix);
}

/// Restore the output window to a floating windowed rect — exits the
/// fullscreen-on-screen placement applied by `set_outer_rect` /
/// `position_on_screen` when the operator switches back to "Floating window".
pub(super) fn set_windowed_floating(runtime: &RenderRuntime) {
    #[cfg(not(target_os = "macos"))]
    if let Ok(wg) = runtime.window.lock() { if let Some(w) = wg.as_ref() {
        // GO and preview setup call placement repeatedly.  Only restore when
        // the destination changed (or at first creation); otherwise a user
        // who has just moved/maximized this ordinary window would be snapped
        // back to its previously saved rect on every cue.
        if !runtime.take_floating_placement_dirty() { return; }
        w.set_fullscreen(None);
        // A floating destination is an ordinary operator window: title bar,
        // system move/resize controls, minimize and maximize all stay usable.
        // Physical monitor outputs go through `set_fullscreen_on_rect` and are
        // kept borderless/input-locked instead.
        w.set_decorations(true);
        w.set_resizable(true);
        w.set_maximized(false);

        let rect = clamp_floating_geometry(
            runtime.floating_geometry(),
            &available_monitor_rects(w),
        );
        w.set_outer_position(PhysicalPosition::new(rect.x, rect.y));
        let _ = w.request_inner_size(PhysicalSize::new(rect.width, rect.height));
        if rect.maximized { w.set_maximized(true); }
    } }
    #[cfg(target_os = "macos")]
    super::macos_window::set_windowed(runtime);
}

/// Physical monitor rectangles exposed by winit.  They are the safe portable
/// equivalent of a work area on Linux/macOS; on Windows they additionally keep
/// a restored window wholly on a connected display even after a monitor was
/// unplugged.  The normal title bar still lets Windows keep clear of taskbar
/// auto-hide regions.
#[cfg(not(target_os = "macos"))]
fn available_monitor_rects(window: &Window) -> Vec<FloatingRect> {
    window.available_monitors().map(|monitor| {
        let p = monitor.position();
        let s = monitor.size();
        FloatingRect { x: p.x, y: p.y, width: s.width, height: s.height }
    }).collect()
}

#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FloatingRect { x: i32, y: i32, width: u32, height: u32 }

/// Clamp a saved floating rect to a currently available display.  The display
/// with the greatest overlap wins; if the old display vanished, the first
/// available display is chosen.  This is intentionally pure and unit-tested.
#[cfg(not(target_os = "macos"))]
fn clamp_floating_geometry(
    saved: FloatingWindowGeometry,
    monitors: &[FloatingRect],
) -> FloatingWindowGeometry {
    let Some(target) = monitors.iter().copied().max_by_key(|monitor| {
        overlap_area(saved.x, saved.y, saved.width, saved.height, *monitor)
    }).or_else(|| monitors.first().copied()) else {
        return saved;
    };
    let width = saved.width.clamp(MIN_FLOATING_WIDTH.min(target.width), target.width.max(1));
    let height = saved.height.clamp(MIN_FLOATING_HEIGHT.min(target.height), target.height.max(1));
    let right = target.x.saturating_add(target.width.saturating_sub(width) as i32);
    let bottom = target.y.saturating_add(target.height.saturating_sub(height) as i32);
    FloatingWindowGeometry {
        x: saved.x.clamp(target.x, right),
        y: saved.y.clamp(target.y, bottom),
        width,
        height,
        maximized: saved.maximized,
    }
}

#[cfg(not(target_os = "macos"))]
fn overlap_area(x: i32, y: i32, width: u32, height: u32, monitor: FloatingRect) -> i64 {
    let right = x.saturating_add(width.min(i32::MAX as u32) as i32);
    let bottom = y.saturating_add(height.min(i32::MAX as u32) as i32);
    let monitor_right = monitor.x.saturating_add(monitor.width.min(i32::MAX as u32) as i32);
    let monitor_bottom = monitor.y.saturating_add(monitor.height.min(i32::MAX as u32) as i32);
    let w = (right.min(monitor_right) - x.max(monitor.x)).max(0) as i64;
    let h = (bottom.min(monitor_bottom) - y.max(monitor.y)).max(0) as i64;
    w * h
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Create the output window and spawn the render thread.
///
/// Blocks until `mpv_render_context_create()` succeeds so that no `loadfile`
/// can reach mpv before the render context is live.
pub(super) fn init(
    runtime: Arc<RenderRuntime>,
    app_handle: &tauri::AppHandle,
    lib: Arc<MpvLib>,
    mpv_ctx: Arc<MpvCtx>,
    slot_registry: Arc<slot::SlotRegistry>,
    pipeline: Arc<PipelineState>,
) -> Result<JoinHandle<()>> {
    let (rwh, rdh, width, height) = create_native_window(app_handle, Arc::clone(&runtime))?;
    runtime.set_size(width, height);

    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

    let render_thread = spawn_render_thread(
        SendableHandles { rwh, rdh, width, height },
        lib, mpv_ctx, slot_registry, Arc::clone(&runtime), pipeline, ready_tx,
    )?;

    // On macOS, Tauri's NSApplication event loop hasn't started yet when setup()
    // runs. If glutin/CGL needs the run loop during context creation, blocking
    // here deadlocks: setup() waits for the render thread, the render thread
    // waits for the run loop, the run loop waits for setup() to return.
    // Solution: let the render thread initialise after the event loop starts and
    // watch for errors on a background watcher thread.
    #[cfg(target_os = "macos")]
    std::thread::Builder::new()
        .name("inkue-render-watcher".into())
        .spawn(move || match ready_rx.recv() {
            Ok(Ok(())) => log::info!("[render] macOS GL context ready"),
            Ok(Err(e)) => log::error!("[render] macOS GL init failed: {e}"),
            Err(_) => log::error!("[render] macOS render thread closed before ready"),
        })
        .ok();

    #[cfg(not(target_os = "macos"))]
    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            // The worker owns all GL state.  Even an initialisation failure
            // must be joined before the caller can tear the mpv handle down.
            runtime.request_shutdown();
            let _ = render_thread.join();
            return Err(error);
        }
        Err(_) => {
            runtime.request_shutdown();
            let _ = render_thread.join();
            return Err(anyhow!("render thread exited before signalling ready"));
        }
    }

    Ok(render_thread)
}

// ---------------------------------------------------------------------------
// Sendable raw-handle pair
// ---------------------------------------------------------------------------

struct SendableHandles {
    rwh:    RawWindowHandle,
    rdh:    RawDisplayHandle,
    width:  u32,
    height: u32,
}
// SAFETY: RawWindowHandle / RawDisplayHandle are plain integer/pointer structs.
// The underlying OS objects outlive the render thread (window lives for the app).
unsafe impl Send for SendableHandles {}

// ---------------------------------------------------------------------------
// Window creation — macOS (AppKit NSWindow via objc2)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn create_native_window(
    app_handle: &tauri::AppHandle,
    runtime: Arc<RenderRuntime>,
) -> Result<(RawWindowHandle, RawDisplayHandle, u32, u32)> {
    super::macos_window::create(app_handle, runtime)
}

// ---------------------------------------------------------------------------
// Window creation — winit (Windows + Linux)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// winit ApplicationHandler — output window event loop
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "macos"))]
struct OutputApp {
    windows: HashMap<WindowId, OutputWindow>,
}

#[cfg(not(target_os = "macos"))]
struct OutputWindow {
    window: Arc<Window>,
    /// Emits `output-keydown` so shortcuts keep working with the output focused.
    app_handle: tauri::AppHandle,
    modifiers: ModifiersState,
    runtime: Arc<RenderRuntime>,
}

/// Write a user-adjusted floating rect back into global Preferences. Geometry
/// is machine-wide output UI state, not project content, so no workspace dirty
/// event is emitted and disk I/O never runs while the workspace mutex is held.
#[cfg(not(target_os = "macos"))]
fn persist_floating_geometry_now_locked(
    app_handle: &tauri::AppHandle,
    output_id: &str,
    geometry: FloatingWindowGeometry,
) {
    use tauri::Manager;
    let Some(state) = app_handle.try_state::<crate::state::AppState>() else { return; };
    let Ok(snapshot) = state.global_preferences_snapshot() else { return; };
    let Some(destination) = snapshot
        .display
        .output_destinations
        .iter()
        .find(|destination| destination.id == output_id && destination.monitor.is_none())
    else { return; };
    if destination.floating_window.as_ref() != Some(&geometry) {
        if let Err(error) = state.update_global_preferences(|preferences| {
            if let Some(destination) = preferences
                .display
                .output_destinations
                .iter_mut()
                .find(|destination| destination.id == output_id && destination.monitor.is_none())
            {
                destination.floating_window = Some(geometry.clone());
            }
        }) {
            // Persistence failure leaves both runtime mirrors unchanged.
            log::warn!("Could not persist floating output geometry: {error}");
            return;
        }
        if let Ok(mut workspace) = state.workspace.lock() {
            if let Some(destination) = workspace
                .preferences
                .display
                .output_destinations
                .iter_mut()
                .find(|destination| destination.id == output_id && destination.monitor.is_none())
            {
                destination.floating_window = Some(geometry);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn schedule_floating_geometry_persist(
    app_handle: &tauri::AppHandle,
    runtime: &Arc<RenderRuntime>,
    geometry: FloatingWindowGeometry,
) {
    let Some(output_id) = runtime.output_id() else { return; };
    let queue = GEOMETRY_PERSIST_QUEUE.get_or_init(GeometryPersistQueue::start);
    queue.submit(PendingGeometryPersist {
        app_handle: app_handle.clone(),
        output_id,
        geometry,
        generation: 0,
    });
}

#[cfg(not(target_os = "macos"))]
fn flush_floating_geometry_persist() {
    if let Some(queue) = GEOMETRY_PERSIST_QUEUE.get() { queue.flush(); }
}

#[cfg(not(target_os = "macos"))]
enum OutputCommand {
    Create {
        runtime: Arc<RenderRuntime>,
        app_handle: tauri::AppHandle,
        tx: std::sync::mpsc::Sender<Result<SendableHandles>>,
    },
}

#[cfg(not(target_os = "macos"))]
struct WindowManager {
    proxy: winit::event_loop::EventLoopProxy<OutputCommand>,
}

#[cfg(not(target_os = "macos"))]
static WINDOW_MANAGER: OnceLock<Result<WindowManager, String>> = OnceLock::new();

#[cfg(not(target_os = "macos"))]
fn window_manager() -> Result<&'static WindowManager> {
    WINDOW_MANAGER
        .get_or_init(|| {
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<WindowManager, String>>(1);
            std::thread::Builder::new()
                .name("inkue-output-window".into())
                .spawn(move || {
                    let event_loop = match build_event_loop() {
                        Ok(el) => el,
                        Err(e) => {
                            let _ = ready_tx.send(Err(format!("{e}")));
                            return;
                        }
                    };
                    let proxy = event_loop.create_proxy();
                    let _ = ready_tx.send(Ok(WindowManager { proxy }));
                    let mut app = OutputApp { windows: HashMap::new() };
                    let result = std::panic::catch_unwind(
                        std::panic::AssertUnwindSafe(|| event_loop.run_app(&mut app)),
                    );
                    if result.is_err() {
                        log::error!("[render] output window event loop panicked");
                    }
                })
                .map_err(|e| format!("spawn output-window thread: {e}"))?;
            ready_rx.recv().map_err(|_| "output window thread exited before event loop was ready".to_string())?
        })
        .as_ref()
        .map_err(|e| anyhow!(e.clone()))
}

/// Translate a winit logical key to the DOM `KeyboardEvent.key` string the
/// frontend shortcut handler expects. winit's `NamedKey` variants are named
/// after the DOM UI Events key values, so `Debug` *is* the mapping — except
/// `Space`, which the DOM spells `" "`.
#[cfg(not(target_os = "macos"))]
fn dom_key(key: &Key) -> Option<String> {
    match key {
        Key::Character(c) => Some(c.to_string()),
        Key::Named(NamedKey::Space) => Some(" ".into()),
        Key::Named(n) => Some(format!("{n:?}")),
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
impl ApplicationHandler<OutputCommand> for OutputApp {
    fn resumed(&mut self, _el: &ActiveEventLoop) {}

    fn user_event(&mut self, el: &ActiveEventLoop, command: OutputCommand) {
        let OutputCommand::Create { runtime, app_handle, tx } = command;
        let attrs = WindowAttributes::default()
            .with_title("Qlisa Output")
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(true)
            .with_window_level(if runtime.window_preferences().0 {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            })
            .with_inner_size(LogicalSize::new(1920u32, 1080u32));

        let window = match el.create_window(attrs) {
            Ok(w)  => Arc::new(w),
            Err(e) => {
                let _ = tx.send(Err(anyhow!("create_window: {e}")));
                return;
            }
        };
        window.set_cursor_visible(!runtime.window_preferences().1);

        let rwh: RawWindowHandle = match window.window_handle() {
            Ok(h)  => h.as_raw(),
            Err(e) => {
                let _ = tx.send(Err(anyhow!("window_handle: {e}")));
                return;
            }
        };
        let rdh: RawDisplayHandle = match el.display_handle() {
            Ok(h)  => h.as_raw(),
            Err(e) => {
                let _ = tx.send(Err(anyhow!("display_handle: {e}")));
                return;
            }
        };

        if let Ok(mut w) = runtime.window.lock() { *w = Some(Arc::clone(&window)); }
        let id = window.id();
        let _ = tx.send(Ok(SendableHandles { rwh, rdh, width: 1920, height: 1080 }));
        self.windows.insert(id, OutputWindow {
            window,
            app_handle,
            modifiers: ModifiersState::empty(),
            runtime,
        });
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(output) = self.windows.get_mut(&id) else { return; };
        let window = &output.window;
        match event {
            WindowEvent::CloseRequested => {
                // Monitor-bound outputs must remain pinned and visible.  A
                // floating output is a normal desktop window, so its close
                // button hides it just like F9 does (without destroying GL).
                if output.runtime.is_floating() {
                    output.runtime.hide();
                    flush_floating_geometry_persist();
                }
            }

            WindowEvent::Resized(size) => {
                output.runtime.set_size(size.width, size.height);
                if let Some(geometry) = output.runtime.record_floating_geometry(window) {
                    schedule_floating_geometry_persist(&output.app_handle, &output.runtime, geometry);
                }
            }

            WindowEvent::Moved(_) => {
                if let Some(geometry) = output.runtime.record_floating_geometry(window) {
                    schedule_floating_geometry_persist(&output.app_handle, &output.runtime, geometry);
                }
            }

            WindowEvent::ModifiersChanged(m) => {
                output.modifiers = m.state();
            }

            WindowEvent::KeyboardInput { event, is_synthetic, .. } => {
                // The output window would swallow these otherwise — GO / panic
                // must keep working while the operator has it focused. Forward
                // to the main webview, which replays them into the regular
                // window-level shortcut handler (repeats included, matching
                // native DOM keydown behaviour).
                //
                // `is_synthetic` must be skipped: on Windows, winit fabricates
                // Pressed events for every key physically held when the window
                // gains focus — F9 (show output) activates this window while
                // F9 is still down, and forwarding that ghost press would
                // instantly toggle the window hidden again.
                if event.state == ElementState::Pressed && !is_synthetic {
                    if let Some(key) = dom_key(&event.logical_key) {
                        use tauri::Emitter;
                        let _ = output.app_handle.emit(
                            "output-keydown",
                            serde_json::json!({
                                "key":   key,
                                "ctrl":  output.modifiers.control_key(),
                                "alt":   output.modifiers.alt_key(),
                                "shift": output.modifiers.shift_key(),
                                "meta":  output.modifiers.super_key(),
                            }),
                        );
                    }
                }
            }

            _ => {}
        }
    }
}

/// Build a winit EventLoop that may be created from any thread.
///
/// winit 0.30 guards EventLoop creation to the main thread by default on both
/// Windows and Linux.  Platform-specific extension traits opt out of that guard.
#[cfg(target_os = "windows")]
fn build_event_loop() -> Result<EventLoop<OutputCommand>> {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    EventLoop::<OutputCommand>::with_user_event()
        .with_any_thread(true)
        .build()
        .map_err(|e| anyhow!("EventLoop (Windows): {e}"))
}

/// Probe whether winit's X11 backend can actually run.
///
/// winit's X11 backend hard-requires `libxkbcommon-x11` and **panics** (not a
/// recoverable `build()` error) during window creation if it is absent — common on
/// Wayland-only installs.  We `dlopen` it up-front so `build_event_loop` can choose
/// Wayland cleanly instead of taking down the whole output engine.
#[cfg(target_os = "linux")]
fn x11_xkb_available() -> bool {
    use std::ffi::CString;
    for name in ["libxkbcommon-x11.so.0", "libxkbcommon-x11.so"] {
        let Ok(c) = CString::new(name) else { continue };
        // SAFETY: valid C string; the handle is closed again immediately.
        let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_LAZY) };
        if !h.is_null() {
            unsafe { libc::dlclose(h); }
            return true;
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn build_event_loop() -> Result<EventLoop<OutputCommand>> {
    // Prefer X11/XWayland over native Wayland for the output window.
    //
    // With a *native Wayland* EGL surface, Mesa's `eglSwapBuffers` blocks on the
    // compositor's frame callback regardless of the swap interval, which serialises
    // this output window's render-thread GL with WebKitGTK's UI compositing on the
    // same iGPU — the Inkue UI then crawls for the entire duration of video playback
    // (the failure the operator reported).  XWayland's X11/DRI EGL path honours
    // `SwapInterval::DontWait` and keeps the two GL clients decoupled, so the UI stays
    // fluid while a video plays.
    //
    // X11 is selected only when `libxkbcommon-x11` is present (winit panics otherwise);
    // otherwise we fall back to native Wayland so the app still runs.  Override with
    // `INKUE_OUTPUT_BACKEND=wayland` for A/B testing.
    let force_wayland = std::env::var("INKUE_OUTPUT_BACKEND").as_deref() == Ok("wayland");
    let use_x11 = !force_wayland && x11_xkb_available();

    let mut b = EventLoop::<OutputCommand>::with_user_event();
    if use_x11 {
        use winit::platform::x11::EventLoopBuilderExtX11;
        b.with_any_thread(true).with_x11();
        log::info!("[render] output window backend: X11/XWayland (default)");
        b.build().map_err(|e| anyhow!("EventLoop (Linux/XWayland): {e}"))
    } else {
        use winit::platform::wayland::EventLoopBuilderExtWayland;
        EventLoopBuilderExtWayland::with_any_thread(&mut b, true);
        if force_wayland {
            log::info!("[render] output window backend: native Wayland (forced via INKUE_OUTPUT_BACKEND)");
        } else {
            log::warn!(
                "[render] output window backend: native Wayland — XWayland unavailable \
                 (libxkbcommon-x11 not found); the UI may lag during video playback. \
                 Install the 'libxkbcommon-x11-0' package to enable the smoother XWayland path."
            );
        }
        b.build().map_err(|e| anyhow!("EventLoop (Linux/Wayland): {e}"))
    }
}

/// Unified window creation for Windows and Linux via winit.
#[cfg(not(target_os = "macos"))]
fn create_native_window(
    app_handle: &tauri::AppHandle,
    runtime: Arc<RenderRuntime>,
) -> Result<(RawWindowHandle, RawDisplayHandle, u32, u32)> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<SendableHandles>>();
    let manager = window_manager()?;
    manager.proxy.send_event(OutputCommand::Create {
        runtime,
        app_handle: app_handle.clone(),
        tx,
    }).map_err(|e| anyhow!("send output-window create request: {e}"))?;
    let h = rx.recv()??;
    Ok((h.rwh, h.rdh, h.width, h.height))
}

// ---------------------------------------------------------------------------
// Spawn render thread
// ---------------------------------------------------------------------------

fn spawn_render_thread(
    handles:  SendableHandles,
    lib:      Arc<MpvLib>,
    mpv_ctx:  Arc<MpvCtx>,
    slot_registry: Arc<slot::SlotRegistry>,
    runtime: Arc<RenderRuntime>,
    pipeline: Arc<PipelineState>,
    ready_tx: std::sync::mpsc::Sender<Result<()>>,
) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("inkue-output-render".into())
        .spawn(move || {
            if let Err(e) = render_thread_main(handles, lib, mpv_ctx, slot_registry, runtime, pipeline, ready_tx) {
                log::error!("[render] fatal: {e}");
            }
        })
        .map_err(|e| anyhow!("spawn render thread: {e}"))
}

/// Free every Render API context from the thread that owns the GL context.
/// libmpv requires callbacks to be unregistered before the context is freed;
/// clearing the slot atomics also prevents a stale pointer from being reused
/// if a caller still holds an idle slot while the output is retiring.
struct RenderContextCleanup {
    lib: Arc<MpvLib>,
    overlay_context: *mut c_void,
    slot_registry: Arc<slot::SlotRegistry>,
}

impl Drop for RenderContextCleanup {
    fn drop(&mut self) {
        for slot in self.slot_registry.all_slots() {
            slot.needs_render_init.store(false, Ordering::Release);
            let context = slot.render_ctx.swap(std::ptr::null_mut(), Ordering::AcqRel);
            free_render_context(&self.lib, context);
        }
        free_render_context(&self.lib, self.overlay_context);
    }
}

fn free_render_context(lib: &MpvLib, context: *mut c_void) {
    if context.is_null() {
        return;
    }
    unsafe {
        (lib.mpv_render_context_set_update_callback)(context, None, std::ptr::null_mut());
        (lib.mpv_render_context_free)(context);
    }
}

/// Wait for renderer work, or return `true` as soon as shutdown is requested.
/// Keeping this small primitive separate makes the wake/stop lifecycle testable
/// without creating a native window, GL context, or libmpv instance.
fn wait_for_render_wake_or_shutdown(shared: &RenderShared, timeout: Duration) -> bool {
    if shared.shutting_down.load(Ordering::Acquire) {
        return true;
    }
    let (lock, cvar) = shared.signal.as_ref();
    let mut ready = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !*ready && !shared.shutting_down.load(Ordering::Acquire) {
        let (next, _) = cvar
            .wait_timeout(ready, timeout)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ready = next;
    }
    *ready = false;
    shared.shutting_down.load(Ordering::Acquire)
}

// ---------------------------------------------------------------------------
// Render thread
// ---------------------------------------------------------------------------

fn render_thread_main(
    handles:  SendableHandles,
    lib:      Arc<MpvLib>,
    mpv_ctx:  Arc<MpvCtx>,
    slot_registry: Arc<slot::SlotRegistry>,
    runtime: Arc<RenderRuntime>,
    pipeline: Arc<PipelineState>,
    ready_tx: std::sync::mpsc::Sender<Result<()>>,
) -> Result<()> {
    macro_rules! try_init {
        ($expr:expr) => {
            match $expr {
                Ok(v) => v,
                Err(e) => {
                    let msg = format!("{e}");
                    let _ = ready_tx.send(Err(anyhow!("{msg}")));
                    return Err(anyhow!("{msg}"));
                }
            }
        };
    }

    // ── 1. glutin Display ────────────────────────────────────────────────────
    let display = try_init!(create_display(handles.rdh, handles.rwh));

    // ── 2. GL config ─────────────────────────────────────────────────────────
    let config_tpl = ConfigTemplateBuilder::new()
        .compatible_with_native_window(handles.rwh)
        .with_alpha_size(8)
        .build();
    let config = try_init!(unsafe {
        display.find_configs(config_tpl)
            .map_err(|e| anyhow!("find_configs: {e}"))?
            .next()
            .ok_or_else(|| anyhow!("no compatible GL config found"))
    });

    // ── 3. Context (OpenGL Core, not yet current) ────────────────────────────
    // macOS exposes only 3.2 and 4.1 core profiles (no 3.3); request 3.2 there.
    // Our shaders are `#version 150 core`, which both 3.2 and 3.3 contexts accept.
    #[cfg(target_os = "macos")]
    let gl_version = Version::new(3, 2);
    #[cfg(not(target_os = "macos"))]
    let gl_version = Version::new(3, 3);
    let ctx_attrs = ContextAttributesBuilder::new()
        .with_context_api(ContextApi::OpenGl(Some(gl_version)))
        .build(Some(handles.rwh));
    let not_current = try_init!(unsafe {
        display.create_context(&config, &ctx_attrs)
            .map_err(|e| anyhow!("create_context: {e}"))
    });

    // ── 4. Window surface ─────────────────────────────────────────────────────
    let w0 = NonZeroU32::new(handles.width).unwrap_or(NonZeroU32::new(1).unwrap());
    let h0 = NonZeroU32::new(handles.height).unwrap_or(NonZeroU32::new(1).unwrap());
    let surf_attrs = SurfaceAttributesBuilder::<WindowSurface>::new()
        .with_srgb(Some(false))
        .build(handles.rwh, w0, h0);
    let surface = try_init!(unsafe {
        display.create_window_surface(&config, &surf_attrs)
            .map_err(|e| anyhow!("create_window_surface: {e}"))
    });

    // ── 5. Make context current on THIS thread ────────────────────────────────
    let ctx = try_init!(not_current.make_current(&surface)
        .map_err(|e| anyhow!("make_current: {e}")));

    // ── 6. vsync ──────────────────────────────────────────────────────────────
    // DontWait on every OS. mpv's own clock (video-sync=desync) paces playback, so
    // our swap is not the timing source — blocking on the driver's vblank only adds
    // a redundant sync point.
    //
    // Do NOT switch Linux to SwapInterval::Wait(1): on Mesa/Wayland with a weak
    // shared-memory iGPU, blocking inside eglSwapBuffers holds a driver lock for the
    // whole vblank wait, serialising this render thread's GL with WebKitGTK's
    // compositing on the main thread — which starved the Inkue UI to ~1 fps for the
    // entire duration of video playback (regression seen 2026-06; reverted). Under a
    // VM with an emulated vblank the same block can stall the whole desktop.
    if let Err(e) = surface.set_swap_interval(&ctx, SwapInterval::DontWait) {
        log::warn!("[render] swap_interval: {e:?}");
    }

    // ── 7. glow GL loader ─────────────────────────────────────────────────────
    // Used only on this render thread — no Arc/sharing needed.
    let display_box = Box::new(display);
    let gl = unsafe {
        glow::Context::from_loader_function_cstr(|name| {
            display_box.get_proc_address(name) as *const _
        })
    };

    // ── 8. Fade-quad + warp shaders ───────────────────────────────────────────
    let (fade_program, fade_vao) = build_fade_shader(&gl)?;
    let (warp_program, warp_vao) = build_warp_shader(&gl)?;

    // ── 9. mpv render context with OpenGL backend ─────────────────────────────
    let display_ptr = &*display_box as *const Display as *mut c_void;
    let mut gl_init = MpvOpenglInitParams {
        get_proc_address:     gl_get_proc_address,
        get_proc_address_ctx: display_ptr,
    };
    let api_str = CString::new("opengl").unwrap();
    let flip_y: i32 = 1;
    let params = [
        MpvRenderParam { type_: MPV_RENDER_PARAM_API_TYPE,           data: api_str.as_ptr() as *mut c_void },
        MpvRenderParam { type_: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, data: &mut gl_init as *mut _ as *mut c_void },
        MpvRenderParam { type_: 0, data: std::ptr::null_mut() },
    ];
    let mut render_ctx: *mut c_void = std::ptr::null_mut();
    let ret = unsafe { (lib.mpv_render_context_create)(&mut render_ctx, mpv_ctx.0, params.as_ptr()) };
    if ret < 0 {
        let _ = ready_tx.send(Err(anyhow!("mpv_render_context_create: {ret}")));
        return Err(anyhow!("mpv_render_context_create: {ret}"));
    }
    log::info!("[render] mpv render context created (OpenGL {}.{} Core)", gl_version.major, gl_version.minor);
    let _ = ready_tx.send(Ok(()));

    // ── 10. Update callback ───────────────────────────────────────────────────
    let signal_ptr = Arc::as_ptr(&runtime.shared.signal) as *mut c_void;
    unsafe { (lib.mpv_render_context_set_update_callback)(render_ctx, Some(on_mpv_update), signal_ptr); }
    // This guard is declared after the current GL context, so every render
    // context is released on this thread while that GL context is still live.
    // It also covers fallible shader setup below.
    let _render_context_cleanup = RenderContextCleanup {
        lib: Arc::clone(&lib),
        overlay_context: render_ctx,
        slot_registry: Arc::clone(&slot_registry),
    };

    // ── 11. Render loop ───────────────────────────────────────────────────────
    let mut w_px = handles.width;
    let mut h_px = handles.height;

    // Layer compositor state: the overlay context (timer OSD / Text Cue /
    // test patterns) renders into its own target like every video slot; the
    // ping-pong pair accumulates the blend stack.
    let (composite_program, composite_vao) = build_composite_shader(&gl)?;
    let (blit_program, blit_vao) = build_blit_shader(&gl)?;
    let (geometry_program, geometry_vao) = build_geometry_shader(&gl)?;
    let mut overlay_target: Option<WarpTarget> = None;
    let mut slot_targets: Vec<Option<WarpTarget>> = Vec::new();
    let mut slot_valid: Vec<bool> = Vec::new();
    let mut pingpong: [Option<WarpTarget>; 2] = [None, None];
    // Network destinations retain their native handle only to host an OpenGL
    // context.  Their program image is rendered to this FBO, never presented
    // to the hidden window.
    let mut network_target: Option<WarpTarget> = None;
    let mut network_readback: Option<NetworkReadback> = None;
    let mut monitor_target: Option<WarpTarget> = None;
    let mut monitor_readback: Option<NetworkReadback> = None;
    let mut last_monitor_capture = std::time::Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(std::time::Instant::now);
    let mut external_targets: HashMap<VoiceId, ExternalFrameTarget> = HashMap::new();
    let mut external_layer_target: Option<WarpTarget> = None;

    // Opt-in output frame-rate cap (Linux).  `INKUE_OUTPUT_FPS=30` makes the render
    // loop present at most ~30 fps, halving the output window's GPU compositing load so
    // a weak shared-memory iGPU keeps headroom for the WebKitGTK UI during playback.
    // Off by default (0/unset = uncapped) — it trades some video smoothness, so it is a
    // knob the operator turns on only if the UI still lags after hwdec/XWayland.
    #[cfg(target_os = "linux")]
    let min_present_interval: Option<Duration> = std::env::var("INKUE_OUTPUT_FPS").ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&fps| fps > 0)
        .map(|fps| Duration::from_micros(1_000_000 / fps as u64));
    #[cfg(target_os = "linux")]
    if let Some(iv) = min_present_interval {
        log::info!("[render] output FPS cap enabled: ~{} fps", 1_000_000 / iv.as_micros().max(1) as u64);
    }
    #[cfg(target_os = "linux")]
    let mut last_present = std::time::Instant::now();

    'render: loop {
        if runtime.is_shutting_down() {
            break 'render;
        }
        // Create render contexts for slots the engine spawned since last pass.
        let slots = slot_snapshot(&slot_registry);
        for s in &slots {
            if runtime.is_shutting_down() {
                break 'render;
            }
            if s.needs_render_init.swap(false, Ordering::AcqRel) {
                let mut gl_init2 = MpvOpenglInitParams {
                    get_proc_address:     gl_get_proc_address,
                    get_proc_address_ctx: display_ptr,
                };
                let api_str2 = CString::new("opengl").unwrap();
                let params2 = [
                    MpvRenderParam { type_: MPV_RENDER_PARAM_API_TYPE,           data: api_str2.as_ptr() as *mut c_void },
                    MpvRenderParam { type_: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, data: &mut gl_init2 as *mut _ as *mut c_void },
                    MpvRenderParam { type_: 0, data: std::ptr::null_mut() },
                ];
                let mut rc: *mut c_void = std::ptr::null_mut();
                let ret = unsafe { (lib.mpv_render_context_create)(&mut rc, s.mpv_ctx.0, params2.as_ptr()) };
                if ret < 0 {
                    log::error!("[render] slot {} render context failed: {ret}", s.index);
                } else {
                    unsafe { (lib.mpv_render_context_set_update_callback)(rc, Some(on_mpv_update), signal_ptr); }
                    s.render_ctx.store(rc, Ordering::Release);
                    log::info!("[render] slot {} render context created", s.index);
                }
            }
        }

        // Per-slot opacity animations pace the loop at 16 ms just like the
        // master fade.
        let master_animating = pipeline.fade.lock().ok()
            .map(|s| s.current_alpha != s.target_alpha)
            .unwrap_or(false);
        let operator_ftb_animating = pipeline.operator_ftb.lock().ok()
            .map(|s| s.current_alpha != s.target_alpha)
            .unwrap_or(false);
        let slots_animating = slots.iter().any(|s| {
            s.state.lock().map(|st| st.anim.is_animating()).unwrap_or(false)
        });
        let needs_animation = master_animating || operator_ftb_animating || slots_animating || runtime.external_animating();
        let timeout = if needs_animation { Duration::from_millis(16) } else { Duration::from_millis(100) };

        if wait_for_render_wake_or_shutdown(&runtime.shared, timeout) {
            break 'render;
        }

        // Apply pending resize from the event loop / window backend.
        let new_w = runtime.shared.width.load(Ordering::Relaxed).max(1);
        let new_h = runtime.shared.height.load(Ordering::Relaxed).max(1);
        if new_w != w_px || new_h != h_px {
            surface.resize(
                &ctx,
                NonZeroU32::new(new_w).unwrap(),
                NonZeroU32::new(new_h).unwrap(),
            );
            w_px = new_w;
            h_px = new_h;
        }

        let (alpha, done) = fade::tick_fade(&pipeline);
        if done { fade::execute_pending(&pipeline); }
        let (operator_alpha, _) = fade::tick_operator_blackout(&pipeline);

        if runtime.is_shutting_down() {
            break 'render;
        }

        // Overlay context (timer OSD / Text Cue / test patterns / win32 path).
        let flags     = unsafe { (lib.mpv_render_context_update)(render_ctx) };
        let has_frame = flags & MPV_RENDER_UPDATE_FRAME != 0;
        let text_active = runtime.shared.text_overlay_active.load(Ordering::Relaxed);
        // Warp params changed since the last pass — must redraw even without a
        // new mpv frame (paused video / held image), or alignment edits would
        // only show on the next frame.
        let warp_dirty = runtime.shared.warp_dirty.swap(false, Ordering::Relaxed)
            || runtime.shared.overlay_dirty.swap(false, Ordering::Relaxed);

        // Tick each slot: advance opacity anims, finish pending unloads, and
        // check for fresh frames.  Ticks must run even while hidden so stop
        // fades can finish, but rendering below is gated on visibility.
        let slots = slot_snapshot(&slot_registry);
        struct LayerDraw {
            slot_index: usize,
            layer_key: u64,
            opacity: f32,
            blend_mode: i32,
            has_new_frame: bool,
            render_ctx: *mut c_void,
        }
        let mut layers: Vec<LayerDraw> = Vec::with_capacity(slots.len());
        let mut any_slot_frame = false;
        for s in &slots {
            if runtime.is_shutting_down() {
                break 'render;
            }
            let rc = s.render_ctx.load(Ordering::Acquire);
            if rc.is_null() {
                continue;
            }
            let sflags = unsafe { (lib.mpv_render_context_update)(rc) };
            let s_new_frame = sflags & MPV_RENDER_UPDATE_FRAME != 0;
            let (opacity, _still_animating) = slot::tick_slot(s);
            let Some((voice, layer_key, blend_mode)) = s
                .state
                .lock()
                .ok()
                .map(|st| (st.voice_id, st.layer_key, st.blend_mode.shader_id()))
            else { continue };
            if voice.is_none() {
                if let Some(v) = slot_valid.get_mut(s.index) { *v = false; }
                continue;
            }
            any_slot_frame |= s_new_frame;
            layers.push(LayerDraw {
                slot_index: s.index,
                layer_key,
                opacity,
                blend_mode,
                has_new_frame: s_new_frame,
                render_ctx: rc,
            });
        }
        layers.sort_by_key(|l| l.layer_key);
        let (external_sources, external_animating, external_dirty) = runtime.tick_external_sources();
        let external_active = !external_sources.is_empty();
        let current_external: std::collections::HashSet<_> = external_sources.iter().map(|source| source.voice).collect();
        external_targets.retain(|voice, target| { if current_external.contains(voice) { true } else { unsafe { gl.delete_framebuffer(target.target.fbo); gl.delete_texture(target.target.tex); } false } });

        if runtime.is_shutting_down() {
            break 'render;
        }

        // Do not commit frames while the output window is hidden.  On Wayland
        // a wl_surface.commit() with a buffer permanently maps the surface, so
        // a single frame emitted before show_output() would make the window
        // appear at startup instead of staying invisible until the operator
        // opens it.  show() sets this flag and wakes the loop so the first
        // committed frame arrives immediately when the window is revealed.
        let network_sink = runtime.network_frame_sink();
        let capture_active = network_sink.is_some()
            && runtime.shared.network_capture_active.load(Ordering::Relaxed);
        let monitor_capture_active = runtime.shared.monitor_capture_active.load(Ordering::Relaxed);
        let output_visible = runtime.shared.visible.load(Ordering::Relaxed);
        if !output_visible && !capture_active && !monitor_capture_active { continue; }
        let monitor_capture_due = monitor_capture_active
            && last_monitor_capture.elapsed() >= Duration::from_millis(250);
        // Skip rendering when nothing changed anywhere: no new frame from any
        // mpv, no animation, no active layers or overlay work.  Text/timer
        // overlays render unconditionally (mpv does not signal OSD-only
        // changes in idle mode).
        if !has_frame && !any_slot_frame && alpha == 0 && operator_alpha == 0 && !text_active && !warp_dirty
            && !needs_animation && !external_animating && !external_dirty && layers.is_empty()
            && !external_active && !monitor_capture_due { continue; }
        // Hidden display outputs render only for a due monitor sample or an
        // actual source change. This avoids turning a held image into a
        // full-resolution 10 fps compositor workload merely because the
        // operator opened the monitor window.
        if !output_visible && !capture_active && monitor_capture_active && !monitor_capture_due {
            continue;
        }

        // Opt-in FPS cap: drop video frames arriving faster than the target interval.
        // Never throttle a fade animation or a Text overlay redraw (must stay smooth);
        // mpv wakes us again on the next frame, so the latest one still presents.
        #[cfg(target_os = "linux")]
        if let Some(iv) = min_present_interval {
            if !needs_animation && !text_active && last_present.elapsed() < iv {
                continue;
            }
        }

        // ── Size all offscreen targets ────────────────────────────────────────
        let mut targets_ok = ensure_warp_target(&gl, &mut overlay_target, w_px, h_px).is_ok();
        targets_ok &= ensure_warp_target(&gl, &mut pingpong[0], w_px, h_px).is_ok();
        targets_ok &= ensure_warp_target(&gl, &mut pingpong[1], w_px, h_px).is_ok();
        if capture_active {
            targets_ok &= ensure_warp_target(&gl, &mut network_target, w_px, h_px).is_ok();
            if network_readback.is_none() {
                match NetworkReadback::new(&gl) {
                    Ok(readback) => network_readback = Some(readback),
                    Err(error) => {
                        log::warn!("[render] network readback unavailable: {error}");
                        runtime.shared.network_capture_active.store(false, Ordering::Relaxed);
                        continue;
                    }
                }
            }
        }
        if monitor_capture_active {
            let (preview_w, preview_h) = monitor_dimensions(w_px, h_px);
            targets_ok &= ensure_warp_target(&gl, &mut monitor_target, preview_w, preview_h).is_ok();
            if monitor_readback.is_none() {
                match NetworkReadback::new(&gl) {
                    Ok(readback) => monitor_readback = Some(readback),
                    Err(error) => {
                        log::warn!("[render] output monitor readback unavailable: {error}");
                        runtime.shared.monitor_capture_active.store(false, Ordering::Relaxed);
                    }
                }
            }
        }
        if external_active { targets_ok &= ensure_warp_target(&gl, &mut external_layer_target, w_px, h_px).is_ok(); }
        if slot_targets.len() < slots.len() {
            slot_targets.resize_with(slots.len(), || None);
            slot_valid.resize(slots.len(), false);
        }
        for l in &layers {
            if let Some(t) = slot_targets.get_mut(l.slot_index) {
                targets_ok &= ensure_warp_target(&gl, t, w_px, h_px).is_ok();
            }
        }
        if !targets_ok {
            log::warn!("[render] compositor targets unavailable — skipping frame");
            continue;
        }

        // Apply the newest NDI/external frame on the render thread.  Workers
        // only own a mailbox and therefore cannot corrupt this GL context.
        for source in &external_sources {
            // A Camera Cue can target more than one output.  Each render
            // pipeline has its own cursor, so reading a frame here must not
            // consume it for another pipeline.  The mailbox still keeps only
            // the newest frame: a slow display drops stale frames rather than
            // building latency.
            let cursor = source.cursor.load(Ordering::Acquire);
            if let Some((sequence, frame)) = source.mailbox.latest_after(cursor) {
                let mut target = external_targets.remove(&source.voice);
                match upload_external_frame(&gl, &mut target, frame) {
                    Ok(()) => {
                        if let Some(target) = target {
                            external_targets.insert(source.voice, target);
                        }
                        source.cursor.store(sequence, Ordering::Release);
                    }
                    Err(error) => {
                        log::warn!("[render] external frame upload: {error}");
                        if let Some(target) = target { external_targets.insert(source.voice, target); }
                    }
                }
            }
        }

        // ── Render mpv contexts into their targets ────────────────────────────
        // Overlay: render every pass **while it shows something** (timer OSD /
        // Text Cue / test pattern) — OSD-only changes never signal a new
        // frame.  While inactive it is neither rendered nor composited: mpv's
        // *idle* render clears the target to opaque black on some libmpv
        // builds (`background=none` ignored in idle — measured on 0.41-dev,
        // Windows), which would mask every video layer below.
        // All render calls pass block_for_target_time=0: the default (1) makes
        // each call sleep until *that* context's frame display time, and with
        // several contexts sharing this one thread the waits serialise — two
        // simultaneous videos stuttered even when one was fully transparent.
        // Our loop is paced by the update callbacks instead; each context just
        // hands over its current frame (video-sync=desync owns the clock).
        let mut no_block: i32 = 0;
        let overlay_on = super::overlay_active(&pipeline);
        if let (true, Some(t)) = (overlay_on, &overlay_target) {
            let mut fbo = MpvOpenglFbo { fbo: t.fbo.0.get() as i32, w: w_px as i32, h: h_px as i32, internal_format: 0 };
            let mut flip = flip_y;
            let rp = [
                MpvRenderParam { type_: MPV_RENDER_PARAM_OPENGL_FBO, data: &mut fbo  as *mut _ as *mut c_void },
                MpvRenderParam { type_: MPV_RENDER_PARAM_FLIP_Y,     data: &mut flip as *mut _ as *mut c_void },
                MpvRenderParam { type_: MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME, data: &mut no_block as *mut _ as *mut c_void },
                MpvRenderParam { type_: 0, data: std::ptr::null_mut() },
            ];
            let ret = unsafe { (lib.mpv_render_context_render)(render_ctx, rp.as_ptr()) };
            if ret < 0 { log::warn!("[render] overlay render: {ret}"); }
        }
        let overlay_valid = overlay_on;

        for l in &layers {
            if runtime.is_shutting_down() {
                break 'render;
            }
            let needs = l.has_new_frame || !slot_valid.get(l.slot_index).copied().unwrap_or(false);
            if !needs {
                continue;
            }
            if let Some(Some(t)) = slot_targets.get(l.slot_index) {
                let mut fbo = MpvOpenglFbo { fbo: t.fbo.0.get() as i32, w: w_px as i32, h: h_px as i32, internal_format: 0 };
                let mut flip = flip_y;
                let rp = [
                    MpvRenderParam { type_: MPV_RENDER_PARAM_OPENGL_FBO, data: &mut fbo  as *mut _ as *mut c_void },
                    MpvRenderParam { type_: MPV_RENDER_PARAM_FLIP_Y,     data: &mut flip as *mut _ as *mut c_void },
                    MpvRenderParam { type_: MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME, data: &mut no_block as *mut _ as *mut c_void },
                    MpvRenderParam { type_: 0, data: std::ptr::null_mut() },
                ];
                let ret = unsafe { (lib.mpv_render_context_render)(l.render_ctx, rp.as_ptr()) };
                if ret < 0 { log::warn!("[render] slot {} render: {ret}", l.slot_index); }
                if let Some(v) = slot_valid.get_mut(l.slot_index) { *v = true; }
            }
        }

        // ── Composite the layer stack (ping-pong) ─────────────────────────────
        // Base = opaque black; each layer blends over the accumulated result.
        let mut src = 0usize; // pingpong[src] holds the accumulated composite
        unsafe {
            let base = pingpong[src].as_ref().map(|t| t.fbo);
            gl.bind_framebuffer(glow::FRAMEBUFFER, base);
            gl.viewport(0, 0, w_px as i32, h_px as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        let (mut slot_at, mut external_at) = (0usize, 0usize);
        while slot_at < layers.len() || external_at < external_sources.len() {
            let external_next = match (layers.get(slot_at), external_sources.get(external_at)) { (Some(slot), Some(external)) => external.layer_key < slot.layer_key, (None, Some(_)) => true, _ => false };
            let dst = 1 - src;
            let (backdrop_tex, dst_fbo) = match (&pingpong[src], &pingpong[dst]) { (Some(a), Some(b)) => (a.tex, b.fbo), _ => break };
            if external_next {
                let external = &external_sources[external_at]; external_at += 1;
                if external.opacity <= 0.0 { continue; }
                let (Some(texture), Some(staging)) = (external_targets.get(&external.voice), external_layer_target.as_ref()) else { continue };
                draw_geometry_pass(&gl, geometry_program, geometry_vao, texture.target.tex, external.geometry, texture.target.w, texture.target.h, w_px, h_px, staging.fbo);
                draw_composite_pass(&gl, composite_program, composite_vao, backdrop_tex, staging.tex, external.blend_mode, external.opacity, w_px, h_px, dst_fbo);
            } else {
                let layer = &layers[slot_at]; slot_at += 1;
                if layer.opacity <= 0.0 { continue; }
                let Some(Some(layer_t)) = slot_targets.get(layer.slot_index) else { continue };
                draw_composite_pass(&gl, composite_program, composite_vao, backdrop_tex, layer_t.tex, layer.blend_mode, layer.opacity, w_px, h_px, dst_fbo);
            }
            src = dst;
        }
        // Overlay (timer / text / patterns) on top — only while active.
        if overlay_valid {
            if let Some(t) = &overlay_target {
                let dst = 1 - src;
                if let (Some(a), Some(b)) = (&pingpong[src], &pingpong[dst]) {
                    draw_composite_pass(
                        &gl, composite_program, composite_vao,
                        a.tex, t.tex, 0, 1.0, w_px, h_px, b.fbo,
                    );
                    src = dst;
                }
            }
        }

        // ── Present / capture: warp (or plain blit) + master fade quad ───────
        let warp = runtime.shared.warp.lock().ok().and_then(|g| *g);
        let final_tex = pingpong[src].as_ref().map(|t| t.tex);
        if capture_active {
            if let (Some(tex), Some(target), Some(sink), Some(readback)) = (
                final_tex,
                network_target.as_ref(),
                network_sink.as_ref(),
                network_readback.as_mut(),
            ) {
                match warp {
                    Some(hinv) => draw_warp_pass_to(&gl, warp_program, warp_vao, tex, &hinv, w_px, h_px, Some(target.fbo)),
                    None => draw_blit_pass_to(&gl, blit_program, blit_vao, tex, w_px, h_px, Some(target.fbo)),
                }
                unsafe {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.fbo));
                    gl.viewport(0, 0, w_px as i32, h_px as i32);
                }
                if alpha > 0 { draw_fade_quad(&gl, fade_program, fade_vao, alpha as f32 / 255.0); }
                if operator_alpha > 0 { draw_fade_quad(&gl, fade_program, fade_vao, operator_alpha as f32 / 255.0); }
                readback.publish_completed(&gl, sink);
                readback.queue(&gl, target.fbo, w_px, h_px);

                // A network-only destination must never commit a hidden
                // Wayland surface.  If a future destination combines display
                // and network capture, present the already-final FBO.
                if output_visible {
                    draw_blit_pass_to(&gl, blit_program, blit_vao, target.tex, w_px, h_px, None);
                    if let Err(e) = surface.swap_buffers(&ctx) { log::warn!("[render] swap: {e:?}"); }
                } else {
                    unsafe { gl.flush(); }
                }
            }
        } else if output_visible {
            if let Some(tex) = final_tex {
                match warp {
                    Some(hinv) => draw_warp_pass_to(&gl, warp_program, warp_vao, tex, &hinv, w_px, h_px, None),
                    None => draw_blit_pass_to(&gl, blit_program, blit_vao, tex, w_px, h_px, None),
                }
            }
            if alpha > 0 { draw_fade_quad(&gl, fade_program, fade_vao, alpha as f32 / 255.0); }
            if operator_alpha > 0 { draw_fade_quad(&gl, fade_program, fade_vao, operator_alpha as f32 / 255.0); }
            if let Err(e) = surface.swap_buffers(&ctx) { log::warn!("[render] swap: {e:?}"); }
        }

        // Tap the already-composited texture for the operator monitor.  This
        // is one small GPU draw plus asynchronous readback, never another
        // mpv context or a second media decode.
        if monitor_capture_active {
            if let Some(readback) = monitor_readback.as_mut() {
                readback.publish_completed_to_monitor(&gl, &runtime.monitor_frames);
                if monitor_capture_due {
                    let (preview_w, preview_h) = monitor_dimensions(w_px, h_px);
                    if let (Some(tex), Some(target)) = (final_tex, monitor_target.as_ref()) {
                        match warp {
                            Some(hinv) => draw_warp_pass_to(
                                &gl, warp_program, warp_vao, tex, &hinv,
                                preview_w, preview_h, Some(target.fbo),
                            ),
                            None => draw_blit_pass_to(
                                &gl, blit_program, blit_vao, tex,
                                preview_w, preview_h, Some(target.fbo),
                            ),
                        }
                        unsafe {
                            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.fbo));
                            gl.viewport(0, 0, preview_w as i32, preview_h as i32);
                        }
                        if alpha > 0 {
                            draw_fade_quad(&gl, fade_program, fade_vao, alpha as f32 / 255.0);
                        }
                        if operator_alpha > 0 {
                            draw_fade_quad(&gl, fade_program, fade_vao, operator_alpha as f32 / 255.0);
                        }
                        readback.queue(&gl, target.fbo, preview_w, preview_h);
                        unsafe { gl.flush(); }
                        last_monitor_capture = std::time::Instant::now();
                    }
                }
            }
        }
        if runtime.is_shutting_down() {
            break 'render;
        }
        unsafe { (lib.mpv_render_context_report_swap)(render_ctx); }
        for l in &layers {
            if runtime.is_shutting_down() {
                break 'render;
            }
            if l.has_new_frame {
                unsafe { (lib.mpv_render_context_report_swap)(l.render_ctx); }
            }
        }
        #[cfg(target_os = "linux")]
        { last_present = std::time::Instant::now(); }
    }

    Ok(())
}

/// Snapshot of the slot registry for one render pass.
fn slot_snapshot(registry: &slot::SlotRegistry) -> Vec<Arc<super::slot::VideoSlot>> {
    registry.all_slots()
}

// ---------------------------------------------------------------------------
// GL proc-address bridge for mpv
// ---------------------------------------------------------------------------

unsafe extern "C" fn gl_get_proc_address(user_ctx: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void {
    let display = unsafe { &*(user_ctx as *const Display) };
    let cname   = unsafe { CStr::from_ptr(name) };
    display.get_proc_address(cname) as *mut c_void
}

// ---------------------------------------------------------------------------
// mpv update callback
// ---------------------------------------------------------------------------

unsafe extern "C" fn on_mpv_update(ctx: *mut c_void) {
    if ctx.is_null() { return; }
    let signal = unsafe { &*(ctx as *const (Mutex<bool>, Condvar)) };
    if let Ok(mut ready) = signal.0.lock() {
        *ready = true;
        signal.1.notify_one();
    }
}

// ---------------------------------------------------------------------------
// Platform-specific glutin Display creation
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn create_display(rdh: RawDisplayHandle, _rwh: RawWindowHandle) -> Result<Display> {
    // Pass None so glutin uses its own temporary invisible window for WGL
    // extension loading — avoids double SetPixelFormat on our actual HWND.
    let display = unsafe {
        Display::new(rdh, DisplayApiPreference::WglThenEgl(None))
            .map_err(|e| anyhow!("WGL display: {e}"))?
    };
    Ok(display)
}

#[cfg(target_os = "macos")]
fn create_display(rdh: RawDisplayHandle, _rwh: RawWindowHandle) -> Result<Display> {
    let display = unsafe {
        Display::new(rdh, DisplayApiPreference::Cgl)
            .map_err(|e| anyhow!("CGL display: {e}"))?
    };
    Ok(display)
}

#[cfg(target_os = "linux")]
fn create_display(rdh: RawDisplayHandle, _rwh: RawWindowHandle) -> Result<Display> {
    // Try EGL first (works on both X11 and Wayland), fall back to GLX (X11 only).
    let display = unsafe {
        Display::new(rdh, DisplayApiPreference::EglThenGlx(Box::new(|_| {})))
            .map_err(|e| anyhow!("EGL/GLX display: {e}"))?
    };
    Ok(display)
}

// ---------------------------------------------------------------------------
// Fade-quad shader (fullscreen black triangle)
// ---------------------------------------------------------------------------

fn build_fade_shader(gl: &glow::Context) -> Result<(glow::Program, glow::VertexArray)> {
    // `#version 150 core` is the highest GLSL accepted by macOS's 3.2 core profile,
    // and is a strict subset of what the Windows/Linux 3.3 contexts accept — one
    // shader for all three. `gl_VertexID` + const array constructors are valid in 150.
    const VERT: &str = r#"
#version 150 core
const vec2 POS[3] = vec2[3](vec2(-1,-1), vec2(3,-1), vec2(-1,3));
void main() { gl_Position = vec4(POS[gl_VertexID], 0.0, 1.0); }
"#;
    const FRAG: &str = r#"
#version 150 core
uniform float u_alpha;
out vec4 color;
void main() { color = vec4(0.0, 0.0, 0.0, u_alpha); }
"#;
    unsafe {
        let vs = gl.create_shader(glow::VERTEX_SHADER).map_err(|e| anyhow!("{e}"))?;
        gl.shader_source(vs, VERT);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) { return Err(anyhow!("vert: {}", gl.get_shader_info_log(vs))); }

        let fs = gl.create_shader(glow::FRAGMENT_SHADER).map_err(|e| anyhow!("{e}"))?;
        gl.shader_source(fs, FRAG);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) { return Err(anyhow!("frag: {}", gl.get_shader_info_log(fs))); }

        let prog = gl.create_program().map_err(|e| anyhow!("{e}"))?;
        gl.attach_shader(prog, vs); gl.attach_shader(prog, fs);
        gl.link_program(prog);
        if !gl.get_program_link_status(prog) { return Err(anyhow!("link: {}", gl.get_program_info_log(prog))); }
        gl.detach_shader(prog, vs); gl.delete_shader(vs);
        gl.detach_shader(prog, fs); gl.delete_shader(fs);

        let vao = gl.create_vertex_array().map_err(|e| anyhow!("{e}"))?;
        log::info!("[render] fade shader compiled");
        Ok((prog, vao))
    }
}

fn draw_fade_quad(gl: &glow::Context, program: glow::Program, vao: glow::VertexArray, alpha: f32) {
    unsafe {
        gl.enable(glow::BLEND);
        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
        gl.use_program(Some(program));
        if let Some(loc) = gl.get_uniform_location(program, "u_alpha") {
            gl.uniform_1_f32(Some(&loc), alpha);
        }
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        gl.bind_vertex_array(None);
        gl.use_program(None);
        gl.disable(glow::BLEND);
    }
}

// ---------------------------------------------------------------------------
// Output warp pass (corner pin / fine rotation)
// ---------------------------------------------------------------------------

/// Offscreen target mpv renders into when the warp is active; the warp pass
/// then samples it with the inverse homography.
struct WarpTarget {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    w:   u32,
    h:   u32,
}

fn monitor_dimensions(width: u32, height: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    if width <= 640 && height <= 360 {
        return (width, height);
    }
    let width_scale = 640.0 / width as f64;
    let height_scale = 360.0 / height as f64;
    let scale = width_scale.min(height_scale);
    (
        ((width as f64 * scale).round() as u32).clamp(1, 640),
        ((height as f64 * scale).round() as u32).clamp(1, 360),
    )
}

/// A small PBO ring keeps GPU readback out of the transport path.  We never
/// wait for a fence: when the GPU has not finished an older capture, that
/// program frame is simply dropped and the next compositor frame is tried.
/// This is the same latest-frame policy as `NetworkOutputManager` and avoids
/// turning a slow receiver into render-thread backpressure.
struct NetworkReadbackSlot {
    pbo: glow::Buffer,
    fence: Option<glow::Fence>,
    width: u32,
    height: u32,
}

struct NetworkReadback {
    slots: Vec<NetworkReadbackSlot>,
    next: usize,
}

impl NetworkReadback {
    fn new(gl: &glow::Context) -> Result<Self> {
        let mut slots = Vec::with_capacity(3);
        unsafe {
            for _ in 0..3 {
                slots.push(NetworkReadbackSlot {
                    pbo: gl.create_buffer().map_err(|error| anyhow!("network readback PBO: {error}"))?,
                    fence: None,
                    width: 0,
                    height: 0,
                });
            }
        }
        Ok(Self { slots, next: 0 })
    }

    /// Drain every completed PBO without waiting for the GPU. OpenGL rows are
    /// normalized to top-down BGRA before the caller publishes the frame.
    fn drain_completed(&mut self, gl: &glow::Context, mut publish: impl FnMut(BgraFrame)) {
        for slot in &mut self.slots {
            let Some(fence) = slot.fence else { continue };
            let state = unsafe { gl.client_wait_sync(fence, 0, 0) };
            if state != glow::ALREADY_SIGNALED && state != glow::CONDITION_SATISFIED {
                continue;
            }
            let byte_len = match slot.width.checked_mul(slot.height).and_then(|pixels| pixels.checked_mul(4)) {
                Some(length) => length as usize,
                None => {
                    unsafe { gl.delete_sync(fence); }
                    slot.fence = None;
                    continue;
                }
            };
            unsafe {
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(slot.pbo));
                let mapped = gl.map_buffer_range(glow::PIXEL_PACK_BUFFER, 0, byte_len as i32, glow::MAP_READ_BIT);
                if !mapped.is_null() {
                    let source = std::slice::from_raw_parts(mapped, byte_len);
                    let row_len = slot.width as usize * 4;
                    let mut top_down = vec![0_u8; byte_len];
                    for row in 0..slot.height as usize {
                        let source_row = slot.height as usize - 1 - row;
                        top_down[row * row_len..(row + 1) * row_len]
                            .copy_from_slice(&source[source_row * row_len..(source_row + 1) * row_len]);
                    }
                    publish(BgraFrame { width: slot.width, height: slot.height, stride: row_len as u32, data: top_down });
                    gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
                }
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                gl.delete_sync(fence);
            }
            slot.fence = None;
        }
    }

    fn publish_completed(&mut self, gl: &glow::Context, sink: &NetworkFrameSink) {
        self.drain_completed(gl, |frame| { sink.publish(frame); });
    }

    fn publish_completed_to_monitor(&mut self, gl: &glow::Context, sink: &MonitorFrameMailbox) {
        self.drain_completed(gl, |frame| sink.publish(frame));
    }

    /// Queue one final composited framebuffer for asynchronous readback.  No
    /// producer-side wait is used; all busy slots mean the newest frame wins.
    fn queue(&mut self, gl: &glow::Context, source: glow::Framebuffer, width: u32, height: u32) {
        let Some(index) = (0..self.slots.len())
            .map(|offset| (self.next + offset) % self.slots.len())
            .find(|&index| self.slots[index].fence.is_none()) else { return };
        self.next = (index + 1) % self.slots.len();
        let slot = &mut self.slots[index];
        let Some(bytes) = width.checked_mul(height).and_then(|pixels| pixels.checked_mul(4)) else { return };
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(source));
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(slot.pbo));
            gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, bytes as i32, glow::STREAM_READ);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(0, 0, width as i32, height as i32, glow::BGRA, glow::UNSIGNED_BYTE, glow::PixelPackData::BufferOffset(0));
            slot.fence = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0).ok();
            slot.width = width;
            slot.height = height;
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }
}

impl Drop for NetworkReadback {
    fn drop(&mut self) {
        // The owning render thread has the current GL context.  Resource
        // deletion is intentionally left to that context's process teardown,
        // matching the output engine's detached-thread shutdown strategy.
    }
}

/// GPU texture backing one latest-frame external video source.  The NDI worker
/// supplies top-down BGRA pixels; upload reverses rows once so sampling uses
/// the same bottom-left texture convention as libmpv's render targets.
struct ExternalFrameTarget {
    target: WarpTarget,
}

fn upload_external_frame(
    gl: &glow::Context,
    target: &mut Option<ExternalFrameTarget>,
    frame: BgraFrame,
) -> Result<()> {
    frame.validate().map_err(|error| anyhow!("external BGRA frame: {error}"))?;
    let needs_resize = target.as_ref().map(|current| {
        current.target.w != frame.width || current.target.h != frame.height
    }).unwrap_or(true);
    if needs_resize {
        let mut texture_target = None;
        ensure_warp_target(gl, &mut texture_target, frame.width, frame.height)?;
        if let Some(old) = target.take() {
            unsafe {
                gl.delete_framebuffer(old.target.fbo);
                gl.delete_texture(old.target.tex);
            }
        }
        *target = texture_target.map(|target| ExternalFrameTarget { target });
    }
    let Some(target) = target.as_ref() else { return Err(anyhow!("external texture allocation failed")); };
    let row_len = frame.width as usize * 4;
    let mut gl_rows = vec![0_u8; row_len * frame.height as usize];
    for destination_row in 0..frame.height as usize {
        let source_row = frame.height as usize - 1 - destination_row;
        let source_offset = source_row * frame.stride as usize;
        gl_rows[destination_row * row_len..(destination_row + 1) * row_len]
            .copy_from_slice(&frame.data[source_offset..source_offset + row_len]);
    }
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(target.target.tex));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D, 0, 0, 0, frame.width as i32, frame.height as i32,
            glow::BGRA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(&gl_rows),
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
    Ok(())
}

/// Create (or resize) the warp FBO to the current window size.
fn ensure_warp_target(
    gl: &glow::Context,
    slot: &mut Option<WarpTarget>,
    w: u32,
    h: u32,
) -> Result<()> {
    if let Some(t) = slot {
        if t.w == w && t.h == h {
            return Ok(());
        }
    }
    unsafe {
        if let Some(old) = slot.take() {
            gl.delete_framebuffer(old.fbo);
            gl.delete_texture(old.tex);
        }
        let tex = gl.create_texture().map_err(|e| anyhow!("warp tex: {e}"))?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D, 0, glow::RGBA8 as i32,
            w as i32, h as i32, 0,
            glow::RGBA, glow::UNSIGNED_BYTE, None,
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);

        let fbo = gl.create_framebuffer().map_err(|e| anyhow!("warp fbo: {e}"))?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0,
        );
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        if status != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(fbo);
            gl.delete_texture(tex);
            return Err(anyhow!("warp FBO incomplete: 0x{status:x}"));
        }
        *slot = Some(WarpTarget { fbo, tex, w, h });
        log::info!("[render] warp target (re)created: {w}x{h}");
    }
    Ok(())
}

/// Fullscreen inverse-homography pass: for every window pixel, sample where in
/// the mpv frame it comes from; pixels outside the destination quad are black.
fn build_warp_shader(gl: &glow::Context) -> Result<(glow::Program, glow::VertexArray)> {
    const VERT: &str = r#"
#version 150 core
const vec2 POS[3] = vec2[3](vec2(-1,-1), vec2(3,-1), vec2(-1,3));
void main() { gl_Position = vec4(POS[gl_VertexID], 0.0, 1.0); }
"#;
    // All warp math is in y-down normalized window space ([0,1]², origin at the
    // top-left — matching the editor UI).  gl_FragCoord is y-up, so flip once
    // on input; the mpv texture is rendered with FLIP_Y (y-up), so flip once
    // more on sampling.
    const FRAG: &str = r#"
#version 150 core
uniform sampler2D u_tex;
uniform mat3  u_hinv;
uniform vec2  u_size;
out vec4 color;
void main() {
    vec2 win = vec2(gl_FragCoord.x / u_size.x, 1.0 - gl_FragCoord.y / u_size.y);
    vec3 t = u_hinv * vec3(win, 1.0);
    if (t.z == 0.0) { color = vec4(0.0, 0.0, 0.0, 1.0); return; }
    vec2 uv = t.xy / t.z;
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        color = vec4(0.0, 0.0, 0.0, 1.0);
    } else {
        color = texture(u_tex, vec2(uv.x, 1.0 - uv.y));
    }
}
"#;
    unsafe {
        let vs = gl.create_shader(glow::VERTEX_SHADER).map_err(|e| anyhow!("{e}"))?;
        gl.shader_source(vs, VERT);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) { return Err(anyhow!("warp vert: {}", gl.get_shader_info_log(vs))); }

        let fs = gl.create_shader(glow::FRAGMENT_SHADER).map_err(|e| anyhow!("{e}"))?;
        gl.shader_source(fs, FRAG);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) { return Err(anyhow!("warp frag: {}", gl.get_shader_info_log(fs))); }

        let prog = gl.create_program().map_err(|e| anyhow!("{e}"))?;
        gl.attach_shader(prog, vs); gl.attach_shader(prog, fs);
        gl.link_program(prog);
        if !gl.get_program_link_status(prog) { return Err(anyhow!("warp link: {}", gl.get_program_info_log(prog))); }
        gl.detach_shader(prog, vs); gl.delete_shader(vs);
        gl.detach_shader(prog, fs); gl.delete_shader(fs);

        let vao = gl.create_vertex_array().map_err(|e| anyhow!("{e}"))?;
        log::info!("[render] warp shader compiled");
        Ok((prog, vao))
    }
}

// ---------------------------------------------------------------------------
// Layer compositor (blend stack) + plain blit
// ---------------------------------------------------------------------------

/// One blend step: `result = blend(backdrop, layer, mode, opacity)`.
///
/// The per-channel math is [`super::blend::GLSL_BLEND_FN`], whose executable
/// spec is the Rust `blend_channel` in `blend.rs` — keep them identical.
fn build_composite_shader(gl: &glow::Context) -> Result<(glow::Program, glow::VertexArray)> {
    const VERT: &str = r#"
#version 150 core
const vec2 POS[3] = vec2[3](vec2(-1,-1), vec2(3,-1), vec2(-1,3));
out vec2 v_uv;
void main() {
    gl_Position = vec4(POS[gl_VertexID], 0.0, 1.0);
    v_uv = POS[gl_VertexID] * 0.5 + 0.5;
}
"#;
    let frag = format!(
        r#"
#version 150 core
uniform sampler2D u_backdrop;
uniform sampler2D u_layer;
uniform int   u_blend_mode;
uniform float u_opacity;
in vec2 v_uv;
out vec4 color;
{}
void main() {{
    vec4 b = texture(u_backdrop, v_uv);
    vec4 s = texture(u_layer, v_uv);
    float sa = clamp(s.a * u_opacity, 0.0, 1.0);
    float ao = sa + b.a * (1.0 - sa);
    vec3 rgb = vec3(0.0);
    for (int c = 0; c < 3; c++) {{
        float blended = (1.0 - b.a) * s[c] + b.a * blend_channel(u_blend_mode, b[c], s[c]);
        rgb[c] = sa * blended + (1.0 - sa) * b.a * b[c];
    }}
    if (ao > 0.0) rgb /= ao;
    color = vec4(rgb, ao);
}}
"#,
        super::blend::GLSL_BLEND_FN,
    );
    build_program(gl, VERT, &frag, "composite")
}

/// Receiver frames need an output-sized transparent layer before blending so
/// Camera Cue geometry has the same meaning as mpv-backed visual cues.
fn build_geometry_shader(gl: &glow::Context) -> Result<(glow::Program, glow::VertexArray)> {
    const VERT: &str = r#"#version 150 core
const vec2 P[3]=vec2[3](vec2(-1,-1),vec2(3,-1),vec2(-1,3));
void main(){gl_Position=vec4(P[gl_VertexID],0,1);}"#;
    const FRAG: &str = r#"#version 150 core
uniform sampler2D u_tex; uniform vec2 u_source_size,u_output_size,u_pan; uniform float u_scale,u_rotation; uniform int u_fit_mode; uniform vec4 u_crop; out vec4 color;
void main(){vec2 p=vec2(gl_FragCoord.x/u_output_size.x,1.0-gl_FragCoord.y/u_output_size.y);vec2 d=p-vec2(.5)-u_pan;float c=cos(u_rotation),s=sin(u_rotation);vec2 q=vec2(c*d.x+s*d.y,-s*d.x+c*d.y)/max(u_scale,.01)+vec2(.5);float sa=u_source_size.x/max(u_source_size.y,1.),oa=u_output_size.x/max(u_output_size.y,1.);vec2 z=vec2(1.);if(u_fit_mode==0){if(sa>oa)z.y=oa/sa;else z.x=sa/oa;}else if(u_fit_mode==1){if(sa>oa)z.x=sa/oa;else z.y=oa/sa;}vec2 uv=(q-(vec2(1.)-z)*.5)/z;if(uv.x<0.||uv.x>1.||uv.y<0.||uv.y>1.){color=vec4(0.);return;}uv=vec2(mix(u_crop.x,1.-u_crop.y,uv.x),mix(u_crop.z,1.-u_crop.w,uv.y));color=texture(u_tex,vec2(uv.x,1.-uv.y));}"#;
    build_program(gl, VERT, FRAG, "external geometry")
}

#[allow(clippy::too_many_arguments)]
fn draw_geometry_pass(gl: &glow::Context, program: glow::Program, vao: glow::VertexArray, texture: glow::Texture, geometry: VideoGeometry, source_w: u32, source_h: u32, output_w: u32, output_h: u32, destination: glow::Framebuffer) {
    let fit = match geometry.fit_mode { super::types::FitMode::Fit => 0, super::types::FitMode::Fill => 1, super::types::FitMode::Stretch => 2 };
    unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, Some(destination)); gl.viewport(0, 0, output_w as i32, output_h as i32); gl.disable(glow::BLEND); gl.use_program(Some(program)); gl.active_texture(glow::TEXTURE0); gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        if let Some(l)=gl.get_uniform_location(program,"u_tex"){gl.uniform_1_i32(Some(&l),0);} if let Some(l)=gl.get_uniform_location(program,"u_source_size"){gl.uniform_2_f32(Some(&l),source_w as f32,source_h as f32);} if let Some(l)=gl.get_uniform_location(program,"u_output_size"){gl.uniform_2_f32(Some(&l),output_w as f32,output_h as f32);} if let Some(l)=gl.get_uniform_location(program,"u_pan"){gl.uniform_2_f32(Some(&l),geometry.pan_x as f32,geometry.pan_y as f32);} if let Some(l)=gl.get_uniform_location(program,"u_scale"){gl.uniform_1_f32(Some(&l),geometry.scale.max(0.01) as f32);} if let Some(l)=gl.get_uniform_location(program,"u_rotation"){gl.uniform_1_f32(Some(&l),geometry.rotation as f32*std::f32::consts::PI/180.0);} if let Some(l)=gl.get_uniform_location(program,"u_fit_mode"){gl.uniform_1_i32(Some(&l),fit);} if let Some(l)=gl.get_uniform_location(program,"u_crop"){gl.uniform_4_f32(Some(&l),geometry.crop_left.clamp(0.0,0.45) as f32,geometry.crop_right.clamp(0.0,0.45) as f32,geometry.crop_top.clamp(0.0,0.45) as f32,geometry.crop_bottom.clamp(0.0,0.45) as f32);} gl.bind_vertex_array(Some(vao)); gl.draw_arrays(glow::TRIANGLES,0,3); gl.bind_vertex_array(None); gl.bind_texture(glow::TEXTURE_2D,None); gl.use_program(None); }
}

/// Plain textured fullscreen blit (composite → window when no warp).
fn build_blit_shader(gl: &glow::Context) -> Result<(glow::Program, glow::VertexArray)> {
    const VERT: &str = r#"
#version 150 core
const vec2 POS[3] = vec2[3](vec2(-1,-1), vec2(3,-1), vec2(-1,3));
out vec2 v_uv;
void main() {
    gl_Position = vec4(POS[gl_VertexID], 0.0, 1.0);
    v_uv = POS[gl_VertexID] * 0.5 + 0.5;
}
"#;
    const FRAG: &str = r#"
#version 150 core
uniform sampler2D u_tex;
in vec2 v_uv;
out vec4 color;
void main() { color = vec4(texture(u_tex, v_uv).rgb, 1.0); }
"#;
    build_program(gl, VERT, FRAG, "blit")
}

/// Compile + link a program and create its (empty) VAO.
fn build_program(
    gl: &glow::Context,
    vert: &str,
    frag: &str,
    name: &str,
) -> Result<(glow::Program, glow::VertexArray)> {
    unsafe {
        let vs = gl.create_shader(glow::VERTEX_SHADER).map_err(|e| anyhow!("{e}"))?;
        gl.shader_source(vs, vert);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) {
            return Err(anyhow!("{name} vert: {}", gl.get_shader_info_log(vs)));
        }

        let fs = gl.create_shader(glow::FRAGMENT_SHADER).map_err(|e| anyhow!("{e}"))?;
        gl.shader_source(fs, frag);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) {
            return Err(anyhow!("{name} frag: {}", gl.get_shader_info_log(fs)));
        }

        let prog = gl.create_program().map_err(|e| anyhow!("{e}"))?;
        gl.attach_shader(prog, vs);
        gl.attach_shader(prog, fs);
        gl.link_program(prog);
        if !gl.get_program_link_status(prog) {
            return Err(anyhow!("{name} link: {}", gl.get_program_info_log(prog)));
        }
        gl.detach_shader(prog, vs);
        gl.delete_shader(vs);
        gl.detach_shader(prog, fs);
        gl.delete_shader(fs);

        let vao = gl.create_vertex_array().map_err(|e| anyhow!("{e}"))?;
        log::info!("[render] {name} shader compiled");
        Ok((prog, vao))
    }
}

/// One blend step of the layer stack into `dst_fbo`.
#[allow(clippy::too_many_arguments)]
fn draw_composite_pass(
    gl: &glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
    backdrop_tex: glow::Texture,
    layer_tex: glow::Texture,
    blend_mode: i32,
    opacity: f32,
    w: u32,
    h: u32,
    dst_fbo: glow::Framebuffer,
) {
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(dst_fbo));
        gl.viewport(0, 0, w as i32, h as i32);
        gl.disable(glow::BLEND);
        gl.use_program(Some(program));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(backdrop_tex));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(layer_tex));
        if let Some(loc) = gl.get_uniform_location(program, "u_backdrop") {
            gl.uniform_1_i32(Some(&loc), 0);
        }
        if let Some(loc) = gl.get_uniform_location(program, "u_layer") {
            gl.uniform_1_i32(Some(&loc), 1);
        }
        if let Some(loc) = gl.get_uniform_location(program, "u_blend_mode") {
            gl.uniform_1_i32(Some(&loc), blend_mode);
        }
        if let Some(loc) = gl.get_uniform_location(program, "u_opacity") {
            gl.uniform_1_f32(Some(&loc), opacity);
        }
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        gl.bind_vertex_array(None);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
    }
}

/// Blit the final composite to a caller-selected framebuffer (`None` is the
/// native output window).  Network capture uses an FBO so the hidden native
/// context never needs a visible surface commit.
fn draw_blit_pass_to(
    gl: &glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
    tex: glow::Texture,
    w: u32,
    h: u32,
    destination: Option<glow::Framebuffer>,
) {
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, destination);
        gl.viewport(0, 0, w as i32, h as i32);
        gl.disable(glow::BLEND);
        gl.use_program(Some(program));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        if let Some(loc) = gl.get_uniform_location(program, "u_tex") {
            gl.uniform_1_i32(Some(&loc), 0);
        }
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        gl.bind_vertex_array(None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
    }
}

/// Draw the warp pass into a caller-selected framebuffer (`None` is the
/// native output window).
fn draw_warp_pass_to(
    gl: &glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
    tex: glow::Texture,
    hinv: &[f32; 9],
    w: u32,
    h: u32,
    destination: Option<glow::Framebuffer>,
) {
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, destination);
        gl.viewport(0, 0, w as i32, h as i32);
        gl.disable(glow::BLEND);
        gl.use_program(Some(program));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        if let Some(loc) = gl.get_uniform_location(program, "u_tex") {
            gl.uniform_1_i32(Some(&loc), 0);
        }
        if let Some(loc) = gl.get_uniform_location(program, "u_hinv") {
            // Our matrix is row-major; transpose=true converts for GLSL.
            gl.uniform_matrix_3_f32_slice(Some(&loc), true, hinv);
        }
        if let Some(loc) = gl.get_uniform_location(program, "u_size") {
            gl.uniform_2_f32(Some(&loc), w as f32, h as f32);
        }
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        gl.bind_vertex_array(None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{monitor_dimensions, RenderRuntime, wait_for_render_wake_or_shutdown};
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use uuid::Uuid;
    use crate::engine::network_io::{BgraFrame, BgraFrameMailbox};
    use crate::engine::output_engine::{LayerStyle, VideoGeometry};

    #[cfg(not(target_os = "macos"))]
    use super::{clamp_floating_geometry, dom_key, FloatingRect};
    #[cfg(not(target_os = "macos"))]
    use winit::keyboard::{Key, NamedKey};
    #[cfg(not(target_os = "macos"))]
    use crate::preferences::FloatingWindowGeometry;

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn dom_key_space_uses_dom_spelling() {
        assert_eq!(dom_key(&Key::Named(NamedKey::Space)).as_deref(), Some(" "));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn dom_key_named_keys_match_dom_values() {
        for (key, dom) in [(NamedKey::Escape, "Escape"), (NamedKey::ArrowUp, "ArrowUp"), (NamedKey::ArrowDown, "ArrowDown"), (NamedKey::Delete, "Delete"), (NamedKey::Backspace, "Backspace"), (NamedKey::F5, "F5"), (NamedKey::F9, "F9")] {
            assert_eq!(dom_key(&Key::Named(key)).as_deref(), Some(dom));
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn dom_key_characters_pass_through() {
        assert_eq!(dom_key(&Key::Character("s".into())).as_deref(), Some("s"));
        assert_eq!(dom_key(&Key::Character("S".into())).as_deref(), Some("S"));
        assert_eq!(dom_key(&Key::Character("[".into())).as_deref(), Some("["));
        assert_eq!(dom_key(&Key::Character(",".into())).as_deref(), Some(","));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn dom_key_dead_keys_are_dropped() { assert_eq!(dom_key(&Key::Dead(None)), None); }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn floating_rect_restores_on_the_monitor_with_the_largest_overlap() {
        let saved = FloatingWindowGeometry { x: 2100, y: 100, width: 900, height: 600, maximized: false };
        let monitors = [
            FloatingRect { x: 0, y: 0, width: 1920, height: 1080 },
            FloatingRect { x: 1920, y: 0, width: 1920, height: 1080 },
        ];
        assert_eq!(
            clamp_floating_geometry(saved.clone(), &monitors),
            saved,
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn floating_rect_moves_onto_a_remaining_monitor_after_topology_change() {
        let saved = FloatingWindowGeometry { x: 2600, y: 700, width: 1000, height: 700, maximized: true };
        let monitors = [FloatingRect { x: 0, y: 0, width: 1920, height: 1080 }];
        assert_eq!(
            clamp_floating_geometry(saved, &monitors),
            FloatingWindowGeometry { x: 920, y: 380, width: 1000, height: 700, maximized: true },
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn floating_rect_is_never_larger_than_the_available_display() {
        let saved = FloatingWindowGeometry { x: -100, y: -100, width: 3000, height: 2000, maximized: false };
        let monitors = [FloatingRect { x: 0, y: 0, width: 800, height: 600 }];
        assert_eq!(
            clamp_floating_geometry(saved, &monitors),
            FloatingWindowGeometry { x: 0, y: 0, width: 800, height: 600, maximized: false },
        );
    }

    #[test]
    fn runtime_state_is_isolated() {
        let a = RenderRuntime::new(); let b = RenderRuntime::new();
        a.show(); a.set_size(640, 360); a.set_warp(Some([1.0; 9]));
        assert!(a.shared.visible.load(Ordering::Relaxed));
        assert!(!b.shared.visible.load(Ordering::Relaxed));
        assert_eq!(a.shared.width.load(Ordering::Relaxed), 640);
        assert_eq!(b.shared.width.load(Ordering::Relaxed), 1920);
        assert_eq!(*a.shared.warp.lock().unwrap(), Some([1.0; 9]));
        assert_eq!(*b.shared.warp.lock().unwrap(), None);
        assert!(*a.shared.signal.0.lock().unwrap());
        assert!(!*b.shared.signal.0.lock().unwrap());
    }

    #[test]
    fn visibility_callback_tracks_native_notifications() {
        let runtime = RenderRuntime::new();
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_observed = Arc::clone(&observed);
        runtime.set_visibility_callback(Arc::new(move |visible| {
            callback_observed.store(visible, Ordering::Release);
        }));

        runtime.notify_visibility(true);
        assert!(observed.load(Ordering::Acquire));
        runtime.notify_visibility(false);
        assert!(!observed.load(Ordering::Acquire));
    }

    #[test]
    fn window_preferences_update_before_or_after_native_window_creation() {
        let runtime = RenderRuntime::new();
        assert_eq!(runtime.window_preferences(), (false, false));

        runtime.configure_output_window("screen-2", false, None, true, true);
        assert_eq!(runtime.window_preferences(), (true, true));

        runtime.configure_output_window("screen-2", false, None, false, false);
        assert_eq!(runtime.window_preferences(), (false, false));
    }

    #[test]
    fn shutdown_wakes_a_blocked_render_waiter_and_stays_set() {
        let runtime = RenderRuntime::new();
        let shared = Arc::clone(&runtime.shared);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            done_tx
                .send(wait_for_render_wake_or_shutdown(&shared, Duration::from_secs(5)))
                .unwrap();
        });

        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        runtime.request_shutdown();
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        waiter.join().unwrap();
        assert!(runtime.is_shutting_down());
    }

    #[test]
    fn external_source_keeps_geometry_style_and_layer_order() {
        let runtime = RenderRuntime::new(); let a = Uuid::new_v4(); let b = Uuid::new_v4();
        let geometry = VideoGeometry { pan_x: 0.2, ..Default::default() };
        let style = LayerStyle { layer: Some(2), opacity: 0.4, blend_mode: crate::engine::output_engine::BlendMode::Screen };
        runtime.add_external_source(a, Arc::new(BgraFrameMailbox::default()), geometry, style, crate::engine::output_engine::slot::resolve_layer_key(Some(2), 2), 0);
        runtime.add_external_source(b, Arc::new(BgraFrameMailbox::default()), VideoGeometry::default(), LayerStyle::default(), crate::engine::output_engine::slot::resolve_layer_key(None, 3), 0);
        let (draws, _, _) = runtime.tick_external_sources();
        assert_eq!(draws.iter().map(|draw| draw.voice).collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(draws[0].geometry, geometry); assert_eq!(draws[0].opacity, 0.4);
        assert_eq!(draws[0].blend_mode, crate::engine::output_engine::BlendMode::Screen.shader_id());
    }

    #[test]
    fn external_hard_stop_forces_cleanup_redraw() {
        let runtime = RenderRuntime::new(); let voice = Uuid::new_v4();
        runtime.add_external_source(voice, Arc::new(BgraFrameMailbox::default()), VideoGeometry::default(), LayerStyle::default(), 1, 0);
        let _ = runtime.tick_external_sources(); assert!(runtime.stop_external_source(voice, 0));
        let (draws, _, dirty) = runtime.tick_external_sources(); assert!(draws.is_empty()); assert!(dirty);
    }

    #[test]
    fn external_stop_retains_frame_for_fade() {
        let runtime = RenderRuntime::new(); let voice = Uuid::new_v4();
        runtime.add_external_source(voice, Arc::new(BgraFrameMailbox::default()), VideoGeometry::default(), LayerStyle::default(), 1, 0);
        assert!(runtime.stop_external_source(voice, 250)); let (draws, animating, _) = runtime.tick_external_sources();
        assert_eq!(draws.len(), 1); assert!(animating);
    }

    #[test]
    fn monitor_preview_preserves_aspect_ratio_inside_bound() {
        assert_eq!(monitor_dimensions(1920, 1080), (640, 360));
        assert_eq!(monitor_dimensions(1080, 1920), (203, 360));
        assert_eq!(monitor_dimensions(320, 180), (320, 180));
        assert_eq!(monitor_dimensions(0, 0), (1, 1));
    }

    #[test]
    fn monitor_activation_clears_stale_frame_and_is_idempotent() {
        let runtime = RenderRuntime::new();
        runtime.monitor_frames.publish(BgraFrame {
            width: 1,
            height: 1,
            stride: 4,
            data: vec![1, 2, 3, 255],
        });
        runtime.set_monitor_capture(true);
        assert!(runtime.monitor_frame_after(0).is_none());
        runtime.monitor_frames.publish(BgraFrame {
            width: 1,
            height: 1,
            stride: 4,
            data: vec![1, 2, 3, 255],
        });
        runtime.set_monitor_capture(true);
        assert!(runtime.monitor_frame_after(0).is_some());
        runtime.set_monitor_capture(false);
        assert!(runtime.monitor_frame_after(0).is_none());
    }
}
