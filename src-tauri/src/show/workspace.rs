//! [`Workspace`] — the top-level save unit for a Qlisa show.
//!
//! Corresponds to a `.qlisa` project file on disk. Legacy `.inkue` files use
//! the same JSON format and remain loadable.

/// Bumped whenever the project JSON format gains a breaking change.
/// Files written by newer Qlisa versions (schema > this) are rejected
/// at load time to prevent silent data corruption.
pub const SCHEMA_VERSION: u32 = 1;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    cue::{registry::CueRegistry, traits::Cue, types::CueType},
    engine::{audio_input::InputPatch, device_manager::{OutputPatch, OutputPatchKind}, dmx_sink::UniverseOutput, fixture::{FixtureGroup, PatchedFixture}, osc_patch::OscPatch},
    preferences::AppPreferences,
};

use super::cue_list::CueList;

// ---------------------------------------------------------------------------
// Path helpers — keep file paths relative in the project JSON so workspaces
// are portable across machines and drive letters.
// ---------------------------------------------------------------------------

/// Recursively walk a cues JSON array and convert absolute `file_path` values
/// to paths relative to `base` (the directory containing the project file).
fn relativize_paths(value: &mut serde_json::Value, base: &std::path::Path) {
    match value {
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                relativize_paths(item, base);
            }
        }
        serde_json::Value::Object(obj) => {
            if let Some(serde_json::Value::String(p)) = obj.get("file_path") {
                let path = std::path::Path::new(p.as_str());
                if path.is_absolute() {
                    if let Ok(rel) = path.strip_prefix(base) {
                        // Use forward slashes so the file is readable on any OS.
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        obj.insert("file_path".into(), serde_json::Value::String(rel_str));
                    }
                    // If strip_prefix fails (file is on a different drive), keep absolute.
                }
            }
            // Recurse into group children.
            if let Some(children) = obj.get_mut("children") {
                relativize_paths(children, base);
            }
        }
        _ => {}
    }
}

/// Recursively walk a cues JSON array and resolve relative `file_path` values
/// to absolute paths using `base` (the directory containing the project file).
fn absolutize_paths(value: &mut serde_json::Value, base: &std::path::Path) {
    match value {
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                absolutize_paths(item, base);
            }
        }
        serde_json::Value::Object(obj) => {
            if let Some(serde_json::Value::String(p)) = obj.get("file_path") {
                let path = std::path::Path::new(p.as_str());
                if path.is_relative() && !p.is_empty() {
                    let abs = base.join(path);
                    obj.insert("file_path".into(),
                        serde_json::Value::String(abs.to_string_lossy().into_owned()));
                }
            }
            if let Some(children) = obj.get_mut("children") {
                absolutize_paths(children, base);
            }
        }
        _ => {}
    }
}

/// Recursively walk cue JSON and replace `file_path` values using `path_map`
/// (absolute path → new relative path).  Paths not present in the map are
/// left unchanged so the subsequent `relativize_paths` pass can handle them.
fn remap_paths(value: &mut serde_json::Value, path_map: &HashMap<PathBuf, String>) {
    match value {
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                remap_paths(item, path_map);
            }
        }
        serde_json::Value::Object(obj) => {
            if let Some(serde_json::Value::String(p)) = obj.get("file_path") {
                let path = PathBuf::from(p.as_str());
                if let Some(new_rel) = path_map.get(&path) {
                    obj.insert("file_path".into(), serde_json::Value::String(new_rel.clone()));
                }
            }
            if let Some(children) = obj.get_mut("children") {
                remap_paths(children, path_map);
            }
        }
        _ => {}
    }
}

/// Count how many cues a `cues` JSON array *intends* to contain, descending
/// recursively into group `children`.  Each array element is one intended cue.
fn count_intended_cues(cues: Option<&serde_json::Value>) -> usize {
    let Some(arr) = cues.and_then(|v| v.as_array()) else { return 0 };
    arr.iter()
        .map(|item| 1 + count_intended_cues(item.get("children")))
        .sum()
}

/// Count how many cues actually loaded, descending into group children — the
/// symmetric counterpart to [`count_intended_cues`].
fn count_loaded_cues(cues: &[Box<dyn Cue>]) -> usize {
    cues.iter()
        .map(|c| 1 + c.child_cues().map(count_loaded_cues).unwrap_or(0))
        .sum()
}

fn sanitize_for_filename(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect();
    s.trim().to_string()
}

fn unique_filename(orig: &str, used: &mut HashSet<String>) -> String {
    if used.insert(orig.to_string()) {
        return orig.to_string();
    }
    let (stem, ext) = orig
        .rfind('.')
        .map_or((orig, ""), |i| (&orig[..i], &orig[i..]));
    let mut n = 1u32;
    loop {
        let candidate = format!("{stem}_{n}{ext}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// Recursively collect `(cue_type, abs_path)` for every cue that carries a
/// media file, including those nested inside groups.
fn collect_all_media_paths(
    cues: &[Box<dyn Cue>],
    out: &mut Vec<(CueType, PathBuf)>,
) {
    for cue in cues {
        if let ct @ (CueType::Audio | CueType::Video | CueType::Image | CueType::MidiFile) =
            cue.cue_type()
        {
            if let Some(path) = cue.media_file_path() {
                if !path.as_os_str().is_empty() {
                    out.push((ct, path.to_path_buf()));
                }
            }
        }
        if let Some(children) = cue.child_cues() {
            collect_all_media_paths(children, out);
        }
    }
}

// ---------------------------------------------------------------------------
// Collect & Save report
// ---------------------------------------------------------------------------

/// Result returned by [`Workspace::collect_and_save`].
#[derive(Debug, Serialize)]
pub struct CollectReport {
    /// Absolute path to the newly created `.qlisa` file.
    pub workspace_path: String,
    /// Number of media files successfully copied to the new location.
    pub files_copied: u32,
    /// Number of files already at their destination (source == destination).
    pub files_skipped: u32,
    /// Paths of files referenced by cues but missing from disk.
    pub files_missing: Vec<String>,
}

/// Serialisable workspace metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMetadata {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

impl WorkspaceMetadata {
    fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            name: name.into(),
            created_at: now,
            modified_at: now,
        }
    }
}

/// The workspace — a complete show document.
pub struct Workspace {
    pub metadata: WorkspaceMetadata,
    /// All cue lists in this workspace.
    pub cue_lists: Vec<CueList>,
    /// ID of the currently active cue list.
    pub active_cue_list_id: Uuid,
    /// Output patch table (shared across cue lists).
    pub output_patches: Vec<OutputPatch>,
    /// ID of the default output patch.
    pub default_output_patch_id: Option<Uuid>,
    /// OSC send patch table.
    pub osc_patches: Vec<OscPatch>,
    /// Live audio input patch table (Mic Cues).
    pub input_patches: Vec<InputPatch>,
    /// DMX universe → destination mapping (sACN / Art-Net outputs).
    pub universe_outputs: Vec<UniverseOutput>,
    /// Lighting fixture patch (named instruments at DMX addresses).
    pub fixtures: Vec<PatchedFixture>,
    /// Fixture groups (drive several fixtures from one Light Cue control).
    pub fixture_groups: Vec<FixtureGroup>,
    /// Application-wide preferences (audio engine, defaults, …).
    pub preferences: AppPreferences,
    /// Path to the project file on disk, if it has been saved.
    pub file_path: Option<PathBuf>,
    /// Whether the workspace has unsaved changes.
    pub is_modified: bool,
    /// Monotonic counter bumped on every mutation ([`mark_modified`]).  The
    /// autosave thread snapshots the workspace only when this changes, so an
    /// idle show is never re-serialised.  Not persisted.
    pub revision: u64,
    /// Number of cues that were present in the file but could **not** be loaded
    /// (unknown type or corrupt data) and were therefore skipped.  Transient
    /// (never serialised); read once after load so the operator can be warned
    /// that part of the show is missing instead of it vanishing silently.
    pub cues_skipped_on_load: usize,
}

impl Workspace {
    /// Create a new, empty workspace with one default cue list.
    pub fn new(name: impl Into<String>) -> Self {
        Self::new_with_preferences(name, AppPreferences::default())
    }

    /// Create a new workspace while mirroring the authoritative machine-wide
    /// Preferences into its legacy/runtime snapshot.
    pub fn new_with_preferences(name: impl Into<String>, preferences: AppPreferences) -> Self {
        let mut default_list = CueList::new("Cue List 1");
        default_list.auto_renumber = preferences.general.auto_renumber_on_reorder;
        let active_id = default_list.id;
        let mut workspace = Self {
            metadata: WorkspaceMetadata::new(name),
            cue_lists: vec![default_list],
            active_cue_list_id: active_id,
            output_patches: vec![OutputPatch::main()],
            default_output_patch_id: Some(OutputPatch::MAIN_ID),
            osc_patches: Vec::new(),
            input_patches: Vec::new(),
            universe_outputs: Vec::new(),
            fixtures: Vec::new(),
            fixture_groups: Vec::new(),
            preferences,
            file_path: None,
            is_modified: false,
            revision: 0,
            cues_skipped_on_load: 0,
        };
        workspace.ensure_main_audio_bus();
        workspace
    }

    /// Normalize the compatible Output Patch table into exclusive audio
    /// buses. Legacy files had no role field, so their default patch becomes
    /// Main in place and keeps its UUID so cue assignments remain valid.
    pub fn ensure_main_audio_bus(&mut self) {
        if self.output_patches.is_empty() {
            self.output_patches.push(OutputPatch::main());
        }
        let main_id = self
            .output_patches
            .iter()
            .find(|patch| patch.is_main())
            .map(|patch| patch.id)
            .or(self.default_output_patch_id)
            .or_else(|| self.output_patches.first().map(|patch| patch.id))
            .unwrap_or(OutputPatch::MAIN_ID);
        let mut found = false;
        for patch in &mut self.output_patches {
            if patch.id == main_id && !found {
                patch.kind = OutputPatchKind::Main;
                patch.enabled = true;
                found = true;
            } else {
                patch.kind = OutputPatchKind::Aux;
            }
        }
        if !found {
            self.output_patches.push(OutputPatch::main());
        }
        self.default_output_patch_id = Some(main_id);
    }

    /// Import legacy physical fields into machine-local bindings and refresh
    /// the runtime patch table from `audio.json`. New workspace files carry
    /// logical bus IDs only, while old files remain readable without a manual
    /// re-patch step.
    pub fn apply_machine_audio_bindings(
        &mut self,
        config: &mut crate::preferences::MachineAudioConfig,
    ) -> bool {
        self.ensure_main_audio_bus();
        let mut changed = false;
        for patch in &mut self.output_patches {
            if patch.is_main() {
                if config.device_id.is_none() && !patch.device_id.is_empty() {
                    config.device_id = Some(patch.device_id.clone());
                    changed = true;
                }
                continue;
            }
            if let Some(binding) = config.aux_buses.iter().find(|b| b.bus_id == patch.id) {
                patch.device_id = binding.device_id.clone();
                patch.channels = binding.channels.clone();
            } else if !patch.device_id.is_empty() || !patch.channels.is_empty() {
                config.aux_buses.push(crate::preferences::MachineAudioBus {
                    bus_id: patch.id,
                    device_id: patch.device_id.clone(),
                    channels: patch.channels.clone(),
                });
                changed = true;
            }
        }
        changed
    }

    /// Return the portable logical bus table. Physical bindings are written
    /// to the machine audio file instead of traveling with a show.
    fn logical_output_patches(&self) -> Vec<OutputPatch> {
        self.output_patches
            .iter()
            .cloned()
            .map(|mut patch| {
                patch.device_id.clear();
                if !patch.is_main() {
                    patch.channels.clear();
                }
                patch
            })
            .collect()
    }

    /// Mark the workspace as modified (unsaved changes exist).
    pub fn mark_modified(&mut self) {
        self.is_modified = true;
        self.revision = self.revision.wrapping_add(1);
        self.metadata.modified_at = Utc::now();
    }

    /// Push the `auto_renumber_on_reorder` preference onto every cue list's
    /// runtime `auto_renumber` flag. Call after loading a workspace or changing
    /// the preference; structural mutations then read the already-synced flag.
    pub fn sync_auto_renumber(&mut self) {
        let auto = self.preferences.general.auto_renumber_on_reorder;
        for cl in &mut self.cue_lists {
            cl.auto_renumber = auto;
        }
    }

    /// The active cue list, identified by `active_cue_list_id`.
    pub fn active_cue_list(&self) -> Option<&CueList> {
        self.cue_lists.iter().find(|cl| cl.id == self.active_cue_list_id)
    }

    /// Mutable access to the active cue list.
    pub fn active_cue_list_mut(&mut self) -> Option<&mut CueList> {
        let id = self.active_cue_list_id;
        self.cue_lists.iter_mut().find(|cl| cl.id == id)
    }

    /// Look up any cue list by its ID.
    pub fn cue_list_by_id(&self, id: Uuid) -> Option<&CueList> {
        self.cue_lists.iter().find(|cl| cl.id == id)
    }

    /// Mutable access to any cue list by its ID.
    pub fn cue_list_by_id_mut(&mut self, id: Uuid) -> Option<&mut CueList> {
        self.cue_lists.iter_mut().find(|cl| cl.id == id)
    }

    // -----------------------------------------------------------------------
    // Persistence
    // -----------------------------------------------------------------------

    /// Serialise the workspace to a JSON string, with file paths made relative
    /// to `save_path` so the project file is portable.
    fn to_json(&self, save_path: &Path) -> Result<String> {
        let base = save_path.parent();

        let mut cue_lists_json: Vec<serde_json::Value> = self
            .cue_lists
            .iter()
            .map(|cl| cl.to_json())
            .collect();

        if let Some(base_dir) = base {
            for cl in &mut cue_lists_json {
                if let Some(cues) = cl.get_mut("cues") {
                    relativize_paths(cues, base_dir);
                }
            }
        }

        let doc = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "workspace": self.metadata,
            "output_patches": self.logical_output_patches(),
            "default_output_patch": self.default_output_patch_id,
            "osc_patches": self.osc_patches,
            "input_patches": self.input_patches,
            "universe_outputs": self.universe_outputs,
            "fixtures": self.fixtures,
            "fixture_groups": self.fixture_groups,
            "preferences": self.preferences,
            "cue_lists": cue_lists_json,
            "active_cue_list_id": self.active_cue_list_id,
        });

        serde_json::to_string_pretty(&doc).context("Failed to serialize workspace")
    }

    /// Serialise the workspace for the crash-recovery snapshot.
    ///
    /// Differs from [`to_json`](Self::to_json) in two ways: media `file_path`s are
    /// kept **absolute** (the recovery file lives in the per-user config dir, not
    /// beside the show's media, so relative paths would not resolve), and the
    /// original `.inkue` path is embedded under `recovery_original_path` so a
    /// restore can target the same file.  Compact (not pretty) since it is
    /// rewritten every few seconds.
    pub fn to_recovery_json(&self) -> Result<String> {
        let cue_lists_json: Vec<serde_json::Value> =
            self.cue_lists.iter().map(|cl| cl.to_json()).collect();

        let doc = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "recovery_original_path": self.file_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            "workspace": self.metadata,
            "output_patches": self.logical_output_patches(),
            "default_output_patch": self.default_output_patch_id,
            "osc_patches": self.osc_patches,
            "input_patches": self.input_patches,
            "universe_outputs": self.universe_outputs,
            "fixtures": self.fixtures,
            "fixture_groups": self.fixture_groups,
            "preferences": self.preferences,
            "cue_lists": cue_lists_json,
            "active_cue_list_id": self.active_cue_list_id,
        });

        serde_json::to_string(&doc).context("Failed to serialize recovery workspace")
    }

    /// Save the workspace to the given path (or the previously saved path).
    pub fn save(&mut self, path: Option<PathBuf>) -> Result<()> {
        let target = path.or_else(|| self.file_path.clone())
            .ok_or_else(|| anyhow::anyhow!("No file path set for workspace"))?;

        let json = self.to_json(&target)?;
        std::fs::write(&target, json)
            .with_context(|| format!("Failed to write workspace to {}", target.display()))?;

        // Derive the workspace name from the filename stem so the title bar
        // reflects the saved file immediately.
        if let Some(stem) = target.file_stem().and_then(|s| s.to_str()) {
            self.metadata.name = stem.to_string();
        }
        self.file_path = Some(target);
        self.is_modified = false;
        Ok(())
    }

    /// Copy all media files referenced by this workspace into
    /// `{target_dir}/{workspace_name}/audio|video|images|midi/` and write a
    /// self-contained `.qlisa` file with updated relative paths.
    ///
    /// The workspace in memory is **not modified** — this is a pure export.
    pub fn collect_and_save(&self, target_dir: &Path) -> Result<CollectReport> {
        let raw_name = if self.metadata.name.is_empty() { "Untitled" } else { &self.metadata.name };
        let safe_name = {
            let s = sanitize_for_filename(raw_name);
            if s.is_empty() { "Untitled".into() } else { s }
        };

        let project_dir = target_dir.join(&safe_name);
        for sub in &["audio", "video", "images", "midi"] {
            std::fs::create_dir_all(project_dir.join(sub))
                .with_context(|| format!("Cannot create {}/{sub}", project_dir.display()))?;
        }

        // Collect (cue_type, abs_path) pairs, deduplicating by path.
        let mut raw: Vec<(CueType, PathBuf)> = Vec::new();
        for cl in &self.cue_lists {
            collect_all_media_paths(&cl.cues, &mut raw);
        }
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let deduped: Vec<(CueType, PathBuf)> =
            raw.into_iter().filter(|(_, p)| seen.insert(p.clone())).collect();

        // Per-subfolder used-filename sets for conflict resolution.
        let mut used: HashMap<&str, HashSet<String>> = [
            ("audio", HashSet::new()),
            ("video", HashSet::new()),
            ("images", HashSet::new()),
            ("midi", HashSet::new()),
        ]
            .into_iter()
            .collect();

        let mut path_map: HashMap<PathBuf, String> = HashMap::new();
        let mut files_copied = 0u32;
        let mut files_skipped = 0u32;
        let mut files_missing: Vec<String> = Vec::new();

        for (cue_type, abs_path) in &deduped {
            if !abs_path.exists() {
                files_missing.push(abs_path.to_string_lossy().into_owned());
                continue;
            }

            let subfolder = match cue_type {
                CueType::Audio => "audio",
                CueType::Video => "video",
                CueType::Image => "images",
                CueType::MidiFile => "midi",
                _ => continue,
            };

            let orig_name = abs_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());

            let dest_name = unique_filename(&orig_name, used.get_mut(subfolder).unwrap());
            let dest_path = project_dir.join(subfolder).join(&dest_name);
            let new_rel = format!("{subfolder}/{dest_name}");

            if abs_path == &dest_path {
                // Source == destination: file is already at the right place.
                files_skipped += 1;
            } else {
                std::fs::copy(abs_path, &dest_path).with_context(|| {
                    format!("Failed to copy {} → {}", abs_path.display(), dest_path.display())
                })?;
                files_copied += 1;
            }
            path_map.insert(abs_path.clone(), new_rel);
        }

        let new_project_path = project_dir.join(format!("{safe_name}.qlisa"));
        let json = self.to_json_collected(&new_project_path, &path_map)?;
        std::fs::write(&new_project_path, json)
            .with_context(|| format!("Failed to write {}", new_project_path.display()))?;

        Ok(CollectReport {
            workspace_path: new_project_path.to_string_lossy().into_owned(),
            files_copied,
            files_skipped,
            files_missing,
        })
    }

    /// Like [`Self::to_json`] but applies `path_map` before the standard
    /// `relativize_paths` step, so collected files get their new relative paths.
    fn to_json_collected(
        &self,
        save_path: &Path,
        path_map: &HashMap<PathBuf, String>,
    ) -> Result<String> {
        let base = save_path.parent();

        let mut cue_lists_json: Vec<serde_json::Value> =
            self.cue_lists.iter().map(|cl| cl.to_json()).collect();

        for cl in &mut cue_lists_json {
            if let Some(cues) = cl.get_mut("cues") {
                remap_paths(cues, path_map);
                if let Some(base_dir) = base {
                    relativize_paths(cues, base_dir);
                }
            }
        }

        let doc = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "workspace": self.metadata,
            "output_patches": self.logical_output_patches(),
            "default_output_patch": self.default_output_patch_id,
            "osc_patches": self.osc_patches,
            "input_patches": self.input_patches,
            "universe_outputs": self.universe_outputs,
            "fixtures": self.fixtures,
            "fixture_groups": self.fixture_groups,
            "preferences": self.preferences,
            "cue_lists": cue_lists_json,
            "active_cue_list_id": self.active_cue_list_id,
        });

        serde_json::to_string_pretty(&doc).context("Failed to serialize collected workspace")
    }

    /// Load a workspace from a project file (`.qlisa` or legacy `.inkue`).
    pub fn load(path: PathBuf, registry: &CueRegistry) -> Result<Self> {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read workspace file: {}", path.display()))?;

        let base_dir = path.parent().map(|p| p.to_path_buf());
        let mut ws = Self::from_json_str(&content, base_dir.as_deref(), registry)?;

        // Derive the name from the filename stem so it always matches the file,
        // even if the JSON still contains an older name (e.g. "Untitled").
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            ws.metadata.name = stem.to_string();
        }
        ws.file_path = Some(path);
        Ok(ws)
    }

    /// Parse a workspace document from a JSON string.
    ///
    /// `base_dir`, when `Some`, is the directory the document's media paths are
    /// relative to (used to absolutize them) — pass the project file's parent
    /// for a normal load.  Pass `None` when the document already stores absolute
    /// paths (the crash-recovery snapshot).  The returned workspace has
    /// `file_path: None` and `is_modified: false`; callers set those.
    pub fn from_json_str(
        content: &str,
        base_dir: Option<&Path>,
        registry: &CueRegistry,
    ) -> Result<Self> {
        let doc: serde_json::Value =
            serde_json::from_str(content).context("Invalid JSON in workspace file")?;

        let file_schema: u32 = doc
            .get("schema_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        if file_schema > SCHEMA_VERSION {
            anyhow::bail!(
                "This workspace was created with a newer version of Inkue \
                 (schema v{file_schema}, this app supports up to v{SCHEMA_VERSION}). \
                 Please update Inkue to open it."
            );
        }

        let metadata: WorkspaceMetadata = serde_json::from_value(
            doc.get("workspace")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Missing 'workspace' key"))?,
        )?;

        let patches_val = doc.get("output_patches").cloned().unwrap_or_default();
        let output_patches: Vec<OutputPatch> =
            serde_json::from_value(patches_val).unwrap_or_default();

        let osc_patches: Vec<OscPatch> = doc
            .get("osc_patches")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let input_patches: Vec<InputPatch> = doc
            .get("input_patches")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let universe_outputs: Vec<UniverseOutput> = doc
            .get("universe_outputs")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let fixtures: Vec<PatchedFixture> = doc
            .get("fixtures")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let fixture_groups: Vec<FixtureGroup> = doc
            .get("fixture_groups")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let default_patch: Option<Uuid> = doc
            .get("default_output_patch")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok());

        let mut preferences: AppPreferences = doc
            .get("preferences")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        // Keep old output_screen/output_transform readable while materialising
        // the stable named default destination for the multi-output runtime.
        preferences.display.migrate_outputs();

        let cue_lists_val = doc
            .get("cue_lists")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut cue_lists = Vec::new();
        for mut cl_val in cue_lists_val {
            if let Some(base) = base_dir {
                if let Some(cues) = cl_val.get_mut("cues") {
                    absolutize_paths(cues, base);
                }
            }
            cue_lists.push(CueList::from_json(cl_val, registry)?);
        }

        if cue_lists.is_empty() {
            cue_lists.push(CueList::new("Cue List 1"));
        }

        // Resolve the active cue list: try the saved ID, fall back to first list.
        let saved_active_id: Option<Uuid> = doc
            .get("active_cue_list_id")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok());
        let active_cue_list_id = saved_active_id
            .filter(|id| cue_lists.iter().any(|cl| cl.id == *id))
            .unwrap_or_else(|| cue_lists[0].id);

        // How many cues did the file intend to contain vs. how many actually
        // loaded?  The difference was dropped by the registry (unknown type or
        // corrupt data) — record it so the caller can warn the operator.
        let intended: usize = doc
            .get("cue_lists")
            .and_then(|v| v.as_array())
            .map(|lists| lists.iter().map(|cl| count_intended_cues(cl.get("cues"))).sum())
            .unwrap_or(0);
        let loaded: usize = cue_lists.iter().map(|cl| count_loaded_cues(&cl.cues)).sum();
        let cues_skipped_on_load = intended.saturating_sub(loaded);

        let mut workspace = Self {
            metadata,
            cue_lists,
            active_cue_list_id,
            output_patches,
            default_output_patch_id: default_patch,
            osc_patches,
            input_patches,
            universe_outputs,
            fixtures,
            fixture_groups,
            preferences,
            file_path: None,
            is_modified: false,
            revision: 0,
            cues_skipped_on_load,
        };
        workspace.ensure_main_audio_bus();
        Ok(workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::registry::CueRegistry;
    use crate::engine::device_manager::OutputPatchKind;
    use crate::engine::network_io::{NdiQuality, SrtMode};
    use crate::preferences::{OutputDestination, OutputSinkKind};

    fn test_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("qlisa-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("create test directory");
        path
    }

    #[test]
    fn new_workspace_applies_default_auto_renumber_to_its_initial_cue_list() {
        let workspace = Workspace::new("New show");
        assert!(workspace.preferences.general.auto_renumber_on_reorder);
        assert!(workspace.cue_lists[0].auto_renumber);
        assert_eq!(workspace.output_patches.iter().filter(|p| p.is_main()).count(), 1);
    }

    #[test]
    fn legacy_patch_table_migrates_default_to_main_without_changing_id() {
        let mut workspace = Workspace::new("Legacy show");
        let legacy = Uuid::new_v4();
        workspace.output_patches = vec![OutputPatch {
            id: legacy,
            name: "Old PA".into(),
            device_id: "machine-device".into(),
            channels: vec![2, 3],
            gain_db: 0.0,
            kind: OutputPatchKind::Aux,
            enabled: true,
        }];
        workspace.default_output_patch_id = Some(legacy);
        workspace.ensure_main_audio_bus();
        assert_eq!(workspace.output_patches[0].id, legacy);
        assert!(workspace.output_patches[0].is_main());
        assert_eq!(workspace.default_output_patch_id, Some(legacy));
    }

    #[test]
    fn workspace_json_does_not_carry_physical_aux_binding() {
        let mut workspace = Workspace::new("Portable show");
        let aux = OutputPatch {
            id: Uuid::new_v4(), name: "Side fill".into(), device_id: "usb:private".into(),
            channels: vec![6, 7], gain_db: 0.0, kind: OutputPatchKind::Aux, enabled: true,
        };
        workspace.output_patches.push(aux);
        let json = workspace.to_json(Path::new("portable.inkue")).expect("serialize");
        let saved = serde_json::from_str::<serde_json::Value>(&json).expect("json");
        let patch = &saved["output_patches"][1];
        assert_eq!(patch["device_id"], "");
        assert_eq!(patch["channels"], serde_json::json!([]));
    }

    #[test]
    fn save_keeps_the_existing_legacy_inkue_path() {
        let dir = test_dir("legacy-save");
        let legacy_path = dir.join("Сцена репетиции.inkue");
        let mut workspace = Workspace::new("Сцена репетиции");
        workspace.file_path = Some(legacy_path.clone());

        workspace.save(None).expect("save legacy project");

        assert!(legacy_path.is_file());
        assert_eq!(workspace.file_path.as_deref(), Some(legacy_path.as_path()));
        let contents = std::fs::read_to_string(&legacy_path).expect("read saved project");
        assert!(Workspace::from_json_str(&contents, Some(&dir), &CueRegistry::new()).is_ok());
        std::fs::remove_dir_all(dir).expect("remove test directory");
    }

    #[test]
    fn collect_and_save_writes_qlisa_in_unicode_path_with_spaces() {
        let test_root = test_dir("collect-save");
        let dir = test_root.join("Проект с пробелом");
        std::fs::create_dir_all(&dir).expect("create target directory");
        let workspace = Workspace::new("Спектакль русский");

        let report = workspace.collect_and_save(&dir).expect("collect and save");
        let expected = dir
            .join("Спектакль русский")
            .join("Спектакль русский.qlisa");
        assert_eq!(PathBuf::from(&report.workspace_path), expected);
        let contents = std::fs::read_to_string(&expected).expect("read collected project");
        assert!(Workspace::from_json_str(&contents, Some(expected.parent().unwrap()), &CueRegistry::new()).is_ok());

        std::fs::remove_file(&expected).expect("remove collected project");
        std::fs::remove_dir_all(test_root)
            .expect("remove test directory");
    }

    #[test]
    fn machine_binding_restores_aux_without_inventing_a_missing_binding() {
        let mut workspace = Workspace::new("Machine bindings");
        let aux_id = Uuid::new_v4();
        workspace.output_patches.push(OutputPatch {
            id: aux_id, name: "Side fill".into(), device_id: String::new(),
            channels: Vec::new(), gain_db: 0.0, kind: OutputPatchKind::Aux, enabled: true,
        });
        let mut config = crate::preferences::MachineAudioConfig::default();
        assert!(!workspace.apply_machine_audio_bindings(&mut config));
        let aux = workspace.output_patches.iter().find(|p| p.id == aux_id).unwrap();
        assert!(aux.device_id.is_empty());
        assert!(aux.channels.is_empty());

        config.aux_buses.push(crate::preferences::MachineAudioBus {
            bus_id: aux_id, device_id: "usb:local".into(), channels: vec![4, 5],
        });
        assert!(!workspace.apply_machine_audio_bindings(&mut config));
        let aux = workspace.output_patches.iter().find(|p| p.id == aux_id).unwrap();
        assert_eq!(aux.device_id, "usb:local");
        assert_eq!(aux.channels, vec![4, 5]);
    }

    #[test]
    fn workspace_roundtrip_preserves_network_output_destinations() {
        let mut workspace = Workspace::new("Network show");
        workspace.preferences.display.output_destinations = vec![
            OutputDestination {
                id: "main".into(),
                name: "Main".into(),
                sink_kind: OutputSinkKind::Display,
                network: Default::default(), monitor: None, floating_window: None,
                enabled: true, always_on_top: false, hide_cursor: false,
                transform: Default::default(), fullscreen_locked: true,
            },
            OutputDestination {
                id: "program-ndi".into(),
                name: "Program NDI".into(),
                sink_kind: OutputSinkKind::Ndi,
                enabled: true,
                network: crate::engine::network_io::NetworkOutputSettings {
                    ndi: crate::engine::network_io::NdiOutputSettings {
                        enabled: true,
                        stream_name: "Studio Program".into(),
                        quality: NdiQuality::LowBandwidth,
                        group: "Production".into(),
                    },
                    ..Default::default()
                },
                monitor: None, floating_window: None, always_on_top: false, hide_cursor: false,
                transform: Default::default(), fullscreen_locked: true,
            },
            OutputDestination {
                id: "program-srt".into(),
                name: "Program SRT".into(),
                sink_kind: OutputSinkKind::Srt,
                enabled: true,
                network: crate::engine::network_io::NetworkOutputSettings {
                    srt: crate::engine::network_io::SrtSettings {
                        enabled: true,
                        mode: SrtMode::Caller,
                        host: "192.0.2.20".into(),
                        port: 10000,
                        latency_ms: 240,
                        passphrase: Some("1234567890".into()),
                        stream_id: Some("show-42".into()),
                        payload_size: Some(1316),
                        too_late_packet_drop: false,
                        bitrate_kbps: Some(6000),
                        width: Some(1920),
                        height: Some(1080),
                        fps: Some(30),
                        codec: Some("h264".into()),
                    },
                    ..Default::default()
                },
                monitor: None, floating_window: None, always_on_top: false, hide_cursor: false,
                transform: Default::default(), fullscreen_locked: true,
            },
        ];
        workspace.preferences.display.default_output_id = "program-ndi".into();

        let json = workspace.to_json(Path::new("network-show.inkue")).expect("serialize");
        let restored = Workspace::from_json_str(&json, None, &CueRegistry::new()).expect("load");

        assert_eq!(restored.preferences.display.default_output_id, "program-ndi");
        assert_eq!(restored.preferences.display.output_destinations, workspace.preferences.display.output_destinations);
    }
}
