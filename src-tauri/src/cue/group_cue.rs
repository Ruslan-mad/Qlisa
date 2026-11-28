//! [`GroupCue`] — contains and fires a list of child cues.
//!
//! ## Modes
//! - **Simultaneous**: all children fire at once; the Group completes when
//!   every child has finished.
//! - **Sequential**: children fire one after another using each child's own
//!   Continue Mode (Auto-Continue, Auto-Follow, Do Not Continue) exactly like
//!   a mini Cue List.

use std::{fmt::Display, time::{Duration, Instant}};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use uuid::Uuid;
use crate::engine::ring_command::FadeCurve as EngineFadeCurve;

use super::{
    context::{CueContext, CueEvent},
    registry::CueRegistry,
    traits::{Cue, CueFactory},
    types::{
        ContinueMode, CueColor, CueId, CueState, CueType, FadeSpec, GroupMode,
        NumberStartStopMode,
    },
};

// ---------------------------------------------------------------------------
// GroupCue
// ---------------------------------------------------------------------------

/// A cue that contains other cues and fires them simultaneously or sequentially.
pub struct GroupCue {
    // ── Identity ──────────────────────────────────────────────────────────
    pub id: CueId,
    name: String,
    number: Option<String>,
    notes: String,
    color: CueColor,

    // ── State ─────────────────────────────────────────────────────────────
    state: CueState,

    // ── Timing ────────────────────────────────────────────────────────────
    pre_wait: Duration,
    post_wait: Duration,
    started_at: Option<Instant>,
    action_started_at: Option<Instant>,
    elapsed_before_pause: Duration,
    action_elapsed_before_pause: Duration,
    in_pre_wait: bool,

    // ── Continue ──────────────────────────────────────────────────────────
    continue_mode: ContinueMode,
    auto_continue_fired: bool,
    execution_generation: u64,

    is_disabled: bool,

    // ── Group-specific ────────────────────────────────────────────────────
    pub mode: GroupMode,
    /// Direct child cues (any type, including nested Groups).
    pub children: Vec<Box<dyn Cue>>,
    /// True when this Group is the user-facing concert Number variant. Number
    /// children are scheduled from offsets relative to the master child.
    number_mode: bool,
    /// True when this Group is a prepared document shown as a page playlist.
    presentation_mode: bool,
    /// The child that owns the Number timeline and determines its duration.
    master_child_id: Option<CueId>,
    /// Action child ID → offset from the master action start (milliseconds).
    action_offsets_ms: std::collections::HashMap<CueId, u64>,
    /// Number-level envelopes shared by all active audio/video children.
    fade_in: Option<FadeSpec>,
    fade_out: Option<FadeSpec>,
    /// Repeat a Playlist's children or a Number's full timeline. Persisted.
    playlist_loop: bool,
    /// Number start/finish flow settings. Persisted only for Number cues.
    number_start_stop_mode: NumberStartStopMode,
    number_start_stop_ids: Vec<CueId>,
    number_finish_start_ids: Vec<CueId>,
    /// Protected from operational bulk/targeted stops; persisted for Numbers only.
    background_screensaver: bool,
    /// Targets queued when the Number reaches its final post-wait.
    number_completion_start_ids: Vec<CueId>,

    // ── Sequential / Playlist mode state (not persisted) ──────────────────
    /// ID of the child currently at the internal playhead in Sequential/Playlist.
    seq_current_id: Option<CueId>,
    /// Set when the sequential chain has finished (last child completed with
    /// DoNotContinue, or all children exhausted).
    seq_done: bool,
    /// When Some, we are waiting for this instant before firing the next child
    /// (AutoContinue post-wait).
    seq_post_wait_until: Option<PendingGroupAdvance>,
    /// Pause anchor used to freeze the group's pre-wait and sequential post-wait.
    paused_at: Option<Instant>,

    // ── StartRandom mode state (not persisted) ────────────────────────────
    /// Child indices not yet played this cycle; refilled + reshuffled when empty.
    random_bag: Vec<usize>,
    /// xorshift64* PRNG state; 0 = unseeded (seeded lazily on first draw).
    rng_state: u64,
    /// Index of the child StartRandom fired most recently (for is_complete).
    random_current_idx: Option<usize>,
    /// Successfully started child cue IDs waiting to be reported to transport.
    fired_child_ids: Vec<CueId>,
    /// Paused slide image retained beneath an AsSlide video until page change.
    presentation_retained_background_id: Option<CueId>,
    runtime_error: Option<String>,
    /// Runtime Number state. These values are intentionally not serialized.
    number_launched_actions: std::collections::HashSet<CueId>,
    number_master_finished: bool,
    /// Number remains Running during its authored Post-Wait so the operator
    /// can see the final timing segment before completion/continuation.
    number_post_wait_until: Option<Instant>,
    /// Stable visual ordering anchor for this Number run, or an inherited
    /// anchor when this is a nested Group action inside a Number.
    visual_group_order: Option<u64>,
    /// Runtime-only override used when this Group is a Number master.
    number_master_autoplay: bool,
    /// Runtime-only fade passed from a parent Number for the current start/stop.
    number_shared_fade: Option<FadeSpec>,
}

fn cue_tree_is_active(cue: &dyn Cue) -> bool {
    cue.is_running()
        || cue.is_paused()
        || cue.child_cues().is_some_and(|children| {
            children.iter().any(|child| cue_tree_is_active(child.as_ref()))
        })
}

fn number_action_runtime_error(name: &str, phase: &str, error: impl Display) -> String {
    format!("Number action '{name}' failed {phase}: {error}")
}

fn number_action_type_supported(cue_type: CueType) -> bool {
    matches!(cue_type, CueType::Audio | CueType::Video | CueType::Image | CueType::Group)
}

fn number_action_is_temporary_media(cue_type: CueType) -> bool {
    number_action_type_supported(cue_type)
}

fn number_seek_target(position_ms: u64, master_duration_ms: Option<u64>) -> u64 {
    master_duration_ms.map_or(position_ms, |duration| position_ms.min(duration))
}

/// A timeline seek repositions an already-running Number. It is not a new GO,
/// so the Number's shared fade-in must never be applied to the seeked master.
/// Keeping this decision explicit makes it harder for a future seek path to
/// accidentally reintroduce the fade envelope.
fn number_seek_shared_fade() -> Option<FadeSpec> {
    None
}

fn number_action_window_active(
    target_ms: u64,
    offset_ms: u64,
    duration_ms: Option<u64>,
    master_duration_ms: Option<u64>,
) -> bool {
    if offset_ms > target_ms || master_duration_ms.is_some_and(|end| target_ms >= end) {
        return false;
    }
    duration_ms.map_or(true, |duration| target_ms.saturating_sub(offset_ms) < duration)
}

fn number_seek_state(was_paused: bool) -> CueState {
    if was_paused { CueState::Paused } else { CueState::Running }
}

fn drain_fired_ids_tree(cue: &mut dyn Cue, collected: &mut Vec<CueId>) {
    collected.extend(cue.take_fired_cue_ids());
    if let Some(children) = cue.child_cues_mut() {
        for child in children {
            drain_fired_ids_tree(child.as_mut(), collected);
        }
    }
}

fn fade_spec_from_json(value: &Value, duration_key: &str, curve_key: &str) -> Option<FadeSpec> {
    let duration_ms = value.get(duration_key).and_then(Value::as_u64)?;
    let curve = value
        .get(curve_key)
        .and_then(|raw| serde_json::from_value(raw.clone()).ok())
        .unwrap_or_default();
    Some(FadeSpec { duration_ms, curve })
}

/// Leaf durations exclude their own waits; nested groups already include
/// their internal waits in `duration()`.
fn child_duration_with_waits(child: &dyn Cue) -> Option<Duration> {
    let duration = child.duration()?;
    if matches!(child.cue_type(), CueType::Group | CueType::Number | CueType::Presentation) {
        Some(duration)
    } else {
        Some(child.pre_wait().saturating_add(duration).saturating_add(child.post_wait()))
    }
}

#[derive(Clone, Copy, Debug)]
struct PendingGroupAdvance {
    due: Instant,
    source_id: CueId,
    source_generation: u64,
    expected_current_id: Option<CueId>,
    require_marker: bool,
    source_paused_at: Option<Instant>,
}

impl GroupCue {
    /// Create a new, empty Group with a fresh UUID.
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "Group".to_string(),
            number: None,
            notes: String::new(),
            color: CueColor::None,
            state: CueState::Standby,
            pre_wait: Duration::ZERO,
            post_wait: Duration::ZERO,
            started_at: None,
            action_started_at: None,
            elapsed_before_pause: Duration::ZERO,
            action_elapsed_before_pause: Duration::ZERO,
            in_pre_wait: false,
            continue_mode: ContinueMode::DoNotContinue,
            auto_continue_fired: false,
            execution_generation: 0,
            is_disabled: false,
            mode: GroupMode::Simultaneous,
            children: Vec::new(),
            number_mode: false,
            presentation_mode: false,
            master_child_id: None,
            action_offsets_ms: std::collections::HashMap::new(),
            fade_in: None,
            fade_out: None,
            playlist_loop: false,
            number_start_stop_mode: NumberStartStopMode::None,
            number_start_stop_ids: Vec::new(),
            number_finish_start_ids: Vec::new(),
            background_screensaver: false,
            number_completion_start_ids: Vec::new(),
            seq_current_id: None,
            seq_done: false,
            seq_post_wait_until: None,
            paused_at: None,
            random_bag: Vec::new(),
            rng_state: 0,
            random_current_idx: None,
            fired_child_ids: Vec::new(),
            presentation_retained_background_id: None,
            runtime_error: None,
            number_launched_actions: std::collections::HashSet::new(),
            number_master_finished: false,
            number_post_wait_until: None,
            visual_group_order: None,
            number_master_autoplay: false,
            number_shared_fade: None,
        }
    }

    /// Create an empty Number. The master is assigned later through the
    /// Number-specific command; an empty Number is intentionally a warning.
    pub fn new_number() -> Self {
        let mut cue = Self::new();
        cue.number_mode = true;
        cue.name = "Number".to_string();
        cue
    }

    /// Create an empty Presentation whose pages advance as a playlist.
    pub fn new_presentation() -> Self {
        let mut cue = Self::new();
        cue.presentation_mode = true;
        cue.mode = GroupMode::Playlist;
        cue.playlist_loop = false;
        cue.name = "Presentation".to_string();
        cue
    }

    /// Deserialise a GroupCue from JSON, using `registry` to reconstruct children.
    pub fn from_json_with_registry(value: &Value, registry: &CueRegistry) -> Result<Box<dyn Cue>> {
        let cue_type = value.get("type").and_then(|v| v.as_str()).unwrap_or_default();
        let is_number = cue_type == "number";
        let is_presentation = cue_type == "presentation";
        let mut cue = if is_number { GroupCue::new_number() } else if is_presentation { GroupCue::new_presentation() } else { GroupCue::new() };

        if let Some(s) = value.get("id").and_then(|v| v.as_str()) {
            cue.id = s.parse().unwrap_or_else(|_| Uuid::new_v4());
        }
        if let Some(s) = value.get("name").and_then(|v| v.as_str()) {
            cue.name = s.to_string();
        }
        if let Some(s) = value.get("number").and_then(|v| v.as_str()) {
            cue.number = Some(s.to_string());
        }
        if let Some(s) = value.get("notes").and_then(|v| v.as_str()) {
            cue.notes = s.to_string();
        }
        if let Some(ms) = value.get("pre_wait_ms").and_then(|v| v.as_u64()) {
            cue.pre_wait = Duration::from_millis(ms);
        }
        if let Some(ms) = value.get("post_wait_ms").and_then(|v| v.as_u64()) {
            cue.post_wait = Duration::from_millis(ms);
        }
        if let Some(cm) = value.get("continue_mode") {
            if let Ok(m) = serde_json::from_value(cm.clone()) {
                cue.continue_mode = m;
            }
        }
        if let Some(col) = value.get("color") {
            if let Ok(c) = serde_json::from_value(col.clone()) {
                cue.color = c;
            }
        }
        if let Some(gm) = value.get("group_mode") {
            if let Ok(m) = serde_json::from_value(gm.clone()) {
                cue.mode = m;
            }
        }
        if let Some(b) = value.get("playlist_loop").and_then(|v| v.as_bool()) {
            cue.playlist_loop = b;
        }
        if is_presentation {
            cue.mode = GroupMode::Playlist;
            cue.playlist_loop = false;
        }

        // Deserialise children recursively.
        if let Some(arr) = value.get("children").and_then(|v| v.as_array()) {
            for child_val in arr {
                match registry.from_json(child_val.clone()) {
                    Ok(child) => cue.children.push(child),
                    Err(e) => log::warn!("[group] skipping unrecognised child: {e}"),
                }
            }
        }
        if let Some(b) = value.get("is_disabled").and_then(|v| v.as_bool()) {
            cue.is_disabled = b;
        }

        if is_number {
            cue.background_screensaver = value.get("background_screensaver").and_then(Value::as_bool).unwrap_or(false);
            for child in &mut cue.children {
                child.set_pre_wait(Duration::ZERO);
            }
            cue.master_child_id = value
                .get("master_child_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok());
            if let Some(offsets) = value.get("action_offsets_ms").and_then(|v| v.as_object()) {
                cue.action_offsets_ms = offsets
                    .iter()
                    .filter_map(|(id, value)| value.as_u64().and_then(|ms| id.parse().ok().map(|id| (id, ms))))
                    .collect();
            }
            cue.action_offsets_ms.retain(|id, _| cue.children.iter().any(|child| child.id() == *id));
            cue.fade_in = fade_spec_from_json(value, "fade_in_ms", "fade_in_curve");
            cue.fade_out = fade_spec_from_json(value, "fade_out_ms", "fade_out_curve");
            if let Some(mode) = value.get("number_start_stop_mode") {
                cue.number_start_stop_mode = serde_json::from_value(mode.clone()).unwrap_or_default();
            }
            cue.number_start_stop_ids = value
                .get("number_start_stop_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|id| id.parse().ok())
                .collect();
            cue.number_finish_start_ids = value
                .get("number_finish_start_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|id| id.parse().ok())
                .collect();
        }

        Ok(Box::new(cue))
    }

    // ── Private helpers ───────────────────────────────────────────────────

    fn number_master_index(&self) -> Option<usize> {
        self.master_child_id
            .and_then(|id| self.children.iter().position(|child| child.id() == id))
    }

    /// Build the complete Number action schedule for a seek. Older shows do
    /// not persist an entry for a zero-offset action; absence still means
    /// "start at zero", not "never due".
    fn number_seek_action_offsets(&self, master_idx: usize) -> Vec<(CueId, u64)> {
        self.children
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != master_idx)
            .map(|(_, child)| {
                (
                    child.id(),
                    self.action_offsets_ms.get(&child.id()).copied().unwrap_or(0),
                )
            })
            .collect()
    }

    fn number_start_action(&mut self, ctx: &CueContext) -> Result<()> {
        self.normalize_number_child_waits();
        let Some(master_idx) = self.number_master_index() else {
            self.runtime_error = Some("Number has no master Audio, Video, or Group cue".into());
            self.state = CueState::Standby;
            return Err(anyhow!("Number has no master Audio, Video, or Group cue"));
        };
        let master_type = self.children[master_idx].cue_type();
        if !matches!(master_type, CueType::Audio | CueType::Video | CueType::Group) {
            self.runtime_error = Some("Number master must be an Audio, Video, or Group cue".into());
            self.state = CueState::Standby;
            return Err(anyhow!("Number master must be an Audio, Video, or Group cue"));
        }

        self.number_launched_actions.clear();
        self.number_master_finished = false;
        self.number_post_wait_until = None;
        self.number_completion_start_ids.clear();
        self.runtime_error = None;
        self.children[master_idx].set_number_master_autoplay(true);
        let number_fade_in = self.fade_in.clone();
        if let Err(error) = self.go_child_with_fade(master_idx, ctx, number_fade_in.as_ref()) {
            return self.abort_number(ctx, format!("Number master failed to start: {error}"));
        }
        for idx in 0..self.children.len() {
            if idx == master_idx || self.children[idx].is_disabled() {
                continue;
            }
            if !number_action_type_supported(self.children[idx].cue_type()) {
                return self.abort_number(
                    ctx,
                    format!("Number action '{}' has unsupported type {:?}", self.children[idx].name(), self.children[idx].cue_type()),
                );
            }
            if self.action_offsets_ms.get(&self.children[idx].id()).copied().unwrap_or(0) == 0 {
                if let Err(error) = self.go_child_with_fade(idx, ctx, number_fade_in.as_ref()) {
                    let message = number_action_runtime_error(self.children[idx].name(), "to start", error);
                    return self.abort_number(ctx, message);
                }
                self.number_launched_actions.insert(self.children[idx].id());
            }
        }
        Ok(())
    }

    fn normalize_number_child_waits(&mut self) {
        // The Number scheduler owns the only action clock. A later edit in a
        // child Time tab must not introduce a hidden second delay at GO time.
        for child in &mut self.children {
            child.set_pre_wait(Duration::ZERO);
        }
    }

    /// Reposition a normal Group on its local timeline. This is used when a
    /// Group is the master of a Number, where the parent clock must be mapped
    /// into the active child instead of calling Group::seek on whichever child
    /// happened to be running at GO time.
    fn seek_timeline_position_impl(&mut self, position_ms: u64, ctx: &CueContext) {
        let scoped_ctx = self.scoped_visual_context(ctx);
        let ctx = scoped_ctx.as_ref().unwrap_or(ctx);
        let mut pending_nested_go_ids = Vec::new();
        for child in &mut self.children {
            drain_fired_ids_tree(child.as_mut(), &mut pending_nested_go_ids);
        }
        let was_paused = self.state == CueState::Paused;
        let elapsed = Duration::from_millis(position_ms);
        let action_elapsed = elapsed.saturating_sub(self.pre_wait);
        let now = Instant::now();
        self.action_elapsed_before_pause = action_elapsed;
        self.elapsed_before_pause = elapsed;
        if was_paused {
            self.started_at = None;
            self.action_started_at = None;
            self.paused_at = Some(now);
        } else {
            self.started_at = Some(now - elapsed);
            self.action_started_at = Some(now - action_elapsed);
            self.paused_at = None;
        }

        let target = position_ms;
        for child in &mut self.children {
            if child.is_running() || child.is_paused() {
                let _ = child.hard_stop(ctx);
            } else if child.has_retained_output() {
                let _ = child.hard_stop(ctx);
            } else {
                let _ = child.reset();
            }
        }
        self.seq_post_wait_until = None;
        self.seq_done = false;
        self.seq_current_id = None;
        self.random_current_idx = None;

        let start = self.pre_wait.as_millis() as u64;
        if target < start {
            self.fired_child_ids.extend(pending_nested_go_ids);
            return;
        }
        let local = target.saturating_sub(start);
        let mut cursor = 0u64;
        let mut selected: Option<(usize, u64)> = None;
        match self.mode {
            GroupMode::Simultaneous => {
                for idx in 0..self.children.len() {
                    let child = &mut self.children[idx];
                    let child_start = if matches!(child.cue_type(), CueType::Group | CueType::Presentation) { 0 } else { child.pre_wait().as_millis() as u64 };
                    let child_active = child.duration().map_or(true, |duration| {
                        local < child_start.saturating_add(duration.as_millis() as u64)
                    });
                    if local >= child_start && child_active {
                        if child.go(ctx).is_ok() {
                            child.seek_timeline_position(local.saturating_sub(child_start), ctx);
                            if was_paused && child.is_running() {
                                let _ = child.pause(ctx);
                            }
                        }
                    }
                }
                let mut synthetic_ids = Vec::new();
                for child in &mut self.children {
                    drain_fired_ids_tree(child.as_mut(), &mut synthetic_ids);
                }
                self.fired_child_ids.extend(pending_nested_go_ids);
                return;
            }
            GroupMode::Sequential | GroupMode::Playlist => {
                for idx in 0..self.children.len() {
                    let child = &self.children[idx];
                    let composite = matches!(child.cue_type(), CueType::Group | CueType::Presentation);
                    let child_pre = if composite { 0 } else { child.pre_wait().as_millis() as u64 };
                    let child_post = if composite { 0 } else { child.post_wait().as_millis() as u64 };
                    let child_start = cursor.saturating_add(child_pre);
                    let child_end = child
                        .duration()
                        .map(|duration| child_start.saturating_add(duration.as_millis() as u64));
                    if child_end.map_or(true, |end| local < end) {
                        selected = Some((idx, local.saturating_sub(child_start)));
                        break;
                    }
                    cursor = child_end.unwrap_or(u64::MAX).saturating_add(child_post);
                }
            }
            GroupMode::StartRandom => {
                self.fired_child_ids.extend(pending_nested_go_ids);
                return;
            }
        }
        let Some((idx, child_position)) = selected else {
            self.fired_child_ids.extend(pending_nested_go_ids);
            return;
        };
        let child_id = self.children[idx].id();
        if self.children[idx].go(ctx).is_err() {
            self.fired_child_ids.extend(pending_nested_go_ids);
            return;
        }
        self.seq_current_id = Some(child_id);
        self.children[idx].seek_timeline_position(child_position, ctx);
        let mut synthetic_ids = Vec::new();
        for child in &mut self.children {
            drain_fired_ids_tree(child.as_mut(), &mut synthetic_ids);
        }
        self.fired_child_ids.extend(pending_nested_go_ids);
        if was_paused && self.children[idx].is_running() {
            let _ = self.children[idx].pause(ctx);
        }
    }

    /// Rebuild a Number's child graph at a position on the master clock.
    /// Actions whose offsets are already due are started and sought; later
    /// actions remain in Standby. The parent keeps its Running/Paused state.
    fn seek_number(&mut self, position_ms: u64, ctx: &CueContext) {
        let scoped_ctx = self.scoped_visual_context(ctx);
        let ctx = scoped_ctx.as_ref().unwrap_or(ctx);
        let Some(master_idx) = self.number_master_index() else {
            self.runtime_error = Some("Number has no master Audio, Video, or Group cue".into());
            return;
        };
        let max_ms = self.children[master_idx]
            .duration()
            .map(|duration| duration.as_millis() as u64);
        let target_ms = number_seek_target(position_ms, max_ms);
        let offsets = self.number_seek_action_offsets(master_idx);
        let action_windows: Vec<_> = offsets
            .iter()
            .map(|(id, offset)| {
                let duration_ms = self
                    .children
                    .iter()
                    .find(|child| child.id() == *id)
                    .and_then(|child| child.duration())
                    .map(|duration| duration.as_millis() as u64);
                (
                    *id,
                    *offset,
                    number_action_window_active(target_ms, *offset, duration_ms, max_ms),
                )
            })
            .collect();
        let was_paused = self.state == CueState::Paused;
        let fired_before_seek = self.fired_child_ids.len();
        let mut pending_nested_go_ids = Vec::new();
        for child in &mut self.children {
            drain_fired_ids_tree(child.as_mut(), &mut pending_nested_go_ids);
        }

        // Keep already-running children alive and seek them in place.  A
        // hard-stop/restart cycle is especially harmful for Video Cues: the
        // new mpv slot starts at frame zero while its FILE_LOADED seek is
        // still pending, and a looped clip can visibly/audio-wise restart
        // before the action-time seek arrives.  Only children whose due state
        // changes need to be stopped or started.
        self.number_launched_actions.clear();
        // Actions whose entire window is behind the seek position have
        // already happened on this timeline. Mark them consumed so the next
        // scheduler tick does not fire them again. Future actions stay
        // unlaunched, which lets a later forward seek enter their windows.
        for (id, offset, active) in &action_windows {
            if !active && *offset <= target_ms {
                self.number_launched_actions.insert(*id);
            }
        }
        self.number_master_finished = false;
        self.number_post_wait_until = None;
        self.runtime_error = None;
        self.in_pre_wait = false;
        self.normalize_number_child_waits();

        let now = Instant::now();
        self.action_elapsed_before_pause = Duration::from_millis(target_ms);
        self.elapsed_before_pause = self.pre_wait + self.action_elapsed_before_pause;
        if was_paused {
            self.started_at = None;
            self.action_started_at = None;
        } else {
            self.started_at = Some(now - self.elapsed_before_pause);
            self.action_started_at = Some(now - self.action_elapsed_before_pause);
        }

        // Seeking is repositioning an already-running Number, not a new GO.
        // Starting the master with the authored Number fade here makes every
        // scrub re-apply the fade envelope from zero.
        let seek_fade = number_seek_shared_fade();
        let master_active = self.children[master_idx].is_running()
            || self.children[master_idx].is_paused();
        if master_active {
            self.children[master_idx].seek_timeline_position(target_ms, ctx);
        } else {
            self.children[master_idx].set_initial_seek_action_ms(target_ms);
            if let Err(error) = self.go_child_with_fade(master_idx, ctx, seek_fade.as_ref()) {
                self.runtime_error = Some(format!("Number master failed to seek: {error}"));
                self.fired_child_ids.truncate(fired_before_seek);
                self.fired_child_ids.extend(pending_nested_go_ids);
                return;
            }
            // Audio masters have no initial-seek transport field.  The
            // regular seek is also useful for a newly started video when the
            // output slot has already loaded synchronously.
            self.children[master_idx].seek_timeline_position(target_ms, ctx);
        }

        for idx in 0..self.children.len() {
            if idx == master_idx {
                continue;
            }
            let id = self.children[idx].id();
            let Some((_, offset, active_at_target)) = action_windows
                .iter()
                .find(|(action_id, _, _)| *action_id == id)
            else {
                continue;
            };
            let offset = *offset;
            let active = self.children[idx].is_running() || self.children[idx].is_paused();
            if !*active_at_target || self.children[idx].is_disabled() {
                if active {
                    let _ = self.children[idx].hard_stop(ctx);
                } else if self.children[idx].has_retained_output() {
                    let _ = self.children[idx].hard_stop(ctx);
                } else {
                    let _ = self.children[idx].reset();
                }
                continue;
            }
            let action_ms = target_ms.saturating_sub(offset);
            if active {
                self.children[idx].seek_timeline_position(action_ms, ctx);
            } else {
                self.children[idx].set_initial_seek_action_ms(action_ms);
                if let Err(error) = self.go_child_with_fade(idx, ctx, None) {
                    self.runtime_error = Some(number_action_runtime_error(
                        self.children[idx].name(),
                        "to seek",
                        error,
                    ));
                    continue;
                }
                // Audio children need the normal seek path; for Video this
                // also queues the action coordinate if mpv is still loading.
                self.children[idx].seek_timeline_position(action_ms, ctx);
            }
            self.number_launched_actions.insert(id);
        }

        if was_paused {
            for child in &mut self.children {
                if child.is_running() {
                    let _ = child.pause(ctx);
                }
            }
            self.state = number_seek_state(true);
        } else {
            self.state = number_seek_state(false);
        }
        // Starting a child to reconstruct the seek position is not a new GO
        // command. Do not leak its nested GO notifications to the transport.
        self.fired_child_ids.truncate(fired_before_seek);
        self.fired_child_ids.extend(pending_nested_go_ids);
    }

    fn number_child_done(&self, child: &dyn Cue, ctx: &CueContext) -> bool {
        let live_voice = child.all_voice_ids().into_iter().any(|voice_id| {
            ctx.audio_engine.voice_is_alive(voice_id)
                || ctx.output_engine.is_voice_playing(voice_id)
        });
        !live_voice
            && (matches!(child.state(), CueState::Completed | CueState::Standby)
                || child.automatic_completion_duration().map(|d| child.action_elapsed() >= d).unwrap_or(false))
    }

    fn stop_looping_number_master_audio(&self, master_idx: usize, ctx: &CueContext) -> Result<()> {
        if !self.playlist_loop {
            return Ok(());
        }

        // Number's timeline clock starts before streamed masters can report
        // stream-ready. If that clock reaches the authored end first, stop
        // only the master's live audio voices before natural completion drops
        // their IDs. Leave visual voices intact so held last frames still work.
        let mut voice_ids = std::collections::HashSet::new();
        for cue_voice_id in self.children[master_idx].all_voice_ids() {
            voice_ids.insert(cue_voice_id);
            if let Some(audio_voice_id) = ctx.output_engine.video_audio_voice(cue_voice_id) {
                voice_ids.insert(audio_voice_id);
            }
            if let Some(audio_voice_id) = ctx.output_engine.attached_cue_audio_voice(cue_voice_id) {
                voice_ids.insert(audio_voice_id);
            }
        }
        for voice_id in voice_ids {
            if ctx.audio_engine.voice_is_alive(voice_id) {
                ctx.audio_engine.stop_voice(voice_id, 0, EngineFadeCurve::Linear)?;
            }
        }
        Ok(())
    }

    fn tick_number(&mut self, ctx: &CueContext) -> Result<()> {
        if self.number_master_finished {
            if self
                .number_post_wait_until
                .is_some_and(|due| Instant::now() >= due)
            {
                if self.playlist_loop {
                    // Close the previous pass completely before restarting its
                    // children. Keep the Number execution identity so an
                    // iteration does not look like a new external GO.
                    self.hard_stop_children(ctx);
                    self.reset_children();
                    self.number_master_finished = false;
                    self.number_post_wait_until = None;
                    self.number_completion_start_ids.clear();
                    let now = Instant::now();
                    self.started_at = Some(now);
                    self.action_started_at = Some(now);
                    self.elapsed_before_pause = Duration::ZERO;
                    self.action_elapsed_before_pause = Duration::ZERO;
                    self.in_pre_wait = false;
                    self.number_start_action(ctx)?;
                } else {
                    self.number_post_wait_until = None;
                }
            }
            return Ok(());
        }
        let Some(master_idx) = self.number_master_index() else {
            return Ok(());
        };

        if self.children[master_idx].state() == CueState::Running {
            if let Err(error) = self.children[master_idx].tick(ctx) {
                return self.abort_number(ctx, format!("Number master failed during playback: {error}"));
            }
        }

        let elapsed_ms = self.action_elapsed().as_millis() as u64;
        for idx in 0..self.children.len() {
            if idx == master_idx {
                continue;
            }
            let id = self.children[idx].id();
            if self.number_launched_actions.contains(&id) {
                if self.children[idx].state() == CueState::Running {
                    if let Err(error) = self.children[idx].tick(ctx) {
                        let message = number_action_runtime_error(self.children[idx].name(), "during playback", error);
                        return self.abort_number(ctx, message);
                    }
                    if self.number_child_done(self.children[idx].as_ref(), ctx) {
                        let _ = self.children[idx].complete_naturally();
                    }
                }
                continue;
            }
            if self.children[idx].is_disabled() {
                continue;
            }
            let offset = self.action_offsets_ms.get(&id).copied().unwrap_or(0);
            if offset <= elapsed_ms {
                // This action starts after the Number fade-in window. Keep
                // only its own authored fade settings.
                if let Err(error) = self.go_child_with_fade(idx, ctx, None) {
                    let message = number_action_runtime_error(self.children[idx].name(), "to start", error);
                    return self.abort_number(ctx, message);
                }
                self.number_launched_actions.insert(id);
            }
        }

        // Check the Number clock as well as the child voice. The outer event
        // loop also uses Number::duration(); this guarantees cleanup happens
        // in this tick before that generic completion path can reset us.
        let master_duration = self.children[master_idx].automatic_completion_duration();
        if let (Some(fade), Some(duration)) = (self.fade_out.as_ref(), master_duration) {
            let fade_duration = Duration::from_millis(fade.duration_ms);
            if fade_duration > Duration::ZERO
                && self.action_elapsed().saturating_add(fade_duration) >= duration
            {
                let number_fade_out = self.fade_out.clone();
                for child in &mut self.children {
                    if child.is_running() || child.is_paused() {
                        let _ = child.stop_with_fade(ctx, number_fade_out.as_ref());
                    }
                }
                self.number_master_finished = true;
                self.number_post_wait_until = Some(Instant::now() + fade_duration + self.post_wait);
                self.number_completion_start_ids = self.number_finish_start_ids.clone();
                return Ok(());
            }
        }
        let master_duration_elapsed = master_duration
            .map(|duration| self.action_elapsed() >= duration)
            .unwrap_or(false);
        if master_duration_elapsed || self.number_child_done(self.children[master_idx].as_ref(), ctx) {
            self.number_master_finished = true;
            self.number_post_wait_until = Some(Instant::now() + self.post_wait);
            self.number_completion_start_ids = self.number_finish_start_ids.clone();
            let number_fade_out = self.fade_out.clone();
            for idx in 0..self.children.len() {
                if idx == master_idx {
                    continue;
                }
                // Number-owned media actions stop when the master ends.
                if number_action_is_temporary_media(self.children[idx].cue_type())
                    && (self.children[idx].is_running() || self.children[idx].is_paused()) {
                    let _ = self.children[idx].stop_with_fade(ctx, number_fade_out.as_ref());
                }
            }
            if let Err(error) = self.stop_looping_number_master_audio(master_idx, ctx) {
                return self.abort_number(ctx, format!("Failed to stop Number master audio before loop: {error}"));
            }
            let _ = self.children[master_idx].complete_naturally();
        }
        Ok(())
    }

    fn start_action(&mut self, ctx: &CueContext) -> Result<()> {
        self.in_pre_wait = false;
        self.action_started_at = Some(Instant::now());
        self.state = CueState::Running;
        self.seq_done = false;
        self.seq_post_wait_until = None;

        if self.number_mode {
            return self.number_start_action(ctx);
        }

        match self.mode {
            GroupMode::Simultaneous => {
                for idx in 0..self.children.len() {
                    if self.children[idx].is_disabled() {
                        continue;
                    }
                    if let Err(e) = self.go_child(idx, ctx) {
                        log::warn!("Group simultaneous: child '{}' failed to start: {e}", self.children[idx].name());
                    }
                }
            }
            GroupMode::Sequential | GroupMode::Playlist => {
                // Fire from the inner playhead (set by `set_active_child` when the
                // user parks the Playhead on a specific child); `None` starts from
                // the first child.  Playlist adds exclusivity + loop inside
                // `fire_next_sequential`.
                let start_after = self.seq_current_id;
                if let Err(e) = self.fire_next_sequential(ctx, start_after) {
                    log::warn!("Group sequential/playlist: first child failed to start: {e}");
                    self.seq_done = true;
                }
            }
            GroupMode::StartRandom => {
                // Fire exactly one randomly-chosen child; never chains.
                self.ensure_rng_seeded();
                if let Some(idx) = self.draw_random_child() {
                    self.random_current_idx = Some(idx);
                    if let Err(e) = self.go_child(idx, ctx) {
                        log::warn!("Group start_random: child '{}' failed to start: {e}", self.children[idx].name());
                        self.random_current_idx = None;
                    }
                }
            }
        }
        Ok(())
    }

    /// Start a direct child, recording its ID only on success. IDs queued by a
    /// nested Group are folded into this Group so the root can drain them once.
    fn go_child(&mut self, idx: usize, ctx: &CueContext) -> Result<()> {
        self.go_child_with_fade(idx, ctx, None)
    }

    fn go_child_with_fade(&mut self, idx: usize, ctx: &CueContext, shared_fade: Option<&FadeSpec>) -> Result<()> {
        let shared_fade = shared_fade.or(self.number_shared_fade.as_ref());
        let child_ctx = self.visual_group_order
            .map(|order| ctx.with_visual_group_order(order));
        let child_ctx = child_ctx.as_ref().unwrap_or(ctx);
        let (id, nested_ids) = {
            let child = &mut self.children[idx];
            if child.is_disabled() {
                anyhow::bail!("Cue '{}' is disarmed", child.name());
            }
            child.go_with_fade(child_ctx, shared_fade)?;
            (child.id(), child.take_fired_cue_ids())
        };
        self.fired_child_ids.push(id);
        self.fired_child_ids.extend(nested_ids);
        Ok(())
    }

    fn scoped_visual_context(&self, ctx: &CueContext) -> Option<CueContext> {
        self.visual_group_order
            .map(|order| ctx.with_visual_group_order(order))
    }

    /// Fire the next sequential child after `after_id` (or the first child if
    /// `after_id` is `None`).
    fn fire_next_sequential(&mut self, ctx: &CueContext, after_id: Option<CueId>) -> Result<()> {
        // Any call that advances the internal playhead supersedes the old
        // timer (in particular, a manual GO while a post-wait is pending).
        self.seq_post_wait_until = None;
        if let Some(previous_id) = after_id {
            if let Some(previous) = self.children.iter_mut().find(|child| child.id() == previous_id) {
                if matches!(previous.state(), CueState::Completed | CueState::Standby) {
                    let _ = previous.complete_naturally();
                }
            }
        }
        let first_idx = match after_id {
            None => 0,
            Some(prev_id) => {
                match self.children.iter().position(|c| c.id() == prev_id) {
                    Some(i) => i + 1,
                    None => return Ok(()),
                }
            }
        };

        let next_idx = (first_idx..self.children.len())
            .find(|&idx| !self.children[idx].is_disabled());
        let Some(next_idx) = next_idx else {
            // Playlist with loop wraps back to the first child instead of ending.
            if self.mode == GroupMode::Playlist
                && self.playlist_loop
                && self.children.iter().any(|child| !child.is_disabled())
            {
                return self.fire_next_sequential(ctx, None);
            }
            self.seq_done = true;
            return Ok(());
        };

        let child_id = self.children[next_idx].id();
        self.seq_current_id = Some(child_id);
        let as_slide = self.presentation_mode && self.mode == GroupMode::Playlist && self.child_is_as_slide_video(next_idx);
        let backdrop_id = if as_slide {
            self.children[..next_idx].iter().rev()
                .find(|child| child.cue_type() == CueType::Image)
                .map(|child| child.id())
        } else { None };
        if !as_slide {
            self.presentation_retained_background_id = None;
        }

        if let Some(backdrop_id) = backdrop_id {
            // Clear the previous page first, then restart/hold the preceding
            // PNG if its authored duration already completed. Start the video
            // last so its layer remains above the static page.
            self.stop_other_children_except(ctx, &[backdrop_id]);
            if let Some(backdrop) = self.children.iter_mut().find(|child| child.id() == backdrop_id) {
                if matches!(backdrop.state(), CueState::Completed | CueState::Standby) {
                    let _ = backdrop.go(ctx);
                }
                if backdrop.is_running() { let _ = backdrop.pause(ctx); }
            }
            self.presentation_retained_background_id = Some(backdrop_id);
        }

        if let Err(e) = self.go_child(next_idx, ctx) {
            log::warn!("Group sequential: child '{}' failed to start: {e}", self.children[next_idx].name());
            // Child rolled back to Standby — treat it as done and advance.
            if let Some(background_id) = self.presentation_retained_background_id.take() {
                self.stop_child_by_id(ctx, background_id);
            }
            self.seq_done = true;
            return Ok(());
        }

        // Playlist is exclusive: starting this child stops any other still playing.
        if self.mode == GroupMode::Playlist {
            if let Some(backdrop_id) = backdrop_id {
                self.stop_other_children_except(ctx, &[child_id, backdrop_id]);
            } else {
                self.stop_other_children(ctx, child_id);
            }
        }
        if self.mode != GroupMode::Playlist || self.presentation_mode {
            self.schedule_sequential_auto_continue(child_id, ctx)?;
        }

        Ok(())
    }

    /// Auto-Continue advances from action start, allowing the current child to
    /// overlap the next. Its clock is polled directly from the child so pause
    /// and media seek behavior match top-level Auto-Continue without a separate
    /// wall-clock deadline.
    fn schedule_sequential_auto_continue(
        &mut self,
        child_id: CueId,
        ctx: &CueContext,
    ) -> Result<bool> {
        if self.mode == GroupMode::Playlist && !self.presentation_mode {
            return Ok(false);
        }
        let Some(idx) = self.children.iter().position(|child| child.id() == child_id) else {
            return Ok(false);
        };
        let (managed, due) = {
            let child = &mut self.children[idx];
            if child.continue_mode() != ContinueMode::AutoContinue
                || !matches!(child.state(), CueState::Running | CueState::Paused | CueState::Completed)
                || !child.is_action_started()
                || child.is_auto_continue_fired()
            {
                return Ok(false);
            }
            (true, child.state() != CueState::Paused && child.auto_continue_elapsed() >= child.post_wait())
        };
        if due {
            self.children[idx].mark_auto_continue_fired();
            self.fire_next_sequential(ctx, Some(child_id))?;
        }
        Ok(managed)
    }

    fn set_pending_advance(&mut self, source_id: CueId, delay: Duration, require_marker: bool) {
        let Some(source) = self.children.iter_mut().find(|child| child.id() == source_id) else {
            return;
        };
        if require_marker {
            source.mark_auto_continue_fired();
        }
        self.seq_post_wait_until = Some(PendingGroupAdvance {
            due: Instant::now() + delay,
            source_id,
            source_generation: source.play_generation(),
            expected_current_id: self.seq_current_id,
            require_marker,
            source_paused_at: None,
        });
    }

    fn pending_advance_is_valid(&self, pending: PendingGroupAdvance) -> bool {
        if self.state != CueState::Running || self.seq_current_id != pending.expected_current_id {
            return false;
        }
        self.children
            .iter()
            .find(|child| child.id() == pending.source_id)
            .is_some_and(|source| {
                source.play_generation() == pending.source_generation
                    && (!pending.require_marker || source.auto_continue_marker() == Some(true))
            })
    }

    /// Tick a child at `idx` and return whether it is now complete.
    fn tick_child_at(&mut self, idx: usize, ctx: &CueContext) -> Result<bool> {
        let (done, fired_ids) = {
            let child = &mut self.children[idx];
            if child.state() == CueState::Running {
                child.tick(ctx)?;
            }

            let live_voice = child.all_voice_ids().into_iter().any(|voice_id| {
                ctx.audio_engine.voice_is_alive(voice_id)
                    || ctx.output_engine.is_voice_playing(voice_id)
            });
            let done = !live_voice
                && (matches!(child.state(), CueState::Completed | CueState::Standby)
                    || child
                        .automatic_completion_duration()
                        .map(|d| child.action_elapsed() >= d)
                        .unwrap_or(false));
            (done, child.take_fired_cue_ids())
        };
        self.fired_child_ids.extend(fired_ids);
        Ok(done)
    }

    /// Drive one tick of a Sequential (or Playlist) group: tick running children,
    /// advance the inner sequence when the current child completes (respecting its
    /// Continue Mode), and honour AutoContinue post-waits.  Playlist's exclusivity
    /// and loop are handled inside `fire_next_sequential`.
    fn tick_sequential(&mut self, ctx: &CueContext) -> Result<()> {
        // Tick every running child so overlapping cues (a manual GO or Auto-Follow
        // fired the next child while a previous one is still playing) keep
        // progressing and finish on their own.  Reset any child that finishes
        // EXCEPT the current sequence driver — its completion advances the sequence.
        let current_id_opt = self.seq_current_id;
        let mut current_done = false;
        for i in 0..self.children.len() {
            if self.children[i].state() == CueState::Running {
                let done = self.tick_child_at(i, ctx)?;
                if done {
                    if Some(self.children[i].id()) == current_id_opt {
                        current_done = true;
                    } else {
                        let _ = self.children[i].complete_naturally();
                    }
                }
            }
        }

        // Post-wait before firing the next child.
        if let Some(pending) = self.seq_post_wait_until.as_mut() {
            let source_paused = self
                .children
                .iter()
                .find(|child| child.id() == pending.source_id)
                .is_some_and(|source| source.is_paused());
            if source_paused {
                pending.source_paused_at.get_or_insert_with(Instant::now);
                return Ok(());
            }
            if let Some(paused_at) = pending.source_paused_at.take() {
                pending.due += paused_at.elapsed();
            }
        }
        if let Some(pending) = self.seq_post_wait_until {
            if !self.pending_advance_is_valid(pending) {
                // If the same sequence driver had its execution marker cleared
                // (Stop/Reset) or its source disappeared, park at this boundary.
                // Otherwise the fallthrough below would see the reset child as
                // complete and immediately arm a fresh post-wait, defeating the
                // cancellation. A retrigger has a different generation and owns
                // a new timeline, so it is allowed to continue normally.
                if self.state == CueState::Running
                    && self.seq_current_id == pending.expected_current_id
                {
                    let source = self
                        .children
                        .iter()
                        .find(|child| child.id() == pending.source_id);
                    let same_execution = source.is_some_and(|source| {
                        source.play_generation() == pending.source_generation
                    });
                    let marker_was_cancelled = source.map_or(true, |source| {
                        pending.require_marker
                            && source.auto_continue_marker() != Some(true)
                    });
                    if same_execution && marker_was_cancelled {
                        self.seq_done = true;
                    }
                }
                self.seq_post_wait_until = None;
            } else if Instant::now() >= pending.due {
                self.seq_post_wait_until = None;
                self.fire_next_sequential(ctx, Some(pending.source_id))?;
                return Ok(());
            } else {
                return Ok(());
            }
        }

        if let Some(current_id) = current_id_opt {
            if self.schedule_sequential_auto_continue(current_id, ctx)? {
                return Ok(());
            }
        }

        let current_id = match current_id_opt {
            Some(id) => id,
            None => return Ok(()),
        };
        let idx = match self.children.iter().position(|c| c.id() == current_id) {
            Some(i) => i,
            None => return Ok(()),
        };
        // The driver child may have finished in a previous tick before we got here.
        if matches!(self.children[idx].state(), CueState::Completed | CueState::Standby) {
            current_done = true;
        }

        if current_done && !self.has_next_sequential_child() {
            self.release_presentation_background(ctx);
        }

        // `seq_done` means the sequence is intentionally paused at this child
        // (a DoNotContinue boundary, or a manual Playhead placement) — wait for a
        // GO rather than auto-advancing.
        if current_done && (!self.seq_done || self.number_master_autoplay) {
            let cm = self.children[idx].continue_mode();
            let pw = self.children[idx].post_wait();
            // Playlist advances even when the child has no continuation mode,
            // but a source that exposes an execution marker can still cancel a
            // stale deadline if an operator stops/resets it during the wait.
            let require_marker = self.children[idx].auto_continue_marker().is_some();
            let _ = self.children[idx].complete_naturally();

            if self.mode == GroupMode::Playlist && !self.presentation_mode {
                // Ordinary Playlists auto-advance regardless of child mode.
                // Presentation uses exclusive switching but respects each page's
                // Continue Mode, like a manual sequence with one active child.
                if pw == Duration::ZERO {
                    self.fire_next_sequential(ctx, Some(current_id))?;
                } else {
                    self.set_pending_advance(current_id, pw, require_marker);
                }
            } else {
                match cm {
                    ContinueMode::DoNotContinue => {
                        if self.number_master_autoplay {
                            if pw.is_zero() {
                                self.fire_next_sequential(ctx, Some(current_id))?;
                            } else {
                                self.set_pending_advance(current_id, pw, false);
                            }
                        } else {
                            self.seq_done = true;
                        }
                    }
                    ContinueMode::AutoContinue => {
                        // A valid Auto-Continue is handled above from the
                        // child execution clock. If the action never started,
                        // park the sequence rather than inventing a timer from
                        // its completion/reset time.
                        self.seq_done = true;
                    }
                    ContinueMode::AutoFollow => {
                        // Auto-Follow occurs after action completion; its
                        // post-wait is a delay before advancing, just as it is
                        // for top-level cues.
                        if pw.is_zero() {
                            self.fire_next_sequential(ctx, Some(current_id))?;
                        } else {
                            self.set_pending_advance(current_id, pw, true);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// `true` when the sequential sequence has paused mid-way (current child
    /// completed with `DoNotContinue`) AND there are more children left to
    /// fire.  Used by [`absorbs_go`](crate::cue::traits::Cue::absorbs_go).
    fn has_next_sequential_child(&self) -> bool {
        match self.seq_current_id {
            Some(current_id) => self
                .children
                .iter()
                .position(|c| c.id() == current_id)
                .map(|i| i + 1 < self.children.len())
                .unwrap_or(false),
            None => !self.children.is_empty(),
        }
    }

    /// Reset all children to Standby and clear sequential/random state.
    fn reset_children(&mut self) {
        for child in &mut self.children {
            let _ = child.reset();
        }
        self.seq_current_id = None;
        self.seq_done = false;
        self.seq_post_wait_until = None;
        self.paused_at = None;
        self.random_bag.clear();
        self.random_current_idx = None;
        self.presentation_retained_background_id = None;
    }

    fn clear_runtime_state(&mut self) {
        self.state = CueState::Standby;
        self.started_at = None;
        self.action_started_at = None;
        self.elapsed_before_pause = Duration::ZERO;
        self.action_elapsed_before_pause = Duration::ZERO;
        self.in_pre_wait = false;
        self.auto_continue_fired = false;
        self.number_launched_actions.clear();
        self.number_master_finished = false;
        self.number_post_wait_until = None;
        self.runtime_error = None;
        self.number_master_autoplay = false;
        self.number_shared_fade = None;
        self.number_completion_start_ids.clear();
        self.visual_group_order = None;
    }

    fn complete_children_naturally(&mut self) {
        for child in &mut self.children {
            let _ = child.complete_naturally();
        }
        self.seq_current_id = None;
        self.seq_done = false;
        self.seq_post_wait_until = None;
        self.paused_at = None;
        self.random_bag.clear();
        self.random_current_idx = None;
    }

    fn hard_stop_children(&mut self, ctx: &CueContext) {
        for child in &mut self.children {
            if child.is_running() || child.is_paused() || child.has_retained_output() {
                let _ = child.hard_stop(ctx);
            }
            child.set_number_master_autoplay(false);
        }
    }

    fn abort_number(&mut self, ctx: &CueContext, error: String) -> Result<()> {
        self.hard_stop_children(ctx);
        self.reset_children();
        self.state = CueState::Standby;
        self.started_at = None;
        self.action_started_at = None;
        self.runtime_error = Some(error.clone());
        Err(anyhow!(error))
    }

    // ── Playlist exclusivity ──────────────────────────────────────────────

    /// Stop (and reset) every running/paused child except `keep_id`.  Used by
    /// Playlist mode so only one child is ever audible at a time.
    fn stop_other_children(&mut self, ctx: &CueContext, keep_id: CueId) {
        self.stop_other_children_except(ctx, &[keep_id]);
    }

    fn stop_other_children_except(&mut self, ctx: &CueContext, keep_ids: &[CueId]) {
        for child in &mut self.children {
            if !keep_ids.contains(&child.id())
                && (child.is_running() || child.is_paused() || child.has_retained_output())
            {
                let _ = child.stop(ctx);
                let _ = child.reset();
            }
        }
    }

    fn stop_child_by_id(&mut self, ctx: &CueContext, id: CueId) {
        if let Some(child) = self.children.iter_mut().find(|child| child.id() == id) {
            if child.is_running() || child.is_paused() || child.has_retained_output() {
                let _ = child.hard_stop(ctx);
            }
            let _ = child.reset();
        }
    }

    fn release_presentation_background(&mut self, ctx: &CueContext) {
        if let Some(id) = self.presentation_retained_background_id.take() {
            self.stop_child_by_id(ctx, id);
        }
    }

    fn child_is_as_slide_video(&self, idx: usize) -> bool {
        let Some(child) = self.children.get(idx) else { return false };
        if child.cue_type() != CueType::Video { return false; }
        let json = child.serialize();
        json.get("presentation_video_imported").and_then(Value::as_bool) == Some(true)
            && json.get("presentation_video_mode").and_then(Value::as_str) == Some("as_slide")
            && json.get("presentation_video_layout_supported").and_then(Value::as_bool) == Some(true)
    }

    // ── StartRandom PRNG (xorshift64*) + shuffle bag ──────────────────────

    /// Lazily seed the PRNG.  xorshift cannot escape a zero state, so the seed is
    /// forced non-zero and mixed from a per-process counter and the wall clock.
    fn ensure_rng_seeded(&mut self) {
        if self.rng_state != 0 {
            return;
        }
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        // Golden-ratio mix so consecutive groups seeded in the same tick diverge.
        self.rng_state = (nanos ^ (n.wrapping_mul(0x9E37_79B9_7F4A_7C15))) | 1;
    }

    /// Advance the xorshift64* generator and return the next pseudo-random u64.
    fn next_rand(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng_state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Refill the bag with `0..children.len()` and Fisher–Yates shuffle it.
    fn refill_random_bag(&mut self) {
        self.random_bag = (0..self.children.len()).collect();
        let len = self.random_bag.len();
        for i in (1..len).rev() {
            let j = (self.next_rand() % (i as u64 + 1)) as usize;
            self.random_bag.swap(i, j);
        }
    }

    /// Draw the next child index for StartRandom, refilling + reshuffling the bag
    /// when it empties (so every child plays once before any repeats).  `None`
    /// only when the group has no children.
    fn draw_random_child(&mut self) -> Option<usize> {
        if self.children.is_empty() {
            return None;
        }
        if self.random_bag.is_empty() {
            self.refill_random_bag();
        }
        while let Some(idx) = self.random_bag.pop() {
            if !self.children[idx].is_disabled() {
                return Some(idx);
            }
        }
        None
    }

    #[cfg(test)]
    fn seed_rng_for_test(&mut self, seed: u64) {
        self.rng_state = seed | 1;
        self.random_bag.clear();
    }
}

impl Default for GroupCue {
    fn default() -> Self {
        Self::new()
    }
}

/// Return all blocking configuration problems for a Number.  The same check
/// is used by preflight and GO so a Number cannot pass one path and fail in
/// the other.
pub fn number_readiness_issues(cue: &dyn Cue) -> Vec<String> {
    if cue.cue_type() != CueType::Number {
        return Vec::new();
    }
    let Some(children) = cue.child_cues() else {
        return vec!["Number needs a master Audio, Video, or Group cue".into()];
    };
    let Some(master_id) = cue.number_master_id() else {
        return vec!["Number needs a master Audio, Video, or Group cue".into()];
    };
    let Some(master) = children.iter().find(|child| child.id() == master_id) else {
        return vec!["Number master cue no longer exists".into()];
    };
    let mut issues = Vec::new();
    if !matches!(master.cue_type(), CueType::Audio | CueType::Video | CueType::Group) {
        issues.push("Number master must be Audio, Video, or Group".into());
    } else if master.is_disabled() {
        issues.push("Number master is disabled".into());
    } else if master.cue_type() == CueType::Group {
        if master.duration().is_none() || master.child_cues().map(|children| children.is_empty()).unwrap_or(true) {
            issues.push("Number Group master needs children with a finite duration".into());
        }
    } else if master.media_file_path().map(|path| path.as_os_str().is_empty() || !path.exists()).unwrap_or(true) {
        issues.push("Number master needs an existing audio or video file".into());
    }
    for child in children {
        if child.id() == master_id || child.is_disabled() {
            continue;
        }
        match child.cue_type() {
            CueType::Audio | CueType::Video | CueType::Image => {
                if child.media_file_path().map(|path| path.as_os_str().is_empty() || !path.exists()).unwrap_or(true) {
                    issues.push(format!("Number action '{}' needs an existing media file", child.name()));
                }
            }
            CueType::Group => {
                if child.child_cues().map(|children| children.is_empty()).unwrap_or(true) {
                    issues.push(format!("Number action '{}' cannot be an empty Group", child.name()));
                }
            }
            _ => issues.push(format!("Number action '{}' has unsupported type {:?}", child.name(), child.cue_type())),
        }
    }
    issues
}

// ---------------------------------------------------------------------------
// Cue trait implementation
// ---------------------------------------------------------------------------

impl Cue for GroupCue {
    // ── Identity ──────────────────────────────────────────────────────────

    fn id(&self) -> CueId { self.id }
    fn cue_type(&self) -> CueType { if self.number_mode { CueType::Number } else if self.presentation_mode { CueType::Presentation } else { CueType::Group } }
    fn name(&self) -> &str { &self.name }
    fn set_name(&mut self, name: String) { self.name = name; }
    fn number(&self) -> Option<&str> { self.number.as_deref() }
    fn set_number(&mut self, number: Option<String>) { self.number = number; }
    fn notes(&self) -> &str { &self.notes }
    fn set_notes(&mut self, notes: String) { self.notes = notes; }
    fn color(&self) -> CueColor { self.color }
    fn set_color(&mut self, color: CueColor) { self.color = color; }
    fn is_disabled(&self) -> bool { self.is_disabled }
    fn set_disabled(&mut self, d: bool) { self.is_disabled = d; }

    fn validate(
        &self,
        _ctx: &crate::cue::validation::ValidationContext,
    ) -> Vec<crate::cue::validation::CueIssue> {
        use crate::cue::validation::CueIssue;

        if !self.number_mode {
            return Vec::new();
        }

        number_readiness_issues(self)
            .into_iter()
            .map(CueIssue::error)
            .collect()
    }

    // ── State ─────────────────────────────────────────────────────────────

    fn state(&self) -> CueState { self.state }

    fn take_fired_cue_ids(&mut self) -> Vec<CueId> {
        std::mem::take(&mut self.fired_child_ids)
    }

    fn take_fade_stop_targets(&mut self) -> Vec<CueId> {
        self.children
            .iter_mut()
            .flat_map(|child| child.take_fade_stop_targets())
            .collect()
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────

    fn load(&mut self, ctx: &CueContext) -> Result<()> {
        for child in &mut self.children {
            child.load(ctx)?;
        }
        Ok(())
    }

    fn go(&mut self, ctx: &CueContext) -> Result<()> {
        if self.number_mode && self.state == CueState::Running {
            return Ok(());
        }
        if self.state == CueState::Running && !self.in_pre_wait {
            match self.mode {
                // Sequential and Playlist absorb a GO by advancing their inner
                // sequence.  Playlist adds exclusivity/loop inside
                // `fire_next_sequential`; the advance logic is otherwise identical.
                GroupMode::Sequential | GroupMode::Playlist => {
                    // Sequence paused (DoNotContinue child finished) → fire next child.
                    if self.seq_done && self.has_next_sequential_child() {
                        self.seq_done = false;
                        let prev_id = self.seq_current_id;
                        return self.fire_next_sequential(ctx, prev_id);
                    }
                    // A child is still running → advance to the next child.  In
                    // Sequential the previous child keeps playing (audio overlap
                    // like top-level cues); in Playlist `fire_next_sequential`
                    // stops it (exclusivity).
                    if let Some(current_id) = self.seq_current_id {
                        self.seq_done = false;
                        return self.fire_next_sequential(ctx, Some(current_id));
                    }
                }
                // StartRandom absorbs every GO by firing another random child.
                GroupMode::StartRandom => {
                    self.ensure_rng_seeded();
                    if let Some(idx) = self.draw_random_child() {
                        self.random_current_idx = Some(idx);
                        if let Err(e) = self.go_child(idx, ctx) {
                            log::warn!("Group start_random: child failed to start: {e}");
                            self.random_current_idx = None;
                        }
                    }
                    return Ok(());
                }
                GroupMode::Simultaneous => {}
            }
        }

        // Assign identity on the accepted GO, before a pre-wait or any media
        // load. Absorbed GO and Running Number no-op paths above keep theirs.
        self.visual_group_order = if self.number_mode {
            ctx.visual_group_order().or_else(|| Some(ctx.output_engine.next_layer_order()))
        } else {
            ctx.visual_group_order()
        };
        self.execution_generation = self.execution_generation.wrapping_add(1);
        self.auto_continue_fired = false;
        self.started_at = Some(Instant::now());
        self.elapsed_before_pause = Duration::ZERO;
        self.action_elapsed_before_pause = Duration::ZERO;
        self.number_launched_actions.clear();
        self.number_master_finished = false;
        self.number_post_wait_until = None;

        if self.pre_wait > Duration::ZERO {
            self.in_pre_wait = true;
            self.state = CueState::Running;
            return Ok(());
        }

        self.start_action(ctx)
    }

    fn stop(&mut self, ctx: &CueContext) -> Result<()> {
        let shared_fade = self.fade_out.clone();
        let shared_fade = shared_fade.as_ref().or(self.number_shared_fade.as_ref());
        for child in &mut self.children {
            if child.is_running() || child.is_paused() || child.has_retained_output() {
                let _ = child.stop_with_fade(ctx, shared_fade);
            }
            child.set_number_master_autoplay(false);
        }
        self.reset_children();
        self.state = CueState::Standby;
        self.started_at = None;
        self.action_started_at = None;
        self.elapsed_before_pause = Duration::ZERO;
        self.action_elapsed_before_pause = Duration::ZERO;
        self.in_pre_wait = false;
        self.auto_continue_fired = false;
        self.number_launched_actions.clear();
        self.number_master_finished = false;
        self.number_post_wait_until = None;
        self.runtime_error = None;
        self.number_master_autoplay = false;
        self.number_shared_fade = None;
        self.number_completion_start_ids.clear();
        self.visual_group_order = None;
        ctx.emit(CueEvent::Stopped { cue_id: self.id });
        Ok(())
    }

    fn pause(&mut self, ctx: &CueContext) -> Result<()> {
        if self.state != CueState::Running {
            return Ok(());
        }
        if let Some(started_at) = self.started_at.take() {
            self.elapsed_before_pause = started_at.elapsed();
        }
        if let Some(action_started_at) = self.action_started_at.take() {
            self.action_elapsed_before_pause = action_started_at.elapsed();
        }
        for child in &mut self.children {
            if child.is_running() {
                let _ = child.pause(ctx);
            }
        }
        self.paused_at = Some(Instant::now());
        self.state = CueState::Paused;
        Ok(())
    }

    fn resume(&mut self, ctx: &CueContext) -> Result<()> {
        if self.state != CueState::Paused {
            return Ok(());
        }
        let scoped_ctx = self.scoped_visual_context(ctx);
        let ctx = scoped_ctx.as_ref().unwrap_or(ctx);
        let paused_for = self.paused_at.take().map(|at| at.elapsed()).unwrap_or_default();
        let now = Instant::now();
        self.started_at = Some(now - self.elapsed_before_pause);
        if !self.in_pre_wait {
            self.action_started_at = Some(now - self.action_elapsed_before_pause);
        }
        if let Some(pending) = self.seq_post_wait_until.as_mut() {
            pending.due += paused_for;
            if let Some(source_paused_at) = pending.source_paused_at.as_mut() {
                *source_paused_at += paused_for;
            }
        }
        if let Some(due) = self.number_post_wait_until.as_mut() {
            *due += paused_for;
        }
        for child in &mut self.children {
            if child.is_paused() && Some(child.id()) != self.presentation_retained_background_id {
                let _ = child.resume(ctx);
            }
        }
        self.state = CueState::Running;
        Ok(())
    }

    fn hard_stop(&mut self, ctx: &CueContext) -> Result<()> {
        for child in &mut self.children {
            if child.is_running() || child.is_paused() || child.has_retained_output() {
                let _ = child.hard_stop(ctx);
            }
            child.set_number_master_autoplay(false);
        }
        self.reset_children();
        self.state = CueState::Standby;
        self.started_at = None;
        self.action_started_at = None;
        self.elapsed_before_pause = Duration::ZERO;
        self.action_elapsed_before_pause = Duration::ZERO;
        self.in_pre_wait = false;
        self.auto_continue_fired = false;
        self.number_launched_actions.clear();
        self.number_master_finished = false;
        self.number_post_wait_until = None;
        self.runtime_error = None;
        self.number_master_autoplay = false;
        self.number_shared_fade = None;
        self.number_completion_start_ids.clear();
        self.visual_group_order = None;
        ctx.emit(CueEvent::Stopped { cue_id: self.id });
        Ok(())
    }

    fn reset(&mut self) -> Result<()> {
        self.reset_children();
        self.clear_runtime_state();
        Ok(())
    }

    fn complete_naturally(&mut self) -> Result<()> {
        self.complete_children_naturally();
        self.clear_runtime_state();
        Ok(())
    }

    fn has_retained_output(&self) -> bool {
        self.children.iter().any(|child| child.has_retained_output())
    }

    fn tick(&mut self, ctx: &CueContext) -> Result<()> {
        let scoped_ctx = self.scoped_visual_context(ctx);
        let ctx = scoped_ctx.as_ref().unwrap_or(ctx);
        if self.state == CueState::Paused {
            return Ok(());
        }
        // ── Pre-wait ──────────────────────────────────────────────────────
        if self.in_pre_wait {
            if self.started_at.map(|t| t.elapsed()).unwrap_or(Duration::ZERO) >= self.pre_wait {
                self.start_action(ctx)?;
            }
            return Ok(());
        }

        if self.state != CueState::Running {
            return Ok(());
        }

        if self.number_mode {
            return self.tick_number(ctx);
        }

        match self.mode {
            // ── Simultaneous ──────────────────────────────────────────────
            GroupMode::Simultaneous => {
                for i in 0..self.children.len() {
                    // A finished child MUST be reset here: the event loop's
                    // completion detector is top-level only and never descends
                    // into group children.  Without this a child that plays out
                    // lingers in Running forever, so is_complete() never fires
                    // and the group (and its children) stay stuck.
                    if self.children[i].state() == CueState::Running && self.tick_child_at(i, ctx)? {
                        let _ = self.children[i].complete_naturally();
                    }
                }
            }

            // ── Sequential / Playlist ─────────────────────────────────────
            // Same driver logic; Playlist's exclusivity + loop live inside
            // `fire_next_sequential`, so no per-mode branch is needed here.
            GroupMode::Sequential | GroupMode::Playlist => {
                self.tick_sequential(ctx)?;
            }

            // ── StartRandom ───────────────────────────────────────────────
            // Exactly one child runs per GO; just tick it and clean up when it
            // finishes (never auto-advances — the next GO draws again).
            GroupMode::StartRandom => {
                for i in 0..self.children.len() {
                    if self.children[i].state() == CueState::Running && self.tick_child_at(i, ctx)? {
                        if Some(i) == self.random_current_idx {
                            self.random_current_idx = None;
                        }
                        let _ = self.children[i].complete_naturally();
                    }
                }
            }
        }

        Ok(())
    }

    fn take_runtime_diagnostic_changed(&mut self) -> bool {
        // The event loop only ticks top-level cues. Drain every descendant so
        // a nested camera diagnostic still refreshes its row after a Group
        // resets that child in the same tick.
        let mut changed = false;
        for child in &mut self.children {
            changed |= child.take_runtime_diagnostic_changed();
        }
        changed
    }

    fn runtime_error(&self) -> Option<&str> {
        self.runtime_error.as_deref()
            .or_else(|| self.children.iter().find_map(|child| child.runtime_error()))
    }

    fn pending_child_post_wait(&self) -> Option<(CueId, Duration, Duration)> {
        if self.number_mode {
            if !self.number_master_finished {
                return None;
            }
            let due = self.number_post_wait_until?;
            let total = self.post_wait;
            if total.is_zero() {
                return None;
            }
            // Number's deadline includes an optional master fade-out before
            // authored Post-Wait. Clamp remaining to the Post-Wait duration so
            // the bar stays empty during that fade prefix.
            let clock_now = self.paused_at.unwrap_or_else(Instant::now);
            let remaining = due.saturating_duration_since(clock_now);
            let elapsed = total.saturating_sub(remaining.min(total));
            return Some((self.id, elapsed, total));
        }
        let pending = self.seq_post_wait_until?;
        if !self.pending_advance_is_valid(pending) {
            return None;
        }
        let source = self.children.iter().find(|child| child.id() == pending.source_id)?;
        let total = source.post_wait();
        if total.is_zero() {
            return None;
        }
        let clock_now = pending.source_paused_at.unwrap_or_else(Instant::now);
        let remaining = pending.due.saturating_duration_since(clock_now);
        Some((pending.source_id, total.saturating_sub(remaining), total))
    }

    fn is_action_started(&self) -> bool {
        !self.in_pre_wait
    }

    // ── Timing ────────────────────────────────────────────────────────────

    fn pre_wait(&self) -> Duration { self.pre_wait }
    fn set_pre_wait(&mut self, d: Duration) { self.pre_wait = d; }
    fn post_wait(&self) -> Duration { self.post_wait }
    fn set_post_wait(&mut self, d: Duration) { self.post_wait = d; }

    fn duration(&self) -> Option<Duration> {
        if self.number_mode {
            return self
                .number_master_index()
                .and_then(|idx| self.children[idx].duration())
                .map(|duration| duration + self.post_wait);
        }
        if self.mode == GroupMode::Playlist && self.playlist_loop {
            return None;
        }
        let inner = match self.mode {
            GroupMode::StartRandom => return None,
            GroupMode::Simultaneous => self
                .children
                .iter()
                .map(|child| child_duration_with_waits(child.as_ref()))
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .max()
                .unwrap_or(Duration::ZERO),
            GroupMode::Sequential | GroupMode::Playlist => {
                let mut total = Duration::ZERO;
                for child in &self.children {
                    if self.mode == GroupMode::Sequential && child.continue_mode() == ContinueMode::AutoContinue {
                        // Auto-Continue fires from action start and may overlap
                        // the previous child, so a simple sum is not reliable.
                        return None;
                    }
                    total = total.checked_add(child_duration_with_waits(child.as_ref())?)?;
                }
                total
            }
        };
        Some(self.pre_wait.saturating_add(inner).saturating_add(self.post_wait))
    }

    fn automatic_completion_duration(&self) -> Option<Duration> {
        if self.number_mode {
            // The Number scheduler owns master EOF and Post-Wait completion.
            // A parent duration must not complete it before that deadline.
            if self.number_master_finished || self.playlist_loop {
                return None;
            }
            let master = self.number_master_index().and_then(|idx| self.children.get(idx))?;
            master.automatic_completion_duration()?;
            return self.duration();
        }
        let duration = self.duration()?;
        for child in &self.children {
            child.automatic_completion_duration()?;
        }
        Some(duration)
    }

    fn media_file_path(&self) -> Option<&std::path::Path> {
        if self.number_mode {
            self.number_master_index().and_then(|idx| self.children[idx].media_file_path())
        } else {
            None
        }
    }

    fn file_duration(&self) -> Option<Duration> {
        if self.number_mode {
            self.number_master_index().and_then(|idx| self.children[idx].file_duration())
        } else {
            self.duration()
        }
    }

    fn elapsed(&self) -> Duration {
        if self.state == CueState::Paused {
            return self.elapsed_before_pause;
        }
        self.started_at.map(|t| t.elapsed()).unwrap_or(Duration::ZERO)
    }

    fn action_elapsed(&self) -> Duration {
        if self.state == CueState::Paused {
            return self.action_elapsed_before_pause;
        }
        self.action_started_at.map(|t| t.elapsed()).unwrap_or(Duration::ZERO)
    }

    // ── Continue ──────────────────────────────────────────────────────────

    fn continue_mode(&self) -> ContinueMode { self.continue_mode }
    fn set_continue_mode(&mut self, mode: ContinueMode) { self.continue_mode = mode; }

    fn seek(&mut self, position_ms: u64, ctx: &CueContext) {
        if self.number_mode && matches!(self.state, CueState::Running | CueState::Paused) {
            self.seek_number(position_ms, ctx);
            return;
        }
        for child in &mut self.children {
            if child.is_running() || child.is_paused() {
                child.seek(position_ms, ctx);
            }
        }
    }

    fn seek_timeline_position(&mut self, position_ms: u64, ctx: &CueContext) {
        if self.number_mode && matches!(self.state, CueState::Running | CueState::Paused) {
            self.seek_number(position_ms, ctx);
        } else {
            self.seek_timeline_position_impl(position_ms, ctx);
        }
    }

    fn is_auto_continue_fired(&self) -> bool { self.auto_continue_fired }
    fn play_generation(&self) -> u64 { self.execution_generation }
    fn auto_continue_marker(&self) -> Option<bool> { Some(self.auto_continue_fired) }
    fn mark_auto_continue_fired(&mut self) { self.auto_continue_fired = true; }
    fn clear_auto_continue_fired(&mut self) { self.auto_continue_fired = false; }

    // ── Group support ─────────────────────────────────────────────────────

    fn is_complete(&self) -> bool {
        if self.number_mode {
            return self.state == CueState::Running
                && !self.in_pre_wait
                && !self.playlist_loop
                && self.number_master_finished
                && self
                    .number_post_wait_until
                    .map_or(true, |due| Instant::now() >= due);
        }
        if self.state != CueState::Running || self.in_pre_wait {
            return false;
        }
        match self.mode {
            GroupMode::Simultaneous => {
                self.children.iter().all(|c| !cue_tree_is_active(c.as_ref()))
            }
            GroupMode::Sequential => {
                // seq_done means either "paused at DoNotContinue child" OR
                // "all children exhausted".  The group is only truly complete
                // when there are NO more children left to fire.
                self.seq_done
                    && !self.has_next_sequential_child()
                    && self.children.iter().all(|c| !cue_tree_is_active(c.as_ref()))
            }
            GroupMode::Playlist => {
                // A looping playlist never completes on its own — it plays until
                // the operator stops it.  Otherwise same rule as Sequential.
                if self.playlist_loop {
                    false
                } else {
                    self.seq_done
                        && !self.has_next_sequential_child()
                        && self.children.iter().all(|c| !cue_tree_is_active(c.as_ref()))
                }
            }
            GroupMode::StartRandom => {
                // Complete once the fired child has finished; the next GO would
                // draw again, but until then the group is idle/done.
                self.random_current_idx.is_none()
                    && self.children.iter().all(|c| !cue_tree_is_active(c.as_ref()))
            }
        }
    }

    fn all_voice_ids(&self) -> Vec<CueId> {
        // A group owns the voices of every child, recursively (nested groups
        // included).  This makes a Group a valid target for a volume/pan Fade or
        // a Stop that needs the actual voice handles.
        self.children.iter().flat_map(|c| c.all_voice_ids()).collect()
    }

    fn stop_on_next_go(&self) -> bool {
        self.presentation_mode
    }

    fn child_cues(&self) -> Option<&[Box<dyn Cue>]> {
        Some(&self.children)
    }

    fn child_cues_mut(&mut self) -> Option<&mut Vec<Box<dyn Cue>>> {
        Some(&mut self.children)
    }

    fn take_children(&mut self) -> Option<Vec<Box<dyn Cue>>> {
        Some(std::mem::take(&mut self.children))
    }

    fn add_child(&mut self, child: Box<dyn Cue>, position: i32) -> Result<()> {
        if self.number_mode && !number_action_type_supported(child.cue_type()) {
            anyhow::bail!("Number actions support Audio, Video, Image, and Group cues");
        }
        let mut child = child;
        if self.number_mode {
            // Number owns one timeline. A child pre-wait would create a hidden
            // second clock and make the visible offset misleading.
            child.set_pre_wait(Duration::ZERO);
        }
        if position < 0 || position as usize >= self.children.len() {
            self.children.push(child);
        } else {
            self.children.insert(position as usize, child);
        }
        Ok(())
    }

    fn remove_child(&mut self, id: &CueId) -> Result<Box<dyn Cue>> {
        let idx = self
            .children
            .iter()
            .position(|c| c.id() == *id)
            .ok_or_else(|| anyhow!("Child cue {:?} not found in group", id))?;
        let child = self.children.remove(idx);
        if self.number_mode {
            if self.master_child_id == Some(*id) {
                self.master_child_id = None;
            }
            self.action_offsets_ms.remove(id);
        }
        Ok(child)
    }

    fn group_mode(&self) -> Option<GroupMode> {
        (!self.number_mode).then_some(self.mode)
    }

    fn set_group_mode(&mut self, mode: GroupMode) {
        self.mode = if self.presentation_mode { GroupMode::Playlist } else { mode };
    }

    fn playlist_loop(&self) -> Option<bool> {
        Some(self.playlist_loop)
    }

    fn set_playlist_loop(&mut self, on: bool) {
        self.playlist_loop = !self.presentation_mode && on;
    }

    fn number_start_stop_specification(&self) -> Option<(NumberStartStopMode, Vec<CueId>)> {
        self.number_mode
            .then_some((self.number_start_stop_mode, self.number_start_stop_ids.clone()))
    }

    fn number_finish_start_ids(&self) -> Vec<CueId> {
        if self.number_mode { self.number_finish_start_ids.clone() } else { Vec::new() }
    }

    fn background_screensaver(&self) -> bool {
        self.number_mode && self.background_screensaver
    }

    fn set_background_screensaver(&mut self, enabled: bool) {
        if self.number_mode {
            self.background_screensaver = enabled;
        }
    }

    fn halt_automatic_flow_for_protected_children(&mut self) {
        self.seq_done = true;
        self.seq_post_wait_until = None;
    }

    fn take_number_completion_start_ids(&mut self) -> Vec<CueId> {
        if self.number_mode && self.playlist_loop {
            // Keep the configured finish targets pending. If loop is disabled
            // during Post-Wait, the final natural completion still starts them.
            return Vec::new();
        }
        std::mem::take(&mut self.number_completion_start_ids)
    }

    fn set_number_master_autoplay(&mut self, enabled: bool) {
        self.number_master_autoplay = enabled;
    }

    fn go_with_fade(&mut self, ctx: &CueContext, shared_fade: Option<&FadeSpec>) -> Result<()> {
        let authored = self.number_shared_fade.clone();
        self.number_shared_fade = shared_fade.cloned();
        let result = self.go(ctx);
        self.number_shared_fade = authored;
        result
    }

    fn stop_with_fade(&mut self, ctx: &CueContext, shared_fade: Option<&FadeSpec>) -> Result<()> {
        let authored = self.number_shared_fade.clone();
        self.number_shared_fade = shared_fade.cloned();
        let result = self.stop(ctx);
        self.number_shared_fade = authored;
        result
    }

    fn number_master_id(&self) -> Option<CueId> {
        self.number_mode.then_some(self.master_child_id).flatten()
    }

    fn set_number_master(&mut self, id: CueId) -> Result<()> {
        if !self.number_mode {
            anyhow::bail!("Not a Number cue");
        }
        let child = self.children.iter().find(|child| child.id() == id)
            .ok_or_else(|| anyhow!("Number master child not found"))?;
        if !matches!(child.cue_type(), CueType::Audio | CueType::Video | CueType::Group) {
            anyhow::bail!("Number master must be Audio, Video, or Group");
        }
        self.master_child_id = Some(id);
        self.action_offsets_ms.remove(&id);
        if let Some(child) = self.children.iter_mut().find(|child| child.id() == id) {
            child.set_pre_wait(Duration::ZERO);
        }
        Ok(())
    }

    fn number_action_offsets(&self) -> Vec<(CueId, u64)> {
        if !self.number_mode { return Vec::new(); }
        self.action_offsets_ms.iter().map(|(id, offset)| (*id, *offset)).collect()
    }

    fn set_number_action_offset(&mut self, id: CueId, offset_ms: u64) -> Result<()> {
        if !self.number_mode {
            anyhow::bail!("Number action not found");
        }
        if self.master_child_id == Some(id) {
            anyhow::bail!("The Number master cannot be an action");
        }
        let Some(child) = self.children.iter_mut().find(|child| child.id() == id) else {
            anyhow::bail!("Number action not found");
        };
        if !number_action_type_supported(child.cue_type()) {
            anyhow::bail!("Number actions support Audio, Video, Image, and Group cues");
        }
        child.set_pre_wait(Duration::ZERO);
        self.action_offsets_ms.insert(id, offset_ms);
        Ok(())
    }

    fn absorbs_go(&self) -> bool {
        if self.state != CueState::Running || self.in_pre_wait {
            return false;
        }
        match self.mode {
            GroupMode::Simultaneous => false,
            // Absorb while another child remains after the current one.
            GroupMode::Sequential => self.has_next_sequential_child(),
            // A looping Playlist absorbs forever (wraps); otherwise as Sequential.
            GroupMode::Playlist => self.playlist_loop || self.has_next_sequential_child(),
            // Each GO fires another random child until the operator stops it.
            GroupMode::StartRandom => true,
        }
    }

    fn holds_playhead(&self) -> bool {
        if self.state != CueState::Running {
            return false;
        }
        match self.mode {
            GroupMode::Simultaneous => false,
            // Keep the Playhead while pre-waiting or while a child remains.
            GroupMode::Sequential => self.in_pre_wait || self.has_next_sequential_child(),
            GroupMode::Playlist => {
                self.in_pre_wait || self.playlist_loop || self.has_next_sequential_child()
            }
            // Random groups hold the Playhead for the whole run (GO re-fires).
            GroupMode::StartRandom => true,
        }
    }

    fn released_playhead(&self) -> bool {
        // The last child has been fired: a child was started, none remain, and we
        // are past any pre-wait.  The group may still be running (overlapping
        // children playing out) but the outer Playhead should move on.  A looping
        // Playlist and a StartRandom group never release (they GO in place).
        if self.state != CueState::Running || self.in_pre_wait {
            return false;
        }
        match self.mode {
            GroupMode::Sequential => {
                self.seq_current_id.is_some() && !self.has_next_sequential_child()
            }
            GroupMode::Playlist => {
                !self.playlist_loop
                    && self.seq_current_id.is_some()
                    && !self.has_next_sequential_child()
            }
            GroupMode::Simultaneous | GroupMode::StartRandom => false,
        }
    }

    fn active_child_id(&self) -> Option<CueId> {
        // Only the ordered modes have a meaningful "next child a GO will fire".
        // StartRandom has none (nothing armed in the UI); Simultaneous fires all.
        if !matches!(self.mode, GroupMode::Sequential | GroupMode::Playlist) {
            return None;
        }
        // Works in every state: Standby (None → first child, or a parked child),
        // Running (the child after the current one), and after the last child has
        // fired (None → the Playhead has left the group).
        match self.seq_current_id {
            Some(id) => {
                let idx = self.children.iter().position(|c| c.id() == id)?;
                self.children.get(idx + 1).map(|c| c.id())
            }
            None => self.children.first().map(|c| c.id()),
        }
    }

    fn presentation_current_child_id(&self) -> Option<CueId> {
        (self.presentation_mode
            && matches!(self.state, CueState::Running | CueState::Paused)
            && !self.in_pre_wait)
            .then_some(self.seq_current_id)
            .flatten()
    }

    fn step_presentation(&mut self, ctx: &CueContext, direction: &str) -> Result<()> {
        if !self.presentation_mode {
            anyhow::bail!("Cue is not a Presentation");
        }
        if !matches!(self.state, CueState::Running | CueState::Paused) {
            anyhow::bail!("Presentation is not running or paused");
        }
        if self.in_pre_wait {
            anyhow::bail!("Presentation is still in pre-wait");
        }
        let current_id = self.seq_current_id
            .ok_or_else(|| anyhow!("Presentation has no current slide"))?;
        let current_idx = self.children.iter().position(|child| child.id() == current_id)
            .ok_or_else(|| anyhow!("Presentation current slide was not found"))?;
        let target_idx = match direction {
            "previous" => (0..current_idx).rev().find(|&idx| !self.children[idx].is_disabled()),
            "next" => (current_idx + 1..self.children.len()).find(|&idx| !self.children[idx].is_disabled()),
            _ => anyhow::bail!("Direction must be 'previous' or 'next'"),
        };
        let Some(target_idx) = target_idx else {
            return Ok(()); // Clamp at either end; never wrap.
        };

        let target_id = self.children[target_idx].id();
        let parent_paused = self.state == CueState::Paused;
        // ImageCue::go creates a new output voice when called while Paused.
        // A retained AsSlide backdrop must release its old slot before it is
        // restarted as the foreground slide, or the old voice becomes orphaned.
        let restart_retained_image = self.presentation_retained_background_id == Some(target_id)
            && self.children[target_idx].cue_type() == CueType::Image
            && self.children[target_idx].is_paused();
        if restart_retained_image {
            self.children[target_idx].hard_stop(ctx)
                .map_err(|error| anyhow!("Could not release retained slide background: {error}"))?;
        }
        let as_slide = self.child_is_as_slide_video(target_idx);
        let backdrop_id = if as_slide {
            self.children[..target_idx].iter().rev()
                .find(|child| child.cue_type() == CueType::Image)
                .map(|child| child.id())
        } else { None };

        // Match normal Presentation GO: an AsSlide video retains the nearest
        // preceding image underneath it, while other slides are exclusive.
        if let Some(id) = backdrop_id {
            if let Some(backdrop) = self.children.iter_mut().find(|child| child.id() == id) {
                if matches!(backdrop.state(), CueState::Completed | CueState::Standby) {
                    let _ = backdrop.go(ctx);
                }
                if backdrop.is_running() {
                    let _ = backdrop.pause(ctx);
                }
            }
        }

        if let Err(error) = self.go_child(target_idx, ctx) {
            if restart_retained_image {
                let restored = self.children[target_idx].go(ctx).is_ok()
                    && (!self.children[target_idx].is_running() || self.children[target_idx].pause(ctx).is_ok());
                if !restored {
                    self.presentation_retained_background_id = None;
                }
            }
            if let Some(background_id) = backdrop_id {
                if Some(background_id) != self.seq_current_id
                    && Some(background_id) != self.presentation_retained_background_id
                {
                    self.stop_child_by_id(ctx, background_id);
                }
            }
            anyhow::bail!("Could not start slide '{}': {error}", self.children[target_idx].name());
        }
        if parent_paused {
            if let Err(error) = self.children[target_idx].pause(ctx) {
                let _ = self.children[target_idx].hard_stop(ctx);
                anyhow::bail!("Could not pause slide '{}': {error}", self.children[target_idx].name());
            }
        }
        self.seq_post_wait_until = None;
        self.seq_done = false;
        self.seq_current_id = Some(target_id);
        self.presentation_retained_background_id = backdrop_id;
        if let Some(backdrop_id) = backdrop_id {
            self.stop_other_children_except(ctx, &[target_id, backdrop_id]);
        } else {
            self.stop_other_children(ctx, target_id);
        }
        self.schedule_sequential_auto_continue(target_id, ctx)?;
        Ok(())
    }

    fn set_active_child(&mut self, child_id: &CueId) -> bool {
        if !matches!(self.mode, GroupMode::Sequential | GroupMode::Playlist) {
            return false;
        }
        let Some(idx) = self.children.iter().position(|c| c.id() == *child_id) else {
            return false;
        };
        // Park the inner playhead on the child BEFORE the target so the next fire
        // (start_action on a standby group, or an absorbed GO on a running one)
        // fires the target.  `None` when the target is the first child.
        self.seq_current_id = if idx == 0 {
            None
        } else {
            Some(self.children[idx - 1].id())
        };
        // Pause here: a running sequence waits for the next GO instead of
        // auto-advancing.  A standby group ignores this — start_action clears
        // seq_done and fires from seq_current_id.
        self.seq_done = true;
        self.seq_post_wait_until = None;
        true
    }

    // ── Serialisation ─────────────────────────────────────────────────────

    fn serialize(&self) -> Value {
        let children: Vec<Value> = self.children.iter().map(|c| c.serialize()).collect();
        let mut value = json!({
            "type": if self.number_mode { "number" } else if self.presentation_mode { "presentation" } else { "group" },
            "cue_type": if self.number_mode { "number" } else if self.presentation_mode { "presentation" } else { "group" },
            "id": self.id,
            "number": self.number,
            "name": self.name,
            "notes": self.notes,
            "color": self.color,
            "pre_wait_ms": self.pre_wait.as_millis() as u64,
            "post_wait_ms": self.post_wait.as_millis() as u64,
            "continue_mode": self.continue_mode,
            "group_mode": self.mode,
            "playlist_loop": if self.presentation_mode { false } else { self.playlist_loop },
            "children": children,
            "is_disabled": self.is_disabled,
        });
        // Keep legacy Group JSON byte-for-byte compatible in shape. Number-only
        // fields must not leak into ordinary groups or alter old workspaces on save.
        if self.number_mode {
            let object = value.as_object_mut().expect("cue serialization is an object");
            object.insert("master_child_id".into(), json!(self.master_child_id));
            object.insert(
                "action_offsets_ms".into(),
                json!(self.action_offsets_ms.iter().map(|(id, offset)| (id.to_string(), *offset)).collect::<std::collections::HashMap<_, _>>()),
            );
            object.insert("fade_in_ms".into(), self.fade_in.as_ref().map(|fade| json!(fade.duration_ms)).unwrap_or(Value::Null));
            object.insert("fade_in_curve".into(), self.fade_in.as_ref().map(|fade| json!(fade.curve)).unwrap_or(Value::Null));
            object.insert("fade_out_ms".into(), self.fade_out.as_ref().map(|fade| json!(fade.duration_ms)).unwrap_or(Value::Null));
            object.insert("fade_out_curve".into(), self.fade_out.as_ref().map(|fade| json!(fade.curve)).unwrap_or(Value::Null));
            object.insert("number_start_stop_mode".into(), json!(self.number_start_stop_mode));
            object.insert("number_start_stop_ids".into(), json!(self.number_start_stop_ids.iter().map(ToString::to_string).collect::<Vec<_>>()));
            object.insert("number_finish_start_ids".into(), json!(self.number_finish_start_ids.iter().map(ToString::to_string).collect::<Vec<_>>()));
            object.insert("background_screensaver".into(), json!(self.background_screensaver));
        }
        value
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Factory for [`GroupCue`].  Register this in [`super::registry::CueRegistry`].
pub struct GroupCueFactory;

impl CueFactory for GroupCueFactory {
    fn create(&self) -> Box<dyn Cue> {
        Box::new(GroupCue::new())
    }

    /// NOTE: This factory's `from_json` is intentionally never called.
    /// [`CueRegistry::from_json`] special-cases `CueType::Group` and calls
    /// [`GroupCue::from_json_with_registry`] directly so that children are
    /// deserialised with the registry.
    fn from_json(&self, _value: Value) -> Result<Box<dyn Cue>> {
        Ok(Box::new(GroupCue::new()))
    }
}

/// Factory for the user-facing Number variant. It shares the recursive child
/// implementation with Group but has its own persisted type and scheduler.
pub struct NumberCueFactory;

impl CueFactory for NumberCueFactory {
    fn create(&self) -> Box<dyn Cue> {
        Box::new(GroupCue::new_number())
    }

    fn from_json(&self, value: Value) -> Result<Box<dyn Cue>> {
        let registry = CueRegistry::new();
        GroupCue::from_json_with_registry(&value, &registry)
    }
}

/// Factory for the recursive Presentation variant.
pub struct PresentationCueFactory;

impl CueFactory for PresentationCueFactory {
    fn create(&self) -> Box<dyn Cue> { Box::new(GroupCue::new_presentation()) }
    fn from_json(&self, value: Value) -> Result<Box<dyn Cue>> {
        let registry = CueRegistry::new();
        GroupCue::from_json_with_registry(&value, &registry)
    }
}

// ---------------------------------------------------------------------------
// Tests — playhead-handling logic (pure, no CueContext required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::audio_cue::AudioCue;
    use crate::cue::browser_cue::BrowserCue;
    use crate::cue::camera_cue::CameraCue;
    use crate::cue::image_cue::ImageCue;
    use crate::cue::light_cue::LightCue;
    use crate::cue::memo_cue::MemoCue;
    use crate::cue::timer_cue::TimerCue;
    use crate::cue::wait_cue::WaitCue;
    use crate::cue::traits::RuntimeState;
    use crate::cue::video_cue::{VideoCue, VideoCueFactory};
    use crate::show::cue_list::CueList;

    #[test]
    fn number_completion_state_diff_includes_all_descendant_media() {
        fn running_number(post_wait: Duration) -> (GroupCue, CueId, [CueId; 3]) {
            let mut master = AudioCue::new();
            let master_id = master.id();
            master.accept_preloaded_audio(
                std::sync::Arc::new(vec![0.0; 57_600]),
                2,
                48_000,
                Duration::from_millis(600),
            );

            let mut video = VideoCue::new();
            video.file_path = Some(std::path::PathBuf::from("number-end-video.mp4"));
            video.accept_preloaded_audio(
                std::sync::Arc::new(vec![0.0; 57_600]),
                2,
                48_000,
                Duration::from_millis(600),
            );
            let video_id = video.id();

            let mut image = ImageCue::new();
            image.file_path = Some(std::path::PathBuf::from("number-end-image.png"));
            image.display_duration_ms = Some(100);
            let image_id = image.id();

            let mut number = GroupCue::new_number();
            number.children = vec![Box::new(master), Box::new(video), Box::new(image)];
            number.set_number_master(master_id).unwrap();
            number.set_number_action_offset(video_id, 0).unwrap();
            number.set_number_action_offset(image_id, 0).unwrap();
            number.set_post_wait(post_wait);
            (number, master_id, [master_id, video_id, image_id])
        }

        fn state_diff(cue: &mut GroupCue, update: impl FnOnce(&mut GroupCue)) -> Vec<(CueId, CueState, CueState)> {
            let before = crate::show::event_loop::cue_tree_cue_states(cue);
            update(cue);
            let after = crate::show::event_loop::cue_tree_cue_states(cue);
            crate::show::event_loop::changed_cue_states(&before, &after)
        }

        // The scheduler tick, not a test-only state assignment, reaches the
        // Number end. This covers both clock completion and an earlier master
        // EOF, plus forward/back repositioning and pause/resume.
        for (seek, pause, early_eof, post_wait) in [
            (false, false, false, Duration::ZERO),
            (true, false, false, Duration::ZERO),
            (false, true, false, Duration::ZERO),
            (false, false, true, Duration::ZERO),
            (false, false, false, Duration::from_millis(20)),
        ] {
            let (context, _) = presentation_context_with_output();
            let (mut number, master_id, child_ids) = running_number(post_wait);
            let number_id = number.id();
            let before_go = crate::show::event_loop::cue_tree_cue_states(&number);
            number.go(&context).unwrap();
            let after_go = crate::show::event_loop::cue_tree_cue_states(&number);
            let mut changes = crate::show::event_loop::changed_cue_states(&before_go, &after_go);
            assert!(number.children.iter().all(|child| child.state() == CueState::Running));
            if seek {
                number.seek_number(80, &context);
                number.seek_number(20, &context);
                assert!(number.children.iter().all(|child| child.state() == CueState::Running));
            }
            if pause {
                number.pause(&context).unwrap();
                assert_eq!(number.state(), CueState::Paused);
                number.tick(&context).unwrap();
                assert_eq!(number.state(), CueState::Paused, "paused Number must not finish on its wall clock");
                number.resume(&context).unwrap();
            }
            if !seek && !pause && !early_eof && post_wait.is_zero() {
                // The finite Image action ends before its 600 ms master.
                std::thread::sleep(Duration::from_millis(120));
                changes.extend(state_diff(&mut number, |number| {
                    number.tick(&context).unwrap();
                }));
                assert_eq!(number.state(), CueState::Running);
                assert_eq!(number.children[2].state(), CueState::Standby);
                assert!(changes.contains(&(child_ids[2], CueState::Running, CueState::Standby)));
            }
            if early_eof {
                // The event loop reaps a naturally ended voice during its
                // completion phase, before it checks the parent Number.
            } else {
                number.action_started_at = Some(Instant::now() - Duration::from_millis(610));
            }

            changes.extend(state_diff(&mut number, |number| {
                if early_eof {
                    number.children
                        .iter_mut()
                        .find(|child| child.id() == master_id)
                        .unwrap()
                        .complete_naturally()
                        .unwrap();
                }
                number.tick(&context).unwrap();
            }));
            if post_wait > Duration::ZERO {
                assert_eq!(number.state(), CueState::Running);
                assert!(!number.is_complete(), "post-wait keeps the Number alive");
                number.number_post_wait_until = Some(Instant::now() - Duration::from_millis(1));
                changes.extend(state_diff(&mut number, |number| {
                    number.tick(&context).unwrap();
                    if number.is_complete() {
                        number.complete_naturally().unwrap();
                    }
                }));
            } else {
                assert!(number.is_complete(), "master end must complete the Number");
                changes.extend(state_diff(&mut number, |number| {
                    number.complete_naturally().unwrap();
                }));
            }

            assert_eq!(number.state(), CueState::Standby);
            assert!(number.children.iter().all(|child| child.state() == CueState::Standby));
            assert!(changes.contains(&(number_id, CueState::Running, CueState::Standby)));
            for child_id in child_ids {
                assert!(changes.contains(&(child_id, CueState::Running, CueState::Standby)),
                    "state event diff must clear media child {child_id}");
            }

            // A new GO followed by a completion in the same scheduler turn
            // must leave the final UI state at the backend's terminal state.
            if !seek && !pause && !early_eof && post_wait.is_zero() {
                let before_retrigger = crate::show::event_loop::cue_tree_cue_states(&number);
                number.go(&context).unwrap();
                let after_retrigger = crate::show::event_loop::cue_tree_cue_states(&number);
                let mut same_tick_changes = crate::show::event_loop::changed_cue_states(&before_retrigger, &after_retrigger);
                number.action_started_at = Some(Instant::now() - Duration::from_millis(610));
                same_tick_changes.extend(state_diff(&mut number, |number| {
                    number.tick(&context).unwrap();
                    if number.is_complete() {
                        number.complete_naturally().unwrap();
                    }
                }));
                let mut states: std::collections::HashMap<_, _> = before_retrigger.into_iter().collect();
                for (id, _, state) in same_tick_changes {
                    states.insert(id, state);
                }
                assert_eq!(states.get(&number_id), Some(&CueState::Standby));
                for child_id in child_ids {
                    assert_eq!(states.get(&child_id), Some(&CueState::Standby));
                }
            }
        }
    }

    #[test]
    fn selected_child_keeps_zero_wait_playlist_loop_advancing() {
        fn run(paused: bool) {
            let ctx = presentation_context();
            let mut playlist = GroupCue::new();
            playlist.mode = GroupMode::Playlist;
            playlist.playlist_loop = true;
            let ids: Vec<_> = (0..3)
                .map(|_| {
                    let mut child = WaitCue::new();
                    child.set_user_action_duration(Some(Duration::ZERO)).unwrap();
                    let id = child.id();
                    playlist.children.push(Box::new(child));
                    id
                })
                .collect();
            let playlist_id = playlist.id();
            playlist.go(&ctx).unwrap();
            if paused {
                playlist.pause(&ctx).unwrap();
            }
            let mut cue_list = CueList::new("test");
            cue_list.push(Box::new(playlist));

            if paused {
                cue_list
                    .set_playhead_for_selection(Some(ids[2]))
                    .unwrap();
                assert_eq!(
                    cue_list.get(&playlist_id).unwrap().active_child_id(),
                    Some(ids[1]),
                    "paused playlist retains its next child"
                );
                cue_list.get_mut(&playlist_id).unwrap().resume(&ctx).unwrap();
            }
            let mut observed = Vec::new();
            for target_idx in [0, 2, 1, 0, 2, 1, 0, 2, 1] {
                cue_list
                    .set_playhead_for_selection(Some(ids[target_idx]))
                    .unwrap();
                cue_list.get_mut(&playlist_id).unwrap().tick(&ctx).unwrap();
                let running_child = cue_list
                    .get(&playlist_id)
                    .unwrap()
                    .child_cues()
                    .unwrap()
                    .iter()
                    .find(|child| child.state() == CueState::Running)
                    .map(|child| child.id());
                observed.push(running_child);
            }
            assert_eq!(
                observed,
                vec![
                    Some(ids[1]), Some(ids[2]), Some(ids[0]),
                    Some(ids[1]), Some(ids[2]), Some(ids[0]),
                    Some(ids[1]), Some(ids[2]), Some(ids[0]),
                ],
                "selection must not interrupt three complete playlist loops (paused={paused})",
            );
        }

        run(false);
        run(true);
    }

    #[test]
    fn standby_selection_and_explicit_playhead_keep_existing_positioning() {
        let mut playlist = GroupCue::new();
        playlist.mode = GroupMode::Playlist;
        let mut ids = Vec::new();
        for _ in 0..3 {
            let child = WaitCue::new();
            ids.push(child.id());
            playlist.children.push(Box::new(child));
        }
        let playlist_id = playlist.id();
        let mut cue_list = CueList::new("test");
        cue_list.push(Box::new(playlist));

        cue_list.set_playhead_for_selection(Some(ids[2])).unwrap();
        assert_eq!(cue_list.get(&playlist_id).unwrap().active_child_id(), Some(ids[2]));

        cue_list.set_playhead(Some(ids[1])).unwrap();
        assert_eq!(cue_list.get(&playlist_id).unwrap().active_child_id(), Some(ids[1]));

        let mut active = GroupCue::new();
        active.mode = GroupMode::Playlist;
        active.playlist_loop = true;
        let active_ids: Vec<_> = (0..3)
            .map(|_| {
                let child = WaitCue::new();
                let id = child.id();
                active.children.push(Box::new(child));
                id
            })
            .collect();
        active.state = CueState::Running;
        active.seq_current_id = Some(active_ids[0]);
        let active_id = active.id();
        let mut active_list = CueList::new("test");
        active_list.push(Box::new(active));
        active_list.set_playhead(Some(active_ids[2])).unwrap();
        assert_eq!(active_list.get(&active_id).unwrap().active_child_id(), Some(active_ids[2]));
    }

    #[test]
    fn rebuilt_completed_image_keeps_pending_playlist_deadline_and_stop_cancels_it() {
        let ctx = presentation_context();
        let mut image = ImageCue::new();
        image.mark_auto_continue_fired();
        image.restore_runtime_state(RuntimeState {
            state: CueState::Completed,
            voice_id: None,
            started_at: None,
            action_started_at: None,
        });
        let image_id = image.id();
        let next = WaitCue::new();
        let next_id = next.id();
        let mut group = GroupCue::new();
        group.mode = GroupMode::Playlist;
        group.state = CueState::Running;
        group.seq_current_id = Some(image_id);
        group.children.extend([Box::new(image) as Box<dyn Cue>, Box::new(next)]);
        let due = std::time::Instant::now() + Duration::from_secs(3);
        let pending = PendingGroupAdvance {
            due,
            source_id: image_id,
            source_generation: 0,
            expected_current_id: Some(image_id),
            require_marker: true,
            source_paused_at: None,
        };
        group.seq_post_wait_until = Some(pending);

        let original = group.children[0].as_ref();
        let extension = original.runtime_extension().unwrap();
        let runtime = original.runtime_state();
        let mut registry = crate::cue::registry::CueRegistry::new();
        registry.register(
            CueType::Image,
            Box::new(crate::cue::image_cue::ImageCueFactory),
        );
        let mut rebuilt = registry.from_json(original.serialize()).unwrap();
        rebuilt.restore_runtime_state(runtime);
        rebuilt.restore_runtime_extension(extension);
        group.children[0] = rebuilt;

        assert_eq!(group.seq_post_wait_until.unwrap().due, due);
        assert!(group.pending_advance_is_valid(pending));
        group.tick(&ctx).unwrap();
        assert_eq!(group.seq_post_wait_until.unwrap().due, due);
        assert!(!group.seq_done);

        group.seq_post_wait_until.as_mut().unwrap().due = std::time::Instant::now();
        group.tick(&ctx).unwrap();
        assert_eq!(group.seq_current_id, Some(next_id));
        assert_eq!(group.children[1].state(), CueState::Running);

        group.stop(&ctx).unwrap();
        assert!(group.seq_post_wait_until.is_none());
        assert_eq!(group.children[0].auto_continue_marker(), Some(false));
        assert_eq!(group.state, CueState::Standby);

        group.children[0].mark_auto_continue_fired();
        group.state = CueState::Running;
        group.seq_current_id = Some(image_id);
        group.seq_post_wait_until = Some(pending);
        group.reset().unwrap();
        assert!(group.seq_post_wait_until.is_none());
        assert_eq!(group.children[0].auto_continue_marker(), Some(false));
        assert_eq!(group.state, CueState::Standby);
    }
    use crate::engine::{
        audio_engine::AudioEngine,
        dmx_engine::DmxEngine,
        engine_traits::OutputEngineApi,
        output_engine::ContentRequest,
        ring_command::VoiceId,
    };
    use crate::preferences::MachineAudioConfig;
    use crossbeam_channel::unbounded;

    #[derive(Default)]
    struct NullOutput {
        started: std::sync::Mutex<Vec<VoiceId>>,
        stopped: std::sync::Mutex<Vec<VoiceId>>,
        requested_durations: std::sync::Mutex<Vec<Option<u64>>>,
        shown_paths: std::sync::Mutex<Vec<String>>,
        seeked: std::sync::Mutex<Vec<(VoiceId, u64)>>,
        visual_orders: std::sync::Mutex<Vec<(String, Option<u64>, Option<u64>)>>,
    }

    impl OutputEngineApi for NullOutput {
        fn show_content(&self, req: ContentRequest<'_>) -> anyhow::Result<VoiceId> {
            let id = Uuid::new_v4();
            self.started.lock().unwrap().push(id);
            self.requested_durations.lock().unwrap().push(req.display_duration_ms);
            self.shown_paths.lock().unwrap().push(req.file_path.to_string_lossy().into_owned());
            self.visual_orders.lock().unwrap().push((
                req.file_path.to_string_lossy().into_owned(),
                req.layer_group_order,
                req.layer_child_order,
            ));
            Ok(id)
        }
        fn stop_content(&self, voice_id: VoiceId, _visual_fade_ms: u32, _audio_fade_ms: u32) {
            self.stopped.lock().unwrap().push(voice_id);
        }
        fn hard_stop_current(&self) {}
        fn panic_stop(&self) {}
        fn video_audio_voice(&self, _voice_id: VoiceId) -> Option<VoiceId> { None }
        fn resync_audio_to_video(&self, _voice_id: VoiceId) {}
        fn get_voice_opacity(&self, _voice_id: VoiceId) -> f32 { 1.0 }
        fn set_voice_opacity(&self, _voice_id: VoiceId, _opacity: f32) {}
        fn stop_voice(&self, _voice_id: VoiceId, _fade_ms: u32) -> anyhow::Result<()> { Ok(()) }
        fn pause_voice(&self, _voice_id: VoiceId) -> anyhow::Result<()> { Ok(()) }
        fn resume_voice(&self, _voice_id: VoiceId) -> anyhow::Result<()> { Ok(()) }
        fn seek_voice_ms(&self, _voice_id: VoiceId, _position_ms: u64) {}
        fn show_text_overlay(&self, _ass_text: &str, _screen_index: Option<u32>) {}
        fn clear_text_overlay(&self) {}
        fn begin_eof_fade_out(&self, _voice_id: VoiceId, _fade_ms: u32) -> bool { false }
        fn devamp_voice(&self, _voice_id: VoiceId, _stop_at_end: bool) {}
        fn start_preloaded(&self, _voice_id: VoiceId) -> bool { false }
        fn seek_voice_action_ms(&self, voice_id: VoiceId, position_ms: u64) {
            self.seeked.lock().unwrap().push((voice_id, position_ms));
        }
    }

    fn presentation_context() -> CueContext {
        presentation_context_with_output().0
    }

    fn presentation_context_with_output() -> (CueContext, std::sync::Arc<NullOutput>) {
        let (events, _receiver) = unbounded();
        let output = std::sync::Arc::new(NullOutput::default());
        let context = CueContext::new(
            AudioEngine::new_silent(&MachineAudioConfig::default()),
            output.clone(), events, 0, Vec::new(), None, None,
            Vec::new(), std::sync::Arc::new(DmxEngine::new()), Vec::new(), Vec::new(),
            Vec::new(), 256,
        );
        (context, output)
    }

    fn three_slide_presentation() -> (GroupCue, CueId, CueId, CueId) {
        let mut presentation = GroupCue::new_presentation();
        let first = WaitCue::new();
        let first_id = first.id();
        let mut disabled = WaitCue::new();
        disabled.set_disabled(true);
        let disabled_id = disabled.id();
        let last = WaitCue::new();
        let last_id = last.id();
        presentation.children = vec![Box::new(first), Box::new(disabled), Box::new(last)];
        (presentation, first_id, disabled_id, last_id)
    }

    #[test]
    fn presentation_manual_step_skips_disabled_and_clamps_at_ends() {
        let ctx = presentation_context();
        let (mut presentation, first_id, disabled_id, last_id) = three_slide_presentation();
        presentation.go(&ctx).unwrap();
        assert_eq!(presentation.presentation_current_child_id(), Some(first_id));

        presentation.step_presentation(&ctx, "previous").unwrap();
        assert_eq!(presentation.presentation_current_child_id(), Some(first_id));
        presentation.step_presentation(&ctx, "next").unwrap();
        assert_eq!(presentation.presentation_current_child_id(), Some(last_id));
        assert_eq!(presentation.children[1].state(), CueState::Standby);
        presentation.step_presentation(&ctx, "next").unwrap();
        assert_eq!(presentation.presentation_current_child_id(), Some(last_id));
        presentation.step_presentation(&ctx, "previous").unwrap();
        assert_eq!(presentation.presentation_current_child_id(), Some(first_id));
        assert_ne!(first_id, disabled_id);
    }

    #[test]
    fn running_presentation_step_keeps_as_slide_background_paused() {
        let (ctx, output) = presentation_context_with_output();
        let mut presentation = GroupCue::new_presentation();
        let mut image = ImageCue::new();
        image.file_path = Some(std::path::PathBuf::from("image.png"));
        let image_id = image.id();
        let mut video = VideoCue::new();
        video.file_path = Some(std::path::PathBuf::from("video.mp4"));
        video.set_pre_wait(Duration::from_secs(60));
        video.presentation_video_imported = true;
        video.presentation_video_mode = crate::cue::types::PresentationVideoMode::AsSlide;
        video.presentation_video_layout_supported = true;
        video.presentation_video_layout = Some(crate::cue::types::PresentationVideoLayout {
            slide_width: 100,
            slide_height: 100,
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            rotation: 0,
            crop_left: 0.0,
            crop_top: 0.0,
            crop_right: 0.0,
            crop_bottom: 0.0,
        });
        let video_id = video.id();
        presentation.children.push(Box::new(image));
        presentation.children.push(Box::new(video));
        presentation.go(&ctx).unwrap();

        presentation.step_presentation(&ctx, "next").unwrap();

        assert_eq!(presentation.presentation_current_child_id(), Some(video_id));
        assert_eq!(presentation.children[0].state(), CueState::Paused);
        assert_eq!(presentation.presentation_retained_background_id, Some(image_id));

        // Backward navigation restarts the retained image. Every replaced
        // output handle must be stopped exactly once across repeated cycles.
        presentation.step_presentation(&ctx, "previous").unwrap();
        assert_eq!(presentation.presentation_current_child_id(), Some(image_id));
        assert_eq!(output.started.lock().unwrap().len(), 2);
        assert_eq!(output.stopped.lock().unwrap().len(), 1);
        presentation.step_presentation(&ctx, "next").unwrap();
        presentation.step_presentation(&ctx, "previous").unwrap();
        presentation.stop(&ctx).unwrap();
        assert_eq!(output.started.lock().unwrap().len(), 3);
        assert_eq!(output.stopped.lock().unwrap().len(), 3);
    }

    #[test]
    fn presentation_step_keeps_parent_paused_and_clears_pending_advance() {
        let ctx = presentation_context();
        let (mut presentation, _first_id, _disabled_id, last_id) = three_slide_presentation();
        presentation.go(&ctx).unwrap();
        let first_id = presentation.seq_current_id.unwrap();
        presentation.seq_post_wait_until = Some(PendingGroupAdvance {
            due: Instant::now() + Duration::from_secs(30),
            source_id: first_id,
            source_generation: 0,
            expected_current_id: Some(first_id),
            require_marker: false,
            source_paused_at: None,
        });
        presentation.pause(&ctx).unwrap();

        presentation.step_presentation(&ctx, "next").unwrap();
        assert_eq!(presentation.state(), CueState::Paused);
        assert_eq!(presentation.children[2].state(), CueState::Paused);
        assert_eq!(presentation.seq_current_id, Some(last_id));
        assert!(presentation.seq_post_wait_until.is_none());

        presentation.resume(&ctx).unwrap();
        assert_eq!(presentation.state(), CueState::Running);
        assert_eq!(presentation.children[2].state(), CueState::Running);
    }

    #[test]
    fn presentation_step_rejects_stopped_presentation_and_ordinary_group() {
        let ctx = presentation_context();
        let (mut presentation, _first_id, _disabled_id, _last_id) = three_slide_presentation();
        assert!(presentation.step_presentation(&ctx, "next").is_err());
        let mut group = GroupCue::new();
        group.state = CueState::Running;
        assert!(group.step_presentation(&ctx, "next").is_err());
    }

    #[test]
    fn presentation_navigation_is_blocked_during_parent_prewait_even_when_paused() {
        let ctx = presentation_context();
        let mut presentation = GroupCue::new_presentation();
        let first = MemoCue::new();
        let second = MemoCue::new();
        let second_id = second.id();
        presentation.children.push(Box::new(first));
        presentation.children.push(Box::new(second));
        assert!(presentation.set_active_child(&second_id));
        presentation.set_pre_wait(Duration::from_secs(60));
        presentation.go(&ctx).unwrap();
        assert_eq!(presentation.presentation_current_child_id(), None);

        presentation.pause(&ctx).unwrap();
        assert!(presentation.step_presentation(&ctx, "next").is_err());
        assert_eq!(presentation.presentation_current_child_id(), None);
        assert!(presentation.children.iter().all(|child| child.state() == CueState::Standby));
    }

    #[test]
    fn number_post_wait_progress_freezes_while_paused_and_clears_at_deadline() {
        let mut number = GroupCue::new_number();
        number.number_master_finished = true;
        number.post_wait = Duration::from_secs(1);

        // The deadline includes a one-second fade prefix. During that prefix,
        // the authored Post-Wait bar remains at zero.
        let fade_started = Instant::now();
        number.number_post_wait_until = Some(fade_started + Duration::from_secs(2));
        let (cue_id, elapsed, total) = number.pending_child_post_wait().unwrap();
        assert_eq!(cue_id, number.id);
        assert_eq!(elapsed, Duration::ZERO);
        assert_eq!(total, Duration::from_secs(1));

        // Freeze halfway through Post-Wait. Repeated reads use paused_at, not
        // wall clock, so the displayed progress does not move while paused.
        let paused_at = Instant::now();
        number.paused_at = Some(paused_at);
        number.number_post_wait_until = Some(paused_at + Duration::from_millis(500));
        let first = number.pending_child_post_wait().unwrap();
        let second = number.pending_child_post_wait().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.1, Duration::from_millis(500));

        number.number_post_wait_until = None;
        assert!(number.pending_child_post_wait().is_none());
    }

    #[test]
    fn number_playlist_loop_restarts_the_graph_after_post_wait_and_preserves_offsets() {
        let ctx = presentation_context();
        let mut number = GroupCue::new_number();
        let mut master = GroupCue::new();
        master.mode = GroupMode::Sequential;
        let mut master_first = WaitCue::new();
        master_first.set_user_action_duration(Some(Duration::from_secs(1))).unwrap();
        let mut master_second = WaitCue::new();
        master_second.set_user_action_duration(Some(Duration::from_secs(1))).unwrap();
        master.children = vec![Box::new(master_first), Box::new(master_second)];
        let master_id = master.id();
        let mut first = ImageCue::new();
        first.file_path = Some(std::path::PathBuf::from("number-first.png"));
        first.display_duration_ms = Some(2_000);
        let first_id = first.id();
        let mut delayed = ImageCue::new();
        delayed.file_path = Some(std::path::PathBuf::from("number-delayed.png"));
        delayed.display_duration_ms = Some(2_000);
        let delayed_id = delayed.id();
        let finish_target = Uuid::new_v4();
        number.children = vec![Box::new(master), Box::new(first), Box::new(delayed)];
        number.master_child_id = Some(master_id);
        number.action_offsets_ms.insert(first_id, 0);
        number.action_offsets_ms.insert(delayed_id, 500);
        number.number_finish_start_ids.push(finish_target);
        number.post_wait = Duration::from_millis(20);
        number.set_playlist_loop(true);

        number.go(&ctx).unwrap();
        let number_generation = number.play_generation();
        let master_generation = number.children[0].play_generation();
        let first_generation = number.children[1].play_generation();
        number.take_fired_cue_ids();
        assert_eq!(number.children[0].state(), CueState::Running);
        assert_eq!(number.children[1].state(), CueState::Running);
        assert_eq!(number.children[2].state(), CueState::Standby);

        // Advance only the Number clock to the end of this master pass.
        number.action_started_at = Some(Instant::now() - Duration::from_secs(3));
        number.tick_number(&ctx).unwrap();
        assert!(number.number_master_finished);
        assert!(!number.is_complete(), "looping Numbers do not complete at master EOF");
        assert_eq!(number.automatic_completion_duration(), None);
        assert!(number.take_number_completion_start_ids().is_empty());
        assert_eq!(number.number_completion_start_ids, vec![finish_target]);
        assert_eq!(number.playlist_loop(), Some(true));
        assert_eq!(number.serialize()["playlist_loop"], true);

        // Pause across the original deadline. Resume must shift that deadline;
        // the next tick must not start a pass until the shifted Post-Wait ends.
        number.pause(&ctx).unwrap();
        let paused_deadline = number.number_post_wait_until.unwrap();
        std::thread::sleep(Duration::from_millis(25));
        number.tick(&ctx).unwrap();
        assert_eq!(number.children[0].play_generation(), master_generation);
        number.resume(&ctx).unwrap();
        assert!(number.number_post_wait_until.unwrap() > Instant::now());
        assert!(number.number_post_wait_until.unwrap() > paused_deadline);
        assert_eq!(number.children[0].play_generation(), master_generation);

        // Let the resumed post-wait expire without another timed sleep.
        number.number_post_wait_until = Some(Instant::now() - Duration::from_millis(1));
        number.tick_number(&ctx).unwrap();
        assert!(number.is_running());
        assert!(!number.number_master_finished);
        assert!(number.action_elapsed() <= Duration::from_millis(5));
        assert_eq!(number.play_generation(), number_generation, "internal passes are not new external GO operations");
        assert_eq!(number.children[0].state(), CueState::Running);
        assert_eq!(number.children[1].state(), CueState::Running);
        assert_eq!(number.children[2].state(), CueState::Standby, "the delayed action keeps its saved offset");
        let master_children = number.children[0].child_cues().unwrap();
        assert_eq!(master_children[0].state(), CueState::Running, "a Group master starts at its first child each pass");
        assert_eq!(master_children[1].state(), CueState::Standby);
        assert_eq!(number.action_offsets_ms.get(&delayed_id), Some(&500));
        assert!(number.children[0].play_generation() > master_generation);
        assert!(number.children[1].play_generation() > first_generation);
        let fired = number.take_fired_cue_ids();
        assert!(fired.contains(&master_id) && fired.contains(&first_id));
        assert!(!fired.contains(&finish_target), "finish targets are not re-fired on an internal pass");

        number.stop(&ctx).unwrap();
        assert_eq!(number.state(), CueState::Standby);
        assert_eq!(number.children[0].state(), CueState::Standby);
        assert_eq!(number.playlist_loop(), Some(true), "Stop clears runtime but keeps the saved loop setting");
    }

    #[test]
    fn number_audio_master_loop_survives_forward_backward_seek_and_stop() {
        let (ctx, output) = presentation_context_with_output();
        let mut number = GroupCue::new_number();
        let mut master = AudioCue::new();
        master.accept_preloaded_audio(
            std::sync::Arc::new(vec![0.0; 192_000]),
            2,
            48_000,
            Duration::from_secs(2),
        );
        let master_id = master.id();
        let mut action = ImageCue::new();
        action.file_path = Some(std::path::PathBuf::from("number-seek-action.png"));
        action.display_duration_ms = Some(2_000);
        let action_id = action.id();
        number.children = vec![Box::new(master), Box::new(action)];
        number.master_child_id = Some(master_id);
        number.action_offsets_ms.insert(action_id, 500);
        number.post_wait = Duration::from_millis(5);
        number.set_playlist_loop(true);
        number.go(&ctx).unwrap();
        let anchor = number.visual_group_order.expect("Number reserves order on GO");
        let parent_generation = number.play_generation();
        let master_generation = number.children[0].play_generation();

        number.seek_number(600, &ctx);
        assert_eq!(number.children[1].state(), CueState::Running);
        assert_eq!(output.visual_orders.lock().unwrap().last().unwrap().1, Some(anchor));
        number.seek_number(100, &ctx);
        assert_eq!(number.children[1].state(), CueState::Standby);
        assert_eq!(number.playlist_loop(), Some(true));

        number.action_started_at = Some(Instant::now() - Duration::from_secs(3));
        number.tick_number(&ctx).unwrap();
        assert!(number.number_master_finished);
        number.number_post_wait_until = Some(Instant::now() - Duration::from_millis(1));
        number.tick_number(&ctx).unwrap();

        assert_eq!(number.play_generation(), parent_generation);
        assert!(number.children[0].play_generation() > master_generation);
        assert_eq!(number.children[0].state(), CueState::Running);
        assert_eq!(number.visual_group_order, Some(anchor), "internal loop keeps the execution anchor");
        assert_eq!(number.children[1].state(), CueState::Standby, "seek did not erase the action's authored offset");
        assert_eq!(number.action_offsets_ms.get(&action_id), Some(&500));

        number.stop(&ctx).unwrap();
        assert_eq!(number.state(), CueState::Standby);
        assert!(number.children.iter().all(|child| child.state() == CueState::Standby));
        assert_eq!(number.visual_group_order, None, "Stop releases the old scope");
        number.go(&ctx).unwrap();
        assert_ne!(number.visual_group_order, Some(anchor), "a new GO gets a new visual priority");
    }

    #[test]
    fn number_loop_off_during_post_wait_completes_and_emits_finish_targets_once() {
        let ctx = presentation_context();
        let mut number = GroupCue::new_number();
        let finish_target = Uuid::new_v4();
        number.state = CueState::Running;
        number.number_master_finished = true;
        number.number_post_wait_until = Some(Instant::now() + Duration::from_secs(1));
        number.number_completion_start_ids.push(finish_target);
        number.set_playlist_loop(true);

        assert!(number.take_number_completion_start_ids().is_empty());
        assert_eq!(number.number_completion_start_ids, vec![finish_target]);
        number.set_playlist_loop(false);
        assert_eq!(number.automatic_completion_duration(), None, "Post-Wait deadline is authoritative after loop is disabled");
        assert!(!number.is_complete());

        number.number_post_wait_until = Some(Instant::now() - Duration::from_millis(1));
        number.tick_number(&ctx).unwrap();
        assert!(number.is_complete());
        assert_eq!(number.take_number_completion_start_ids(), vec![finish_target]);
        assert!(number.take_number_completion_start_ids().is_empty());
    }


    #[test]
    fn number_seek_action_windows_respect_offsets_duration_and_master_end() {
        assert!(number_action_window_active(1_000, 1_000, Some(500), Some(5_000)));
        assert!(!number_action_window_active(1_500, 1_000, Some(500), Some(5_000)));
        assert!(!number_action_window_active(999, 1_000, Some(500), Some(5_000)));
        assert!(number_action_window_active(4_999, 1_000, None, Some(5_000)));
        assert!(!number_action_window_active(5_000, 1_000, None, Some(5_000)));
        assert_eq!(number_seek_target(9_000, Some(5_000)), 5_000);
    }

    #[test]
    fn number_seek_includes_legacy_actions_without_a_saved_offset() {
        // Recovery data showed that Number #2 worked because it persisted
        // `video_id: 0`, while #1 and #3 omitted that legacy-default entry.
        // Both forms must keep the video due during a seek.
        let mut number = GroupCue::new_number();
        let master = AudioCue::new();
        let master_id = master.id();
        let action = VideoCue::new();
        let action_id = action.id();
        number.children.push(Box::new(master));
        number.children.push(Box::new(action));
        number.master_child_id = Some(master_id);

        let offsets = number.number_seek_action_offsets(0);
        assert_eq!(offsets, vec![(action_id, 0)]);
        assert!(number_action_window_active(8_000, offsets[0].1, None, Some(10_000)));
    }

    #[test]
    fn number_seek_rebuilds_active_windows_forward_and_backward_without_relaunching_finished_actions() {
        let ctx = presentation_context();
        let mut number = GroupCue::new_number();
        let mut master = AudioCue::new();
        master.accept_preloaded_audio(
            std::sync::Arc::new(vec![0.0; 192_000]),
            2,
            48_000,
            Duration::from_secs(2),
        );
        let master_id = master.id();
        master.go(&ctx).unwrap();

        let mut finished = WaitCue::new();
        finished.set_user_action_duration(Some(Duration::from_millis(100))).unwrap();
        let finished_id = finished.id();
        let mut first = WaitCue::new();
        first.set_user_action_duration(Some(Duration::from_millis(200))).unwrap();
        let first_id = first.id();
        let mut second = WaitCue::new();
        second.set_user_action_duration(Some(Duration::from_millis(200))).unwrap();
        let second_id = second.id();

        number.children = vec![Box::new(master), Box::new(finished), Box::new(first), Box::new(second)];
        number.master_child_id = Some(master_id);
        number.action_offsets_ms.insert(finished_id, 0);
        number.action_offsets_ms.insert(first_id, 200);
        number.action_offsets_ms.insert(second_id, 500);
        number.state = CueState::Running;

        number.seek_number(250, &ctx);
        assert_eq!(number.children[2].state(), CueState::Running);
        assert_eq!(number.children[3].state(), CueState::Standby);
        assert_eq!(number.children[1].state(), CueState::Standby);
        let first_generation = number.children[2].play_generation();
        assert!(number.number_launched_actions.contains(&finished_id));

        number.tick_number(&ctx).unwrap();
        assert_eq!(number.children[1].play_generation(), 0, "finished window must not be launched on the next tick");
        assert_eq!(number.children[2].play_generation(), first_generation);

        number.seek_number(550, &ctx);
        assert_eq!(number.children[2].state(), CueState::Standby);
        assert_eq!(number.children[3].state(), CueState::Running);
        assert!(number.number_launched_actions.contains(&first_id));
        assert!(number.number_launched_actions.contains(&second_id));
        let second_generation = number.children[3].play_generation();
        number.tick_number(&ctx).unwrap();
        assert_eq!(number.children[2].play_generation(), first_generation, "a finished window must not relaunch on the next tick");
        assert_eq!(number.children[3].play_generation(), second_generation);

        number.seek_number(250, &ctx);
        assert_eq!(number.children[2].state(), CueState::Running);
        assert_eq!(number.children[3].state(), CueState::Standby);
        assert!(number.number_launched_actions.contains(&finished_id));
    }

    #[test]
    fn number_seek_preserves_paused_state_and_repositions_nested_group_timeline() {
        let ctx = presentation_context();
        let mut number = GroupCue::new_number();
        let mut master = WaitCue::new();
        master.set_user_action_duration(Some(Duration::from_secs(2))).unwrap();
        let master_id = master.id();
        master.go(&ctx).unwrap();

        let mut action = GroupCue::new();
        action.mode = GroupMode::Sequential;
        let mut first = WaitCue::new();
        first.set_user_action_duration(Some(Duration::from_millis(100))).unwrap();
        let mut second = WaitCue::new();
        second.set_user_action_duration(Some(Duration::from_millis(100))).unwrap();
        let second_id = second.id();
        action.children = vec![Box::new(first), Box::new(second)];
        let action_id = action.id();
        let nested_real_go = Uuid::new_v4();
        action.fired_child_ids.push(nested_real_go);

        number.children = vec![Box::new(master), Box::new(action)];
        number.master_child_id = Some(master_id);
        number.action_offsets_ms.insert(action_id, 0);
        number.state = CueState::Paused;
        number.seek_number(150, &ctx);

        assert_eq!(number.take_fired_cue_ids(), vec![nested_real_go], "nested real GO waits survive timeline reconstruction");
        assert_eq!(number.state, CueState::Paused);
        assert_eq!(number.children[1].state(), CueState::Paused);
        assert!((Duration::from_millis(150)..=Duration::from_millis(155))
            .contains(&number.children[1].action_elapsed()));
        assert_eq!(number.children[1].child_cues().unwrap()[1].id(), second_id);
        assert_eq!(number.children[1].child_cues().unwrap()[1].state(), CueState::Paused);

        number.seek_number(50, &ctx);
        assert_eq!(number.children[1].action_elapsed(), Duration::from_millis(50));
        assert_eq!(number.children[1].child_cues().unwrap()[0].state(), CueState::Paused);
        assert_eq!(number.children[1].child_cues().unwrap()[1].state(), CueState::Standby);

        number.seek_number(150, &ctx);
        assert_eq!(number.children[1].action_elapsed(), Duration::from_millis(150));
        assert_eq!(number.children[1].child_cues().unwrap()[1].state(), CueState::Paused);
        assert!(number.take_fired_cue_ids().is_empty(), "seek-generated nested GO IDs must not leak on later ticks");
    }

    #[test]
    fn simultaneous_group_seek_keeps_indefinite_image_active_after_offset() {
        let (ctx, _) = presentation_context_with_output();
        let mut number = GroupCue::new_number();
        let mut master = WaitCue::new();
        master.set_user_action_duration(Some(Duration::from_secs(2))).unwrap();
        let master_id = master.id();
        master.go(&ctx).unwrap();

        let mut image = ImageCue::new();
        image.file_path = Some(std::path::PathBuf::from("indefinite.png"));
        let mut action = GroupCue::new();
        action.mode = GroupMode::Simultaneous;
        action.children.push(Box::new(image));
        let action_id = action.id();
        number.children = vec![Box::new(master), Box::new(action)];
        number.master_child_id = Some(master_id);
        number.action_offsets_ms.insert(action_id, 0);
        number.state = CueState::Running;

        number.seek_number(500, &ctx);

        let action = number.children[1].as_ref();
        assert_eq!(action.state(), CueState::Running);
        assert!((Duration::from_millis(500)..=Duration::from_millis(505))
            .contains(&action.action_elapsed()));
        assert_eq!(action.child_cues().unwrap()[0].state(), CueState::Running);
    }

    #[test]
    fn number_seek_repositions_finite_image_clock_and_keeps_indefinite_image_active() {
        let (ctx, output) = presentation_context_with_output();
        let mut number = GroupCue::new_number();
        let mut master = WaitCue::new();
        master.set_user_action_duration(Some(Duration::from_secs(2))).unwrap();
        let master_id = master.id();
        master.go(&ctx).unwrap();

        let mut image = ImageCue::new();
        image.file_path = Some(std::path::PathBuf::from("number.png"));
        image.display_duration_ms = Some(40);
        let image_id = image.id();
        let mut hold = ImageCue::new();
        hold.file_path = Some(std::path::PathBuf::from("hold.png"));
        let hold_id = hold.id();

        number.children = vec![Box::new(master), Box::new(image), Box::new(hold)];
        number.master_child_id = Some(master_id);
        number.action_offsets_ms.insert(image_id, 100);
        number.action_offsets_ms.insert(hold_id, 300);
        number.state = CueState::Running;

        number.seek_number(110, &ctx);
        assert_eq!(number.children[1].state(), CueState::Running);
        assert!(number.children[1].action_elapsed() >= Duration::from_millis(10));
        assert_eq!(output.requested_durations.lock().unwrap().as_slice(), &[None]);

        std::thread::sleep(Duration::from_millis(45));
        number.tick_number(&ctx).unwrap();
        assert_eq!(number.children[1].state(), CueState::Standby, "expired nested image must be reaped by Number");
        assert!(!number.children[1].has_retained_output());

        number.seek_number(110, &ctx);
        assert_eq!(number.children[1].state(), CueState::Running, "rewind re-enters the image window");
        let image_generation = number.children[1].play_generation();
        number.children[1].set_disabled(true);
        std::thread::sleep(Duration::from_millis(45));
        number.tick_number(&ctx).unwrap();
        assert_eq!(number.children[1].state(), CueState::Standby, "a running image still expires after it is disarmed");
        number.seek_number(110, &ctx);
        assert_eq!(number.children[1].state(), CueState::Standby, "a disabled image is not launched by seek");
        assert_eq!(number.children[1].play_generation(), image_generation);
        number.seek_number(310, &ctx);
        assert_eq!(number.children[1].state(), CueState::Standby);
        assert_eq!(number.children[2].state(), CueState::Running);
        std::thread::sleep(Duration::from_millis(45));
        number.tick_number(&ctx).unwrap();
        assert_eq!(number.children[2].state(), CueState::Running, "indefinite image must not expire by clock");
    }

    #[test]
    fn number_seek_repositions_video_within_window_and_rebuilds_windows_forward_and_back() {
        let (ctx, output) = presentation_context_with_output();
        let mut number = GroupCue::new_number();
        let mut master = WaitCue::new();
        master.set_user_action_duration(Some(Duration::from_secs(2))).unwrap();
        let master_id = master.id();
        master.go(&ctx).unwrap();

        let first_path = "first.mp4".to_string();
        let second_path = "second.mp4".to_string();
        let mut first_json = VideoCue::new().serialize();
        first_json["file_path"] = serde_json::json!(first_path.clone());
        first_json["cached_duration_ms"] = serde_json::json!(300);
        let mut first = VideoCueFactory.from_json(first_json).unwrap();
        first.accept_preloaded_audio(
            std::sync::Arc::new(vec![0.0; 480]),
            1,
            48_000,
            Duration::from_millis(10),
        );
        let first_id = first.id();
        let mut second_json = VideoCue::new().serialize();
        second_json["file_path"] = serde_json::json!(second_path.clone());
        second_json["cached_duration_ms"] = serde_json::json!(300);
        let mut second = VideoCueFactory.from_json(second_json).unwrap();
        second.accept_preloaded_audio(
            std::sync::Arc::new(vec![0.0; 480]),
            1,
            48_000,
            Duration::from_millis(10),
        );
        let second_id = second.id();

        number.children = vec![Box::new(master), first, second];
        number.master_child_id = Some(master_id);
        number.set_number_action_offset(first_id, 100).unwrap();
        number.set_number_action_offset(second_id, 500).unwrap();
        number.state = CueState::Running;
        let pending_real_go = Uuid::new_v4();
        number.fired_child_ids.push(pending_real_go);

        number.seek_number(150, &ctx);
        assert_eq!(number.take_fired_cue_ids(), vec![pending_real_go], "seek preserves already queued real GO notifications");
        assert_eq!(number.children[1].state(), CueState::Running);
        assert_eq!(number.children[2].state(), CueState::Standby);
        let first_voice = number.children[1].playing_voice_id().unwrap();
        assert_eq!(output.seeked.lock().unwrap().last(), Some(&(first_voice, 50)));

        number.seek_number(200, &ctx);
        assert_eq!(output.seeked.lock().unwrap().last(), Some(&(first_voice, 100)));
        assert_eq!(output.shown_paths.lock().unwrap().iter().filter(|path| *path == &first_path).count(), 1,
            "seeking inside an active action must not reload the video");

        number.seek_number(550, &ctx);
        assert_eq!(number.children[1].state(), CueState::Standby);
        assert_eq!(number.children[2].state(), CueState::Running);
        assert!(number.take_fired_cue_ids().is_empty(), "seek must not publish synthetic child GO notifications");
        let second_voice = number.children[2].playing_voice_id().unwrap();
        assert_eq!(output.seeked.lock().unwrap().last(), Some(&(second_voice, 50)));

        number.seek_number(200, &ctx);
        assert_eq!(number.children[1].state(), CueState::Running);
        assert_eq!(number.children[2].state(), CueState::Standby);
        assert_eq!(output.shown_paths.lock().unwrap().iter().filter(|path| *path == &first_path).count(), 2,
            "rewinding into a finished window must start it again");
        number.tick_number(&ctx).unwrap();
        assert!(number.take_fired_cue_ids().is_empty(), "synthetic GO IDs must not leak on the next Number tick");
    }

    #[test]
    fn nested_group_delayed_video_keeps_number_order_scope() {
        let (ctx, output) = presentation_context_with_output();
        let mut master = AudioCue::new();
        master.accept_preloaded_audio(
            std::sync::Arc::new(vec![0.0; 192_000]),
            2,
            48_000,
            Duration::from_secs(2),
        );
        let master_id = master.id();

        let mut video_json = VideoCue::new().serialize();
        video_json["file_path"] = serde_json::json!("delayed-grandchild.mp4");
        video_json["cached_duration_ms"] = serde_json::json!(500);
        let mut video = VideoCueFactory.from_json(video_json).unwrap();
        video.accept_preloaded_audio(
            std::sync::Arc::new(vec![0.0; 48_000]),
            2,
            48_000,
            Duration::from_millis(500),
        );
        video.set_pre_wait(Duration::from_millis(5));
        let mut nested = GroupCue::new();
        nested.children.push(video);
        let nested_id = nested.id();

        let mut number = GroupCue::new_number();
        number.children = vec![Box::new(master), Box::new(nested)];
        number.set_number_master(master_id).unwrap();
        number.set_number_action_offset(nested_id, 0).unwrap();
        number.go(&ctx).unwrap();
        let anchor = number.visual_group_order.expect("Number reserves order on GO");
        assert_eq!(number.children[1].state(), CueState::Running);
        assert_eq!(number.children[1].child_cues().unwrap()[0].state(), CueState::Running);
        std::thread::sleep(Duration::from_millis(10));
        number.tick(&ctx).unwrap();

        let orders = output.visual_orders.lock().unwrap();
        let delayed = orders.iter().find(|(path, _, _)| path == "delayed-grandchild.mp4").unwrap();
        assert_eq!(delayed.1, Some(anchor));
    }

    #[test]
    fn number_seek_plan_preserves_requested_pause_state() {
        assert_eq!(number_seek_state(true), CueState::Paused);
        assert_eq!(number_seek_state(false), CueState::Running);
    }

    #[test]
    fn number_seek_does_not_apply_shared_fade() {
        // Scrubbing repositions an already-running Number. It must not restart
        // the authored fade-in envelope.
        assert!(number_seek_shared_fade().is_none());
    }

    #[test]
    fn number_readiness_rejects_missing_master() {
        let number = GroupCue::new_number();
        let issues = number_readiness_issues(&number);
        assert_eq!(issues, vec!["Number needs a master Audio, Video, or Group cue"]);
    }

    #[test]
    fn number_readiness_rejects_disabled_master() {
        let path = std::env::temp_dir().join(format!("qlisa-number-disabled-{}.wav", Uuid::new_v4()));
        std::fs::write(&path, b"test").unwrap();
        let mut number = GroupCue::new_number();
        let mut master = AudioCue::new();
        master.file_path = Some(path.clone());
        master.set_disabled(true);
        let master_id = master.id();
        number.master_child_id = Some(master_id);
        number.children.push(Box::new(master));
        let issues = number_readiness_issues(&number);
        assert!(issues.iter().any(|issue| issue == "Number master is disabled"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn number_action_tick_failure_has_operator_facing_context() {
        assert_eq!(
            number_action_runtime_error("Image", "during playback", "surface stopped"),
            "Number action 'Image' failed during playback: surface stopped",
        );
    }

    #[test]
    fn number_v1_media_actions_include_image_and_reject_light_or_browser() {
        assert!(number_action_type_supported(CueType::Audio));
        assert!(number_action_type_supported(CueType::Video));
        assert!(number_action_type_supported(CueType::Image));
        assert!(number_action_type_supported(CueType::Group));
        assert!(!number_action_type_supported(CueType::Light));
        assert!(!number_action_type_supported(CueType::Browser));
        assert!(number_action_is_temporary_media(CueType::Image));
        assert!(!number_action_is_temporary_media(CueType::Light));
    }

    #[test]
    fn number_readiness_reports_unconfigured_image_action() {
        let mut number = GroupCue::new_number();
        let master = AudioCue::new();
        let master_id = master.id();
        number.master_child_id = Some(master_id);
        number.children.push(Box::new(master));
        number.children.push(Box::new(ImageCue::new()));
        let issues = number_readiness_issues(&number);
        assert!(issues.iter().any(|issue| issue.contains("action") && issue.contains("existing media file")));
    }

    #[test]
    fn number_readiness_accepts_existing_master_and_checks_actions() {
        let path = std::env::temp_dir().join(format!("qlisa-number-readiness-{}.wav", Uuid::new_v4()));
        std::fs::write(&path, b"test").unwrap();
        let mut number = GroupCue::new_number();
        let mut master = AudioCue::new();
        master.file_path = Some(path.clone());
        let master_id = master.id();
        number.master_child_id = Some(master_id);
        number.children.push(Box::new(master));
        assert!(number_readiness_issues(&number).is_empty());
        number.children.push(Box::new(ImageCue::new()));
        assert!(number_readiness_issues(&number).iter().any(|issue| issue.contains("action") && issue.contains("existing media file")));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn number_readiness_reports_legacy_unsupported_children() {
        let path = std::env::temp_dir().join(format!("qlisa-number-unsupported-{}.wav", Uuid::new_v4()));
        std::fs::write(&path, b"test").unwrap();
        let mut number = GroupCue::new_number();
        let mut master = AudioCue::new();
        master.file_path = Some(path.clone());
        let master_id = master.id();
        number.master_child_id = Some(master_id);
        number.children.push(Box::new(master));
        number.children.push(Box::new(LightCue::new()));
        number.children.push(Box::new(BrowserCue::new()));
        let issues = number_readiness_issues(&number);
        assert_eq!(issues.iter().filter(|issue| issue.contains("unsupported type")).count(), 2);
        let _ = std::fs::remove_file(path);
    }

    /// A Sequential group with `n` memo children, marked Running.
    fn running_seq_group(n: usize) -> GroupCue {
        let mut g = GroupCue::new();
        g.mode = GroupMode::Sequential;
        for _ in 0..n {
            g.children.push(Box::new(MemoCue::new()));
        }
        g.state = CueState::Running;
        g
    }

    #[test]
    fn absorbs_and_holds_while_more_children_remain() {
        let mut g = running_seq_group(3);
        // Inner playhead on the first child → another child remains.
        g.seq_current_id = Some(g.children[0].id());
        assert!(g.absorbs_go());
        assert!(g.holds_playhead());
        assert!(!g.released_playhead());
    }

    #[test]
    fn releases_playhead_on_last_child() {
        let mut g = running_seq_group(3);
        g.seq_current_id = Some(g.children[2].id()); // last child fired
        assert!(!g.absorbs_go());
        assert!(!g.holds_playhead());
        assert!(g.released_playhead());
    }

    #[test]
    fn timer_hold_at_zero_keeps_group_alive_after_finite_sibling_duration() {
        let mut timer = TimerCue::new();
        timer.set_user_action_duration(Some(Duration::ZERO)).unwrap();
        timer.restore_runtime_state(RuntimeState { state: CueState::Running, voice_id: None, started_at: None, action_started_at: None });

        let mut wait = WaitCue::new();
        wait.set_user_action_duration(Some(Duration::from_secs(1))).unwrap();

        let mut group = GroupCue::new();
        group.state = CueState::Running;
        group.action_started_at = Some(Instant::now() - Duration::from_secs(2));
        group.children.push(Box::new(timer));
        group.children.push(Box::new(wait));
        assert_eq!(group.duration(), Some(Duration::from_secs(1)));
        assert!(group.action_elapsed() >= Duration::from_secs(1));
        assert_eq!(group.automatic_completion_duration(), None);
        assert!(!group.is_complete());

        let mut nested = GroupCue::new();
        nested.children.push(group.children.remove(0));
        nested.children.push(Box::new(WaitCue::new()));
        let mut parent = GroupCue::new();
        parent.children.push(Box::new(nested));
        parent.children.push(Box::new(WaitCue::new()));
        assert_eq!(parent.automatic_completion_duration(), None);
    }

    #[test]
    fn single_child_group_releases_immediately() {
        let mut g = running_seq_group(1);
        g.seq_current_id = Some(g.children[0].id());
        assert!(!g.holds_playhead());
        assert!(g.released_playhead());
        assert!(!g.absorbs_go());
    }

    #[test]
    fn holds_playhead_during_pre_wait() {
        let mut g = running_seq_group(2);
        g.in_pre_wait = true;
        g.seq_current_id = None;
        assert!(g.holds_playhead());
        assert!(!g.released_playhead());
        assert!(!g.absorbs_go()); // a GO during pre-wait is not absorbed
    }

    #[test]
    fn active_child_id_is_next_after_inner_playhead() {
        let mut g = running_seq_group(3);
        g.seq_current_id = Some(g.children[0].id());
        let expected = g.children[1].id();
        assert_eq!(g.active_child_id(), Some(expected));
        g.seq_current_id = Some(g.children[2].id());
        assert_eq!(g.active_child_id(), None); // no child after the last
    }

    #[test]
    fn simultaneous_group_never_holds_playhead() {
        let mut g = running_seq_group(2);
        g.mode = GroupMode::Simultaneous;
        assert!(!g.holds_playhead());
        assert!(!g.released_playhead());
        assert!(!g.absorbs_go());
        assert_eq!(g.active_child_id(), None);
    }

    #[test]
    fn simultaneous_camera_child_keeps_runtime_diagnostic_change_after_reset() {
        let mut camera = CameraCue::new();
        camera.set_runtime_error_for_test("FFmpeg SRT video stream ended");
        camera.reset().unwrap(); // mirrors the simultaneous-group cleanup path

        let mut group = GroupCue::new();
        group.mode = GroupMode::Simultaneous;
        group.children.push(Box::new(camera));

        assert!(group.take_runtime_diagnostic_changed());
        assert!(
            !group.take_runtime_diagnostic_changed(),
            "the change is consumed once so the event loop refreshes once"
        );
    }

    #[test]
    fn set_active_child_parks_inner_playhead() {
        let mut g = running_seq_group(3);
        let second = g.children[1].id();
        assert!(g.set_active_child(&second));
        assert_eq!(g.active_child_id(), Some(second));

        // Parking on the first child clears the inner playhead.
        let first = g.children[0].id();
        assert!(g.set_active_child(&first));
        assert_eq!(g.seq_current_id, None);
        assert_eq!(g.active_child_id(), Some(first));
    }

    #[test]
    fn set_active_child_rejected_for_simultaneous() {
        let mut g = running_seq_group(2);
        g.mode = GroupMode::Simultaneous;
        let child = g.children[0].id();
        assert!(!g.set_active_child(&child));
    }

    // ── New modes: Playlist + StartRandom ─────────────────────────────────

    fn running_group(mode: GroupMode, n: usize) -> GroupCue {
        let mut g = GroupCue::new();
        g.mode = mode;
        for _ in 0..n {
            g.children.push(Box::new(MemoCue::new()));
        }
        g.state = CueState::Running;
        g
    }

    #[test]
    fn start_random_plays_each_child_once_before_repeating() {
        let mut g = running_group(GroupMode::StartRandom, 4);
        g.seed_rng_for_test(12345);
        let cycle1: Vec<usize> = (0..4).map(|_| g.draw_random_child().unwrap()).collect();
        let mut s1 = cycle1.clone();
        s1.sort();
        assert_eq!(s1, vec![0, 1, 2, 3], "every child drawn exactly once per cycle");
        // The bag refills → the next cycle is another full permutation.
        let cycle2: Vec<usize> = (0..4).map(|_| g.draw_random_child().unwrap()).collect();
        let mut s2 = cycle2.clone();
        s2.sort();
        assert_eq!(s2, vec![0, 1, 2, 3]);
    }

    #[test]
    fn start_random_bag_refills_when_empty() {
        let mut g = running_group(GroupMode::StartRandom, 3);
        g.seed_rng_for_test(7);
        for _ in 0..3 {
            g.draw_random_child();
        }
        assert!(g.random_bag.is_empty());
        assert!(g.draw_random_child().is_some(), "draw after exhaustion refills the bag");
        assert_eq!(g.random_bag.len(), 2, "one drawn from a freshly refilled bag of 3");
    }

    #[test]
    fn start_random_is_deterministic_for_a_seed() {
        let mut a = running_group(GroupMode::StartRandom, 5);
        let mut b = running_group(GroupMode::StartRandom, 5);
        a.seed_rng_for_test(999);
        b.seed_rng_for_test(999);
        let sa: Vec<usize> = (0..10).map(|_| a.draw_random_child().unwrap()).collect();
        let sb: Vec<usize> = (0..10).map(|_| b.draw_random_child().unwrap()).collect();
        assert_eq!(sa, sb, "same seed → same draw sequence");
    }

    #[test]
    fn playlist_trait_methods_mirror_sequential() {
        let mut g = running_group(GroupMode::Playlist, 3);
        g.seq_current_id = Some(g.children[0].id()); // more children remain
        assert!(g.absorbs_go());
        assert!(g.holds_playhead());
        assert!(!g.released_playhead());
        let expected = g.children[1].id();
        assert_eq!(g.active_child_id(), Some(expected));
        // Last child, no loop → releases the Playhead like Sequential.
        g.seq_current_id = Some(g.children[2].id());
        assert!(g.released_playhead());
        assert!(!g.holds_playhead());
    }

    #[test]
    fn looping_playlist_never_releases_playhead() {
        let mut g = running_group(GroupMode::Playlist, 3);
        g.playlist_loop = true;
        g.seq_current_id = Some(g.children[2].id()); // last child fired
        assert!(g.absorbs_go(), "a looping playlist keeps absorbing GO");
        assert!(g.holds_playhead());
        assert!(!g.released_playhead(), "never releases — loops until stopped");
    }

    #[test]
    fn start_random_trait_methods() {
        let mut g = running_group(GroupMode::StartRandom, 3);
        assert!(g.absorbs_go(), "each GO fires another random child");
        assert!(g.holds_playhead());
        assert!(!g.released_playhead());
        assert_eq!(g.active_child_id(), None, "nothing armed for a random group");
        let child = g.children[0].id();
        assert!(!g.set_active_child(&child), "cannot park a random group");
    }

    #[test]
    fn is_complete_start_random() {
        let mut g = running_group(GroupMode::StartRandom, 3);
        g.random_current_idx = Some(0);
        assert!(!g.is_complete(), "not complete while a random pick is active");
        g.random_current_idx = None;
        assert!(g.is_complete(), "complete once the fired child has finished");
    }

    #[test]
    fn is_complete_playlist_loop_never_completes() {
        let mut g = running_group(GroupMode::Playlist, 3);
        g.playlist_loop = true;
        g.seq_done = true; // even if the sequence thinks it's exhausted
        assert!(!g.is_complete(), "a looping playlist never completes on its own");
    }

    #[test]
    fn is_complete_playlist_no_loop_matches_sequential() {
        let mut g = running_group(GroupMode::Playlist, 2);
        g.seq_current_id = Some(g.children[1].id()); // last child
        g.seq_done = true;
        assert!(g.is_complete());
    }

    #[test]
    fn number_persists_master_and_offsets_without_polluting_group_json() {
        let mut number = GroupCue::new_number();
        let mut master = AudioCue::new();
        master.set_pre_wait(Duration::from_secs(2));
        let master_id = master.id();
        number.add_child(Box::new(master), -1).unwrap();
        number.set_number_master(master_id).unwrap();
        let action = AudioCue::new();
        let action_id = action.id();
        number.add_child(Box::new(action), -1).unwrap();
        number.set_number_action_offset(action_id, 1200).unwrap();
        number.fade_in = Some(FadeSpec::new(800));
        number.fade_out = Some(FadeSpec { duration_ms: 1200, curve: crate::cue::types::FadeCurve::Linear });

        let saved = number.serialize();
        assert_eq!(saved["type"], "number");
        assert_eq!(saved["master_child_id"], master_id.to_string());
        assert_eq!(saved["action_offsets_ms"][action_id.to_string()], 1200);
        assert_eq!(saved["fade_in_ms"], 800);
        assert_eq!(saved["fade_out_ms"], 1200);
        assert_eq!(number.children[0].pre_wait(), Duration::ZERO);

        let group = GroupCue::new().serialize();
        assert!(group.get("master_child_id").is_none());
        assert!(group.get("action_offsets_ms").is_none());
    }

    #[test]
    fn shared_fade_keeps_the_longer_child_duration() {
        let child = FadeSpec { duration_ms: 1600, curve: crate::cue::types::FadeCurve::Exponential };
        let shared = FadeSpec::new(800);
        let merged = crate::cue::types::combine_fade_specs(Some(&child), Some(&shared)).unwrap();
        assert_eq!(merged.duration_ms, 1600);
        assert_eq!(merged.curve, shared.curve);
    }

    #[test]
    fn removing_number_child_clears_master_and_offset() {
        let mut number = GroupCue::new_number();
        let action = AudioCue::new();
        let action_id = action.id();
        number.add_child(Box::new(action), -1).unwrap();
        number.set_number_action_offset(action_id, 500).unwrap();
        number.set_number_master(action_id).unwrap();
        number.remove_child(&action_id).unwrap();
        assert!(number.number_master_id().is_none());
        assert!(number.number_action_offsets().is_empty());
    }

    #[test]
    fn number_offset_is_the_only_child_clock_at_go() {
        let mut number = GroupCue::new_number();
        let mut master = AudioCue::new();
        let master_id = master.id();
        master.set_pre_wait(Duration::from_secs(3));
        number.add_child(Box::new(master), -1).unwrap();
        number.set_number_master(master_id).unwrap();
        let mut action = AudioCue::new();
        let action_id = action.id();
        action.set_pre_wait(Duration::from_secs(4));
        number.add_child(Box::new(action), -1).unwrap();
        number.set_number_action_offset(action_id, 700).unwrap();

        // Simulate a later edit in the child Time tab, then the Number GO
        // preflight. The scheduler must erase both hidden waits.
        number.children[1].set_pre_wait(Duration::from_secs(9));
        number.normalize_number_child_waits();
        assert_eq!(number.children[0].pre_wait(), Duration::ZERO);
        assert_eq!(number.children[1].pre_wait(), Duration::ZERO);
        assert_eq!(number.number_action_offsets()[0].1, 700);
    }

    #[test]
    fn number_accepts_only_v1_action_types() {
        let mut number = GroupCue::new_number();
        assert!(number.add_child(Box::new(AudioCue::new()), -1).is_ok());
        assert!(number.add_child(Box::new(VideoCue::new()), -1).is_ok());
        assert!(number.add_child(Box::new(ImageCue::new()), -1).is_ok());
        assert!(number.add_child(Box::new(LightCue::new()), -1).is_err());
        assert!(number.add_child(Box::new(MemoCue::new()), -1).is_err());
        assert!(number.add_child(Box::new(GroupCue::new()), -1).is_ok());
        assert!(number.add_child(Box::new(GroupCue::new_number()), -1).is_err());
    }

    #[test]
    fn group_duration_accounts_for_nested_waits_and_sequential_children() {
        let mut group = GroupCue::new();
        group.mode = GroupMode::Sequential;
        let mut first = AudioCue::new();
        first.set_pre_wait(Duration::from_millis(100));
        first.set_post_wait(Duration::from_millis(200));
        first.accept_preloaded_audio(std::sync::Arc::new(vec![0.0; 2]), 1, 1, Duration::from_secs(2));
        let mut second = AudioCue::new();
        second.accept_preloaded_audio(std::sync::Arc::new(vec![0.0; 3]), 1, 1, Duration::from_secs(3));
        group.children.push(Box::new(first));
        group.children.push(Box::new(second));
        group.pre_wait = Duration::from_millis(50);
        group.post_wait = Duration::from_millis(75);
        assert_eq!(group.duration(), Some(Duration::from_millis(5425)));
    }

    #[test]
    fn indefinite_group_modes_do_not_report_a_false_duration() {
        let mut random = GroupCue::new();
        random.mode = GroupMode::StartRandom;
        random.children.push(Box::new(AudioCue::new()));
        assert_eq!(random.duration(), None);

        let mut sequential = GroupCue::new();
        sequential.mode = GroupMode::Sequential;
        let mut child = AudioCue::new();
        child.set_continue_mode(ContinueMode::AutoContinue);
        child.accept_preloaded_audio(std::sync::Arc::new(vec![0.0; 1]), 1, 1, Duration::from_secs(1));
        sequential.children.push(Box::new(child));
        assert_eq!(sequential.duration(), None);
    }

    #[test]
    fn number_master_autoplay_is_runtime_only_and_resettable() {
        let mut group = GroupCue::new();
        group.set_number_master_autoplay(true);
        assert!(group.number_master_autoplay);
        group.reset().unwrap();
        assert!(!group.number_master_autoplay);
    }
}
