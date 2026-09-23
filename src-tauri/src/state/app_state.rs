//! Global application state shared across Tauri command handlers.
//!
//! All mutable state is wrapped in `Arc<Mutex<...>>` so it can be safely
//! accessed from multiple Tauri command handler threads.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::{
    cue::{
        audio_cue::AudioCueFactory, browser_cue::BrowserCueFactory, fade_cue::FadeCueFactory,
        group_cue::GroupCueFactory, image_cue::ImageCueFactory, light_cue::LightCueFactory,
        memo_cue::MemoCueFactory, midi_cue::MidiCueFactory, osc_cue::OscCueFactory,
        registry::CueRegistry, stop_cue::StopCueFactory, types::CueType,
        video_cue::VideoCueFactory, wait_cue::WaitCueFactory,
    },
    engine::{
        timecode_receiver::TimecodeReceiver, AudioEngine, DmxEngine, OscServer, OutputEngine,
    },
    show::{undo_stack::UndoStack, Workspace},
};

/// Serializes every global Preferences read-modify-save, including native
/// output-window geometry writes. This prevents a partial Settings apply from
/// writing a stale full tree over a neighboring section.
pub static GLOBAL_PREFERENCES_WRITE_GATE: Mutex<()> = Mutex::new(());

/// The Tauri managed state object.
pub struct AppState {
    /// The current workspace (project file).
    pub workspace: Arc<Mutex<Workspace>>,
    /// Authoritative machine-wide Preferences, mirrored into the workspace
    /// only for runtime consumers and legacy serialization compatibility.
    pub global_preferences: Arc<Mutex<crate::preferences::AppPreferences>>,
    /// Whether `preferences.json` has been materialized yet.  The first New
    /// or real workspace load seeds it; later projects never replace it.
    pub global_preferences_file_present: Arc<AtomicBool>,
    /// False only when an existing global file was unreadable/invalid. Such a
    /// file must not be overwritten by legacy-workspace migration.
    pub global_preferences_migration_allowed: Arc<AtomicBool>,
    /// The audio engine (shared; owns its own real-time thread internally).
    pub audio_engine: Arc<AudioEngine>,
    /// The unified output engine (video + image via libmpv Win32 window).
    pub output_engine: Arc<OutputEngine>,
    /// OSC receive server (background UDP listener thread).
    pub osc_server: Arc<OscServer>,
    /// DMX-over-IP lighting engine (owns its own ~40Hz output thread).
    pub dmx_engine: Arc<DmxEngine>,
    /// The cue type registry used for workspace de/serialisation.
    pub registry: Arc<Mutex<CueRegistry>>,
    /// Set of cue IDs whose audio files are currently being decoded in the
    /// background.  Used to show a "Loading…" indicator in the UI.
    pub loading_cues: Arc<Mutex<HashSet<Uuid>>>,
    /// Runtime-only media facts shown in the cue list. Probes happen off the
    /// command/UI path and are deduplicated by resolved file path.
    pub media_metadata: Arc<Mutex<crate::engine::media_metadata::MediaMetadataCache>>,
    /// Undo / redo history for the active cue list.
    pub undo_stack: Arc<Mutex<UndoStack>>,
    /// In-app clipboard: the last cue or cue-set copied via Ctrl+C
    /// (serialised JSON).
    pub clipboard: Arc<Mutex<Option<serde_json::Value>>>,
    /// Timestamp of the last GO trigger in ms since Unix epoch.
    /// Used to enforce `double_go_protection_ms` — any GO within that window
    /// is silently dropped.  Lock-free so it adds zero latency to the hot path.
    pub last_go_at: Arc<AtomicU64>,
    /// Timecode receiver (MTC / LTC) — `None` until the first `set_tc_config`.
    pub tc_receiver: Arc<Mutex<Option<Arc<TimecodeReceiver>>>>,
    /// MIDI input listener driving per-cue MIDI triggers — `None` while
    /// triggers are disabled for this machine.
    pub midi_listener: Arc<Mutex<Option<Arc<crate::engine::midi_trigger::MidiTriggerListener>>>>,
    /// The sole active headphone-preview voice. This is deliberately outside
    /// the workspace: previews are operator-local and must not alter cue or
    /// show state.
    pub preview_session: Arc<Mutex<Option<PreviewSession>>>,
    /// Monotonic request token for asynchronous headphone preview decoding.
    /// A newer drag/toggle invalidates older decodes before they can install a
    /// voice, so an out-of-order completion can never jump the preview back.
    pub preview_generation: Arc<AtomicU64>,
    /// Isolated FFmpeg conversion jobs and their cancellation handles.
    pub media_converter: Arc<crate::media_converter::MediaConverterManager>,
}

/// Runtime ownership of a preview mix. A new preview replaces this one, so
/// no two cue/panel previews can remain audible at once. `voice_id` is the
/// primary (Number master) voice used for the preview playhead; `voice_ids`
/// contains every voice in the mix and is stopped/pruned as one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewSession {
    pub cue_id: Uuid,
    pub voice_id: Uuid,
    pub voice_ids: std::sync::Arc<Vec<Uuid>>,
    /// Monotonic lifecycle token. UI observers discard lower-generation events
    /// so a delayed EOF/stop from an older preview cannot erase its replacement.
    pub generation: u64,
}

impl PreviewSession {
    pub fn new(cue_id: Uuid, voice_ids: Vec<Uuid>, generation: u64) -> Option<Self> {
        let voice_id = *voice_ids.first()?;
        Some(Self {
            cue_id,
            voice_id,
            voice_ids: std::sync::Arc::new(voice_ids),
            generation,
        })
    }

    pub fn all_voice_ids(&self) -> &[Uuid] {
        self.voice_ids.as_slice()
    }
}

impl AppState {
    /// Build the initial application state from already-constructed engines.
    pub fn new(
        audio_engine: Arc<AudioEngine>,
        output_engine: Arc<OutputEngine>,
        osc_server: Arc<OscServer>,
        dmx_engine: Arc<DmxEngine>,
        tc_receiver: Option<Arc<TimecodeReceiver>>,
        global_preferences: crate::preferences::AppPreferences,
        global_preferences_file_present: bool,
        global_preferences_migration_allowed: bool,
    ) -> Self {
        let workspace = Workspace::new_with_preferences("Untitled", global_preferences.clone());

        let mut registry = CueRegistry::new();
        registry.register(CueType::Audio, Box::new(AudioCueFactory));
        registry.register(CueType::Fade, Box::new(FadeCueFactory));
        registry.register(CueType::Midi, Box::new(MidiCueFactory));
        registry.register(
            CueType::MidiFile,
            Box::new(crate::cue::midi_file_cue::MidiFileCueFactory),
        );
        registry.register(CueType::Group, Box::new(GroupCueFactory));
        registry.register(
            CueType::Number,
            Box::new(crate::cue::group_cue::NumberCueFactory),
        );
        registry.register(CueType::Light, Box::new(LightCueFactory));
        registry.register(CueType::Memo, Box::new(MemoCueFactory));
        registry.register(CueType::Osc, Box::new(OscCueFactory));
        registry.register(CueType::Stop, Box::new(StopCueFactory));
        registry.register(
            CueType::Devamp,
            Box::new(crate::cue::devamp_cue::DevampCueFactory),
        );
        registry.register(CueType::Video, Box::new(VideoCueFactory));
        registry.register(CueType::Image, Box::new(ImageCueFactory));
        registry.register(CueType::Mic, Box::new(crate::cue::mic_cue::MicCueFactory));
        registry.register(
            CueType::Timecode,
            Box::new(crate::cue::timecode_cue::TimecodeCueFactory),
        );
        registry.register(
            CueType::Text,
            Box::new(crate::cue::text_cue::TextCueFactory),
        );
        registry.register(
            CueType::Camera,
            Box::new(crate::cue::camera_cue::CameraCueFactory),
        );
        registry.register(CueType::Browser, Box::new(BrowserCueFactory));
        registry.register(CueType::Wait, Box::new(WaitCueFactory));
        registry.register(
            CueType::Script,
            Box::new(crate::cue::script_cue::ScriptCueFactory),
        );
        // Command cues: eight distinct types over one shared implementation.
        for action in crate::cue::control_cue::ALL_CONTROL_ACTIONS {
            registry.register(
                action.cue_type(),
                Box::new(crate::cue::control_cue::ControlCueFactory(action)),
            );
        }

        Self {
            workspace: Arc::new(Mutex::new(workspace)),
            global_preferences: Arc::new(Mutex::new(global_preferences)),
            global_preferences_file_present: Arc::new(AtomicBool::new(
                global_preferences_file_present,
            )),
            global_preferences_migration_allowed: Arc::new(AtomicBool::new(
                global_preferences_migration_allowed,
            )),
            audio_engine,
            output_engine,
            osc_server,
            dmx_engine,
            registry: Arc::new(Mutex::new(registry)),
            loading_cues: Arc::new(Mutex::new(HashSet::new())),
            media_metadata: Arc::new(Mutex::new(
                crate::engine::media_metadata::MediaMetadataCache::default(),
            )),
            undo_stack: Arc::new(Mutex::new(UndoStack::new())),
            clipboard: Arc::new(Mutex::new(None)),
            last_go_at: Arc::new(AtomicU64::new(0)),
            tc_receiver: Arc::new(Mutex::new(tc_receiver)),
            midi_listener: Arc::new(Mutex::new(None)),
            preview_session: Arc::new(Mutex::new(None)),
            preview_generation: Arc::new(AtomicU64::new(0)),
            media_converter: Arc::new(crate::media_converter::MediaConverterManager::new()),
        }
    }

    /// Clone the authoritative machine-wide Preferences without touching the
    /// workspace lock.
    pub fn global_preferences_snapshot(
        &self,
    ) -> Result<crate::preferences::AppPreferences, String> {
        self.global_preferences
            .lock()
            .map(|preferences| preferences.clone())
            .map_err(|error| error.to_string())
    }

    /// Persist global Preferences first, then update the in-memory authority.
    /// The workspace mirror is updated by the caller after this succeeds, so a
    /// failed write cannot make runtime/UI state claim an unapplied setting.
    pub fn save_global_preferences(
        &self,
        mut preferences: crate::preferences::AppPreferences,
    ) -> Result<crate::preferences::AppPreferences, String> {
        let mut current = self
            .global_preferences
            .lock()
            .map_err(|error| error.to_string())?;
        crate::preferences::inject_runtime_audio_buffer_size(
            &mut preferences,
            crate::machine_config::load().buffer_size,
        );
        crate::machine_config::save_global_preferences(&preferences)
            .map_err(|error| error.to_string())?;
        *current = preferences.clone();
        self.global_preferences_file_present
            .store(true, Ordering::Release);
        self.global_preferences_migration_allowed
            .store(true, Ordering::Release);
        Ok(preferences)
    }

    /// Atomically update and persist the global Preferences tree while only
    /// holding the global-preferences mutex (never the workspace mutex).
    pub fn update_global_preferences<F>(
        &self,
        update: F,
    ) -> Result<crate::preferences::AppPreferences, String>
    where
        F: FnOnce(&mut crate::preferences::AppPreferences),
    {
        let mut current = self
            .global_preferences
            .lock()
            .map_err(|error| error.to_string())?;
        let mut next = current.clone();
        update(&mut next);
        crate::preferences::inject_runtime_audio_buffer_size(
            &mut next,
            crate::machine_config::load().buffer_size,
        );
        crate::machine_config::save_global_preferences(&next).map_err(|error| error.to_string())?;
        *current = next.clone();
        self.global_preferences_file_present
            .store(true, Ordering::Release);
        self.global_preferences_migration_allowed
            .store(true, Ordering::Release);
        Ok(next)
    }
}
