//! Timed text overlay driven by the show engine's normal Cue lifecycle.

use std::{collections::HashMap, path::{Path, PathBuf}, sync::Arc, time::{Duration, Instant}};

use anyhow::Result;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    context::{CueContext, CueEvent},
    traits::{Cue, CueFactory, RuntimeState},
    types::{ContinueMode, CueColor, CueId, CueState, CueType, db_to_linear},
};
use crate::engine::{ring_command::{FadeCurve, VoiceId}, voice::Voice};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerMode { #[default] Countdown, Countup }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerFormat { Seconds, #[default] MinutesSeconds }

pub struct TimerCue {
    id: CueId, name: String, number: Option<String>, notes: String, color: CueColor,
    state: CueState, duration: Duration, mode: TimerMode, format: TimerFormat,
    font: String, font_size: u32, text_color: String,
    geometry: crate::engine::output_engine::VideoGeometry,
    geometry_by_output: HashMap<String, crate::engine::output_engine::VideoGeometry>,
    output_id: Option<String>, output_ids: Vec<String>, screen_index: Option<u32>,
    sound_start: bool, sound_end: bool, sound_start_preset: String, sound_end_preset: String,
    sound_start_path: Option<PathBuf>, sound_end_path: Option<PathBuf>,
    sound_start_samples: Option<(Arc<Vec<f32>>, u16, u32)>, sound_end_samples: Option<(Arc<Vec<f32>>, u16, u32)>,
    sound_volume: f32, sound_patch_id: Option<Uuid>, finish_and_hide: bool,
    pre_wait: Duration, post_wait: Duration, continue_mode: ContinueMode,
    elapsed_before_pause: Duration, action_elapsed_before_pause: Duration,
    started_at: Option<Instant>, action_started_at: Option<Instant>, in_pre_wait: bool,
    generation: u64, start_sound_fired: bool, end_sound_fired: bool,
    auto_continue_fired: bool,
    zero_reached: bool, zero_duration: Option<Duration>,
    active_sound_voice: Option<VoiceId>, latched_output_id: Option<String>,
    latched_output_ids: Vec<String>, latched_screen_index: Option<u32>, last_rendered: Option<String>,
    disabled: bool,
}

impl TimerCue {
    pub fn new() -> Self { Self {
        id: Uuid::new_v4(), name: "Timer".into(), number: None, notes: String::new(), color: CueColor::None,
        state: CueState::Standby, duration: Duration::from_secs(60), mode: TimerMode::Countdown,
        format: TimerFormat::MinutesSeconds, font: "Arial".into(), font_size: 72, text_color: "#FFFFFF".into(),
        geometry: crate::engine::output_engine::VideoGeometry::default(), geometry_by_output: HashMap::new(),
        output_id: None, output_ids: Vec::new(), screen_index: None, sound_start: false, sound_end: false,
        sound_start_preset: "notification_1".into(), sound_end_preset: "notification_1".into(),
        sound_start_path: None, sound_end_path: None, sound_start_samples: None, sound_end_samples: None,
        sound_volume: 1.0, sound_patch_id: None, finish_and_hide: false,
        pre_wait: Duration::ZERO, post_wait: Duration::ZERO, continue_mode: ContinueMode::DoNotContinue,
        elapsed_before_pause: Duration::ZERO, action_elapsed_before_pause: Duration::ZERO,
        started_at: None, action_started_at: None, in_pre_wait: false,
        generation: 0, start_sound_fired: false, zero_reached: false, zero_duration: None, auto_continue_fired: false,
        end_sound_fired: false, active_sound_voice: None, latched_output_id: None,
        latched_output_ids: Vec::new(), latched_screen_index: None, last_rendered: None, disabled: false,
    } }

    fn elapsed_now(&self) -> Duration {
        self.elapsed_before_pause + self.started_at.map(|at| at.elapsed()).unwrap_or(Duration::ZERO)
    }
    fn action_elapsed_now(&self) -> Duration {
        self.action_elapsed_before_pause + self.action_started_at.map(|at| at.elapsed()).unwrap_or(Duration::ZERO)
    }
    fn shown_value(&self, elapsed: Duration) -> Duration {
        if self.zero_reached {
            return match self.mode { TimerMode::Countdown => Duration::ZERO, TimerMode::Countup => self.zero_duration.unwrap_or(self.duration) };
        }
        match self.mode {
            TimerMode::Countdown => self.duration.saturating_sub(elapsed),
            TimerMode::Countup => elapsed.min(self.duration),
        }
    }
    fn display(&self, elapsed: Duration) -> String {
        let value = self.shown_value(elapsed);
        let secs = match self.mode {
            TimerMode::Countdown if !value.is_zero() => (value.as_millis() + 999) / 1000,
            _ => value.as_secs() as u128,
        } as u64;
        match self.format {
            TimerFormat::Seconds => secs.to_string(),
            TimerFormat::MinutesSeconds => format!("{:02}:{:02}", secs / 60, secs % 60),
        }
    }
    fn expired(&self, elapsed: Duration) -> bool { elapsed >= self.duration }

    fn render(&mut self, context: &CueContext, force: bool) {
        let value = self.display(self.action_elapsed_now());
        if !force && self.last_rendered.as_deref() == Some(&value) { return; }
        self.last_rendered = Some(value.clone());
        let ass = build_ass(&value, &self.font, self.font_size, &self.text_color);
        context.output_engine.show_timer_overlay_with_geometry_for_outputs(
            self.id, &ass, self.latched_output_id.as_deref(), &self.latched_output_ids,
            self.latched_screen_index.or(context.output_screen),
            &self.geometry, &self.geometry_by_output,
        );
    }

    fn clear(&mut self, context: &CueContext) {
        context.output_engine.clear_timer_overlay_for_outputs(self.id, self.latched_output_id.as_deref(), &self.latched_output_ids);
        self.last_rendered = None;
        self.latched_output_id = None;
        self.latched_output_ids.clear();
    }

    fn play_sound(&mut self, context: &CueContext, start: bool) -> Result<()> {
        self.stop_sound(context);
        let cached = if start { self.sound_start_samples.as_ref() } else { self.sound_end_samples.as_ref() };
        let preset = if start { &self.sound_start_preset } else { &self.sound_end_preset };
        let (base, channels, rate) = if *preset == "custom" {
            let Some((pcm, channels, rate)) = cached else {
                let path = if start { &self.sound_start_path } else { &self.sound_end_path };
                anyhow::bail!("Timer custom sound is not ready: {}", path.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| "no file selected".into()));
            };
            (pcm.as_ref().clone(), *channels, *rate)
        } else {
            let Some((pcm, channels, rate)) = builtin_sound(preset) else { anyhow::bail!("Unknown built-in timer sound: {preset}") };
            (pcm.as_ref().clone(), *channels, *rate)
        };
        let volume = self.sound_volume.clamp(0.0, 1.0);
        let fade = ((rate as usize * 5) / 1000).max(1);
        let mut samples = base;
        if channels == 1 {
            let mono = samples;
            samples = mono.into_iter().flat_map(|sample| [sample, sample]).collect();
        }
        let frames = samples.len() / 2;
        for (frame, pair) in samples.chunks_exact_mut(2).enumerate() {
            let edge = (frame.min(frames.saturating_sub(1).saturating_sub(frame)) as f32 / fade as f32).clamp(0.0, 1.0);
            pair[0] *= volume * edge;
            pair[1] *= volume * edge;
        }
        let mut voice = Voice::new(Arc::new(samples), 2, rate, 1.0, 0.0);
        let mut device = None;
        if let Some(patch) = context.resolve_patch_checked(self.sound_patch_id)? {
            if let Some(&ch) = patch.channels.first() { voice.out_l = ch as usize; }
            voice.out_r = patch.channels.get(1).copied().or_else(|| patch.channels.first().copied()).unwrap_or(1) as usize;
            voice.patched = !patch.is_main();
            voice.patch_id = Some(patch.id);
            voice.patch_slot = context.output_patches.iter().position(|p| p.id == patch.id).map(|i| i as u8);
            voice.inner.set_patch_gain(db_to_linear(patch.gain_db as f64) as f32);
            if !patch.is_main() && !patch.device_id.is_empty() { device = Some(patch.device_id.clone()); }
        }
        self.active_sound_voice = Some(context.audio_engine.play_voice_routed(voice, device.as_deref())?);
        Ok(())
    }

    fn stop_sound(&mut self, context: &CueContext) {
        if let Some(id) = self.active_sound_voice.take() { let _ = context.audio_engine.stop_voice(id, 5, FadeCurve::Linear); }
    }

    fn load_custom_sounds(&mut self, context: &CueContext, validate_enabled: bool) -> Result<()> {
        self.sound_start_samples = load_timer_custom_sound(self.sound_start, &self.sound_start_preset, self.sound_start_path.as_deref(), "start", validate_enabled)?;
        self.sound_end_samples = load_timer_custom_sound(self.sound_end, &self.sound_end_preset, self.sound_end_path.as_deref(), "end", validate_enabled)?;
        for (enabled, preset, path, samples, edge) in [
            (self.sound_start, &self.sound_start_preset, &self.sound_start_path, &self.sound_start_samples, "start"),
            (self.sound_end, &self.sound_end_preset, &self.sound_end_path, &self.sound_end_samples, "end"),
        ] {
            if validate_enabled && enabled && preset == "custom" && samples.is_none() {
                anyhow::bail!("Timer {edge} sound is custom but no decodable file is selected: {}", path.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| "no file selected".into()));
            }
        }
        if validate_enabled && (self.sound_start || self.sound_end) { context.resolve_patch_checked(self.sound_patch_id)?; }
        Ok(())
    }

    fn start(&mut self, context: &CueContext) -> Result<()> {
        self.clear(context);
        self.elapsed_before_pause = Duration::ZERO;
        self.started_at = Some(Instant::now());
        self.action_elapsed_before_pause = Duration::ZERO;
        self.action_started_at = None;
        self.in_pre_wait = !self.pre_wait.is_zero();
        self.latched_output_id = self.output_id.clone();
        self.latched_output_ids = self.output_ids.clone();
        self.latched_screen_index = self.screen_index;
        self.generation = self.generation.wrapping_add(1);
        self.start_sound_fired = false;
        self.end_sound_fired = false;
        self.auto_continue_fired = false;
        self.zero_reached = false;
        self.zero_duration = None;
        self.state = CueState::Running;
        if !self.in_pre_wait { self.start_action(context)?; }
        Ok(())
    }

    fn start_action(&mut self, context: &CueContext) -> Result<()> {
        self.in_pre_wait = false;
        self.action_elapsed_before_pause = Duration::ZERO;
        self.action_started_at = Some(Instant::now());
        context.emit(CueEvent::ActionStarted { cue_id: self.id });
        if !self.start_sound_fired {
            if self.sound_start { self.play_sound(context, true)?; }
            self.start_sound_fired = true;
        }
        self.render(context, true);
        Ok(())
    }
}

impl Default for TimerCue { fn default() -> Self { Self::new() } }

fn build_ass(text: &str, font: &str, size: u32, color: &str) -> String {
    let hex = color.trim_start_matches('#');
    let (r,g,b) = if hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        (u8::from_str_radix(&hex[0..2],16).unwrap_or(255), u8::from_str_radix(&hex[2..4],16).unwrap_or(255), u8::from_str_radix(&hex[4..6],16).unwrap_or(255))
    } else { (255,255,255) };
    let font = font.replace(['{','}','\\'], "");
    format!("{{\\an5\\fn{font}\\fs{size}\\c&H00{b:02X}{g:02X}{r:02X}&\\bord2\\shad1}}{text}")
}

fn decode_embedded_wav(bytes: &[u8]) -> Option<(Arc<Vec<f32>>, u16, u32)> {
    if bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" { return None; }
    let (mut format, mut channels, mut rate, mut bits, mut data) = (0u16, 0u16, 0u32, 0u16, None);
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let id = bytes.get(offset..offset + 4)?;
        let len = u32::from_le_bytes(bytes.get(offset + 4..offset + 8)?.try_into().ok()?) as usize;
        let start = offset + 8;
        let end = start.checked_add(len)?.min(bytes.len());
        let chunk = bytes.get(start..end)?;
        if id == b"fmt " && chunk.len() >= 16 {
            format = u16::from_le_bytes(chunk[0..2].try_into().ok()?);
            channels = u16::from_le_bytes(chunk[2..4].try_into().ok()?);
            rate = u32::from_le_bytes(chunk[4..8].try_into().ok()?);
            bits = u16::from_le_bytes(chunk[14..16].try_into().ok()?);
        } else if id == b"data" { data = Some(chunk); }
        offset = start.checked_add(len + (len & 1))?;
    }
    if format != 1 || bits != 16 || channels == 0 || rate == 0 { return None; }
    let bytes = data?;
    let samples = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / i16::MAX as f32).collect();
    Some((Arc::new(samples), channels, rate))
}

fn builtin_sound(id: &str) -> Option<&'static (Arc<Vec<f32>>, u16, u32)> {
    if !matches!(id, "notification_1" | "notification_2" | "notification_3" | "notification_4" | "notification_5") { return None; }
    static SOUND: std::sync::OnceLock<Option<(Arc<Vec<f32>>, u16, u32)>> = std::sync::OnceLock::new();
    SOUND.get_or_init(|| decode_embedded_wav(include_bytes!("../../assets/timer-sounds/notification_1.wav"))).as_ref()
}

fn load_timer_custom_sound(
    enabled: bool,
    preset: &str,
    path: Option<&Path>,
    edge: &str,
    validate_enabled: bool,
) -> Result<Option<(Arc<Vec<f32>>, u16, u32)>> {
    if !enabled || preset != "custom" { return Ok(None); }
    let Some(path) = path else {
        if validate_enabled { anyhow::bail!("Timer {edge} sound is custom but no file is selected"); }
        return Ok(None);
    };
    match load_custom_sound(Some(path)) {
        Ok(samples) => Ok(samples),
        Err(error) if validate_enabled => Err(error),
        Err(error) => { log::warn!("Timer {edge} sound is not ready yet: {error:#}"); Ok(None) }
    }
}

fn load_custom_sound(path: Option<&Path>) -> Result<Option<(Arc<Vec<f32>>, u16, u32)>> {
    let Some(path) = path else { return Ok(None) };
    type Decoded = (Arc<Vec<f32>>, u16, u32);
    type CacheKey = (PathBuf, u64, std::time::SystemTime);
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<CacheKey, Decoded>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let metadata = std::fs::metadata(&key).map_err(|error| anyhow::anyhow!("Timer sound file is unavailable '{}': {error}", key.display()))?;
    let cache_key = (key.clone(), metadata.len(), metadata.modified().unwrap_or(std::time::UNIX_EPOCH));
    if let Ok(cache) = cache.lock() {
        if let Some(cached) = cache.get(&cache_key) { return Ok(Some(cached.clone())); }
    };
    let decoded = match crate::cue::media_decode::decode_audio_track_legacy(path) {
        Ok(Some((samples, channels, rate))) if channels == 1 || channels == 2 => (Arc::new(samples), channels, rate),
        Ok(Some((_, channels, _))) => anyhow::bail!("Timer sound must be mono or stereo; file has {channels} channels: {}", key.display()),
        Ok(None) => anyhow::bail!("Timer sound file has no audio track: {}", key.display()),
        Err(error) => anyhow::bail!("Timer sound file could not be decoded '{}': {error}", key.display()),
    };
    const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
    let byte_len = decoded.0.len().saturating_mul(std::mem::size_of::<f32>());
    if byte_len <= MAX_CACHE_BYTES {
        if let Ok(mut cache) = cache.lock() {
            cache.retain(|(cached_path, _, _), _| cached_path != &key);
            let cached_bytes: usize = cache.values().map(|(samples, _, _)| samples.len() * std::mem::size_of::<f32>()).sum();
            if cached_bytes.saturating_add(byte_len) > MAX_CACHE_BYTES { cache.clear(); }
            cache.insert(cache_key, decoded.clone());
        }
    }
    Ok(Some(decoded))
}

impl Cue for TimerCue {
    fn id(&self) -> CueId { self.id }
    fn cue_type(&self) -> CueType { CueType::Timer }
    fn name(&self) -> &str { &self.name }
    fn set_name(&mut self, n: String) { self.name = n; }
    fn number(&self) -> Option<&str> { self.number.as_deref() }
    fn set_number(&mut self, n: Option<String>) { self.number = n; }
    fn notes(&self) -> &str { &self.notes }
    fn set_notes(&mut self, n: String) { self.notes = n; }
    fn color(&self) -> CueColor { self.color }
    fn set_color(&mut self, c: CueColor) { self.color = c; }
    fn is_disabled(&self) -> bool { self.disabled }
    fn set_disabled(&mut self, v: bool) { self.disabled = v; }
    fn state(&self) -> CueState { self.state }
    fn load(&mut self, context: &CueContext) -> Result<()> { self.load_custom_sounds(context, false) }
    fn go(&mut self, context: &CueContext) -> Result<()> { self.load_custom_sounds(context, true)?; self.stop_sound(context); self.start(context) }
    fn stop(&mut self, context: &CueContext) -> Result<()> {
        self.stop_sound(context); self.clear(context); self.started_at = None; self.action_started_at = None;
        self.elapsed_before_pause = Duration::ZERO; self.action_elapsed_before_pause = Duration::ZERO; self.in_pre_wait = false;
        self.auto_continue_fired = false; self.zero_reached = false; self.zero_duration = None;
        self.state = CueState::Standby; context.emit(CueEvent::Stopped { cue_id: self.id }); Ok(())
    }
    fn pause(&mut self, _context: &CueContext) -> Result<()> {
        if self.state == CueState::Running {
            self.elapsed_before_pause = self.elapsed_now(); self.started_at = None;
            self.action_elapsed_before_pause = self.action_elapsed_now(); self.action_started_at = None;
            self.state = CueState::Paused;
        }
        Ok(())
    }
    fn resume(&mut self, context: &CueContext) -> Result<()> {
        if self.state == CueState::Paused {
            let now = Instant::now();
            self.started_at = Some(now);
            if !self.in_pre_wait { self.action_started_at = Some(now); }
            self.state = CueState::Running;
            if !self.in_pre_wait { self.render(context, true); }
        }
        Ok(())
    }
    fn hard_stop(&mut self, context: &CueContext) -> Result<()> { self.stop(context) }
    fn reset(&mut self) -> Result<()> { self.started_at = None; self.action_started_at = None; self.elapsed_before_pause = Duration::ZERO; self.action_elapsed_before_pause = Duration::ZERO; self.in_pre_wait = false; self.state = CueState::Standby; self.start_sound_fired = false; self.end_sound_fired = false; self.zero_reached = false; self.zero_duration = None; self.auto_continue_fired = false; Ok(()) }
    fn tick(&mut self, context: &CueContext) -> Result<()> {
        if self.state != CueState::Running { return Ok(()); }
        if self.zero_reached && self.finish_and_hide { return Ok(()); }
        if self.in_pre_wait && self.elapsed_now() >= self.pre_wait { self.start_action(context)?; }
        if self.in_pre_wait { return Ok(()); }
        let elapsed = self.action_elapsed_now();
        if self.expired(elapsed) && !self.zero_reached {
            self.zero_reached = true;
            self.zero_duration = Some(self.duration);
            self.render(context, false);
            self.end_sound_fired = true;
            if self.sound_end {
                if let Err(error) = self.play_sound(context, false) { log::error!("Timer end signal failed: {error:#}"); }
            }
            if self.finish_and_hide {
                self.clear(context);
            }
            context.emit(CueEvent::ActionCompleted { cue_id: self.id });
        } else { self.render(context, false); }
        Ok(())
    }
    fn pre_wait(&self) -> Duration { self.pre_wait }
    fn is_action_started(&self) -> bool { !self.in_pre_wait }
    fn set_pre_wait(&mut self, d: Duration) { self.pre_wait = d; }
    fn post_wait(&self) -> Duration { self.post_wait }
    fn set_post_wait(&mut self, d: Duration) { self.post_wait = d; }
    fn duration(&self) -> Option<Duration> { Some(self.duration) }
    fn visual_geometry(&self) -> Option<crate::engine::output_engine::VideoGeometry> { Some(self.geometry) }
    fn visual_geometry_by_output(&self) -> Option<HashMap<String, crate::engine::output_engine::VideoGeometry>> {
        Some(self.geometry_by_output.clone())
    }
    fn apply_live_visual_patch(&mut self, patch: crate::cue::traits::LiveVisualPatch) {
        if let Some(geometry) = patch.geometry { self.geometry = geometry; }
        if let Some(overrides) = patch.geometry_by_output { self.geometry_by_output = overrides; }
    }
    fn refresh_live_visual(&mut self, context: &CueContext) {
        if matches!(self.state, CueState::Running | CueState::Paused) && !self.in_pre_wait {
            self.render(context, true);
        }
    }
    fn automatic_completion_duration(&self) -> Option<Duration> {
        if self.in_pre_wait { return None; }
        self.finish_and_hide.then_some(if self.zero_reached { Duration::ZERO } else { self.duration })
    }
    fn set_user_action_duration(&mut self, d: Option<Duration>) -> Result<()> { self.duration = d.unwrap_or(Duration::ZERO); Ok(()) }
    fn elapsed(&self) -> Duration { if self.state == CueState::Running || self.state == CueState::Paused { self.elapsed_now() } else { Duration::ZERO } }
    fn action_elapsed(&self) -> Duration { if self.state == CueState::Running || self.state == CueState::Paused { self.action_elapsed_now() } else { Duration::ZERO } }
    fn continue_mode(&self) -> ContinueMode { self.continue_mode }
    fn set_continue_mode(&mut self, mode: ContinueMode) { self.continue_mode = mode; }
    fn is_visual(&self) -> bool { true }
    fn stop_on_next_go(&self) -> bool { false }
    fn play_generation(&self) -> u64 { self.generation }
    fn has_retained_output(&self) -> bool { self.active_sound_voice.is_some() }
    fn all_voice_ids(&self) -> Vec<VoiceId> { self.active_sound_voice.into_iter().collect() }
    fn auto_continue_elapsed(&self) -> Duration { self.action_elapsed_now() }
    fn is_auto_continue_fired(&self) -> bool { self.auto_continue_fired }
    fn auto_continue_marker(&self) -> Option<bool> { Some(self.auto_continue_fired) }
    fn mark_auto_continue_fired(&mut self) { self.auto_continue_fired = true; }
    fn clear_auto_continue_fired(&mut self) { self.auto_continue_fired = false; }
    fn runtime_state(&self) -> RuntimeState { RuntimeState { state: self.state, voice_id: self.active_sound_voice, started_at: self.started_at, action_started_at: self.action_started_at } }
    fn restore_runtime_state(&mut self, snap: RuntimeState) { self.state = snap.state; self.active_sound_voice = snap.voice_id; self.started_at = snap.started_at; self.action_started_at = snap.action_started_at; }
    fn runtime_extension(&self) -> Option<Value> { Some(json!({
        "elapsed_ms": self.elapsed_before_pause.as_millis() as u64,
        "started_at_ms": self.started_at.map(|t| t.elapsed().as_millis() as u64),
        "action_elapsed_ms": self.action_elapsed_before_pause.as_millis() as u64,
        "action_started_at_ms": self.action_started_at.map(|t| t.elapsed().as_millis() as u64),
        "in_pre_wait": self.in_pre_wait,
        "zero_reached": self.zero_reached,
        "zero_duration_ms": self.zero_duration.map(|d| d.as_millis() as u64),
        "generation": self.generation, "start_sound_fired": self.start_sound_fired, "end_sound_fired": self.end_sound_fired,
        "auto_continue_fired": self.auto_continue_fired,
        "output_id": self.latched_output_id, "output_ids": self.latched_output_ids, "screen_index": self.latched_screen_index,
        "last_rendered": self.last_rendered,
    })) }
    fn restore_runtime_extension(&mut self, value: Value) {
        self.elapsed_before_pause = Duration::from_millis(value["elapsed_ms"].as_u64().unwrap_or(0));
        if let Some(ms) = value["started_at_ms"].as_u64() { self.elapsed_before_pause += Duration::from_millis(ms); self.started_at = Some(Instant::now()); }
        self.action_elapsed_before_pause = Duration::from_millis(value["action_elapsed_ms"].as_u64().unwrap_or(0));
        if let Some(ms) = value["action_started_at_ms"].as_u64() { self.action_elapsed_before_pause += Duration::from_millis(ms); self.action_started_at = Some(Instant::now()); }
        self.in_pre_wait = value["in_pre_wait"].as_bool().unwrap_or(false);
        self.zero_reached = value["zero_reached"].as_bool().unwrap_or(false);
        self.zero_duration = value["zero_duration_ms"].as_u64().map(Duration::from_millis);
        self.generation = value["generation"].as_u64().unwrap_or(0);
        self.start_sound_fired = value["start_sound_fired"].as_bool().unwrap_or(false);
        self.end_sound_fired = value["end_sound_fired"].as_bool().unwrap_or(false);
        self.auto_continue_fired = value["auto_continue_fired"].as_bool().unwrap_or(false);
        self.latched_output_id = value["output_id"].as_str().map(str::to_owned);
        self.latched_output_ids = serde_json::from_value(value["output_ids"].clone()).unwrap_or_default();
        self.latched_screen_index = value["screen_index"].as_u64().map(|n| n as u32);
        self.last_rendered = None;
    }
    fn refresh_runtime_after_rebuild(&mut self, context: &CueContext) -> Result<()> {
        self.load_custom_sounds(context, false)?;
        if self.zero_reached && self.finish_and_hide { self.clear(context); }
        else if matches!(self.state, CueState::Running | CueState::Paused) && !self.in_pre_wait { self.render(context, true); }
        Ok(())
    }
    fn media_file_path(&self) -> Option<&Path> { self.sound_start_path.as_deref().or(self.sound_end_path.as_deref()) }
    fn additional_media_file_paths(&self) -> Vec<&Path> {
        match (self.sound_start_path.as_deref(), self.sound_end_path.as_deref()) {
            (Some(start), Some(end)) if start != end => vec![end],
            _ => Vec::new(),
        }
    }
    fn serialize(&self) -> Value { json!({
        "type":"timer", "cue_type":"timer", "id":self.id, "name":self.name, "number":self.number,
        "notes":self.notes, "color":self.color, "is_disabled":self.disabled,
        "pre_wait_ms":self.pre_wait.as_millis() as u64, "post_wait_ms":self.post_wait.as_millis() as u64,
        "continue_mode":self.continue_mode, "timer_duration_ms":self.duration.as_millis() as u64,
        "mode":self.mode, "format":self.format, "font":self.font, "font_size":self.font_size,
        "geometry":self.geometry, "geometry_by_output":self.geometry_by_output,
        "text_color":self.text_color, "output_id":self.output_id, "output_ids":self.output_ids,
        "screen_index":self.screen_index, "sound_start":self.sound_start, "sound_end":self.sound_end,
        "sound_start_preset":self.sound_start_preset, "sound_end_preset":self.sound_end_preset,
        "sound_start_path":self.sound_start_path, "sound_end_path":self.sound_end_path,
        "sound_volume":self.sound_volume, "sound_patch_id":self.sound_patch_id,
        "finish_and_hide":self.finish_and_hide,
    }) }
}

pub struct TimerCueFactory;
impl CueFactory for TimerCueFactory {
    fn create(&self) -> Box<dyn Cue> { Box::new(TimerCue::new()) }
    fn from_json(&self, v: Value) -> Result<Box<dyn Cue>> {
        let mut c = TimerCue::new();
        if let Some(s)=v["id"].as_str(){ c.id=s.parse().unwrap_or_else(|_|Uuid::new_v4()); }
        if let Some(s)=v["name"].as_str(){ c.name=s.into(); }
        if let Some(s)=v["number"].as_str(){ c.number=Some(s.into()); }
        if let Some(s)=v["notes"].as_str(){ c.notes=s.into(); }
        if let Some(n)=v["timer_duration_ms"].as_u64().or_else(|| v["duration_ms"].as_u64()){ c.duration=Duration::from_millis(n); }
        if let Ok(x)=serde_json::from_value(v["mode"].clone()){ c.mode=x; }
        if let Ok(x)=serde_json::from_value(v["format"].clone()){ c.format=x; }
        if let Some(s)=v["font"].as_str(){ c.font=s.into(); }
        if let Some(n)=v["font_size"].as_u64(){ c.font_size=n.clamp(8,300) as u32; }
        if let Some(s)=v["text_color"].as_str(){ c.text_color=s.into(); }
        if let Ok(geometry)=serde_json::from_value(v["geometry"].clone()){ c.geometry=geometry; }
        if let Ok(overrides)=serde_json::from_value(v["geometry_by_output"].clone()){ c.geometry_by_output=overrides; }
        if let Some(s)=v["output_id"].as_str(){ c.output_id=Some(s.into()); }
        c.output_ids=serde_json::from_value(v["output_ids"].clone()).unwrap_or_default();
        c.screen_index=v["screen_index"].as_u64().map(|n|n as u32);
        c.sound_start=v["sound_start"].as_bool().unwrap_or(false); c.sound_end=v["sound_end"].as_bool().unwrap_or(false);
        let normalize_preset = |value: Option<&str>| match value.unwrap_or("notification_1") {
            "notification_1" | "custom" => value.unwrap_or("notification_1").to_string(),
            "notification_2" | "notification_3" | "notification_4" | "notification_5" => "notification_1".into(),
            _ => "notification_1".into(),
        };
        c.sound_start_preset=normalize_preset(v["sound_start_preset"].as_str());
        c.sound_end_preset=normalize_preset(v["sound_end_preset"].as_str());
        c.sound_start_path=v["sound_start_path"].as_str().map(PathBuf::from);
        c.sound_end_path=v["sound_end_path"].as_str().map(PathBuf::from);
        c.sound_volume=v["sound_volume"].as_f64().unwrap_or(1.0).clamp(0.0,1.0) as f32;
        c.finish_and_hide=v["finish_and_hide"].as_bool().unwrap_or(false);
        c.sound_patch_id=v["sound_patch_id"].as_str().and_then(|s|s.parse().ok());
        if let Ok(x)=serde_json::from_value(v["color"].clone()){c.color=x;}
        if let Ok(x)=serde_json::from_value(v["continue_mode"].clone()){c.continue_mode=x;}
        c.pre_wait=Duration::from_millis(v["pre_wait_ms"].as_u64().unwrap_or(0)); c.post_wait=Duration::from_millis(v["post_wait_ms"].as_u64().unwrap_or(0));
        c.disabled=v["is_disabled"].as_bool().unwrap_or(false);
        Ok(Box::new(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cue::{context::{CueContext, CueEvent}},
        engine::{dmx_engine::DmxEngine, engine_traits::OutputEngineApi, output_engine::ContentRequest, ring_command::VoiceId, AudioEngine},
        preferences::MachineAudioConfig,
    };
    use crossbeam_channel::{unbounded, Receiver};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NullOutput { timer_geometry_redraws: Arc<AtomicUsize> }
    impl OutputEngineApi for NullOutput {
        fn show_content(&self, _req: ContentRequest<'_>) -> Result<VoiceId> { anyhow::bail!("unused in timer tests") }
        fn stop_content(&self, _voice_id: VoiceId, _visual_fade_ms: u32, _audio_fade_ms: u32) {}
        fn hard_stop_current(&self) {}
        fn panic_stop(&self) {}
        fn video_audio_voice(&self, _voice_id: VoiceId) -> Option<VoiceId> { None }
        fn resync_audio_to_video(&self, _voice_id: VoiceId) {}
        fn get_voice_opacity(&self, _voice_id: VoiceId) -> f32 { 1.0 }
        fn set_voice_opacity(&self, _voice_id: VoiceId, _opacity: f32) {}
        fn stop_voice(&self, _voice_id: VoiceId, _fade_ms: u32) -> Result<()> { Ok(()) }
        fn pause_voice(&self, _voice_id: VoiceId) -> Result<()> { Ok(()) }
        fn resume_voice(&self, _voice_id: VoiceId) -> Result<()> { Ok(()) }
        fn seek_voice_ms(&self, _voice_id: VoiceId, _position_ms: u64) {}
        fn show_text_overlay(&self, _ass_text: &str, _screen_index: Option<u32>) {}
        fn show_timer_overlay_with_geometry_for_outputs(
            &self, _owner: Uuid, _ass_text: &str, _output_id: Option<&str>, _output_ids: &[String],
            _screen_index: Option<u32>, _geometry: &crate::engine::output_engine::VideoGeometry,
            _geometry_by_output: &HashMap<String, crate::engine::output_engine::VideoGeometry>,
        ) {
            self.timer_geometry_redraws.fetch_add(1, Ordering::Relaxed);
        }
        fn clear_text_overlay(&self) {}
        fn begin_eof_fade_out(&self, _voice_id: VoiceId, _fade_ms: u32) -> bool { false }
        fn devamp_voice(&self, _voice_id: VoiceId, _stop_at_end: bool) {}
        fn start_preloaded(&self, _voice_id: VoiceId) -> bool { false }
    }

    fn test_context() -> (CueContext, Receiver<CueEvent>) {
        let (context, receiver, _) = test_context_with_redraw_counter();
        (context, receiver)
    }

    fn test_context_with_redraw_counter() -> (CueContext, Receiver<CueEvent>, Arc<AtomicUsize>) {
        let (events, receiver) = unbounded();
        let audio = AudioEngine::new_silent(&MachineAudioConfig::default());
        let redraws = Arc::new(AtomicUsize::new(0));
        (CueContext::new(
            audio, Arc::new(NullOutput { timer_geometry_redraws: Arc::clone(&redraws) }), events, 0, Vec::new(), None, None, Vec::new(),
            Arc::new(DmxEngine::new()), Vec::new(), Vec::new(), Vec::new(), 256,
        ), receiver, redraws)
    }

    #[test]
    fn countdown_format_uses_ceiling_until_zero_and_countup_uses_floor() {
        let cue = TimerCue::new();
        let cases = [
            (Duration::ZERO, "01:00"),
            (Duration::from_millis(1), "01:00"),
            (Duration::from_millis(999), "01:00"),
            (Duration::from_secs(1), "00:59"),
            (Duration::from_millis(59_999), "00:01"),
            (Duration::from_secs(60), "00:00"),
        ];
        for (elapsed, expected) in cases { assert_eq!(cue.display(elapsed), expected); }
        let mut countup = TimerCue::new();
        countup.mode = TimerMode::Countup;
        assert_eq!(countup.display(Duration::from_millis(999)), "00:00");
        assert_eq!(countup.display(Duration::from_secs(1)), "00:01");
    }

    #[test]
    fn defaults_roundtrip_and_legacy_json_are_stable() {
        let cue = TimerCue::new();
        assert_eq!(cue.duration, Duration::from_secs(60));
        assert!(!cue.finish_and_hide);
        assert!(!cue.sound_start && !cue.sound_end);
        assert_eq!(cue.sound_volume, 1.0);
        let factory = TimerCueFactory;
        let rebuilt = factory.from_json(cue.serialize()).unwrap();
        assert_eq!(rebuilt.cue_type(), CueType::Timer);
        assert_eq!(rebuilt.duration(), Some(Duration::from_secs(60)));
        assert_eq!(rebuilt.serialize()["format"], "minutes_seconds");
        let legacy = factory.from_json(json!({ "type": "timer", "name": "Old timer" })).unwrap();
        assert_eq!(legacy.name(), "Old timer");
        assert_eq!(legacy.duration(), Some(Duration::from_secs(60)));

        let mut geometry = TimerCue::new();
        geometry.geometry = crate::engine::output_engine::VideoGeometry { pan_y: 0.2, scale: 1.1, ..Default::default() };
        geometry.geometry_by_output.insert(
            "ndi-program".into(),
            crate::engine::output_engine::VideoGeometry { pan_x: -0.2, ..Default::default() },
        );
        let restored = factory.from_json(geometry.serialize()).unwrap();
        assert_eq!(restored.serialize()["geometry"]["pan_y"], 0.2);
        assert_eq!(restored.serialize()["geometry_by_output"]["ndi-program"]["pan_x"], -0.2);
    }

    #[test]
    fn malformed_non_ascii_color_falls_back_without_panicking() {
        let ass = build_ass("00:10", "Arial", 72, "#éééééé");
        assert!(ass.contains("\\c&H00FFFFFF&"));
    }

    #[test]
    fn hold_at_zero_fires_completion_and_end_signal_once_but_remains_running() {
        let (context, events) = test_context();
        let mut cue = TimerCue::new();
        cue.duration = Duration::from_millis(10);
        cue.sound_end = true;
        cue.go(&context).unwrap();
        cue.action_started_at = None;
        cue.action_elapsed_before_pause = cue.duration;
        cue.tick(&context).unwrap();
        assert_eq!(cue.state, CueState::Running);
        assert_eq!(cue.display(Duration::from_secs(60)), "00:00");
        assert!(cue.end_sound_fired);
        let signal = cue.active_sound_voice;
        assert!(signal.is_some());
        assert!(matches!(events.try_iter().last(), Some(CueEvent::ActionCompleted { .. })));
        cue.tick(&context).unwrap();
        assert_eq!(cue.active_sound_voice, signal);
        assert!(events.try_iter().next().is_none());
    }

    #[test]
    fn pause_resume_and_restart_keep_clock_state_without_repeating_start_sound() {
        let (context, _events) = test_context();
        let mut cue = TimerCue::new();
        cue.sound_start = true;
        cue.go(&context).unwrap();
        let voice = cue.active_sound_voice;
        cue.action_elapsed_before_pause = Duration::from_secs(12);
        cue.pause(&context).unwrap();
        let paused_elapsed = cue.action_elapsed();
        cue.resume(&context).unwrap();
        assert!(cue.action_elapsed() >= paused_elapsed);
        assert_eq!(cue.active_sound_voice, voice);
        cue.duration = Duration::from_secs(20);
        cue.go(&context).unwrap();
        assert!(cue.action_elapsed() < Duration::from_secs(1));
        assert_eq!(cue.generation, 2);
        assert!(cue.start_sound_fired);
    }

    #[test]
    fn pre_wait_pause_does_not_start_action_or_signal_early() {
        let (context, events) = test_context();
        let mut cue = TimerCue::new();
        cue.set_pre_wait(Duration::from_secs(2));
        cue.sound_start = true;
        cue.go(&context).unwrap();
        assert!(cue.in_pre_wait);
        assert!(!cue.start_sound_fired);
        cue.elapsed_before_pause = Duration::from_secs(1);
        cue.pause(&context).unwrap();
        cue.resume(&context).unwrap();
        assert!(cue.in_pre_wait);
        assert!(!cue.start_sound_fired);
        cue.started_at = None;
        cue.elapsed_before_pause = Duration::from_secs(2);
        cue.tick(&context).unwrap();
        assert!(!cue.in_pre_wait);
        assert!(cue.start_sound_fired);
        assert!(events.try_iter().any(|event| matches!(event, CueEvent::ActionStarted { .. })));
    }

    #[test]
    fn zero_duration_does_not_complete_before_pre_wait_and_post_wait_clock_keeps_running() {
        let (context, _events) = test_context();
        let mut cue = TimerCue::new();
        cue.duration = Duration::ZERO;
        cue.finish_and_hide = true;
        cue.set_pre_wait(Duration::from_secs(3));
        cue.go(&context).unwrap();
        assert_eq!(cue.automatic_completion_duration(), None);
        cue.in_pre_wait = false;
        cue.action_started_at = None;
        cue.action_elapsed_before_pause = Duration::ZERO;
        cue.tick(&context).unwrap();
        assert!(cue.zero_reached);
        assert_eq!(cue.automatic_completion_duration(), Some(Duration::ZERO));

        let mut held = TimerCue::new();
        held.duration = Duration::from_secs(1);
        held.zero_reached = true;
        held.action_elapsed_before_pause = Duration::from_secs(5);
        held.action_started_at = None;
        assert_eq!(held.auto_continue_elapsed(), Duration::from_secs(5));
        assert_eq!(held.display(held.action_elapsed()), "00:00");
    }

    #[test]
    fn paused_runtime_survives_rebuild_without_overwriting_new_authoring_settings() {
        let mut cue = TimerCue::new();
        cue.state = CueState::Paused;
        cue.elapsed_before_pause = Duration::from_secs(12);
        cue.action_elapsed_before_pause = Duration::from_secs(9);
        cue.start_sound_fired = true;
        cue.end_sound_fired = false;
        let runtime = cue.runtime_state();
        let extension = cue.runtime_extension().unwrap();
        let rebuilt = TimerCueFactory.from_json(cue.serialize()).unwrap();
        let mut rebuilt = rebuilt;
        let mut edited_json = rebuilt.serialize();
        edited_json["finish_and_hide"] = Value::Bool(true);
        rebuilt = TimerCueFactory.from_json(edited_json).unwrap();
        rebuilt.restore_runtime_state(runtime);
        rebuilt.restore_runtime_extension(extension);
        assert_eq!(rebuilt.state(), CueState::Paused);
        assert_eq!(rebuilt.elapsed(), Duration::from_secs(12));
        assert_eq!(rebuilt.action_elapsed(), Duration::from_secs(9));
        assert!(rebuilt.serialize()["finish_and_hide"].as_bool().unwrap());
    }

    #[test]
    fn live_geometry_redraw_keeps_paused_zero_cycle_and_end_signal_latches() {
        let (context, _, redraws) = test_context_with_redraw_counter();
        let mut cue = TimerCue::new();
        cue.state = CueState::Paused;
        cue.zero_reached = true;
        cue.zero_duration = Some(cue.duration);
        cue.end_sound_fired = true;
        cue.generation = 7;
        cue.action_elapsed_before_pause = Duration::from_secs(60);
        cue.active_sound_voice = Some(Uuid::new_v4());
        let before_voice = cue.active_sound_voice;
        let before_elapsed = cue.action_elapsed();
        let before_redraws = redraws.load(Ordering::Relaxed);

        cue.apply_live_visual_patch(crate::cue::traits::LiveVisualPatch {
            geometry: Some(crate::engine::output_engine::VideoGeometry { pan_x: 0.2, ..Default::default() }),
            ..Default::default()
        });
        cue.refresh_live_visual(&context);

        assert_eq!(redraws.load(Ordering::Relaxed), before_redraws + 1);
        assert_eq!(cue.state, CueState::Paused);
        assert_eq!(cue.action_elapsed(), before_elapsed);
        assert!(cue.zero_reached);
        assert!(cue.end_sound_fired);
        assert_eq!(cue.generation, 7);
        assert_eq!(cue.active_sound_voice, before_voice);
        assert_eq!(cue.geometry.pan_x, 0.2);
    }

    #[test]
    fn natural_completion_keeps_end_signal_until_explicit_stop() {
        let (context, _events) = test_context();
        let mut cue = TimerCue::new();
        cue.duration = Duration::ZERO;
        cue.finish_and_hide = true;
        cue.sound_end = true;
        cue.go(&context).unwrap();
        cue.tick(&context).unwrap();
        let signal = cue.active_sound_voice.expect("end signal should start");
        cue.complete_naturally().unwrap();
        assert_eq!(cue.state(), CueState::Standby);
        assert!(cue.has_retained_output());
        assert_eq!(cue.all_voice_ids(), vec![signal]);
        cue.stop(&context).unwrap();
        assert!(!cue.has_retained_output());
        assert!(!cue.zero_reached);
        assert_eq!(cue.zero_duration, None);
        assert_eq!(cue.elapsed(), Duration::ZERO);
        assert_eq!(cue.state(), CueState::Standby);
    }

    #[test]
    fn media_paths_include_two_signals_once_and_preserve_end_only_path() {
        let mut cue = TimerCue::new();
        let end = PathBuf::from("end.wav");
        cue.sound_end_path = Some(end.clone());
        assert_eq!(cue.media_file_path(), Some(end.as_path()));
        assert!(cue.additional_media_file_paths().is_empty());

        let start = PathBuf::from("start.wav");
        cue.sound_start_path = Some(start.clone());
        assert_eq!(cue.media_file_path(), Some(start.as_path()));
        assert_eq!(cue.additional_media_file_paths(), vec![end.as_path()]);
        cue.sound_end_path = Some(start);
        assert!(cue.additional_media_file_paths().is_empty());
    }

    #[test]
    fn hold_at_zero_stays_zero_after_duration_edit_until_restart() {
        let mut cue = TimerCue::new();
        cue.zero_reached = true;
        cue.zero_duration = Some(Duration::from_secs(60));
        cue.duration = Duration::from_secs(70);
        assert_eq!(cue.display(Duration::from_secs(60)), "00:00");
        assert_eq!(cue.automatic_completion_duration(), None);
        cue.finish_and_hide = true;
        assert_eq!(cue.automatic_completion_duration(), Some(Duration::ZERO));
    }
}
