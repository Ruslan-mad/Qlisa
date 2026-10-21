//! Number lifecycle regressions for silent Video masters and looping actions.
//!
//! The recording output engine does not decode the virtual image paths. Using
//! image paths also makes VideoCue's audio probe return `None`, which models a
//! video with no audio track without requiring a native decoder or hardware.

mod common;

use std::time::Duration;

use common::{full_registry, recording_context, EngineCall};
use inkue_lib::cue::group_cue::GroupCue;
use inkue_lib::cue::registry::CueRegistry;
use inkue_lib::cue::traits::Cue;
use inkue_lib::cue::types::{CueState, CueType};

fn silent_video(
    registry: &CueRegistry,
    path: &str,
    output_id: &str,
    duration: Duration,
    loop_count: u32,
) -> Box<dyn Cue> {
    let mut value = registry.create(&CueType::Video).unwrap().serialize();
    value["file_path"] = serde_json::json!(path);
    value["output_ids"] = serde_json::json!([output_id]);
    value["loop_count"] = serde_json::json!(loop_count);
    let mut cue = registry.from_json(value).unwrap();
    cue.set_runtime_duration(duration);
    cue
}

fn two_video_number(master_loops: u32) -> (GroupCue, uuid::Uuid, uuid::Uuid) {
    let registry = full_registry();
    let master = silent_video(
        &registry,
        "short-master.png",
        "stage-left",
        Duration::from_millis(35),
        master_loops,
    );
    let master_id = master.id();
    let action = silent_video(
        &registry,
        "long-action.png",
        "stage-right",
        Duration::from_millis(110),
        u32::MAX,
    );
    let action_id = action.id();

    let mut number = GroupCue::new_number();
    number.children.push(master);
    number.children.push(action);
    number.set_number_master(master_id).unwrap();
    number.set_number_action_offset(action_id, 0).unwrap();
    (number, master_id, action_id)
}

#[test]
fn two_silent_infinite_video_loops_keep_number_running_past_both_source_lengths() {
    let (mut number, master_id, action_id) = two_video_number(u32::MAX);
    let (context, _events, log) = recording_context();

    assert_eq!(
        number.duration(),
        None,
        "an infinite Video master has no Number deadline"
    );
    number.go(&context).unwrap();

    std::thread::sleep(Duration::from_millis(180));
    number.tick(&context).unwrap();

    assert_eq!(number.state(), CueState::Running);
    assert!(!number.is_complete());
    assert!(number
        .child_cues()
        .unwrap()
        .iter()
        .all(|child| child.is_running()));
    assert_eq!(
        number
            .child_cues()
            .unwrap()
            .iter()
            .map(|child| child.all_voice_ids().len())
            .collect::<Vec<_>>(),
        vec![1, 1],
        "both silent Video cues keep their visual voices",
    );
    let calls = log.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, EngineCall::OutputShowContent { .. }))
            .count(),
        2,
        "Number starts both videos on their separately configured outputs",
    );
    assert!(!calls
        .iter()
        .any(|call| matches!(call, EngineCall::AudioPlayRouted { .. })));
    assert_eq!(number.number_master_id(), Some(master_id));
    assert_ne!(master_id, action_id);
}

#[test]
fn finite_video_master_ends_number_and_stops_its_infinite_video_action() {
    let (mut number, master_id, action_id) = two_video_number(1);
    let (context, _events, log) = recording_context();

    assert_eq!(number.duration(), Some(Duration::from_millis(70)));
    number.go(&context).unwrap();
    std::thread::sleep(Duration::from_millis(90));
    number.tick(&context).unwrap();

    assert!(
        number.is_complete(),
        "finite master owns the Number end time"
    );
    let children = number.child_cues().unwrap();
    assert_eq!(
        children
            .iter()
            .find(|child| child.id() == master_id)
            .unwrap()
            .state(),
        CueState::Standby
    );
    assert_eq!(
        children
            .iter()
            .find(|child| child.id() == action_id)
            .unwrap()
            .state(),
        CueState::Standby
    );
    assert!(log
        .lock()
        .unwrap()
        .iter()
        .any(|call| matches!(call, EngineCall::OutputStopContent { .. })));
}
