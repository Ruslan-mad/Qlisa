//! Small, project-scoped cache for derived media preview data.
//!
//! Cache entries are independent versioned files. They never contain source
//! media or decoded PCM. A corrupt, missing, or unwritable cache is a miss.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"QLCACHE1";
const CACHE_VERSION: u32 = 1;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const SCOPE_LIMIT_BYTES: u64 = 256 * 1024 * 1024;
const PRUNE_TO_BYTES: u64 = 224 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct CacheScope {
    root: PathBuf,
    project_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EntryHeader {
    cache_version: u32,
    kind: String,
    source_identity: String,
    /// Used only to promote entries between project scopes. It is not part of
    /// the key, so moving a project and its relative media preserves the key.
    source_path: String,
    source_size: u64,
    source_mtime_ns: String,
    params: String,
    payload_kind: String,
    payload_sha256: String,
}

#[derive(Debug, Clone)]
struct SourceIdentity {
    identity: String,
    canonical_path: PathBuf,
    size: u64,
    mtime_ns: String,
}

#[derive(Debug, Clone)]
struct Entry {
    header: EntryHeader,
    payload: Vec<u8>,
}

impl CacheScope {
    pub fn at(root: PathBuf, project_dir: Option<PathBuf>) -> Self { Self { root, project_dir } }

    /// A saved project's cache sits beside its JSON project file.
    pub fn for_project(project_path: &Path) -> Self {
        let root = append_suffix(project_path, ".cache");
        let project_dir = project_path.parent().map(Path::to_path_buf);
        Self { root, project_dir }
    }

    /// Unopened workspaces get an isolated cache directory. The caller passes
    /// a stable workspace identity (the workspace creation timestamp).
    pub fn for_unsaved(workspace_identity: &str) -> Self {
        let root = crate::machine_config::config_base_dir()
            .join("Inkue")
            .join("media-cache")
            .join(hash_text(workspace_identity));
        Self { root, project_dir: None }
    }

    pub fn from_workspace(project_path: Option<&Path>, workspace_identity: &str) -> Self {
        project_path.map(Self::for_project).unwrap_or_else(|| Self::for_unsaved(workspace_identity))
    }

    pub fn root(&self) -> &Path { &self.root }

    fn source(&self, path: &Path) -> Option<SourceIdentity> {
        source_identity(path, self.project_dir.as_deref())
    }

    fn entry_path(&self, header: &EntryHeader) -> PathBuf {
        self.root.join(format!("{}.qcache", entry_key(header)))
    }

    /// Load an entry or generate it once for concurrent requests of the same
    /// key. The generator runs outside the cache and workspace locks.
    pub fn get_or_generate<F>(
        &self,
        path: &Path,
        kind: &str,
        params: &str,
        payload_kind: &str,
        generate: F,
    ) -> Option<Vec<u8>>
    where
        F: FnOnce() -> Option<Vec<u8>>,
    {
        self.get_or_generate_validated(path, kind, params, payload_kind, |_| true, generate)
    }

    pub fn get_or_generate_validated<F, V>(
        &self,
        path: &Path,
        kind: &str,
        params: &str,
        payload_kind: &str,
        valid_payload: V,
        generate: F,
    ) -> Option<Vec<u8>>
    where
        F: FnOnce() -> Option<Vec<u8>>,
        V: Fn(&[u8]) -> bool,
    {
        let Some(source) = self.source(path) else { return generate() };
        let header = make_header(&source, kind, params, payload_kind);
        let cache_path = self.entry_path(&header);
        if let Some(payload) = read_entry(&cache_path, &header) {
            if valid_payload(&payload) {
                touch_best_effort(&cache_path);
                return Some(payload);
            }
            let _ = fs::remove_file(&cache_path);
        }

        let key = cache_path.to_string_lossy().into_owned();
        let key_lock = cache_key_lock(&key);
        let _guard = key_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(payload) = read_entry(&cache_path, &header) {
            if valid_payload(&payload) {
                touch_best_effort(&cache_path);
                return Some(payload);
            }
            let _ = fs::remove_file(&cache_path);
        }

        let payload = generate()?;
        if payload.len() as u64 > MAX_ENTRY_BYTES { return Some(payload); }
        // Do not attach output from a source that changed while generation ran
        // to the fingerprint captured before the generator started.
        let Some(current) = self.source(path) else { return Some(payload) };
        if !same_fingerprint(&source, &current) { return Some(payload); }
        let entry = Entry { header, payload: payload.clone() };
        match write_entry_atomic(&cache_path, &entry) {
            Ok(()) => prune_scope(&self.root),
            Err(error) => log::debug!("Preview cache write failed for {}: {error}", cache_path.display()),
        }
        Some(payload)
    }

    /// True when this source has an older entry for the same request. Callers
    /// use this to avoid trusting already-decoded cue PCM after a replacement.
    pub fn has_stale_entry(&self, path: &Path, kind: &str, payload_kind: &str) -> bool {
        let Some(source) = self.source(path) else { return false };
        let Ok(entries) = fs::read_dir(&self.root) else { return false };
        entries.flatten().any(|item| {
            if !is_owned_cache_file(&item.path()) { return false; }
            let Some(header) = read_header(&item.path()) else { return false };
            header.kind == kind
                && header.source_identity == source.identity
                && header.payload_kind == payload_kind
                && !same_fingerprint_from_header(&source, &header)
        })
    }

    /// Promote valid entries to a new project scope. Each key is recalculated
    /// against the destination's project-relative media identity.
    pub fn promote_to(&self, destination: &CacheScope) {
        self.promote_mapped(destination, &HashMap::new(), false);
    }

    /// Promote entries whose media was copied by Collect and Save. `path_map`
    /// maps original absolute paths to destination paths.
    pub fn promote_collected(&self, destination: &CacheScope, path_map: &HashMap<PathBuf, PathBuf>) {
        let canonical_map: HashMap<PathBuf, PathBuf> = path_map.iter().filter_map(|(source, target)| {
            Some((source.canonicalize().ok()?, target.canonicalize().ok()?))
        }).collect();
        self.promote_mapped(destination, &canonical_map, true);
    }

    fn promote_mapped(&self, destination: &CacheScope, path_map: &HashMap<PathBuf, PathBuf>, require_mapping: bool) {
        let Ok(entries) = fs::read_dir(&self.root) else { return };
        for item in entries.flatten() {
            let source_entry_path = item.path();
            if !is_owned_cache_file(&source_entry_path) { continue; }
            let Some(mut entry) = read_any_entry(&source_entry_path) else { continue };
            let old_source = self.resolve_entry_source(&entry.header);
            let mapped = path_map.get(&old_source);
            if require_mapping && mapped.is_none() { continue; }
            let new_path = mapped.cloned().unwrap_or_else(|| old_source.clone());
            let Some(original_source) = self.source(&old_source) else { continue };
            let Some(source) = destination.source(&new_path) else { continue };

            // Reject stale entries before a Save As or collect operation can
            // bless old pixels/peaks with the new source fingerprint.
            if !same_fingerprint_from_header(&original_source, &entry.header) || source.size != entry.header.source_size { continue; }
            entry.header.source_identity = source.identity;
            entry.header.source_path = source.canonical_path.to_string_lossy().into_owned();
            entry.header.source_size = source.size;
            entry.header.source_mtime_ns = source.mtime_ns;
            let target_path = destination.entry_path(&entry.header);
            match write_entry_atomic(&target_path, &entry) {
                Ok(()) => prune_scope(&destination.root),
                Err(error) => log::debug!("Preview cache promotion failed for {}: {error}", target_path.display()),
            }
        }
    }

    fn resolve_entry_source(&self, header: &EntryHeader) -> PathBuf {
        if let (Some(base), Some(relative)) = (
            self.project_dir.as_ref(),
            header.source_identity.strip_prefix("rel:"),
        ) {
            let candidate = base.join(relative);
            if let Ok(canonical) = candidate.canonicalize() { return canonical; }
        }
        PathBuf::from(&header.source_path)
    }
}

/// Revision used by the UI to detect a replacement at the same cue path.
pub fn media_source_revision(path: &Path, project_dir: Option<&Path>) -> Option<String> {
    let source = source_identity(path, project_dir)?;
    let mut hash = Sha256::new();
    hash.update(source.identity.as_bytes());
    hash.update([0]);
    hash.update(source.size.to_le_bytes());
    hash.update(source.mtime_ns.as_bytes());
    Some(format!("{:x}", hash.finalize()))
}

fn source_identity(path: &Path, project_dir: Option<&Path>) -> Option<SourceIdentity> {
    let canonical_path = path.canonicalize().ok()?;
    let metadata = fs::metadata(&canonical_path).ok()?;
    if !metadata.is_file() { return None; }
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    let mtime_ns = modified.as_nanos().to_string();
    let identity = project_dir
        .and_then(|base| base.canonicalize().ok())
        .and_then(|base| canonical_path.strip_prefix(base).ok().map(normalize_relative))
        .map(|relative| format!("rel:{relative}"))
        .unwrap_or_else(|| format!("abs:{}", normalize_absolute(&canonical_path)));
    Some(SourceIdentity { identity, canonical_path, size: metadata.len(), mtime_ns })
}

fn make_header(source: &SourceIdentity, kind: &str, params: &str, payload_kind: &str) -> EntryHeader {
    EntryHeader {
        cache_version: CACHE_VERSION,
        kind: kind.to_owned(),
        source_identity: source.identity.clone(),
        source_path: source.canonical_path.to_string_lossy().into_owned(),
        source_size: source.size,
        source_mtime_ns: source.mtime_ns.clone(),
        params: params.to_owned(),
        payload_kind: payload_kind.to_owned(),
        payload_sha256: String::new(),
    }
}

fn entry_key(header: &EntryHeader) -> String {
    let mut hash = Sha256::new();
    hash.update(header.cache_version.to_le_bytes());
    let size = header.source_size.to_string();
    for part in [header.kind.as_str(), header.source_identity.as_str(), size.as_str(),
        header.source_mtime_ns.as_str(), header.params.as_str(), header.payload_kind.as_str()] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())
}

fn same_fingerprint(a: &SourceIdentity, b: &SourceIdentity) -> bool {
    a.identity == b.identity && a.size == b.size && a.mtime_ns == b.mtime_ns
}

fn same_fingerprint_from_header(source: &SourceIdentity, header: &EntryHeader) -> bool {
    source.size == header.source_size && source.mtime_ns == header.source_mtime_ns
}

fn read_entry(path: &Path, expected: &EntryHeader) -> Option<Vec<u8>> {
    let entry = read_any_entry(path)?;
    same_cache_request(&entry.header, expected).then_some(entry.payload)
}

fn same_cache_request(a: &EntryHeader, b: &EntryHeader) -> bool {
    a.cache_version == b.cache_version && a.kind == b.kind
        && a.source_identity == b.source_identity && a.source_size == b.source_size
        && a.source_mtime_ns == b.source_mtime_ns && a.params == b.params
        && a.payload_kind == b.payload_kind
}

fn read_header(path: &Path) -> Option<EntryHeader> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    if length > MAX_ENTRY_BYTES + MAX_HEADER_BYTES as u64 + 12 { return None; }
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic).ok()?;
    if &magic != MAGIC { return None; }
    let mut header_len = [0u8; 4];
    file.read_exact(&mut header_len).ok()?;
    let header_len = u32::from_le_bytes(header_len) as usize;
    if header_len == 0 || header_len > MAX_HEADER_BYTES { return None; }
    let mut bytes = vec![0u8; header_len];
    file.read_exact(&mut bytes).ok()?;
    let header: EntryHeader = serde_json::from_slice(&bytes).ok()?;
    if header.cache_version != CACHE_VERSION { return None; }
    Some(header)
}

fn read_any_entry(path: &Path) -> Option<Entry> {
    let header = read_header(path)?;
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut fixed = [0u8; 12];
    file.read_exact(&mut fixed).ok()?;
    let header_len = u32::from_le_bytes(fixed[8..12].try_into().ok()?) as u64;
    let payload_len = length.checked_sub(12 + header_len)?;
    if payload_len > MAX_ENTRY_BYTES { return None; }
    file.seek(SeekFrom::Start(12 + header_len)).ok()?;
    let mut payload = Vec::with_capacity(payload_len as usize);
    file.read_to_end(&mut payload).ok()?;
    if payload.len() as u64 != payload_len || hash_text_bytes(&payload) != header.payload_sha256 { return None; }
    Some(Entry { header, payload })
}

fn write_entry_atomic(path: &Path, entry: &Entry) -> std::io::Result<()> {
    let mut entry = entry.clone();
    entry.header.payload_sha256 = hash_text_bytes(&entry.payload);
    let header = serde_json::to_vec(&entry.header)
        .map_err(std::io::Error::other)?;
    if header.len() > MAX_HEADER_BYTES || entry.payload.len() as u64 > MAX_ENTRY_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "cache entry too large"));
    }
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    if path.exists() {
        if read_any_entry(path).is_some() { return Ok(()); }
        // Replace only this exact content-addressed cache entry. This is
        // needed on Windows, where rename does not replace an existing file.
        fs::remove_file(path)?;
    }
    let temp_path = path.with_extension(format!("qcache-{}.tmp", uuid::Uuid::new_v4()));
    let write_result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp_path)?;
        file.write_all(MAGIC)?;
        file.write_all(&(header.len() as u32).to_le_bytes())?;
        file.write_all(&header)?;
        file.write_all(&entry.payload)?;
        file.sync_all()?;
        match fs::rename(&temp_path, path) {
            Ok(()) => Ok(()),
            Err(error) => Err(error),
        }
    })();
    if write_result.is_err() { let _ = fs::remove_file(&temp_path); }
    write_result
}

fn prune_scope(root: &Path) {
    prune_scope_limits(root, SCOPE_LIMIT_BYTES, PRUNE_TO_BYTES);
}

fn prune_scope_limits(root: &Path, scope_limit: u64, prune_to: u64) {
    let Ok(entries) = fs::read_dir(root) else { return };
    let mut files = Vec::new();
    let mut total = 0u64;
    for item in entries.flatten() {
        let path = item.path();
        if !is_owned_cache_file(&path) { continue; }
        let Ok(metadata) = item.metadata() else { continue };
        if !metadata.is_file() { continue; }
        total = total.saturating_add(metadata.len());
        files.push((metadata.modified().unwrap_or(UNIX_EPOCH), metadata.len(), path));
    }
    if total <= scope_limit { return; }
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, size, path) in files {
        if total <= prune_to { break; }
        if fs::remove_file(path).is_ok() { total = total.saturating_sub(size); }
    }
}

fn is_owned_cache_file(path: &Path) -> bool {
    if path.extension().and_then(|extension| extension.to_str()) != Some("qcache") { return false; }
    path.file_stem().and_then(|stem| stem.to_str()).is_some_and(|stem| {
        stem.len() == 64 && stem.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn touch_best_effort(path: &Path) {
    // File timestamps serve as a cheap best-effort LRU signal. Failure is a
    // cache concern only and does not affect the preview.
    if let Ok(file) = OpenOptions::new().write(true).open(path) {
        let times = std::fs::FileTimes::new().set_modified(SystemTime::now());
        let _ = file.set_times(times);
    }
}

fn hash_text_bytes(value: &[u8]) -> String { format!("{:x}", Sha256::digest(value)) }

fn normalize_relative(path: &Path) -> String { path.to_string_lossy().replace('\\', "/") }
fn normalize_absolute(path: &Path) -> String { normalize_relative(path) }
fn hash_text(value: &str) -> String { format!("{:x}", Sha256::digest(value.as_bytes())) }

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

type KeyLocks = HashMap<String, Weak<Mutex<()>>>;
fn cache_key_lock(key: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<KeyLocks>> = OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    locks.retain(|_, weak| weak.strong_count() > 0);
    if let Some(lock) = locks.get(key).and_then(Weak::upgrade) { return lock; }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key.to_owned(), Arc::downgrade(&lock));
    lock
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("qlisa-cache-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn cache_hit_skips_generator_and_params_are_part_of_key() {
        let dir = temp_dir();
        let media = dir.join("clip.wav");
        fs::write(&media, b"source").unwrap();
        let scope = CacheScope { root: dir.join("cache"), project_dir: Some(dir.clone()) };
        assert_eq!(scope.get_or_generate(&media, "waveform", "bins=2000", "json", || Some(b"peaks".to_vec())), Some(b"peaks".to_vec()));
        assert_eq!(scope.get_or_generate(&media, "waveform", "bins=2000", "json", || panic!("hit must skip generation")), Some(b"peaks".to_vec()));
        assert_eq!(scope.get_or_generate(&media, "waveform", "bins=16000", "json", || Some(b"detail".to_vec())), Some(b"detail".to_vec()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_entries_are_misses_and_regenerate() {
        let dir = temp_dir();
        let media = dir.join("clip.wav");
        fs::write(&media, b"source").unwrap();
        let scope = CacheScope { root: dir.join("cache"), project_dir: Some(dir.clone()) };
        let first = scope.get_or_generate(&media, "waveform", "bins=2", "json", || Some(b"one".to_vec())).unwrap();
        let path = fs::read_dir(scope.root()).unwrap().next().unwrap().unwrap().path();
        fs::write(path, b"broken").unwrap();
        let second = scope.get_or_generate(&media, "waveform", "bins=2", "json", || Some(b"two".to_vec())).unwrap();
        assert_eq!(first, b"one");
        assert_eq!(second, b"two");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_change_invalidates_and_promotion_rekeys_relative_identity() {
        let dir = temp_dir();
        let old_project = dir.join("old");
        let new_project = dir.join("new");
        fs::create_dir_all(old_project.join("media")).unwrap();
        fs::create_dir_all(new_project.join("media")).unwrap();
        let old_media = old_project.join("media/clip.wav");
        fs::write(&old_media, b"source").unwrap();
        let old_scope = CacheScope { root: old_project.join("show.qlisa.cache"), project_dir: Some(old_project.clone()) };
        let new_scope = CacheScope { root: new_project.join("show.qlisa.cache"), project_dir: Some(new_project.clone()) };
        old_scope.get_or_generate(&old_media, "waveform", "bins=2000", "json", || Some(b"peaks".to_vec()));
        // Save As changes cache scope while this source remains outside the
        // destination project directory, so its new identity becomes absolute.
        old_scope.promote_to(&new_scope);
        assert_eq!(new_scope.get_or_generate(&old_media, "waveform", "bins=2000", "json", || panic!("promoted entry must hit")), Some(b"peaks".to_vec()));
        fs::write(&old_media, b"replacement with new size").unwrap();
        assert_eq!(new_scope.get_or_generate(&old_media, "waveform", "bins=2000", "json", || Some(b"fresh".to_vec())), Some(b"fresh".to_vec()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn same_size_source_with_new_mtime_misses() {
        let dir = temp_dir();
        let media = dir.join("clip.wav");
        fs::write(&media, b"first!").unwrap();
        let scope = CacheScope { root: dir.join("cache"), project_dir: Some(dir.clone()) };
        scope.get_or_generate(&media, "waveform", "bins=2", "json", || Some(b"old".to_vec()));
        let previous = fs::metadata(&media).unwrap().modified().unwrap();
        fs::write(&media, b"other!").unwrap();
        OpenOptions::new().write(true).open(&media).unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(previous + std::time::Duration::from_secs(2)))
            .unwrap();
        assert!(scope.has_stale_entry(&media, "waveform", "json"));
        assert_eq!(scope.get_or_generate(&media, "waveform", "bins=1200", "json", || Some(b"new-detail".to_vec())), Some(b"new-detail".to_vec()));
        assert_eq!(scope.get_or_generate(&media, "waveform", "bins=2", "json", || Some(b"new".to_vec())), Some(b"new".to_vec()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saved_project_scope_reuses_entries_after_reopen() {
        let dir = temp_dir();
        let project = dir.join("show.qlisa");
        let media = dir.join("clip.wav");
        fs::write(&project, "{}").unwrap();
        fs::write(&media, b"source").unwrap();
        let opened = CacheScope::for_project(&project);
        opened.get_or_generate(&media, "waveform", "bins=2", "json", || Some(b"peaks".to_vec()));
        let reopened = CacheScope::for_project(&project);
        assert_eq!(reopened.get_or_generate(&media, "waveform", "bins=2", "json", || panic!("reopened project must reuse cache")), Some(b"peaks".to_vec()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn first_save_promotes_only_this_unsaved_workspace_scope() {
        let dir = temp_dir();
        let project = dir.join("show.qlisa");
        let media = dir.join("media/clip.wav");
        fs::create_dir_all(media.parent().unwrap()).unwrap();
        fs::write(&media, b"source").unwrap();
        let unsaved = CacheScope::at(dir.join("unsaved-workspace-scope"), None);
        let saved = CacheScope::for_project(&project);
        unsaved.get_or_generate(&media, "thumbnail", "seek=false;width=400", "jpeg", || Some(b"jpeg".to_vec()));
        unsaved.promote_to(&saved);
        assert_eq!(saved.get_or_generate(&media, "thumbnail", "seek=false;width=400", "jpeg", || panic!("first save must promote cache")), Some(b"jpeg".to_vec()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn collect_promotion_remaps_copied_media_even_when_mtime_changes() {
        let dir = temp_dir();
        let source_project = dir.join("source");
        let target_project = dir.join("target");
        fs::create_dir_all(source_project.join("media")).unwrap();
        fs::create_dir_all(target_project.join("media")).unwrap();
        let source_media = source_project.join("media/clip.wav");
        let target_media = target_project.join("media/clip.wav");
        fs::write(&source_media, b"source").unwrap();
        fs::copy(&source_media, &target_media).unwrap();
        let copied_time = fs::metadata(&target_media).unwrap().modified().unwrap();
        OpenOptions::new().write(true).open(&target_media).unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(copied_time + std::time::Duration::from_secs(2)))
            .unwrap();
        let source_scope = CacheScope { root: source_project.join("show.qlisa.cache"), project_dir: Some(source_project.clone()) };
        let target_scope = CacheScope { root: target_project.join("show.qlisa.cache"), project_dir: Some(target_project.clone()) };
        source_scope.get_or_generate(&source_media, "waveform", "bins=2", "json", || Some(b"peaks".to_vec()));
        let map = HashMap::from([(source_media.clone(), target_media.clone())]);
        source_scope.promote_collected(&target_scope, &map);
        assert_eq!(target_scope.get_or_generate(&target_media, "waveform", "bins=2", "json", || panic!("collected cache must hit")), Some(b"peaks".to_vec()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn payload_checksum_detects_corruption_and_pruning_only_removes_owned_entries() {
        let dir = temp_dir();
        let media = dir.join("clip.wav");
        fs::write(&media, b"source").unwrap();
        let scope = CacheScope { root: dir.join("cache"), project_dir: Some(dir.clone()) };
        scope.get_or_generate(&media, "waveform", "bins=2", "json", || Some(b"payload".to_vec()));
        let entry_path = fs::read_dir(scope.root()).unwrap().next().unwrap().unwrap().path();
        let mut entry = read_any_entry(&entry_path).unwrap();
        entry.payload[0] ^= 0xff;
        let header = serde_json::to_vec(&entry.header).unwrap();
        let mut damaged = MAGIC.to_vec();
        damaged.extend_from_slice(&(header.len() as u32).to_le_bytes());
        damaged.extend_from_slice(&header);
        damaged.extend_from_slice(&entry.payload);
        fs::write(&entry_path, damaged).unwrap();
        assert!(read_any_entry(&entry_path).is_none());
        fs::remove_file(&entry_path).unwrap();

        let oldest = scope.root.join(format!("{}.qcache", "a".repeat(64)));
        let newer = scope.root.join(format!("{}.qcache", "b".repeat(64)));
        let unrelated = scope.root.join("keep.qcache");
        for path in [&oldest, &newer] { fs::write(path, b"123456").unwrap(); }
        fs::write(&unrelated, b"keep").unwrap();
        let old_time = SystemTime::now() - std::time::Duration::from_secs(60);
        OpenOptions::new().write(true).open(&oldest).unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old_time)).unwrap();
        prune_scope_limits(scope.root(), 10, 8);
        assert!(!oldest.exists());
        assert!(newer.exists());
        assert!(unrelated.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unwritable_cache_scope_returns_generated_data() {
        let dir = temp_dir();
        let media = dir.join("clip.wav");
        let root_file = dir.join("cache-is-a-file");
        fs::write(&media, b"source").unwrap();
        fs::write(&root_file, b"not a directory").unwrap();
        let scope = CacheScope { root: root_file, project_dir: Some(dir.clone()) };
        assert_eq!(scope.get_or_generate(&media, "thumbnail", "width=400", "jpeg", || Some(b"generated".to_vec())), Some(b"generated".to_vec()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn moved_project_and_relative_media_keep_cache_key() {
        let dir = temp_dir();
        let old_project = dir.join("old");
        fs::create_dir_all(old_project.join("media")).unwrap();
        let media = old_project.join("media/clip.wav");
        fs::write(&media, b"source").unwrap();
        let old_scope = CacheScope { root: old_project.join("show.qlisa.cache"), project_dir: Some(old_project.clone()) };
        old_scope.get_or_generate(&media, "waveform", "bins=2000", "json", || Some(b"peaks".to_vec()));
        let new_project = dir.join("moved");
        fs::rename(&old_project, &new_project).unwrap();
        let new_media = new_project.join("media/clip.wav");
        let moved_scope = CacheScope { root: new_project.join("show.qlisa.cache"), project_dir: Some(new_project) };
        assert_eq!(moved_scope.get_or_generate(&new_media, "waveform", "bins=2000", "json", || panic!("relocated cache must hit")), Some(b"peaks".to_vec()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_requests_generate_once() {
        let dir = temp_dir();
        let media = dir.join("clip.wav");
        fs::write(&media, b"source").unwrap();
        let scope = Arc::new(CacheScope { root: dir.join("cache"), project_dir: Some(dir.clone()) });
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let threads: Vec<_> = (0..8).map(|_| {
            let scope = scope.clone();
            let media = media.clone();
            let count = count.clone();
            std::thread::spawn(move || scope.get_or_generate(&media, "waveform", "bins=2", "json", || {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(10));
                Some(b"peaks".to_vec())
            }))
        }).collect();
        for thread in threads { assert_eq!(thread.join().unwrap(), Some(b"peaks".to_vec())); }
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
