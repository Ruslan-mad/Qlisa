//! Pinned, per-user media runtime bootstrap for Windows.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::{AppHandle, Emitter, Manager};

const MANIFEST: &str = include_str!("../../scripts/runtime-manifest.json");
const FULL_INSTALLER_MANIFEST: &str = include_str!("../../scripts/full-installer-manifest.json");
const PENDING: &str = ".reinstall-pending";
static PREPARE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRuntimeStatus {
    pub ffmpeg: bool,
    pub ffprobe: bool,
    pub libmpv: bool,
    pub ready: bool,
    pub version: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct MediaRuntimeProgress {
    pub phase: String,
    pub percent: Option<u8>,
    pub message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u32,
    components: Vec<serde_json::Value>,
}
fn manifest() -> Result<Manifest, String> {
    serde_json::from_str(MANIFEST).map_err(|e| format!("Invalid embedded runtime manifest: {e}"))
}
fn component(id: &str) -> Result<serde_json::Value, String> {
    let m = manifest()?;
    if m.schema_version != 1 {
        return Err("Unsupported runtime manifest version".into());
    }
    m.components
        .into_iter()
        .find(|v| v["id"] == id)
        .ok_or_else(|| format!("Runtime manifest has no component {id}"))
}
pub fn runtime_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local).join("Qlisa").join("runtime");
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            return PathBuf::from(profile).join("AppData").join("Local").join("Qlisa").join("runtime");
        }
        return std::env::temp_dir().join("Qlisa").join("runtime");
    }
    #[cfg(not(windows))]
    { std::env::temp_dir().join("Qlisa").join("runtime") }
}
fn pending_path() -> PathBuf {
    runtime_dir().join(PENDING)
}
pub fn reinstall_pending() -> bool {
    pending_path().exists()
}
fn expected_hash(id: &str, file: &str) -> Result<String, String> {
    if id == "vulkan-loader" {
        return serde_json::from_str::<serde_json::Value>(FULL_INSTALLER_MANIFEST)
            .map_err(|e| format!("Invalid full-installer manifest: {e}"))?["runtimeFiles"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["fileName"] == file))
            .and_then(|x| x["sha256"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| format!("Full-installer manifest has no hash for {file}"));
    }
    let c = component(id)?;
    if id == "libmpv" {
        return c["sha256"]
            .as_str()
            .map(str::to_owned)
            .ok_or("Manifest missing libmpv SHA256".into());
    }
    c["runtimeFiles"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["fileName"] == file))
        .and_then(|x| x["sha256"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("Manifest missing hash for {file}"))
}
fn ffmpeg_member(root: &str, name: &str) -> Result<String, String> {
    let safe = |s: &str| !s.is_empty() && !s.contains(['/', '\\', ':']) && s != "." && s != "..";
    if !safe(root) || !safe(name) { return Err("Unsafe path in embedded runtime manifest".into()); }
    Ok(format!("{root}/bin/{name}"))
}
fn hash_file(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut b = [0u8; 128 * 1024];
    loop {
        let n = f.read(&mut b).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
pub fn verified_file(path: &Path, id: &str, name: &str) -> bool {
    if reinstall_pending() {
        return false;
    }
    expected_hash(id, name)
        .ok()
        .and_then(|want| {
            hash_file(path)
                .ok()
                .map(|got| got.eq_ignore_ascii_case(&want))
        })
        .unwrap_or(false)
}
pub fn status() -> MediaRuntimeStatus {
    #[cfg(not(windows))]
    { return MediaRuntimeStatus { ffmpeg: true, ffprobe: true, libmpv: true, ready: true, version: Some("system runtime".into()) }; }
    #[cfg(windows)]
    {
    let root = runtime_dir();
    let pending = reinstall_pending();
    let ffmpeg = !pending
        && verified_file(
            &root.join("ffmpeg.exe"),
            "ffmpeg-gyan-full-9.0.2",
            "ffmpeg.exe",
        );
    let ffprobe = !pending
        && verified_file(
            &root.join("ffprobe.exe"),
            "ffmpeg-gyan-full-9.0.2",
            "ffprobe.exe",
        );
    let libmpv = !pending && verified_file(&root.join("libmpv-2.dll"), "libmpv", "libmpv-2.dll");
    let version = libmpv.then(|| component("libmpv").ok()).flatten()
        .and_then(|v| v["version"].as_str().map(str::to_owned));
    MediaRuntimeStatus {
        ffmpeg,
        ffprobe,
        libmpv,
        ready: ffmpeg && ffprobe && libmpv,
        version,
    }
    }
}
fn full_package_ready(media_ready: bool, full_payload_present: bool, cached_loader_verified: bool) -> bool {
    media_ready && (!full_payload_present || cached_loader_verified)
}

fn ensure_seeded_runtime_ready(seeded_full: bool, runtime_ready: bool) -> Result<(), &'static str> {
    if seeded_full && !runtime_ready {
        return Err("Full-installer runtime seed did not produce a ready runtime");
    }
    Ok(())
}
pub fn status_for_app(app: &AppHandle) -> MediaRuntimeStatus {
    let mut current = status();
    #[cfg(windows)]
    {
        let full_payload_present = match full_resource_root(app) {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(error) => {
                log::error!("Could not validate full-installer payload marker: {error}");
                current.ready = false;
                return current;
            }
        };
        let cached_loader_verified = verified_file(&runtime_dir().join("vulkan-1.dll"), "vulkan-loader", "vulkan-1.dll");
        current.ready = full_package_ready(current.ffmpeg && current.ffprobe && current.libmpv, full_payload_present, cached_loader_verified);
    }
    #[cfg(not(windows))]
    let _ = app;
    current
}
fn progress(app: &AppHandle, phase: &str, percent: Option<u8>, message: &str) {
    let _ = app.emit(
        "media-runtime-progress",
        MediaRuntimeProgress {
            phase: phase.into(),
            percent,
            message: message.into(),
        },
    );
}
fn download(url: &str, dest: &Path, app: &AppHandle, label: &str, start: u8, end: u8) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Qlisa runtime bootstrap")
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .get(url)
        .send()
        .map_err(|e| format!("Download failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Download failed: {e}"))?;
    let total = response.content_length();
    let mut file = fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut buf = [0; 128 * 1024];
    let mut got = 0u64;
    loop {
        let n = response
            .read(&mut buf)
            .map_err(|e| format!("Download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        let pct = total.filter(|n| *n > 0).map(|n| {
            let fraction = got.saturating_mul(100) / n;
            (start as u64 + fraction.min(100) * (end - start) as u64 / 100).min(end as u64) as u8
        });
        progress(app, "download", pct, &format!("Скачивание {label}…"));
    }
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}
fn extract_member(archive: &Path, member: &str, dest: &Path) -> Result<(), String> {
    if archive
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("7z"))
    {
        #[cfg(windows)]
        {
            let mut reader =
                sevenz_rust2::ArchiveReader::open(archive, sevenz_rust2::Password::empty())
                    .map_err(|e| format!("Не удалось открыть 7z-архив: {e}"))?;
            let mut found = false;
            reader
                .for_each_entries(|entry, contents| {
                    if entry.name() == member {
                        let mut output = fs::File::create(dest)?;
                        std::io::copy(contents, &mut output)?;
                        found = true;
                        return Ok(false);
                    } else {
                        // Solid 7z blocks must be decoded in order, including
                        // entries that are not the requested member.
                        std::io::copy(contents, &mut std::io::sink())?;
                    }
                    Ok(true)
                })
                .map_err(|e| format!("Ошибка распаковки 7z-архива: {e}"))?;
            if !found {
                return Err(format!("В 7z-архиве нет ожидаемого файла {member}"));
            }
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            return Err("Извлечение 7z доступно только в Windows runtime bootstrap".into());
        }
    }
    let stdout = fs::File::create(dest).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let output = std::process::Command::new("tar.exe")
        .args(["-xOf"])
        .arg(archive)
        .arg("--")
        .arg(member)
        .stdout(std::process::Stdio::from(stdout))
        .output()
        .map_err(|e| format!("Не удалось открыть архив: {e}"))?;
    #[cfg(not(windows))]
    let output = std::process::Command::new("tar")
        .args(["-xOf"])
        .arg(archive)
        .arg("--")
        .arg(member)
        .stdout(std::process::Stdio::from(stdout))
        .output()
        .map_err(|e| format!("Не удалось открыть архив: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "В архиве нет ожидаемого файла {member}: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}
fn verify_hash(path: &Path, wanted: &str, what: &str) -> Result<(), String> {
    let got = hash_file(path)?;
    if !got.eq_ignore_ascii_case(wanted) {
        return Err(format!(
            "SHA-256 не совпадает для {what}: ожидался {wanted}, получен {got}"
        ));
    }
    Ok(())
}
fn replace_file(staged: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<u16> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = destination.as_os_str().encode_wide().chain(Some(0)).collect();
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            return Err(format!("Could not install {}: {}", destination.display(), std::io::Error::last_os_error()));
        }
    }
    #[cfg(not(windows))]
    fs::rename(staged, destination)
        .map_err(|e| format!("Could not install {}: {e}", destination.display()))?;
    Ok(())
}
fn seed_file(source: &Path, destination: &Path, expected: &str, label: &str) -> Result<bool, String> {
    // The full package is a release input. Validate it even when a user's
    // existing cache is valid, so a missing or damaged installer payload is
    // never hidden by that cache or by the network downloader.
    verify_hash(source, expected, &format!("bundled {label}"))?;
    if hash_file(destination)
        .ok()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
    {
        return Ok(false);
    }
    let staged = destination.with_extension(format!("{}.seed", destination.extension().and_then(|e| e.to_str()).unwrap_or("tmp")));
    let result = (|| {
        fs::copy(source, &staged).map_err(|e| format!("Could not stage bundled {label}: {e}"))?;
        verify_hash(&staged, expected, &format!("staged {label}"))?;
        replace_file(&staged, destination)
    })();
    if staged.exists() { fs::remove_file(&staged).ok(); }
    result?;
    Ok(true)
}
fn full_resource_root(app: &AppHandle) -> Result<Option<PathBuf>, String> {
    let root = app.path().resource_dir().map_err(|e| format!("Could not resolve application resources: {e}"))?;
    let payload = root.join("full-runtime");
    let marker = payload.join("full-package.json");
    if !marker.exists() {
        return Ok(None);
    }
    let bundled = fs::read_to_string(&marker).map_err(|e| format!("Could not read full-installer marker: {e}"))?;
    let expected: serde_json::Value = serde_json::from_str(FULL_INSTALLER_MANIFEST)
        .map_err(|e| format!("Invalid embedded full-installer manifest: {e}"))?;
    let actual: serde_json::Value = serde_json::from_str(&bundled)
        .map_err(|e| format!("Invalid bundled full-installer marker: {e}"))?;
    if actual != expected {
        return Err("Bundled full-installer manifest does not match this application build".into());
    }
    Ok(Some(payload))
}
fn seed_full_runtime(app: &AppHandle, base: &Path) -> Result<bool, String> {
    let Some(payload) = full_resource_root(app)? else { return Ok(false); };
    let full: serde_json::Value = serde_json::from_str(FULL_INSTALLER_MANIFEST)
        .map_err(|e| format!("Invalid embedded full-installer manifest: {e}"))?;
    for record in full["runtimeFiles"].as_array().ok_or("Full-installer manifest has no runtimeFiles")? {
        let name = record["fileName"].as_str().ok_or("Full-installer manifest has an unnamed runtime file")?;
        if name.contains(['/', '\\', ':']) || name == "." || name == ".." {
            return Err("Unsafe path in full-installer manifest".into());
        }
        let hash = record["sha256"].as_str().ok_or_else(|| format!("Full-installer manifest has no hash for {name}"))?;
        seed_file(&payload.join(name), &base.join(name), hash, name)?;
    }
    fs::remove_file(pending_path()).ok();
    Ok(true)
}
fn files_to_replace(valid: [bool; 3], force: bool, reinstall_pending: bool) -> [bool; 3] {
    let replace_all = force || reinstall_pending;
    valid.map(|is_valid| replace_all || !is_valid)
}
fn prepare_sync(app: &AppHandle, force: bool) -> Result<MediaRuntimeStatus, String> {
    let _guard = PREPARE_LOCK
        .lock()
        .map_err(|_| "Runtime preparation lock is poisoned".to_string())?;
    let base = runtime_dir();
    fs::create_dir_all(&base).map_err(|e| format!("Не удалось создать {}: {e}", base.display()))?;
    let seeded_full = seed_full_runtime(app, &base)?;
    let current = status();
    ensure_seeded_runtime_ready(seeded_full, current.ready).map_err(str::to_string)?;
    if current.ready && (!force || seeded_full) {
        return Ok(current);
    }
    // A valid DLL may already be loaded by OutputEngine during normal startup.
    // Keep it in place when repairing only a missing/corrupt sibling file.
    let replace = files_to_replace(
        [current.ffmpeg, current.ffprobe, current.libmpv],
        force,
        reinstall_pending(),
    );
    let ff = component("ffmpeg-gyan-full-9.0.2")?;
    let mpv = component("libmpv")?;
    let work = std::env::temp_dir().join(format!("qlisa-media-runtime-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let ff_url = ff["archive"]["url"]
            .as_str()
            .ok_or("Manifest missing FFmpeg URL")?;
        let ff_archive_hash = ff["archive"]["sha256"]
            .as_str()
            .ok_or("Manifest missing FFmpeg archive hash")?;
        let ff_archive = work.join("ffmpeg.zip");
        progress(app, "download", Some(0), "Скачивание медиакомпонентов…");
        download(ff_url, &ff_archive, app, "FFmpeg", 0, 75)?;
        verify_hash(&ff_archive, ff_archive_hash, "FFmpeg archive")?;
        let ff_root = ff["archive"]["rootDirectory"]
            .as_str()
            .ok_or("Manifest missing FFmpeg archive root")?;
        let mpv_url = mpv["build"]["archiveUrl"]
            .as_str()
            .ok_or("Manifest missing libmpv URL")?;
        let mpv_archive_hash = mpv["build"]["archiveSha256"]
            .as_str()
            .ok_or("Manifest missing libmpv archive hash")?;
        let mpv_archive = work.join("mpv.7z");
        download(mpv_url, &mpv_archive, app, "libmpv", 75, 95)?;
        verify_hash(&mpv_archive, mpv_archive_hash, "libmpv archive")?;
        progress(app, "verify", Some(95), "Проверка медиакомпонентов…");
        let temp_ff = ["ffmpeg.exe", "ffprobe.exe"].map(|n| work.join(n));
        for (i, n) in ["ffmpeg.exe", "ffprobe.exe"].iter().enumerate() {
            progress(app, "extract", Some(96 + i as u8), &format!("Подготовка {n}…"));
            let member = ffmpeg_member(ff_root, n)?;
            extract_member(&ff_archive, &member, &temp_ff[i])?;
            verify_hash(&temp_ff[i], &expected_hash("ffmpeg-gyan-full-9.0.2", n)?, n)?;
        }
        let temp_mpv = work.join("libmpv-2.dll");
        progress(app, "extract", Some(98), "Подготовка libmpv…");
        extract_member(&mpv_archive, "libmpv-2.dll", &temp_mpv)?;
        verify_hash(
            &temp_mpv,
            &expected_hash("libmpv", "libmpv-2.dll")?,
            "libmpv-2.dll",
        )?;
        // Promote only verified files. Same-volume rename is atomic for each file.
        for (index, (src, name)) in temp_ff
            .iter()
            .zip(["ffmpeg.exe", "ffprobe.exe"])
            .chain(std::iter::once((&temp_mpv, "libmpv-2.dll")))
            .enumerate()
        {
            if !replace[index] { continue; }
            let staged = base.join(format!("{name}.new"));
            fs::copy(src, &staged).map_err(|e| e.to_string())?;
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                let from: Vec<u16> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
                let to: Vec<u16> = base
                    .join(name)
                    .as_os_str()
                    .encode_wide()
                    .chain(Some(0))
                    .collect();
                let ok = unsafe {
                    windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                        from.as_ptr(),
                        to.as_ptr(),
                        windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                            | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
                    )
                };
                if ok == 0 {
                    return Err(format!(
                        "Could not install {name}: {}",
                        std::io::Error::last_os_error()
                    ));
                }
            }
            #[cfg(not(windows))]
            fs::rename(&staged, base.join(name))
                .map_err(|e| format!("Could not install {name}: {e}"))?;
        }
        fs::remove_file(pending_path()).ok();
        progress(app, "complete", Some(100), "Медиакомпоненты готовы");
        Ok(status())
    })();
    fs::remove_dir_all(work).ok();
    result
}
pub async fn prepare(app: AppHandle, force: bool) -> Result<MediaRuntimeStatus, String> {
    #[cfg(not(windows))]
    { let _ = (app, force); return Ok(status()); }
    #[cfg(windows)]
    {
    tauri::async_runtime::spawn_blocking(move || prepare_sync(&app, force))
        .await
        .map_err(|e| e.to_string())?
    }
}
pub fn schedule_reinstall() -> Result<(), String> {
    fs::create_dir_all(runtime_dir()).map_err(|e| e.to_string())?;
    fs::write(pending_path(), b"reinstall on next launch").map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_media_runtime_status(app: AppHandle) -> MediaRuntimeStatus {
    status_for_app(&app)
}
#[tauri::command]
pub async fn prepare_media_runtime(
    app: AppHandle,
    force: bool,
) -> Result<MediaRuntimeStatus, String> {
    prepare(app, force).await
}
#[tauri::command]
pub fn schedule_media_runtime_reinstall() -> Result<(), String> {
    schedule_reinstall()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_pins_hashes_and_urls() {
        assert_eq!(expected_hash("libmpv", "libmpv-2.dll").unwrap().len(), 64);
        assert!(component("ffmpeg-gyan-full-9.0.2").unwrap()["archive"]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://"));
        assert_eq!(expected_hash("vulkan-loader", "vulkan-1.dll").unwrap(), "e933f507df7162eb64de289c5c5ce4ecb48584e5c2b507eea4750b1c870ae6bd");
    }
    #[test]
    fn archive_members_reject_path_traversal() {
        assert_eq!(ffmpeg_member("pinned-root", "ffmpeg.exe").unwrap(), "pinned-root/bin/ffmpeg.exe");
        assert!(ffmpeg_member("../escape", "ffmpeg.exe").is_err());
        assert!(ffmpeg_member("pinned-root", "../escape").is_err());
    }
    #[cfg(windows)]
    #[test]
    fn extracts_7z_member_with_lzma() {
        let work = std::env::temp_dir().join(format!("qlisa-7z-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&work).unwrap();
        let archive = work.join("fixture.7z");
        let output = work.join("libmpv-2.dll");
        fs::write(
            &archive,
            include_bytes!("../tests/fixtures/libmpv-extract-fixture.7z"),
        )
        .unwrap();
        extract_member(&archive, "libmpv-2.dll", &output).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"fixture dll payload");
        fs::remove_dir_all(work).unwrap();
    }
    #[test]
    fn repair_replaces_only_the_missing_file() {
        assert_eq!(files_to_replace([true, false, true], false, false), [false, true, false]);
        assert_eq!(files_to_replace([true, false, true], true, false), [true; 3]);
        assert_eq!(files_to_replace([true, false, true], false, true), [true; 3]);
    }
    #[test]
    fn status_requires_all_three_valid_files() {
        let s = status();
        assert_eq!(s.ready, s.ffmpeg && s.ffprobe && s.libmpv);
    }
    #[test]
    fn full_payload_requires_cached_vulkan_loader_before_frontend_ready() {
        assert!(!full_package_ready(true, true, false));
        assert!(full_package_ready(true, true, true));
        assert!(full_package_ready(true, false, false));
        assert!(!full_package_ready(false, false, false));
    }
    #[test]
    fn invalid_full_seed_fails_instead_of_falling_back_to_download() {
        assert!(ensure_seeded_runtime_ready(true, false).is_err());
        assert!(ensure_seeded_runtime_ready(true, true).is_ok());
        assert!(ensure_seeded_runtime_ready(false, false).is_ok());
    }
    #[test]
    fn seed_file_verifies_before_skip_and_preserves_destination_on_bad_input() {
        use sha2::{Digest, Sha256};
        let root = std::env::temp_dir().join(format!("qlisa-seed-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.bin");
        let destination = root.join("runtime.bin");
        let pinned = b"pinned payload";
        let wrong = b"wrong payload";
        let hash = format!("{:x}", Sha256::digest(pinned));
        fs::write(&source, pinned).unwrap();
        fs::write(&destination, b"old payload").unwrap();
        assert!(seed_file(&source, &destination, &hash, "test payload").unwrap());
        assert_eq!(fs::read(&destination).unwrap(), pinned);
        assert!(!seed_file(&source, &destination, &hash, "test payload").unwrap());

        fs::write(&source, wrong).unwrap();
        assert!(seed_file(&source, &destination, &hash, "test payload").is_err());
        assert_eq!(fs::read(&destination).unwrap(), pinned);
        fs::remove_dir_all(root).unwrap();
    }
}
