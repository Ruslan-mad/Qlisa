//! Fade overlay helpers (master blackout quad).
//!
//! `FADE_STATE` is the single source of truth for the current overlay alpha.
//! `tick_fade()` is called by the render thread each frame to advance the
//! animation.  `execute_pending()` fires when a fade completes.  No separate
//! fade thread is needed; the render loop drives animation timing.

use super::types::FadePending;
use super::{cs, PipelineState};

// ---------------------------------------------------------------------------
// Alpha state
// ---------------------------------------------------------------------------

/// Hard-cut the overlay to `alpha` with no animation.
///
/// Sets `current_alpha`, `target_alpha`, and resets `duration_ms` so that
/// `tick_fade()` holds at this value without transitioning.  Calling only
/// `s.current_alpha = alpha` while leaving a stale `target_alpha` would cause
/// `tick_fade()` to immediately snap back to the old target.
pub(super) fn set_overlay_alpha(p: &PipelineState, alpha: u8) {
        if let Ok(mut s) = p.fade.lock() {
            s.current_alpha = alpha;
            s.target_alpha = alpha;
            s.start_alpha = alpha;
            s.duration_ms = 0;
            s.start_time = std::time::Instant::now();
        }
}

// ---------------------------------------------------------------------------
// Per-frame tick + pending action executor
// ---------------------------------------------------------------------------

/// Advance the fade animation by one render-thread frame.
///
/// Returns `(current_alpha, did_complete)`.  `did_complete` is `true` exactly
/// once — on the frame where `current_alpha` first reaches `target_alpha`.
/// The caller should invoke `execute_pending()` when `did_complete` is `true`.
pub(super) fn tick_fade(p: &PipelineState) -> (u8, bool) {
    let mut state = match p.fade.lock() {
        Ok(s) => s,
        Err(_) => return (0, false),
    };

    if state.current_alpha == state.target_alpha {
        return (state.current_alpha, false);
    }

    let elapsed = state.start_time.elapsed().as_millis() as u32;
    let t = if state.duration_ms == 0 {
        1.0_f32
    } else {
        (elapsed as f32 / state.duration_ms as f32).min(1.0)
    };
    let start = state.start_alpha as f32;
    let end = state.target_alpha as f32;
    let alpha = (start + (end - start) * t).round().clamp(0.0, 255.0) as u8;
    state.current_alpha = alpha;

    let done = t >= 1.0;
    if done {
        state.current_alpha = state.target_alpha;
    }
    (alpha, done)
}

/// Start or reverse the dedicated operator FTB layer. This state is separate
/// from cue fades, so content changes cannot clear an operator blackout.
pub(super) fn set_operator_blackout(p: &PipelineState, enabled: bool, duration_ms: u32) {
    if let Ok(mut state) = p.operator_ftb.lock() {
        state.start_alpha = state.current_alpha;
        state.target_alpha = if enabled { 255 } else { 0 };
        state.duration_ms = duration_ms;
        state.start_time = std::time::Instant::now();
        if duration_ms == 0 {
            state.current_alpha = state.target_alpha;
            state.start_alpha = state.target_alpha;
        }
    }
}

/// Advance the dedicated operator FTB animation. It has no pending action.
pub(super) fn tick_operator_blackout(p: &PipelineState) -> (u8, bool) {
    let mut state = match p.operator_ftb.lock() {
        Ok(s) => s,
        Err(_) => return (0, false),
    };
    if state.current_alpha == state.target_alpha {
        return (state.current_alpha, false);
    }
    let elapsed = state.start_time.elapsed().as_millis() as u32;
    let t = if state.duration_ms == 0 {
        1.0_f32
    } else {
        (elapsed as f32 / state.duration_ms as f32).min(1.0)
    };
    let start = state.start_alpha as f32;
    let end = state.target_alpha as f32;
    state.current_alpha = (start + (end - start) * t).round().clamp(0.0, 255.0) as u8;
    let done = t >= 1.0;
    if done {
        state.current_alpha = state.target_alpha;
    }
    (state.current_alpha, done)
}

/// Execute the action that was pending behind a completed fade.
///
/// Called by the render thread immediately after `tick_fade()` returns
/// `did_complete = true`.
pub(super) fn execute_pending(p: &PipelineState) {
    let pending = p.fade.lock().ok().and_then(|mut s| s.pending.take());

    match pending {
        Some(FadePending::Stop) => {
            // Guard: new content may have been loaded while the stop fade ran.
            // In that case, don't issue a `stop` command — just clear the overlay.
            let has_new_content = p.current_voice.lock().ok()
                .map(|cv| cv.is_some())
                .unwrap_or(false);
            if has_new_content {
                set_overlay_alpha(p, 0);
                return;
            }
            let Some(_permit) = p.enter_mpv_call() else {
                return;
            };
            if let (Some(lib), Some(ctx)) = (p.mpv_lib.lock().ok().and_then(|x| x.clone()), p.mpv_ctx.lock().ok().and_then(|x| x.clone())) {
                unsafe {
                    let stop = cs("stop");
                    let args: [*const std::ffi::c_char; 2] = [stop.as_ptr(), std::ptr::null()];
                (lib.mpv_command)(ctx.0, args.as_ptr());
                }
            }
            // Overlay stays at alpha=255 (black); mpv has no content to show.
        }
        None => {
            // Fade-in completed — nothing more to do.
        }
    }
}
