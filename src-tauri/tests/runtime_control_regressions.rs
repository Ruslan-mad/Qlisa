//! Regression coverage for runtime control, Group sequencing, and Cue clocks.
//!
//! These cases use the recording engines from `common`; no audio device or
//! output window is needed. Short bounded waits exercise clock deadlines.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    full_registry, recording_context, recording_context_with, recording_context_with_video_audio,
    preload_silent_video_audio, EngineCall,
};
use inkue_lib::cue::context::CueContext;
use inkue_lib::cue::fade_cue::FadeCue;
use inkue_lib::cue::group_cue::GroupCue;
use inkue_lib::cue::registry::CueRegistry;
use inkue_lib::cue::traits::Cue;
use inkue_lib::cue::types::{ContinueMode, CueState, CueType, GroupMode};
use inkue_lib::engine::audio_input::InputPatch;
use inkue_lib::show::cue_list::CueList;
use inkue_lib::show::transport::{hard_stop_cue_tree, Transport};

fn command(registry: &CueRegistry, kind: CueType, target_ids: &[uuid::Uuid]) -> Box<dyn Cue> {
    let mut json = registry.create(&kind).unwrap().serialize();
    json["target_cue_ids"] = serde_json::json!(target_ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>());
    registry.from_json(json).unwrap()
}

fn list_of(cues: Vec<Box<dyn Cue>>) -> CueList {
    let mut list = CueList::new("Runtime control regressions");
    for cue in cues {
        list.push(cue);
    }
    list
}

fn preloaded_audio(registry: &CueRegistry) -> Box<dyn Cue> {
    let mut cue = registry.create(&CueType::Audio).unwrap();
    cue.accept_preloaded_audio(
        Arc::new(vec![0.0_f32; 4_800]),
        1,
        48_000,
        Duration::from_secs(30),
    );
    cue
}

#[test]
fn deleting_running_audio_tree_hard_stops_its_voice_before_detach() {
    let registry = full_registry();
    let mut cue = preloaded_audio(&registry);
    let (context, _events, _) = recording_context();

    cue.go(&context).unwrap();
    let voice_id = cue.playing_voice_id().unwrap();
    assert!(context.audio_engine.voice_is_alive(voice_id));

    hard_stop_cue_tree(&context, cue.as_mut()).unwrap();

    assert!(!context.audio_engine.voice_is_alive(voice_id));
    assert!(!cue.is_running());
    assert!(cue.all_voice_ids().is_empty());
}

#[test]
fn deleting_running_group_hard_stops_nested_audio_ownership() {
    let registry = full_registry();
    let child = preloaded_audio(&registry);
    let child_id = child.id();
    let mut group = GroupCue::new();
    group.children.push(child);
    let (context, _events, _) = recording_context();

    group.go(&context).unwrap();
    let voice_id = group
        .all_voice_ids()
        .into_iter()
        .next()
        .expect("running group owns its child's audio voice");
    assert_eq!(group.child_cues().unwrap()[0].id(), child_id);
    assert!(context.audio_engine.voice_is_alive(voice_id));

    hard_stop_cue_tree(&context, &mut group).unwrap();

    assert!(!context.audio_engine.voice_is_alive(voice_id));
    assert!(group.all_voice_ids().is_empty());
    assert!(!group.child_cues().unwrap()[0].is_running());
}

#[test]
fn deleting_running_visual_also_stops_paired_audio_voice() {
    let registry = full_registry();
    let mut json = registry.create(&CueType::Video).unwrap().serialize();
    json["file_path"] = serde_json::json!("test.mp4");
    let mut video = registry.from_json(json).unwrap();
    preload_silent_video_audio(video.as_mut());
    let (context, _events, log) = recording_context_with_video_audio();

    video.go(&context).unwrap();
    let visual_id = video.playing_voice_id().unwrap();
    hard_stop_cue_tree(&context, video.as_mut()).unwrap();

    let calls = log.lock().unwrap();
    assert!(calls
        .iter()
        .any(|call| matches!(call, EngineCall::AudioStopVoice { fade_ms: 0 })));
    assert!(calls
        .iter()
        .any(|call| matches!(call, EngineCall::OutputStopVoice { fade_ms: 0 })));
    assert!(!video.is_running());
    assert_ne!(visual_id, uuid::Uuid::nil());
}

fn configured_wait(
    registry: &CueRegistry,
    duration: Duration,
    continue_mode: ContinueMode,
    post_wait: Duration,
) -> Box<dyn Cue> {
    let mut cue = registry.create(&CueType::Wait).unwrap();
    cue.set_user_action_duration(Some(duration)).unwrap();
    cue.set_continue_mode(continue_mode);
    cue.set_post_wait(post_wait);
    cue
}

#[test]
fn seeking_audio_to_duration_keeps_a_live_voice_running_until_stopped() {
    let registry = full_registry();
    let mut audio = preloaded_audio(&registry);
    let duration = audio.duration().unwrap();
    let (context, _events, _) = recording_context();

    audio.go(&context).unwrap();
    let voice_id = audio.playing_voice_id().unwrap();
    assert!(context.audio_engine.voice_is_alive(voice_id));
    let continue_clock_before_seek = audio.auto_continue_elapsed();

    audio.seek(duration.as_millis() as u64, &context);

    assert!(audio.action_elapsed() >= duration);
    assert!(
        audio.auto_continue_elapsed() < duration,
        "scrubbing the media playhead to the end must not advance the Auto-Continue clock"
    );
    assert!(audio.auto_continue_elapsed() <= continue_clock_before_seek + Duration::from_millis(5));
    assert!(
        audio.is_running(),
        "seek must not complete the Cue by itself"
    );
    assert!(
        context.audio_engine.voice_is_alive(voice_id),
        "reaching time_done via seek must not be mistaken for engine voice completion"
    );

    audio.stop(&context).unwrap();
    assert!(
        !context.audio_engine.voice_is_alive(voice_id),
        "the test audio engine should report a stopped voice as no longer alive"
    );
}

fn audio_devamp_count(log: &common::CallLog) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|call| matches!(call, EngineCall::AudioDevamp { .. }))
        .count()
}

fn audio_gain_count(log: &common::CallLog) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|call| matches!(call, EngineCall::AudioSetGain { .. }))
        .count()
}

fn audio_play_count(log: &common::CallLog) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|call| matches!(call, EngineCall::AudioPlayRouted { .. }))
        .count()
}

#[test]
fn start_targeting_another_start_executes_the_nested_action() {
    let registry = full_registry();
    let target = registry.create(&CueType::Wait).unwrap();
    let target_id = target.id();
    let inner_start = command(&registry, CueType::Start, &[target_id]);
    let inner_start_id = inner_start.id();
    let outer_start = command(&registry, CueType::Start, &[inner_start_id]);
    let mut list = list_of(vec![target, inner_start, outer_start]);
    list.playhead_cue_id = Some(list.cues[2].id());
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();

    assert!(
        list.get(&target_id).unwrap().is_running(),
        "Start targeting a Start cue must execute that cue's action as well",
    );
}

#[test]
fn cyclic_start_actions_terminate_without_retriggering_forever() {
    let registry = full_registry();
    let first_id = uuid::Uuid::new_v4();
    let second_id = uuid::Uuid::new_v4();
    let mut first_json = registry.create(&CueType::Start).unwrap().serialize();
    first_json["id"] = serde_json::json!(first_id);
    first_json["target_cue_ids"] = serde_json::json!([second_id]);
    let first = registry.from_json(first_json).unwrap();
    let mut second_json = registry.create(&CueType::Start).unwrap().serialize();
    second_json["id"] = serde_json::json!(second_id);
    second_json["target_cue_ids"] = serde_json::json!([first_id]);
    let second = registry.from_json(second_json).unwrap();
    let mut list = list_of(vec![first, second]);
    list.playhead_cue_id = Some(first_id);
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context);

    let result = transport.go(&mut list).unwrap();

    assert!(
        result.fired.len() <= 2,
        "cyclic Start actions must be bounded: {:?}",
        result.fired
    );
}

#[test]
fn start_child_cannot_retrigger_its_containing_group() {
    let registry = full_registry();
    let mut audio = preloaded_audio(&registry);
    let audio_id = audio.id();
    let mut group = GroupCue::new();
    group.mode = GroupMode::Simultaneous;
    let group_id = group.id();
    let start_parent = command(&registry, CueType::Start, &[group_id]);
    group.children.push(audio);
    group.children.push(start_parent);
    let mut list = list_of(vec![Box::new(group)]);
    list.playhead_cue_id = Some(group_id);
    let (context, _events, log) = recording_context();
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();

    let audio_starts = log
        .lock()
        .unwrap()
        .iter()
        .filter(|call| matches!(call, EngineCall::AudioPlayRouted { .. }))
        .count();
    assert_eq!(
        audio_starts, 1,
        "Start targeting its containing Group must not replay siblings"
    );
    assert!(list.get_recursive(&audio_id).unwrap().is_running());
}

#[test]
fn simultaneous_group_stop_all_cancels_sibling_start_action() {
    let registry = full_registry();
    let target = registry.create(&CueType::Wait).unwrap();
    let target_id = target.id();
    let stop_all = command(&registry, CueType::Stop, &[]);
    let start_target = command(&registry, CueType::Start, &[target_id]);
    let mut group = GroupCue::new();
    group.mode = GroupMode::Simultaneous;
    group.children.push(stop_all);
    group.children.push(start_target);
    let group_id = group.id();
    let mut list = list_of(vec![target, Box::new(group)]);
    list.playhead_cue_id = Some(group_id);
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();

    assert_eq!(
        list.get(&target_id).unwrap().state(),
        CueState::Standby,
        "Stop All in a simultaneous Group must cancel its sibling Start action"
    );
}

#[test]
fn nested_group_reset_cancels_later_start_action() {
    let registry = full_registry();
    let target = registry.create(&CueType::Wait).unwrap();
    let target_id = target.id();
    let later_start = command(&registry, CueType::Start, &[target_id]);
    let later_start_id = later_start.id();
    let reset_later_start = command(&registry, CueType::Reset, &[later_start_id]);
    let mut group = GroupCue::new();
    group.mode = GroupMode::Simultaneous;
    group.children.push(reset_later_start);
    group.children.push(later_start);
    let group_id = group.id();
    let start_group = command(&registry, CueType::Start, &[group_id]);
    let start_group_id = start_group.id();
    let mut list = list_of(vec![target, Box::new(group), start_group]);
    list.playhead_cue_id = Some(start_group_id);
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();

    assert_eq!(
        list.get(&target_id).unwrap().state(),
        CueState::Standby,
        "Reset inside a Group started by Start must cancel a later fired Start child"
    );
}

#[test]
fn sequential_group_auto_continue_overlaps_children_at_action_start() {
    let registry = full_registry();
    let first = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::AutoContinue,
        Duration::ZERO,
    );
    let second = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    );
    let mut group = GroupCue::new();
    group.mode = GroupMode::Sequential;
    group.children.push(first);
    group.children.push(second);
    let (context, _events, _) = recording_context();

    group.go(&context).unwrap();

    assert_eq!(group.children[0].state(), CueState::Running);
    assert_eq!(
        group.children[1].state(),
        CueState::Running,
        "zero-wait Auto-Continue should start the next child while the first is still running"
    );
}

#[test]
fn sequential_group_auto_continue_post_wait_runs_from_action_start() {
    let registry = full_registry();
    let first = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::AutoContinue,
        Duration::from_millis(30),
    );
    let second = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    );
    let mut group = GroupCue::new();
    group.mode = GroupMode::Sequential;
    group.children.push(first);
    group.children.push(second);
    let (context, _events, _) = recording_context();

    group.go(&context).unwrap();
    assert_eq!(group.children[0].state(), CueState::Running);
    assert_eq!(group.children[1].state(), CueState::Standby);

    std::thread::sleep(Duration::from_millis(45));
    group.tick(&context).unwrap();

    assert_eq!(group.children[0].state(), CueState::Running);
    assert_eq!(
        group.children[1].state(),
        CueState::Running,
        "Auto-Continue post-wait is measured from action start, not action completion"
    );
}

#[test]
fn sequential_group_auto_follow_waits_for_completion_then_post_wait() {
    let registry = full_registry();
    let first = configured_wait(
        &registry,
        Duration::from_millis(25),
        ContinueMode::AutoFollow,
        Duration::from_millis(50),
    );
    let second = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    );
    let mut group = GroupCue::new();
    group.mode = GroupMode::Sequential;
    group.children.push(first);
    group.children.push(second);
    let (context, _events, _) = recording_context();

    group.go(&context).unwrap();
    assert_eq!(group.children[0].state(), CueState::Running);
    assert_eq!(group.children[1].state(), CueState::Standby);

    std::thread::sleep(Duration::from_millis(35));
    group.tick(&context).unwrap();
    assert_eq!(group.children[0].state(), CueState::Standby);
    assert_eq!(
        group.children[1].state(),
        CueState::Standby,
        "Auto-Follow must wait for completion and then honor its post-wait"
    );

    std::thread::sleep(Duration::from_millis(60));
    group.tick(&context).unwrap();

    assert_eq!(group.children[1].state(), CueState::Running);
}

#[test]
fn sequential_group_absorbed_go_invalidates_old_auto_continue_deadline() {
    let registry = full_registry();
    let first = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::AutoContinue,
        Duration::from_millis(50),
    );
    let second = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    );
    let third = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    );
    let mut group = GroupCue::new();
    group.mode = GroupMode::Sequential;
    group.children.push(first);
    group.children.push(second);
    group.children.push(third);
    let (context, _events, _) = recording_context();

    group.go(&context).unwrap();
    group.go(&context).unwrap(); // An operator GO advances to child 2 before A's timer.
    assert_eq!(group.children[1].state(), CueState::Running);
    assert_eq!(group.children[2].state(), CueState::Standby);

    std::thread::sleep(Duration::from_millis(65));
    group.tick(&context).unwrap();

    assert_eq!(group.children[1].state(), CueState::Running);
    assert_eq!(
        group.children[2].state(),
        CueState::Standby,
        "Auto-Continue deadline belonging to child A must not skip child B after an absorbed GO"
    );
}

#[test]
fn sequential_group_auto_continue_deadline_freezes_with_paused_child() {
    let registry = full_registry();
    let first = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::AutoContinue,
        Duration::from_millis(100),
    );
    let second = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    );
    let mut group = GroupCue::new();
    group.mode = GroupMode::Sequential;
    group.children.push(first);
    group.children.push(second);
    let (context, _events, _) = recording_context();

    group.go(&context).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    group.children[0].pause(&context).unwrap();

    std::thread::sleep(Duration::from_millis(110));
    group.tick(&context).unwrap();
    assert_eq!(
        group.children[1].state(),
        CueState::Standby,
        "a paused child must not fire its Auto-Continue successor at the old deadline"
    );

    group.children[0].resume(&context).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    group.tick(&context).unwrap();
    assert_eq!(group.children[1].state(), CueState::Standby);

    std::thread::sleep(Duration::from_millis(80));
    group.tick(&context).unwrap();
    assert_eq!(group.children[1].state(), CueState::Running);
}

fn assert_reset_cancels_group_post_wait(
    mode: GroupMode,
    source: Box<dyn Cue>,
    time_before_pending: Duration,
) {
    let registry = full_registry();
    let source_id = source.id();
    let next = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    );
    let next_id = next.id();
    let mut group = GroupCue::new();
    group.mode = mode;
    group.children.push(source);
    group.children.push(next);
    let group_id = group.id();
    let reset_source = command(&registry, CueType::Reset, &[source_id]);
    let reset_id = reset_source.id();
    let mut list = list_of(vec![Box::new(group), reset_source]);
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context.clone());

    list.playhead_cue_id = Some(group_id);
    transport.go(&mut list).unwrap();
    if !time_before_pending.is_zero() {
        std::thread::sleep(time_before_pending);
    }
    list.get_mut(&group_id).unwrap().tick(&context).unwrap();
    assert_eq!(
        list.get_recursive(&source_id).unwrap().state(),
        CueState::Standby,
        "the source should have completed and been reset before its post-wait"
    );

    list.playhead_cue_id = Some(reset_id);
    transport.go(&mut list).unwrap();
    std::thread::sleep(Duration::from_millis(55));
    list.get_mut(&group_id).unwrap().tick(&context).unwrap();

    assert_eq!(
        list.get_recursive(&next_id).unwrap().state(),
        CueState::Standby,
        "Reset of the completed source must invalidate its pending Group advance"
    );

    // A stale pending entry must not fall through into the normal completion
    // path and re-arm itself. Wait another complete post-wait interval and
    // tick again; only an operator GO should advance the sequence now.
    std::thread::sleep(Duration::from_millis(55));
    list.get_mut(&group_id).unwrap().tick(&context).unwrap();
    assert_eq!(
        list.get_recursive(&next_id).unwrap().state(),
        CueState::Standby,
        "Reset must not re-arm the invalidated post-wait on a later tick"
    );

    list.playhead_cue_id = Some(group_id);
    transport.go(&mut list).unwrap();
    assert_eq!(
        list.get_recursive(&next_id).unwrap().state(),
        CueState::Running,
        "a manual GO should still advance after the stale auto-advance is cancelled"
    );
}

#[test]
fn reset_source_cancels_sequential_group_auto_follow_post_wait() {
    let registry = full_registry();
    let source = configured_wait(
        &registry,
        Duration::from_millis(5),
        ContinueMode::AutoFollow,
        Duration::from_millis(40),
    );

    assert_reset_cancels_group_post_wait(GroupMode::Sequential, source, Duration::from_millis(12));
}

#[test]
fn reset_source_cancels_playlist_post_wait() {
    let registry = full_registry();
    let source = configured_wait(
        &registry,
        Duration::from_millis(5),
        ContinueMode::AutoFollow,
        Duration::from_millis(40),
    );

    assert_reset_cancels_group_post_wait(GroupMode::Playlist, source, Duration::from_millis(12));
}

#[test]
fn reset_cancels_playlist_post_wait_for_instant_memo_child() {
    let registry = full_registry();
    let mut source = registry.create(&CueType::Memo).unwrap();
    source.set_post_wait(Duration::from_millis(40));

    assert_reset_cancels_group_post_wait(GroupMode::Playlist, source, Duration::ZERO);
}

#[test]
fn duplicate_start_actions_advance_a_running_sequential_group_only_once() {
    let registry = full_registry();
    let mut sequence = GroupCue::new();
    sequence.mode = GroupMode::Sequential;
    sequence.children.push(configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    ));
    sequence.children.push(configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    ));
    sequence.children.push(configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    ));
    let sequence_id = sequence.id();
    let first_start = command(&registry, CueType::Start, &[sequence_id]);
    let second_start = command(&registry, CueType::Start, &[sequence_id]);
    let mut simultaneous = GroupCue::new();
    simultaneous.mode = GroupMode::Simultaneous;
    simultaneous.children.push(first_start);
    simultaneous.children.push(second_start);
    let simultaneous_id = simultaneous.id();
    let mut list = list_of(vec![Box::new(sequence), Box::new(simultaneous)]);
    let (context, _events, _) = recording_context();
    list.get_mut(&sequence_id).unwrap().go(&context).unwrap();
    list.playhead_cue_id = Some(simultaneous_id);
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();

    let running_children = list
        .get(&sequence_id)
        .unwrap()
        .child_cues()
        .unwrap()
        .iter()
        .filter(|child| child.state() == CueState::Running)
        .count();
    assert_eq!(
        running_children, 2,
        "the original child and exactly one absorbed-GO child should be running"
    );
    assert_eq!(
        list.get(&sequence_id).unwrap().child_cues().unwrap()[2].state(),
        CueState::Standby,
        "two Start actions in one batch must not advance the Group twice"
    );
}

fn assert_action_clock_freezes(
    cue: &mut Box<dyn Cue>,
    context: &inkue_lib::cue::context::CueContext,
    label: &str,
) {
    cue.go(context).unwrap();
    assert_eq!(cue.state(), CueState::Running, "{label} must enter Running");
    std::thread::sleep(Duration::from_millis(12));
    cue.pause(context).unwrap();
    assert_eq!(cue.state(), CueState::Paused, "{label} must enter Paused");
    let elapsed_at_pause = cue.elapsed();
    let action_at_pause = cue.action_elapsed();

    std::thread::sleep(Duration::from_millis(25));

    assert!(
        cue.elapsed() <= elapsed_at_pause + Duration::from_millis(5),
        "{label} elapsed clock advanced while paused"
    );
    assert!(
        cue.action_elapsed() <= action_at_pause + Duration::from_millis(5),
        "{label} action clock advanced while paused"
    );
    cue.resume(context).unwrap();
    std::thread::sleep(Duration::from_millis(18));
    assert!(
        cue.action_elapsed() >= action_at_pause + Duration::from_millis(10),
        "{label} action clock did not resume"
    );
    cue.stop(context).unwrap();
}

#[test]
fn pause_resume_freezes_action_clocks_for_media_and_timed_cues() {
    let registry = full_registry();
    let input_patch = InputPatch::new("Test input", "fake-input-device", vec![0, 1]);
    let (context, _events, _) =
        recording_context_with(Vec::new(), Vec::new(), vec![input_patch.clone()]);

    let audio = preloaded_audio(&registry);

    let mut video_json = registry.create(&CueType::Video).unwrap().serialize();
    video_json["file_path"] = serde_json::json!("video/test.mp4");
    let mut video = registry.from_json(video_json).unwrap();
    preload_silent_video_audio(video.as_mut());

    let mut image_json = registry.create(&CueType::Image).unwrap().serialize();
    image_json["file_path"] = serde_json::json!("image/test.png");
    let image = registry.from_json(image_json).unwrap();

    let mut text_json = registry.create(&CueType::Text).unwrap().serialize();
    text_json["text"] = serde_json::json!("clock test");
    let text = registry.from_json(text_json).unwrap();

    let mut mic_json = registry.create(&CueType::Mic).unwrap().serialize();
    mic_json["input_patch_id"] = serde_json::json!(input_patch.id);
    let mic = registry.from_json(mic_json).unwrap();

    let midi = registry.create(&CueType::MidiFile).unwrap();
    let mut group = GroupCue::new();
    group.mode = GroupMode::Sequential;
    group.children.push(configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    ));
    let fade = FadeCue::new();
    let wait = configured_wait(
        &registry,
        Duration::from_secs(5),
        ContinueMode::DoNotContinue,
        Duration::ZERO,
    );

    let mut cues: Vec<(&str, Box<dyn Cue>)> = vec![
        ("Audio", audio),
        ("Video", video),
        ("Image", image),
        ("Text", text),
        ("Mic", mic),
        ("MIDI File", midi),
        ("Group", Box::new(group)),
        ("Fade", Box::new(fade)),
        ("Wait", wait),
    ];

    for (label, cue) in &mut cues {
        assert_action_clock_freezes(cue, &context, label);
    }
}

fn assert_pre_wait_pauses_without_starting_action(
    cue: &mut Box<dyn Cue>,
    context: &inkue_lib::cue::context::CueContext,
    label: &str,
) {
    cue.set_pre_wait(Duration::from_millis(18));
    cue.go(context).unwrap();
    cue.pause(context).unwrap();
    assert_eq!(
        cue.state(),
        CueState::Paused,
        "{label} pre-wait must be pausable"
    );

    std::thread::sleep(Duration::from_millis(30));
    cue.tick(context).unwrap();
    assert!(
        !cue.is_action_started(),
        "{label} started its action while paused in pre-wait"
    );
    assert_eq!(cue.state(), CueState::Paused);

    cue.resume(context).unwrap();
    std::thread::sleep(Duration::from_millis(24));
    cue.tick(context).unwrap();
    assert!(
        cue.is_action_started(),
        "{label} did not resume its pre-wait clock"
    );
    cue.stop(context).unwrap();
}

#[test]
fn pause_resume_freezes_pre_wait_for_midi_file_mic_image_and_text() {
    let registry = full_registry();
    let input_patch = InputPatch::new("Test input", "fake-input-device", vec![0, 1]);
    let (context, _events, _) =
        recording_context_with(Vec::new(), Vec::new(), vec![input_patch.clone()]);

    let midi = registry.create(&CueType::MidiFile).unwrap();

    let mut mic_json = registry.create(&CueType::Mic).unwrap().serialize();
    mic_json["input_patch_id"] = serde_json::json!(input_patch.id);
    let mic = registry.from_json(mic_json).unwrap();

    let mut image_json = registry.create(&CueType::Image).unwrap().serialize();
    image_json["file_path"] = serde_json::json!("image/test.png");
    let image = registry.from_json(image_json).unwrap();

    let mut text_json = registry.create(&CueType::Text).unwrap().serialize();
    text_json["text"] = serde_json::json!("pre-wait test");
    let text = registry.from_json(text_json).unwrap();

    let mut cues: Vec<(&str, Box<dyn Cue>)> = vec![
        ("MIDI File", midi),
        ("Mic", mic),
        ("Image", image),
        ("Text", text),
    ];
    for (label, cue) in &mut cues {
        assert_pre_wait_pauses_without_starting_action(cue, &context, label);
    }
}

#[test]
fn paused_audio_child_keeps_group_incomplete_and_retains_voice_until_resume_or_stop() {
    let registry = full_registry();
    let audio = preloaded_audio(&registry);
    let mut group = GroupCue::new();
    group.mode = GroupMode::Simultaneous;
    group.children.push(audio);
    let (context, _events, _) = recording_context();

    group.go(&context).unwrap();
    let voice_id = group.children[0].playing_voice_id().unwrap();
    assert!(context.audio_engine.voice_is_alive(voice_id));

    group.children[0].pause(&context).unwrap();

    assert_eq!(group.children[0].state(), CueState::Paused);
    assert!(!group.is_complete(), "a paused child is still active work");
    assert!(group.all_voice_ids().contains(&voice_id));
    assert!(context.audio_engine.voice_is_alive(voice_id));

    group.children[0].resume(&context).unwrap();
    assert_eq!(group.children[0].state(), CueState::Running);
    group.children[0].pause(&context).unwrap();
    group.stop(&context).unwrap();

    assert_eq!(group.children[0].state(), CueState::Standby);
    assert!(!context.audio_engine.voice_is_alive(voice_id));
}

#[test]
fn stop_targeting_a_group_child_stops_the_nested_cue() {
    let registry = full_registry();
    let child = registry.create(&CueType::Wait).unwrap();
    let child_id = child.id();
    let mut group = GroupCue::new();
    group.mode = GroupMode::Simultaneous;
    group.children.push(child);
    let group_id = group.id();
    let stop = command(&registry, CueType::Stop, &[child_id]);
    let stop_id = stop.id();
    let mut list = list_of(vec![Box::new(group), stop]);
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context);

    list.playhead_cue_id = Some(group_id);
    transport.go(&mut list).unwrap();
    assert!(list.get_recursive(&child_id).unwrap().is_running());

    list.playhead_cue_id = Some(stop_id);
    let result = transport.go(&mut list).unwrap();

    assert_eq!(
        list.get_recursive(&child_id).unwrap().state(),
        CueState::Standby,
        "a Stop Cue must resolve a target nested under a Group",
    );
    assert!(result.stopped.contains(&child_id));
}

#[test]
fn simultaneous_group_skips_a_disarmed_child() {
    let registry = full_registry();
    let mut disarmed = registry.create(&CueType::Wait).unwrap();
    let disarmed_id = disarmed.id();
    disarmed.set_disabled(true);
    let enabled = registry.create(&CueType::Wait).unwrap();
    let enabled_id = enabled.id();
    let mut group = GroupCue::new();
    group.mode = GroupMode::Simultaneous;
    group.children.push(disarmed);
    group.children.push(enabled);
    let group_id = group.id();
    let mut list = list_of(vec![Box::new(group)]);
    list.playhead_cue_id = Some(group_id);
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();

    assert_eq!(
        list.get_recursive(&disarmed_id).unwrap().state(),
        CueState::Standby
    );
    assert!(list.get_recursive(&enabled_id).unwrap().is_running());
}

#[test]
fn sequential_group_skips_disarmed_child() {
    let registry = full_registry();
    let mut disarmed = registry.create(&CueType::Wait).unwrap();
    let disarmed_id = disarmed.id();
    disarmed.set_disabled(true);
    let enabled = registry.create(&CueType::Wait).unwrap();
    let enabled_id = enabled.id();
    let mut group = GroupCue::new();
    group.mode = GroupMode::Sequential;
    group.children.push(disarmed);
    group.children.push(enabled);
    let group_id = group.id();
    let mut list = list_of(vec![Box::new(group)]);
    list.playhead_cue_id = Some(group_id);
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();

    assert_eq!(
        list.get_recursive(&disarmed_id).unwrap().state(),
        CueState::Standby
    );
    assert!(list.get_recursive(&enabled_id).unwrap().is_running());
}

#[test]
fn playlist_skips_disarmed_child() {
    let registry = full_registry();
    let mut disarmed = registry.create(&CueType::Wait).unwrap();
    let disarmed_id = disarmed.id();
    disarmed.set_disabled(true);
    let enabled = registry.create(&CueType::Wait).unwrap();
    let enabled_id = enabled.id();
    let mut group = GroupCue::new();
    group.mode = GroupMode::Playlist;
    group.children.push(disarmed);
    group.children.push(enabled);
    let group_id = group.id();
    let mut list = list_of(vec![Box::new(group)]);
    list.playhead_cue_id = Some(group_id);
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();

    assert_eq!(
        list.get_recursive(&disarmed_id).unwrap().state(),
        CueState::Standby
    );
    assert!(list.get_recursive(&enabled_id).unwrap().is_running());
}

#[test]
fn playlist_start_child_executes_its_control_action() {
    let registry = full_registry();
    let target = registry.create(&CueType::Wait).unwrap();
    let target_id = target.id();
    let start = command(&registry, CueType::Start, &[target_id]);
    let mut playlist = GroupCue::new();
    playlist.mode = GroupMode::Playlist;
    playlist.children.push(start);
    let playlist_id = playlist.id();
    let mut list = list_of(vec![target, Box::new(playlist)]);
    list.playhead_cue_id = Some(playlist_id);
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();

    assert!(
        list.get(&target_id).unwrap().is_running(),
        "a Start cue fired by a Playlist must execute its action through Transport",
    );
}

#[test]
fn absorbed_sequential_group_go_dispatches_nested_child_action() {
    let registry = full_registry();
    let target = registry.create(&CueType::Wait).unwrap();
    let target_id = target.id();
    let first_child = registry.create(&CueType::Wait).unwrap();
    let start = command(&registry, CueType::Start, &[target_id]);
    let mut group = GroupCue::new();
    group.mode = GroupMode::Sequential;
    group.children.push(first_child);
    group.children.push(start);
    let group_id = group.id();
    let mut list = list_of(vec![target, Box::new(group)]);
    list.playhead_cue_id = Some(group_id);
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();
    assert!(list.get(&group_id).unwrap().absorbs_go());
    // Park the Playhead on the running group as an operator can do manually;
    // the next GO is absorbed by the group's inner sequence.
    list.playhead_cue_id = Some(group_id);
    transport.go(&mut list).unwrap();

    assert!(
        list.get(&target_id).unwrap().is_running(),
        "GO absorbed by Sequential Group must dispatch the newly fired Start child",
    );
}

#[test]
fn playlist_stop_child_stops_its_target() {
    let registry = full_registry();
    let mut target = registry.create(&CueType::Wait).unwrap();
    let target_id = target.id();
    let (context, _events, _) = recording_context();
    target.go(&context).unwrap();
    let stop = command(&registry, CueType::Stop, &[target_id]);
    let mut playlist = GroupCue::new();
    playlist.mode = GroupMode::Playlist;
    playlist.children.push(stop);
    let playlist_id = playlist.id();
    let mut list = list_of(vec![target, Box::new(playlist)]);
    list.playhead_cue_id = Some(playlist_id);
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();

    assert_eq!(list.get(&target_id).unwrap().state(), CueState::Standby);
}

#[test]
fn playlist_fade_child_receives_nested_target_voice() {
    let registry = full_registry();
    let audio = preloaded_audio(&registry);
    let audio_id = audio.id();
    let mut fade = FadeCue::new();
    fade.target_cue_ids = vec![audio_id];
    fade.target_volume_db = -30.0;
    fade.fade_duration_ms = 0;
    let mut playlist = GroupCue::new();
    playlist.mode = GroupMode::Playlist;
    playlist.children.push(Box::new(fade));
    let playlist_id = playlist.id();
    let mut list = list_of(vec![audio, Box::new(playlist)]);
    let (context, _events, log) = recording_context();
    list.get_mut(&audio_id).unwrap().go(&context).unwrap();
    list.playhead_cue_id = Some(playlist_id);
    let mut transport = Transport::new(context.clone());

    transport.go(&mut list).unwrap();
    list.get_mut(&playlist_id).unwrap().tick(&context).unwrap();

    assert!(
        audio_gain_count(&log) > 0,
        "a Fade cue inside Playlist must receive the targeted audio voice and drive it",
    );
}

#[test]
fn playlist_devamp_child_releases_nested_target_voice() {
    let registry = full_registry();
    let audio = preloaded_audio(&registry);
    let audio_id = audio.id();
    let devamp = command(&registry, CueType::Devamp, &[audio_id]);
    let mut playlist = GroupCue::new();
    playlist.mode = GroupMode::Playlist;
    playlist.children.push(devamp);
    let playlist_id = playlist.id();
    let mut list = list_of(vec![audio, Box::new(playlist)]);
    let (context, _events, log) = recording_context();
    list.get_mut(&audio_id).unwrap().go(&context).unwrap();
    list.playhead_cue_id = Some(playlist_id);
    let mut transport = Transport::new(context);

    transport.go(&mut list).unwrap();

    assert_eq!(audio_devamp_count(&log), 1);
}

#[test]
fn video_auto_continue_starts_audio_then_targeted_stop_stops_video() {
    let registry = full_registry();
    let mut video_json = registry.create(&CueType::Video).unwrap().serialize();
    video_json["file_path"] = serde_json::json!("video/loop.mp4");
    let mut video = registry.from_json(video_json).unwrap();
    preload_silent_video_audio(video.as_mut());
    video.set_continue_mode(ContinueMode::AutoContinue);
    let video_id = video.id();

    let mut audio = preloaded_audio(&registry);
    audio.set_continue_mode(ContinueMode::AutoFollow);
    let audio_id = audio.id();
    let stop = command(&registry, CueType::Stop, &[video_id]);
    let stop_id = stop.id();
    let mut list = list_of(vec![video, audio, stop]);
    let (context, _events, _) = recording_context();
    let mut transport = Transport::new(context);

    list.playhead_cue_id = Some(video_id);
    let chain = transport.go(&mut list).unwrap();
    assert_eq!(chain.triggered, vec![video_id, audio_id]);
    assert!(list.get(&video_id).unwrap().is_running());
    assert!(list.get(&audio_id).unwrap().is_running());
    assert_eq!(list.playhead_cue_id, Some(stop_id));

    // The recording engine has no playback clock/status thread. Fire the Stop
    // that the audio's Auto-Follow reaches after natural completion.
    let stopped = transport.go(&mut list).unwrap();

    assert!(stopped.stopped.contains(&video_id));
    assert_eq!(list.get(&video_id).unwrap().state(), CueState::Standby);
}

fn start_video_audio_stop_scenario(
    registry: &CueRegistry,
    audio_mode: ContinueMode,
    audio_post_wait: Duration,
) -> (
    CueList,
    CueContext,
    common::CallLog,
    common::VoiceLiveness,
    Transport,
    uuid::Uuid,
    uuid::Uuid,
    uuid::Uuid,
    u64,
) {
    let mut video_json = registry.create(&CueType::Video).unwrap().serialize();
    video_json["file_path"] = serde_json::json!("video/loop.mp4");
    video_json["loop_count"] = serde_json::json!(u32::MAX);
    let mut video = registry.from_json(video_json).unwrap();
    preload_silent_video_audio(video.as_mut());
    video.set_continue_mode(ContinueMode::AutoContinue);
    let video_id = video.id();

    let mut audio = preloaded_audio(registry);
    audio.set_continue_mode(audio_mode);
    audio.set_post_wait(audio_post_wait);
    let audio_id = audio.id();
    let stop = command(registry, CueType::Stop, &[video_id]);
    let stop_id = stop.id();
    let mut list = list_of(vec![video, audio, stop]);
    let (context, _events, log, voices) = common::recording_context_with_voice_liveness();
    let mut transport = Transport::new(context.clone());

    list.set_playhead(Some(video_id)).unwrap();
    let initial = transport.go(&mut list).unwrap();
    assert_eq!(initial.triggered, vec![video_id, audio_id]);
    assert_eq!(list.playhead_cue_id, Some(stop_id));
    assert_eq!(audio_play_count(&log), 1);
    let token = list.continuation_plan(audio_id).unwrap().token;
    assert_eq!(
        list.continuation_plan(audio_id).unwrap().target_ids,
        vec![stop_id],
        "Audio's successor must be snapshotted as Stop when Audio starts"
    );

    (
        list, context, log, voices, transport, video_id, audio_id, stop_id, token,
    )
}

#[test]
fn selecting_running_audio_does_not_replace_its_armed_auto_follow_successor() {
    let registry = full_registry();
    let (mut list, context, log, voices, mut transport, video_id, audio_id, stop_id, token) =
        start_video_audio_stop_scenario(&registry, ContinueMode::AutoFollow, Duration::ZERO);

    // The operator scrubs the running Audio cue (the backend command calls
    // Cue::seek) and selects its row, moving the visible/current Playhead back
    // onto Audio while its captured continuation successor remains Stop.
    for position_ms in [4_000, 11_000, 2_500, 18_000] {
        list.get_mut(&audio_id).unwrap().seek(position_ms, &context);
    }
    list.set_playhead(Some(audio_id)).unwrap();
    assert_eq!(list.playhead_cue_id, Some(audio_id));

    // Model the engine reporting natural EOF; EventLoop resets the completed
    // cue and marks its continuation execution before dispatching the captured
    // Auto-Follow plan. Keep the operator's selection on Audio after EOF too.
    let voice_id = list.get(&audio_id).unwrap().playing_voice_id().unwrap();
    voices.complete(voice_id);
    let audio = list.get_mut(&audio_id).unwrap();
    audio.reset().unwrap();
    audio.mark_auto_continue_fired();
    list.set_playhead(Some(audio_id)).unwrap();
    let auto_follow = transport
        .continue_from_source(&mut list, audio_id, token)
        .unwrap();

    assert!(
        auto_follow.fired.contains(&stop_id),
        "Auto-Follow must fire the successor armed when the Audio cue was started; got fired={:?}, triggered={:?}",
        auto_follow.fired,
        auto_follow.triggered
    );
    assert!(auto_follow.stopped.contains(&video_id));
    assert_eq!(list.get(&video_id).unwrap().state(), CueState::Standby);
    assert_eq!(
        audio_play_count(&log),
        1,
        "selection/scrubbing must not make the Auto-Follow GO restart Audio"
    );
}

#[test]
fn selection_during_delayed_auto_follow_does_not_redirect_captured_stop() {
    let registry = full_registry();
    let post_wait = Duration::from_millis(40);
    let (mut list, context, log, voices, mut transport, video_id, audio_id, stop_id, token) =
        start_video_audio_stop_scenario(&registry, ContinueMode::AutoFollow, post_wait);

    for position_ms in [5_000, 12_000, 3_000] {
        list.get_mut(&audio_id).unwrap().seek(position_ms, &context);
    }
    list.set_playhead(Some(audio_id)).unwrap();

    let voice_id = list.get(&audio_id).unwrap().playing_voice_id().unwrap();
    voices.complete(voice_id);
    let audio = list.get_mut(&audio_id).unwrap();
    audio.reset().unwrap();
    audio.mark_auto_continue_fired();

    // Selection changes after completion but before the real post-wait deadline
    // must not retarget the pending Auto-Follow to Audio itself.
    list.set_playhead(Some(audio_id)).unwrap();
    std::thread::sleep(post_wait + Duration::from_millis(15));
    let result = transport
        .continue_from_source(&mut list, audio_id, token)
        .unwrap();

    assert!(result.fired.contains(&stop_id));
    assert!(result.stopped.contains(&video_id));
    assert_eq!(audio_play_count(&log), 1);
}

#[test]
fn explicit_goto_cancels_delayed_captured_auto_follow() {
    let registry = full_registry();
    let post_wait = Duration::from_millis(40);
    let mut video_json = registry.create(&CueType::Video).unwrap().serialize();
    video_json["file_path"] = serde_json::json!("video/loop.mp4");
    video_json["loop_count"] = serde_json::json!(u32::MAX);
    let mut video = registry.from_json(video_json).unwrap();
    preload_silent_video_audio(video.as_mut());
    video.set_continue_mode(ContinueMode::AutoContinue);
    let video_id = video.id();

    let mut audio = preloaded_audio(&registry);
    audio.set_continue_mode(ContinueMode::AutoFollow);
    audio.set_post_wait(post_wait);
    let audio_id = audio.id();
    let stop = command(&registry, CueType::Stop, &[video_id]);
    let stop_id = stop.id();
    let goto = command(&registry, CueType::Goto, &[video_id]);
    let goto_id = goto.id();
    let mut list = list_of(vec![video, audio, stop, goto]);
    let (context, _events, _log, voices) = common::recording_context_with_voice_liveness();
    let mut transport = Transport::new(context);

    list.set_playhead(Some(video_id)).unwrap();
    assert_eq!(
        transport.go(&mut list).unwrap().triggered,
        vec![video_id, audio_id]
    );
    let token = list.continuation_plan(audio_id).unwrap().token;

    // Model natural Audio completion and the event loop arming its delayed AF.
    let voice_id = list.get(&audio_id).unwrap().playing_voice_id().unwrap();
    voices.complete(voice_id);
    let audio = list.get_mut(&audio_id).unwrap();
    audio.reset().unwrap();
    audio.mark_auto_continue_fired();

    // An explicit Goto action is an operator-directed transport command, unlike
    // merely selecting a row (covered above). It must cancel the captured
    // continuation so it cannot later replace the Goto destination with Stop.
    list.set_playhead(Some(goto_id)).unwrap();
    transport.go(&mut list).unwrap();
    assert_eq!(list.playhead_cue_id, Some(video_id));
    std::thread::sleep(post_wait + Duration::from_millis(15));
    let stale = transport
        .continue_from_source(&mut list, audio_id, token)
        .unwrap();

    assert!(stale.fired.is_empty());
    assert!(!stale.fired.contains(&stop_id));
    assert!(list.get(&video_id).unwrap().is_running());
}

#[test]
fn selection_during_delayed_auto_continue_does_not_redirect_captured_stop() {
    let registry = full_registry();
    let post_wait = Duration::from_millis(40);
    let (mut list, context, log, _voices, mut transport, video_id, audio_id, stop_id, token) =
        start_video_audio_stop_scenario(&registry, ContinueMode::AutoContinue, post_wait);

    for position_ms in [10_000, 1_000, 15_000] {
        list.get_mut(&audio_id).unwrap().seek(position_ms, &context);
    }
    list.set_playhead(Some(audio_id)).unwrap();
    std::thread::sleep(post_wait + Duration::from_millis(15));
    assert!(list.get(&audio_id).unwrap().auto_continue_elapsed() >= post_wait);
    list.get_mut(&audio_id).unwrap().mark_auto_continue_fired();

    let result = transport
        .continue_from_source(&mut list, audio_id, token)
        .unwrap();

    assert!(result.fired.contains(&stop_id));
    assert!(result.stopped.contains(&video_id));
    assert_eq!(list.get(&audio_id).unwrap().state(), CueState::Running);
    assert_eq!(audio_play_count(&log), 1);
}

#[test]
fn short_audio_completing_before_auto_continue_post_wait_keeps_captured_successor() {
    let registry = full_registry();
    let post_wait = Duration::from_millis(120);

    let mut video_json = registry.create(&CueType::Video).unwrap().serialize();
    video_json["file_path"] = serde_json::json!("video/loop.mp4");
    video_json["loop_count"] = serde_json::json!(u32::MAX);
    let mut video = registry.from_json(video_json).unwrap();
    preload_silent_video_audio(video.as_mut());
    video.set_continue_mode(ContinueMode::AutoContinue);
    let video_id = video.id();

    // 480 mono samples at 48 kHz is a real 10 ms media duration. The recording
    // engine does not end voices on its own, so the test reports natural EOF
    // explicitly after the cue's action clock passes that duration.
    let mut audio = registry.create(&CueType::Audio).unwrap();
    audio.accept_preloaded_audio(
        Arc::new(vec![0.0_f32; 480]),
        1,
        48_000,
        Duration::from_millis(10),
    );
    audio.set_continue_mode(ContinueMode::AutoContinue);
    audio.set_post_wait(post_wait);
    let audio_id = audio.id();
    let stop = command(&registry, CueType::Stop, &[video_id]);
    let stop_id = stop.id();
    let mut list = list_of(vec![video, audio, stop]);
    let (context, _events, log, voices) = common::recording_context_with_voice_liveness();
    let mut transport = Transport::new(context);

    list.set_playhead(Some(video_id)).unwrap();
    let initial = transport.go(&mut list).unwrap();
    assert_eq!(initial.triggered, vec![video_id, audio_id]);
    assert_eq!(audio_play_count(&log), 1);
    let token = list.continuation_plan(audio_id).unwrap().token;
    assert_eq!(
        list.continuation_plan(audio_id).unwrap().target_ids,
        vec![stop_id],
        "the delayed Auto-Continue successor is captured when Audio starts"
    );

    // Mimic the operator selecting/scrubbing Audio while it runs. Its action
    // finishes before the Auto-Continue clock reaches the configured delay.
    list.set_playhead(Some(audio_id)).unwrap();
    std::thread::sleep(Duration::from_millis(25));
    let cue = list.get(&audio_id).unwrap();
    let media_duration = cue.duration().unwrap();
    let continue_elapsed = cue.auto_continue_elapsed();
    assert!(cue.action_elapsed() >= media_duration);
    assert!(continue_elapsed < post_wait);
    let remaining = post_wait.saturating_sub(continue_elapsed);
    assert!(remaining > Duration::from_millis(50));

    // Natural completion resets the cue before its delayed Auto-Continue is
    // due. The event loop captures the remaining delay and marks this exact
    // execution; neither reset nor changing selection may discard its plan.
    let voice_id = list.get(&audio_id).unwrap().playing_voice_id().unwrap();
    voices.complete(voice_id);
    let audio = list.get_mut(&audio_id).unwrap();
    audio.reset().unwrap();
    audio.mark_auto_continue_fired();
    assert_eq!(list.get(&audio_id).unwrap().state(), CueState::Standby);
    assert_eq!(
        list.continuation_plan(audio_id).unwrap().token,
        token,
        "natural EOF before post-wait must preserve the captured continuation"
    );

    // Before the remaining deadline has elapsed, the event-loop equivalent
    // must keep waiting. Then dispatch through the captured-source API rather
    // than a manual GO, which intentionally follows the current Playhead.
    list.set_playhead(Some(audio_id)).unwrap();
    std::thread::sleep(remaining + Duration::from_millis(15));
    let result = transport
        .continue_from_source(&mut list, audio_id, token)
        .unwrap();

    assert!(result.fired.contains(&stop_id));
    assert!(result.stopped.contains(&video_id));
    assert_eq!(list.get(&video_id).unwrap().state(), CueState::Standby);
    assert_eq!(audio_play_count(&log), 1, "Audio must not be replayed");
}

#[test]
fn stop_all_cancels_delayed_auto_follow_after_source_naturally_completed() {
    let registry = full_registry();
    let post_wait = Duration::from_millis(40);
    let (mut list, _context, _log, voices, mut transport, video_id, audio_id, stop_id, token) =
        start_video_audio_stop_scenario(&registry, ContinueMode::AutoFollow, post_wait);

    let voice_id = list.get(&audio_id).unwrap().playing_voice_id().unwrap();
    voices.complete(voice_id);
    let audio = list.get_mut(&audio_id).unwrap();
    audio.reset().unwrap();
    audio.mark_auto_continue_fired();
    assert_eq!(list.get(&audio_id).unwrap().state(), CueState::Standby);

    transport.stop_all(&mut list).unwrap();
    std::thread::sleep(post_wait + Duration::from_millis(10));
    let stale = transport
        .continue_from_source(&mut list, audio_id, token)
        .unwrap();

    assert!(!stale.fired.contains(&stop_id));
    assert!(!stale.stopped.contains(&video_id));
    assert_eq!(list.get(&video_id).unwrap().state(), CueState::Standby);
}
