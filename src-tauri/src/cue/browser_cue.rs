//! Browser Cue: one safe external HTTP(S) page on the shared browser surface.
//!
//! The page owns its own refresh loop (for example a local dashboard using
//! `setInterval`). Qlisa only navigates/reloads it at GO and controls the
//! shared surface lifecycle.

use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    context::{CueContext, CueEvent},
    traits::{Cue, CueFactory, ExecutionMarker, RuntimeState},
    types::{ContinueMode, CueColor, CueId, CueState, CueType},
};

pub struct BrowserCue {
    id: CueId,
    name: String,
    number: Option<String>,
    notes: String,
    color: CueColor,
    state: CueState,
    pre_wait: Duration,
    post_wait: Duration,
    started_at: Option<Instant>,
    action_started_at_value: Option<Instant>,
    elapsed_before_pause: Duration,
    action_elapsed_before_pause: Duration,
    continue_mode: ContinueMode,
    pub url: String,
    pub output_id: Option<String>,
    pub output_ids: Vec<String>,
    /// Navigate on every GO when true. When false, keep the loaded page alive
    /// and show it again without creating a new WebView.
    pub reload_on_go: bool,
    /// CSS/WebView zoom scale, clamped by the shared surface manager.
    pub zoom: f64,
    is_disabled: bool,
    marker: ExecutionMarker,
    runtime_error: Option<String>,
}

impl BrowserCue {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "Browser Cue".into(),
            number: None,
            notes: String::new(),
            color: CueColor::None,
            state: CueState::Standby,
            pre_wait: Duration::ZERO,
            post_wait: Duration::ZERO,
            started_at: None,
            action_started_at_value: None,
            elapsed_before_pause: Duration::ZERO,
            action_elapsed_before_pause: Duration::ZERO,
            continue_mode: ContinueMode::DoNotContinue,
            url: "http://localhost:3000".into(),
            output_id: None,
            output_ids: Vec::new(),
            reload_on_go: true,
            zoom: 1.0,
            is_disabled: false,
            marker: ExecutionMarker::default(),
            runtime_error: None,
        }
    }

    fn start_action(&mut self, context: &CueContext) -> Result<()> {
        let output_id = self
            .output_id
            .as_deref()
            .or_else(|| self.output_ids.first().map(String::as_str));
        if let Err(error) = context.output_engine.start_browser_surface(
            self.id,
            &self.url,
            self.reload_on_go,
            self.zoom,
            output_id,
        ) {
            self.runtime_error = Some(error.to_string());
            self.state = CueState::Standby;
            self.started_at = None;
            return Err(error);
        }
        self.runtime_error = None;
        self.action_started_at_value = Some(Instant::now());
        self.action_elapsed_before_pause = Duration::ZERO;
        context.record_browser_start(self.id);
        context.emit(CueEvent::ActionStarted { cue_id: self.id });
        Ok(())
    }
}

impl Default for BrowserCue {
    fn default() -> Self {
        Self::new()
    }
}

impl Cue for BrowserCue {
    fn id(&self) -> CueId {
        self.id
    }
    fn cue_type(&self) -> CueType {
        CueType::Browser
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn set_name(&mut self, name: String) {
        self.name = name;
    }
    fn number(&self) -> Option<&str> {
        self.number.as_deref()
    }
    fn set_number(&mut self, number: Option<String>) {
        self.number = number;
    }
    fn notes(&self) -> &str {
        &self.notes
    }
    fn set_notes(&mut self, notes: String) {
        self.notes = notes;
    }
    fn color(&self) -> CueColor {
        self.color
    }
    fn set_color(&mut self, color: CueColor) {
        self.color = color;
    }
    fn is_disabled(&self) -> bool {
        self.is_disabled
    }
    fn set_disabled(&mut self, disabled: bool) {
        self.is_disabled = disabled;
    }
    fn state(&self) -> CueState {
        self.state
    }

    fn load(&mut self, _context: &CueContext) -> Result<()> {
        Ok(())
    }

    fn go(&mut self, context: &CueContext) -> Result<()> {
        if self.state == CueState::Running {
            return Ok(());
        }
        self.marker.begin();
        self.state = CueState::Running;
        self.started_at = Some(Instant::now());
        self.action_started_at_value = None;
        self.elapsed_before_pause = Duration::ZERO;
        self.action_elapsed_before_pause = Duration::ZERO;
        self.runtime_error = None;
        if self.pre_wait.is_zero() {
            self.start_action(context)
        } else {
            Ok(())
        }
    }

    fn stop(&mut self, context: &CueContext) -> Result<()> {
        context.output_engine.stop_browser_surface(self.id, false)?;
        self.marker.cancel();
        self.state = CueState::Standby;
        self.started_at = None;
        self.action_started_at_value = None;
        self.action_elapsed_before_pause = Duration::ZERO;
        context.emit(CueEvent::Stopped { cue_id: self.id });
        Ok(())
    }

    fn pause(&mut self, _context: &CueContext) -> Result<()> {
        // A web page owns its refresh loop. There is no meaningful paused
        // rendering state without injecting page-specific JavaScript, so keep
        // the cue Running while the shared surface remains live.
        Ok(())
    }

    fn resume(&mut self, _context: &CueContext) -> Result<()> {
        // See pause(): the page continues to update, matching the actual
        // surface state and the Camera cue's no-op transport semantics.
        Ok(())
    }

    fn hard_stop(&mut self, context: &CueContext) -> Result<()> {
        context.output_engine.stop_browser_surface(self.id, true)?;
        self.marker.cancel();
        self.state = CueState::Standby;
        self.started_at = None;
        self.action_started_at_value = None;
        self.action_elapsed_before_pause = Duration::ZERO;
        context.emit(CueEvent::Stopped { cue_id: self.id });
        Ok(())
    }

    fn reset(&mut self) -> Result<()> {
        // Reset is called without a CueContext by the generic transport. The
        // surface is therefore hidden by the preceding stop/hard-stop path;
        // ownership is still cleared in the next lifecycle call.
        self.marker.cancel();
        self.state = CueState::Standby;
        self.started_at = None;
        self.action_started_at_value = None;
        self.elapsed_before_pause = Duration::ZERO;
        self.action_elapsed_before_pause = Duration::ZERO;
        Ok(())
    }

    fn tick(&mut self, context: &CueContext) -> Result<()> {
        if self.state == CueState::Running
            && self.started_at.is_some()
            && self.elapsed() >= self.pre_wait
            && self.action_started_at().is_none()
        {
            self.start_action(context)?;
        }
        Ok(())
    }

    fn runtime_error(&self) -> Option<&str> {
        self.runtime_error.as_deref()
    }
    fn is_action_started(&self) -> bool {
        self.pre_wait.is_zero() || self.action_started_at().is_some()
    }
    fn pre_wait(&self) -> Duration {
        self.pre_wait
    }
    fn set_pre_wait(&mut self, d: Duration) {
        self.pre_wait = d;
    }
    fn post_wait(&self) -> Duration {
        self.post_wait
    }
    fn set_post_wait(&mut self, d: Duration) {
        self.post_wait = d;
    }
    fn duration(&self) -> Option<Duration> {
        None
    }
    fn elapsed(&self) -> Duration {
        if self.state == CueState::Paused {
            return self.elapsed_before_pause;
        }
        self.started_at
            .map(|time| time.elapsed())
            .unwrap_or_default()
    }
    fn action_elapsed(&self) -> Duration {
        if self.state == CueState::Paused {
            return self.action_elapsed_before_pause;
        }
        self.action_started_at_value
            .map(|time| time.elapsed())
            .unwrap_or_default()
    }
    fn continue_mode(&self) -> ContinueMode {
        self.continue_mode
    }
    fn set_continue_mode(&mut self, mode: ContinueMode) {
        self.continue_mode = mode;
    }
    fn stop_on_next_go(&self) -> bool {
        true
    }
    fn is_visual(&self) -> bool {
        true
    }
    fn play_generation(&self) -> u64 {
        self.marker.generation()
    }
    fn is_auto_continue_fired(&self) -> bool {
        self.marker.is_fired()
    }
    fn auto_continue_marker(&self) -> Option<bool> {
        Some(self.marker.is_fired())
    }
    fn mark_auto_continue_fired(&mut self) {
        self.marker.mark_fired();
    }
    fn clear_auto_continue_fired(&mut self) {
        self.marker.cancel();
    }
    fn runtime_state(&self) -> RuntimeState {
        RuntimeState {
            state: self.state,
            voice_id: None,
            started_at: self.started_at,
            action_started_at: self.action_started_at_value,
        }
    }
    fn restore_runtime_state(&mut self, snap: RuntimeState) {
        self.state = snap.state;
        self.started_at = snap.started_at;
        self.action_started_at_value = snap.action_started_at;
    }

    fn serialize(&self) -> Value {
        json!({
            "type": "browser", "cue_type": "browser", "id": self.id,
            "number": self.number, "name": self.name, "notes": self.notes,
            "color": self.color, "pre_wait_ms": self.pre_wait.as_millis() as u64,
            "post_wait_ms": self.post_wait.as_millis() as u64,
            "continue_mode": self.continue_mode, "url": self.url,
            "output_id": self.output_id, "output_ids": self.output_ids,
            "reload_on_go": self.reload_on_go, "zoom": self.zoom,
            "is_disabled": self.is_disabled,
        })
    }
}

impl BrowserCue {
    // The browser manager is the source of truth for whether the surface was
    // shown. This lightweight marker keeps pre-wait behaviour deterministic.
    fn action_started_at(&self) -> Option<Instant> {
        self.action_started_at_value
    }
}

pub struct BrowserCueFactory;

impl CueFactory for BrowserCueFactory {
    fn create(&self) -> Box<dyn Cue> {
        Box::new(BrowserCue::new())
    }
    fn from_json(&self, value: Value) -> anyhow::Result<Box<dyn Cue>> {
        let mut cue = BrowserCue::new();
        if let Some(s) = value.get("id").and_then(Value::as_str) {
            cue.id = s.parse().unwrap_or_else(|_| Uuid::new_v4());
        }
        if let Some(s) = value.get("number").and_then(Value::as_str) {
            cue.number = Some(s.into());
        }
        if let Some(s) = value.get("name").and_then(Value::as_str) {
            cue.name = s.into();
        }
        if let Some(s) = value.get("notes").and_then(Value::as_str) {
            cue.notes = s.into();
        }
        if let Some(c) = value
            .get("color")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            cue.color = c;
        }
        if let Some(ms) = value.get("pre_wait_ms").and_then(Value::as_u64) {
            cue.pre_wait = Duration::from_millis(ms);
        }
        if let Some(ms) = value.get("post_wait_ms").and_then(Value::as_u64) {
            cue.post_wait = Duration::from_millis(ms);
        }
        if let Some(mode) = value
            .get("continue_mode")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            cue.continue_mode = mode;
        }
        if let Some(s) = value.get("url").and_then(Value::as_str) {
            cue.url = s.into();
        }
        cue.output_id = value
            .get("output_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        cue.output_ids = value
            .get("output_ids")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if cue.output_ids.is_empty() {
            if let Some(id) = cue.output_id.clone() {
                cue.output_ids.push(id);
            }
        }
        if let Some(b) = value.get("reload_on_go").and_then(Value::as_bool) {
            cue.reload_on_go = b;
        }
        if let Some(z) = value.get("zoom").and_then(Value::as_f64) {
            cue.zoom = z.clamp(0.25, 3.0);
        }
        if let Some(b) = value.get("is_disabled").and_then(Value::as_bool) {
            cue.is_disabled = b;
        }
        Ok(Box::new(cue))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_defaults_are_safe_and_reusable() {
        let cue = BrowserCue::new();
        assert_eq!(cue.cue_type(), CueType::Browser);
        assert!(cue.reload_on_go);
        assert_eq!(cue.zoom, 1.0);
        assert!(cue.is_visual());
        assert!(cue.stop_on_next_go());
    }
    #[test]
    fn browser_serialization_roundtrips_fields() {
        let factory = BrowserCueFactory;
        let mut cue = BrowserCue::new();
        cue.url = "http://localhost:8080/dashboard".into();
        cue.reload_on_go = false;
        cue.zoom = 1.25;
        let rebuilt = factory.from_json(cue.serialize()).unwrap();
        let json = rebuilt.serialize();
        assert_eq!(json["url"], "http://localhost:8080/dashboard");
        assert_eq!(json["reload_on_go"], false);
        assert_eq!(json["zoom"], 1.25);
    }
}
