# Audio level investigation

## Finding and fix

`Voice::pan_gains` previously applied equal-power pan to every voice. This scaled both channels of a centered stereo cue by `sqrt(0.5) = 0.7071068`, or `-3.0103 dB`, while a stereo player at unity preserves each channel. The method now uses a balance pan for non-live voices with more than one channel:

`left = clamp(1 - max(pan, 0), 0, 1)`

`right = clamp(1 + min(pan, 0), 0, 1)`

At center, gains are `(1, 1)`. At the left and right edges they are `(1, 0)` and `(0, 1)`. Between center and an edge, only the opposite channel attenuates. The original left and right samples remain separate in ordinary stereo output. This adds `3.0103 dB` to the centered per-channel output of existing stereo cues; saved cue levels and pans remain unchanged.

Mono file cues retain the prior equal-power law, including `0.7071068` per output at center. Live Mic input also retains its previous equal-power behavior. Stereo audio preview uses the shared file-voice path and now has unity gain at center. Stereo Video audio follows the same rule when its decoder provides more than one channel; mono sources retain equal-power behavior. A cue with both physical output indices assigned to one channel still uses the existing output fold: `(program_l + program_r) * FRAC_1_SQRT_2`. That fold can exceed unity for correlated material; no new normalization or clipping protection was added.

## Signal path and other gain stages

Audio is decoded to interleaved float samples by `media_decode`; mono stream frames are duplicated into the right slot when read. `AudioCue::start_audio_action` converts cue `volume_db` to linear gain and applies the cue mute flag. The callback then applies pan, patch gain and any fade. The default Main patch has `gain_db = 0 dB` (unity). The callback applies master gain after mixing; its initial value is `1.0` (unity), and the transport command can change it at runtime. The callback sends the canonical stereo program tap after master gain. Ordinary stereo file, streaming and video voices share `Voice::pan_gains`; live Mic voices use the preserved equal-power branch.

The resulting per-channel Main path at center, before fades, is:

`decoded_sample × cue_gain × balance_pan_gain × patch_gain × master_gain`

For stereo at center, `balance_pan_gain` is `1.0`; with cue level, Main patch and master all at `0 dB`, the source channel reaches its output at unity. Mono and live voices keep the old `0.7071068` center coefficient. A mute produces exact zero. A nonzero cue, patch, or master setting multiplies this value and can add or remove level independently.

An explicit level matrix replaces pan for physical device output. Its crosspoints are converted from dB to linear gain; a `0 dB` crosspoint is unity. A stereo identity matrix at `0 dB` still sends each source channel at unity to its corresponding device output. Matrix rows are summed. For a mono cue, the engine presents the same mono sample as both input channels; routing both rows at `0 dB` to one output sums them to `2 × sample` (+6.0206 dB). A matrix that routes only one row avoids that doubling. An all-silent matrix intentionally silences physical output. The canonical program tap uses the corrected ordinary pan path; matrix physical routing remains unchanged.

The canonical stereo program tap is built from the pan/cue path before physical matrix routing. Therefore its level can differ from the device output when a matrix is configured. Main routes through the configured program stream; a Main patch does not add a second physical patch route. Aux patches use their own bound stream and patch gain. Headphone preview gain is a separate machine setting and does not affect program cues.

## Local level settings checked

The machine config reports `preview_gain_db = 0 dB`; this is preview-only. The global Preferences file reports `default_volume_db = 0 dB`; this is the default for newly created cues, not a master playback control. The selected backend is WASAPI Shared. Shared mode delegates endpoint/session behavior to Windows, so the repository cannot establish the effective Windows session volume, endpoint volume, driver gain, or interface analog level. No claim about those levels follows from the saved backend choice.

## Comparing against another player

Use the same file and exact region in both applications. Set the cue to `0 dB`, unmuted, centered pan, Main patch at `0 dB`, and master at `0 dB`. Set the other player to unity and disable its normalization, EQ, effects, and crossfade. Capture both outputs digitally at the same device format and sample rate, then compare aligned per-channel RMS or LUFS over the same non-silent region. Record endpoint/session volume and any interface controls. This comparison can identify the application-level contribution; physical output measurements are needed to account for Windows and interface gain.

## Relevant source locations

- `src-tauri/src/cue/media_decode.rs:244` — mono stream frame duplication.
- `src-tauri/src/cue/audio_cue.rs:364` — cue dB to linear gain; `:547` patch gain; `:561` matrix install.
- `src-tauri/src/engine/voice.rs:721` — stereo balance / mono and live equal-power pan; `:327` mute-aware voice gain.
- `src-tauri/src/engine/audio_engine.rs:2833` — pan, patch and matrix gain preparation; `:2999` canonical pan mix; `:3008` matrix device route; `:3066` master gain.
- `src-tauri/src/engine/device_manager.rs:48` and `:89` — patch gain field and Main's unity default.
- `src-tauri/src/preferences.rs:139` and `:215` — preview and new-cue defaults.
- `src-tauri/src/engine/audio_engine.rs:4955` — callback test checks cue, patch and master multiplication; adjacent tests cover stereo balance, mono pan, and explicit matrix routing.

The source change is limited to the existing pan-gain calculation. Cue, patch, master and saved gain values are unchanged.
