//! Media thumbnail generation for the inspector previews.
//!
//! A throwaway libmpv context with `vo=image` decodes one frame headlessly and
//! writes it as a JPEG — one code path covers video **and** image files (every
//! format the playback engine accepts previews identically). Thumbnails are
//! cached on disk keyed by path + size + mtime, so a file is only decoded
//! once until it changes.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use base64::Engine as _;

use super::mpv_sys::{MpvLib, MPV_EVENT_END_FILE, MPV_EVENT_SHUTDOWN};
use super::media_cache::CacheScope;

/// Thumbnail width in pixels (height follows the aspect ratio).
const THUMB_WIDTH: u32 = 400;
/// Raw-file fallback cap: images the browser can show natively (e.g. SVG,
/// which mpv cannot rasterise without librsvg) are sent as-is below this size.
const RAW_FALLBACK_MAX_BYTES: u64 = 10 * 1024 * 1024;

fn legacy_cache_scope() -> CacheScope {
    CacheScope::at(cache_dir(), None)
}

fn cache_dir() -> PathBuf {
    crate::machine_config::config_base_dir()
        .join("Inkue")
        .join("thumbnails")
}

fn jpeg_data_url(bytes: &[u8]) -> String {
    format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// Return a `data:` URL thumbnail for a media file, generating and caching it
/// on first request.
///
/// `seek_into` picks a representative frame ~15 % into the file (videos —
/// frame 0 is often black); still images always use their single frame.
pub fn media_thumbnail(lib: &MpvLib, path: &Path, seek_into: bool) -> Result<String> {
    media_thumbnail_in_scope(Some(lib), path, seek_into, &legacy_cache_scope())
}

pub fn media_thumbnail_in_scope(lib: Option<&MpvLib>, path: &Path, seek_into: bool, scope: &CacheScope) -> Result<String> {
    let params = format!("seek_into={seek_into};width={THUMB_WIDTH}");
    let cached = scope.get_or_generate_validated(path, "thumbnail", &params, "jpeg", is_jpeg, || {
        lib.and_then(|lib| render_one_frame(lib, path, seek_into).ok())
    });
    match cached {
        Some(bytes) => Ok(jpeg_data_url(&bytes)),
        None => raw_image_fallback(path).ok_or_else(|| anyhow!("could not decode thumbnail for {}", path.display())),
    }
}

fn raw_image_fallback(path: &Path) -> Option<String> {
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => return None,
    };
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > RAW_FALLBACK_MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

/// Filmstrip for the video trimmer: `tiles` frames evenly spread across the
/// file, as JPEG `data:` URLs in playback order. Disk-cached like thumbnails.
///
/// `tile_width` sizes each frame (the trimmer strip uses small tiles, the
/// drag scrub-preview a denser strip of larger ones).
pub fn video_filmstrip(
    lib: &MpvLib,
    path: &Path,
    tiles: usize,
    tile_width: u32,
) -> Result<Vec<String>> {
    video_filmstrip_in_scope(Some(lib), path, tiles, tile_width, &legacy_cache_scope())
}

pub fn video_filmstrip_in_scope(
    lib: Option<&MpvLib>,
    path: &Path,
    tiles: usize,
    tile_width: u32,
    scope: &CacheScope,
) -> Result<Vec<String>> {
    let tiles = tiles.clamp(2, 48);
    let tile_width = tile_width.clamp(80, 640);
    let params = format!("tiles={tiles};width={tile_width};range=full");
    let payload = scope.get_or_generate_validated(
        path, "video-filmstrip", &params, "jpeg-list-v1",
        |b| decode_jpeg_list(b, tiles).is_some(),
        || {
            let lib = lib?;
            let duration = super::output_engine::OutputEngine::probe_duration(lib, path)?;
            let step_secs = (duration.as_secs_f64() / tiles as f64).max(0.1);
            render_strip(lib, path, tiles, tile_width, None, None, step_secs)
                .and_then(|frames| encode_jpeg_list(&frames)).ok()
        },
    );
    let bytes = payload.ok_or_else(|| anyhow!("could not render filmstrip for {}", path.display()))?;
    let frames = decode_jpeg_list(&bytes, tiles).ok_or_else(|| anyhow!("invalid filmstrip cache"))?;
    Ok(frames.iter().map(|b| jpeg_data_url(b)).collect())
}

/// Decode one frame of `path` into a JPEG via a throwaway `vo=image` context.
/// Filmstrip over a time range, for the zoomed clip editor: `tiles` frames
/// evenly spread across `[start_s, end_s]`. Requests use millisecond precision
/// so a trim or zoom change cannot reuse frames from another range.
pub fn video_filmstrip_range(
    lib: &MpvLib,
    path: &Path,
    start_s: f64,
    end_s: f64,
    tiles: usize,
    tile_width: u32,
) -> Result<Vec<String>> {
    video_filmstrip_range_in_scope(Some(lib), path, start_s, end_s, tiles, tile_width, &legacy_cache_scope())
}

pub fn video_filmstrip_range_in_scope(
    lib: Option<&MpvLib>,
    path: &Path,
    start_s: f64,
    end_s: f64,
    tiles: usize,
    tile_width: u32,
    scope: &CacheScope,
) -> Result<Vec<String>> {
    let tiles = tiles.clamp(2, 24);
    let tile_width = tile_width.clamp(80, 640);
    if end_s <= start_s || start_s < 0.0 || !start_s.is_finite() || !end_s.is_finite() {
        return Err(anyhow!("invalid filmstrip range {start_s}..{end_s}"));
    }
    let params = range_cache_params(start_s, end_s, tiles, tile_width);
    let payload = scope.get_or_generate_validated(
        path, "video-filmstrip", &params, "jpeg-list-v1",
        |b| decode_jpeg_list(b, tiles).is_some(),
        || {
            let lib = lib?;
            let step_secs = ((end_s - start_s) / tiles as f64).max(0.001);
            render_strip(lib, path, tiles, tile_width, Some(start_s), Some(end_s), step_secs)
                .and_then(|frames| encode_jpeg_list(&frames)).ok()
        },
    ).ok_or_else(|| anyhow!("could not render filmstrip for {}", path.display()))?;
    let frames = decode_jpeg_list(&payload, tiles).ok_or_else(|| anyhow!("invalid filmstrip cache"))?;
    Ok(frames.iter().map(|b| jpeg_data_url(b)).collect())
}

fn render_strip(lib: &MpvLib, path: &Path, tiles: usize, width: u32, start: Option<f64>, end: Option<f64>, step: f64) -> Result<Vec<Vec<u8>>> {
    let out_dir = std::env::temp_dir().join(format!("inkue-strip-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&out_dir)?;
    let frames = tiles.to_string();
    let scale = format!("scale={width}:-2");
    let start_text = start.map(|v| format!("{v:.3}"));
    let end_text = end.map(|v| format!("{v:.3}"));
    let step_text = format!("{step:.3}");
    let mut options = vec![("frames", frames.as_str()), ("sstep", step_text.as_str()), ("vf", scale.as_str())];
    if let Some(value) = start_text.as_deref() { options.push(("start", value)); }
    if let Some(value) = end_text.as_deref() { options.push(("end", value)); }
    let result = render_frames_into(lib, path, &out_dir, &options);
    let _ = std::fs::remove_dir_all(&out_dir);
    let frames = result?;
    if frames.is_empty() { return Err(anyhow!("mpv produced no frames for {}", path.display())); }
    Ok(frames)
}

fn encode_jpeg_list(frames: &[Vec<u8>]) -> Result<Vec<u8>> {
    if frames.is_empty() || frames.len() > 48 { return Err(anyhow!("invalid filmstrip frame count")); }
    let mut payload = Vec::new();
    payload.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    for frame in frames {
        if !is_jpeg(frame) { return Err(anyhow!("mpv produced an invalid JPEG frame")); }
        payload.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        payload.extend_from_slice(frame);
    }
    Ok(payload)
}

fn decode_jpeg_list(payload: &[u8], expected: usize) -> Option<Vec<Vec<u8>>> {
    if payload.len() < 4 { return None; }
    let count = u32::from_le_bytes(payload.get(..4)?.try_into().ok()?) as usize;
    // Short files can end before the requested frame count. Preserve the old
    // behavior and cache any non-empty prefix that mpv successfully wrote.
    if count == 0 || count > expected || count > 48 { return None; }
    let mut offset = 4usize;
    let mut frames = Vec::with_capacity(count);
    for _ in 0..count {
        let len = u32::from_le_bytes(payload.get(offset..offset.checked_add(4)?)?.try_into().ok()?) as usize;
        offset += 4;
        let end = offset.checked_add(len)?;
        let frame = payload.get(offset..end)?.to_vec();
        if !is_jpeg(&frame) { return None; }
        frames.push(frame);
        offset = end;
    }
    (offset == payload.len()).then_some(frames)
}

fn is_jpeg(bytes: &[u8]) -> bool { bytes.len() >= 4 && bytes.starts_with(&[0xff, 0xd8]) && bytes.ends_with(&[0xff, 0xd9]) }

fn range_cache_params(start_s: f64, end_s: f64, tiles: usize, tile_width: u32) -> String {
    let start_ms = (start_s * 1000.0).round() as i64;
    let end_ms = (end_s * 1000.0).round() as i64;
    format!("tiles={tiles};width={tile_width};range_ms={start_ms}:{end_ms}")
}

fn render_one_frame(lib: &MpvLib, path: &Path, seek_into: bool) -> Result<Vec<u8>> {
    // Unique temp dir per request: vo=image names files 00000001.jpg, so
    // concurrent generations must not share an outdir.
    let out_dir = std::env::temp_dir().join(format!("inkue-thumb-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&out_dir)?;
    let result = render_one_frame_into(lib, path, seek_into, &out_dir);
    let _ = std::fs::remove_dir_all(&out_dir);
    result
}

fn render_one_frame_into(
    lib: &MpvLib,
    path: &Path,
    seek_into: bool,
    out_dir: &Path,
) -> Result<Vec<u8>> {
    let scale = format!("scale={THUMB_WIDTH}:-2");
    let mut opts: Vec<(&str, &str)> = vec![("frames", "1"), ("vf", &scale)];
    if seek_into {
        opts.push(("start", "15%"));
    }
    render_frames_into(lib, path, out_dir, &opts)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("mpv produced no frame for {}", path.display()))
}

/// Run a throwaway `vo=image` mpv over `path` with the given extra options and
/// return the produced JPEGs in playback order.
fn render_frames_into(
    lib: &MpvLib,
    path: &Path,
    out_dir: &Path,
    extra_opts: &[(&str, &str)],
) -> Result<Vec<Vec<u8>>> {
    let cs = |s: &str| CString::new(s).expect("no interior NUL in literal");
    let opt = |ctx: *mut std::ffi::c_void, k: &str, v: &str| {
        let (k, v) = (cs(k), cs(v));
        unsafe { (lib.mpv_set_option_string)(ctx, k.as_ptr(), v.as_ptr()) };
    };

    unsafe {
        let ctx = (lib.mpv_create)();
        if ctx.is_null() {
            return Err(anyhow!("mpv_create() returned null for thumbnail"));
        }

        opt(ctx, "vo", "image");
        opt(ctx, "vo-image-format", "jpg");
        opt(
            ctx,
            "vo-image-outdir",
            &out_dir.to_string_lossy().replace('\\', "/"),
        );
        opt(ctx, "audio", "no");
        opt(ctx, "hwdec", "no");
        opt(ctx, "untimed", "yes");
        for (k, v) in extra_opts {
            opt(ctx, k, v);
        }

        if (lib.mpv_initialize)(ctx) < 0 {
            (lib.mpv_terminate_destroy)(ctx);
            return Err(anyhow!("mpv_initialize() failed for thumbnail"));
        }

        let path_str = path.to_string_lossy().replace('\\', "/");
        let path_cstr = match CString::new(path_str.as_str()) {
            Ok(c) => c,
            Err(_) => {
                (lib.mpv_terminate_destroy)(ctx);
                return Err(anyhow!("path contains NUL byte"));
            }
        };
        let cmd = cs("loadfile");
        let replace = cs("replace");
        let args: [*const std::ffi::c_char; 4] = [
            cmd.as_ptr(),
            path_cstr.as_ptr(),
            replace.as_ptr(),
            std::ptr::null(),
        ];
        (lib.mpv_command)(ctx, args.as_ptr());

        // `frames=N` plays exactly N frames then ends the file — the JPEGs are
        // written before END_FILE fires. Generous deadline: a filmstrip does
        // several keyframe seeks through the file.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = (lib.mpv_wait_event)(ctx, remaining.as_secs_f64().max(0.01));
            if event.is_null() {
                break;
            }
            let id = (*event).event_id;
            if id == MPV_EVENT_END_FILE || id == MPV_EVENT_SHUTDOWN {
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        (lib.mpv_terminate_destroy)(ctx);
    }

    // vo=image names files 00000001.jpg, 00000002.jpg, … — lexicographic
    // order is playback order.
    let mut produced: Vec<PathBuf> = std::fs::read_dir(out_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "jpg").unwrap_or(false))
        .collect();
    produced.sort();
    produced
        .iter()
        .map(|p| std::fs::read(p).map_err(Into::into))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_revision_is_stable_for_unchanged_file() {
        let dir = std::env::temp_dir().join("inkue-thumb-test-stable");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("a.bin");
        std::fs::write(&f, b"hello").unwrap();
        assert_eq!(super::super::media_cache::media_source_revision(&f, None), super::super::media_cache::media_source_revision(&f, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_revision_changes_when_size_changes() {
        let dir = std::env::temp_dir().join("inkue-thumb-test-size");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("a.bin");
        std::fs::write(&f, b"hello").unwrap();
        let k1 = super::super::media_cache::media_source_revision(&f, None);
        std::fs::write(&f, b"hello world, longer content").unwrap();
        let k2 = super::super::media_cache::media_source_revision(&f, None);
        assert_ne!(k1, k2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_identity_differs_per_path() {
        let dir = std::env::temp_dir().join("inkue-thumb-test-path");
        let _ = std::fs::create_dir_all(&dir);
        let (fa, fb) = (dir.join("a.bin"), dir.join("b.bin"));
        std::fs::write(&fa, b"same").unwrap();
        std::fs::write(&fb, b"same").unwrap();
        assert_ne!(super::super::media_cache::media_source_revision(&fa, None), super::super::media_cache::media_source_revision(&fb, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_revision_none_for_missing_file() {
        assert_eq!(super::super::media_cache::media_source_revision(Path::new("Z:/definitely/not/here.mp4"), None), None);
    }

    #[test]
    fn jpeg_data_url_encodes_base64() {
        assert_eq!(jpeg_data_url(b"\xff\xd8"), "data:image/jpeg;base64,/9g=");
    }

    #[test]
    fn raw_fallback_rejects_unknown_extensions() {
        assert!(raw_image_fallback(Path::new("C:/x/clip.mp4")).is_none());
        assert!(raw_image_fallback(Path::new("C:/x/track.wav")).is_none());
    }

    #[test]
    fn short_filmstrip_prefix_is_valid_and_ranges_do_not_collide() {
        let frame = vec![0xff, 0xd8, 1, 0xff, 0xd9];
        let packed = encode_jpeg_list(std::slice::from_ref(&frame)).unwrap();
        assert_eq!(decode_jpeg_list(&packed, 12), Some(vec![frame]));
        assert_eq!(range_cache_params(1.0001, 2.0, 8, 320), range_cache_params(1.0002, 2.0, 8, 320));
        assert_ne!(range_cache_params(1.0, 2.0, 8, 320), range_cache_params(1.1, 2.0, 8, 320));
    }
}

#[cfg(test)]
mod preview_cache_integration_smoke {
    use super::*;
    use crate::engine::mpv_sys::MpvLib;
    use std::process::Command;

    fn make_video(ffmpeg: &Path, path: &Path, half_duration: &str, total_frames: &str) {
        let red_source = format!("color=c=red:s=320x180:r=10:d={half_duration}");
        let blue_source = format!("color=c=blue:s=320x180:r=10:d={half_duration}");
        let status = Command::new(ffmpeg)
            .args([
                "-hide_banner", "-loglevel", "error", "-y",
                "-f", "lavfi", "-i", red_source.as_str(),
                "-f", "lavfi", "-i", blue_source.as_str(),
                "-filter_complex", "[0:v][1:v]concat=n=2:v=1:a=0,format=yuv420p",
                "-an", "-frames:v", total_frames, "-c:v", "libx264", "-preset", "ultrafast", "-g", "10",
                "-keyint_min", "10", "-sc_threshold", "0", "-movflags", "+faststart",
            ])
            .arg(path)
            .status()
            .expect("start the installed ffmpeg runtime");
        assert!(status.success(), "ffmpeg failed to create {}", path.display());
    }

    #[test]
    fn project_preview_cache_survives_scope_reconstruction_with_runtime() {
        if std::env::var_os("QLISA_CACHE_SMOKE_RUNTIME").is_none() {
            eprintln!("skipping preview cache integration smoke; set QLISA_CACHE_SMOKE_RUNTIME=1 to enable it");
            return;
        }

        let runtime = crate::media_runtime::runtime_dir();
        let ffmpeg = runtime.join("ffmpeg.exe");
        let libmpv = runtime.join("libmpv-2.dll");
        assert!(ffmpeg.is_file(), "missing installed ffmpeg: {}", ffmpeg.display());
        assert!(libmpv.is_file(), "missing installed libmpv: {}", libmpv.display());

        let root = std::env::temp_dir().join(format!("qlisa-preview-cache-smoke-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let project = root.join("smoke.qlisa");
        std::fs::write(&project, "{}").unwrap();
        let short = root.join("short.mp4");
        let long = root.join("two-colors.mp4");
        make_video(&ffmpeg, &short, "0.15", "3");
        make_video(&ffmpeg, &long, "1.2", "24");

        let lib = MpvLib::load().expect("load installed libmpv runtime");
        let short_duration = super::super::output_engine::OutputEngine::probe_duration(&lib, &short)
            .expect("probe generated short video").as_secs_f64();
        let long_duration = super::super::output_engine::OutputEngine::probe_duration(&lib, &long)
            .expect("probe generated long video").as_secs_f64();
        assert!((0.25..=0.4).contains(&short_duration), "short video duration was {short_duration}s");
        assert!(long_duration > 2.0, "long video duration was {long_duration}s");
        let first_scope = CacheScope::for_project(&project);
        let short_thumb = media_thumbnail_in_scope(Some(&lib), &short, false, &first_scope).unwrap();
        let short_full = video_filmstrip_in_scope(Some(&lib), &short, 12, 160, &first_scope).unwrap();
        let short_range = video_filmstrip_range_in_scope(Some(&lib), &short, 0.2, 0.3, 12, 160, &first_scope).unwrap();
        let thumb_zero = media_thumbnail_in_scope(Some(&lib), &long, false, &first_scope).unwrap();
        let thumb_into = media_thumbnail_in_scope(Some(&lib), &long, true, &first_scope).unwrap();
        let full = video_filmstrip_in_scope(Some(&lib), &long, 8, 160, &first_scope).unwrap();
        let red_range = video_filmstrip_range_in_scope(Some(&lib), &long, 0.0, 1.1, 4, 160, &first_scope).unwrap();
        let blue_range = video_filmstrip_range_in_scope(Some(&lib), &long, 1.2, 2.3, 4, 160, &first_scope).unwrap();

        for data in [&short_thumb, &thumb_zero, &thumb_into] {
            assert!(data.starts_with("data:image/jpeg;base64,") && data.len() > 64);
        }
        assert!((1..=12).contains(&short_full.len()), "short full filmstrip had {} frames", short_full.len());
        assert!((1..=12).contains(&short_range.len()), "short range filmstrip had {} frames", short_range.len());
        assert!(!full.is_empty() && full.iter().all(|frame| frame.len() > 64));
        assert!(!red_range.is_empty() && !blue_range.is_empty());
        assert_ne!(red_range[0], blue_range[0], "separate red and blue source ranges should decode distinct frames");
        assert_ne!(range_cache_params(0.0, 1.1, 4, 160), range_cache_params(1.2, 2.3, 4, 160));

        // Reconstruct the saved-project scope and prove every generated asset
        // is a persistent cache hit: without libmpv, a miss cannot decode MP4.
        let reopened_scope = CacheScope::for_project(&project);
        assert_eq!(media_thumbnail_in_scope(None, &short, false, &reopened_scope).unwrap(), short_thumb);
        assert_eq!(video_filmstrip_in_scope(None, &short, 12, 160, &reopened_scope).unwrap(), short_full);
        assert_eq!(video_filmstrip_range_in_scope(None, &short, 0.2, 0.3, 12, 160, &reopened_scope).unwrap(), short_range);
        assert_eq!(media_thumbnail_in_scope(None, &long, false, &reopened_scope).unwrap(), thumb_zero);
        assert_eq!(media_thumbnail_in_scope(None, &long, true, &reopened_scope).unwrap(), thumb_into);
        assert_eq!(video_filmstrip_in_scope(None, &long, 8, 160, &reopened_scope).unwrap(), full);
        assert_eq!(video_filmstrip_range_in_scope(None, &long, 0.0, 1.1, 4, 160, &reopened_scope).unwrap(), red_range);
        assert_eq!(video_filmstrip_range_in_scope(None, &long, 1.2, 2.3, 4, 160, &reopened_scope).unwrap(), blue_range);
        assert!(std::fs::read_dir(reopened_scope.root()).unwrap().count() >= 6);

        drop(lib);
        std::fs::remove_dir_all(root).unwrap();
    }
}
