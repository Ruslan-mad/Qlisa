//! Shared external WebView surface used by Browser Cues.
//!
//! This is intentionally a single surface for the whole application. Browser
//! pages are exclusive visual sources in the MVP; they are not composited by
//! libmpv and are not sent to NDI/SRT. Keeping one window also prevents a cue
//! list with many browser cues from creating one WebView/process per cue.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use serde::Serialize;
use tauri::{Emitter, Manager, WebviewWindow};
use url::Url;
use uuid::Uuid;

use super::types::ScreenInfo;

const LABEL: &str = "browser-surface";

fn monitor_matches_target(
    position: (i32, i32),
    size: (u32, u32),
    target: &ScreenInfo,
) -> bool {
    position == (target.x, target.y) && size == (target.width, target.height)
}

/// Validate and normalise a browser URL. Only HTTP(S) is allowed; this keeps
/// file/data/javascript/custom-protocol URLs out of the external WebView.
pub fn validate_browser_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).map_err(|_| anyhow!("Browser URL is invalid"))?;
    match url.scheme() {
        "http" | "https" if url.host_str().is_some() => Ok(url),
        _ => Err(anyhow!("Browser URL must use http or https")),
    }
}

fn display_url(url: &Url) -> String {
    // Never expose query/fragment values in status events or diagnostics: a
    // local page may carry a token in either field.
    let mut safe = url.clone();
    let _ = safe.set_username("");
    let _ = safe.set_password(None);
    safe.set_query(None);
    safe.set_fragment(None);
    safe.to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowserSurfaceState {
    pub active: bool,
    pub cue_id: Option<Uuid>,
    pub output_id: Option<String>,
    pub url: Option<String>,
    pub status: String,
    pub error: Option<String>,
    #[serde(skip)]
    generation: u64,
}

impl Default for BrowserSurfaceState {
    fn default() -> Self {
        Self {
            active: false,
            cue_id: None,
            output_id: None,
            url: None,
            status: "idle".into(),
            error: None,
            generation: 0,
        }
    }
}

/// Application-wide owner of the one Browser WebView.
#[derive(Clone)]
pub struct BrowserSurfaceManager {
    app_handle: tauri::AppHandle,
    state: Arc<Mutex<BrowserSurfaceState>>,
    loaded_url: Arc<Mutex<Option<Url>>>,
}

impl BrowserSurfaceManager {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self {
            app_handle,
            state: Arc::new(Mutex::new(BrowserSurfaceState::default())),
            loaded_url: Arc::new(Mutex::new(None)),
        }
    }

    pub fn state(&self) -> BrowserSurfaceState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn emit_state(&self) {
        let _ = self.app_handle.emit("browser-surface-state", self.state());
    }

    fn get_window(&self) -> Option<WebviewWindow> {
        self.app_handle.get_webview_window(LABEL)
    }

    fn is_current(&self, generation: u64) -> bool {
        self.state
            .lock()
            .map(|state| state.generation == generation)
            .unwrap_or(false)
    }

    fn set_error(&self, generation: u64, error: String) {
        log::warn!("[browser] native operation failed generation={generation}: {error}");
        if let Ok(mut state) = self.state.lock() {
            if state.generation != generation {
                return;
            }
            state.active = false;
            state.status = "error".into();
            state.error = Some(error);
        }
        self.emit_state();
    }

    fn start_on_main_thread(
        &self,
        generation: u64,
        cue_id: Uuid,
        url: Url,
        reload_on_go: bool,
        zoom: f64,
        monitor: Option<ScreenInfo>,
    ) {
        if !self.is_current(generation) {
            log::info!("[browser] ignoring stale start generation={generation} cue={cue_id}");
            return;
        }

        let safe_url = display_url(&url);
        let result = (|| -> Result<()> {
            let window = match self.get_window() {
                Some(window) => {
                    let should_navigate = reload_on_go
                        || self
                            .loaded_url
                            .lock()
                            .ok()
                            .and_then(|loaded| loaded.as_ref().map(|loaded| loaded != &url))
                            .unwrap_or(true);
                    if should_navigate {
                        window
                            .navigate(url.clone())
                            .map_err(|_| anyhow!("Browser page could not be loaded"))?;
                    }
                    window
                }
                // The Browser surface is declared as a hidden window in
                // tauri.conf.json. Creating a WebView here used to run inside
                // the main-thread dispatcher during GO. WebView2 creation can
                // synchronously initialise a browser process, which freezes
                // the whole app before this callback can publish "ready".
                // Reuse the pre-created surface so GO only performs a normal
                // navigation and window transition.
                None => return Err(anyhow!("Browser surface window is unavailable")),
            };

            // A stop/new-owner request can arrive while the native operation
            // above is in progress. Do not let stale work reveal an old cue.
            if !self.is_current(generation) {
                let _ = window.hide();
                log::info!(
                    "[browser] hiding stale native start generation={generation} cue={cue_id}"
                );
                return Ok(());
            }

            if let Ok(mut loaded) = self.loaded_url.lock() {
                *loaded = Some(url);
            }
            // A previous GO may have left this hidden window in native
            // fullscreen.  Exit it before moving; native window managers
            // commonly ignore position/size changes while fullscreen.
            window
                .set_fullscreen(false)
                .map_err(|_| anyhow!("Browser surface could not leave fullscreen"))?;
            if let Some(target) = monitor {
                let native_monitor = window
                    .available_monitors()
                    .map_err(|_| anyhow!("Browser surface could not enumerate outputs"))?
                    .into_iter()
                    .find(|candidate| {
                        let position = candidate.position();
                        let size = candidate.size();
                        monitor_matches_target(
                            (position.x, position.y),
                            (size.width, size.height),
                            &target,
                        )
                    })
                    .ok_or_else(|| anyhow!("Browser output monitor is unavailable"))?;
                window
                    .set_position(tauri::Position::Physical(*native_monitor.position()))
                    .map_err(|_| anyhow!("Browser surface could not select output"))?;
                window
                    .set_size(tauri::Size::Physical(*native_monitor.size()))
                    .map_err(|_| anyhow!("Browser surface could not size to output"))?;
            }
            window
                .set_zoom(zoom.clamp(0.25, 3.0))
                .map_err(|_| anyhow!("Browser zoom could not be applied"))?;
            // The surface is borderless and pre-created, so entering native
            // fullscreen here no longer performs WebView construction on the
            // UI dispatcher. Keep it to cover the monitor and its taskbar.
            window
                .set_fullscreen(true)
                .map_err(|_| anyhow!("Browser surface could not enter fullscreen"))?;
            window
                .show()
                // Do not focus the output monitor. Focusing a WebView output
                // can steal keyboard input from the operator window.
                .map_err(|_| anyhow!("Browser surface could not be shown"))?;
            Ok(())
        })();

        match result {
            Ok(()) => {
                if let Ok(mut state) = self.state.lock() {
                    if state.generation == generation {
                        state.active = true;
                        state.cue_id = Some(cue_id);
                        state.status = "ready".into();
                        state.error = None;
                        state.url = Some(safe_url);
                    }
                }
                log::info!("[browser] native ready generation={generation} cue={cue_id}");
                self.emit_state();
            }
            Err(error) => {
                if let Some(window) = self.get_window() {
                    let _ = window.hide();
                }
                log::warn!(
                    "[browser] native start failed generation={generation} cue={cue_id}: {error}"
                );
                self.set_error(generation, error.to_string())
            }
        }
    }

    pub fn start(
        &self,
        cue_id: Uuid,
        raw_url: &str,
        reload_on_go: bool,
        zoom: f64,
        output_id: Option<&str>,
        monitor: Option<ScreenInfo>,
    ) -> Result<()> {
        let url = validate_browser_url(raw_url)?;
        let safe_url = display_url(&url);
        let generation = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow!("Browser surface state is unavailable"))?;
            reserve_start(
                &mut state,
                cue_id,
                output_id.map(str::to_owned),
                Some(safe_url.clone()),
            )
        };
        log::info!(
            "[browser] start reserved generation={generation} cue={cue_id} url={} output={output_id:?}",
            safe_url
        );
        self.emit_state();
        // A main-thread callback can be delayed by native window work. Do not
        // leave the cue permanently in `loading` if the platform dispatcher or
        // WebView runtime is unavailable. The generation bump invalidates any
        // late callback before it can reveal a stale page.
        let timeout_manager = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            timeout_manager.fail_if_loading(generation, "Browser surface start timed out");
        });
        let manager = self.clone();
        self.app_handle
            .run_on_main_thread(move || {
                manager.start_on_main_thread(
                    generation,
                    cue_id,
                    url,
                    reload_on_go,
                    zoom,
                    monitor,
                );
            })
            .map_err(|error| {
                let message = format!("Browser surface request could not be scheduled: {error}");
                self.set_error(generation, message.clone());
                anyhow!(message)
            })?;
        Ok(())
    }

    pub fn stop(&self, cue_id: Uuid, hard: bool) -> Result<()> {
        let generation = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow!("Browser surface state is unavailable"))?;
            match reserve_stop(&mut state, cue_id, hard) {
                Some(generation) => generation,
                None => return Ok(()),
            }
        };
        log::info!("[browser] stop reserved generation={generation} cue={cue_id} hard={hard}");
        if hard {
            // Keep the native WebView allocated, but make the next GO perform
            // a fresh navigation instead of reviving a page after Hard Stop.
            if let Ok(mut loaded) = self.loaded_url.lock() {
                *loaded = None;
            }
        }
        self.emit_state();
        let manager = self.clone();
        self.app_handle
            .run_on_main_thread(move || {
                if !manager.is_current(generation) {
                    return;
                }
                if let Some(window) = manager.get_window() {
                    // Keep the pre-created WebView alive for reuse. Closing it
                    // here would put the next GO back on the blocking builder
                    // path and reintroduce the original UI hang.
                    let result = window.hide();
                    if result.is_ok() && hard {
                        // Hard Stop also stops page-owned timers and scripts.
                        // `about:blank` is local to the WebView and avoids
                        // navigating to the app shell (which would start a
                        // second Qlisa frontend instance).
                        if let Err(error) = window.navigate(
                            Url::parse("about:blank").expect("about:blank is a valid URL"),
                        ) {
                            log::warn!("[browser] hard stop blank navigation failed generation={generation} cue={cue_id}: {error}");
                        }
                    }
                    if let Err(error) = result {
                        log::warn!("[browser] native stop failed generation={generation} cue={cue_id}: {error}");
                        manager.set_error(
                            generation,
                            format!("Browser surface could not stop: {error}"),
                        );
                    }
                    else {
                        log::info!("[browser] native stop complete generation={generation} cue={cue_id} hard={hard}");
                    }
                }
            })
            .map_err(|error| anyhow!("Browser surface stop could not be scheduled: {error}"))?;
        Ok(())
    }

    fn fail_if_loading(&self, generation: u64, message: &str) {
        let should_emit = self
            .state
            .lock()
            .map(|mut state| mark_loading_timeout(&mut state, generation, message))
            .unwrap_or(false);
        if should_emit {
            log::warn!("[browser] {message} generation={generation}");
            self.emit_state();
        }
    }
}

fn mark_loading_timeout(state: &mut BrowserSurfaceState, generation: u64, message: &str) -> bool {
    if state.generation != generation || state.status != "loading" {
        return false;
    }
    state.generation = state.generation.wrapping_add(1);
    state.active = false;
    state.cue_id = None;
    state.output_id = None;
    state.status = "error".into();
    state.error = Some(message.into());
    true
}

fn reserve_start(
    state: &mut BrowserSurfaceState,
    cue_id: Uuid,
    output_id: Option<String>,
    url: Option<String>,
) -> u64 {
    state.generation = state.generation.wrapping_add(1);
    if cue_id != Uuid::nil() {
        state.active = true;
        state.cue_id = Some(cue_id);
        state.output_id = output_id;
        state.url = url;
        state.status = "loading".into();
        state.error = None;
    }
    state.generation
}

fn reserve_stop(state: &mut BrowserSurfaceState, cue_id: Uuid, hard: bool) -> Option<u64> {
    if state.cue_id != Some(cue_id) {
        return None;
    }
    state.generation = state.generation.wrapping_add(1);
    state.active = false;
    state.cue_id = None;
    state.output_id = None;
    state.status = if hard { "closed" } else { "stopped" }.into();
    state.error = None;
    Some(state.generation)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        mark_loading_timeout, reserve_start, reserve_stop, validate_browser_url,
        monitor_matches_target, BrowserSurfaceState, LABEL,
    };
    use crate::engine::output_engine::types::ScreenInfo;

    #[test]
    fn only_http_and_https_urls_are_allowed() {
        assert!(validate_browser_url("http://localhost:8080/live").is_ok());
        assert!(validate_browser_url("https://example.test/page").is_ok());
        assert!(validate_browser_url("file:///tmp/index.html").is_err());
        assert!(validate_browser_url("javascript:alert(1)").is_err());
        assert!(validate_browser_url("data:text/html,hello").is_err());
    }

    #[test]
    fn url_query_is_accepted_without_being_part_of_safe_display() {
        let url = validate_browser_url("http://localhost:8080/live?token=secret#x").unwrap();
        assert_eq!(url.host_str(), Some("localhost"));
        assert_eq!(url.query(), Some("token=secret"));
        // The manager's event payload strips query and fragment; the raw URL
        // remains only in the cue's persisted configuration.
        assert!(!super::display_url(&url).contains("secret"));
    }

    #[test]
    fn request_generation_rejects_stale_start_stop_and_new_owner() {
        let mut state = BrowserSurfaceState::default();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let first_generation = reserve_start(
            &mut state,
            first,
            Some("main".into()),
            Some("http://localhost:3000/one".into()),
        );
        let second_generation = reserve_start(
            &mut state,
            second,
            Some("main".into()),
            Some("http://localhost:3000/two".into()),
        );
        assert_ne!(first_generation, second_generation);
        assert_eq!(state.cue_id, Some(second));
        assert_eq!(state.status, "loading");
        assert_eq!(reserve_stop(&mut state, first, false), None);
        let stopped_generation = reserve_stop(&mut state, second, false).unwrap();
        assert_ne!(stopped_generation, second_generation);
        assert!(!state.active);
        assert_eq!(state.status, "stopped");
        let third_generation = reserve_start(
            &mut state,
            first,
            None,
            Some("http://localhost:3000/three".into()),
        );
        assert_ne!(third_generation, stopped_generation);
        assert_eq!(state.cue_id, Some(first));
        assert!(state.active);
    }

    #[test]
    fn request_reservation_does_not_perform_native_work() {
        let mut state = BrowserSurfaceState::default();
        let cue_id = Uuid::new_v4();
        let generation = reserve_start(
            &mut state,
            cue_id,
            None,
            Some("http://localhost:3000".into()),
        );
        assert_eq!(state.generation, generation);
        assert_eq!(state.status, "loading");
    }

    #[test]
    fn browser_surface_is_precreated_from_dedicated_bootstrap_page() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../../tauri.conf.json")).unwrap();
        let window = config["app"]["windows"]
            .as_array()
            .and_then(|windows| windows.iter().find(|window| window["label"] == LABEL))
            .expect("browser surface window must be declared in Tauri config");
        assert_eq!(window["url"], "browser-surface.html");
        assert_eq!(window["visible"], false);
        assert_eq!(window["alwaysOnTop"], true);
        assert_eq!(window["skipTaskbar"], true);
    }

    #[test]
    fn monitor_routing_matches_physical_identity_not_enumeration_order() {
        let target = ScreenInfo {
            index: 1,
            width: 1920,
            height: 1080,
            x: 1920,
            y: 0,
            is_primary: false,
        };
        assert!(monitor_matches_target((1920, 0), (1920, 1080), &target));
        assert!(!monitor_matches_target((0, 0), (1920, 1080), &target));
    }

    #[test]
    fn loading_timeout_invalidates_only_the_current_request() {
        let mut state = BrowserSurfaceState::default();
        let cue_id = Uuid::new_v4();
        let generation = reserve_start(
            &mut state,
            cue_id,
            Some("main".into()),
            Some("http://localhost:3000".into()),
        );
        assert!(mark_loading_timeout(&mut state, generation, "timeout"));
        assert_eq!(state.status, "error");
        assert!(!state.active);
        assert_eq!(state.cue_id, None);
        assert!(!mark_loading_timeout(
            &mut state,
            generation,
            "stale timeout"
        ));
    }
}
