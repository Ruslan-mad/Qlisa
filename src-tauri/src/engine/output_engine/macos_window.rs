//! macOS native output window for the unified GL path (`render.rs`).
//!
//! winit cannot be used here: its `EventLoop` must own the AppKit main thread,
//! which Tauri's `NSApplication` already runs.  So we create and drive a plain
//! borderless `NSWindow` directly through the Objective-C runtime (`objc2`),
//! hand its `contentView` (an `NSView`) to `glutin` as the CGL drawable, and let
//! the shared render thread in `render.rs` do everything else exactly as it does
//! on Windows/Linux.
//!
//! ## Threading
//!
//! Every AppKit call must run on the main thread.  `create()` is invoked from
//! `OutputEngine::new()` inside Tauri's `.setup()`, which *is* the main thread,
//! so the window is built inline there.  The runtime control helpers
//! (`show`/`hide`/`position_on_screen`/`toggle_fullscreen`) are called later from
//! Tauri command / event-loop worker threads, so they marshal onto the main
//! thread via `AppHandle::run_on_main_thread`.
//!
//! Cocoa selectors are rock-stable, so we drive AppKit via raw `msg_send!` rather
//! than `objc2-app-kit`'s version-churny typed bindings.  AppKit is linked by
//! `build.rs` (`cargo::rustc-link-lib=framework=AppKit`).

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use super::render::RenderRuntime;

use anyhow::{anyhow, Result};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle,
};

// AppKit constants (stable ABI values from <AppKit/AppKit.h>).
/// `NSWindowStyleMaskResizable` (1 << 3) — resizable window without a title bar.
/// Using this alone keeps the window borderless (no auto-show on app activation)
/// while giving the OS-managed resize grips at the edges.
const NS_WINDOW_STYLE_MASK_RESIZABLE: usize = 1 << 3;
const NS_BACKING_STORE_BUFFERED: usize = 2;
/// Normal window level (0) — output sits alongside other windows and can go behind them.
const NS_NORMAL_WINDOW_LEVEL: isize = 0;
/// `NSFloatingWindowLevel` — above ordinary app windows, below system UI.
const NS_FLOATING_WINDOW_LEVEL: isize = 3;
/// Level used when the output window is fullscreen.  Must be above the menu-bar level
/// (24) so the window truly covers the whole screen including the status bar.
const NS_FULLSCREEN_WINDOW_LEVEL: isize = 25;
/// `NSWindowCollectionBehaviorCanJoinAllSpaces` (1 << 0).
const NS_COLLECTION_CAN_JOIN_ALL_SPACES: usize = 1 << 0;
/// `NSWindowCollectionBehaviorFullScreenAuxiliary` (1 << 8) — lets the borderless
/// output coexist over another app's native-fullscreen space.
const NS_COLLECTION_FULLSCREEN_AUXILIARY: usize = 1 << 8;

const INITIAL_WIDTH: f64 = 960.0;
const INITIAL_HEIGHT: f64 = 540.0;

/// Per-output native state. The raw NSWindow is intentionally retained for the
/// app lifetime (`setReleasedWhenClosed: false`); the Arc itself is owned by its
/// RenderRuntime, so multiple output engines never share native state.
pub(super) struct MacOutputWindow {
    pub(super) raw_window: AtomicUsize,
    pub(super) fullscreen: AtomicBool,
    pub(super) saved_frame: Mutex<Option<(f64, f64, f64, f64)>>,
    pub(super) app_handle: tauri::AppHandle,
    pub(super) runtime: Weak<RenderRuntime>,
}

// ---------------------------------------------------------------------------
// Public API (called from render.rs / OutputEngine)
// ---------------------------------------------------------------------------

/// Create the borderless output `NSWindow` and return the raw handles + initial
/// size (physical pixels) for the render thread's `glutin` surface.
pub(super) fn create(
    app_handle: &tauri::AppHandle,
    runtime: Arc<RenderRuntime>,
) -> Result<(RawWindowHandle, RawDisplayHandle, u32, u32)> {
    // Build on the main thread.  In normal startup we already are it (`.setup()`),
    // so build inline; otherwise dispatch and wait.
    let (view_ptr, window_ptr, width, height) = if MainThreadMarker::new().is_some() {
        build_window()
    } else {
        let (tx, rx) = std::sync::mpsc::channel::<(usize, usize, u32, u32)>();
        app_handle
            .run_on_main_thread(move || {
                let _ = tx.send(build_window());
            })
            .map_err(|e| anyhow!("run_on_main_thread (window create): {e}"))?;
        rx.recv()
            .map_err(|_| anyhow!("main-thread NSWindow creation did not complete"))?
    };

    let state = Arc::new(MacOutputWindow {
        raw_window: AtomicUsize::new(window_ptr),
        fullscreen: AtomicBool::new(false),
        saved_frame: Mutex::new(None),
        app_handle: app_handle.clone(),
        runtime: Arc::downgrade(&runtime),
    });
    if MainThreadMarker::new().is_some() {
        register_resize_observer(window_ptr, &state);
    } else {
        let (tx, rx) = std::sync::mpsc::channel();
        let observer_state = Arc::clone(&state);
        app_handle.run_on_main_thread(move || {
            register_resize_observer(window_ptr as *mut AnyObject, &observer_state);
            let _ = tx.send(());
        }).map_err(|e| anyhow!("run_on_main_thread (resize observer): {e}"))?;
        rx.recv().map_err(|_| anyhow!("resize observer registration did not complete"))?;
    }
    if let Ok(mut slot) = runtime.mac_window.lock() { *slot = Some(state); }
    let (always_on_top, hide_cursor) = runtime.window_preferences();
    apply_preferences(&runtime, always_on_top, hide_cursor);

    let ns_view = NonNull::new(view_ptr as *mut c_void)
        .ok_or_else(|| anyhow!("NSWindow contentView was nil"))?;
    let window_handle = AppKitWindowHandle::new(ns_view);
    let rwh = RawWindowHandle::AppKit(window_handle);
    let rdh = RawDisplayHandle::AppKit(AppKitDisplayHandle::new());
    Ok((rwh, rdh, width, height))
}

/// Order the output window to the front (show).
pub(super) fn show(runtime: &RenderRuntime) {
    on_main(runtime, |window, _state| unsafe {
        let _: () = msg_send![window, orderFrontRegardless];
    });
}

/// Order the output window out (hide).
pub(super) fn hide(runtime: &RenderRuntime) {
    on_main(runtime, |window, _state| unsafe {
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![window, orderOut: nil];
    });
}

/// Apply per-destination window policy without rebuilding the GL surface.
/// A transparent cursor rect affects only the output view, unlike
/// `NSCursor::hide`, which would hide the pointer across the whole desktop.
pub(super) fn apply_preferences(
    runtime: &RenderRuntime,
    always_on_top: bool,
    hide_cursor: bool,
) {
    on_main(runtime, move |window, state| unsafe {
        let level = if state.fullscreen.load(Ordering::SeqCst) {
            NS_FULLSCREEN_WINDOW_LEVEL
        } else if always_on_top {
            NS_FLOATING_WINDOW_LEVEL
        } else {
            NS_NORMAL_WINDOW_LEVEL
        };
        let _: () = msg_send![window, setLevel: level];

        let view: *mut AnyObject = msg_send![window, contentView];
        if view.is_null() {
            return;
        }
        let _: () = msg_send![view, discardCursorRects];
        if hide_cursor {
            let image_alloc: Allocated<AnyObject> = msg_send_id![class!(NSImage), alloc];
            let image: Retained<AnyObject> =
                msg_send_id![image_alloc, initWithSize: NSSize::new(1.0, 1.0)];
            let cursor_alloc: Allocated<AnyObject> = msg_send_id![class!(NSCursor), alloc];
            let cursor: Retained<AnyObject> = msg_send_id![
                cursor_alloc,
                initWithImage: &*image,
                hotSpot: NSPoint::new(0.0, 0.0)
            ];
            let bounds: NSRect = msg_send![view, bounds];
            let _: () = msg_send![view, addCursorRect: bounds, cursor: &*cursor];
        }
    });
}

/// Place the window fullscreen onto `NSScreen[screen_index]` (clamped).
pub(super) fn position_on_screen(runtime: &RenderRuntime, screen_index: u32) {
    on_main(runtime, move |window, state| unsafe {
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        if screens.is_null() {
            return;
        }
        let count: usize = msg_send![screens, count];
        if count == 0 {
            return;
        }
        let idx = (screen_index as usize).min(count - 1);
        let screen: *mut AnyObject = msg_send![screens, objectAtIndex: idx];
        if screen.is_null() {
            return;
        }
        let frame: NSRect = msg_send![screen, frame];
        let _: () = msg_send![window, setMovableByWindowBackground: false];
        let _: () = msg_send![window, setStyleMask: 0usize];
        // Raise above the menu bar so the window truly covers the full screen.
        let _: () = msg_send![window, setLevel: NS_FULLSCREEN_WINDOW_LEVEL];
        let _: () = msg_send![window, setFrame: frame, display: true];
        state.fullscreen.store(true, Ordering::SeqCst);
        // Use physical pixels so the GL surface covers the full screen on Retina.
        let view: *mut AnyObject = msg_send![window, contentView];
        let phys: NSSize = msg_send![view, convertSizeToBacking: frame.size];
        if let Some(rt) = state.runtime.upgrade() { super::render::set_surface_size(&rt, phys.width as u32, phys.height as u32); }
    });
}

/// Restore the saved windowed frame if the window is currently fullscreen
/// (no-op otherwise).  Used when the operator selects "Floating window".
pub(super) fn set_windowed(runtime: &RenderRuntime) {
    if runtime.mac_window.lock().ok().and_then(|g| g.as_ref().map(|s| s.fullscreen.load(Ordering::SeqCst))).unwrap_or(false) {
        toggle_fullscreen(runtime);
    }
}

/// Toggle the window between its saved windowed frame and fullscreen on its
/// current screen — the macOS counterpart of winit's `Fullscreen::Borderless`.
pub(super) fn toggle_fullscreen(runtime: &RenderRuntime) {
    on_main(runtime, |window, state| unsafe {
        if state.fullscreen.load(Ordering::SeqCst) {
            // Fallback if no saved frame (e.g. window was shown via position_on_screen
            // without ever being in windowed mode first).
            let (x, y, w, h) = state.saved_frame
                .lock()
                .unwrap()
                .unwrap_or((100.0, 100.0, 960.0, 540.0));
            let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
            let _: () = msg_send![window, setStyleMask: NS_WINDOW_STYLE_MASK_RESIZABLE];
            let _: () = msg_send![window, setMovableByWindowBackground: true];
            // Restore the configured windowed stacking level before resizing.
            let windowed_level = state
                .runtime
                .upgrade()
                .map(|runtime| {
                    if runtime.window_preferences().0 {
                        NS_FLOATING_WINDOW_LEVEL
                    } else {
                        NS_NORMAL_WINDOW_LEVEL
                    }
                })
                .unwrap_or(NS_NORMAL_WINDOW_LEVEL);
            let _: () = msg_send![window, setLevel: windowed_level];
            let _: () = msg_send![window, setFrame: rect, display: true];
            // Physical pixels for the GL surface.
            let view: *mut AnyObject = msg_send![window, contentView];
            let phys: NSSize = msg_send![view, convertSizeToBacking: NSSize::new(w, h)];
            if let Some(rt) = state.runtime.upgrade() { super::render::set_surface_size(&rt, phys.width as u32, phys.height as u32); }
            state.fullscreen.store(false, Ordering::SeqCst);
        } else {
            let cur: NSRect = msg_send![window, frame];
            *state.saved_frame.lock().unwrap() =
                Some((cur.origin.x, cur.origin.y, cur.size.width, cur.size.height));
            let mut screen: *mut AnyObject = msg_send![window, screen];
            if screen.is_null() {
                screen = msg_send![class!(NSScreen), mainScreen];
            }
            if !screen.is_null() {
                let frame: NSRect = msg_send![screen, frame];
                let _: () = msg_send![window, setMovableByWindowBackground: false];
                let _: () = msg_send![window, setStyleMask: 0usize];
                // Raise above the menu bar for true fullscreen coverage.
                let _: () = msg_send![window, setLevel: NS_FULLSCREEN_WINDOW_LEVEL];
                let _: () = msg_send![window, setFrame: frame, display: true];
                // Physical pixels for the GL surface.
                let view: *mut AnyObject = msg_send![window, contentView];
                let phys: NSSize = msg_send![view, convertSizeToBacking: frame.size];
                if let Some(rt) = state.runtime.upgrade() { super::render::set_surface_size(&rt, phys.width as u32, phys.height as u32); }
            }
            state.fullscreen.store(true, Ordering::SeqCst);
        }
    });
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Build the NSWindow on the current (main) thread; store it and return the
/// `contentView` pointer + initial size in **physical pixels**.
fn build_window() -> (usize, usize, u32, u32) {
    unsafe {
        // Center on the main screen (the one with the menu bar).
        let (win_x, win_y) = {
            let ms: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
            if ms.is_null() {
                (100.0_f64, 100.0_f64)
            } else {
                let sf: NSRect = msg_send![ms, frame];
                (
                    sf.origin.x + (sf.size.width - INITIAL_WIDTH) / 2.0,
                    sf.origin.y + (sf.size.height - INITIAL_HEIGHT) / 2.0,
                )
            }
        };
        let rect = NSRect::new(
            NSPoint::new(win_x, win_y),
            NSSize::new(INITIAL_WIDTH, INITIAL_HEIGHT),
        );
        // alloc/init are memory-management-family selectors: objc2 requires
        // `msg_send_id!` (not `msg_send!`) so the +1 retain is tracked.
        let alloc: Allocated<AnyObject> = msg_send_id![class!(NSWindow), alloc];
        // Borderless-resizable: NSWindowStyleMaskResizable alone (= 8) keeps the window
        // frameless so AppKit never auto-shows it on app activation (NSWindowStyleMaskTitled
        // triggers that), while still providing OS-managed resize grips at the edges.
        let window: Retained<AnyObject> = msg_send_id![
            alloc,
            initWithContentRect: rect,
            styleMask: NS_WINDOW_STYLE_MASK_RESIZABLE,
            backing: NS_BACKING_STORE_BUFFERED,
            defer: false
        ];
        // Raw pointer to the (heap-stable) NSWindow; the `forget` below leaks the
        // retain so the window outlives this Retained and lives for the whole app.
        let window_ptr: *mut AnyObject = (&*window as *const AnyObject) as *mut AnyObject;

        // Keep alive forever; closing must not deallocate it.
        let _: () = msg_send![window_ptr, setReleasedWhenClosed: false];
        // Drag the borderless window by its background.
        let _: () = msg_send![window_ptr, setMovableByWindowBackground: true];
        // Normal level in windowed mode; raised above the menu bar when fullscreen.
        let _: () = msg_send![window_ptr, setLevel: NS_NORMAL_WINDOW_LEVEL];
        let behavior: usize =
            NS_COLLECTION_CAN_JOIN_ALL_SPACES | NS_COLLECTION_FULLSCREEN_AUXILIARY;
        let _: () = msg_send![window_ptr, setCollectionBehavior: behavior];
        let _: () = msg_send![window_ptr, setOpaque: true];

        // Paint the window black behind the GL surface so there is never a white
        // flash between show and the first committed frame.
        let black: *mut AnyObject = msg_send![class!(NSColor), blackColor];
        let _: () = msg_send![window_ptr, setBackgroundColor: black];

        let view: *mut AnyObject = msg_send![window_ptr, contentView];

        // Physical pixel size — critical for Retina displays.  CGL/glutin work in
        // physical pixels, so passing logical size would render content in only the
        // bottom-left fraction of the framebuffer.
        let phys: NSSize =
            msg_send![view, convertSizeToBacking: NSSize::new(INITIAL_WIDTH, INITIAL_HEIGHT)];
        let phys_w = (phys.width as u32).max(1);
        let phys_h = (phys.height as u32).max(1);

        std::mem::forget(window);

        // Output window starts hidden; shown on first GO or by F9 / View menu.
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![window_ptr, orderOut: nil];

        // Keep GL surface size in sync when the user drags the window border.
        log::info!(
            "[macos-window] NSWindow created (resizable, \
             {INITIAL_WIDTH}x{INITIAL_HEIGHT} logical at ({win_x},{win_y}), \
             {phys_w}x{phys_h} physical)"
        );

        (view as usize, window_ptr as usize, phys_w, phys_h)
    }
}

/// Update `GL_WIDTH`/`GL_HEIGHT` from the current window's physical pixel size.
/// Called from `windowDidResize:` (main thread).
fn update_physical_size(state: &MacOutputWindow) {
    let ptr = state.raw_window.load(Ordering::SeqCst);
    if ptr == 0 {
        return;
    }
    unsafe {
        let window = ptr as *mut AnyObject;
        let view: *mut AnyObject = msg_send![window, contentView];
        let bounds: NSRect = msg_send![view, bounds];
        let phys: NSSize = msg_send![view, convertSizeToBacking: bounds.size];
        let w = (phys.width as u32).max(1);
        let h = (phys.height as u32).max(1);
        if let Some(runtime) = state.runtime.upgrade() {
            super::render::set_surface_size(&runtime, w, h);
            let (always_on_top, hide_cursor) = runtime.window_preferences();
            apply_preferences(&runtime, always_on_top, hide_cursor);
        }
    }
}

/// Register an `NSNotificationCenter` observer so that when the user resizes the
/// window by dragging its edge, the GL surface is immediately updated.
fn register_resize_observer(window_ptr: *mut AnyObject, state: &Arc<MacOutputWindow>) {
    use block2::RcBlock;
    unsafe {
        let name: *mut AnyObject = msg_send![
            class!(NSString),
            stringWithUTF8String: c"NSWindowDidResizeNotification".as_ptr()
        ];
        // queue: nil → block runs on the thread that posts the notification (main).
        let state = Arc::clone(state);
        let block = RcBlock::new(move |_notif: *mut AnyObject| {
            update_physical_size(&state);
        });
        let nc: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _obs: *mut AnyObject = msg_send![
            nc,
            addObserverForName: name,
            object: window_ptr,
            queue: nil,
            usingBlock: &*block
        ];
        // NSNotificationCenter copies the block; we abandon our Rc without
        // dropping so the block stays alive for the app's lifetime.
        std::mem::forget(block);
    }
}

/// Run `f` with the live `*mut NSWindow` on the main thread (inline if already
/// there, otherwise marshalled via the Tauri app handle).
fn on_main<F>(runtime: &RenderRuntime, f: F)
where
    F: FnOnce(*mut AnyObject, Arc<MacOutputWindow>) + Send + 'static,
{
    let state = runtime.mac_window.lock().ok().and_then(|g| g.as_ref().cloned());
    let Some(state) = state else { return; };
    let app_handle = state.app_handle.clone();
    let run = move || {
        let ptr = state.raw_window.load(Ordering::SeqCst);
        if ptr != 0 {
            f(ptr as *mut AnyObject, state);
        }
    };

    if MainThreadMarker::new().is_some() {
        run();
    } else { let _ = app_handle.run_on_main_thread(run); }
}
