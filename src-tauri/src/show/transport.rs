//! Show transport — GO, STOP, PAUSE, and continue mode chaining.
//!
//! The [`Transport`] struct is the primary interface between the UI commands
//! and the cue execution system.  It holds a reference to the active
//! [`CueList`] and a [`CueContext`] so it can drive cue lifecycle methods.

use anyhow::{anyhow, Result};
use std::collections::HashSet;

use crate::cue::{
    context::CueContext,
    control_cue::ControlAction,
    types::{ContinueMode, CueId, CueState, CueType, NumberStartStopMode},
};
use crate::engine::ring_command::{FadeCurve, VoiceId};

use super::cue_list::{ContinuationPlan, CueList};

/// Result returned by [`Transport::go`].
pub struct GoResult {
    /// IDs of all cues triggered by this GO (including chained Auto-Continue /
    /// Auto-Follow cues).  The first element is always the primary cue.
    pub triggered: Vec<CueId>,
    /// IDs of cues whose `go()` was successfully invoked, including nested
    /// Group children and successful Start targets.
    pub fired: Vec<CueId>,
    /// IDs of cues stopped by a Stop Cue's action during this GO.
    pub stopped: Vec<CueId>,
}

#[derive(Default)]
struct ActionDispatchResult {
    triggered: Vec<CueId>,
    fired: Vec<CueId>,
    stopped: Vec<CueId>,
    cancelled_actions: HashSet<CueId>,
    started_targets: HashSet<CueId>,
}

/// Manages playback state for a single [`CueList`].
pub struct Transport {
    pub context: CueContext,
}

fn collect_running_ids(cue: &dyn crate::cue::traits::Cue, out: &mut Vec<CueId>) {
    if cue.is_running() || cue.is_paused() {
        out.push(cue.id());
    }
    if let Some(children) = cue.child_cues() {
        for child in children {
            collect_running_ids(child.as_ref(), out);
        }
    }
}

fn collect_subtree_ids(cue: &dyn crate::cue::traits::Cue, out: &mut HashSet<CueId>) {
    out.insert(cue.id());
    if let Some(children) = cue.child_cues() {
        for child in children {
            collect_subtree_ids(child.as_ref(), out);
        }
    }
}

fn collect_active_browser_ids(cues: &[Box<dyn crate::cue::traits::Cue>], out: &mut Vec<CueId>) {
    for cue in cues {
        if cue.cue_type() == CueType::Browser && (cue.is_running() || cue.is_paused()) {
            out.push(cue.id());
        }
        if let Some(children) = cue.child_cues() {
            collect_active_browser_ids(children, out);
        }
    }
}

fn cue_tree_is_active(cue: &dyn crate::cue::traits::Cue) -> bool {
    cue.is_running()
        || cue.is_paused()
        || cue.child_cues().is_some_and(|children| {
            children
                .iter()
                .any(|child| cue_tree_is_active(child.as_ref()))
        })
}

fn collect_number_media_targets(
    cue: &dyn crate::cue::traits::Cue,
    cue_type: CueType,
    out: &mut Vec<CueId>,
) {
    if cue.cue_type() == cue_type && (cue.is_running() || cue.is_paused()) {
        out.push(cue.id());
    }
    if let Some(children) = cue.child_cues() {
        for child in children {
            collect_number_media_targets(child.as_ref(), cue_type.clone(), out);
        }
    }
}

fn number_start_stop_targets(
    cue_list: &CueList,
    source_id: CueId,
    mode: NumberStartStopMode,
    selected: &[CueId],
) -> Vec<CueId> {
    // Persisted settings may predate the inspector guard. Do not let a Number
    // stop itself or one of its own children before it has started.
    let source_tree_ids: HashSet<CueId> = cue_list
        .get_recursive(&source_id)
        .map(|cue| {
            let mut ids = HashSet::new();
            collect_subtree_ids(cue, &mut ids);
            ids
        })
        .unwrap_or_default();
    let mut ids = Vec::new();
    match mode {
        NumberStartStopMode::None => {}
        NumberStartStopMode::All => ids.extend(
            cue_list
                .cues
                .iter()
                .filter(|cue| cue.id() != source_id && cue_tree_is_active(cue.as_ref()))
                .map(|cue| cue.id()),
        ),
        NumberStartStopMode::Audio => {
            for cue in &cue_list.cues {
                if cue.id() != source_id {
                    collect_number_media_targets(cue.as_ref(), CueType::Audio, &mut ids);
                }
            }
        }
        NumberStartStopMode::Video => {
            for cue in &cue_list.cues {
                if cue.id() != source_id {
                    collect_number_media_targets(cue.as_ref(), CueType::Video, &mut ids);
                }
            }
        }
        NumberStartStopMode::Selected => ids.extend(
            selected.iter().copied().filter(|id| {
                is_external_number_flow_target(&source_tree_ids, *id)
                    && cue_list
                        .get_recursive(id)
                        .is_some_and(|cue| cue_tree_is_active(cue))
            }),
        ),
    }
    dedup_ids(&mut ids);
    ids
}

fn is_external_number_flow_target(source_tree_ids: &HashSet<CueId>, target_id: CueId) -> bool {
    !source_tree_ids.contains(&target_id)
}

fn cue_has_descendant(cue: &dyn crate::cue::traits::Cue, descendant_id: CueId) -> bool {
    cue.child_cues().is_some_and(|children| {
        children.iter().any(|child| {
            child.id() == descendant_id || cue_has_descendant(child.as_ref(), descendant_id)
        })
    })
}

fn dedup_ids(ids: &mut Vec<CueId>) {
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(*id));
}

/// Hard-stop one cue tree before its objects are removed from a cue list.
///
/// Cue lifecycle methods normally own the voice IDs they created. Deletion is
/// different: once the cue is detached, no later transport tick can reach it.
/// Capture every recursively-owned voice first, hard-stop the cue tree, then
/// issue direct engine stops as a backstop for stale cue bookkeeping. The
/// output engine also stops a Video voice's paired audio voice.
pub fn hard_stop_cue_tree(
    context: &CueContext,
    cue: &mut dyn crate::cue::traits::Cue,
) -> Result<()> {
    let voice_ids = cue.all_voice_ids();
    let paired_audio_ids: Vec<VoiceId> = voice_ids
        .iter()
        .filter_map(|voice_id| context.output_engine.video_audio_voice(*voice_id))
        .collect();
    let mut first_error = None;

    if let Err(error) = cue.hard_stop(context) {
        first_error = Some(error);
    }

    for voice_id in voice_ids.into_iter().chain(paired_audio_ids.into_iter()) {
        if let Err(error) = context
            .audio_engine
            .stop_voice(voice_id, 0, FadeCurve::Linear)
        {
            first_error.get_or_insert(error);
        }
        if let Err(error) = context.output_engine.stop_voice(voice_id, 0) {
            first_error.get_or_insert(error);
        }
    }

    first_error.map_or(Ok(()), Err)
}

/// Set the playhead of the innermost Group containing `target`, then park each
/// ancestor on that Group so the next GO reaches the nested target.
fn set_nested_group_playhead(
    cues: &mut [Box<dyn crate::cue::traits::Cue>],
    target_id: CueId,
) -> bool {
    for cue in cues.iter_mut() {
        if cue
            .child_cues()
            .is_some_and(|children| children.iter().any(|child| child.id() == target_id))
        {
            return cue.set_active_child(&target_id);
        }
        if let Some(children) = cue.child_cues_mut() {
            if let Some(nested_group_id) = set_nested_group_playhead_in(children, target_id) {
                return cue.set_active_child(&nested_group_id);
            }
        }
    }
    false
}

fn set_nested_group_playhead_in(
    cues: &mut [Box<dyn crate::cue::traits::Cue>],
    target_id: CueId,
) -> Option<CueId> {
    for cue in cues.iter_mut() {
        let cue_id = cue.id();
        if cue
            .child_cues()
            .is_some_and(|children| children.iter().any(|child| child.id() == target_id))
        {
            return cue.set_active_child(&target_id).then_some(cue_id);
        }
        if let Some(children) = cue.child_cues_mut() {
            if let Some(nested_group_id) = set_nested_group_playhead_in(children, target_id) {
                return cue.set_active_child(&nested_group_id).then_some(cue_id);
            }
        }
    }
    None
}

impl Transport {
    /// Create a new transport bound to the given context.
    pub fn new(context: CueContext) -> Self {
        Self { context }
    }

    fn empty_go_result() -> GoResult {
        GoResult {
            triggered: Vec::new(),
            fired: Vec::new(),
            stopped: Vec::new(),
        }
    }

    /// Reconcile the lifecycle of every Browser cue after an actual shared
    /// surface start. This runs after the new owner has taken the WebView, so
    /// stopping an older cue cannot hide the new owner: BrowserCue::stop uses
    /// the manager's owner check. The recursive walk also covers Group
    /// children and nested Groups, where the ordinary top-level GO filter
    /// cannot see the running cue.
    fn reconcile_browser_ownership(&self, cue_list: &mut CueList) -> Vec<CueId> {
        let starts = self.context.take_browser_starts();
        if starts.is_empty() {
            return Vec::new();
        }
        let Some(owner_id) = starts.last().copied() else {
            return Vec::new();
        };
        let is_active_browser = cue_list
            .get_recursive(&owner_id)
            .is_some_and(|cue| cue.cue_type() == CueType::Browser && cue.is_running());
        if !is_active_browser {
            return Vec::new();
        }
        let mut stopped = Vec::new();
        let mut active_ids = Vec::new();
        collect_active_browser_ids(&cue_list.cues, &mut active_ids);
        for old_id in active_ids.into_iter().filter(|id| *id != owner_id) {
            if let Some(old) = cue_list.get_mut_recursive(&old_id) {
                if old.cue_type() == CueType::Browser && (old.is_running() || old.is_paused()) {
                    let _ = old.stop(&self.context);
                    stopped.push(old_id);
                }
            }
            cue_list.remove_continuation_plan(old_id);
        }
        dedup_ids(&mut stopped);
        stopped
    }

    fn go_to_continuation_target(
        &mut self,
        cue_list: &mut CueList,
        plan: &ContinuationPlan,
    ) -> Result<GoResult> {
        let Some(target_id) = cue_list.next_continuation_target(plan) else {
            return Ok(Self::empty_go_result());
        };
        cue_list.playhead_cue_id = Some(target_id);
        self.go(cue_list)
    }

    /// Dispatch a continuation whose source has reached its Auto-Continue or
    /// Auto-Follow boundary. Source generation and plan token protect against
    /// stale deadlines; the current UI Playhead is deliberately irrelevant.
    pub fn continue_from_source(
        &mut self,
        cue_list: &mut CueList,
        source_id: CueId,
        token: u64,
    ) -> Result<GoResult> {
        let Some(plan) = cue_list.continuation_plan(source_id).cloned() else {
            return Ok(Self::empty_go_result());
        };
        if plan.token != token || plan.source_id != source_id {
            return Ok(Self::empty_go_result());
        }
        let source_valid = cue_list.get_recursive(&source_id).is_some_and(|source| {
            source.play_generation() == plan.source_generation
                && source.continue_mode() != ContinueMode::DoNotContinue
                && source.auto_continue_marker() != Some(false)
        });
        if !source_valid {
            cue_list.remove_continuation_plan(source_id);
            return Ok(Self::empty_go_result());
        }
        let Some(plan) = cue_list.take_continuation_plan(source_id) else {
            return Ok(Self::empty_go_result());
        };
        self.go_to_continuation_target(cue_list, &plan)
    }

    /// Apply action specifications for cues started by GO, including children
    /// started later by a Group tick. All cue types share this dispatcher so a
    /// nested Stop/Fade/Devamp/Control cue gets the same recursive semantics.
    pub(crate) fn dispatch_fired_cues(
        &mut self,
        cue_list: &mut CueList,
        fired_ids: &[CueId],
    ) -> GoResult {
        let mut result = ActionDispatchResult::default();
        let mut dispatched = HashSet::new();
        let mut path = HashSet::new();
        for cue_id in fired_ids.iter().copied() {
            // A previous action in the same Group batch may have stopped or
            // reset this cue. Do not run its action spec after cancellation.
            if result.cancelled_actions.contains(&cue_id) {
                continue;
            }
            self.dispatch_action_recursive(
                cue_list,
                cue_id,
                &mut path,
                &mut dispatched,
                &mut result,
            );
        }
        dedup_ids(&mut result.fired);
        dedup_ids(&mut result.triggered);
        dedup_ids(&mut result.stopped);
        result.stopped.extend(self.reconcile_browser_ownership(cue_list));
        dedup_ids(&mut result.stopped);
        GoResult {
            triggered: result.triggered,
            fired: result.fired,
            stopped: result.stopped,
        }
    }

    fn dispatch_action_recursive(
        &self,
        cue_list: &mut CueList,
        cue_id: CueId,
        path: &mut HashSet<CueId>,
        dispatched: &mut HashSet<CueId>,
        result: &mut ActionDispatchResult,
    ) {
        if result.cancelled_actions.contains(&cue_id)
            || path.contains(&cue_id)
            || !dispatched.insert(cue_id)
        {
            return;
        }
        let specs = cue_list.get_recursive(&cue_id).map(|cue| {
            (
                cue.fade_specification(),
                cue.devamp_specification(),
                cue.stop_specification(),
                cue.control_specification(),
            )
        });
        let Some((fade_spec, devamp_spec, stop_spec, control_spec)) = specs else {
            return;
        };
        path.insert(cue_id);

        if let Some(spec) = fade_spec {
            let mut voice_infos: Vec<(VoiceId, f32, f32)> = Vec::new();
            let mut visual_targets: Vec<(VoiceId, f32)> = Vec::new();
            for target_id in &spec.target_cue_ids {
                let Some(target) = cue_list.get_recursive(target_id) else {
                    continue;
                };
                if target.is_visual() {
                    let Some(voice) = target.playing_voice_id() else {
                        continue;
                    };
                    visual_targets
                        .push((voice, self.context.output_engine.get_voice_opacity(voice)));
                    if target.cue_type() == CueType::Video {
                        if let Some(audio_id) = self.context.output_engine.video_audio_voice(voice)
                        {
                            voice_infos.push((
                                audio_id,
                                self.context.audio_engine.get_voice_gain(audio_id),
                                self.context.audio_engine.get_voice_pan(audio_id),
                            ));
                        }
                    }
                } else {
                    for voice in target.all_voice_ids() {
                        voice_infos.push((
                            voice,
                            self.context.audio_engine.get_voice_gain(voice),
                            self.context.audio_engine.get_voice_pan(voice),
                        ));
                    }
                }
            }
            let opacity = spec
                .target_visual_alpha
                .map(|alpha| 1.0 - alpha as f32 / 255.0)
                .unwrap_or_else(|| spec.target_gain_linear.clamp(0.0, 1.0));
            if let Some(cue) = cue_list.get_mut_recursive(&cue_id) {
                cue.set_fade_voices(voice_infos, visual_targets, opacity);
            }
        }

        if let Some((stop_at_end, target_ids)) = devamp_spec {
            for target_id in target_ids {
                let Some(target) = cue_list.get_recursive(&target_id) else {
                    continue;
                };
                if target.is_visual() {
                    if let Some(voice) = target.playing_voice_id() {
                        self.context.output_engine.devamp_voice(voice, stop_at_end);
                        if let Some(audio_id) = self.context.output_engine.video_audio_voice(voice)
                        {
                            let _ = self
                                .context
                                .audio_engine
                                .devamp_voice(audio_id, stop_at_end);
                        }
                    }
                } else {
                    for voice in target.all_voice_ids() {
                        let _ = self.context.audio_engine.devamp_voice(voice, stop_at_end);
                    }
                }
            }
        }

        if let Some((hard, target_ids)) = stop_spec {
            let stop_all = target_ids.is_empty();
            let ids_to_stop: Vec<CueId> = if target_ids.is_empty() {
                cue_list
                    .cues
                    .iter()
                    .filter(|cue| cue_tree_is_active(cue.as_ref()) && cue.id() != cue_id)
                    .map(|cue| cue.id())
                    .collect()
            } else {
                target_ids.into_iter().filter(|id| *id != cue_id).collect()
            };
            for target_id in ids_to_stop {
                if let Some(target) = cue_list.get_mut_recursive(&target_id) {
                    collect_subtree_ids(target, &mut result.cancelled_actions);
                    collect_running_ids(target, &mut result.stopped);
                    result.stopped.push(target_id);
                    if hard {
                        let _ = target.hard_stop(&self.context);
                    } else {
                        let _ = target.stop(&self.context);
                    }
                }
                let cancelled: Vec<CueId> = result.cancelled_actions.iter().copied().collect();
                for cancelled_id in cancelled {
                    cue_list.remove_continuation_plan(cancelled_id);
                }
                cue_list.remove_continuation_plan(target_id);
            }
            if stop_all {
                // A source may already be reset to Standby while its delayed
                // Auto-Follow is pending. Stop All still cancels that chain.
                cue_list.clear_continuation_plans();
            }
            dedup_ids(&mut result.stopped);
        }

        if let Some((action, target_ids)) = control_spec {
            for target_id in target_ids.into_iter().filter(|id| *id != cue_id) {
                if action == ControlAction::Goto {
                    let goto_succeeded = if cue_list.index_of(&target_id).is_some() {
                        cue_list.playhead_cue_id = Some(target_id);
                        true
                    } else {
                        // A nested Goto changes the selection path, not target
                        // cue execution; keep GoResult's fired semantics.
                        set_nested_group_playhead(&mut cue_list.cues, target_id)
                    };
                    if goto_succeeded {
                        // Explicit Goto is a flow change and cancels existing
                        // delayed chains. Transport::go may bind a fresh plan
                        // for this Goto cue after action dispatch completes.
                        cue_list.clear_continuation_plans();
                    }
                    continue;
                }

                let target_successors = if action == ControlAction::Start {
                    cue_list.continuation_successors(target_id)
                } else {
                    None
                };
                let mut nested_fired = Vec::new();
                let mut target_started = false;
                let mut target_generation_before = None;
                if let Some(target) = cue_list.get_mut_recursive(&target_id) {
                    match action {
                        ControlAction::Start
                            if !path.contains(&target_id)
                                && !target.is_disabled()
                                && !result.started_targets.contains(&target_id)
                                && !cue_has_descendant(&*target, cue_id) =>
                        {
                            target_generation_before = Some(target.play_generation());
                            if target.go(&self.context).is_ok() {
                                target_started = true;
                                result.started_targets.insert(target_id);
                                nested_fired.extend(target.take_fired_cue_ids());
                            }
                        }
                        ControlAction::Start => continue,
                        ControlAction::Pause => {
                            let _ = target.pause(&self.context);
                        }
                        ControlAction::Resume => {
                            let _ = target.resume(&self.context);
                        }
                        ControlAction::Load => {
                            let _ = target.preload(&self.context);
                        }
                        ControlAction::Reset => {
                            collect_subtree_ids(target, &mut result.cancelled_actions);
                            collect_running_ids(target, &mut result.stopped);
                            let _ = target.hard_stop(&self.context);
                            let _ = target.reset();
                        }
                        ControlAction::Arm => target.set_disabled(false),
                        ControlAction::Disarm => target.set_disabled(true),
                        ControlAction::Goto => unreachable!("handled above"),
                    }
                    result.triggered.push(target_id);
                }
                if target_started {
                    let new_target_execution = target_generation_before.is_some_and(|before| {
                        cue_list
                            .get_recursive(&target_id)
                            .is_some_and(|target| target.play_generation() != before)
                    });
                    if new_target_execution {
                        if let Some(successors) = target_successors {
                            let continuation =
                                cue_list.get_recursive(&target_id).and_then(|target| {
                                    (target.continue_mode() != ContinueMode::DoNotContinue)
                                        .then_some(target.play_generation())
                                });
                            if let Some(generation) = continuation {
                                cue_list.bind_continuation(target_id, generation, successors);
                            } else {
                                cue_list.remove_continuation_plan(target_id);
                            }
                        }
                    }
                    result.fired.push(target_id);
                    result.fired.extend(nested_fired.iter().copied());
                    for nested_id in nested_fired {
                        self.dispatch_action_recursive(
                            cue_list, nested_id, path, dispatched, result,
                        );
                    }
                    self.dispatch_action_recursive(cue_list, target_id, path, dispatched, result);
                }
                if action == ControlAction::Reset {
                    cue_list.remove_continuation_plan(target_id);
                    let cancelled: Vec<CueId> = result.cancelled_actions.iter().copied().collect();
                    for cancelled_id in cancelled {
                        cue_list.remove_continuation_plan(cancelled_id);
                    }
                }
            }
        }
        path.remove(&cue_id);
    }

    // -----------------------------------------------------------------------
    // GO
    // -----------------------------------------------------------------------

    /// Trigger the cue at the Playhead.
    ///
    /// Returns a [`GoResult`] containing:
    /// - `triggered`: IDs of **all** cues fired in this call (primary + chains).
    /// - `stopped`: IDs of cues stopped by a Stop Cue action.
    ///
    /// Sequence:
    /// 1. Read the cue at the Playhead.
    /// 2. Advance the Playhead to the next cue.
    /// 3. Stop any running cues with `stop_on_next_go()` (visual-only logic).
    /// 4. Call `cue.go()`.
    /// 5. Execute any stop action declared by `cue.stop_specification()` — this
    ///    runs **before** chain evaluation so Auto-Follow cannot start a cue that
    ///    the Stop Cue would immediately kill.
    /// 6. Chain via Auto-Continue (post_wait = 0) or instant Auto-Follow.
    pub fn go(&mut self, cue_list: &mut CueList) -> Result<GoResult> {
        let cue_id = match cue_list.playhead_cue_id {
            Some(id) => id,
            None => {
                return Ok(GoResult {
                    triggered: vec![],
                    fired: vec![],
                    stopped: vec![],
                })
            }
        };

        // If the cue at the playhead is disabled (e.g., toggled while the
        // playhead was parked on it), advance past it and retry.
        if cue_list.get(&cue_id).is_some_and(|c| c.is_disabled()) {
            cue_list.advance_playhead();
            return self.go(cue_list);
        }

        // If the cue wants to absorb this GO (e.g., a Sequential Group paused
        // mid-sequence), delegate to the cue and skip outer Playhead advancement.
        if cue_list.get(&cue_id).is_some_and(|c| c.absorbs_go()) {
            if let Some(cue) = cue_list.get_mut(&cue_id) {
                cue.go(&self.context)?;
            }
            let mut fired = vec![cue_id];
            if let Some(cue) = cue_list.get_mut(&cue_id) {
                fired.extend(cue.take_fired_cue_ids());
            }
            let action_result = self.dispatch_fired_cues(cue_list, &fired);
            fired.extend(action_result.fired);
            let mut triggered = vec![cue_id];
            triggered.extend(action_result.triggered);
            let mut stopped = action_result.stopped;
            // If that GO fired the group's last child, release the outer Playhead
            // to the cue after the group so the next GO continues the outer list.
            if cue_list.get(&cue_id).is_some_and(|c| c.released_playhead()) {
                cue_list.advance_playhead();
            }
            dedup_ids(&mut fired);
            dedup_ids(&mut triggered);
            dedup_ids(&mut stopped);
            return Ok(GoResult {
                triggered,
                fired,
                stopped,
            });
        }

        // Freeze this execution's successor path before advancing the visible
        // Playhead or dispatching actions (a Goto action may move it).
        let continuation_successors = cue_list.continuation_successors(cue_id);
        let (source_was_active, source_generation_before) = cue_list
            .get(&cue_id)
            .map(|cue| {
                (
                    cue.state() == CueState::Running || cue.state() == CueState::Paused,
                    cue.play_generation(),
                )
            })
            .unwrap_or((false, 0));

        // Advance playhead before triggering (matches QLab behaviour).
        cue_list.advance_playhead();

        // Stop any running cues that should automatically stop on the next GO
        // (only the Text Cue opts in — visual cues are layers and never
        // auto-stop each other).  If a visual type ever opts back in, it only
        // stops when the incoming cue is also visual.
        let incoming_is_visual = cue_list
            .get(&cue_id)
            .map(|c| c.is_visual())
            .unwrap_or(false);

        let top_level_stop_ids: Vec<CueId> = cue_list
            .cues
            .iter()
            .filter(|c| {
                if !c.is_running() || c.id() == cue_id || !c.stop_on_next_go() {
                    return false;
                }
                !c.is_visual() || incoming_is_visual
            })
            .map(|c| c.id())
            .collect();
        for id in &top_level_stop_ids {
            if let Some(cue) = cue_list.get_mut(id) {
                let _ = cue.stop(&self.context);
            }
            cue_list.remove_continuation_plan(*id);
        }

        // Number can optionally clear currently-running media before its
        // master starts. Resolve targets against the full recursive list so
        // selected nested cues work exactly like command targets.
        let number_start_stop_ids = cue_list
            .get(&cue_id)
            .and_then(|cue| cue.number_start_stop_specification())
            .map(|(mode, selected)| number_start_stop_targets(cue_list, cue_id, mode, &selected))
            .unwrap_or_default();
        for id in &number_start_stop_ids {
            if let Some(cue) = cue_list.get_mut_recursive(id) {
                let _ = cue.stop(&self.context);
            }
            cue_list.remove_continuation_plan(*id);
        }

        // Trigger the cue.
        {
            let cue = cue_list
                .get_mut(&cue_id)
                .ok_or_else(|| anyhow!("Cue not found: {:?}", cue_id))?;
            cue.go(&self.context)?;
        }
        let mut fired = vec![cue_id];
        if let Some(cue) = cue_list.get_mut(&cue_id) {
            fired.extend(cue.take_fired_cue_ids());
        }

        // All action cue side effects use the shared recursive dispatcher.
        let action_result = self.dispatch_fired_cues(cue_list, &fired);
        fired.extend(action_result.fired);
        let mut stopped = top_level_stop_ids;
        stopped.extend(number_start_stop_ids);
        stopped.extend(action_result.stopped);
        let commanded = action_result.triggered;

        // Read continue-mode metadata after go() (state may have changed for
        // instant cues that complete synchronously).
        let (continue_mode, post_wait, is_still_running, holds_playhead) = cue_list
            .cues
            .iter()
            .find(|c| c.id() == cue_id)
            .map(|c| {
                (
                    c.continue_mode(),
                    c.post_wait(),
                    c.state() == CueState::Running,
                    c.holds_playhead(),
                )
            })
            .ok_or_else(|| anyhow!("Cue not found after go: {:?}", cue_id))?;
        let source_generation = cue_list
            .get(&cue_id)
            .map(|cue| cue.play_generation())
            .unwrap_or(0);
        let execution_started = !source_was_active || source_generation != source_generation_before;

        let continuation_token =
            if execution_started && continue_mode != ContinueMode::DoNotContinue {
                match (continuation_successors, Some(source_generation)) {
                    (Some(successors), Some(generation)) => {
                        Some(cue_list.bind_continuation(cue_id, generation, successors))
                    }
                    _ => {
                        cue_list.remove_continuation_plan(cue_id);
                        None
                    }
                }
            } else if execution_started {
                cue_list.remove_continuation_plan(cue_id);
                None
            } else {
                None
            };

        // Sequential groups retain the outer Playhead while running so that
        // subsequent GOs are routed into the group's internal sequence via
        // absorbs_go().  The advance_playhead() already moved the Playhead
        // forward; we move it back here.  The event loop will advance it again
        // once the group completes.
        if is_still_running && holds_playhead {
            cue_list.playhead_cue_id = Some(cue_id);
        }

        // Determine whether to chain immediately:
        //
        // • AutoContinue + post_wait = 0 → always chain now (audio or instant).
        //   Mark the flag so the event loop skips this cue.
        // • AutoFollow on an instant cue  → chain now (cue already completed).
        //   Audio AutoFollow is handled by the event loop on voice completion.
        let chain_now = execution_started
            && ((continue_mode == ContinueMode::AutoContinue && post_wait.is_zero())
                || (!is_still_running
                    && continue_mode == ContinueMode::AutoFollow
                    && post_wait.is_zero()));

        if execution_started && continue_mode == ContinueMode::AutoContinue && post_wait.is_zero() {
            if let Some(cue) = cue_list.get_mut(&cue_id) {
                cue.mark_auto_continue_fired();
            }
        }

        // Cues a Command cue acted on are reported as triggered so the UI
        // refreshes their row — their state changed without them being GO'd.
        let mut triggered = vec![cue_id];
        triggered.extend(commanded);

        if chain_now {
            let mut rest = if let Some(token) = continuation_token {
                let plan = cue_list
                    .continuation_plan(cue_id)
                    .filter(|plan| plan.token == token)
                    .cloned();
                if let Some(plan) = plan {
                    cue_list.take_continuation_plan(cue_id);
                    self.go_to_continuation_target(cue_list, &plan)?
                } else {
                    Self::empty_go_result()
                }
            } else {
                Self::empty_go_result()
            };
            triggered.append(&mut rest.triggered);
            fired.append(&mut rest.fired);
            stopped.extend(rest.stopped);
        }

        dedup_ids(&mut fired);
        dedup_ids(&mut triggered);
        dedup_ids(&mut stopped);
        Ok(GoResult {
            triggered,
            fired,
            stopped,
        })
    }

    // -----------------------------------------------------------------------
    // CART MODE — trigger by ID
    // -----------------------------------------------------------------------

    /// Trigger a specific cue by ID (Cart Mode).
    ///
    /// Parks the Playhead on `cue_id` then fires it via the normal GO path so
    /// Auto-Continue / Auto-Follow chains still work.
    pub fn go_by_id(&mut self, cue_list: &mut CueList, cue_id: &CueId) -> Result<GoResult> {
        cue_list.set_playhead(Some(*cue_id))?;
        self.go(cue_list)
    }

    /// Start targets requested by a Number after its final post-wait. This is
    /// deliberately separate from outer Playhead GO: the Number has already
    /// completed and the selected targets must not move the Playhead.
    pub fn start_number_targets(
        &mut self,
        cue_list: &mut CueList,
        source_id: CueId,
        target_ids: &[CueId],
    ) -> GoResult {
        let source_descendants: HashSet<CueId> = cue_list
            .get_recursive(&source_id)
            .map(|cue| {
                let mut ids = HashSet::new();
                collect_subtree_ids(cue, &mut ids);
                ids
            })
            .unwrap_or_default();
        let mut fired = Vec::new();
        let mut triggered = Vec::new();
        let mut stopped = Vec::new();
        let mut seen = HashSet::new();
        for target_id in target_ids.iter().copied() {
            if !seen.insert(target_id) || source_descendants.contains(&target_id) {
                continue;
            }
            let started = if let Some(target) = cue_list.get_mut_recursive(&target_id) {
                if target.is_disabled() || target.is_running() || target.is_paused() {
                    false
                } else if target.go(&self.context).is_ok() {
                    fired.push(target_id);
                    fired.extend(target.take_fired_cue_ids());
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if started {
                triggered.push(target_id);
            }
        }
        if !fired.is_empty() {
            let result = self.dispatch_fired_cues(cue_list, &fired);
            fired.extend(result.fired);
            triggered.extend(result.triggered);
            stopped.extend(result.stopped);
        }
        dedup_ids(&mut fired);
        dedup_ids(&mut triggered);
        dedup_ids(&mut stopped);
        GoResult { triggered, fired, stopped }
    }

    // -----------------------------------------------------------------------
    // STOP / PAUSE / RESUME
    // -----------------------------------------------------------------------

    /// Stop a specific cue (with soft fade-out).
    pub fn stop_cue(&mut self, cue_list: &mut CueList, cue_id: &CueId) -> Result<()> {
        let cue = cue_list
            .get_mut_recursive(cue_id)
            .ok_or_else(|| anyhow!("Cue not found: {:?}", cue_id))?;
        cue.stop(&self.context)?;
        cue_list.remove_continuation_plan(*cue_id);
        Ok(())
    }

    /// Hard-stop a specific cue (immediate cut, no fade).
    pub fn hard_stop_cue(&mut self, cue_list: &mut CueList, cue_id: &CueId) -> Result<()> {
        let cue = cue_list
            .get_mut_recursive(cue_id)
            .ok_or_else(|| anyhow!("Cue not found: {:?}", cue_id))?;
        cue.hard_stop(&self.context)?;
        cue_list.remove_continuation_plan(*cue_id);
        Ok(())
    }

    /// Stop all running cues with a soft fade-out.
    pub fn stop_all(&mut self, cue_list: &mut CueList) -> Result<()> {
        let running_ids: Vec<CueId> = cue_list
            .cues
            .iter()
            .filter(|c| c.is_running() || c.is_paused())
            .map(|c| c.id())
            .collect();

        for id in running_ids {
            if let Some(cue) = cue_list.get_mut(&id) {
                let _ = cue.stop(&self.context);
            }
            cue_list.remove_continuation_plan(id);
        }
        cue_list.clear_continuation_plans();
        Ok(())
    }

    /// Panic stop (double-Escape): hard-stop all running cues, then cut
    /// everything the engines are outputting regardless of cue bookkeeping.
    ///
    /// The per-cue pass alone is not enough: a cue whose state desynced from
    /// its engine voice (state says Standby/Completed but the voice still
    /// plays) is invisible to the `is_running()` filter and unstoppable
    /// through its own `hard_stop()`.  The engine-level backstop silences
    /// those too, and the final reset pass clears their stale bookkeeping.
    pub fn hard_stop_all(&mut self, cue_list: &mut CueList) -> Result<()> {
        let running_ids: Vec<CueId> = cue_list
            .cues
            .iter()
            .filter(|c| c.is_running() || c.is_paused())
            .map(|c| c.id())
            .collect();

        for id in running_ids {
            if let Some(cue) = cue_list.get_mut(&id) {
                let _ = cue.hard_stop(&self.context);
            }
            cue_list.remove_continuation_plan(id);
        }
        cue_list.clear_continuation_plans();

        let _ = self.context.audio_engine.panic_stop_all();
        self.context.output_engine.panic_stop();
        let _ = self.context.output_engine.clear_browser_surface(true);

        for cue in cue_list.cues.iter_mut() {
            let _ = cue.reset();
        }
        for id in cue_list.all_cue_ids() {
            cue_list.remove_continuation_plan(id);
        }
        Ok(())
    }

    /// Pause a specific cue.
    pub fn pause_cue(&mut self, cue_list: &mut CueList, cue_id: &CueId) -> Result<()> {
        let cue = cue_list
            .get_mut_recursive(cue_id)
            .ok_or_else(|| anyhow!("Cue not found: {:?}", cue_id))?;
        cue.pause(&self.context)
    }

    /// Resume a paused cue.
    pub fn resume_cue(&mut self, cue_list: &mut CueList, cue_id: &CueId) -> Result<()> {
        let cue = cue_list
            .get_mut_recursive(cue_id)
            .ok_or_else(|| anyhow!("Cue not found: {:?}", cue_id))?;
        cue.resume(&self.context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn persisted_number_flow_targets_exclude_the_number_tree() {
        let number_id = Uuid::new_v4();
        let nested_action_id = Uuid::new_v4();
        let external_id = Uuid::new_v4();
        let source_tree_ids = HashSet::from([number_id, nested_action_id]);

        assert!(!is_external_number_flow_target(&source_tree_ids, number_id));
        assert!(!is_external_number_flow_target(&source_tree_ids, nested_action_id));
        assert!(is_external_number_flow_target(&source_tree_ids, external_id));
    }
}
