//! Application-wide preference types. The authoritative tree is persisted in
//! the machine config directory as `Inkue/preferences.json`; the same serde
//! shape remains inside `.inkue` as a compatibility/runtime mirror.
//!
//! Each top-level category (audio, general, network, display) is its own
//! struct so future categories can be added without touching existing ones.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::cue::types::{CueColor, CueType};

use crate::cue::types::FadeCurve;

/// Machine-local binding for a logical Aux audio bus. The bus ID is saved in
/// the show; this record stays in `audio.json` and is safe to change per rig.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineAudioBus {
    pub bus_id: uuid::Uuid,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub channels: Vec<u16>,
}

// ---------------------------------------------------------------------------
// Audio backend choice
// ---------------------------------------------------------------------------

/// Audio output backend.
///
/// `WasapiShared` / `WasapiExclusive` / `Asio` are Windows-specific.
/// `SystemDefault` is used on Mac / Linux where cpal picks CoreAudio or ALSA.
/// The `Default` impl is therefore per-OS, and every load goes through
/// [`AudioBackend::for_this_platform`] — see it for why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AudioBackend {
    #[cfg_attr(target_os = "windows", default)]
    WasapiShared,
    WasapiExclusive,
    Asio,
    #[cfg_attr(not(target_os = "windows"), default)]
    SystemDefault,
}

impl AudioBackend {
    /// Whether this backend can exist on the OS Inkue is running on.
    ///
    /// Driver *presence* is deliberately not considered: ASIO stays selectable
    /// on a Windows build with the feature compiled in even when no driver is
    /// installed, because `open_stream_inner` already falls back to the default
    /// host with a warning and a developer's saved config must survive.
    pub fn is_available_here(self) -> bool {
        #[cfg(target_os = "windows")]
        {
            #[cfg(feature = "asio-support")]
            {
                !matches!(self, AudioBackend::SystemDefault)
            }
            #[cfg(not(feature = "asio-support"))]
            {
                matches!(
                    self,
                    AudioBackend::WasapiShared | AudioBackend::WasapiExclusive
                )
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            matches!(self, AudioBackend::SystemDefault)
        }
    }

    /// This backend, or this platform's default when it cannot exist here.
    ///
    /// A machine config travels: copied between machines, restored from a
    /// backup, or simply written by a build whose default was Windows-only.
    /// Without this, Linux ran on `WasapiShared` — harmless (both resolve to
    /// `cpal::default_host()`) but it logged `backend=WasapiShared` on ALSA and
    /// left Preferences displaying a backend it could not offer.
    #[must_use]
    pub fn for_this_platform(self) -> Self {
        if self.is_available_here() {
            self
        } else {
            Self::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Audio preferences
// ---------------------------------------------------------------------------

/// Hardware-specific audio settings — device, backend, buffer size.
///
/// Stored in `%APPDATA%\Inkue\audio.json`, **not** in the workspace file,
/// because they describe the physical machine rather than the show.
/// Moving a `.inkue` file to another machine keeps its show defaults intact
/// while this config adapts to the local hardware.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineAudioConfig {
    /// WASAPI/ASIO backend to use.
    #[serde(default)]
    pub backend: AudioBackend,

    /// Identifier of the selected output device.  `None` = system default.
    #[serde(default)]
    pub device_id: Option<String>,

    /// Human-readable name of the selected output device, captured at selection
    /// time so a banner can show "Focusrite Scarlett…" instead of the raw
    /// WASAPI endpoint id even when the device is currently absent.  `None` =
    /// system default (or selected before this field existed).
    #[serde(default)]
    pub device_name: Option<String>,

    /// Identifier of the dedicated headphone / preview output. `None` means
    /// preview is deliberately disabled: preview commands return silence and a
    /// visible error rather than ever falling back to the main (program) PA.
    /// Like the main output it is machine-specific and is never stored in a
    /// workspace file.
    #[serde(default)]
    pub preview_device_id: Option<String>,

    /// Friendly label captured with [`Self::preview_device_id`], retained so a
    /// disconnected headphone interface remains intelligible in Settings.
    #[serde(default)]
    pub preview_device_name: Option<String>,

    /// ASIO stereo pair used by operator preview on the already-open stream.
    /// Ignored when the backend is not ASIO.
    #[serde(default)]
    pub preview_asio_pair: Option<u32>,

    /// Operator preview fader in dB. Machine-local and never part of a show.
    #[serde(default)]
    pub preview_gain_db: f32,

    /// Identifier of the selected audio **input** device for Mic Cues / live
    /// capture.  `None` = system default input.  Machine-specific, like
    /// `device_id`.
    #[serde(default)]
    pub input_device_id: Option<String>,

    /// Output buffer size in samples.
    /// Only applied for WASAPI Exclusive; ignored in Shared mode (Windows
    /// controls the period) and ASIO mode (driver controls its own buffer).
    #[serde(default = "MachineAudioConfig::default_buffer_size")]
    pub buffer_size: u32,

    /// ASIO output pair index (0 = Out 1-2, 1 = Out 3-4, …).
    /// Ignored when backend is not ASIO.
    #[serde(default)]
    pub asio_out_pair: u32,

    /// Physical bindings for logical Aux buses. Main uses `device_id` and
    /// `asio_out_pair` above; Preview uses its dedicated fields below.
    #[serde(default)]
    pub aux_buses: Vec<MachineAudioBus>,
}

impl MachineAudioConfig {
    fn default_buffer_size() -> u32 {
        256
    }
}

impl Default for MachineAudioConfig {
    fn default() -> Self {
        Self {
            backend: AudioBackend::default(),
            device_id: None,
            device_name: None,
            preview_device_id: None,
            preview_device_name: None,
            preview_asio_pair: None,
            preview_gain_db: 0.0,
            input_device_id: None,
            buffer_size: Self::default_buffer_size(),
            asio_out_pair: 0,
            aux_buses: Vec::new(),
        }
    }
}

#[cfg(test)]
mod machine_audio_config_tests {
    use super::MachineAudioConfig;

    #[test]
    fn old_machine_audio_config_migrates_with_preview_disabled() {
        let config: MachineAudioConfig = serde_json::from_str(r#"{
            "backend": "wasapi_shared",
            "device_id": "main",
            "device_name": "Main speakers"
        }"#)
        .expect("legacy machine config");

        assert_eq!(config.preview_device_id, None);
        assert_eq!(config.preview_device_name, None);
        assert_eq!(config.preview_asio_pair, None);
        assert_eq!(config.preview_gain_db, 0.0);
        assert_eq!(config.input_device_id, None);
        assert_eq!(config.buffer_size, 256);
    }
}

/// Machine-global audio defaults, mirrored into the workspace for compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioPreferences {
    /// Default volume (dB) applied to newly created cues.
    #[serde(default)]
    pub default_volume_db: f32,

    /// Duration (ms) of the soft fade-out applied on Stop.
    #[serde(default = "AudioPreferences::default_fade_out_ms")]
    pub default_fade_out_ms: u32,

    /// Default fade curve for newly created cues.
    #[serde(default = "AudioPreferences::default_fade_curve")]
    pub default_fade_curve: FadeCurve,

    /// Runtime-only: the machine's configured audio buffer size, injected at
    /// startup from `MachineAudioConfig` so `CueContext` can pass it to
    /// `ensure_input_feed`.  Never serialised into the workspace file.
    #[serde(skip)]
    pub audio_buffer_size: u32,
}

impl AudioPreferences {
    fn default_fade_out_ms() -> u32 {
        500
    }
    fn default_fade_curve() -> FadeCurve {
        FadeCurve::Linear
    }
}

impl Default for AudioPreferences {
    fn default() -> Self {
        Self {
            default_volume_db: 0.0,
            default_fade_out_ms: Self::default_fade_out_ms(),
            default_fade_curve: Self::default_fade_curve(),
            audio_buffer_size: 256,
        }
    }
}

// ---------------------------------------------------------------------------
// Reserved category structs (empty, ready for future content)
// ---------------------------------------------------------------------------

/// Row height for the Cue List table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CueRowHeight {
    Compact,
    #[default]
    Normal,
    Tall,
}

/// General app-behaviour preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralPreferences {
    /// Minimum time (ms) between two GO triggers.  A second GO fired within
    /// this window is silently ignored to prevent accidental double-presses
    /// during live shows.  Set to 0 to disable.
    #[serde(default = "GeneralPreferences::default_double_go_protection_ms")]
    pub double_go_protection_ms: u32,

    /// When true, deleting a cue via the keyboard shows a confirmation dialog.
    #[serde(default)]
    pub confirm_before_delete: bool,

    /// When true, the cue list automatically scrolls to keep the Playhead
    /// visible after each GO.
    #[serde(default = "GeneralPreferences::default_auto_scroll_to_playhead")]
    pub auto_scroll_to_playhead: bool,

    /// Height of each row in the cue list table.
    #[serde(default)]
    pub cue_row_height: CueRowHeight,

    /// When true, reordering/adding/removing cues rewrites every cue number to
    /// match its position (1, 2, 3…). This is the built-in default. When false,
    /// cue numbers remain stable through reordering and resequencing is an
    /// explicit action (Action → Renumber All Cues).
    #[serde(default = "GeneralPreferences::default_auto_renumber_on_reorder")]
    pub auto_renumber_on_reorder: bool,

    /// Colour assigned when a cue is first created, keyed by cue type. If this
    /// field is absent, loading uses the built-in palette. An explicitly saved
    /// map (including an empty one) is preserved without merging defaults.
    #[serde(default = "GeneralPreferences::default_cue_colors")]
    pub default_cue_colors: HashMap<CueType, CueColor>,
}

impl GeneralPreferences {
    fn default_double_go_protection_ms() -> u32 {
        500
    }
    fn default_auto_scroll_to_playhead() -> bool {
        true
    }

    fn default_auto_renumber_on_reorder() -> bool {
        true
    }

    fn default_cue_colors() -> HashMap<CueType, CueColor> {
        [
            (CueType::Fade, CueColor::Orange),
            (CueType::Group, CueColor::Yellow),
            (CueType::Memo, CueColor::Black),
            (CueType::Number, CueColor::Yellow),
            (CueType::Pause, CueColor::Red),
            (CueType::Resume, CueColor::Green),
            (CueType::Start, CueColor::Green),
            (CueType::Stop, CueColor::Red),
        ]
        .into_iter()
        .collect()
    }

    pub fn default_cue_color(&self, cue_type: &CueType) -> CueColor {
        self.default_cue_colors
            .get(cue_type)
            .copied()
            .unwrap_or(CueColor::None)
    }
}

impl Default for GeneralPreferences {
    fn default() -> Self {
        Self {
            double_go_protection_ms: Self::default_double_go_protection_ms(),
            confirm_before_delete: false,
            auto_scroll_to_playhead: Self::default_auto_scroll_to_playhead(),
            cue_row_height: CueRowHeight::default(),
            auto_renumber_on_reorder: Self::default_auto_renumber_on_reorder(),
            default_cue_colors: Self::default_cue_colors(),
        }
    }
}

/// OSC receive server configuration — stored machine-level in
/// `%APPDATA%\Inkue\osc.json`, not in the workspace file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OscReceiveConfig {
    /// When `false` the server is not started (or is stopped if already running).
    #[serde(default)]
    pub enabled: bool,
    /// UDP port to listen on.  Default: 53001.
    #[serde(default = "OscReceiveConfig::default_port")]
    pub port: u16,
    /// IP addresses allowed to send OSC commands.  Empty list = accept all.
    #[serde(default)]
    pub allowed_ips: Vec<String>,

    /// When `true`, Inkue broadcasts the running cue's name and number to
    /// `feedback_host:feedback_port` whenever the active cue changes.
    #[serde(default)]
    pub feedback_enabled: bool,
    /// Destination hostname or IP for OSC feedback (e.g. `"127.0.0.1"`).
    #[serde(default = "OscReceiveConfig::default_feedback_host")]
    pub feedback_host: String,
    /// Destination UDP port for OSC feedback.  Default: 53000.
    #[serde(default = "OscReceiveConfig::default_feedback_port")]
    pub feedback_port: u16,
    /// Send rate (Hz) for the per-slot media progress messages
    /// (`/inkue/cue/{i}/progress|elapsed|remaining|duration`).
    /// `0` disables progress feedback (cue number/name still sent on change).
    #[serde(default = "OscReceiveConfig::default_feedback_progress_hz")]
    pub feedback_progress_hz: u8,
}

impl OscReceiveConfig {
    fn default_port() -> u16 {
        53001
    }
    fn default_feedback_host() -> String {
        "127.0.0.1".into()
    }
    fn default_feedback_port() -> u16 {
        53000
    }
    fn default_feedback_progress_hz() -> u8 {
        10
    }
}

impl Default for OscReceiveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: Self::default_port(),
            allowed_ips: Vec::new(),
            feedback_enabled: false,
            feedback_host: Self::default_feedback_host(),
            feedback_port: Self::default_feedback_port(),
            feedback_progress_hz: Self::default_feedback_progress_hz(),
        }
    }
}

/// Network preferences (OSC, MIDI, Art-Net, …).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkPreferences {}

/// Machine-level network interface selection — stored in `network.json`,
/// not in the workspace file, because it describes the physical machine.
///
/// `None` everywhere = Automatic: bind to all interfaces and let the OS
/// routing table pick the egress interface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkInterfaceConfig {
    /// OS name of the selected interface (e.g. `"Ethernet"`, `"en0"`, `"eth0"`).
    #[serde(default)]
    pub interface_name: Option<String>,
    /// IPv4 address of the interface captured at selection time.  Used as the
    /// bind address, and as a fallback match if the interface was renamed.
    #[serde(default)]
    pub interface_ip: Option<String>,
}

/// How a cue's colour tag is rendered in the Cue List.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CueColorStyle {
    /// A 4px tinted strip along the left edge of the row.
    Stripe,
    /// The entire row background tinted with the cue's colour.
    #[default]
    FullRow,
}

/// Where on the output window the cue timer is anchored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimerPosition {
    /// Centered horizontally and vertically — large display.
    #[default]
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Display preferences (output surface, colour theme, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayPreferences {
    /// Whether the Live media strip is shown in the Clip Editor dock.
    #[serde(default = "default_true")]
    pub show_live_panel: bool,

    /// Whether the Slice editing strip is shown in the Clip Editor dock.
    #[serde(default = "default_true")]
    pub show_slice_panel: bool,

    /// Last selected Clip Editor tab. Kept as a string for forwards-compatible
    /// workspace migration; the frontend repairs invalid/hidden values.
    #[serde(default = "DisplayPreferences::default_clip_editor_active_tab")]
    pub clip_editor_active_tab: String,

    /// Monitor index for the unified output surface.
    /// `None` = floating windowed (no fixed screen).
    /// `Some(n)` = fullscreen on monitor n (0 = primary).
    #[serde(default)]
    pub output_screen: Option<u32>,

    /// When `true`, a countdown timer is drawn on the output window showing
    /// the timing of the currently running audio cue.
    #[serde(default)]
    pub show_output_timer: bool,

    /// When `true` the timer counts down (time remaining).
    /// When `false` (default) it counts up (elapsed position in the file).
    #[serde(default)]
    pub timer_count_down: bool,

    /// Font family name for the output timer (e.g. `"Arial"`, `"Courier New"`).
    #[serde(default = "DisplayPreferences::default_timer_font")]
    pub timer_font: String,

    /// Font size for the output timer, in mpv OSD points.
    /// Default 120 is suitable for center; use 60–80 for corner positions.
    #[serde(default = "DisplayPreferences::default_timer_font_size")]
    pub timer_font_size: u32,

    /// Where on the output window the timer is drawn.
    #[serde(default)]
    pub timer_position: TimerPosition,

    /// When `true`, milliseconds are shown after the seconds (e.g. `00:00.000`).
    #[serde(default)]
    pub timer_show_ms: bool,

    /// Margin in pixels from the screen edge for corner positions.
    /// Ignored when `timer_position` is `Center`.
    #[serde(default = "DisplayPreferences::default_timer_margin")]
    pub timer_margin: u32,

    /// When `true` (and `show_output_timer` is also `true`), the timer is shown
    /// in a small always-on-top floating Win32 window instead of as an OSD
    /// overlay on the output surface.
    #[serde(default)]
    pub timer_floating: bool,

    /// UI colour theme: `"dark"`, `"light"`, or `"system"` (follows OS setting).
    #[serde(default = "DisplayPreferences::default_theme")]
    pub theme: String,

    /// How a cue's colour tag is rendered in the Cue List (stripe vs full row).
    #[serde(default)]
    pub cue_color_style: CueColorStyle,

    /// Global projector-alignment transform (Preferences → Display), composed
    /// on top of every cue's own geometry by the output engine.
    #[serde(default)]
    pub output_transform: crate::engine::output_engine::OutputTransform,

    /// Named physical output destinations.  The first entry is created for
    /// legacy workspaces and deliberately uses the stable id `default`.
    #[serde(default = "DisplayPreferences::default_output_destinations")]
    pub output_destinations: Vec<OutputDestination>,
    /// Destination used when a cue does not select an output explicitly.
    #[serde(default = "DisplayPreferences::default_output_id")]
    pub default_output_id: String,
}

/// Output sink kind.  `Display` is intentionally an extensible enum: network
/// sinks such as NDI/SRT can be added without changing cue routing semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputSinkKind {
    Display,
    /// A named NDI program feed.  It has no native window or monitor.
    Ndi,
    /// A named SRT program feed.  It has no native window or monitor.
    Srt,
}

impl Default for OutputSinkKind {
    fn default() -> Self {
        Self::Display
    }
}

/// Last normal (non-maximized) geometry of a floating output window.
///
/// This belongs to the named destination, rather than the machine-wide app
/// configuration: a show can deliberately place a confidence window beside
/// its operator controls.  The native backend clamps it against the currently
/// connected displays before restoring it, so unplugging a display never
/// leaves an output inaccessible off-screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloatingWindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub maximized: bool,
}

impl Default for FloatingWindowGeometry {
    fn default() -> Self {
        Self {
            x: 100,
            y: 100,
            width: 1280,
            height: 720,
            maximized: false,
        }
    }
}

/// A named output destination and its placement/calibration settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputDestination {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub sink_kind: OutputSinkKind,
    /// Protocol-specific settings for a network sink.  This is retained on
    /// display destinations too so changing a destination's kind in a future
    /// version never discards an operator's carefully-entered network setup.
    #[serde(default)]
    pub network: crate::engine::network_io::NetworkOutputSettings,
    /// `None` is a floating output; `Some(index)` is fullscreen on that monitor.
    #[serde(default)]
    pub monitor: Option<u32>,
    /// Restored only while `monitor` is `None`.  `None` migrates old projects
    /// to the standard 1280×720 floating rect on their first use.
    #[serde(default)]
    pub floating_window: Option<FloatingWindowGeometry>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Keep this destination's native output window above ordinary windows.
    /// Defaults to `false` so older workspaces retain the platform's normal
    /// window stacking behaviour.
    #[serde(default)]
    pub always_on_top: bool,
    /// Hide the system pointer while it is over this destination's output
    /// window. Defaults to `false` for backwards-compatible deserialisation.
    #[serde(default)]
    pub hide_cursor: bool,
    #[serde(default)]
    pub transform: crate::engine::output_engine::OutputTransform,
    /// Fullscreen windows are input-locked; only explicit commands may alter it.
    #[serde(default = "default_true")]
    pub fullscreen_locked: bool,
}

fn default_true() -> bool {
    true
}

impl DisplayPreferences {
    fn default_clip_editor_active_tab() -> String {
        "Live".into()
    }

    fn default_theme() -> String {
        "system".into()
    }
    fn default_timer_font() -> String {
        crate::bundled_fonts::FONT_FAMILY.into()
    }
    fn default_timer_font_size() -> u32 {
        120
    }
    fn default_timer_margin() -> u32 {
        50
    }
    fn default_output_id() -> String {
        "default".into()
    }
    fn default_output_destinations() -> Vec<OutputDestination> {
        // Empty is intentional: migrate_outputs() must be able to distinguish
        // an absent legacy field and copy output_screen/output_transform.
        Vec::new()
    }

    /// Fill the new model from legacy fields without removing those fields.
    /// This is called on load, while save continues to serialize legacy keys.
    pub fn migrate_outputs(&mut self) {
        if self.output_destinations.is_empty() {
            self.output_destinations = vec![OutputDestination {
                id: "default".into(),
                name: "Main".into(),
                sink_kind: OutputSinkKind::Display,
                network: Default::default(),
                monitor: self.output_screen,
                enabled: true,
                floating_window: None,
                transform: self.output_transform,
                fullscreen_locked: true,
                always_on_top: false,
                hide_cursor: false,
            }];
        }
        if self.default_output_id.is_empty()
            || !self
                .output_destinations
                .iter()
                .any(|o| o.id == self.default_output_id)
        {
            self.default_output_id = self.output_destinations[0].id.clone();
        }
    }
}

impl Default for DisplayPreferences {
    fn default() -> Self {
        Self {
            show_live_panel: true,
            show_slice_panel: true,
            clip_editor_active_tab: Self::default_clip_editor_active_tab(),
            output_screen: None,
            show_output_timer: false,
            timer_count_down: false,
            timer_font: Self::default_timer_font(),
            timer_font_size: Self::default_timer_font_size(),
            timer_position: TimerPosition::default(),
            timer_show_ms: false,
            timer_margin: Self::default_timer_margin(),
            timer_floating: false,
            theme: Self::default_theme(),
            cue_color_style: CueColorStyle::default(),
            output_transform: crate::engine::output_engine::OutputTransform::default(),
            output_destinations: vec![OutputDestination {
                id: "default".into(),
                name: "Main".into(),
                sink_kind: OutputSinkKind::Display,
                network: Default::default(),
                monitor: None,
                enabled: true,
                floating_window: None,
                transform: crate::engine::output_engine::OutputTransform::default(),
                fullscreen_locked: true,
                always_on_top: false,
                hide_cursor: false,
            }],
            default_output_id: Self::default_output_id(),
        }
    }
}

// ---------------------------------------------------------------------------
// Root preferences struct
// ---------------------------------------------------------------------------

/// All application preferences. The authoritative copy is machine-global;
/// `Workspace.preferences` retains this serde shape as a compatibility mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppPreferences {
    #[serde(default)]
    pub audio: AudioPreferences,
    #[serde(default)]
    pub general: GeneralPreferences,
    #[serde(default)]
    pub network: NetworkPreferences,
    #[serde(default)]
    pub display: DisplayPreferences,
}

/// Inject the machine audio period into the runtime mirror. The field is
/// deliberately `serde(skip)`, so this can never leak into either persisted
/// Preferences or the legacy workspace snapshot.
pub fn inject_runtime_audio_buffer_size(preferences: &mut AppPreferences, buffer_size: u32) {
    preferences.audio.audio_buffer_size = buffer_size;
}

/// Repair the small amount of output structure that old or hand-edited
/// Preferences files may violate. Existing entries keep their order and
/// values; only unusable IDs, duplicate IDs, names, and an invalid default
/// are repaired. Returns whether the serialized tree changed.
pub fn normalize_global_preferences(preferences: &mut AppPreferences) -> bool {
    let before = serde_json::to_value(&*preferences).ok();
    preferences.display.migrate_outputs();

    let mut seen_ids = std::collections::HashSet::new();
    preferences.display.output_destinations.retain(|destination| {
        let id = destination.id.trim();
        if id.is_empty() || !seen_ids.insert(destination.id.clone()) {
            return false;
        }
        true
    });
    if preferences.display.output_destinations.is_empty() {
        preferences.display.output_destinations = DisplayPreferences::default().output_destinations;
    }
    for destination in &mut preferences.display.output_destinations {
        if destination.name.trim().is_empty() {
            destination.name = destination.id.clone();
        }
    }
    if !preferences
        .display
        .output_destinations
        .iter()
        .any(|destination| destination.id == preferences.display.default_output_id)
    {
        preferences.display.default_output_id = preferences.display.output_destinations[0].id.clone();
    }
    match (before, serde_json::to_value(&*preferences).ok()) {
        (Some(before), Some(after)) => before != after,
        // All current preference fields are serializable. Treat a future
        // serialization failure as changed so normalization is never silently
        // skipped.
        _ => true,
    }
}

/// Decide whether an automatic workspace migration may write the global file.
/// An invalid existing file is deliberately preserved until an explicit
/// Preferences Apply replaces it.
pub fn should_persist_global_migration(
    file_present: bool,
    migration_allowed: bool,
    changed: bool,
) -> bool {
    !file_present || (migration_allowed && changed)
}

/// Resolve the machine-wide Preferences when a workspace is opened.
///
/// With no global file, the first real workspace seeds it from its migrated
/// legacy snapshot.  Once the file exists, its values win; only destinations
/// whose IDs are absent globally are imported so cues from older projects do
/// not lose their output routing.  The global default ID is deliberately
/// never replaced by a project's default.
pub fn resolve_global_preferences(
    existing: Option<AppPreferences>,
    workspace: &AppPreferences,
) -> (AppPreferences, bool) {
    let mut legacy = workspace.clone();
    legacy.display.migrate_outputs();
    normalize_global_preferences(&mut legacy);

    let Some(mut global) = existing else {
        return (legacy, true);
    };

    let mut changed = normalize_global_preferences(&mut global);
    for destination in legacy.display.output_destinations {
        if !global
            .display
            .output_destinations
            .iter()
            .any(|known| known.id == destination.id)
        {
            global.display.output_destinations.push(destination);
            changed = true;
        }
    }
    changed |= normalize_global_preferences(&mut global);
    (global, changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(id: &str, name: &str) -> OutputDestination {
        let mut output = DisplayPreferences::default().output_destinations.remove(0);
        output.id = id.into();
        output.name = name.into();
        output
    }

    #[test]
    fn absent_global_file_seeds_from_the_first_real_workspace() {
        let mut workspace = AppPreferences::default();
        workspace.display.output_destinations = vec![destination("legacy", "Legacy")];
        workspace.display.default_output_id = "legacy".into();
        workspace.general.confirm_before_delete = true;

        let (global, should_save) = resolve_global_preferences(None, &workspace);
        assert!(should_save);
        assert_eq!(global.display.default_output_id, "legacy");
        assert!(global.general.confirm_before_delete);
        assert_eq!(global.display.output_destinations[0].id, "legacy");
    }

    #[test]
    fn runtime_audio_buffer_is_injected_without_changing_serialization() {
        let mut preferences = AppPreferences::default();
        inject_runtime_audio_buffer_size(&mut preferences, 1024);
        assert_eq!(preferences.audio.audio_buffer_size, 1024);
        let json = serde_json::to_value(&preferences).unwrap();
        assert!(json["audio"].get("audio_buffer_size").is_none());
    }

    #[test]
    fn legacy_preferences_json_remains_deserializable_for_global_migration() {
        let mut preferences: AppPreferences = serde_json::from_value(serde_json::json!({
            "audio": { "default_volume_db": -3.0 },
            "general": { "confirm_before_delete": true },
            "display": { "output_screen": 2 }
        }))
        .unwrap();
        preferences.display.migrate_outputs();
        let roundtrip: AppPreferences = serde_json::from_value(
            serde_json::to_value(&preferences).unwrap(),
        )
        .unwrap();
        assert_eq!(roundtrip.audio.default_volume_db, -3.0);
        assert!(roundtrip.general.confirm_before_delete);
        assert_eq!(roundtrip.display.output_destinations[0].monitor, Some(2));
    }

    #[test]
    fn existing_global_values_win_while_missing_output_ids_are_merged() {
        let mut global = AppPreferences::default();
        global.general.confirm_before_delete = false;
        global.display.default_output_id = "global".into();
        global.display.output_destinations = vec![destination("global", "Global")];

        let mut workspace = AppPreferences::default();
        workspace.general.confirm_before_delete = true;
        workspace.display.output_destinations = vec![destination("legacy", "Legacy")];
        workspace.display.default_output_id = "legacy".into();

        let (resolved, should_save) = resolve_global_preferences(Some(global), &workspace);
        assert!(should_save);
        assert!(!resolved.general.confirm_before_delete);
        assert_eq!(resolved.display.default_output_id, "global");
        assert_eq!(resolved.display.output_destinations.len(), 2);
        assert!(resolved.display.output_destinations.iter().any(|o| o.id == "legacy"));
    }

    #[test]
    fn output_merge_normalizes_duplicates_without_replacing_global_default() {
        let mut global = AppPreferences::default();
        global.display.default_output_id = "kept".into();
        global.display.output_destinations = vec![
            destination("kept", "Kept"),
            destination("kept", "Duplicate"),
            destination("", "Empty"),
        ];
        let workspace = AppPreferences::default();

        let (resolved, changed) = resolve_global_preferences(Some(global), &workspace);
        assert!(changed);
        assert_eq!(resolved.display.default_output_id, "kept");
        assert_eq!(resolved.display.output_destinations.iter().filter(|o| o.id == "kept").count(), 1);
    }

    #[test]
    fn invalid_global_file_never_allows_automatic_migration_write() {
        assert!(!should_persist_global_migration(true, false, true));
        assert!(should_persist_global_migration(false, true, true));
        assert!(should_persist_global_migration(true, true, true));
        assert!(!should_persist_global_migration(true, true, false));
    }

    #[test]
    fn legacy_general_preferences_receive_new_built_in_defaults() {
        let prefs: GeneralPreferences = serde_json::from_value(serde_json::json!({
            "double_go_protection_ms": 500,
            "confirm_before_delete": false,
            "auto_scroll_to_playhead": true,
            "cue_row_height": "normal"
        }))
        .unwrap();

        assert!(prefs.auto_renumber_on_reorder);
        assert_eq!(prefs.default_cue_color(&CueType::Fade), CueColor::Orange);
        assert_eq!(prefs.default_cue_color(&CueType::Group), CueColor::Yellow);
        assert_eq!(prefs.default_cue_color(&CueType::Number), CueColor::Yellow);
        assert_eq!(prefs.default_cue_color(&CueType::Memo), CueColor::Black);
        assert_eq!(prefs.default_cue_color(&CueType::Pause), CueColor::Red);
        assert_eq!(prefs.default_cue_color(&CueType::Resume), CueColor::Green);
        assert_eq!(prefs.default_cue_color(&CueType::Start), CueColor::Green);
        assert_eq!(prefs.default_cue_color(&CueType::Stop), CueColor::Red);
        assert_eq!(prefs.default_cue_color(&CueType::Text), CueColor::None);
        assert_eq!(prefs.default_cue_color(&CueType::Audio), CueColor::None);
    }

    #[test]
    fn explicit_user_defaults_override_or_clear_built_in_defaults() {
        let prefs: GeneralPreferences = serde_json::from_value(serde_json::json!({
            "auto_renumber_on_reorder": false,
            "default_cue_colors": { "fade": "purple" }
        }))
        .unwrap();

        assert!(!prefs.auto_renumber_on_reorder);
        assert_eq!(prefs.default_cue_color(&CueType::Fade), CueColor::Purple);
        assert_eq!(prefs.default_cue_color(&CueType::Group), CueColor::None);

        let cleared: GeneralPreferences = serde_json::from_value(serde_json::json!({
            "default_cue_colors": {}
        }))
        .unwrap();
        assert_eq!(cleared.default_cue_color(&CueType::Fade), CueColor::None);
    }

    #[test]
    fn default_cue_colors_roundtrip_by_type_without_affecting_other_types() {
        let mut prefs = GeneralPreferences::default();
        prefs.default_cue_colors.insert(CueType::Audio, CueColor::Purple);
        prefs.default_cue_colors.insert(CueType::Stop, CueColor::Red);

        let json = serde_json::to_value(&prefs).unwrap();
        assert_eq!(json["default_cue_colors"]["audio"], "purple");
        assert_eq!(json["default_cue_colors"]["stop"], "red");

        let restored: GeneralPreferences = serde_json::from_value(json).unwrap();
        assert_eq!(restored.default_cue_color(&CueType::Audio), CueColor::Purple);
        assert_eq!(restored.default_cue_color(&CueType::Stop), CueColor::Red);
        assert_eq!(restored.default_cue_color(&CueType::Video), CueColor::None);
    }

    #[test]
    fn the_platform_default_backend_is_one_this_os_has() {
        // Guards the per-OS `#[default]`: a Linux/macOS build defaulting to
        // WASAPI logged "backend=WasapiShared" on ALSA and left Preferences
        // showing a backend `get_available_backends` never offers.
        assert!(AudioBackend::default().is_available_here());
    }

    #[test]
    fn a_backend_from_another_os_is_coerced_on_load() {
        let foreign = if cfg!(target_os = "windows") {
            AudioBackend::SystemDefault
        } else {
            AudioBackend::WasapiShared
        };
        assert!(!foreign.is_available_here());
        assert_eq!(foreign.for_this_platform(), AudioBackend::default());
    }

    #[test]
    fn a_backend_this_os_supports_is_left_alone() {
        let native = AudioBackend::default();
        assert_eq!(native.for_this_platform(), native);
    }

    #[test]
    fn machine_config_json_roundtrips_through_the_platform_backend() {
        let json = serde_json::to_string(&AudioBackend::default()).unwrap();
        let back: AudioBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(back.for_this_platform(), AudioBackend::default());
    }

    #[test]
    fn legacy_display_migrates_to_stable_default_without_losing_transform() {
        let mut p: DisplayPreferences = serde_json::from_value(serde_json::json!({
            "output_screen": 2,
            "output_transform": { "scale": 1.25 }
        }))
        .unwrap();
        p.migrate_outputs();
        assert_eq!(p.default_output_id, "default");
        assert_eq!(p.output_destinations.len(), 1);
        assert_eq!(p.output_destinations[0].monitor, Some(2));
        assert_eq!(p.output_destinations[0].transform.scale, 1.25);
        let saved = serde_json::to_value(&p).unwrap();
        assert_eq!(saved["output_screen"], 2);
        assert!(saved["output_destinations"].is_array());
    }

    #[test]
    fn legacy_display_gets_clip_editor_visibility_defaults() {
        let p: DisplayPreferences = serde_json::from_value(serde_json::json!({
            "output_screen": null,
            "theme": "dark"
        }))
        .unwrap();
        assert!(p.show_live_panel);
        assert!(p.show_slice_panel);
        assert_eq!(p.clip_editor_active_tab, "Live");
        assert_eq!(p.cue_color_style, CueColorStyle::FullRow);
    }

    #[test]
    fn explicit_stripe_cue_color_style_survives_deserialization() {
        let prefs: DisplayPreferences = serde_json::from_value(serde_json::json!({
            "cue_color_style": "stripe"
        }))
        .unwrap();

        assert_eq!(prefs.cue_color_style, CueColorStyle::Stripe);
    }

    #[test]
    fn clip_editor_visibility_roundtrips_with_workspace_preferences() {
        let mut p = DisplayPreferences::default();
        p.show_live_panel = false;
        p.show_slice_panel = true;
        p.clip_editor_active_tab = "Slice".into();
        let back: DisplayPreferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(!back.show_live_panel);
        assert!(back.show_slice_panel);
        assert_eq!(back.clip_editor_active_tab, "Slice");
    }

    #[test]
    fn destination_ids_are_not_regenerated_on_roundtrip() {
        let p = DisplayPreferences::default();
        let json = serde_json::to_string(&p).unwrap();
        let back: DisplayPreferences = serde_json::from_str(&json).unwrap();
        assert_eq!(back.output_destinations[0].id, "default");
    }

    #[test]
    fn older_destinations_load_without_floating_window_geometry() {
        let p: DisplayPreferences = serde_json::from_value(serde_json::json!({
            "output_destinations": [{
                "id": "confidence", "name": "Confidence", "monitor": null,
                "enabled": true, "transform": {}
            }],
            "default_output_id": "confidence"
        }))
        .unwrap();
        assert_eq!(p.output_destinations[0].floating_window, None);
        assert!(!p.output_destinations[0].always_on_top);
        assert!(!p.output_destinations[0].hide_cursor);
    }

    #[test]
    fn output_window_preferences_roundtrip_per_destination() {
        let mut p = DisplayPreferences::default();
        p.output_destinations[0].always_on_top = true;
        p.output_destinations[0].hide_cursor = true;

        let saved = serde_json::to_value(&p).unwrap();
        assert_eq!(saved["output_destinations"][0]["always_on_top"], true);
        assert_eq!(saved["output_destinations"][0]["hide_cursor"], true);

        let restored: DisplayPreferences = serde_json::from_value(saved).unwrap();
        assert!(restored.output_destinations[0].always_on_top);
        assert!(restored.output_destinations[0].hide_cursor);
    }

    #[test]
    fn network_destination_roundtrips_and_old_destinations_get_network_defaults() {
        let p: DisplayPreferences = serde_json::from_value(serde_json::json!({
            "output_destinations": [
                { "id": "main", "name": "Main", "sink_kind": "display", "enabled": true, "monitor": null, "transform": {} },
                { "id": "ndi", "name": "Program NDI", "sink_kind": "ndi", "enabled": true, "monitor": null, "transform": {}, "network": { "ndi": { "enabled": true, "stream_name": "Program", "quality": "low_bandwidth" } } }
            ],
            "default_output_id": "main"
        }))
        .unwrap();
        assert_eq!(p.output_destinations[0].network.ndi.stream_name, "Qlisa Program");
        assert_eq!(p.output_destinations[1].sink_kind, OutputSinkKind::Ndi);
        assert_eq!(p.output_destinations[1].network.ndi.stream_name, "Program");
        assert_eq!(p.output_destinations[1].network.ndi.quality, crate::engine::network_io::NdiQuality::LowBandwidth);
    }
}
