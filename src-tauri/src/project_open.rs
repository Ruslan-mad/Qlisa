//! File-open requests delivered by the operating system or a second instance.
//!
//! The frontend owns workspace loading so it can apply its unsaved-work guard.
//! This queue only transports paths to the main window.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use tauri::{AppHandle, Emitter, State};

pub const PROJECT_OPEN_REQUESTED_EVENT: &str = "project-open-requested";

#[derive(Default)]
pub struct PendingProjectOpens(pub Mutex<VecDeque<PathBuf>>);

impl PendingProjectOpens {
    pub fn from_startup_args<I, S>(args: I, cwd: &Path) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut pending = VecDeque::new();
        for arg in args {
            let path = PathBuf::from(arg.as_ref());
            if is_supported_project_path(&path) {
                pending.push_back(resolve_path(path, cwd));
            }
        }
        Self(Mutex::new(pending))
    }

    pub fn enqueue_instance_args(&self, args: &[String], cwd: &str) -> bool {
        let cwd = Path::new(cwd);
        let mut added = false;
        if let Ok(mut pending) = self.0.lock() {
            for arg in args {
                let path = PathBuf::from(arg);
                if is_supported_project_path(&path) {
                    pending.push_back(resolve_path(path, cwd));
                    added = true;
                }
            }
        }
        added
    }

    fn drain(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|mut pending| {
                pending
                    .drain(..)
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn is_supported_project_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("qlisa" | "inkue" | "wincue")
    )
}

fn resolve_path(path: PathBuf, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

#[tauri::command]
pub fn drain_pending_project_opens(pending: State<'_, Arc<PendingProjectOpens>>) -> Vec<String> {
    pending.drain()
}

pub fn emit_project_open_requested(app: &AppHandle) {
    let _ = app.emit(PROJECT_OPEN_REQUESTED_EVENT, ());
}

#[cfg(test)]
mod tests {
    use super::PendingProjectOpens;
    use std::path::Path;

    #[test]
    fn startup_args_accept_current_and_legacy_extensions_only() {
        let cwd = std::env::temp_dir().join("Qlisa Shows");
        let pending = PendingProjectOpens::from_startup_args(
            [
                "show.qlisa",
                "legacy.inkue",
                "legacy.wincue",
                "notes.json",
                "without-extension",
            ],
            &cwd,
        );

        assert_eq!(
            pending.drain(),
            vec![
                cwd.join("show.qlisa").to_string_lossy().into_owned(),
                cwd.join("legacy.inkue").to_string_lossy().into_owned(),
                cwd.join("legacy.wincue").to_string_lossy().into_owned(),
            ]
        );
    }

    #[test]
    fn startup_args_keep_unicode_spaces_and_absolute_paths() {
        let cwd = std::env::temp_dir().join("Рабочие файлы");
        let absolute_project = std::env::temp_dir().join("Концерт в Москве.qlisa");
        let pending = PendingProjectOpens::from_startup_args(
            [
                absolute_project.as_os_str(),
                Path::new("Сцена 2.inkue").as_os_str(),
            ],
            &cwd,
        );

        assert_eq!(
            pending.drain(),
            vec![
                absolute_project.to_string_lossy().into_owned(),
                cwd.join("Сцена 2.inkue").to_string_lossy().into_owned()
            ]
        );
    }

    #[test]
    fn second_instance_args_are_queued_and_drained_once() {
        let pending = PendingProjectOpens::default();
        let cwd = std::env::temp_dir().join("Другое окно");
        let cwd = cwd.to_string_lossy().into_owned();

        assert!(pending.enqueue_instance_args(
            &[
                "Открыть проект.qlisa".to_string(),
                "Архив.wincue".to_string(),
                "ignored.txt".to_string(),
            ],
            &cwd,
        ));
        assert_eq!(
            pending.drain(),
            vec![
                Path::new(&cwd)
                    .join("Открыть проект.qlisa")
                    .to_string_lossy()
                    .into_owned(),
                Path::new(&cwd)
                    .join("Архив.wincue")
                    .to_string_lossy()
                    .into_owned()
            ]
        );
        assert_eq!(pending.drain(), Vec::<String>::new());
        assert!(!pending.enqueue_instance_args(&["ignored.txt".to_string()], &cwd));
    }
}
