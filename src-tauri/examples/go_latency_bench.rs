//! Measure production Transport GO latency to the first consumed audio frame
//! or first captured non-black compositor frame.
//!
//! Usage:
//! cargo run --release --example go_latency_bench -- <manifest.json> <results.json> --audio-config <audio.json> [--repeats 10] [--only <fixture.name>]
//!
//! This is a diagnostic harness. It does not use the WebView or invoke a Tauri
//! command for GO. The compositor capture path runs at 30 fps and adds PBO
//! readback and frame inspection overhead. Its timestamp is a capture-time
//! estimate; it does not measure display scanout. Audio reports the first
//! positive voice position observed from the real output callback. Physical
//! output is digitally muted with the real engine's master gain, so this does
//! not measure sound at a speaker.

use std::{
    env,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::unbounded;
use inkue_lib::{
    cue::{
        context::CueContext,
        registry::CueRegistry,
        types::CueType,
        audio_cue::AudioCueFactory,
        image_cue::ImageCueFactory,
        video_cue::VideoCueFactory,
    },
    engine::{
        output_engine::OutputTransform,
        AudioEngine, DmxEngine, OutputEngine,
    },
    preferences::{
        FloatingWindowGeometry, MachineAudioConfig, OutputDestination,
        OutputSinkKind,
    },
    show::{cue_list::CueList, transport::Transport},
};
use serde::Deserialize;
use serde_json::{json, Value};

const TRIAL_TIMEOUT: Duration = Duration::from_secs(10);
const CAPTURE_SESSION: u64 = 1;
const OUTPUT_ID: &str = "default";

#[derive(Debug, Deserialize)]
struct Manifest {
    fixtures: Vec<Fixture>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    name: String,
    kind: String,
    path: PathBuf,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    bytes: Option<u64>,
    #[serde(default)]
    codec: Option<String>,
    #[serde(default)]
    sample_rate: Option<u32>,
}

struct Options {
    manifest_path: PathBuf,
    results_path: PathBuf,
    audio_config_path: PathBuf,
    repeats: usize,
    only: Option<String>,
}

fn usage() -> &'static str {
    "usage: go_latency_bench <manifest.json> <results.json> --audio-config <audio.json> [--repeats 10] [--only <fixture.name>]"
}

fn parse_options() -> Result<Options> {
    let mut args = env::args().skip(1);
    let manifest_path = args.next().context(usage())?.into();
    let results_path = args.next().context(usage())?.into();
    let mut audio_config_path = None;
    let mut repeats = 10usize;
    let mut only = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--audio-config" => audio_config_path = Some(PathBuf::from(args.next().context("--audio-config requires a path")?)),
            "--repeats" => repeats = args.next().context("--repeats requires a number")?.parse().context("invalid repeat count")?,
            "--only" => only = Some(args.next().context("--only requires a fixture name")?),
            _ => bail!("unknown argument '{arg}'\n{}", usage()),
        }
    }
    if repeats == 0 || repeats > 100 {
        bail!("--repeats must be between 1 and 100");
    }
    Ok(Options {
        manifest_path,
        results_path,
        audio_config_path: audio_config_path.context("--audio-config is required")?,
        repeats,
        only,
    })
}

fn read_machine_audio_config(path: &Path) -> Result<MachineAudioConfig> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read machine audio config {}", path.display()))?;
    let mut config: MachineAudioConfig = serde_json::from_str(&text)
        .with_context(|| format!("parse machine audio config {}", path.display()))?;
    config.backend = config.backend.for_this_platform();
    Ok(config)
}

fn configure_single_output(output: &OutputEngine) -> Result<()> {
    output.configure_outputs(
        &[OutputDestination {
            id: OUTPUT_ID.into(),
            name: "GO Latency Bench".into(),
            sink_kind: OutputSinkKind::Display,
            network: Default::default(),
            monitor: None,
            floating_window: Some(FloatingWindowGeometry {
                x: 40,
                y: 40,
                width: 1280,
                height: 720,
                maximized: false,
            }),
            enabled: true,
            always_on_top: false,
            hide_cursor: false,
            transform: OutputTransform::default(),
            fullscreen_locked: false,
        }],
        OUTPUT_ID,
    )?;
    Ok(())
}

fn unix_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(u64::MAX as u128) as u64
}

fn is_black(frame: &inkue_lib::engine::network_io::BgraFrame) -> bool {
    (0..frame.height as usize).all(|row| {
        let row_start = row * frame.stride as usize;
        (0..frame.width as usize).all(|column| {
            let offset = row_start + column * 4;
            frame.data[offset..offset + 3].iter().all(|channel| *channel <= 1)
        })
    })
}

fn wait_for_black(
    output: &OutputEngine,
    after_sequence: &mut u64,
    deadline: Instant,
    transport: &Transport,
    list: &mut CueList,
    cue_id: uuid::Uuid,
    next_tick: &mut Instant,
) -> Result<u64> {
    loop {
        match output.output_monitor_frame(OUTPUT_ID, *after_sequence, CAPTURE_SESSION)? {
            inkue_lib::engine::output_engine::OutputMonitorFrameRead::Frame {
                sequence, frame, ..
            } => {
                *after_sequence = sequence;
                if is_black(&frame) {
                    return Ok(sequence);
                }
            }
            inkue_lib::engine::output_engine::OutputMonitorFrameRead::Unchanged { sequence, .. } => {
                *after_sequence = sequence;
            }
            inkue_lib::engine::output_engine::OutputMonitorFrameRead::NoFrame => {}
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for a black compositor frame before GO");
        }
        tick_if_due(transport, list, cue_id, next_tick)?;
        thread::sleep(Duration::from_millis(1));
    }
}

fn wait_for_audio_stop(audio: &AudioEngine, callbacks_before: u64, deadline: Instant) -> Result<()> {
    let target = callbacks_before.saturating_add(2);
    while audio.callback_count() < target {
        if Instant::now() >= deadline {
            bail!("timed out waiting for output callbacks to drain the stop command");
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn make_cue_list(fixture: &Fixture, registry: &CueRegistry) -> Result<(CueList, uuid::Uuid)> {
    let id = uuid::Uuid::new_v4();
    let kind = match fixture.kind.to_ascii_lowercase().as_str() {
        "audio" => "audio",
        "video" => "video",
        "image" => "image",
        other => bail!("fixture '{}' has unsupported kind '{other}'", fixture.name),
    };
    let cue_json = json!({
        "type": kind,
        "cue_type": kind,
        "id": id,
        "name": fixture.name,
        "file_path": fixture.path,
        "pre_wait_ms": 0,
        "post_wait_ms": 0,
        "loop_count": 0,
        "fade_in_ms": null,
        "fade_out_ms": null,
        "video_fade_in_ms": null,
        "video_fade_out_ms": null,
        "display_duration_ms": null,
        "hold_last_frame": true,
    });
    let list_json = json!({
        "name": "GO Latency Bench",
        "mode": "sequential",
        "playhead_cue_id": id,
        "cues": [cue_json],
    });
    Ok((CueList::from_json(list_json, registry)?, id))
}

fn probe_and_inject_audio_metadata(list: &mut CueList, id: uuid::Uuid, fixture: &Fixture) -> Result<Option<inkue_lib::cue::media_decode::AudioStreamInfo>> {
    if fixture.kind.eq_ignore_ascii_case("image") {
        return Ok(None);
    }
    let probe = inkue_lib::cue::media_decode::probe_audio_track(&fixture.path)
        .with_context(|| format!("probe audio metadata for {}", fixture.path.display()))?;
    if fixture.kind.eq_ignore_ascii_case("audio") && probe.is_none() {
        bail!("audio fixture '{}' has no audio stream", fixture.name);
    }
    if let Some(info) = probe {
        let duration = info.total_frames.map(|frames| {
            Duration::from_secs_f64(frames as f64 / info.sample_rate.max(1) as f64)
        });
        list.get_mut(&id)
            .context("fixture cue disappeared after registry construction")?
            .accept_preloaded_stream(fixture.path.clone(), info.channels, info.sample_rate, duration);
        return Ok(Some(info));
    }
    Ok(None)
}

fn rebuild_fixture_cue(
    list: &mut CueList,
    registry: &CueRegistry,
    fixture: &Fixture,
    cue_id: uuid::Uuid,
    audio_info: Option<inkue_lib::cue::media_decode::AudioStreamInfo>,
) -> Result<()> {
    let cue_json = list.get(&cue_id).context("fixture cue disappeared before rebuild")?.serialize();
    let mut cue = registry.from_json(cue_json)?;
    if let Some(info) = audio_info {
        let duration = info.total_frames.map(|frames| {
            Duration::from_secs_f64(frames as f64 / info.sample_rate.max(1) as f64)
        });
        cue.accept_preloaded_stream(fixture.path.clone(), info.channels, info.sample_rate, duration);
    }
    if !list.replace_cue_recursive(&cue_id, cue) {
        bail!("fixture cue disappeared during rebuild");
    }
    Ok(())
}

fn make_registry() -> CueRegistry {
    let mut registry = CueRegistry::new();
    registry.register(CueType::Audio, Box::new(AudioCueFactory));
    registry.register(CueType::Video, Box::new(VideoCueFactory));
    registry.register(CueType::Image, Box::new(ImageCueFactory));
    registry
}

fn tick_cue(transport: &Transport, list: &mut CueList, id: uuid::Uuid) -> Result<()> {
    let cue = list.get_mut(&id).context("fixture cue disappeared during tick")?;
    cue.tick(&transport.context)?;
    Ok(())
}

fn tick_if_due(transport: &Transport, list: &mut CueList, cue_id: uuid::Uuid, next_tick: &mut Instant) -> Result<()> {
    let now = Instant::now();
    if now >= *next_tick {
        tick_cue(transport, list, cue_id)?;
        while *next_tick <= now {
            *next_tick += Duration::from_millis(33);
        }
    }
    Ok(())
}

fn run_trial(
    fixture: &Fixture,
    repeat_index: usize,
    registry: &CueRegistry,
    audio_info: Option<inkue_lib::cue::media_decode::AudioStreamInfo>,
    transport: &mut Transport,
    list: &mut CueList,
    cue_id: uuid::Uuid,
    audio: &AudioEngine,
    output: &OutputEngine,
    capture_cursor: &mut u64,
    capture_enabled: bool,
    next_tick: &mut Instant,
) -> Value {
    let reset_started = Instant::now();
    let reset_deadline = reset_started + TRIAL_TIMEOUT;
    let cleanup_result = (|| -> Result<()> {
        transport.hard_stop_all(std::slice::from_mut(list))?;
        let callbacks_before_stop = audio.callback_count();
        audio.panic_stop_all()?;
        wait_for_audio_stop(audio, callbacks_before_stop, reset_deadline)?;
        output.panic_stop();
        rebuild_fixture_cue(list, registry, fixture, cue_id, audio_info)?;
        if capture_enabled {
            output.set_output_monitor_source(Some(OUTPUT_ID), CAPTURE_SESSION)?;
            wait_for_black(output, capture_cursor, reset_deadline, transport, list, cue_id, next_tick)?;
        }
        Ok(())
    })();
    if let Err(error) = cleanup_result {
        return json!({
            "fixture": fixture.name,
            "repeatIndex": repeat_index,
            "ok": false,
            "phase": "cleanup",
            "error": error.to_string(),
            "cleanupMs": reset_started.elapsed().as_secs_f64() * 1000.0,
        });
    }

    // Let stop commands and decoder cancellation settle outside the timed
    // interval. Ticks still follow their process-wide phase during this wait.
    let settle_deadline = Instant::now() + Duration::from_millis(137);
    while Instant::now() < settle_deadline {
        if let Err(error) = tick_if_due(transport, list, cue_id, next_tick) {
            return failed_trial(fixture, repeat_index, "settle_tick", error, 0.0);
        }
        thread::sleep(Duration::from_millis(1));
    }

    let callbacks_before_go = audio.callback_count();
    let go_wall_us = unix_micros();
    let go_started = Instant::now();
    let go_result = transport.go_by_id(list, &cue_id);
    let go_dispatch_ms = go_started.elapsed().as_secs_f64() * 1000.0;
    if let Err(error) = go_result {
        return json!({
            "fixture": fixture.name,
            "repeatIndex": repeat_index,
            "ok": false,
            "phase": "go",
            "error": error.to_string(),
            "goDispatchMs": go_dispatch_ms,
        });
    }

    let deadline = go_started + TRIAL_TIMEOUT;
    if fixture.kind.eq_ignore_ascii_case("audio") {
        loop {
            let now = Instant::now();
            if let Err(error) = tick_if_due(transport, list, cue_id, next_tick) {
                return failed_trial(fixture, repeat_index, "tick", error, go_dispatch_ms);
            }
            let voice_id = list.get(&cue_id).and_then(|cue| cue.playing_voice_id());
            if let Some(position_ms) = voice_id.and_then(|voice| audio.voice_position_ms(voice)) {
                if position_ms > 0 {
                    let first_consumed_ms = go_started.elapsed().as_secs_f64() * 1000.0;
                    return json!({
                        "fixture": fixture.name,
                        "repeatIndex": repeat_index,
                        "processFirst": repeat_index == 0,
                        "ok": true,
                        "measurement": "go_to_first_rt_consumed_audio_frames",
                        "goDispatchMs": go_dispatch_ms,
                        "firstMs": first_consumed_ms,
                        "firstPositionMs": position_ms,
                        "callbackCountBeforeGo": callbacks_before_go,
                        "callbackCountAtFirstPosition": audio.callback_count(),
                        "callbackCountDelta": audio.callback_count().saturating_sub(callbacks_before_go),
                        "audioPositionPollMs": 1,
                    });
                }
            }
            if now >= deadline {
                return failed_trial(fixture, repeat_index, "audio_wait", anyhow!("timed out waiting for voice_position_ms > 0"), go_dispatch_ms);
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    loop {
        let now = Instant::now();
        if let Err(error) = tick_if_due(transport, list, cue_id, next_tick) {
            return failed_trial(fixture, repeat_index, "tick", error, go_dispatch_ms);
        }
        match output.output_monitor_frame(OUTPUT_ID, *capture_cursor, CAPTURE_SESSION) {
            Ok(inkue_lib::engine::output_engine::OutputMonitorFrameRead::Frame {
                sequence,
                captured_at_us,
                frame,
                ..
            }) => {
                *capture_cursor = sequence;
                if captured_at_us < go_wall_us {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                if !is_black(&frame) {
                    let capture_from_go_ms = captured_at_us.saturating_sub(go_wall_us) as f64 / 1000.0;
                    let frame_observed_ms = go_started.elapsed().as_secs_f64() * 1000.0;
                    if capture_from_go_ms > frame_observed_ms + 250.0 {
                        return json!({
                            "fixture": fixture.name,
                            "repeatIndex": repeat_index,
                            "processFirst": repeat_index == 0,
                            "ok": false,
                            "phase": "capture_clock_sanity",
                            "error": "capture timestamp is more than 250 ms ahead of the monotonic observation time",
                            "goDispatchMs": go_dispatch_ms,
                            "captureFromGoMs": capture_from_go_ms,
                            "frameObservedMs": frame_observed_ms,
                        });
                    }
                    let video_diagnostic = if fixture.kind.eq_ignore_ascii_case("video") {
                        let voice_id = list.get(&cue_id).and_then(|cue| cue.playing_voice_id());
                        output.video_runtime_diagnostics().into_iter()
                            .find(|entry| Some(entry.cue_voice_id) == voice_id)
                            .map(|entry| json!({
                                "fileLoaded": entry.file_loaded,
                                "paused": entry.paused,
                                "decoderFormat": entry.decoder_format,
                                "hwdecBackend": entry.hwdec_backend,
                                "width": entry.width,
                                "height": entry.height,
                                "fps": entry.fps,
                            }))
                    } else { None };
                    return json!({
                        "fixture": fixture.name,
                        "repeatIndex": repeat_index,
                        "processFirst": repeat_index == 0,
                        "ok": true,
                        "measurement": "go_to_first_nonblack_native_compositor_capture",
                        "goDispatchMs": go_dispatch_ms,
                        "firstMs": capture_from_go_ms,
                        "frameObservedMs": frame_observed_ms,
                        "capturedAtUnixMicros": captured_at_us,
                        "captureSequence": sequence,
                        "captureWidth": frame.width,
                        "captureHeight": frame.height,
                        "videoRuntimeDiagnosticAtCapture": video_diagnostic,
                        "captureFps": 30,
                        "captureOverhead": "native output monitor PBO readback, sequence polling, and full-frame RGB black scan",
                    });
                }
            }
            Ok(inkue_lib::engine::output_engine::OutputMonitorFrameRead::Unchanged { sequence, .. }) => {
                *capture_cursor = sequence;
            }
            Ok(inkue_lib::engine::output_engine::OutputMonitorFrameRead::NoFrame) => {}
            Err(error) => return failed_trial(fixture, repeat_index, "capture", error.into(), go_dispatch_ms),
        }
        if now >= deadline {
            return failed_trial(fixture, repeat_index, "visual_wait", anyhow!("timed out waiting for a non-black native compositor frame"), go_dispatch_ms);
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn failed_trial(fixture: &Fixture, repeat_index: usize, phase: &str, error: anyhow::Error, go_dispatch_ms: f64) -> Value {
    json!({
        "fixture": fixture.name,
        "repeatIndex": repeat_index,
        "processFirst": repeat_index == 0,
        "ok": false,
        "phase": phase,
        "error": error.to_string(),
        "goDispatchMs": go_dispatch_ms,
    })
}

fn percentile(sorted: &[f64], percentile: f64) -> Option<f64> {
    if sorted.is_empty() { return None; }
    let rank = (sorted.len() as f64 * percentile).ceil() as usize;
    sorted.get(rank.saturating_sub(1).min(sorted.len() - 1)).copied()
}

fn median(sorted: &[f64]) -> Option<f64> {
    match sorted.len() {
        0 => None,
        length if length % 2 == 0 => Some((sorted[length / 2 - 1] + sorted[length / 2]) / 2.0),
        length => sorted.get(length / 2).copied(),
    }
}

fn summarize(fixture: &Fixture, records: &[Value]) -> Value {
    let first = records.iter().find(|record| record["repeatIndex"] == 0 && record["ok"] == true);
    let mut repeat_times = records
        .iter()
        .filter(|record| record["repeatIndex"].as_u64().is_some_and(|index| index > 0) && record["ok"] == true)
        .filter_map(|record| record["firstMs"].as_f64())
        .collect::<Vec<_>>();
    repeat_times.sort_by(f64::total_cmp);
    let mut dispatch_times = records.iter().filter(|record| record["ok"] == true)
        .filter_map(|record| record["goDispatchMs"].as_f64()).collect::<Vec<_>>();
    dispatch_times.sort_by(f64::total_cmp);
    let failures = records.iter().filter(|record| record["ok"] != true).count();
    json!({
        "name": fixture.name,
        "kind": fixture.kind,
        "path": fixture.path,
        "bytes": fixture.bytes,
        "format": fixture.format,
        "codec": fixture.codec,
        "width": fixture.width,
        "height": fixture.height,
        "durationMs": fixture.duration_ms,
        "sampleRate": fixture.sample_rate,
        "firstMs": first.and_then(|record| record["firstMs"].as_f64()),
        "repeatMedianMs": median(&repeat_times),
        "repeatMinMs": repeat_times.first().copied(),
        "repeatP95Ms": percentile(&repeat_times, 0.95),
        "repeatMaxMs": repeat_times.last().copied(),
        "goDispatchMedianMs": median(&dispatch_times),
        "failedTrials": failures,
        "trialCount": records.len(),
        "successfulRepeatCount": repeat_times.len(),
    })
}

fn run_benchmark(options: Options, audio_config: MachineAudioConfig, fixtures: Vec<Fixture>) -> Result<()> {
    let profile_base = options
        .manifest_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("tmp/go-latency"));
    let isolated_profile = fs::canonicalize(profile_base)
        .with_context(|| format!("resolve benchmark profile root {}", profile_base.display()))?
        .join("profile");
    fs::create_dir_all(&isolated_profile)?;
    env::set_var("APPDATA", &isolated_profile);
    env::set_var("QLISA_MONITOR_CAPTURE_FPS", "30");

    // Tauri context with no WebView window. OutputEngine still owns one real
    // native compositor window, created through the existing benchmark path.
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default().build(context)?;
    let app_handle = app.handle().clone();

    let audio = AudioEngine::new(&audio_config)
        .context("open the configured real audio output; silent-engine fallback is intentionally disabled")?;
    audio.set_master_gain(0.0);
    let output = Arc::new(OutputEngine::new(Arc::clone(&audio), app_handle.clone())?);
    configure_single_output(&output)?;
    let visual_fixtures = fixtures.iter().any(|fixture| !fixture.kind.eq_ignore_ascii_case("audio"));
    if visual_fixtures && !output.set_output_monitor_source(Some(OUTPUT_ID), CAPTURE_SESSION)? {
        bail!("native output monitor capture selection was rejected");
    }

    let (events, _event_receiver) = unbounded();
    let context = CueContext::new(
        audio.clone(),
        output.clone(),
        events,
        0,
        Vec::new(),
        None,
        None,
        Vec::new(),
        Arc::new(DmxEngine::new()),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        audio_config.buffer_size,
    );
    let mut transport = Transport::new(context);
    let registry = make_registry();
    let mut summaries = Vec::new();
    let mut all_records = Vec::new();
    let mut capture_cursor = 0u64;
    // Keep one cue-tick phase across setup, cleanup, and every GO.
    let mut next_tick = Instant::now() + Duration::from_millis(33);

    for fixture in &fixtures {
        if !fixture.path.is_file() {
            bail!("fixture '{}' does not exist: {}", fixture.name, fixture.path.display());
        }
        let (mut list, cue_id) = make_cue_list(fixture, &registry)?;
        let audio_info = probe_and_inject_audio_metadata(&mut list, cue_id, fixture)?;
        let mut records = Vec::with_capacity(options.repeats);
        for repeat_index in 0..options.repeats {
            let record = run_trial(
                fixture,
                repeat_index,
                &registry,
                audio_info,
                &mut transport,
                &mut list,
                cue_id,
                &audio,
                &output,
                &mut capture_cursor,
                !fixture.kind.eq_ignore_ascii_case("audio"),
                &mut next_tick,
            );
            all_records.push(record.clone());
            records.push(record);
        }
        summaries.push(summarize(fixture, &records));
    }

    let report = json!({
        "schemaVersion": 1,
        "measurementName": "GO to first media progress",
        "createdAtUnixMicros": unix_micros(),
        "audioConfigPath": options.audio_config_path,
        "audioBackendConfigured": format!("{:?}", audio_config.backend),
        "audioDeviceNameConfigured": audio_config.device_name,
        "audioDeviceIdConfigured": audio_config.device_id,
        "audioSampleRateActual": audio.sample_rate(),
        "audioCallbackCountAtReport": audio.callback_count(),
        "audioMasterGain": 0.0,
        "isolatedAppDataProfile": isolated_profile.join("profile"),
        "runtimeLocalAppData": env::var("LOCALAPPDATA").ok(),
        "outputCount": 1,
        "outputWindowGeometry": { "width": 1280, "height": 720, "fullscreen": false },
        "transportGoMode": "direct Transport::go_by_id call; no click, Tauri command, IPC, or WebView dispatch",
        "warmupPolicy": "repeatIndex 0 is first launch in this process; later repeats are process-warm. Windows file cache is not cleared; this is not a disk-cold measurement.",
        "audioMetric": "GO to first voice_position_ms > 0, indicating frames consumed by the real audio callback; digital master gain is zero, so physical acoustic onset is not measured",
        "audioPreparation": "Audio metadata is probed before GO and injected as AudioStreamInfo; no whole-file PCM preload. Cue objects are rebuilt through CueRegistry after each hard-stop so each trial starts a fresh stream.",
        "visualMetric": "GO to first non-black RGB frame from native output compositor capture, timed with capture captured_at_us",
        "visualCaptureLimits": [
            "Capture runs at 30 fps and adds compositor capture, PBO readback, polling, and full-frame black-scan overhead.",
            "PBO timestamp and frame cadence make this a quantized capture-time estimate and an upper bound on visibility through this capture path.",
            "The metric does not measure display scanout or photons at the display.",
            "A legitimate black opening frame is skipped until the first non-black frame appears."
        ],
        "failedTrialPolicy": "Each trial has a 10-second GO and cleanup timeout. Failed attempts remain in trialRecords and failedTrials.",
        "summaries": summaries,
        "trialRecords": all_records,
    });
    if let Some(parent) = options.results_path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(&options.results_path, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("write results {}", options.results_path.display()))?;

    audio.panic_stop_all()?;
    output.panic_stop();
    drop(transport);
    drop(output);
    drop(audio);
    app_handle.exit(0);
    Ok(())
}

fn main() -> Result<()> {
    let options = parse_options()?;
    // Read the user's machine audio file before redirecting APPDATA.
    let audio_config = read_machine_audio_config(&options.audio_config_path)?;
    let manifest_text = fs::read_to_string(&options.manifest_path)
        .with_context(|| format!("read fixture manifest {}", options.manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&manifest_text)
        .with_context(|| format!("parse fixture manifest {}", options.manifest_path.display()))?;
    let fixtures = if let Some(name) = options.only.as_ref() {
        let fixture = manifest.fixtures.into_iter().find(|fixture| fixture.name == *name)
            .ok_or_else(|| anyhow!("fixture '{name}' was not found in {}", options.manifest_path.display()))?;
        vec![fixture]
    } else {
        manifest.fixtures
    };
    if fixtures.is_empty() {
        bail!("manifest contains no fixtures");
    }
    run_benchmark(options, audio_config, fixtures)
}
