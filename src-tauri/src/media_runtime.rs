//! Pinned, per-user media runtime bootstrap for Windows.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::{AppHandle, Emitter};

const MANIFEST: &str = include_str!("../../scripts/runtime-manifest.json");
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
            "ffmpeg-btbn-gpl-n9.0",
            "ffmpeg.exe",
        );
    let ffprobe = !pending
        && verified_file(
            &root.join("ffprobe.exe"),
            "ffmpeg-btbn-gpl-n9.0",
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
fn prepare_sync(app: &AppHandle, force: bool) -> Result<MediaRuntimeStatus, String> {
    let _guard = PREPARE_LOCK
        .lock()
        .map_err(|_| "Runtime preparation lock is poisoned".to_string())?;
    let current = status();
    if current.ready && !force {
        return Ok(current);
    }
    let ff = component("ffmpeg-btbn-gpl-n9.0")?;
    let mpv = component("libmpv")?;
    let base = runtime_dir();
    fs::create_dir_all(&base).map_err(|e| format!("Не удалось создать {}: {e}", base.display()))?;
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
            verify_hash(&temp_ff[i], &expected_hash("ffmpeg-btbn-gpl-n9.0", n)?, n)?;
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
        for (src, name) in temp_ff
            .iter()
            .zip(["ffmpeg.exe", "ffprobe.exe"])
            .chain(std::iter::once((&temp_mpv, "libmpv-2.dll")))
        {
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
pub fn get_media_runtime_status() -> MediaRuntimeStatus {
    status()
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
        assert!(component("ffmpeg-btbn-gpl-n9.0").unwrap()["archive"]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://"));
    }
    #[test]
    fn archive_members_reject_path_traversal() {
        assert_eq!(ffmpeg_member("pinned-root", "ffmpeg.exe").unwrap(), "pinned-root/bin/ffmpeg.exe");
        assert!(ffmpeg_member("../escape", "ffmpeg.exe").is_err());
        assert!(ffmpeg_member("pinned-root", "../escape").is_err());
    }
    #[test]
    fn status_requires_all_three_valid_files() {
        let s = status();
        assert_eq!(s.ready, s.ffmpeg && s.ffprobe && s.libmpv);
    }
}
