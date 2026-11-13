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
const INSTALLED_MANIFEST: &str = "installed-runtime.json";
static PREPARE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRuntimeStatus {
    pub ffmpeg: bool,
    pub ffprobe: bool,
    pub libmpv: bool,
    pub ready: bool,
    pub state: String,
    pub installed_kit_id: Option<String>,
    pub recommended_kit_id: String,
    pub update_available: bool,
    pub operation_pending: bool,
    pub repair_reason: Option<String>,
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
    #[serde(rename = "recommendedKitId")]
    recommended_kit_id: String,
    #[serde(rename = "approvedKits")]
    approved_kits: Vec<serde_json::Value>,
    components: Vec<serde_json::Value>,
}
fn manifest() -> Result<Manifest, String> {
    serde_json::from_str(MANIFEST).map_err(|e| format!("Invalid embedded runtime manifest: {e}"))
}
fn recommended_kit_id() -> Result<String, String> {
    let manifest = manifest()?;
    if manifest.schema_version != 1 { return Err("Unsupported runtime manifest version".into()); }
    Ok(manifest.recommended_kit_id)
}
fn kit(id: &str) -> Result<serde_json::Value, String> {
    let manifest = manifest()?;
    if manifest.schema_version != 1 { return Err("Unsupported runtime manifest version".into()); }
    manifest.approved_kits.into_iter().find(|v| v["id"] == id)
        .ok_or_else(|| format!("Runtime manifest does not approve kit {id}"))
}
fn kit_component<'a>(kit: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
    kit["components"].as_array()?.iter().find(|component| component["id"] == id)
}
fn kit_file_hash(kit: &serde_json::Value, component_id: &str, file: &str) -> Option<String> {
    kit_component(kit, component_id)?["runtimeFiles"].as_array()?.iter()
        .find(|record| record["fileName"] == file)?["sha256"].as_str().map(str::to_owned)
}
fn all_approved_kits() -> Result<Vec<serde_json::Value>, String> {
    let manifest = manifest()?;
    if manifest.schema_version != 1 { return Err("Unsupported runtime manifest version".into()); }
    Ok(manifest.approved_kits)
}
fn canonical_component_id(id: &str) -> Option<&'static str> {
    match id {
        "ffmpeg" | "ffprobe" | "ffmpeg-gyan-full-9.0.2" => Some("ffmpeg"),
        "libmpv" => Some("libmpv"),
        _ => None,
    }
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
fn installed_manifest_path() -> PathBuf {
    runtime_dir().join(INSTALLED_MANIFEST)
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
    let component_id = canonical_component_id(id)
        .ok_or_else(|| format!("Unknown runtime component {id}"))?;
    let recommended = recommended_kit_id()?;
    let kit = kit(&recommended)?;
    kit_file_hash(&kit, component_id, file)
        .ok_or_else(|| format!("Recommended kit {recommended} has no hash for {file}"))
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
#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeInstall {
    Missing,
    Approved(String),
    RepairRequired(String),
}
fn kit_matches_runtime(kit: &serde_json::Value, root: &Path) -> bool {
    [
        ("ffmpeg", "ffmpeg.exe"),
        ("ffmpeg", "ffprobe.exe"),
        ("libmpv", "libmpv-2.dll"),
    ].iter().all(|(component_id, file)| {
        let Some(wanted) = kit_file_hash(kit, component_id, file) else { return false; };
        let Ok(actual) = hash_file(&root.join(file)) else { return false; };
        actual.eq_ignore_ascii_case(&wanted)
    })
}
fn installed_kit_id_from_file(root: &Path) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(&fs::read(root.join(INSTALLED_MANIFEST)).ok()?).ok()?;
    (value["schema_version"] == 1).then(|| value["kit_id"].as_str().map(str::to_owned)).flatten()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingOperation {
    schema_version: u32,
    action: String,
    target_kit_id: String,
}
fn read_pending_operation() -> Result<Option<PendingOperation>, String> {
    let path = pending_path();
    if !path.exists() { return Ok(None); }
    let bytes = fs::read(&path).map_err(|e| format!("Could not read pending runtime operation: {e}"))?;
    parse_pending_operation(&bytes)
}
fn parse_pending_operation(bytes: &[u8]) -> Result<Option<PendingOperation>, String> {
    let parsed = serde_json::from_slice::<PendingOperation>(&bytes);
    if let Ok(operation) = parsed {
        if operation.schema_version == 1
            && matches!(operation.action.as_str(), "update" | "repair")
            && kit(&operation.target_kit_id).is_ok()
        {
            return Ok(Some(operation));
        }
        return Err("Pending media runtime operation has an unsupported target or format".into());
    }
    if bytes != b"reinstall on next launch" {
        return Err("Pending media runtime operation marker is invalid".into());
    }
    // Earlier builds wrote an opaque marker. Treat it as the explicitly
    // requested update that preceded this build.
    Ok(Some(PendingOperation {
        schema_version: 1,
        action: "update".into(),
        target_kit_id: recommended_kit_id()?,
    }))
}
fn write_json_atomic(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let temporary = path.with_extension("json.new");
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let mut file = fs::File::create(&temporary).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    replace_file(&temporary, path)
}
fn record_installed_kit(kit_id: &str) -> Result<(), String> {
    let approved = kit(kit_id)?;
    let path = installed_manifest_path();
    let value = serde_json::json!({
        "schema_version": 1,
        "kit_id": approved["id"].as_str().ok_or("Approved kit has no ID")?,
    });
    write_json_atomic(&path, &value)
}
fn write_pending_operation(action: &str, target_kit_id: &str) -> Result<(), String> {
    if !matches!(action, "update" | "repair") { return Err("Unsupported media runtime operation".into()); }
    let target = kit(target_kit_id)?;
    let value = serde_json::json!({
        "schema_version": 1,
        "action": action,
        "target_kit_id": target["id"].as_str().ok_or("Approved kit has no ID")?,
    });
    write_json_atomic(&pending_path(), &value)
}
fn inspect_runtime_with_catalog(root: &Path, kits: &[serde_json::Value], sidecar_id: Option<&str>) -> RuntimeInstall {
    let names = ["ffmpeg.exe", "ffprobe.exe", "libmpv-2.dll"];
    let present = names.iter().filter(|name| root.join(name).is_file()).count();
    if present == 0 { return RuntimeInstall::Missing; }

    if let Some(id) = sidecar_id {
        if let Some(known) = kits.iter().find(|known| known["id"] == id) {
            if kit_matches_runtime(known, root) { return RuntimeInstall::Approved(id.to_owned()); }
        }
    }
    if let Some(known) = kits.iter().find(|known| kit_matches_runtime(known, root)) {
        if let Some(id) = known["id"].as_str() { return RuntimeInstall::Approved(id.to_owned()); }
    }

    let missing = names.iter().filter(|name| !root.join(name).is_file()).copied().collect::<Vec<_>>();
    let reason = if !missing.is_empty() {
        format!("Runtime kit is incomplete; missing {}", missing.join(", "))
    } else {
        "Installed files do not match an approved runtime kit".into()
    };
    RuntimeInstall::RepairRequired(reason)
}
fn inspect_runtime(root: &Path) -> RuntimeInstall {
    let kits = match all_approved_kits() {
        Ok(kits) => kits,
        Err(error) => return RuntimeInstall::RepairRequired(error),
    };
    let sidecar_id = installed_kit_id_from_file(root);
    inspect_runtime_with_catalog(root, &kits, sidecar_id.as_deref())
}
fn should_seed_pending_full_kit(
    current: &RuntimeInstall,
    pending: Option<&PendingOperation>,
    bundled_kit_id: &str,
) -> bool {
    !matches!(current, RuntimeInstall::Approved(_))
        && pending.is_some_and(|operation| {
            operation.action == "repair" && operation.target_kit_id == bundled_kit_id
        })
}
fn component_matches_catalog(path: &Path, kits: &[serde_json::Value], component_id: &str, file: &str) -> bool {
    let Ok(actual) = hash_file(path) else { return false; };
    kits.iter().any(|kit| kit_file_hash(kit, component_id, file)
        .is_some_and(|wanted| actual.eq_ignore_ascii_case(&wanted)))
}
fn component_matches_any_kit(path: &Path, component_id: &str, file: &str) -> bool {
    all_approved_kits().ok().is_some_and(|kits| component_matches_catalog(path, &kits, component_id, file))
}
#[cfg(windows)]
pub fn mpv_api_supported(version: u32) -> bool {
    all_approved_kits().ok().is_some_and(|kits| kits.iter().any(|approved| {
        let Some(minimum) = kit_component(approved, "libmpv").and_then(|component| component.get("minimumClientApi")) else { return false; };
        let required_major = minimum["major"].as_u64().unwrap_or(u64::MAX) as u32;
        let required_minor = minimum["minor"].as_u64().unwrap_or(u64::MAX) as u32;
        let actual_major = version >> 16;
        let actual_minor = version & 0xffff;
        actual_major == required_major && actual_minor >= required_minor
    }))
}
pub fn verified_file(path: &Path, id: &str, name: &str) -> bool {
    if id == "vulkan-loader" {
        return expected_hash(id, name).ok().and_then(|want| hash_file(path).ok()
            .map(|got| got.eq_ignore_ascii_case(&want))).unwrap_or(false);
    }
    let Some(component_id) = canonical_component_id(id) else { return false; };
    let root = runtime_dir();
    let is_runtime_file = path.parent().is_some_and(|parent| parent == root.as_path());
    if is_runtime_file {
        // A scheduled operation runs after relaunch. Do not load the current
        // DLL while replacing it; status remains based on its verified hash.
        if reinstall_pending() { return false; }
        let RuntimeInstall::Approved(kit_id) = inspect_runtime(&root) else { return false; };
        let Ok(kit) = kit(&kit_id) else { return false; };
        return kit_file_hash(&kit, component_id, name).is_some_and(|want| {
            hash_file(path).ok().is_some_and(|got| got.eq_ignore_ascii_case(&want))
        });
    }
    component_matches_any_kit(path, component_id, name)
}
fn runtime_state(install: &RuntimeInstall, recommended: &str) -> (&'static str, bool) {
    match install {
        RuntimeInstall::Missing => ("missing", false),
        RuntimeInstall::Approved(id) if id == recommended => ("ready", false),
        RuntimeInstall::Approved(_) => ("update-available", true),
        RuntimeInstall::RepairRequired(_) => ("repair-required", false),
    }
}
fn runtime_status(root: &Path) -> MediaRuntimeStatus {
    let recommended = recommended_kit_id().unwrap_or_default();
    let install = inspect_runtime(root);
    let (installed, repair_reason) = match &install {
        RuntimeInstall::Missing => (None, None),
        RuntimeInstall::Approved(id) => (Some(id.clone()), None),
        RuntimeInstall::RepairRequired(reason) => (None, Some(reason.clone())),
    };
    let (state, update_available) = runtime_state(&install, &recommended);
    let ready = installed.is_some();
    let installed_kit = installed.as_deref().and_then(|id| kit(id).ok());
    let ffmpeg = component_matches_any_kit(&root.join("ffmpeg.exe"), "ffmpeg", "ffmpeg.exe");
    let ffprobe = component_matches_any_kit(&root.join("ffprobe.exe"), "ffmpeg", "ffprobe.exe");
    let libmpv = component_matches_any_kit(&root.join("libmpv-2.dll"), "libmpv", "libmpv-2.dll");
    let version = installed_kit.as_ref().and_then(|kit| kit_component(kit, "libmpv"))
        .and_then(|component| component["version"].as_str().map(str::to_owned));
    MediaRuntimeStatus {
        ffmpeg,
        ffprobe,
        libmpv,
        ready,
        state: state.into(),
        installed_kit_id: installed,
        recommended_kit_id: recommended,
        update_available,
        operation_pending: reinstall_pending(),
        repair_reason,
        version,
    }
}
pub fn status() -> MediaRuntimeStatus {
    #[cfg(not(windows))]
    { return MediaRuntimeStatus { ffmpeg: true, ffprobe: true, libmpv: true, ready: true, state: "ready".into(), installed_kit_id: Some("system".into()), recommended_kit_id: "system".into(), update_available: false, operation_pending: false, repair_reason: None, version: Some("system runtime".into()) }; }
    #[cfg(windows)]
    { runtime_status(&runtime_dir()) }
}
pub fn status_for_app(app: &AppHandle) -> MediaRuntimeStatus {
    let current = status();
    #[cfg(windows)]
    {
        match full_resource_root(app) {
            Ok(_) => {},
            Err(error) => {
                log::error!("Could not validate full-installer payload marker: {error}");
                // A malformed optional full-installer payload must not invalidate a
                // separately verified installed runtime kit.
            }
        }
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
    let current = inspect_runtime(base);
    let already_approved = matches!(&current, RuntimeInstall::Approved(_));
    let pending_operation = if reinstall_pending() { read_pending_operation()? } else { None };
    let pending = pending_operation.is_some();
    let payload = match full_resource_root(app) {
        Ok(Some(payload)) => payload,
        Ok(None) => return Ok(false),
        Err(error) if already_approved || pending => {
            log::warn!("Ignoring invalid optional full-installer payload for an installed runtime operation: {error}");
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let full: serde_json::Value = serde_json::from_str(FULL_INSTALLER_MANIFEST)
        .map_err(|e| format!("Invalid embedded full-installer manifest: {e}"))?;
    let records = full["runtimeFiles"].as_array().ok_or("Full-installer manifest has no runtimeFiles")?;
    let bundled_kit_id = full["kitId"].as_str().ok_or("Full-installer manifest has no kitId")?;
    let pending_full_seed = should_seed_pending_full_kit(&current, pending_operation.as_ref(), bundled_kit_id);
    if already_approved || (pending && !pending_full_seed) {
        // Full installers may restore the separate Vulkan loader, but must
        // never replace a compatible media kit or block its network update.
        if let Some(record) = records.iter().find(|r| r["fileName"] == "vulkan-1.dll") {
            let name = record["fileName"].as_str().ok_or("Full-installer manifest has an unnamed runtime file")?;
            let hash = record["sha256"].as_str().ok_or("Full-installer manifest has no Vulkan hash")?;
            if let Err(error) = seed_file(&payload.join(name), &base.join(name), hash, name) {
                log::warn!("Could not refresh optional Vulkan loader without replacing the media kit: {error}");
            }
        }
        return Ok(false);
    }
    for record in records {
        let name = record["fileName"].as_str().ok_or("Full-installer manifest has an unnamed runtime file")?;
        if name.contains(['/', '\\', ':']) || name == "." || name == ".." {
            return Err("Unsafe path in full-installer manifest".into());
        }
        let hash = record["sha256"].as_str().ok_or_else(|| format!("Full-installer manifest has no hash for {name}"))?;
        verify_hash(&payload.join(name), hash, &format!("bundled {name}"))?;
    }
    for record in records {
        let name = record["fileName"].as_str().ok_or("Full-installer manifest has an unnamed runtime file")?;
        let hash = record["sha256"].as_str().ok_or_else(|| format!("Full-installer manifest has no hash for {name}"))?;
        seed_file(&payload.join(name), &base.join(name), hash, name)?;
    }
    if !matches!(inspect_runtime(base), RuntimeInstall::Approved(id) if id == bundled_kit_id) {
        return Err("Full-installer seed did not match an approved runtime kit".into());
    }
    record_installed_kit(bundled_kit_id)?;
    Ok(true)
}
fn component_needs_install(root: &Path, component: &serde_json::Value) -> bool {
    component["runtimeFiles"].as_array().is_none_or(|files| files.iter().any(|file| {
        let Some(name) = file["fileName"].as_str() else { return true; };
        let Some(hash) = file["sha256"].as_str() else { return true; };
        hash_file(&root.join(name)).ok().is_none_or(|actual| !actual.eq_ignore_ascii_case(hash))
    }))
}
fn validate_component(root: &Path, component: &serde_json::Value) -> Result<(), String> {
    let id = component["id"].as_str().ok_or("Runtime component has no ID")?;
    match id {
        "ffmpeg" => {
            let executable = root.join("ffmpeg.exe");
            let mut version_command = std::process::Command::new(&executable);
            version_command.arg("-version");
            hide_console(&mut version_command);
            let output = version_command.output()
                .map_err(|e| format!("Could not verify FFmpeg: {e}"))?;
            let text = String::from_utf8_lossy(&output.stdout);
            let required = component["requiredVersionText"].as_str().unwrap_or_default();
            if !output.status.success() || !text.contains(required) {
                return Err("Downloaded FFmpeg does not report the pinned version".into());
            }
            let mut protocol_command = std::process::Command::new(executable);
            protocol_command.args(["-hide_banner", "-protocols"]);
            hide_console(&mut protocol_command);
            let output = protocol_command.output()
                .map_err(|e| format!("Could not verify FFmpeg protocols: {e}"))?;
            let text = String::from_utf8_lossy(&output.stdout);
            for protocol in component["requiredProtocols"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
                if !output.status.success() || !text.lines().any(|line| line.trim() == protocol) {
                    return Err(format!("Downloaded FFmpeg does not provide required protocol {protocol}"));
                }
            }
        }
        "libmpv" => {
            // The approved binary hash pins the exact client ABI. Runtime API
            // compatibility is also checked when mpv_sys loads the DLL.
        }
        _ => return Err(format!("Unsupported runtime component {id}")),
    }
    Ok(())
}
#[cfg(windows)]
fn hide_console(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
fn hide_console(_: &mut std::process::Command) {}
fn promote_staged_files(
    base: &Path,
    backup: &Path,
    staged_files: &[(PathBuf, String, String)],
    target: &serde_json::Value,
    target_id: &str,
) -> Result<(), String> {
    fs::create_dir_all(backup).map_err(|e| e.to_string())?;
    let mut replaced = Vec::<String>::new();
    let install_result = (|| {
        for (staged, name, wanted) in staged_files {
            verify_hash(staged, wanted, name)?;
            let destination = base.join(name);
            if destination.exists() {
                let saved = backup.join(name);
                fs::copy(&destination, &saved).map_err(|e| format!("Could not back up {name}: {e}"))?;
            }
            let promote = base.join(format!("{name}.new"));
            fs::copy(staged, &promote).map_err(|e| format!("Could not stage {name}: {e}"))?;
            replace_file(&promote, &destination)?;
            replaced.push(name.clone());
        }
        if !kit_matches_runtime(target, base) { return Err("Installed files do not match the approved runtime kit".into()); }
        record_installed_kit(target_id)?;
        Ok(())
    })();
    if let Err(error) = install_result {
        for name in replaced.iter().rev() {
            let saved = backup.join(name);
            if saved.exists() {
                let rollback = base.join(format!("{name}.rollback"));
                fs::copy(saved, &rollback).map_err(|e| format!("{error}; rollback of {name} failed: {e}"))?;
                replace_file(&rollback, &base.join(name)).map_err(|e| format!("{error}; rollback of {name} failed: {e}"))?;
            } else {
                fs::remove_file(base.join(name)).ok();
            }
        }
        return Err(error);
    }
    Ok(())
}
fn finish_pending_operation<T>(path: &Path, result: Result<T, String>) -> Result<T, String> {
    if result.is_err() && path.exists() { fs::remove_file(path).ok(); }
    result
}
fn prepare_sync(app: &AppHandle, force: bool) -> Result<MediaRuntimeStatus, String> {
    let _guard = PREPARE_LOCK
        .lock()
        .map_err(|_| "Runtime preparation lock is poisoned".to_string())?;
    let result = prepare_sync_locked(app, force);
    finish_pending_operation(&pending_path(), result)
}
fn prepare_sync_locked(app: &AppHandle, force: bool) -> Result<MediaRuntimeStatus, String> {
    let base = runtime_dir();
    fs::create_dir_all(&base).map_err(|e| format!("Не удалось создать {}: {e}", base.display()))?;
    let pending = read_pending_operation()?;
    if let Err(error) = seed_full_runtime(app, &base) {
        if !matches!(inspect_runtime(&base), RuntimeInstall::Missing) { return Err(error); }
        log::warn!("Ignoring invalid optional full-installer seed for an empty runtime; using approved download: {error}");
    }
    if force && pending.is_none() {
        return Err("Schedule a runtime update and restart the app before replacing loaded components".into());
    }
    if pending.is_none() {
        match inspect_runtime(&base) {
            RuntimeInstall::Approved(_) if !force => return Ok(status_for_app(app)),
            RuntimeInstall::RepairRequired(reason) => return Err(format!("Media runtime needs manual repair: {reason}")),
            RuntimeInstall::Missing => {},
            RuntimeInstall::Approved(_) => {},
        }
    }
    let target_id = pending.as_ref().map(|op| op.target_kit_id.clone()).unwrap_or(recommended_kit_id()?);
    let target = kit(&target_id)?;
    let components = target["components"].as_array().ok_or("Approved runtime kit has no components")?;
    let required = components.iter().filter(|component| component_needs_install(&base, component)).collect::<Vec<_>>();
    if required.is_empty() {
        record_installed_kit(&target_id)?;
        fs::remove_file(pending_path()).ok();
        return Ok(status_for_app(app));
    }
    let work = std::env::temp_dir().join(format!("qlisa-media-runtime-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut staged_files = Vec::<(PathBuf, String, String)>::new();
        let total = required.len().max(1) as u8;
        for (index, component) in required.iter().enumerate() {
            let id = component["id"].as_str().ok_or("Runtime component has no ID")?;
            let asset = &component["asset"];
            let url = asset["url"].as_str().ok_or("Runtime component asset has no URL")?;
            let archive_hash = asset["sha256"].as_str().ok_or("Runtime component asset has no SHA-256")?;
            let extension = asset["format"].as_str().unwrap_or("zip");
            let archive = work.join(format!("{id}.{extension}"));
            let start = (index as u8 * 90 / total).min(90);
            let end = (((index as u8 + 1) * 90 / total).max(start + 1)).min(95);
            download(url, &archive, app, id, start, end)?;
            verify_hash(&archive, archive_hash, &format!("{id} archive"))?;
            for record in component["runtimeFiles"].as_array().ok_or("Runtime component has no files")? {
                let name = record["fileName"].as_str().ok_or("Runtime file has no name")?;
                if name.contains(['/', '\\', ':']) || name == "." || name == ".." {
                    return Err("Unsafe path in runtime kit manifest".into());
                }
                let wanted = record["sha256"].as_str().ok_or("Runtime file has no SHA-256")?;
                if hash_file(&base.join(name)).ok().is_some_and(|actual| actual.eq_ignore_ascii_case(wanted)) { continue; }
                let staged = work.join(format!("{name}.verified"));
                extract_member(&archive, name, &staged)?;
                verify_hash(&staged, wanted, name)?;
                staged_files.push((staged, name.to_owned(), wanted.to_owned()));
            }
            // Verify FFmpeg's advertised build and protocol support before any
            // replacement. Other components are pinned by the approved file hash.
            let component_root = work.join(format!("verify-{id}"));
            fs::create_dir_all(&component_root).map_err(|e| e.to_string())?;
            for (staged, name, _) in staged_files.iter().filter(|(_, name, _)| {
                component["runtimeFiles"].as_array().is_some_and(|files| {
                    files.iter().any(|r| r["fileName"].as_str() == Some(name.as_str()))
                })
            }) {
                fs::copy(staged, component_root.join(name)).map_err(|e| e.to_string())?;
            }
            if id == "ffmpeg" {
                for file in component["runtimeFiles"].as_array().into_iter().flatten() {
                    let name = file["fileName"].as_str().ok_or("FFmpeg runtime file has no name")?;
                    if !component_root.join(name).exists() {
                        fs::copy(base.join(name), component_root.join(name)).map_err(|e| e.to_string())?;
                    }
                }
            }
            if id == "ffmpeg" { validate_component(&component_root, component)?; }
        }
        progress(app, "verify", Some(96), "Проверка медиакомпонентов…");
        promote_staged_files(&base, &work.join("backup"), &staged_files, &target, &target_id)?;
        fs::remove_file(pending_path()).ok();
        progress(app, "complete", Some(100), "Медиакомпоненты готовы");
        Ok(status_for_app(app))
    })();
    fs::remove_dir_all(work).ok();
    if result.is_err() && pending.is_some() { fs::remove_file(pending_path()).ok(); }
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
pub fn schedule_reinstall(action: Option<String>) -> Result<(), String> {
    fs::create_dir_all(runtime_dir()).map_err(|e| e.to_string())?;
    let action = action.unwrap_or_else(|| "update".into());
    if !matches!(action.as_str(), "update" | "repair") { return Err("Unsupported media runtime operation".into()); }
    if action == "repair" {
        if let Some(id) = installed_kit_id_from_file(&runtime_dir()) {
            if kit(&id).is_ok() { return write_pending_operation(&action, &id); }
        }
    }
    write_pending_operation(&action, &recommended_kit_id()?)
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
pub fn schedule_media_runtime_reinstall(action: Option<String>) -> Result<(), String> {
    schedule_reinstall(action)
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
    #[cfg(windows)]
    #[test]
    fn mpv_api_must_meet_an_approved_kit_minimum() {
        assert!(mpv_api_supported((2 << 16) | 5));
        assert!(mpv_api_supported((2 << 16) | 6));
        assert!(!mpv_api_supported((2 << 16) | 4));
        assert!(!mpv_api_supported((3 << 16) | 0));
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
    fn runtime_state_distinguishes_update_and_repair_without_hiding_ready_kits() {
        assert_eq!(runtime_state(&RuntimeInstall::Missing, "new"), ("missing", false));
        assert_eq!(runtime_state(&RuntimeInstall::Approved("old".into()), "new"), ("update-available", true));
        assert_eq!(runtime_state(&RuntimeInstall::Approved("new".into()), "new"), ("ready", false));
        assert_eq!(runtime_state(&RuntimeInstall::RepairRequired("bad hash".into()), "new"), ("repair-required", false));
    }
    #[test]
    fn bundled_seed_is_allowed_only_for_matching_manual_repair_of_unapproved_cache() {
        let bundled = "win-x64-2026.11.14.1";
        let repair = PendingOperation { schema_version: 1, action: "repair".into(), target_kit_id: bundled.into() };
        assert!(should_seed_pending_full_kit(&RuntimeInstall::RepairRequired("partial".into()), Some(&repair), bundled));
        assert!(should_seed_pending_full_kit(&RuntimeInstall::Missing, Some(&repair), bundled));

        let update = PendingOperation { schema_version: 1, action: "update".into(), target_kit_id: bundled.into() };
        assert!(!should_seed_pending_full_kit(&RuntimeInstall::RepairRequired("partial".into()), Some(&update), bundled));
        let other_target = PendingOperation { schema_version: 1, action: "repair".into(), target_kit_id: "older-kit".into() };
        assert!(!should_seed_pending_full_kit(&RuntimeInstall::RepairRequired("partial".into()), Some(&other_target), bundled));
        assert!(!should_seed_pending_full_kit(&RuntimeInstall::Approved(bundled.into()), Some(&repair), bundled));
    }
    #[test]
    fn pending_marker_accepts_only_known_legacy_text_or_approved_operation() {
        assert!(parse_pending_operation(b"random invalid marker").is_err());
        let legacy = parse_pending_operation(b"reinstall on next launch").unwrap().unwrap();
        assert_eq!(legacy.action, "update");
        let recommended = recommended_kit_id().unwrap();
        let marker = serde_json::to_vec(&PendingOperation { schema_version: 1, action: "repair".into(), target_kit_id: recommended.clone() }).unwrap();
        let parsed = parse_pending_operation(&marker).unwrap().unwrap();
        assert_eq!(parsed.action, "repair");
        assert_eq!(parsed.target_kit_id, recommended);
    }
    #[test]
    fn component_selection_skips_a_component_whose_files_already_match() {
        use sha2::{Digest, Sha256};
        let root = std::env::temp_dir().join(format!("qlisa-kit-select-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let bytes_a = b"ffmpeg-a";
        let bytes_b = b"ffmpeg-b";
        let hash_a = format!("{:x}", Sha256::digest(bytes_a));
        let hash_b = format!("{:x}", Sha256::digest(bytes_b));
        let component = serde_json::json!({ "id":"ffmpeg", "runtimeFiles":[
            {"fileName":"ffmpeg.exe", "sha256":hash_a},
            {"fileName":"ffprobe.exe", "sha256":hash_b}
        ]});
        fs::write(root.join("ffmpeg.exe"), bytes_a).unwrap();
        fs::write(root.join("ffprobe.exe"), b"old ffprobe").unwrap();
        assert!(component_needs_install(&root, &component));
        fs::write(root.join("ffprobe.exe"), bytes_b).unwrap();
        assert!(!component_needs_install(&root, &component));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn failed_staged_install_restores_already_replaced_files() {
        use sha2::{Digest, Sha256};
        let root = std::env::temp_dir().join(format!("qlisa-kit-rollback-{}", uuid::Uuid::new_v4()));
        let backup = root.join("backup");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ffmpeg.exe"), b"old version").unwrap();
        let staged = root.join("new.ffmpeg");
        fs::write(&staged, b"new version").unwrap();
        let hash = format!("{:x}", Sha256::digest(b"new version"));
        let staged_files = vec![
            (staged, "ffmpeg.exe".to_string(), hash),
            (root.join("missing-stage"), "ffprobe.exe".to_string(), "00".repeat(32)),
        ];
        let target = serde_json::json!({});
        assert!(promote_staged_files(&root, &backup, &staged_files, &target, "unknown-kit").is_err());
        assert_eq!(fs::read(root.join("ffmpeg.exe")).unwrap(), b"old version");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn failed_pending_operation_clears_marker_without_invalidating_old_kit() {
        use sha2::{Digest, Sha256};
        let root = std::env::temp_dir().join(format!("qlisa-pending-failure-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let files = [("ffmpeg.exe", b"old ffmpeg".as_slice()), ("ffprobe.exe", b"old ffprobe".as_slice()), ("libmpv-2.dll", b"old mpv".as_slice())];
        for (name, bytes) in files { fs::write(root.join(name), bytes).unwrap(); }
        let hashes = files.map(|(_, bytes)| format!("{:x}", Sha256::digest(bytes)));
        let old_kit = serde_json::json!({"id":"old", "components":[
            {"id":"ffmpeg", "runtimeFiles":[
                {"fileName":"ffmpeg.exe", "sha256":hashes[0]},
                {"fileName":"ffprobe.exe", "sha256":hashes[1]}]},
            {"id":"libmpv", "runtimeFiles":[{"fileName":"libmpv-2.dll", "sha256":hashes[2]}]}
        ]});
        let catalog = vec![old_kit];
        let marker = root.join(PENDING);
        fs::write(&marker, b"pending").unwrap();
        let old_status = runtime_state(&RuntimeInstall::Approved("old".into()), "new");
        assert_eq!(old_status, ("update-available", true));
        let result: Result<(), String> = finish_pending_operation(&marker, Err("download failed".into()));
        assert!(result.is_err());
        assert!(!marker.exists());
        assert_eq!(inspect_runtime_with_catalog(&root, &catalog, Some("old")), RuntimeInstall::Approved("old".into()));
        for (name, bytes) in files { assert_eq!(fs::read(root.join(name)).unwrap(), bytes); }
        assert_eq!(old_status, ("update-available", true));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn only_a_complete_approved_kit_is_ready() {
        use sha2::{Digest, Sha256};
        let root = std::env::temp_dir().join(format!("qlisa-inspect-kit-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let kit = |id: &str, ff: &[u8], probe: &[u8], mpv: &[u8]| serde_json::json!({
            "id":id, "components":[
                {"id":"ffmpeg", "runtimeFiles":[
                    {"fileName":"ffmpeg.exe", "sha256":format!("{:x}", Sha256::digest(ff))},
                    {"fileName":"ffprobe.exe", "sha256":format!("{:x}", Sha256::digest(probe))}]},
                {"id":"libmpv", "runtimeFiles":[{"fileName":"libmpv-2.dll", "sha256":format!("{:x}", Sha256::digest(mpv))}]}
            ]
        });
        let old = kit("old", b"old ffmpeg", b"old ffprobe", b"old mpv");
        let new = kit("new", b"new ffmpeg", b"new ffprobe", b"new mpv");
        let catalog = vec![old.clone(), new.clone()];
        fs::write(root.join("ffmpeg.exe"), b"old ffmpeg").unwrap();
        fs::write(root.join("ffprobe.exe"), b"old ffprobe").unwrap();
        fs::write(root.join("libmpv-2.dll"), b"old mpv").unwrap();
        assert_eq!(inspect_runtime_with_catalog(&root, &catalog, Some("old")), RuntimeInstall::Approved("old".into()));

        // Every file is individually trusted, but the mixed set is no kit.
        fs::write(root.join("ffprobe.exe"), b"new ffprobe").unwrap();
        let individually_trusted = component_matches_catalog(&root.join("ffmpeg.exe"), &catalog, "ffmpeg", "ffmpeg.exe")
            && component_matches_catalog(&root.join("ffprobe.exe"), &catalog, "ffmpeg", "ffprobe.exe")
            && component_matches_catalog(&root.join("libmpv-2.dll"), &catalog, "libmpv", "libmpv-2.dll");
        assert!(individually_trusted);
        assert!(matches!(inspect_runtime_with_catalog(&root, &catalog, Some("old")), RuntimeInstall::RepairRequired(_)));

        fs::write(root.join("ffprobe.exe"), b"corrupt").unwrap();
        assert!(matches!(inspect_runtime_with_catalog(&root, &catalog, Some("old")), RuntimeInstall::RepairRequired(_)));
        fs::remove_dir_all(root).unwrap();
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
