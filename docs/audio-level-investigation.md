# Audio level investigation

## Finding

The normal cue route applies an equal-power pan law, including at center. At `pan = 0`, `Voice::pan_gains` returns `sqrt(0.5) = 0.7071068` for each side ([voice.rs](../src-tauri/src/engine/voice.rs)). This is `20 log10(0.7071068) = -3.0103 dB` per output channel relative to a player that sends each decoded channel at unity. It is a confirmed, deterministic channel-level difference. It could explain a report of Qlisa sounding roughly 20% quieter, but “20% quieter” has no defined audio measurement and cannot be established from this coefficient alone.

For a stereo cue with samples `L` and `R`, the centered outputs are `0.707L` and `0.707R`. Their summed power is half the unity-per-channel result, or `-3.01 dB`. For a mono cue with sample `x`, the engine duplicates it to both channels, producing `0.707x` on each; the two-channel summed power then equals the power of the single source channel `x`. Each output channel is still 3.01 dB lower. Correlation, downmixing, and the playback system affect perceived or measured loudness.

## Signal path and other gain stages

Audio is decoded to interleaved float samples by `media_decode`; mono stream frames are duplicated into the right slot when read. `AudioCue::start_audio_action` converts cue `volume_db` to linear gain and applies the cue mute flag. The callback then applies pan, patch gain and any fade. The default Main patch has `gain_db = 0 dB` (unity). The callback applies master gain after mixing; its initial value is `1.0` (unity), and the transport command can change it at runtime. The callback sends the canonical stereo program tap after master gain.

The resulting per-channel Main path at center, before fades, is:

`decoded_sample × cue_gain × 0.7071068 × patch_gain × master_gain`

With cue level `0 dB`, Main patch `0 dB`, and master `0 dB`, this is `0.7071068` per channel, or `-3.0103 dB`. A mute produces exact zero. A nonzero cue, patch, or master setting multiplies this value and can add or remove level independently.

An explicit level matrix replaces pan for physical device output. Its crosspoints are converted from dB to linear gain; a `0 dB` crosspoint is unity. Thus, a stereo identity matrix at `0 dB` produces each source channel at unity on its corresponding device output, which is 3.0103 dB higher per channel than the centered Main pan route, before cue, patch, fade, and master gains. Matrix rows are summed. For a mono cue, the engine presents the same mono sample as both input channels; routing both rows at `0 dB` to one output sums them to `2 × sample` (+6.0206 dB). A matrix that routes only one row avoids that doubling. An all-silent matrix intentionally silences physical output.

The canonical stereo program tap is built from the pan/cue path before physical matrix routing. Therefore its level can differ from the device output when a matrix is configured. Main routes through the configured program stream; a Main patch does not add a second physical patch route. Aux patches use their own bound stream and patch gain. Headphone preview gain is a separate machine setting and does not affect program cues.

## Local level settings checked

The machine config reports `preview_gain_db = 0 dB`; this is preview-only. The global Preferences file reports `default_volume_db = 0 dB`; this is the default for newly created cues, not a master playback control. The selected backend is WASAPI Shared. Shared mode delegates endpoint/session behavior to Windows, so the repository cannot establish the effective Windows session volume, endpoint volume, driver gain, or interface analog level. No claim about those levels follows from the saved backend choice.

## Comparing against another player

Use the same file and exact region in both applications. Set the cue to `0 dB`, unmuted, centered pan, Main patch at `0 dB`, and master at `0 dB`. Set the other player to unity and disable its normalization, EQ, effects, and crossfade. Capture both outputs digitally at the same device format and sample rate, then compare aligned per-channel RMS or LUFS over the same non-silent region. Record endpoint/session volume and any interface controls. Repeat with an explicit `0 dB` stereo identity matrix: the predicted per-channel difference from ordinary centered Main output is `+3.0103 dB`. This comparison can identify the application-level contribution; physical output measurements are needed to account for Windows and interface gain.

## Relevant source locations

- `src-tauri/src/cue/media_decode.rs:244` — mono stream frame duplication.
- `src-tauri/src/cue/audio_cue.rs:364` — cue dB to linear gain; `:547` patch gain; `:561` matrix install.
- `src-tauri/src/engine/voice.rs:721` — pan law; `:327` mute-aware voice gain.
- `src-tauri/src/engine/audio_engine.rs:2833` — pan, patch and matrix gain preparation; `:2999` canonical pan mix; `:3008` matrix device route; `:3066` master gain.
- `src-tauri/src/engine/device_manager.rs:48` and `:89` — patch gain field and Main's unity default.
- `src-tauri/src/preferences.rs:139` and `:215` — preview and new-cue defaults.
- `src-tauri/src/engine/audio_engine.rs:4950` — existing test checks cue, patch, pan and master multiplication.

No source-level gain behavior was changed for this investigation.
