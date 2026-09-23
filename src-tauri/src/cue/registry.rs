//! [`CueRegistry`] maps [`CueType`] to a [`CueFactory`].
//!
//! To add a new cue type, implement [`Cue`](super::traits::Cue) and
//! [`CueFactory`](super::traits::CueFactory), then call
//! [`CueRegistry::register`] at startup.  No other code needs to change.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use serde_json::Value;

use super::{
    traits::{Cue, CueFactory},
    types::CueType,
};

/// Global factory registry.  All cue types must be registered before any
/// workspace can be loaded from JSON.
pub struct CueRegistry {
    factories: HashMap<CueType, Box<dyn CueFactory>>,
}

impl CueRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Register a factory for the given cue type.
    /// Overwrites any previously registered factory for that type.
    pub fn register(&mut self, cue_type: CueType, factory: Box<dyn CueFactory>) {
        self.factories.insert(cue_type, factory);
    }

    /// Create a fresh, default-initialised cue of the given type.
    pub fn create(&self, cue_type: &CueType) -> Result<Box<dyn Cue>> {
        self.factories
            .get(cue_type)
            .map(|f| f.create())
            .ok_or_else(|| anyhow!("No factory registered for cue type: {:?}", cue_type))
    }

    /// Deserialise a cue from its persisted JSON representation.
    /// The JSON must contain a `"type"` field that matches a registered [`CueType`].
    ///
    /// [`CueType::Group`] is handled specially: children are deserialised
    /// recursively using this same registry, bypassing the normal factory path.
    pub fn from_json(&self, value: Value) -> Result<Box<dyn Cue>> {
        let cue_type: CueType = serde_json::from_value(
            value
                .get("type")
                .cloned()
                .ok_or_else(|| anyhow!("Cue JSON missing 'type' field"))?,
        )?;

        if matches!(cue_type, CueType::Group | CueType::Number) {
            return super::group_cue::GroupCue::from_json_with_registry(&value, self);
        }

        self.factories
            .get(&cue_type)
            .ok_or_else(|| anyhow!("No factory registered for cue type: {:?}", cue_type))?
            .from_json(value)
    }

    /// Returns `true` if a factory is registered for the given type.
    pub fn has(&self, cue_type: &CueType) -> bool {
        self.factories.contains_key(cue_type)
    }
}

impl Default for CueRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::{
        audio_cue::AudioCueFactory,
        browser_cue::BrowserCueFactory,
        camera_cue::CameraCueFactory,
        control_cue::{ControlCueFactory, ALL_CONTROL_ACTIONS},
        devamp_cue::DevampCueFactory,
        fade_cue::FadeCueFactory,
        group_cue::GroupCueFactory,
        image_cue::ImageCueFactory,
        light_cue::LightCueFactory,
        memo_cue::{MemoCue, MemoCueFactory},
        mic_cue::MicCueFactory,
        midi_cue::MidiCueFactory,
        midi_file_cue::MidiFileCueFactory,
        osc_cue::OscCueFactory,
        script_cue::ScriptCueFactory,
        stop_cue::StopCueFactory,
        text_cue::TextCueFactory,
        timecode_cue::TimecodeCueFactory,
        video_cue::VideoCueFactory,
        wait_cue::WaitCueFactory,
        types::CueColor,
    };

    fn full_test_registry() -> CueRegistry {
        let mut registry = CueRegistry::new();
        registry.register(CueType::Audio, Box::new(AudioCueFactory));
        registry.register(CueType::Camera, Box::new(CameraCueFactory));
        registry.register(CueType::Browser, Box::new(BrowserCueFactory));
        registry.register(CueType::Devamp, Box::new(DevampCueFactory));
        registry.register(CueType::Fade, Box::new(FadeCueFactory));
        registry.register(CueType::Group, Box::new(GroupCueFactory));
        registry.register(CueType::Image, Box::new(ImageCueFactory));
        registry.register(CueType::Light, Box::new(LightCueFactory));
        registry.register(CueType::Memo, Box::new(MemoCueFactory));
        registry.register(CueType::Mic, Box::new(MicCueFactory));
        registry.register(CueType::Midi, Box::new(MidiCueFactory));
        registry.register(CueType::MidiFile, Box::new(MidiFileCueFactory));
        registry.register(CueType::Osc, Box::new(OscCueFactory));
        registry.register(CueType::Script, Box::new(ScriptCueFactory));
        registry.register(CueType::Stop, Box::new(StopCueFactory));
        registry.register(CueType::Text, Box::new(TextCueFactory));
        registry.register(CueType::Timecode, Box::new(TimecodeCueFactory));
        registry.register(CueType::Video, Box::new(VideoCueFactory));
        registry.register(CueType::Wait, Box::new(WaitCueFactory));
        for action in ALL_CONTROL_ACTIONS {
            registry.register(action.cue_type(), Box::new(ControlCueFactory(action)));
        }
        registry
    }

    #[test]
    fn register_and_create_memo() {
        let mut registry = CueRegistry::new();
        registry.register(CueType::Memo, Box::new(MemoCueFactory));

        let cue = registry.create(&CueType::Memo).expect("should create memo");
        assert_eq!(cue.cue_type(), CueType::Memo);
    }

    #[test]
    fn create_unknown_type_returns_error() {
        let registry = CueRegistry::new();
        let result = registry.create(&CueType::Audio);
        assert!(result.is_err(), "Expected error for unregistered type");
    }

    #[test]
    fn from_json_roundtrip_memo() {
        let mut registry = CueRegistry::new();
        registry.register(CueType::Memo, Box::new(MemoCueFactory));

        let mut cue = MemoCue::new();
        cue.set_name("Test Memo".to_string());
        cue.set_number(Some("1".to_string()));
        let json = cue.serialize();

        let deserialized = registry.from_json(json).expect("should deserialize");
        assert_eq!(deserialized.name(), "Test Memo");
        assert_eq!(deserialized.number(), Some("1"));
    }

    #[test]
    fn cue_types_have_no_implicit_color_and_missing_color_loads_neutral() {
        let registry = full_test_registry();
        let cue_types = [
            CueType::Audio,
            CueType::Memo,
            CueType::Wait,
            CueType::Group,
            CueType::Fade,
            CueType::Stop,
            CueType::Video,
            CueType::Image,
            CueType::Osc,
            CueType::Midi,
            CueType::MidiFile,
            CueType::Light,
            CueType::Mic,
            CueType::Timecode,
            CueType::Text,
            CueType::Camera,
            CueType::Browser,
            CueType::Devamp,
            CueType::Start,
            CueType::Pause,
            CueType::Resume,
            CueType::Load,
            CueType::Reset,
            CueType::Goto,
            CueType::Arm,
            CueType::Disarm,
            CueType::Script,
        ];

        for cue_type in cue_types {
            let cue = registry.create(&cue_type).unwrap();
            assert_eq!(cue.color(), CueColor::None, "new {cue_type} cue");

            let mut legacy_json = cue.serialize();
            legacy_json.as_object_mut().unwrap().remove("color");
            let legacy = registry.from_json(legacy_json).unwrap();
            assert_eq!(legacy.color(), CueColor::None, "legacy {cue_type} cue");

            let mut explicitly_colored_json = cue.serialize();
            explicitly_colored_json["color"] = serde_json::json!("purple");
            let explicitly_colored = registry.from_json(explicitly_colored_json).unwrap();
            assert_eq!(explicitly_colored.color(), CueColor::Purple, "saved {cue_type} cue");
        }
    }

    #[test]
    fn from_json_missing_type_returns_error() {
        let registry = CueRegistry::new();
        let json = serde_json::json!({ "name": "orphan" });
        assert!(registry.from_json(json).is_err());
    }

    #[test]
    fn register_and_create_audio() {
        let mut registry = CueRegistry::new();
        registry.register(CueType::Audio, Box::new(AudioCueFactory));
        let cue = registry.create(&CueType::Audio).expect("should create audio cue");
        assert_eq!(cue.cue_type(), CueType::Audio);
    }
}
