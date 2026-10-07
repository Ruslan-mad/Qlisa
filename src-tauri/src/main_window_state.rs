//! Machine-local placement for the primary application window.

use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use tauri::{PhysicalPosition, PhysicalRect, PhysicalSize, WebviewWindow};

const MIN_WIDTH: u32 = 900;
const MIN_HEIGHT: u32 = 600;
const MAX_DIMENSION: u32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Rect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedWindowState {
    version: u8,
    rect: Rect,
    maximized: bool,
}

impl SavedWindowState {
    fn valid(self) -> bool {
        self.version == 1
            && self.rect.width >= MIN_WIDTH
            && self.rect.height >= MIN_HEIGHT
            && self.rect.width <= MAX_DIMENSION
            && self.rect.height <= MAX_DIMENSION
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WorkArea {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn path() -> PathBuf {
    crate::machine_config::config_base_dir()
        .join("Inkue")
        .join("main-window-state.json")
}

fn load() -> Option<SavedWindowState> {
    let bytes = fs::read(path()).ok()?;
    let state: SavedWindowState = serde_json::from_slice(&bytes).ok()?;
    state.valid().then_some(state)
}

fn save(state: SavedWindowState) {
    let destination = path();
    let Some(parent) = destination.parent() else { return };
    if let Err(error) = fs::create_dir_all(parent) {
        log::warn!("Could not create main window state directory: {error}");
        return;
    }
    let temporary = destination.with_extension("json.tmp");
    let result = (|| -> std::io::Result<()> {
        let bytes = serde_json::to_vec(&state)?;
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        atomic_replace(&temporary, &destination)
    })();
    if let Err(error) = result {
        log::warn!("Could not save main window state: {error}");
        let _ = fs::remove_file(temporary);
    }
}

#[cfg(windows)]
fn atomic_replace(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

#[cfg(not(windows))]
fn atomic_replace(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

fn clamp_rect(rect: Rect, work: WorkArea, scale_factor: f64) -> Rect {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let min_width = ((MIN_WIDTH as f64 * scale).round() as u32).min(work.width);
    let min_height = ((MIN_HEIGHT as f64 * scale).round() as u32).min(work.height);
    let width = rect.width.max(min_width).min(work.width);
    let height = rect.height.max(min_height).min(work.height);
    // Keep at least a small strip visible so the operator can drag the window
    // back if monitor layouts changed since the previous run.
    let visible_x = width.min(120) as i64;
    let visible_y = height.min(80) as i64;
    let min_x = work.x as i64 - width as i64 + visible_x;
    let max_x = work.x as i64 + work.width as i64 - visible_x;
    let min_y = work.y as i64;
    let max_y = work.y as i64 + work.height as i64 - visible_y;
    Rect {
        x: (rect.x as i64).clamp(min_x, max_x) as i32,
        y: (rect.y as i64).clamp(min_y, max_y) as i32,
        width,
        height,
    }
}

fn intersection_area(rect: Rect, area: WorkArea) -> u64 {
    let left = (rect.x as i64).max(area.x as i64);
    let top = (rect.y as i64).max(area.y as i64);
    let right = (rect.x as i64 + rect.width as i64)
        .min(area.x as i64 + area.width as i64);
    let bottom = (rect.y as i64 + rect.height as i64)
        .min(area.y as i64 + area.height as i64);
    if right <= left || bottom <= top { 0 } else { ((right - left) * (bottom - top)) as u64 }
}

pub(crate) type SharedState = Arc<Mutex<Option<SavedWindowState>>>;

pub(crate) fn shared() -> SharedState {
    Arc::new(Mutex::new(load()))
}

fn current_rect(window: &WebviewWindow) -> Option<Rect> {
    let position = window.outer_position().ok()?;
    // set_size() controls the client area, so store the matching inner size.
    // This avoids adding the Windows resize frame on every launch.
    let size = window.inner_size().ok()?;
    Some(Rect { x: position.x, y: position.y, width: size.width, height: size.height })
}

fn monitors(window: &WebviewWindow) -> Vec<(WorkArea, f64)> {
    window.available_monitors().unwrap_or_default().into_iter().map(|monitor| {
        let area: &PhysicalRect<i32, u32> = monitor.work_area();
        (WorkArea {
            x: area.position.x,
            y: area.position.y,
            width: area.size.width,
            height: area.size.height,
        }, monitor.scale_factor())
    }).collect()
}

pub(crate) fn restore(window: &WebviewWindow, shared: &SharedState) {
    let Some(mut state) = shared.lock().ok().and_then(|guard| *guard) else {
        if let Some(rect) = current_rect(window) {
            if let Ok(mut guard) = shared.lock() {
                *guard = Some(SavedWindowState { version: 1, rect, maximized: false });
            }
        }
        return;
    };

    let available = monitors(window);
    if !available.is_empty() {
        let best = available.iter()
            .filter(|(area, _)| intersection_area(state.rect, *area) > 0)
            .max_by_key(|(area, _)| intersection_area(state.rect, *area));
        if let Some((work_area, scale)) = best {
            state.rect = clamp_rect(state.rect, *work_area, *scale);
            let _ = window.set_position(PhysicalPosition::new(state.rect.x, state.rect.y));
            let _ = window.set_size(PhysicalSize::new(state.rect.width, state.rect.height));
        } else if let Ok(Some(primary)) = window.primary_monitor() {
            let area = primary.work_area();
            let work_area = WorkArea {
                x: area.position.x,
                y: area.position.y,
                width: area.size.width,
                height: area.size.height,
            };
            // If the saved monitor was removed, move the whole window into the
            // primary work area instead of leaving only a draggable strip.
            state.rect.x = work_area.x;
            state.rect.y = work_area.y;
            state.rect = clamp_rect(state.rect, work_area, primary.scale_factor());
            let _ = window.set_position(PhysicalPosition::new(state.rect.x, state.rect.y));
            let _ = window.set_size(PhysicalSize::new(state.rect.width, state.rect.height));
        }
    }
    if state.maximized {
        let _ = window.maximize();
    }
    if let Ok(mut guard) = shared.lock() {
        *guard = Some(state);
    }
}

/// Track only normal geometry. A minimize event must preserve a previous
/// maximize flag and its normal rectangle.
fn apply_observed_state(state: &mut SavedWindowState, rect: Option<Rect>, minimized: bool, maximized: bool) {
    if minimized { return; }
    state.maximized = maximized;
    if !maximized {
        if let Some(rect) = rect.filter(|r| r.width >= MIN_WIDTH && r.height >= MIN_HEIGHT) {
            state.rect = rect;
        }
    }
}

pub(crate) fn track(window: &WebviewWindow, shared: &SharedState) {
    let minimized = window.is_minimized().unwrap_or(false);
    let maximized = window.is_maximized().unwrap_or(false);
    let rect = if minimized || maximized { None } else { current_rect(window) };
    if let Ok(mut guard) = shared.lock() {
        let Some(state) = guard.as_mut() else { return };
        apply_observed_state(state, rect, minimized, maximized);
    }
}

/// Persist the last cached state without querying a destroyed window handle.
pub(crate) fn flush(shared: &SharedState) {
    if let Ok(guard) = shared.lock() {
        if let Some(state) = *guard {
            if state.valid() { save(state); }
        }
    }
}

pub(crate) fn flush_window(window: &WebviewWindow, shared: &SharedState) {
    track(window, shared);
    flush(shared);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_offscreen_window_to_negative_monitor_work_area() {
        let work = WorkArea { x: -1920, y: 40, width: 1920, height: 1040 };
        let rect = Rect { x: 3000, y: -1200, width: 1400, height: 900 };
        assert_eq!(clamp_rect(rect, work, 1.0), Rect {
            x: -120,
            y: 40,
            width: 1400,
            height: 900,
        });
    }

    #[test]
    fn caps_window_to_work_area_and_respects_minimum_when_possible() {
        let work = WorkArea { x: 0, y: 0, width: 1000, height: 700 };
        assert_eq!(clamp_rect(Rect { x: -900, y: 600, width: 2000, height: 1000 }, work, 1.0),
            Rect { x: -880, y: 600, width: 1000, height: 700 });
    }

    #[test]
    fn rejects_invalid_state_and_tracks_maximize_without_replacing_normal_rect() {
        let invalid = SavedWindowState { version: 1, rect: Rect { x: 0, y: 0, width: 20, height: 20 }, maximized: false };
        assert!(!invalid.valid());
        let normal = Rect { x: -600, y: 80, width: 1200, height: 800 };
        let mut state = SavedWindowState { version: 1, rect: normal, maximized: false };
        apply_observed_state(&mut state, None, false, true);
        assert!(state.maximized);
        assert_eq!(state.rect, normal);
        apply_observed_state(&mut state, None, true, false);
        assert!(state.maximized);
        assert_eq!(state.rect, normal);
        apply_observed_state(&mut state, Some(Rect { x: 5, y: 6, width: 1280, height: 800 }), false, false);
        assert!(!state.maximized);
        assert_eq!(state.rect, Rect { x: 5, y: 6, width: 1280, height: 800 });
    }
}
