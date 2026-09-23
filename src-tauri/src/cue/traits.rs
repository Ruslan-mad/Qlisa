//! The [`Cue`] trait — the universal contract for every cue type in Inkue.
//!
//! All cue types (Audio, Memo, Wait, Group, …) implement this trait so that the
//! Show Engine can drive them uniformly through `dyn Cue`.  The trait is
//! **object-safe**: every method either takes `&self`/`&mut self` or returns
//! a type with a known size.

use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::Value;

use super::{
    context::CueContext,
    types::{ContinueMode, CueColor, CueId, CueState, CueType, FadeAction, GroupMode, NumberStartStopMode},
};

/// Resolve a media cue's playable file interval.  File coordinates are used
/// by waveforms/filmstrips, while action coordinates begin at the trim start.
pub(crate) fn media_bounds_ms(
    file_duration: Option<Duration>,
    start: Option<Duration>,
    end: Option<Duration>,
) -> (u64, u64) {
    let file_end = file_duration.map(|d| d.as_millis() as u64).unwrap_or(u64::MAX);
    let start_ms = start.map(|d| d.as_millis() as u64).unwrap_or(0).min(file_end);
    let end_ms = end
        .map(|d| d.as_millis() as u64)
        .unwrap_or(file_end)
        .min(file_end)
        .max(start_ms);
    (start_ms, end_ms)
}

/// Convert action-relative elapsed time into a file-relative playhead.  The
/// conversion accounts for playback rate and wraps finite/infinite loops to
/// the current playable pass.  At the end of a finite play it reports the
/// trimmed end rather than wrapping back to the beginning.
pub(crate) fn media_position_from_action_ms(
    action_elapsed: Duration,
    file_duration: Option<Duration>,
    start: Option<Duration>,
    end: Option<Duration>,
    rate: f64,
    loop_count: u32,
) -> u64 {
    let (start_ms, end_ms) = media_bounds_ms(file_duration, start, end);
    let span = end_ms.saturating_sub(start_ms);
    if span == 0 {
        return start_ms;
    }
    let rate = if rate.is_finite() && rate > 0.0 { rate } else { 1.0 };
    let action_ms = action_elapsed.as_millis() as f64;
    let progressed = (action_ms * rate).round().max(0.0);
    let offset = if loop_count == 0 {
        progressed.min(span as f64) as u64
    } else if loop_count == u32::MAX {
        (progressed as u64) % span
    } else {
        let total = span.saturating_mul(loop_count as u64 + 1);
        if progressed >= total as f64 {
            span
        } else {
            (progressed as u64) % span
        }
    };
    start_ms.saturating_add(offset).min(end_ms)
}

/// Convert a file-relative position into an action-relative offset for a
/// file-coordinate seek.  A file click selects the first pass of a loop.
pub(crate) fn action_position_from_file_ms(
    file_position_ms: u64,
    file_duration: Option<Duration>,
    start: Option<Duration>,
    end: Option<Duration>,
    rate: f64,
) -> u64 {
    let (start_ms, end_ms) = media_bounds_ms(file_duration, start, end);
    let file_ms = file_position_ms.clamp(start_ms, end_ms);
    let rate = if rate.is_finite() && rate > 0.0 { rate } else { 1.0 };
    ((file_ms.saturating_sub(start_ms)) as f64 / rate).round() as u64
}

#[cfg(test)]
mod media_timing_tests {
    use super::{action_position_from_file_ms, media_position_from_action_ms};
    use std::time::Duration;

    #[test]
    fn trimmed_file_position_includes_start_offset() {
        assert_eq!(
            media_position_from_action_ms(
                Duration::from_millis(250),
                Some(Duration::from_secs(10)),
                Some(Duration::from_secs(2)),
                Some(Duration::from_secs(8)),
                1.0,
                0,
            ),
            2_250,
        );
    }

    #[test]
    fn rate_and_loop_playhead_wraps_inside_trim() {
        assert_eq!(
            media_position_from_action_ms(
                Duration::from_millis(750),
                Some(Duration::from_secs(10)),
                Some(Duration::from_secs(2)),
                Some(Duration::from_secs(4)),
                2.0,
                u32::MAX,
            ),
            3_500,
        );
    }

    #[test]
    fn looped_timeline_seek_wraps_to_the_current_source_pass() {
        assert_eq!(
            media_position_from_action_ms(
                Duration::from_secs(26),
                Some(Duration::from_secs(10)),
                None,
                None,
                1.0,
                u32::MAX,
            ),
            6_000,
        );
        assert_eq!(
            media_position_from_action_ms(
                Duration::from_secs(13),
                Some(Duration::from_secs(10)),
                Some(Duration::from_secs(2)),
                Some(Duration::from_secs(8)),
                1.0,
                u32::MAX,
            ),
            3_000,
        );
    }

    #[test]
    fn file_seek_clamps_and_converts_back_to_action_time() {
        assert_eq!(
            action_position_from_file_ms(
                99_000,
                Some(Duration::from_secs(10)),
                Some(Duration::from_secs(2)),
                Some(Duration::from_secs(8)),
                2.0,
            ),
            3_000,
        );
        assert_eq!(
            action_position_from_file_ms(
                500,
                Some(Duration::from_secs(10)),
                Some(Duration::from_secs(2)),
                Some(Duration::from_secs(8)),
                1.0,
            ),
            0,
        );
    }
}

// ---------------------------------------------------------------------------
// RuntimeState — volatile playback state that survives a cue rebuild
// ---------------------------------------------------------------------------

/// Snapshot of the volatile runtime state that must survive a cue rebuild
/// performed by `update_cue`.  Captured from the old instance and injected
/// into the freshly-rebuilt one so a running cue is not interrupted.
pub struct RuntimeState {
    pub state: CueState,
    /// Active audio voice ID (audio cues only).
    pub voice_id: Option<CueId>,
    /// Instant when `go()` was called (start of pre-wait).
    pub started_at: Option<Instant>,
    /// Instant when the action began (after pre-wait expired).
    pub action_started_at: Option<Instant>,
}

/// Per-execution token and Auto-Continue marker shared by synchronous cue
/// types. A new GO invalidates pending deadlines from the prior execution;
/// Stop/Reset clears the marker without reusing that execution token.
#[derive(Default)]
pub(crate) struct ExecutionMarker {
    generation: u64,
    auto_continue_fired: bool,
}

impl ExecutionMarker {
    pub(crate) fn begin(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.auto_continue_fired = false;
    }

    pub(crate) fn cancel(&mut self) {
        self.auto_continue_fired = false;
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn is_fired(&self) -> bool {
        self.auto_continue_fired
    }

    pub(crate) fn mark_fired(&mut self) {
        self.auto_continue_fired = true;
    }
}

/// Live audio parameters to re-apply to a cue's currently-playing voice after an
/// inspector edit, so volume/pan changes take effect without restarting
/// playback.  Returned by [`Cue::live_audio_params`].
#[derive(Debug, Clone)]
pub struct LiveAudioParams {
    /// The engine voice id to update.
    pub voice_id: CueId,
    /// Linear gain to apply (already converted from dB).
    pub gain: f32,
    /// Stereo pan (-1.0 .. 1.0).
    pub pan: f32,
    /// Crosspoint levels in dB, `[input][patch channel]`, when the cue has a
    /// level matrix.  `None` clears any matrix on the voice, putting it back
    /// on pan routing.  `update_cue` resolves the columns against the cue's
    /// Output Patch, exactly as the GO path does.
    pub level_matrix: Option<Vec<Vec<f64>>>,
}

/// Persisted audio settings that can safely be changed on an existing cue
/// object. `None` means leave a field unchanged; `Some(None)` clears the
/// optional crosspoint matrix.
#[derive(Debug, Clone, Default)]
pub struct LiveAudioPatch {
    pub volume_db: Option<f64>,
    pub pan: Option<f32>,
    pub level_matrix: Option<Option<Vec<Vec<f64>>>>,
}

/// Persisted visual settings which the output engine can apply to an existing
/// Video/Image/Camera voice without replacing its cue or worker.
#[derive(Debug, Clone, Default)]
pub struct LiveVisualPatch {
    pub geometry: Option<crate::engine::output_engine::VideoGeometry>,
    pub layer_style: Option<crate::engine::output_engine::LayerStyle>,
}

// ---------------------------------------------------------------------------
// CueFactory trait
// ---------------------------------------------------------------------------

/// Factory for a specific cue type.  Each cue type registers one factory in
/// the [`super::registry::CueRegistry`].
pub trait CueFactory: Send + Sync {
    /// Create a new, empty cue of this type with a fresh UUID.
    fn create(&self) -> Box<dyn Cue>;

    /// Deserialise a cue from its JSON representation.
    #[allow(clippy::wrong_self_convention)]
    fn from_json(&self, value: Value) -> Result<Box<dyn Cue>>;
}

// ---------------------------------------------------------------------------
// Cue trait
// ---------------------------------------------------------------------------

/// The universal cue contract.  Implementors must be `Send` so they can be
/// moved across thread boundaries (e.g., when loading a workspace on a worker
/// thread).
pub trait Cue: Send {
    // -----------------------------------------------------------------------
    // Identity
    // -----------------------------------------------------------------------

    /// Unique identifier for this cue instance.
    fn id(&self) -> CueId;

    /// The discriminant type of this cue (Audio, Memo, …).
    fn cue_type(&self) -> CueType;

    /// Human-readable name of the cue (editable by the operator).
    fn name(&self) -> &str;

    /// Update the cue's name.
    fn set_name(&mut self, name: String);

    /// Optional alphanumeric cue number (e.g. "1", "1.5", "Intro").
    /// This is a *string*, not a numeric index.
    fn number(&self) -> Option<&str>;

    /// Update the cue number.  Pass `None` to clear it.
    fn set_number(&mut self, number: Option<String>);

    /// Free-form notes visible in the inspector.
    fn notes(&self) -> &str;

    /// Update the notes field.
    fn set_notes(&mut self, notes: String);

    /// Colour label shown on the cue row in the Cue List.
    fn color(&self) -> CueColor;

    /// Update the colour label.
    fn set_color(&mut self, color: CueColor);

    /// Whether this cue is disabled.  Disabled cues are skipped by the
    /// transport — the Playhead advances past them automatically.
    fn is_disabled(&self) -> bool {
        false
    }

    /// Enable or disable this cue.
    fn set_disabled(&mut self, _disabled: bool) {}

    /// Optional timecode trigger: the SMPTE position at which this cue fires
    /// when the CueList's TC sync is enabled.  `None` = not TC-triggered.
    fn tc_trigger(&self) -> Option<&crate::engine::timecode_types::TcTrigger> {
        None
    }

    /// Set or clear the TC trigger for this cue.
    fn set_tc_trigger(&mut self, _trigger: Option<crate::engine::timecode_types::TcTrigger>) {}

    /// For media cues (Audio, Video, Image): the file path as stored in the
    /// cue (may be relative to the workspace directory).  Used by the command
    /// layer to detect broken cues without calling `serialize()`.
    fn media_file_path(&self) -> Option<&std::path::Path> {
        None
    }

    /// Read-only status for a managed network input owned by this cue.
    fn network_input_diagnostics(
        &self,
    ) -> Option<crate::engine::network_io::NetworkInputRuntimeDiagnostics> {
        None
    }

    /// Memo Cue only: the note it carries.  A Memo has no file, so the cue
    /// list shows this in the Target column instead — which is the whole point
    /// of the cue type, and what makes an imported `[Unconverted …]`
    /// placeholder readable in the stack.
    fn memo_text(&self) -> Option<&str> {
        None
    }

    // -----------------------------------------------------------------------
    // State
    // -----------------------------------------------------------------------

    /// Current lifecycle state.
    fn state(&self) -> CueState;

    /// Consume IDs of nested cues that this cue has actually fired since the
    /// previous drain. Non-Group cues do not fire other cues.
    fn take_fired_cue_ids(&mut self) -> Vec<CueId> {
        Vec::new()
    }

    /// `true` if the cue is currently in [`CueState::Running`].
    fn is_running(&self) -> bool {
        self.state() == CueState::Running
    }

    /// `true` if the cue is currently in [`CueState::Paused`].
    fn is_paused(&self) -> bool {
        self.state() == CueState::Paused
    }

    /// `true` while this cue owns a preloaded or in-flight runtime resource
    /// even though its lifecycle state may still read Standby.  Image and Video
    /// cues override this for Load Cue resources; undo/redo and destructive
    /// rebuilds must refuse while it is true.
    fn is_preloaded_or_loading(&self) -> bool {
        false
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Pre-load any resources needed for fast execution (e.g., decode audio
    /// into memory).  Called when the workspace is opened or when the
    /// operator manually loads a cue.
    fn load(&mut self, context: &CueContext) -> Result<()>;

    /// Trigger the cue at the Playhead.  Starts the pre-wait timer.
    fn go(&mut self, context: &CueContext) -> Result<()>;

    /// Trigger the cue with an optional parent fade envelope. Number uses
    /// this to apply one common fade without changing child settings.
    fn go_with_fade(&mut self, context: &CueContext, _shared_fade: Option<&crate::cue::types::FadeSpec>) -> Result<()> {
        self.go(context)
    }

    /// Stop the cue with a short fade-out (default 0.5 s).  Resets to Standby.
    fn stop(&mut self, context: &CueContext) -> Result<()>;

    /// Stop with an optional parent fade envelope. The default keeps the
    /// normal cue-specific stop behaviour.
    fn stop_with_fade(&mut self, context: &CueContext, _shared_fade: Option<&crate::cue::types::FadeSpec>) -> Result<()> {
        self.stop(context)
    }

    /// Suspend execution mid-action.
    fn pause(&mut self, context: &CueContext) -> Result<()>;

    /// Resume a paused cue.
    fn resume(&mut self, context: &CueContext) -> Result<()>;

    /// Immediately cut playback without any fade.  Used on double-Escape.
    fn hard_stop(&mut self, context: &CueContext) -> Result<()>;

    /// Reset the cue to its initial Standby state (clears elapsed time etc.).
    fn reset(&mut self) -> Result<()>;

    /// Called by the event loop at ~30 fps for every Running cue.
    ///
    /// The default implementation is a no-op.  Audio cues override this to
    /// handle the pre-wait phase: once `pre_wait` has elapsed the audio
    /// action starts without `go()` having to block on a timer.
    fn tick(&mut self, _context: &CueContext) -> Result<()> {
        Ok(())
    }

    /// Volatile operator-facing runtime failure. Unlike workspace validation,
    /// this is set by a running cue and is cleared when that cue is run again.
    fn runtime_error(&self) -> Option<&str> {
        None
    }

    /// Volatile non-fatal runtime diagnostic, such as a missing optional
    /// stream. It does not change the cue lifecycle state.
    fn runtime_warning(&self) -> Option<&str> {
        None
    }

    /// Returns `true` once a runtime diagnostic changed and the UI should
    /// refresh its cue summary. Implementations consume the flag here so a
    /// steady state does not cause cue-list refreshes every tick.
    fn take_runtime_diagnostic_changed(&mut self) -> bool {
        false
    }

    /// Returns `false` while the cue is in its Pre-Wait phase (i.e. `go()`
    /// has been called but the action has not yet started).
    ///
    /// The event loop uses this to avoid firing Auto-Continue before the
    /// action — and therefore Post-Wait — has actually begun.
    /// Default: `true` (most cue types start their action synchronously).
    fn is_action_started(&self) -> bool {
        true
    }

    // -----------------------------------------------------------------------
    // Timing
    // -----------------------------------------------------------------------

    /// Delay inserted before the cue's action begins.
    fn pre_wait(&self) -> Duration;

    /// Update the Pre-Wait duration.
    fn set_pre_wait(&mut self, d: Duration);

    /// Delay after the action *starts* (not ends) before continue mode fires.
    fn post_wait(&self) -> Duration;

    /// Update the Post-Wait duration.
    fn set_post_wait(&mut self, d: Duration);

    /// Total duration of the cue's action, if known in advance.
    /// Returns `None` for open-ended cues (e.g., looping audio).
    fn duration(&self) -> Option<Duration>;

    /// Update a user-configured action duration without replacing the cue
    /// object.  Only cue types whose duration is an authored property (Wait,
    /// Fade, Image, Text and Light) override this; media and runtime-derived
    /// durations deliberately reject the request.
    ///
    /// `None` means an indefinite duration and is supported only by Image and
    /// Text cues.  Callers validate the cue type and lifecycle before using
    /// this setter so a rejected request never creates an Undo entry.
    fn set_user_action_duration(&mut self, _duration: Option<Duration>) -> Result<()> {
        anyhow::bail!("Duration cannot be edited for {} cue", self.cue_type())
    }

    /// Total time elapsed since `go()` was called (including pre-wait).
    fn elapsed(&self) -> Duration;

    /// Time elapsed since the action started (i.e., after pre-wait).
    fn action_elapsed(&self) -> Duration;

    /// Monotonic time since the action started for Auto-Continue scheduling.
    /// Seekable cues override this so scrubbing changes the media playhead and
    /// progress display without moving the transport's Post-Wait deadline.
    fn auto_continue_elapsed(&self) -> Duration {
        self.action_elapsed()
    }

    // -----------------------------------------------------------------------
    // Continue mode
    // -----------------------------------------------------------------------

    /// What happens after this cue's Post-Wait expires.
    fn continue_mode(&self) -> ContinueMode;

    /// Update the continue mode.
    fn set_continue_mode(&mut self, mode: ContinueMode);

    // -----------------------------------------------------------------------
    // Runtime helpers
    // -----------------------------------------------------------------------

    /// Seek to `position_ms` from the start of the cue's action.
    ///
    /// For audio cues this repositions the audio voice.  For video cues it
    /// issues an mpv seek and re-anchors the paired audio voice.  Non-seekable
    /// cue types (Memo, Stop, …) use the default no-op.
    ///
    /// The caller is responsible for updating any transport-level timing only
    /// when the cue is actually running or paused; calling seek on a standby
    /// cue has no effect.
    fn seek(&mut self, _position_ms: u64, _ctx: &CueContext) {}

    /// One-shot action-time seek consumed by the next video start. Number
    /// uses it so a freshly opened video is positioned before its first frame
    /// is revealed instead of starting at frame zero and seeking afterward.
    fn set_initial_seek_action_ms(&mut self, _position_ms: u64) {}

    /// Seek a composite cue on its own timeline. Leaf media use the regular
    /// action-relative seek; Group overrides this to select the correct
    /// child for sequential/playlist timelines.
    fn seek_timeline_position(&mut self, position_ms: u64, ctx: &CueContext) {
        self.seek(position_ms, ctx);
    }

    /// Seek a media cue using a file-relative position (milliseconds).  The
    /// legacy [`seek`](Self::seek) contract remains action-relative for the
    /// Inspector scrub bar; this separate entry point is used by timeline
    /// views whose coordinates are the source file/waveform coordinates.
    fn seek_file_position(&mut self, _file_position_ms: u64, _ctx: &CueContext) {}

    /// Return the source-file playhead position for an active media leaf.
    /// Implementations prefer the engine's position (which includes loop and
    /// slice jumps) and fall back to the cue clock when the engine has not
    /// exposed a position yet.  Non-media cues return `None` and therefore do
    /// not cause engine queries in the 30 Hz event loop.
    fn media_position_ms(
        &self,
        _audio_engine: &crate::engine::AudioEngine,
        _output_engine: &crate::engine::OutputEngine,
    ) -> Option<u64> {
        None
    }

    /// Compute the file-relative position from the cue clock.  This is also
    /// used for the immediate timing event emitted after a seek, before an
    /// asynchronous engine seek command has reached its playback thread.
    fn media_position_for_action_ms(&self, _action_elapsed: Duration) -> Option<u64> {
        None
    }

    /// Inject pre-decoded audio samples that were decoded *outside* the
    /// workspace mutex.  The caller decodes on a background thread, then
    /// briefly re-acquires the mutex to call this method.  Non-audio cues
    /// ignore the call (default no-op).
    fn accept_preloaded_audio(
        &mut self,
        _samples: std::sync::Arc<Vec<f32>>,
        _channels: u16,
        _sample_rate: u32,
        _duration: std::time::Duration,
    ) {
    }

    /// Inject probed media metadata. PCM stays on the bounded streaming ring
    /// and is opened by the voice only when GO starts the cue.
    fn accept_preloaded_stream(
        &mut self,
        _path: std::path::PathBuf,
        _channels: u16,
        _sample_rate: u32,
        _duration: Option<Duration>,
    ) {
    }

    /// Returns the active audio voice ID if this cue is currently playing
    /// through the audio engine.  Non-audio cues return `None` (default).
    /// Used by the event loop to correlate [`crate::engine::ring_command::AudioStatus::Completed`]
    /// events back to the owning cue.
    fn playing_voice_id(&self) -> Option<CueId> {
        None
    }

    /// Every audio voice this cue currently controls, **recursively**.
    ///
    /// For a leaf cue this is just its own [`playing_voice_id`](Self::playing_voice_id)
    /// (the default). A [`GroupCue`](crate::cue::group_cue::GroupCue) overrides this
    /// to flatten the voices of all its children (at any depth), so a single Group
    /// can be a first-class target for volume/pan Fades and Stops — the transport
    /// asks "what voices does this target own?" instead of assuming one voice on a
    /// top-level cue.
    fn all_voice_ids(&self) -> Vec<CueId> {
        self.playing_voice_id().into_iter().collect()
    }

    /// Monotonically increasing counter incremented on every `go()` call.
    /// Reserved for diagnostics / future use.  Default: 0.
    fn play_generation(&self) -> u64 {
        0
    }

    /// Returns `true` if Auto-Continue has already been fired for the
    /// **current** play of this cue.  The transport sets this flag
    /// synchronously inside `go()` before chaining, so the event loop
    /// never sees the cue as needing a second chain.
    fn is_auto_continue_fired(&self) -> bool {
        false
    }

    /// Expose the per-execution marker when the cue type stores one. `None`
    /// means the type has no marker; otherwise `false` invalidates a delayed
    /// Auto-Follow after Stop/Reset/retrigger.
    fn auto_continue_marker(&self) -> Option<bool> {
        None
    }

    /// Mark Auto-Continue as fired for the current play.
    /// Called by [`Transport::go`] immediately after chaining.
    fn mark_auto_continue_fired(&mut self) {}

    /// Reset the Auto-Continue fired flag.  Called by `go()` (new play) and
    /// `reset()` / `stop()` (cue stopped or completed).
    fn clear_auto_continue_fired(&mut self) {}

    /// Full duration of the underlying source file, **without** start/end
    /// markers applied.  Audio cues override this; other types return `duration()`.
    fn file_duration(&self) -> Option<Duration> {
        self.duration()
    }

    /// Return a cheap clone of the pre-decoded audio data already in memory,
    /// or `None` if the cue has not been decoded yet.
    ///
    /// Used by [`update_cue`](crate::commands::cue_cmds::update_cue) to
    /// preserve decoded samples across cue rebuilds (name/colour/timing
    /// changes must not force a re-decode).  Non-audio cues return `None`.
    fn extract_decoded_audio(
        &self,
    ) -> Option<(std::sync::Arc<Vec<f32>>, u16, u32, Duration)> {
        None
    }

    /// Downsample the decoded audio into `bins` peak values (0.0 – 1.0) for
    /// waveform display.  Returns `None` if no audio data is loaded yet.
    /// Non-audio cues always return `None` (default).
    fn waveform_peaks(&self, _bins: usize) -> Option<Vec<f32>> {
        None
    }

    /// Called when the underlying media's total duration becomes known at
    /// runtime (e.g., after a video file's metadata loads in the surface
    /// window).  Non-video cues can ignore this call (default no-op).
    fn set_runtime_duration(&mut self, _duration: std::time::Duration) {}

    /// The Output Patch this cue routes audio through, when it has one
    /// assigned.  `None` = no patch (audio-producing cues then use the
    /// workspace default patch).  Used by the cue list's Output column.
    fn output_patch_id(&self) -> Option<uuid::Uuid> {
        None
    }

    /// If `true`, [`Transport::go`] will automatically stop this cue when the
    /// next GO fires.  Default: `false`. Text and exclusive Browser cues opt
    /// in; composited visual cues (Video/Image/Camera) remain independent.
    fn stop_on_next_go(&self) -> bool {
        false
    }

    /// `true` for cue types that occupy the visual output surface (Video,
    /// Image, Camera, Browser). Used by the transport's stop-on-next-GO filter and by
    /// Fade target resolution.  Override instead of adding the type to a
    /// match in `show/transport.rs`.
    fn is_visual(&self) -> bool {
        false
    }

    /// Fade Cue only: returns the fade parameters so the transport can resolve
    /// target voices and call [`set_fade_voices`] before the first tick.
    fn fade_specification(&self) -> Option<FadeAction> {
        None
    }

    /// Inject resolved audio voice IDs (and their current gains) into this cue,
    /// plus per-layer visual fade targets for any Video/Image/Camera targets.
    ///
    /// - `voices`: `(audio_voice_id, start_gain, start_pan)` for each audio target.
    /// - `visual_targets`: `(output_voice_id, start_opacity)` for each visual
    ///   target's layer.
    /// - `visual_target_opacity`: layer opacity when the fade completes
    ///   (0.0 = black, 1.0 = fully visible).
    ///
    /// Called by [`crate::show::transport::Transport::go`] after `go()` so that
    /// `tick()` knows which voices/layers to update.
    fn set_fade_voices(
        &mut self,
        _voices: Vec<(CueId, f32, f32)>,
        _visual_targets: Vec<(CueId, f32)>,
        _visual_target_opacity: f32,
    ) {}

    /// Stop Cue only: describes what to stop after `go()` completes.
    ///
    /// Returns `Some((hard_stop, target_cue_ids))` where:
    /// - `hard_stop` — `true` = immediate cut, `false` = soft fade.
    /// - `target_cue_ids` — empty = stop all, non-empty = stop those UUIDs only.
    ///
    /// Transport reads this and executes the stop **before** evaluating
    /// Auto-Follow chains, preventing the chained cue from being killed.
    fn stop_specification(&self) -> Option<(bool, Vec<CueId>)> {
        None
    }

    /// `true` while this cue plays with an active slice program — the UI's
    /// time display then follows the engine-reported **media position**
    /// instead of wall-clock elapsed (a vamp holds position while the clock
    /// runs on).  Default: `false`.
    fn uses_sliced_playback(&self) -> bool {
        false
    }

    /// Devamp Cue only: describes which cues to devamp after `go()`.
    ///
    /// Returns `Some((stop_at_end, target_cue_ids))`:
    /// - `stop_at_end` — `true` = the target stops at the end of its current
    ///   slice, `false` = it continues into the next slice.
    ///
    /// Transport resolves each target's voices (audio + visual + a video's
    /// paired audio voice) and forwards the devamp to the engines.
    fn devamp_specification(&self) -> Option<(bool, Vec<CueId>)> {
        None
    }

    /// Prepare this cue so a later Start begins instantly (Load Cue):
    /// decoders warm and buffers filled, but **nothing audible or visible**.
    ///
    /// The default brings the cue up and pauses it, which is right for an
    /// Audio Cue (a paused voice is silent) and harmless for instant cues.
    /// Visual cues override it: going and pausing would put their first frame
    /// on the output, which is the opposite of what loading means.
    fn preload(&mut self, context: &CueContext) -> Result<()> {
        self.go(context)?;
        self.pause(context)
    }

    /// Command cues only (Start, Pause, Resume, Load, Reset, Goto, Arm,
    /// Disarm): describes what to do to which cues after `go()`.
    ///
    /// Returns `Some((action, target_cue_ids))`. Unlike a Stop Cue, an empty
    /// target list yields `None` rather than "every cue" — resetting or
    /// starting a whole show by omission is not a useful default.
    ///
    /// Transport executes this alongside the stop specification, before
    /// Auto-Follow is evaluated, and resolves targets recursively so a cue
    /// nested in a Group can be addressed.
    fn control_specification(
        &self,
    ) -> Option<(crate::cue::control_cue::ControlAction, Vec<CueId>)> {
        None
    }

    /// Fade Cue only: drain the cue ids whose cues should be hard-stopped now
    /// that a `stop_at_end` fade has finished.  The event loop calls this every
    /// tick and stops the returned cues (the Fade cannot reach the cue list from
    /// `tick`).  Default: nothing to stop.
    fn take_fade_stop_targets(&mut self) -> Vec<CueId> {
        Vec::new()
    }

    /// Resolve stop/fade targets from cue-number strings to UUIDs.
    ///
    /// Called once per cue after the whole cue list is loaded, allowing cues
    /// saved in the old format (number only, no UUID) to be upgraded
    /// in-memory for the current session.  Default implementation is a no-op.
    fn resolve_stop_target(&mut self, _number_to_id: &std::collections::HashMap<String, CueId>) {}

    /// Fade Cue only: resolve target UUIDs from cue-number labels.
    fn resolve_fade_targets(&mut self, _number_to_id: &std::collections::HashMap<String, CueId>) {}

    /// Assign the explicit target metadata used by a context-created command
    /// cue. Stop, Fade, Devamp and Control cues override this; regular cue
    /// types return `false` so callers cannot silently create an unbound
    /// command. The method is infallible for supported types, allowing a new
    /// cue to be synchronised after structural auto-renumbering without a
    /// second rebuild or undo snapshot.
    fn set_context_target(&mut self, _target_id: CueId, _target_number: Option<String>) -> bool {
        false
    }

    /// Capture the volatile runtime state so it can be transplanted into a
    /// freshly-rebuilt instance.  Called by `update_cue` just before the
    /// old cue is replaced.  Default returns a Standby snapshot with no voice
    /// or timing — cue types that carry runtime state must override this.
    fn runtime_state(&self) -> RuntimeState {
        RuntimeState {
            state: self.state(),
            voice_id: self.playing_voice_id(),
            started_at: None,
            action_started_at: None,
        }
    }

    /// Inject a previously captured [`RuntimeState`] into this instance.
    /// Called by `update_cue` after rebuilding so a running cue continues
    /// uninterrupted.  Default is a no-op.
    fn restore_runtime_state(&mut self, _snap: RuntimeState) {}

    /// Live audio parameters to push to the cue's currently-playing voice after
    /// an inspector edit (volume / pan), so changes apply without restarting.
    /// Returns `None` when the cue has no live voice.  Default is `None`.
    fn live_audio_params(&self) -> Option<LiveAudioParams> {
        None
    }

    /// Persist live-safe audio settings in place. The command layer calls this
    /// only for cue types and fields it has already validated.
    fn apply_live_audio_patch(&mut self, _patch: LiveAudioPatch) {}

    /// Per-cue visual geometry (Video / Image cues only).  Used by `update_cue`
    /// to live-apply Geometry-tab edits to the content currently on the output
    /// window.  Default is `None` (no visual output).
    fn visual_geometry(&self) -> Option<crate::engine::output_engine::VideoGeometry> {
        None
    }

    /// Compositing properties (layer / opacity / blend mode) for visual cues.
    /// Used by `update_cue` to live-apply Compositing edits to the cue's slot.
    /// Default is `None` (no visual output).
    fn layer_style(&self) -> Option<crate::engine::output_engine::LayerStyle> {
        None
    }

    /// Persist live-safe visual settings in place. The command layer applies
    /// the resulting geometry/layer state to an existing output voice.
    fn apply_live_visual_patch(&mut self, _patch: LiveVisualPatch) {}

    // -----------------------------------------------------------------------
    // Group support
    // -----------------------------------------------------------------------

    /// Returns `true` once the cue has naturally finished all of its work and
    /// is ready to be reset.  The default (`false`) means the event loop uses
    /// voice-completion and time-based detection instead.
    ///
    /// [`GroupCue`](crate::cue::group_cue::GroupCue) overrides this: it
    /// becomes `true` when every child has completed.
    fn is_complete(&self) -> bool {
        false
    }

    /// Read-only view of direct child cues.  Returns `None` for non-Group cues.
    fn child_cues(&self) -> Option<&[Box<dyn Cue>]> {
        None
    }

    /// Mutable view of direct child cues.  Returns `None` for non-Group cues.
    fn child_cues_mut(&mut self) -> Option<&mut Vec<Box<dyn Cue>>> {
        None
    }

    /// Consume and return all children (for `ungroup`).  Returns `None` for
    /// non-Group cues.
    fn take_children(&mut self) -> Option<Vec<Box<dyn Cue>>> {
        None
    }

    /// Add a child cue at `position` (−1 = append).  Returns `Err` for
    /// non-Group cues.
    fn add_child(&mut self, _child: Box<dyn Cue>, _position: i32) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("Not a Group cue"))
    }

    /// Remove and return the child with the given ID.  Returns `Err` for
    /// non-Group cues or if the child is not found.
    fn remove_child(&mut self, _id: &CueId) -> anyhow::Result<Box<dyn Cue>> {
        Err(anyhow::anyhow!("Not a Group cue"))
    }

    /// The mode of this Group cue (`None` for non-Group cues).
    fn group_mode(&self) -> Option<GroupMode> {
        None
    }

    /// Update the Group mode.  No-op for non-Group cues.
    fn set_group_mode(&mut self, _mode: GroupMode) {}

    /// Whether this Playlist Group loops (wraps last child → first).  `None` for
    /// non-Group cues and irrelevant for other modes.
    fn playlist_loop(&self) -> Option<bool> {
        None
    }

    /// Enable/disable Playlist looping.  No-op for non-Group cues.
    fn set_playlist_loop(&mut self, _on: bool) {}

    /// Number-only flow settings. Ordinary cues return `None`/empty values.
    fn number_start_stop_specification(&self) -> Option<(NumberStartStopMode, Vec<CueId>)> {
        None
    }
    fn number_finish_start_ids(&self) -> Vec<CueId> { Vec::new() }
    fn take_number_completion_start_ids(&mut self) -> Vec<CueId> { Vec::new() }

    /// Runtime-only mode used when a Group is the master inside a Number.
    /// Ordinary Groups keep their authored continuation behaviour.
    fn set_number_master_autoplay(&mut self, _enabled: bool) {}

    /// Number-only metadata. A Number reuses the Group child container but
    /// keeps its master and action offsets hidden from ordinary Group UI.
    fn number_master_id(&self) -> Option<CueId> { None }
    fn set_number_master(&mut self, _id: CueId) -> anyhow::Result<()> {
        anyhow::bail!("Not a Number cue")
    }
    fn number_action_offsets(&self) -> Vec<(CueId, u64)> { Vec::new() }
    fn set_number_action_offset(&mut self, _id: CueId, _offset_ms: u64) -> anyhow::Result<()> {
        anyhow::bail!("Not a Number cue")
    }

    /// Returns `true` if this cue wants to consume the next outer GO press
    /// without the transport advancing the Playhead.
    ///
    /// Only [`GroupCue`](crate::cue::group_cue::GroupCue) in Sequential mode
    /// overrides this: when the internal sequence has paused at a
    /// `DoNotContinue` child and more children remain, it absorbs GO to fire
    /// the next child internally instead of advancing the outer Playhead.
    fn absorbs_go(&self) -> bool {
        false
    }

    /// Returns `true` if this cue retains the outer Playhead on itself while
    /// it is running, so that subsequent GO presses are routed into its own
    /// internal sequence rather than advancing the outer Playhead.
    ///
    /// Only Sequential [`GroupCue`] overrides this.  The event loop is
    /// responsible for advancing the outer Playhead once the cue completes.
    fn holds_playhead(&self) -> bool {
        false
    }

    /// Returns `true` once a cue that held the outer Playhead has fired
    /// everything it will fire and the Playhead should now move on to the next
    /// outer cue — even though this cue may still be running (e.g. overlapping
    /// audio children still playing out).
    ///
    /// Only Sequential [`GroupCue`] overrides this: it becomes `true` the moment
    /// its **last** child is fired, so the next GO continues the outer list
    /// instead of being absorbed.  The transport (on GO) and the event loop (on
    /// auto-advance) both consult it to release the Playhead.
    fn released_playhead(&self) -> bool {
        false
    }

    /// For a running Sequential [`GroupCue`]: the ID of the child that is
    /// currently active — either running right now, or the next one to fire on
    /// GO (when the sequence is paused at a `DoNotContinue` child).
    ///
    /// Returns `None` for non-Group cues and for Simultaneous groups (the
    /// frontend derives activity from each child's own `state()` instead).
    fn active_child_id(&self) -> Option<CueId> {
        None
    }

    /// Point a Sequential [`GroupCue`]'s internal playhead at `child_id` so the
    /// next GO fires that child (and the sequence continues from there).
    ///
    /// Returns `true` if `child_id` is a direct child of a Sequential group.
    /// Default: `false` (non-Group cues and Simultaneous groups ignore it).
    fn set_active_child(&mut self, child_id: &CueId) -> bool {
        let _ = child_id;
        false
    }

    // -----------------------------------------------------------------------
    // Preflight validation
    // -----------------------------------------------------------------------

    /// Report problems with this cue's external dependencies (dangling
    /// Stop/Fade target, unpatched fixture, absent MIDI port, …) for the
    /// "Check Workspace" preflight.  The default returns no issues; cue types
    /// with external dependencies override this.
    ///
    /// Media-file existence is **not** checked here — the command layer covers
    /// that centrally via [`media_file_path`](Self::media_file_path) so it can
    /// also drive relink.
    fn validate(&self, _ctx: &super::validation::ValidationContext) -> Vec<super::validation::CueIssue> {
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Serialisation
    // -----------------------------------------------------------------------

    /// Serialise this cue to a JSON [`Value`] for `.inkue` file persistence.
    /// The returned object must include a `"type"` field matching
    /// [`CueType`]'s serialised form.
    fn serialize(&self) -> Value;
}
