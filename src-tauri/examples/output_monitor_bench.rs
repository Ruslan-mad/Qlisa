//! Native Output Monitor benchmark.
//!
//! Run with `cargo run --release --example output_monitor_bench -- <video> [seconds] [off|4|30] [layers] [webview|checks|webview-checks]`.
//! This measures the native renderer, mailbox, and production packet encoder.
//! Add `webview` as the final argument to exercise the real Output Monitor window,
//! Tauri IPC, and Canvas conversion. It does not measure display scanout.

use std::{
    env,
    path::Path,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use serde_json::json;
use inkue_lib::{
    commands::preferences_cmds::{
        encode_output_monitor_packet, OutputMonitorFrontendMetrics, OutputMonitorSource,
    },
    engine::{
        audio_engine::AudioEngine,
        output_engine::{ContentRequest, LayerStyle, OutputEngine, OutputTransform, VideoGeometry},
    },
    preferences::{FloatingWindowGeometry, MachineAudioConfig, OutputDestination, OutputSinkKind},
};
use tauri::{Emitter, Manager, State};

struct BenchState {
    output: Arc<OutputEngine>,
    frontend: Arc<std::sync::Mutex<FrontendAggregate>>,
    sources: Vec<(String, String)>,
}

#[derive(Default)]
struct FrontendAggregate {
    samples: u64,
    active_samples: u64,
    received_fps_total: f64,
    displayed_fps_total: f64,
    frame_age_average_total: f64,
    frame_age_average_samples: u64,
    frame_age_average_min: Option<f64>,
    frame_age_average_max: Option<f64>,
    frame_age_maximum_max: Option<f64>,
    conversion_average_total: f64,
    conversion_samples: u64,
    conversion_maximum_max: Option<f64>,
    request_average_total: f64,
    request_samples: u64,
    request_maximum_max: Option<f64>,
    received_frames: u64,
    displayed_frames: u64,
    latest_session: Option<u64>,
}

impl FrontendAggregate {
    fn record(&mut self, metrics: OutputMonitorFrontendMetrics) {
        self.samples = self.samples.saturating_add(1);
        if metrics.active {
            self.active_samples = self.active_samples.saturating_add(1);
        }
        if metrics.active {
            self.received_fps_total += metrics.received_fps.unwrap_or_default();
            self.displayed_fps_total += metrics.displayed_fps.unwrap_or_default();
        }
        self.received_frames = self.received_frames.saturating_add(metrics.received_frames);
        self.displayed_frames = self.displayed_frames.saturating_add(metrics.displayed_frames);
        if let Some(value) = metrics.frame_age_average_ms {
            self.frame_age_average_total += value;
            self.frame_age_average_samples = self.frame_age_average_samples.saturating_add(1);
            self.frame_age_average_min = Some(self.frame_age_average_min.map_or(value, |previous| previous.min(value)));
            self.frame_age_average_max = Some(self.frame_age_average_max.map_or(value, |previous| previous.max(value)));
        }
        if let Some(value) = metrics.frame_age_max_ms {
            self.frame_age_maximum_max = Some(self.frame_age_maximum_max.map_or(value, |previous| previous.max(value)));
        }
        if let Some(value) = metrics.conversion_average_ms {
            self.conversion_average_total += value;
            self.conversion_samples = self.conversion_samples.saturating_add(1);
        }
        if let Some(value) = metrics.conversion_max_ms {
            self.conversion_maximum_max = Some(self.conversion_maximum_max.map_or(value, |previous| previous.max(value)));
        }
        if let Some(value) = metrics.request_average_ms {
            self.request_average_total += value;
            self.request_samples = self.request_samples.saturating_add(1);
        }
        if let Some(value) = metrics.request_max_ms {
            self.request_maximum_max = Some(self.request_maximum_max.map_or(value, |previous| previous.max(value)));
        }
        self.latest_session = metrics.session.or(self.latest_session);
    }

    fn summary(&self) -> serde_json::Value {
        json!({
            "samples": self.samples,
            "activeSamples": self.active_samples,
            "receivedFpsAverage": (self.active_samples > 0).then(|| self.received_fps_total / self.active_samples as f64),
            "displayedFpsAverage": (self.active_samples > 0).then(|| self.displayed_fps_total / self.active_samples as f64),
            "fpsAverageSampleCount": self.active_samples,
            "receivedFrames": self.received_frames,
            "displayedFrames": self.displayed_frames,
            "frameAgeAverageMs": (self.frame_age_average_samples > 0).then(|| self.frame_age_average_total / self.frame_age_average_samples as f64),
            "frameAgeAverageMinMs": self.frame_age_average_min,
            "frameAgeAverageMaxMs": self.frame_age_average_max,
            "frameAgeObservedMaxMs": self.frame_age_maximum_max,
            "conversionAverageMs": (self.conversion_samples > 0).then(|| self.conversion_average_total / self.conversion_samples as f64),
            "conversionObservedMaxMs": self.conversion_maximum_max,
            "requestAverageMs": (self.request_samples > 0).then(|| self.request_average_total / self.request_samples as f64),
            "requestObservedMaxMs": self.request_maximum_max,
            "latestSession": self.latest_session,
        })
    }
}

#[tauri::command]
fn list_output_monitor_sources(state: State<'_, BenchState>) -> Vec<OutputMonitorSource> {
    state.sources.iter().map(|(id, name)| OutputMonitorSource { id: id.clone(), name: name.clone() }).collect()
}

#[tauri::command]
fn set_output_monitor_source(
    source_id: Option<String>,
    selection_token: u64,
    state: State<'_, BenchState>,
) -> Result<(), String> {
    state.output.set_output_monitor_source(source_id.as_deref(), selection_token)
        .map_err(|error| error.to_string())?
        .then_some(())
        .ok_or_else(|| "stale output monitor selection token".to_string())
}

#[tauri::command]
async fn get_output_monitor_frame(
    source_id: String,
    after_sequence: Option<u64>,
    selection_token: u64,
    state: State<'_, BenchState>,
) -> Result<tauri::ipc::Response, String> {
    let output = Arc::clone(&state.output);
    let frame = output.output_monitor_frame(&source_id, after_sequence.unwrap_or(0), selection_token)
        .map_err(|error| error.to_string())?;
    let request_delay_ms = env::var("QLISA_MONITOR_BENCH_REQUEST_DELAY_MS")
        .ok().and_then(|value| value.parse::<u64>().ok()).unwrap_or(0).min(1000);
    tauri::async_runtime::spawn_blocking(move || {
        use inkue_lib::engine::output_engine::OutputMonitorFrameRead;
        let started = Instant::now();
        if request_delay_ms > 0 {
            thread::sleep(Duration::from_millis(request_delay_ms));
        }
        match frame {
                OutputMonitorFrameRead::NoFrame => encode_output_monitor_packet(0, 0, 0, 0,
                    0, selection_token, 0, started.elapsed().as_micros().min(u32::MAX as u128) as u32, &[]),
                OutputMonitorFrameRead::Unchanged { sequence, session, captured_at_us } =>
                    encode_output_monitor_packet(2, 0, 0, 0, sequence, session, captured_at_us,
                        started.elapsed().as_micros().min(u32::MAX as u128) as u32, &[]),
                OutputMonitorFrameRead::Frame { sequence, session, captured_at_us, frame } => {
                    frame.validate().map_err(|error| error.to_string())?;
                    if frame.width == 0 || frame.height == 0 || frame.width > 640 || frame.height > 360 {
                        return Err("output monitor frame dimensions exceed 640×360".to_string());
                    }
                    let black = (0..frame.height as usize).all(|row| {
                        let row_start = row * frame.stride as usize;
                        (0..frame.width as usize).all(|column| {
                            let offset = row_start + column * 4;
                            frame.data[offset..offset + 3] == [0, 0, 0]
                        })
                    });
                    let status = if black { 3 } else { 1 };
                    let payload = if black { &[][..] } else { &frame.data[..] };
                    let prepare_us = started.elapsed().as_micros().min(u32::MAX as u128) as u32;
                    encode_output_monitor_packet(status, frame.width, frame.height, frame.stride,
                        sequence, session, captured_at_us, prepare_us, payload)
                }
            }
            .map(tauri::ipc::Response::new)
    })
    .await
    .map_err(|error| format!("Output monitor encoder task failed: {error}"))?
}

#[tauri::command]
fn report_output_monitor_frontend_diagnostics(
    metrics: OutputMonitorFrontendMetrics,
    state: State<'_, BenchState>,
) -> Result<(), String> {
    if metrics.source_id.as_ref().is_some_and(|source_id| !state.sources.iter().any(|(id, _)| id == source_id)) {
        return Err("unexpected output monitor source id".into());
    }
    let values = [metrics.received_fps, metrics.displayed_fps, metrics.frame_age_last_ms,
        metrics.frame_age_average_ms, metrics.frame_age_max_ms, metrics.conversion_average_ms,
        metrics.conversion_max_ms, metrics.request_average_ms, metrics.request_max_ms];
    if values.into_iter().flatten().any(|value| !value.is_finite() || value < 0.0) {
        return Err("output monitor metrics must be finite non-negative numbers".into());
    }
    let mut aggregate = state.frontend.lock().map_err(|_| "benchmark metrics lock poisoned".to_string())?;
    aggregate.record(metrics);
    Ok(())
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct FileTime {
    low: u32,
    high: u32,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct ProcessMemoryCountersEx {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_usage: usize,
}

#[cfg(windows)]
fn process_metrics() -> (Option<f64>, Option<u64>) {
    #[link(name = "Kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn GetProcessTimes(
            process: *mut std::ffi::c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    #[link(name = "Psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut ProcessMemoryCountersEx,
            size: u32,
        ) -> i32;
    }
    fn to_100ns(time: &FileTime) -> u64 {
        (u64::from(time.high) << 32) | u64::from(time.low)
    }

    static LAST: std::sync::OnceLock<std::sync::Mutex<Option<(u64, Instant)>>> =
        std::sync::OnceLock::new();
    let now = Instant::now();
    let mut creation = FileTime::default();
    let mut exit = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    let process = unsafe { GetCurrentProcess() };
    let cpu = if unsafe {
        GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user)
    } != 0
    {
        let current = to_100ns(&kernel).saturating_add(to_100ns(&user));
        LAST.get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .ok()
            .and_then(|mut last| {
                let percent = last.and_then(|(previous, when)| {
                    let wall = now.duration_since(when).as_secs_f64();
                    (wall > 0.0).then(|| {
                        ((current.saturating_sub(previous) as f64 / 10_000_000.0)
                            / wall
                            / thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64
                            * 100.0)
                            .clamp(0.0, 100.0)
                    })
                });
                *last = Some((current, now));
                percent
            })
    } else {
        None
    };
    let mut memory = ProcessMemoryCountersEx {
        cb: std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        ..Default::default()
    };
    let working_set = (unsafe {
        GetProcessMemoryInfo(
            process,
            &mut memory,
            std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        )
    } != 0)
        .then_some(memory.working_set_size as u64);
    (cpu, working_set)
}

fn video_runtime_summary(output: &OutputEngine) -> serde_json::Value {
    let diagnostics = output.video_runtime_diagnostics();
    serde_json::json!({
        "slots": diagnostics.len(),
        "loaded": diagnostics.iter().filter(|slot| slot.file_loaded).count(),
        "droppedFrames": diagnostics.iter().filter_map(|slot| slot.dropped_frames).sum::<u64>(),
        "delayedFrames": diagnostics.iter().filter_map(|slot| slot.delayed_frames).sum::<u64>(),
        "fps": diagnostics.iter().filter_map(|slot| slot.fps).collect::<Vec<_>>(),
        "sizes": diagnostics.iter().filter_map(|slot| slot.width.zip(slot.height)).collect::<Vec<_>>(),
    })
}

fn video_runtime_drop_counts(output: &OutputEngine) -> (u64, u64) {
    let diagnostics = output.video_runtime_diagnostics();
    (
        diagnostics.iter().filter_map(|slot| slot.dropped_frames).sum(),
        diagnostics.iter().filter_map(|slot| slot.delayed_frames).sum(),
    )
}

fn is_image_path(path: &Path) -> bool {
    matches!(path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "tif" | "tiff"))
}

fn configure_output(output: &OutputEngine, path: &Path, layers: u32, is_image: bool) -> Result<()> {
    for layer in 0..layers {
        output.show_content(ContentRequest {
            file_path: path,
            is_image,
            fade_in_ms: 500,
            loop_count: u32::MAX,
            initial_seek_action_ms: None,
            start_ms: None,
            end_ms: None,
            screen_index: None,
            output_id: None,
            audio_voice_id: None,
            display_duration_ms: None,
            hold_last_frame: true,
            geometry: VideoGeometry::default(),
            live_source: false,
            layer_style: LayerStyle { layer: Some(layer + 1), ..LayerStyle::default() },
            slices: Vec::new(),
            preload: false,
        })?;
    }
    if env::var_os("QLISA_MONITOR_BENCH_NO_OVERLAY").is_none() {
        output.set_output_transform(OutputTransform {
            scale: 0.98,
            corners: [[0.005, 0.005], [-0.005, 0.005], [0.005, -0.005], [-0.005, -0.005]],
            ..OutputTransform::default()
        });
    }
    Ok(())
}

fn write_solid_bmp(path: &Path, rgb: [u8; 3]) -> Result<()> {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 64;
    let row_bytes = (WIDTH * 3 + 3) & !3;
    let pixel_bytes = row_bytes * HEIGHT;
    let mut bmp = vec![0_u8; (54 + pixel_bytes) as usize];
    let file_bytes = bmp.len() as u32;
    bmp[0..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&file_bytes.to_le_bytes());
    bmp[10..14].copy_from_slice(&54_u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40_u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&(WIDTH as i32).to_le_bytes());
    bmp[22..26].copy_from_slice(&(HEIGHT as i32).to_le_bytes());
    bmp[26..28].copy_from_slice(&1_u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24_u16.to_le_bytes());
    bmp[34..38].copy_from_slice(&pixel_bytes.to_le_bytes());
    for row in 0..HEIGHT as usize {
        let start = 54 + row * row_bytes as usize;
        for col in 0..WIDTH as usize {
            let pixel = start + col * 3;
            bmp[pixel..pixel + 3].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        }
    }
    std::fs::write(path, bmp).with_context(|| format!("write color fixture {}", path.display()))
}

fn show_check_image(output: &OutputEngine, output_id: &str, path: &Path) -> Result<()> {
    output.show_content(ContentRequest {
        file_path: path,
        is_image: true,
        fade_in_ms: 0,
        loop_count: 0,
        initial_seek_action_ms: None,
        start_ms: None,
        end_ms: None,
        screen_index: None,
        output_id: Some(output_id),
        audio_voice_id: None,
        display_duration_ms: None,
        hold_last_frame: true,
        geometry: VideoGeometry::default(),
        live_source: false,
        layer_style: LayerStyle::default(),
        slices: Vec::new(),
        preload: false,
    })?;
    Ok(())
}

fn assert_frame_color(
    frame: &inkue_lib::engine::network_io::BgraFrame,
    rgb: [u8; 3],
    aspect: f64,
    label: &str,
) -> Result<()> {
    frame.validate().map_err(anyhow::Error::msg)?;
    if frame.width > 640 || frame.height > 360 {
        return Err(anyhow!("{label}: monitor frame exceeds 640×360: {}×{}", frame.width, frame.height));
    }
    let got_aspect = frame.width as f64 / frame.height as f64;
    if (got_aspect - aspect).abs() > 0.035 {
        return Err(anyhow!("{label}: aspect ratio {got_aspect:.3} differs from expected {aspect:.3}"));
    }
    let offset = (frame.height as usize / 2) * frame.stride as usize
        + (frame.width as usize / 2) * 4;
    let got = &frame.data[offset..offset + 3];
    let expected = [rgb[2], rgb[1], rgb[0]];
    if got.iter().zip(expected).any(|(actual, wanted)| actual.abs_diff(wanted) > 32) {
        return Err(anyhow!("{label}: center BGRA {:?}, expected near {:?}", got, expected));
    }
    Ok(())
}

fn check_capture(
    output: &OutputEngine,
    source_id: &str,
    sequence: &mut u64,
    session: u64,
    rgb: [u8; 3],
    aspect: f64,
    label: &str,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut last_mismatch = None;
    loop {
        match output.output_monitor_frame(source_id, *sequence, session)? {
            inkue_lib::engine::output_engine::OutputMonitorFrameRead::Frame {
                sequence: next_sequence, session: frame_session, frame, ..
            } => {
                *sequence = next_sequence;
                if frame_session != session {
                    return Err(anyhow!("frame session {frame_session} does not match requested session {session}"));
                }
                match assert_frame_color(&frame, rgb, aspect, label) {
                    Ok(()) => return Ok(()),
                    Err(error) => last_mismatch = Some(error),
                }
            }
            inkue_lib::engine::output_engine::OutputMonitorFrameRead::Unchanged { .. }
            | inkue_lib::engine::output_engine::OutputMonitorFrameRead::NoFrame => {}
        }
        if Instant::now() >= deadline {
            return Err(last_mismatch.unwrap_or_else(|| anyhow!("timed out waiting for '{label}' frame from '{source_id}'")));
        }
        thread::sleep(Duration::from_millis(40));
    }
}

fn check_destinations(name_a: &str, name_b: &str) -> [OutputDestination; 2] {
    [
        OutputDestination {
            id: "default".into(), name: name_a.into(),
            sink_kind: OutputSinkKind::Display, network: Default::default(), monitor: None,
            floating_window: Some(FloatingWindowGeometry { x: 40, y: 40, width: 320, height: 180, maximized: false }),
            enabled: true, always_on_top: false, hide_cursor: false,
            transform: OutputTransform::default(), fullscreen_locked: false,
        },
        OutputDestination {
            id: "bench-b".into(), name: name_b.into(),
            sink_kind: OutputSinkKind::Display, network: Default::default(), monitor: None,
            floating_window: Some(FloatingWindowGeometry { x: 400, y: 40, width: 320, height: 568, maximized: false }),
            enabled: true, always_on_top: false, hide_cursor: false,
            transform: OutputTransform::default(), fullscreen_locked: false,
        },
    ]
}

fn create_check_fixtures(fixture_dir: &Path) -> Result<std::collections::HashMap<&'static str, std::path::PathBuf>> {
    let colors = [
        ("red", [255, 0, 0]), ("green", [0, 255, 0]), ("blue", [0, 0, 255]),
        ("white", [255, 255, 255]), ("black", [0, 0, 0]),
    ];
    std::fs::create_dir_all(fixture_dir)?;
    let mut paths = std::collections::HashMap::new();
    for (name, rgb) in colors {
        let path = fixture_dir.join(format!("{name}.bmp"));
        write_solid_bmp(&path, rgb)?;
        paths.insert(name, path);
    }
    Ok(paths)
}

fn run_integration_checks(output: &OutputEngine, fixture_dir: &Path) -> Result<serde_json::Value> {
    let checks = (|| -> Result<serde_json::Value> {
        let screens = output.list_screens();
        if screens.is_empty() { return Err(anyhow!("no connected display available")); }
        output.configure_outputs(&check_destinations("Bench landscape", "Bench portrait"), "default")?;
        let paths = create_check_fixtures(fixture_dir)?;
        let colors = [
            ("red", [255, 0, 0]), ("green", [0, 255, 0]), ("blue", [0, 0, 255]),
            ("white", [255, 255, 255]), ("black", [0, 0, 0]),
        ];
        show_check_image(output, "default", paths["red"].as_path())?;
        show_check_image(output, "bench-b", paths["blue"].as_path())?;

        let mut session = 1_u64;
        let mut default_sequence = 0_u64;
        let mut portrait_sequence = 0_u64;
        if !output.set_output_monitor_source(Some("default"), session)? { return Err(anyhow!("initial A source selection was rejected")); }
        check_capture(output, "default", &mut default_sequence, session, [255, 0, 0], 16.0 / 9.0, "A landscape")?;
        session += 1;
        if !output.set_output_monitor_source(Some("bench-b"), session)? { return Err(anyhow!("initial B source selection was rejected")); }
        check_capture(output, "bench-b", &mut portrait_sequence, session, [0, 0, 255], 320.0 / 568.0, "B portrait")?;

        for cycle in 0..10 {
            session += 1;
            if !output.set_output_monitor_source(Some("default"), session)? { return Err(anyhow!("A selection rejected at cycle {cycle}")); }
            check_capture(output, "default", &mut default_sequence, session, [255, 0, 0], 16.0 / 9.0, "A→B→A return")?;
            session += 1;
            if !output.set_output_monitor_source(Some("bench-b"), session)? { return Err(anyhow!("B selection rejected at cycle {cycle}")); }
            check_capture(output, "bench-b", &mut portrait_sequence, session, [0, 0, 255], 320.0 / 568.0, "A→B→A portrait")?;
            if output.set_output_monitor_source(None, session - 1)? {
                return Err(anyhow!("stale source token was accepted at cycle {cycle}"));
            }
            check_capture(output, "bench-b", &mut portrait_sequence, session, [0, 0, 255], 320.0 / 568.0, "stale token kept B active")?;
        }

        for cycle in 0..10 {
            session += 1;
            if !output.set_output_monitor_source(None, session)? { return Err(anyhow!("disable rejected at cycle {cycle}")); }
            if output.output_monitor_frame("default", default_sequence, session - 1).is_ok() {
                return Err(anyhow!("old frame session remained readable after disable at cycle {cycle}"));
            }
            session += 1;
            if !output.set_output_monitor_source(Some("default"), session)? { return Err(anyhow!("re-enable rejected at cycle {cycle}")); }
            check_capture(output, "default", &mut default_sequence, session, [255, 0, 0], 16.0 / 9.0, "disable→re-enable")?;
        }

        let mut color_checks = Vec::new();
        for (name, rgb) in colors {
            output.panic_stop();
            show_check_image(output, "default", paths[name].as_path())?;
            check_capture(output, "default", &mut default_sequence, session, rgb, 16.0 / 9.0, name)?;
            color_checks.push(name);
        }

        output.panic_stop();
        show_check_image(output, "default", paths["red"].as_path())?;
        check_capture(output, "default", &mut default_sequence, session, [255, 0, 0], 16.0 / 9.0, "master fade red baseline")?;
        output.set_overlay_alpha_direct(255);
        check_capture(output, "default", &mut default_sequence, session, [0, 0, 0], 16.0 / 9.0, "master fade black")?;
        output.set_overlay_alpha_direct(0);
        check_capture(output, "default", &mut default_sequence, session, [255, 0, 0], 16.0 / 9.0, "master fade clear")?;

        if !output.toggle_output_ftb("default")? { return Err(anyhow!("FTB enable did not enter blackout")); }
        check_capture(output, "default", &mut default_sequence, session, [0, 0, 0], 16.0 / 9.0, "FTB black")?;
        if output.toggle_output_ftb("default")? { return Err(anyhow!("FTB disable remained in blackout")); }
        check_capture(output, "default", &mut default_sequence, session, [255, 0, 0], 16.0 / 9.0, "FTB clear")?;

        Ok(json!({
            "result": "passed",
            "displayCount": screens.len(),
            "outputMonitor": { "sourceSwitchCycles": 10, "disableReenableCycles": 10, "staleTokensRejected": true },
            "frames": { "maxWidth": 640, "maxHeight": 360, "landscapeAspect": 16.0 / 9.0, "portraitAspect": 320.0 / 568.0 },
            "colorSequence": color_checks,
            "masterFade": true,
            "operatorFadeToBlack": true,
            "limitations": ["Layer opacity/blend/crop/text/timer composition is not covered by these checks."],
        }))
    })();
    let _ = output.set_output_monitor_source(None, u64::MAX);
    output.panic_stop();
    checks
}

fn run_webview_benchmark(
    handle: tauri::AppHandle,
    output: Arc<OutputEngine>,
    frontend: Arc<std::sync::Mutex<FrontendAggregate>>,
    seconds: u64,
    layers: u32,
    mode: String,
    is_image: bool,
    reopen_hidden_window: bool,
) -> Result<()> {
    thread::sleep(Duration::from_millis(500));
    let open_started = Instant::now();
    loop {
        let _ = handle.emit("output-monitor-opened", ());
        thread::sleep(Duration::from_millis(250));
        let has_report = frontend.lock().map(|metrics| metrics.samples > 0).unwrap_or(false);
        if has_report || open_started.elapsed() >= Duration::from_secs(2) { break; }
    }

    let started = Instant::now();
    let run_for = Duration::from_secs(seconds);
    let (_, ram_start) = process_metrics();
    let video_start = video_runtime_summary(&output);
    let mut cpu_total = 0.0;
    let mut cpu_max = 0.0_f64;
    let mut cpu_samples = 0_u64;
    let mut ram_max = 0_u64;
    let (drop_start, delay_start) = video_runtime_drop_counts(&output);
    let mut dropped_observed_max = drop_start;
    let mut delayed_observed_max = delay_start;
    let mut window_reopens = 0_u64;
    while started.elapsed() < run_for {
        let (cpu, ram) = process_metrics();
        if let Some(value) = cpu { cpu_total += value; cpu_max = cpu_max.max(value); cpu_samples += 1; }
        if let Some(value) = ram { ram_max = ram_max.max(value); }
        let (dropped, delayed) = video_runtime_drop_counts(&output);
        dropped_observed_max = dropped_observed_max.max(dropped);
        delayed_observed_max = delayed_observed_max.max(delayed);
        if reopen_hidden_window {
            let window = handle.get_webview_window("output-monitor")
                .ok_or_else(|| anyhow!("configured output-monitor window disappeared"))?;
            if !window.is_visible().unwrap_or(false) {
                window.show().map_err(|error| anyhow!("show output-monitor window: {error}"))?;
                handle.emit("output-monitor-opened", ())?;
                window_reopens = window_reopens.saturating_add(1);
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
    let (_, ram_end) = process_metrics();
    let frontend_summary = frontend.lock().map(|metrics| metrics.summary()).unwrap_or_else(|_| json!({"error":"metrics lock poisoned"}));
    let video_end = video_runtime_summary(&output);
    let capture = output.output_monitor_diagnostics(Some("default")).ok();
    let request_response_delay_ms = env::var("QLISA_MONITOR_BENCH_REQUEST_DELAY_MS")
        .ok().and_then(|value| value.parse::<u64>().ok()).unwrap_or(0).min(1000);
    output.panic_stop();
    println!("{}", json!({
        "mode": mode, "durationSeconds": seconds, "layers": layers, "imageInput": is_image,
        "processCpuAveragePercent": (cpu_samples > 0).then(|| cpu_total / cpu_samples as f64),
        "processCpuMaximumPercent": (cpu_samples > 0).then_some(cpu_max),
        "workingSetStartBytes": ram_start, "workingSetEndBytes": ram_end,
        "workingSetMaximumBytes": (ram_max > 0).then_some(ram_max),
        "videoRuntimeStart": video_start, "videoRuntimeEnd": video_end,
        "videoRuntimeObservedMaximum": {
            "droppedFrames": dropped_observed_max,
            "delayedFrames": delayed_observed_max,
        },
        "outputMonitorWindowReopens": window_reopens,
        "requestResponseDelayMs": request_response_delay_ms,
        "capture": capture,
        "frontend": frontend_summary, "includesIpcOrWebView": true,
    }));
    handle.exit(0);
    Ok(())
}

#[cfg(not(windows))]
fn process_metrics() -> (Option<f64>, Option<u64>) {
    (None, None)
}

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let video = args.next().context("usage: output_monitor_bench <video> [seconds] [off|4|30]")?;
    let seconds = args.next().map(|value| value.parse()).transpose()?.unwrap_or(30_u64);
    let mode = args.next().unwrap_or_else(|| "30".into());
    let layers = args.next().map(|value| value.parse()).transpose()?.unwrap_or(1_u32).clamp(1, 3);
    let fifth_arg = args.next();
    let webview_checks = fifth_arg.as_deref() == Some("webview-checks");
    let webview = fifth_arg.as_deref().is_some_and(|arg| matches!(arg, "webview" | "webview-checks"));
    let checks = fifth_arg.as_deref() == Some("checks");
    let capture_fps = match mode.as_str() {
        "off" => None,
        "4" => Some(4_u32),
        "30" => Some(30_u32),
        _ => return Err(anyhow!("mode must be off, 4, or 30")),
    };
    let video_path = Path::new(&video);
    if !video_path.is_file() {
        return Err(anyhow!("video file does not exist: {}", video_path.display()));
    }
    let video_path = video_path.to_path_buf();
    let is_image = is_image_path(&video_path);

    // Keep Tauri's profile writes out of the operator's real Preferences.
    let isolated_profile = env::temp_dir().join(format!("QlisaOutputMonitorBench-{}", std::process::id()));
    std::fs::create_dir_all(&isolated_profile)?;
    env::set_var("APPDATA", &isolated_profile);
    let monitor_capture_fps = if checks || webview_checks { Some(30) } else { capture_fps };
    if let Some(fps) = monitor_capture_fps {
        env::set_var("QLISA_MONITOR_CAPTURE_FPS", fps.to_string());
    } else {
        env::remove_var("QLISA_MONITOR_CAPTURE_FPS");
    }
    let no_overlay = env::var_os("QLISA_MONITOR_BENCH_NO_OVERLAY").is_some();

    let mut context = tauri::generate_context!();
    if webview {
        context.config_mut().app.windows.retain(|window| window.label == "output-monitor");
        if let Some(window) = context.config_mut().app.windows.first_mut() {
            window.visible = true;
        } else {
            return Err(anyhow!("configured output-monitor window is missing"));
        }
    } else {
        context.config_mut().app.windows.clear();
    }
    let mut builder = tauri::Builder::default();
    if webview {
        let webview_video_path = video_path.clone();
        let webview_fixture_dir = isolated_profile.join("check-fixtures");
        builder = builder
            .invoke_handler(tauri::generate_handler![
                list_output_monitor_sources,
                set_output_monitor_source,
                get_output_monitor_frame,
                report_output_monitor_frontend_diagnostics,
            ])
            .setup(move |app| {
                let audio = AudioEngine::new_silent(&MachineAudioConfig::default());
                let output = Arc::new(OutputEngine::new(audio, app.handle().clone())?);
                let sources = if webview_checks {
                    output.configure_outputs(&check_destinations("BenchA", "BenchB"), "default")?;
                    let paths = create_check_fixtures(&webview_fixture_dir)?;
                    show_check_image(&output, "default", paths["red"].as_path())?;
                    show_check_image(&output, "bench-b", paths["blue"].as_path())?;
                    vec![("default".into(), "BenchA".into()), ("bench-b".into(), "BenchB".into())]
                } else {
                    configure_output(&output, &webview_video_path, layers, is_image)?;
                    if !no_overlay { output.set_output_timer(Some("Output Monitor Bench")); }
                    if monitor_capture_fps.is_some() { vec![("default".into(), "Main".into())] } else { Vec::new() }
                };
                if monitor_capture_fps.is_some() { output.set_output_monitor_source(Some("default"), 1)?; }
                app.manage(BenchState { output, frontend: Arc::default(), sources });
                Ok(())
            });
    }
    let app = builder.build(context)?;
    if webview {
        let handle = app.handle().clone();
        let worker_handle = handle.clone();
        let run_mode = if webview_checks { "30".to_string() } else { mode };
        let worker = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    if let Some(state) = worker_handle.try_state::<BenchState>() {
                        return run_webview_benchmark(
                            worker_handle.clone(),
                            state.output.clone(),
                            state.frontend.clone(),
                            seconds,
                            layers,
                            run_mode.clone(),
                            is_image,
                            webview_checks,
                        );
                    }
                    if Instant::now() >= deadline {
                        return Err(anyhow!("Tauri setup did not register benchmark state within 10 seconds"));
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }))
            .unwrap_or_else(|_| Err(anyhow!("webview benchmark worker panicked")));
            if result.is_err() {
                worker_handle.exit(1);
            }
            result
        });
        app.run(|_, _| {});
        worker.join().map_err(|_| anyhow!("webview benchmark worker panicked"))??;
        return Ok(());
    }
    let app_handle = app.handle().clone();
    let audio = AudioEngine::new_silent(&MachineAudioConfig::default());
    let output = Arc::new(OutputEngine::new(audio, app_handle)?);
    if checks {
        let report = run_integration_checks(&output, &isolated_profile.join("check-fixtures"))?;
        println!("{}", json!({
            "mode": "checks",
            "captureFps": monitor_capture_fps,
            "checks": report,
        }));
        return Ok(());
    }
    configure_output(&output, &video_path, layers, is_image)?;
    if !no_overlay { output.set_output_timer(Some("Output Monitor Bench")); }
    if capture_fps.is_some() {
        output.set_output_monitor_source(Some("default"), 1)?;
    }

    eprintln!("mode={mode}, seconds={seconds}, layers={layers}, profile={}", isolated_profile.display());
    let started = Instant::now();
    let mut next_sample = started;
    let mut next_poll = started;
    let mut sequence = 0_u64;
    let mut frame_packets = 0_u64;
    let mut black_packets = 0_u64;
    let mut unchanged_packets = 0_u64;
    let mut no_frame_packets = 0_u64;
    let mut packet_bytes = 0_u64;
    let mut prepare_total_us = 0_u64;
    let mut prepare_max_us = 0_u64;
    let mut last_frame_age_ms = None;
    let mut age_total_ms = 0.0_f64;
    let mut age_max_ms = 0.0_f64;
    let mut center_bgra = None;
    let mut cpu_total = 0.0;
    let mut cpu_samples = 0_u64;
    let mut cpu_max = 0.0_f64;
    let mut ram_max = 0_u64;
    let (_, ram_start) = process_metrics();
    let video_diagnostics_start = video_runtime_summary(&output);
    let run_for = Duration::from_secs(seconds);
    let period = Duration::from_nanos(1_000_000_000 / capture_fps.unwrap_or(30) as u64);
    while started.elapsed() < Duration::from_secs(seconds) {
        if capture_fps.is_some() {
            match output.output_monitor_frame("default", sequence, 1)? {
                inkue_lib::engine::output_engine::OutputMonitorFrameRead::Frame {
                    sequence: next_sequence,
                    session,
                    captured_at_us,
                    frame,
                } => {
                    let black = (0..frame.height as usize).all(|row| {
                        let row_start = row * frame.stride as usize;
                        (0..frame.width as usize).all(|column| {
                            let offset = row_start + column * 4;
                            frame.data[offset..offset + 3] == [0, 0, 0]
                        })
                    });
                    let center_offset = (frame.height as usize / 2) * frame.stride as usize
                        + (frame.width as usize / 2) * 4;
                    center_bgra.get_or_insert([
                        frame.data[center_offset],
                        frame.data[center_offset + 1],
                        frame.data[center_offset + 2],
                    ]);
                    let status = if black { 3 } else { 1 };
                    let prepare_started = Instant::now();
                    let payload = if black { &[][..] } else { &frame.data[..] };
                    let packet = encode_output_monitor_packet(
                        status,
                        frame.width,
                        frame.height,
                        frame.stride,
                        next_sequence,
                        session,
                        captured_at_us,
                        0,
                        payload,
                    ).map_err(anyhow::Error::msg)?;
                    let prepare_us = prepare_started.elapsed().as_micros();
                    let payload_len = payload.len();
                    debug_assert_eq!(packet.len(), 64 + payload_len);
                    sequence = next_sequence;
                    if black { black_packets += 1; } else { frame_packets += 1; }
                    packet_bytes += packet.len() as u64;
                    prepare_total_us += prepare_us as u64;
                    prepare_max_us = prepare_max_us.max(prepare_us as u64);
                    let age_ms = unix_age_ms(captured_at_us);
                    last_frame_age_ms = Some(age_ms);
                    age_total_ms += age_ms;
                    age_max_ms = age_max_ms.max(age_ms);
                }
                inkue_lib::engine::output_engine::OutputMonitorFrameRead::Unchanged { sequence: next_sequence, .. } => {
                    sequence = next_sequence;
                    unchanged_packets += 1;
                }
                inkue_lib::engine::output_engine::OutputMonitorFrameRead::NoFrame => no_frame_packets += 1,
            }
        }
        if Instant::now() >= next_sample {
            let (cpu, ram) = process_metrics();
            if let Some(cpu) = cpu {
                cpu_total += cpu;
                cpu_samples += 1;
                cpu_max = cpu_max.max(cpu);
            }
            if let Some(ram) = ram {
                ram_max = ram_max.max(ram);
            }
            next_sample = Instant::now() + Duration::from_secs(1);
        }
        next_poll += period;
        let now = Instant::now();
        if next_poll < now {
            next_poll = now;
        }
        thread::sleep(next_poll.saturating_duration_since(Instant::now()));
    }
    let diagnostics = output.output_monitor_diagnostics(Some("default")).ok();
    let video_diagnostics_end = video_runtime_summary(&output);
    let (_, ram_end) = process_metrics();
    let run_seconds = started.elapsed().as_secs_f64().min(run_for.as_secs_f64());
    println!("{}", serde_json::json!({
        "mode": mode,
        "durationSeconds": seconds,
        "layers": layers,
        "imageInput": is_image,
        "sequence": sequence,
        "framePackets": frame_packets,
        "blackPackets": black_packets,
        "unchangedPackets": unchanged_packets,
        "noFramePackets": no_frame_packets,
        "observedFps": (frame_packets + black_packets) as f64 / run_seconds.max(f64::EPSILON),
        "packetBytes": packet_bytes,
        "packetPrepareAverageUs": if frame_packets > 0 { Some(prepare_total_us as f64 / frame_packets as f64) } else { None },
        "packetPrepareMaximumUs": if frame_packets > 0 { Some(prepare_max_us) } else { None },
        "frameAgeLastMs": last_frame_age_ms,
        "frameAgeAverageMs": if frame_packets + black_packets > 0 { Some(age_total_ms / (frame_packets + black_packets) as f64) } else { None },
        "frameAgeMaximumMs": if frame_packets + black_packets > 0 { Some(age_max_ms) } else { None },
        "centerBgra": center_bgra,
        "processCpuAveragePercent": if cpu_samples > 0 { Some(cpu_total / cpu_samples as f64) } else { None },
        "processCpuMaximumPercent": if cpu_samples > 0 { Some(cpu_max) } else { None },
        "workingSetStartBytes": ram_start,
        "workingSetEndBytes": ram_end,
        "workingSetMaximumBytes": if ram_max > 0 { Some(ram_max) } else { None },
        "videoRuntimeStart": video_diagnostics_start,
        "videoRuntimeEnd": video_diagnostics_end,
        "capture": diagnostics,
        "includesIpcOrWebView": false,
    }));
    Ok(())
}

fn unix_age_ms(captured_at_us: u64) -> f64 {
    let now_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    now_us.saturating_sub(captured_at_us) as f64 / 1000.0
}
