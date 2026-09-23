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
    /// The child that owns the Number timeline and determines its duration.
    master_child_id: Option<CueId>,
    /// Action child ID → offset from the master action start (milliseconds).
    action_offsets_ms: std::collections::HashMap<CueId, u64>,
    /// Number-level envelopes shared by all active audio/video children.
    fade_in: Option<FadeSpec>,
    fade_out: Option<FadeSpec>,
    /// Playlist mode only: wrap from the last child back to the first instead of
    /// ending (QLab's looping playlist). Persisted.
    playlist_loop: bool,
    /// Number start/finish flow settings. Persisted only for Number cues.
    number_start_stop_mode: NumberStartStopMode,
    number_start_stop_ids: Vec<CueId>,
    number_finish_start_ids: Vec<CueId>,
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
    runtime_error: Option<String>,
    /// Runtime Number state. These values are intentionally not serialized.
    number_launched_actions: std::collections::HashSet<CueId>,
    number_master_finished: bool,
    /// Number remains Running during its authored Post-Wait so the operator
    /// can see the final timing segment before completion/continuation.
    number_post_wait_until: Option<Instant>,
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

fn number_due_action_ids(target_ms: u64, offsets: &[(CueId, u64)]) -> Vec<CueId> {
    offsets
        .iter()
        .filter_map(|(id, offset)| (*offset <= target_ms).then_some(*id))
        .collect()
}

fn number_seek_state(was_paused: bool) -> CueState {
    if was_paused { CueState::Paused } else { CueState::Running }
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
    if matches!(child.cue_type(), CueType::Group | CueType::Number) {
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
            master_child_id: None,
            action_offsets_ms: std::collections::HashMap::new(),
            fade_in: None,
            fade_out: None,
            playlist_loop: false,
            number_start_stop_mode: NumberStartStopMode::None,
            number_start_stop_ids: Vec::new(),
            number_finish_start_ids: Vec::new(),
            number_completion_start_ids: Vec::new(),
            seq_current_id: None,
            seq_done: false,
            seq_post_wait_until: None,
            paused_at: None,
            random_bag: Vec::new(),
            rng_state: 0,
            random_current_idx: None,
            fired_child_ids: Vec::new(),
            runtime_error: None,
            number_launched_actions: std::collections::HashSet::new(),
            number_master_finished: false,
            number_post_wait_until: None,
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

    /// Deserialise a GroupCue from JSON, using `registry` to reconstruct children.
    pub fn from_json_with_registry(value: &Value, registry: &CueRegistry) -> Result<Box<dyn Cue>> {
        let is_number = value.get("type").and_then(|v| v.as_str()) == Some("number");
        let mut cue = if is_number { GroupCue::new_number() } else { GroupCue::new() };

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
        let target = position_ms;
        for child in &mut self.children {
            if child.is_running() || child.is_paused() {
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
            return;
        }
        let local = target.saturating_sub(start);
        let mut cursor = 0u64;
        let mut selected: Option<(usize, u64)> = None;
        match self.mode {
            GroupMode::Simultaneous => {
                for idx in 0..self.children.len() {
                    let child = &mut self.children[idx];
                    let child_start = if child.cue_type() == CueType::Group { 0 } else { child.pre_wait().as_millis() as u64 };
                    let child_len = child.duration().map(|d| d.as_millis() as u64).unwrap_or(0);
                    if local >= child_start && local <= child_start.saturating_add(child_len) {
                        if child.go(ctx).is_ok() {
                            child.seek_timeline_position(local.saturating_sub(child_start), ctx);
                        }
                    }
                }
                return;
            }
            GroupMode::Sequential | GroupMode::Playlist => {
                for idx in 0..self.children.len() {
                    let child = &self.children[idx];
                    let composite = child.cue_type() == CueType::Group;
                    let child_pre = if composite { 0 } else { child.pre_wait().as_millis() as u64 };
                    let child_post = if composite { 0 } else { child.post_wait().as_millis() as u64 };
                    let child_start = cursor.saturating_add(child_pre);
                    let child_len = child.duration().map(|d| d.as_millis() as u64).unwrap_or(0);
                    let child_end = child_start.saturating_add(child_len);
                    if local <= child_end {
                        selected = Some((idx, local.saturating_sub(child_start)));
                        break;
                    }
                    cursor = child_end.saturating_add(child_post);
                }
            }
            GroupMode::StartRandom => return,
        }
        let Some((idx, child_position)) = selected else { return; };
        let child_id = self.children[idx].id();
        if self.children[idx].go(ctx).is_err() {
            return;
        }
        self.seq_current_id = Some(child_id);
        self.children[idx].seek_timeline_position(child_position, ctx);
    }

    /// Rebuild a Number's child graph at a position on the master clock.
    /// Actions whose offsets are already due are started and sought; later
    /// actions remain in Standby. The parent keeps its Running/Paused state.
    fn seek_number(&mut self, position_ms: u64, ctx: &CueContext) {
        let Some(master_idx) = self.number_master_index() else {
            self.runtime_error = Some("Number has no master Audio, Video, or Group cue".into());
            return;
        };
        let max_ms = self.children[master_idx]
            .duration()
            .map(|duration| duration.as_millis() as u64);
        let target_ms = number_seek_target(position_ms, max_ms);
        let offsets = self.number_seek_action_offsets(master_idx);
        let due_actions = number_due_action_ids(target_ms, &offsets);
        let was_paused = self.state == CueState::Paused;

        // Keep already-running children alive and seek them in place.  A
        // hard-stop/restart cycle is especially harmful for Video Cues: the
        // new mpv slot starts at frame zero while its FILE_LOADED seek is
        // still pending, and a looped clip can visibly/audio-wise restart
        // before the action-time seek arrives.  Only children whose due state
        // changes need to be stopped or started.
        self.number_launched_actions.clear();
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
                return;
            }
            // Audio masters have no initial-seek transport field.  The
            // regular seek is also useful for a newly started video when the
            // output slot has already loaded synchronously.
            self.children[master_idx].seek_timeline_position(target_ms, ctx);
        }

        for idx in 0..self.children.len() {
            if idx == master_idx || self.children[idx].is_disabled() {
                continue;
            }
            let id = self.children[idx].id();
            let offset = self.action_offsets_ms.get(&id).copied().unwrap_or(0);
            let active = self.children[idx].is_running() || self.children[idx].is_paused();
            if !due_actions.contains(&id) {
                if active {
                    let _ = self.children[idx].hard_stop(ctx);
                } else {
                    let _ = self.children[idx].reset();
                }
                continue;
            }
            let action_ms = target_ms.saturating_sub(offset);
            if active {
                self.children[idx].seek(action_ms, ctx);
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
                self.children[idx].seek(action_ms, ctx);
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
    }

    fn number_child_done(&self, child: &dyn Cue, ctx: &CueContext) -> bool {
        let live_voice = child.all_voice_ids().into_iter().any(|voice_id| {
            ctx.audio_engine.voice_is_alive(voice_id)
                || ctx.output_engine.is_voice_playing(voice_id)
        });
        !live_voice
            && (matches!(child.state(), CueState::Completed | CueState::Standby)
                || child.duration().map(|d| child.action_elapsed() >= d).unwrap_or(false))
    }

    fn tick_number(&mut self, ctx: &CueContext) -> Result<()> {
        if self.number_master_finished {
            if self
                .number_post_wait_until
                .is_some_and(|due| Instant::now() >= due)
            {
                self.number_post_wait_until = None;
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
            if idx == master_idx || self.children[idx].is_disabled() {
                continue;
            }
            let id = self.children[idx].id();
            if self.number_launched_actions.contains(&id) {
                if self.children[idx].state() == CueState::Running {
                    if let Err(error) = self.children[idx].tick(ctx) {
                        let message = number_action_runtime_error(self.children[idx].name(), "during playback", error);
                        return self.abort_number(ctx, message);
                    }
                }
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
        let master_duration = self.children[master_idx].duration();
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
            let _ = self.children[master_idx].reset();
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
        let (id, nested_ids) = {
            let child = &mut self.children[idx];
            if child.is_disabled() {
                anyhow::bail!("Cue '{}' is disarmed", child.name());
            }
            child.go_with_fade(ctx, shared_fade)?;
            (child.id(), child.take_fired_cue_ids())
        };
        self.fired_child_ids.push(id);
        self.fired_child_ids.extend(nested_ids);
        Ok(())
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
                    let _ = previous.reset();
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
        if let Err(e) = self.go_child(next_idx, ctx) {
            log::warn!("Group sequential: child '{}' failed to start: {e}", self.children[next_idx].name());
            // Child rolled back to Standby — treat it as done and advance.
            self.seq_done = true;
            return Ok(());
        }

        // Playlist is exclusive: starting this child stops any other still playing.
        if self.mode == GroupMode::Playlist {
            self.stop_other_children(ctx, child_id);
        } else {
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
        if self.mode == GroupMode::Playlist {
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
                        .duration()
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
                        let _ = self.children[i].reset();
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
            let _ = self.children[idx].reset();

            if self.mode == GroupMode::Playlist {
                // A Playlist plays one child at a time and auto-advances through
                // ALL of them regardless of each child's Continue Mode (it is a
                // playlist, not a manual sequence).  `fire_next_sequential` wraps
                // to the first child when looping, or sets `seq_done` at the end.
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
    }

    fn hard_stop_children(&mut self, ctx: &CueContext) {
        for child in &mut self.children {
            if child.is_running() || child.is_paused() {
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
        for child in &mut self.children {
            if child.id() != keep_id && (child.is_running() || child.is_paused()) {
                let _ = child.stop(ctx);
                let _ = child.reset();
            }
        }
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
    fn cue_type(&self) -> CueType { if self.number_mode { CueType::Number } else { CueType::Group } }
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
            if child.is_running() || child.is_paused() {
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
            if child.is_paused() {
                let _ = child.resume(ctx);
            }
        }
        self.state = CueState::Running;
        Ok(())
    }

    fn hard_stop(&mut self, ctx: &CueContext) -> Result<()> {
        for child in &mut self.children {
            if child.is_running() || child.is_paused() {
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
        ctx.emit(CueEvent::Stopped { cue_id: self.id });
        Ok(())
    }

    fn reset(&mut self) -> Result<()> {
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
        Ok(())
    }

    fn tick(&mut self, ctx: &CueContext) -> Result<()> {
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
                        let _ = self.children[i].reset();
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
                        let _ = self.children[i].reset();
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
        self.mode = mode;
    }

    fn playlist_loop(&self) -> Option<bool> {
        (!self.number_mode).then_some(self.playlist_loop)
    }

    fn set_playlist_loop(&mut self, on: bool) {
        self.playlist_loop = on;
    }

    fn number_start_stop_specification(&self) -> Option<(NumberStartStopMode, Vec<CueId>)> {
        self.number_mode
            .then_some((self.number_start_stop_mode, self.number_start_stop_ids.clone()))
    }

    fn number_finish_start_ids(&self) -> Vec<CueId> {
        if self.number_mode { self.number_finish_start_ids.clone() } else { Vec::new() }
    }

    fn take_number_completion_start_ids(&mut self) -> Vec<CueId> {
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
            "type": if self.number_mode { "number" } else { "group" },
            "cue_type": if self.number_mode { "number" } else { "group" },
            "id": self.id,
            "number": self.number,
            "name": self.name,
            "notes": self.notes,
            "color": self.color,
            "pre_wait_ms": self.pre_wait.as_millis() as u64,
            "post_wait_ms": self.post_wait.as_millis() as u64,
            "continue_mode": self.continue_mode,
            "group_mode": self.mode,
            "playlist_loop": self.playlist_loop,
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
    use crate::cue::video_cue::VideoCue;

    #[test]
    fn number_seek_plan_clamps_master_and_starts_only_due_actions() {
        let due = Uuid::new_v4();
        let future = Uuid::new_v4();
        let offsets = vec![(due, 1_000), (future, 6_000)];
        let target = number_seek_target(9_000, Some(5_000));

        assert_eq!(target, 5_000);
        assert_eq!(number_due_action_ids(target, &offsets), vec![due]);
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
        assert_eq!(number_due_action_ids(8_000, &offsets), vec![action_id]);
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
