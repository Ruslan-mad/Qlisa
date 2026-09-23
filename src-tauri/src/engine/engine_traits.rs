//! Engine interface traits held by [`CueContext`](crate::cue::context::CueContext).
//!
//! The cue and transport layers drive playback through these traits rather than
//! the concrete engines. In production the real [`AudioEngine`], [`OutputEngine`]
//! and [`DmxEngine`] implement them (thin forwarding to the existing inherent
//! methods, so behaviour is unchanged). In tests, lightweight doubles implement
//! them, which is what lets `CueContext` — and therefore `Transport::go` — be
//! constructed and exercised without an audio device, a GL window, or libmpv.
//!
//! The trait surface is exactly the set of methods reached via
//! `context.{audio,output,dmx}_engine.*` from the cue and transport layers; the
//! event loop keeps its own concrete engine handles and is unaffected.

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use uuid::Uuid;

use super::audio_engine::{AudioEngine, SyntheticFeedProducer};
use super::dmx_engine::{ChannelWidth, DmxEngine};
use super::output_engine::{ContentRequest, OutputEngine};
use super::ring_command::{FadeCurve, VoiceId};
use super::voice::Voice;

// ---------------------------------------------------------------------------
// Audio engine
// ---------------------------------------------------------------------------

/// The audio-engine operations the cue and transport layers depend on.
pub trait AudioEngineApi: Send + Sync {
    fn play_voice_routed(&self, voice: Voice, device_id: Option<&str>) -> Result<VoiceId>;
    fn play_voice_paused_routed(&self, voice: Voice, device_id: Option<&str>) -> Result<VoiceId>;
    fn stop_voice(&self, voice_id: VoiceId, fade_ms: u32, fade_curve: FadeCurve) -> Result<()>;
    fn pause_voice(&self, voice_id: VoiceId) -> Result<()>;
    fn resume_voice(&self, voice_id: VoiceId) -> Result<()>;
    fn seek_voice(&self, voice_id: VoiceId, frame_pos: u64) -> Result<()>;
    /// Release the voice's current slice loop (Devamp Cue).
    fn devamp_voice(&self, voice_id: VoiceId, stop_at_end: bool) -> Result<()>;
    fn set_voice_gain(&self, voice_id: VoiceId, gain: f32) -> Result<()>;
    fn get_voice_gain(&self, voice_id: VoiceId) -> f32;
    fn set_voice_pan(&self, voice_id: VoiceId, pan: f32) -> Result<()>;
    /// Live crosspoint routing; `None` returns the voice to pan routing.
    fn set_voice_level_matrix(
        &self,
        voice_id: VoiceId,
        matrix: Option<&super::voice::LevelMatrix>,
    ) -> Result<()>;
    fn get_voice_pan(&self, voice_id: VoiceId) -> f32;
    fn sample_rate(&self) -> u32;
    /// Whether this voice is still in an active playback state.
    fn voice_is_alive(&self, voice_id: VoiceId) -> bool { let _ = voice_id; false }
    fn ensure_input_feed(&self, device_id: Option<&str>, buffer_size: u32) -> Result<Uuid>;
    fn register_synthetic_feed(
        &self,
        channels: usize,
        sample_rate: u32,
    ) -> Result<(Uuid, SyntheticFeedProducer)>;
    fn register_network_feed(
        &self,
        channels: usize,
        sample_rate: u32,
    ) -> Result<(Uuid, SyntheticFeedProducer)> {
        self.register_synthetic_feed(channels, sample_rate)
    }
    #[allow(clippy::too_many_arguments)]
    fn play_mic_voice(
        &self,
        feed_id: Uuid,
        in_l: usize,
        in_r: usize,
        out_l: usize,
        out_r: usize,
        gain: f32,
        pan: f32,
        fade_in_ms: u32,
        fade_curve: FadeCurve,
    ) -> Result<VoiceId>;
    /// Start a live input voice with Output Patch metadata. The default keeps
    /// compatibility with lightweight test engines; the real engine routes the
    /// voice to the selected device and applies the patch gain/meter slot.
    #[allow(clippy::too_many_arguments)]
    fn play_mic_voice_routed(
        &self,
        feed_id: Uuid,
        in_l: usize,
        in_r: usize,
        out_l: usize,
        out_r: usize,
        gain: f32,
        pan: f32,
        fade_in_ms: u32,
        fade_curve: FadeCurve,
        patch_id: Option<Uuid>,
        patch_slot: Option<u8>,
        patch_gain: f32,
        device_id: Option<&str>,
    ) -> Result<VoiceId> {
        let _ = (patch_id, patch_slot, device_id);
        self.play_mic_voice(
            feed_id,
            in_l,
            in_r,
            out_l,
            out_r,
            gain * patch_gain,
            pan,
            fade_in_ms,
            fade_curve,
        )
    }
    fn panic_stop_all(&self) -> Result<()>;
}

impl AudioEngineApi for AudioEngine {
    fn play_voice_routed(&self, voice: Voice, device_id: Option<&str>) -> Result<VoiceId> {
        AudioEngine::play_voice_routed(self, voice, device_id)
    }
    fn play_voice_paused_routed(&self, voice: Voice, device_id: Option<&str>) -> Result<VoiceId> {
        AudioEngine::play_voice_paused_routed(self, voice, device_id)
    }
    fn stop_voice(&self, voice_id: VoiceId, fade_ms: u32, fade_curve: FadeCurve) -> Result<()> {
        AudioEngine::stop_voice(self, voice_id, fade_ms, fade_curve)
    }
    fn pause_voice(&self, voice_id: VoiceId) -> Result<()> {
        AudioEngine::pause_voice(self, voice_id)
    }
    fn resume_voice(&self, voice_id: VoiceId) -> Result<()> {
        AudioEngine::resume_voice(self, voice_id)
    }
    fn seek_voice(&self, voice_id: VoiceId, frame_pos: u64) -> Result<()> {
        AudioEngine::seek_voice(self, voice_id, frame_pos)
    }
    fn devamp_voice(&self, voice_id: VoiceId, stop_at_end: bool) -> Result<()> {
        AudioEngine::devamp_voice(self, voice_id, stop_at_end)
    }
    fn set_voice_gain(&self, voice_id: VoiceId, gain: f32) -> Result<()> {
        AudioEngine::set_voice_gain(self, voice_id, gain)
    }
    fn get_voice_gain(&self, voice_id: VoiceId) -> f32 {
        AudioEngine::get_voice_gain(self, voice_id)
    }
    fn set_voice_pan(&self, voice_id: VoiceId, pan: f32) -> Result<()> {
        AudioEngine::set_voice_pan(self, voice_id, pan)
    }
    fn set_voice_level_matrix(
        &self,
        voice_id: VoiceId,
        matrix: Option<&super::voice::LevelMatrix>,
    ) -> Result<()> {
        AudioEngine::set_voice_level_matrix(self, voice_id, matrix)
    }
    fn get_voice_pan(&self, voice_id: VoiceId) -> f32 {
        AudioEngine::get_voice_pan(self, voice_id)
    }
    fn sample_rate(&self) -> u32 {
        AudioEngine::sample_rate(self)
    }
    fn voice_is_alive(&self, voice_id: VoiceId) -> bool {
        AudioEngine::voice_is_alive(self, voice_id)
    }
    fn ensure_input_feed(&self, device_id: Option<&str>, buffer_size: u32) -> Result<Uuid> {
        AudioEngine::ensure_input_feed(self, device_id, buffer_size)
    }
    fn register_synthetic_feed(
        &self,
        channels: usize,
        sample_rate: u32,
    ) -> Result<(Uuid, SyntheticFeedProducer)> {
        AudioEngine::register_synthetic_feed(self, channels, sample_rate)
    }
    fn register_network_feed(
        &self,
        channels: usize,
        sample_rate: u32,
    ) -> Result<(Uuid, SyntheticFeedProducer)> {
        AudioEngine::register_network_feed(self, channels, sample_rate)
    }
    fn play_mic_voice(
        &self,
        feed_id: Uuid,
        in_l: usize,
        in_r: usize,
        out_l: usize,
        out_r: usize,
        gain: f32,
        pan: f32,
        fade_in_ms: u32,
        fade_curve: FadeCurve,
    ) -> Result<VoiceId> {
        AudioEngine::play_mic_voice(
            self, feed_id, in_l, in_r, out_l, out_r, gain, pan, fade_in_ms, fade_curve,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn play_mic_voice_routed(
        &self,
        feed_id: Uuid,
        in_l: usize,
        in_r: usize,
        out_l: usize,
        out_r: usize,
        gain: f32,
        pan: f32,
        fade_in_ms: u32,
        fade_curve: FadeCurve,
        patch_id: Option<Uuid>,
        patch_slot: Option<u8>,
        patch_gain: f32,
        device_id: Option<&str>,
    ) -> Result<VoiceId> {
        AudioEngine::play_mic_voice_routed(
            self,
            feed_id,
            in_l,
            in_r,
            out_l,
            out_r,
            gain,
            pan,
            fade_in_ms,
            fade_curve,
            patch_id,
            patch_slot,
            patch_gain,
            device_id,
        )
    }
    fn panic_stop_all(&self) -> Result<()> {
        AudioEngine::panic_stop_all(self)
    }
}

// ---------------------------------------------------------------------------
// Output engine
// ---------------------------------------------------------------------------

/// The output-engine operations the cue and transport layers depend on.
pub trait OutputEngineApi: Send + Sync {
    /// Start the application's single shared external Browser WebView.
    /// Compatibility test doubles can leave this unsupported.
    fn start_browser_surface(
        &self,
        cue_id: uuid::Uuid,
        url: &str,
        reload_on_go: bool,
        zoom: f64,
        output_id: Option<&str>,
    ) -> Result<()> {
        let _ = (cue_id, url, reload_on_go, zoom, output_id);
        anyhow::bail!("Browser surfaces are unavailable in this output engine")
    }
    fn stop_browser_surface(&self, cue_id: uuid::Uuid, hard: bool) -> Result<()> {
        let _ = (cue_id, hard);
        Ok(())
    }
    /// Clear the shared browser surface when a transport reset has no cue
    /// context available to call the owner-specific stop method.
    fn clear_browser_surface(&self, hard: bool) -> Result<()> {
        let _ = hard;
        Ok(())
    }
    fn show_content(&self, req: ContentRequest<'_>) -> Result<VoiceId>;
    /// Fan out one visual cue to multiple named destinations. The first voice
    /// remains the canonical voice; the concrete engine propagates lifecycle
    /// operations to the other voices in the group.
    fn show_content_multi(
        &self,
        req: ContentRequest<'_>,
        output_ids: &[String],
    ) -> Result<VoiceId> {
        if output_ids.is_empty() {
            return self.show_content(req);
        }
        self.show_content(ContentRequest {
            output_id: Some(output_ids[0].as_str()),
            screen_index: None,
            ..req
        })
    }
    /// Show a live, receiver-owned BGRA mailbox. Test doubles may leave this
    /// unsupported; production OutputEngine uploads the latest frame on its
    /// render thread.
    fn show_external_bgra_source(
        &self,
        output_id: Option<&str>,
        source: Arc<super::network_io::BgraFrameMailbox>,
        geometry: super::output_engine::VideoGeometry,
        layer_style: super::output_engine::LayerStyle,
        fade_in_ms: u32,
    ) -> Result<VoiceId> {
        let _ = (output_id, source, geometry, layer_style, fade_in_ms);
        anyhow::bail!("External BGRA sources are unavailable in this output engine")
    }
    fn show_external_bgra_source_multi(
        &self,
        output_id: Option<&str>,
        output_ids: &[String],
        source: Arc<super::network_io::BgraFrameMailbox>,
        geometry: super::output_engine::VideoGeometry,
        layer_style: super::output_engine::LayerStyle,
        fade_in_ms: u32,
    ) -> Result<VoiceId> {
        if output_ids.is_empty() {
            return self.show_external_bgra_source(
                output_id,
                source,
                geometry,
                layer_style,
                fade_in_ms,
            );
        }
        let mut first = None;
        for id in output_ids {
            let voice = self.show_external_bgra_source(
                Some(id.as_str()),
                Arc::clone(&source),
                geometry,
                layer_style,
                fade_in_ms,
            )?;
            first.get_or_insert(voice);
        }
        first.ok_or_else(|| anyhow::anyhow!("No output destinations selected"))
    }
    /// Associate a live cue's synthetic-feed audio with its visual voice so a
    /// destination reconfiguration cannot leave sound playing on its own.
    /// Compatibility test doubles may safely ignore the association.
    fn attach_cue_audio_voice(&self, visual_voice_id: VoiceId, audio_voice_id: VoiceId) {
        let _ = (visual_voice_id, audio_voice_id);
    }
    fn stop_content(&self, voice_id: VoiceId, visual_fade_ms: u32, audio_fade_ms: u32);
    fn hard_stop_current(&self);
    fn panic_stop(&self);
    /// The AudioEngine voice carrying a video voice's audio track, if any.
    fn video_audio_voice(&self, voice_id: VoiceId) -> Option<VoiceId>;
    /// Whether mpv still reports this voice as actively playing.
    fn is_voice_playing(&self, voice_id: VoiceId) -> bool { let _ = voice_id; false }
    /// Re-anchor a video's paired audio voice to its actual picture position.
    fn resync_audio_to_video(&self, voice_id: VoiceId);
    /// Current animated opacity (0.0–1.0) of a voice's layer.
    fn get_voice_opacity(&self, voice_id: VoiceId) -> f32;
    /// Directly drive a voice's layer opacity (Fade Cue tick, ~30 fps).
    fn set_voice_opacity(&self, voice_id: VoiceId, opacity: f32);
    fn stop_voice(&self, voice_id: VoiceId, fade_ms: u32) -> Result<()>;
    fn pause_voice(&self, voice_id: VoiceId) -> Result<()>;
    fn resume_voice(&self, voice_id: VoiceId) -> Result<()>;
    fn seek_voice_ms(&self, voice_id: VoiceId, position_ms: u64);
    /// Seek a Video Cue in action-time coordinates. The output slot converts
    /// that position through its trim and loop settings after mpv reports the
    /// source duration, so a seek issued immediately after GO remains correct.
    fn seek_voice_action_ms(&self, voice_id: VoiceId, position_ms: u64) {
        self.seek_voice_ms(voice_id, position_ms);
    }
    fn show_text_overlay(&self, ass_text: &str, screen_index: Option<u32>);
    /// Output-id aware text overlay; compatibility default targets the legacy
    /// default surface for test doubles and older integrations. Each output
    /// owns one ASS overlay slot: a later Text Cue replaces the previous text,
    /// and clearing a cue clears that slot. This is deliberately singleton
    /// ownership (not a refcount), matching the existing Text Cue
    /// stop-on-next-GO semantics.
    fn show_text_overlay_for_output(
        &self,
        ass_text: &str,
        output_id: Option<&str>,
        screen_index: Option<u32>,
    ) {
        let _ = output_id;
        self.show_text_overlay(ass_text, screen_index);
    }
    fn show_text_overlay_for_outputs(
        &self,
        ass_text: &str,
        output_id: Option<&str>,
        output_ids: &[String],
        screen_index: Option<u32>,
    ) {
        if output_ids.is_empty() {
            self.show_text_overlay_for_output(ass_text, output_id, screen_index);
        } else {
            for id in output_ids {
                self.show_text_overlay_for_output(ass_text, Some(id.as_str()), None);
            }
        }
    }
    fn clear_text_overlay(&self);
    /// Clear an overlay on one named output. Compatibility implementations
    /// may keep targeting their legacy default surface.
    fn clear_text_overlay_for_output(&self, output_id: Option<&str>) {
        let _ = output_id;
        self.clear_text_overlay();
    }
    fn clear_text_overlay_for_outputs(&self, output_id: Option<&str>, output_ids: &[String]) {
        if output_ids.is_empty() {
            self.clear_text_overlay_for_output(output_id);
        } else {
            for id in output_ids {
                self.clear_text_overlay_for_output(Some(id.as_str()));
            }
        }
    }
    /// Start the visual fade that lands exactly on the content's natural end.
    /// Returns `false` when `voice_id` is no longer on the output window.
    fn begin_eof_fade_out(&self, voice_id: VoiceId, fade_ms: u32) -> bool;
    /// Release the visual voice's current slice loop (Devamp Cue).
    fn devamp_voice(&self, voice_id: VoiceId, stop_at_end: bool);
    /// Reveal + unpause content that was preloaded by a Load Cue.
    /// `false` = this voice was not preloaded.
    fn start_preloaded(&self, voice_id: VoiceId) -> bool;
}

impl OutputEngineApi for OutputEngine {
    fn start_browser_surface(
        &self,
        cue_id: uuid::Uuid,
        url: &str,
        reload_on_go: bool,
        zoom: f64,
        output_id: Option<&str>,
    ) -> Result<()> {
        OutputEngine::start_browser_surface(self, cue_id, url, reload_on_go, zoom, output_id)
    }
    fn stop_browser_surface(&self, cue_id: uuid::Uuid, hard: bool) -> Result<()> {
        OutputEngine::stop_browser_surface(self, cue_id, hard)
    }
    fn clear_browser_surface(&self, hard: bool) -> Result<()> {
        OutputEngine::clear_browser_surface(self, hard)
    }
    fn show_content(&self, req: ContentRequest<'_>) -> Result<VoiceId> {
        OutputEngine::show_content(self, req)
    }
    fn show_content_multi(
        &self,
        req: ContentRequest<'_>,
        output_ids: &[String],
    ) -> Result<VoiceId> {
        OutputEngine::show_content_multi(self, req, output_ids)
    }
    fn show_external_bgra_source(
        &self,
        output_id: Option<&str>,
        source: Arc<super::network_io::BgraFrameMailbox>,
        geometry: super::output_engine::VideoGeometry,
        layer_style: super::output_engine::LayerStyle,
        fade_in_ms: u32,
    ) -> Result<VoiceId> {
        OutputEngine::show_external_bgra_source(
            self,
            output_id,
            source,
            geometry,
            layer_style,
            fade_in_ms,
        )
    }
    fn show_external_bgra_source_multi(
        &self,
        output_id: Option<&str>,
        output_ids: &[String],
        source: Arc<super::network_io::BgraFrameMailbox>,
        geometry: super::output_engine::VideoGeometry,
        layer_style: super::output_engine::LayerStyle,
        fade_in_ms: u32,
    ) -> Result<VoiceId> {
        OutputEngine::show_external_bgra_source_multi(
            self,
            output_id,
            output_ids,
            source,
            geometry,
            layer_style,
            fade_in_ms,
        )
    }
    fn attach_cue_audio_voice(&self, visual_voice_id: VoiceId, audio_voice_id: VoiceId) {
        OutputEngine::attach_cue_audio_voice(self, visual_voice_id, audio_voice_id)
    }
    fn stop_content(&self, voice_id: VoiceId, visual_fade_ms: u32, audio_fade_ms: u32) {
        OutputEngine::stop_content(self, voice_id, visual_fade_ms, audio_fade_ms)
    }
    fn hard_stop_current(&self) {
        OutputEngine::hard_stop_current(self)
    }
    fn panic_stop(&self) {
        OutputEngine::panic_stop(self)
    }
    fn video_audio_voice(&self, voice_id: VoiceId) -> Option<VoiceId> {
        OutputEngine::video_audio_voice(self, voice_id)
    }
    fn is_voice_playing(&self, voice_id: VoiceId) -> bool {
        OutputEngine::is_voice_playing(self, voice_id)
    }
    fn resync_audio_to_video(&self, voice_id: VoiceId) {
        OutputEngine::resync_audio_to_video(self, voice_id)
    }
    fn get_voice_opacity(&self, voice_id: VoiceId) -> f32 {
        OutputEngine::get_voice_opacity(self, voice_id)
    }
    fn set_voice_opacity(&self, voice_id: VoiceId, opacity: f32) {
        OutputEngine::set_voice_opacity(self, voice_id, opacity)
    }
    fn stop_voice(&self, voice_id: VoiceId, fade_ms: u32) -> Result<()> {
        OutputEngine::stop_voice(self, voice_id, fade_ms)
    }
    fn pause_voice(&self, voice_id: VoiceId) -> Result<()> {
        OutputEngine::pause_voice(self, voice_id)
    }
    fn resume_voice(&self, voice_id: VoiceId) -> Result<()> {
        OutputEngine::resume_voice(self, voice_id)
    }
    fn seek_voice_ms(&self, voice_id: VoiceId, position_ms: u64) {
        OutputEngine::seek_voice_ms(self, voice_id, position_ms)
    }
    fn seek_voice_action_ms(&self, voice_id: VoiceId, position_ms: u64) {
        OutputEngine::seek_voice_action_ms(self, voice_id, position_ms)
    }
    fn show_text_overlay(&self, ass_text: &str, screen_index: Option<u32>) {
        OutputEngine::show_text_overlay(self, ass_text, screen_index)
    }
    fn show_text_overlay_for_output(
        &self,
        ass_text: &str,
        output_id: Option<&str>,
        screen_index: Option<u32>,
    ) {
        OutputEngine::show_text_overlay_on_output(self, ass_text, output_id, screen_index)
    }
    fn clear_text_overlay(&self) {
        OutputEngine::clear_text_overlay(self)
    }
    fn clear_text_overlay_for_output(&self, output_id: Option<&str>) {
        OutputEngine::clear_text_overlay_on_output(self, output_id)
    }
    fn begin_eof_fade_out(&self, voice_id: VoiceId, fade_ms: u32) -> bool {
        OutputEngine::begin_eof_fade_out(self, voice_id, fade_ms)
    }
    fn devamp_voice(&self, voice_id: VoiceId, stop_at_end: bool) {
        OutputEngine::devamp_voice(self, voice_id, stop_at_end)
    }
    fn start_preloaded(&self, voice_id: VoiceId) -> bool {
        OutputEngine::start_preloaded(self, voice_id)
    }
}

// ---------------------------------------------------------------------------
// DMX engine
// ---------------------------------------------------------------------------

/// The DMX-engine operations the cue layer depends on (Light Cue fades).
pub trait DmxEngineApi: Send + Sync {
    fn submit_fade(
        &self,
        universe: u16,
        channel: u16,
        width: ChannelWidth,
        target_norm: f64,
        dur: Duration,
        curve: FadeCurve,
    );
}

impl DmxEngineApi for DmxEngine {
    fn submit_fade(
        &self,
        universe: u16,
        channel: u16,
        width: ChannelWidth,
        target_norm: f64,
        dur: Duration,
        curve: FadeCurve,
    ) {
        DmxEngine::submit_fade(self, universe, channel, width, target_norm, dur, curve)
    }
}
