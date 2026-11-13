//! Commands that import static page documents as a Presentation cue tree.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{Emitter, Manager, State};
use uuid::Uuid;

use crate::{
    cue::{
        group_cue::GroupCue,
        image_cue::ImageCue,
        traits::Cue,
        types::{CueType, PresentationVideoMode},
        video_cue::VideoCue,
    },
    show::Workspace,
    state::AppState,
};

#[derive(Debug, Clone, Serialize)]
pub struct ImportedSlideVideo {
    pub name: String,
    pub status: String,
    pub cue_id: Option<String>,
    pub error: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportedSlide {
    pub number: usize,
    pub image_cue_id: String,
    pub image_path: String,
    pub videos: Vec<ImportedSlideVideo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentImportResult {
    pub cue_id: String,
    pub title: String,
    pub source: String,
    pub page_count: usize,
    pub video_count: usize,
    pub media_scan_error: Option<String>,
    pub warnings: Vec<String>,
    pub report: Vec<String>,
    pub slides: Vec<ImportedSlide>,
}

#[derive(Clone)]
struct ImportTarget {
    created_at: chrono::DateTime<chrono::Utc>,
    file_path: Option<PathBuf>,
    active_list_id: Uuid,
    revision: u64,
    parent_id: Option<Uuid>,
}

struct ImportDirectoryGuard {
    path: PathBuf,
    keep: bool,
}

impl Drop for ImportDirectoryGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn validate_target(workspace: &Workspace, target: &ImportTarget) -> Result<(), String> {
    if workspace.metadata.created_at != target.created_at
        || workspace.file_path != target.file_path
        || workspace.active_cue_list_id != target.active_list_id
        || workspace.revision != target.revision
    {
        return Err("Проект или список изменился во время импорта. Повторите импорт.".into());
    }
    let cue_list = workspace.active_cue_list().ok_or("Нет активного списка кью")?;
    if let Some(parent_id) = target.parent_id {
        let parent = cue_list.get_recursive(&parent_id).ok_or("Целевой контейнер удалён во время импорта")?;
        if !matches!(parent.cue_type(), CueType::Group | CueType::Presentation) {
            return Err("Целевой кью больше не является контейнером".into());
        }
    }
    Ok(())
}

fn validate_generated_path(path: &Path, root: &Path) -> Result<PathBuf, String> {
    let canonical_root = root.canonicalize().map_err(|error| format!("Не удалось проверить папку импорта: {error}"))?;
    let canonical = path.canonicalize().map_err(|error| format!("Подготовленный файл не найден: {} ({error})", path.display()))?;
    if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
        return Err(format!("Подготовленный файл вне папки импорта: {}", path.display()));
    }
    Ok(canonical)
}

fn presentation_layout_matches_video_aspect(
    layout: crate::cue::types::PresentationVideoLayout,
    display_aspect: Option<f64>,
) -> Result<(), &'static str> {
    let Some(display_aspect) = display_aspect.filter(|value| value.is_finite() && *value > 0.0) else {
        return Err("Не удалось проверить пропорции видео; доступен полноэкранный режим");
    };
    let frame_aspect = (layout.slide_width as f64 * layout.width)
        / (layout.slide_height as f64 * layout.height);
    if !frame_aspect.is_finite() || frame_aspect <= 0.0 {
        return Err("Не удалось проверить пропорции видео; доступен полноэкранный режим");
    }
    // MPV reports a floating-point display aspect. Allow only tiny metadata
    // rounding; a real aspect mismatch would add bars inside the slide frame.
    let relative_difference = (display_aspect - frame_aspect).abs() / frame_aspect;
    if relative_difference > 0.001 {
        return Err("Рамка меняет пропорции видео; доступен полноэкранный режим");
    }
    Ok(())
}

#[tauri::command]
pub async fn import_document(
    path: String,
    position: i32,
    parent_id: Option<String>,
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DocumentImportResult, String> {
    let source = PathBuf::from(path);
    if !source.is_file() {
        return Err("Файл документа не найден".into());
    }
    let parent_id = parent_id
        .map(|value| Uuid::parse_str(&value).map_err(|error| error.to_string()))
        .transpose()?;
    let target = {
        let workspace = state.workspace.lock().map_err(|error| error.to_string())?;
        let cue_list = workspace.active_cue_list().ok_or("Нет активного списка кью")?;
        if let Some(parent_id) = parent_id {
            let parent = cue_list.get_recursive(&parent_id).ok_or("Целевой контейнер не найден")?;
            if !matches!(parent.cue_type(), CueType::Group | CueType::Presentation) {
                return Err("Документ можно добавить только в Group или Presentation".into());
            }
        }
        ImportTarget {
            created_at: workspace.metadata.created_at.clone(),
            file_path: workspace.file_path.clone(),
            active_list_id: workspace.active_cue_list_id,
            revision: workspace.revision,
            parent_id,
        }
    };

    let app_data = app_handle.path().app_data_dir().map_err(|error| error.to_string())?;
    let imports_root = app_data.join("document-imports");
    std::fs::create_dir_all(&imports_root).map_err(|error| format!("Не удалось создать папку импортов: {error}"))?;
    let destination = imports_root.join(Uuid::new_v4().to_string());
    std::fs::create_dir(&destination).map_err(|error| format!("Не удалось создать папку документа: {error}"))?;
    let mut destination_guard = ImportDirectoryGuard { path: destination.clone(), keep: false };

    let worker_source = source.clone();
    let worker_destination = destination.clone();
    let output_engine = std::sync::Arc::clone(&state.output_engine);
    let prepared_result = tauri::async_runtime::spawn_blocking(move || {
        let prepared = crate::document_import::prepare_document(&worker_source, &worker_destination)
            .map_err(|error| error.to_string())?;
        let (mut media, extraction_warning) = if worker_source.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("pptx")) {
            match crate::presentation_media::extract_presentation_media(&worker_source, &worker_destination) {
                Ok(report) => (Some(report), None),
                Err(error) => (None, Some(format!("Видео в документе не удалось проверить или извлечь: {error}"))),
            }
        } else {
            (None, None)
        };
        let mut video_durations = std::collections::HashMap::<PathBuf, std::time::Duration>::new();
        if let Some(media) = media.as_mut() {
            let mpv = output_engine.try_mpv_lib_arc();
            let mut probe_messages = Vec::new();
            for slide in &mut media.slides {
                for video in &mut slide.videos {
                    if video.status != crate::presentation_media::PresentationMediaStatus::Added { continue; }
                    let Some(path) = video.path.as_deref() else {
                        video.status = crate::presentation_media::PresentationMediaStatus::Missing;
                        video.error = Some("Не найден файл извлечённого видео".into());
                        probe_messages.push((format!("Слайд {}: видео '{}' не добавлено — файл не найден.", slide.slide_number, video.name), format!("Слайд {}: видео '{}' не извлечено.", slide.slide_number, video.name)));
                        continue;
                    };
                    let Some(mpv) = mpv.as_deref() else {
                        video.status = crate::presentation_media::PresentationMediaStatus::Unsupported;
                        video.error = Some("Среда видеовоспроизведения недоступна для проверки файла".into());
                        probe_messages.push((format!("Слайд {}: видео '{}' извлечено, но не добавлено — среда видеовоспроизведения недоступна.", slide.slide_number, video.name), format!("Слайд {}: видео '{}' извлечено, но не добавлено.", slide.slide_number, video.name)));
                        continue;
                    };
                    match crate::engine::OutputEngine::probe_media_info(mpv, path) {
                        Some(info) if info.width.is_some_and(|value| value > 0)
                            && info.height.is_some_and(|value| value > 0)
                            && info.duration.is_some() => {
                                if let Some(duration) = info.duration {
                                    if let Ok(canonical) = path.canonicalize() { video_durations.insert(canonical, duration); }
                                }
                                if video.supported {
                                    if let Some(layout) = video.layout {
                                        if let Err(error) = presentation_layout_matches_video_aspect(layout, info.display_aspect) {
                                            video.supported = false;
                                            video.error = Some(error.into());
                                            probe_messages.push((
                                                format!("Слайд {}: видео '{}' добавлено на весь экран — {}.", slide.slide_number, video.name, error),
                                                format!("Слайд {}: точное размещение видео '{}' не поддерживается — {}.", slide.slide_number, video.name, error),
                                            ));
                                        }
                                    }
                                }
                            }
                        _ => {
                            video.status = crate::presentation_media::PresentationMediaStatus::Unsupported;
                            video.error = Some("Видео не прочиталось: не удалось получить размер кадра и длительность".into());
                            let _ = std::fs::remove_file(path);
                            probe_messages.push((format!("Слайд {}: видео '{}' извлечено, но не прочиталось.", slide.slide_number, video.name), format!("Слайд {}: видео '{}' не добавлено: файл не прочитался.", slide.slide_number, video.name)));
                        }
                    }
                }
            }
            for (warning, line) in probe_messages {
                media.warnings.push(warning);
                media.report.push(line);
            }
        }
        Ok::<_, String>((prepared, media, extraction_warning, video_durations))
    }).await.map_err(|error| format!("Ошибка фонового импорта: {error}"))?;

    let (prepared, media, extraction_warning, video_durations) = match prepared_result {
        Ok(result) => result,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&destination);
            return Err(error);
        }
    };
    if prepared.pages.is_empty() {
        let _ = std::fs::remove_dir_all(&destination);
        return Err("Документ не содержит страниц".into());
    }
    let pages = match prepared.pages.iter().map(|path| validate_generated_path(path, &destination)).collect::<Result<Vec<_>, _>>() {
        Ok(paths) => paths,
        Err(error) => { let _ = std::fs::remove_dir_all(&destination); return Err(error); }
    };

    let mut warnings = prepared.warnings;
    let mut report = Vec::new();
    let mut media_by_slide: std::collections::HashMap<usize, Vec<crate::presentation_media::ExtractedPresentationVideo>> = std::collections::HashMap::new();
    if let Some(error) = extraction_warning.as_ref() {
        warnings.push(error.clone());
        report.push(error.clone());
    }
    if let Some(media) = media {
        warnings.extend(media.warnings);
        report.extend(media.report);
        for slide in media.slides {
            media_by_slide.entry(slide.slide_number).or_default().extend(slide.videos);
        }
    } else if source.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("ppt")) {
        warnings.push("Для извлечения видео сохраните презентацию в формате PPTX или добавьте видео вручную.".into());
    }

    let mut presentation = GroupCue::new_presentation();
    presentation.set_name(prepared.title.clone());
    let preferences = state.global_preferences_snapshot()?;
    crate::commands::cue_cmds::apply_default_cue_color(&mut presentation, &preferences.general);
    let registry = state.registry.lock().map_err(|error| error.to_string())?;
    let presentation_id = presentation.id();
    let mut slides = Vec::with_capacity(pages.len());
    let mut preload_assignments = Vec::new();
    let mut video_count = 0usize;
    for (page_index, page_path) in pages.iter().enumerate() {
        let slide_number = page_index + 1;
        let mut image_json = ImageCue::new().serialize();
        crate::commands::cue_cmds::apply_initial_image_black_bars_default(
            &mut image_json,
            &page_path.to_string_lossy(),
            &preferences.general.default_black_bars,
        );
        image_json["file_path"] = serde_json::Value::String(page_path.to_string_lossy().into_owned());
        let mut image = registry.from_json(image_json).map_err(|error| error.to_string())?;
        image.set_name(format!("{} — страница {slide_number}", prepared.title));
        let image_id = image.id();
        crate::commands::cue_cmds::apply_default_cue_color(image.as_mut(), &preferences.general);
        let image = crate::commands::cue_cmds::apply_new_cue_output_defaults(image, &preferences.display, &registry)
            .map_err(|error| error.to_string())?;
        presentation.children.push(image);
        let mut slide = ImportedSlide {
            number: slide_number,
            image_cue_id: image_id.to_string(),
            image_path: page_path.to_string_lossy().into_owned(),
            videos: Vec::new(),
        };
        if let Some(videos) = media_by_slide.remove(&slide_number) {
            for video in videos {
                let name = video.name.clone();
                if video.status != crate::presentation_media::PresentationMediaStatus::Added {
                    slide.videos.push(ImportedSlideVideo {
                        name,
                        status: match video.status {
                            crate::presentation_media::PresentationMediaStatus::Missing => "missing",
                            _ => "unsupported",
                        }.into(),
                        cue_id: None,
                        error: video.error,
                        warning: None,
                    });
                    continue;
                }
                let Some(path) = video.path else {
                    slide.videos.push(ImportedSlideVideo { name, status: "missing".into(), cue_id: None, error: Some("Не найден локальный файл видео".into()), warning: None });
                    continue;
                };
                let video_path = match validate_generated_path(&path, &destination) {
                    Ok(path) => path,
                    Err(error) => {
                        slide.videos.push(ImportedSlideVideo { name, status: "missing".into(), cue_id: None, error: Some(error), warning: None });
                        continue;
                    }
                };
                let mut cue = VideoCue::new();
                cue.set_name(format!("{} — {}", slide_number, name));
                cue.file_path = Some(video_path.clone());
                cue.presentation_video_imported = true;
                cue.presentation_video_layout = video.layout;
                cue.presentation_video_layout_supported = video.supported && video.layout.is_some_and(|layout| layout.supports_exact_as_slide());
                cue.presentation_video_layout_error = video.error.clone().or_else(|| {
                    (!cue.presentation_video_layout_supported).then(|| "Исходное размещение невозможно точно повторить".into())
                });
                cue.presentation_video_mode = PresentationVideoMode::Fullscreen;
                crate::commands::cue_cmds::apply_default_cue_color(&mut cue, &preferences.general);
                let cue = crate::commands::cue_cmds::apply_new_video_black_bars_default(Box::new(cue), &preferences.general, &registry)
                    .map_err(|error| error.to_string())?;
                let mut cue = crate::commands::cue_cmds::apply_new_cue_output_defaults(cue, &preferences.display, &registry)
                    .map_err(|error| error.to_string())?;
                let cue_uuid = cue.id();
                let cue_id = cue_uuid.to_string();
                let duration = video_durations.get(&video_path).copied();
                if let Some(duration) = duration { cue.set_runtime_duration(duration); }
                presentation.children.push(cue);
                preload_assignments.push((cue_uuid, video_path, duration));
                video_count += 1;
                let layout_error = (!video.supported).then(|| video.error.unwrap_or_else(|| "Видео добавлено на весь экран; точная область слайда не поддерживается".into()));
                report.push(format!("Слайд {slide_number}: видео '{}' извлечено и добавлено как Video Cue.", name));
                slide.videos.push(ImportedSlideVideo { name, status: "added".into(), cue_id: Some(cue_id), error: None, warning: layout_error });
            }
        }
        slides.push(slide);
    }
    for (slide_number, videos) in media_by_slide {
        warnings.push(format!("Видео из слайда {slide_number} пропущено: номер слайда вне импортированного документа."));
        report.extend(videos.into_iter().map(|video| format!("Слайд {slide_number}: не добавлено видео {}", video.name)));
    }
    drop(registry);

    let result = DocumentImportResult {
        cue_id: presentation_id.to_string(),
        title: prepared.title,
        source: source.to_string_lossy().into_owned(),
        page_count: pages.len(),
        video_count,
        media_scan_error: extraction_warning,
        warnings,
        report,
        slides,
    };

    let insertion = (|| -> Result<(), String> {
        let mut workspace = state.workspace.lock().map_err(|error| error.to_string())?;
        validate_target(&workspace, &target)?;
        let snapshot = {
            let cue_list = workspace.active_cue_list().ok_or("Нет активного списка кью")?;
            super::undo_cmds::take_snapshot(cue_list)
        };
        state.undo_stack.lock().map_err(|error| error.to_string())?.push_action(snapshot);
        let cue_list = workspace.active_cue_list_mut().ok_or("Нет активного списка кью")?;
        if let Some(parent_id) = target.parent_id {
            let parent = cue_list.get_mut_recursive(&parent_id).ok_or("Целевой контейнер удалён")?;
            parent.add_child(Box::new(presentation), position).map_err(|error| error.to_string())?;
        } else {
            let index = if position < 0 { cue_list.cues.len() } else { position as usize };
            cue_list.insert(index, Box::new(presentation));
        }
        workspace.mark_modified();
        Ok(())
    })();
    if let Err(error) = insertion {
        let _ = std::fs::remove_dir_all(&destination);
        return Err(error);
    }
    destination_guard.keep = true;
    super::cue_cmds::schedule_video_preload(&state, app_handle.clone(), preload_assignments);
    let _ = app_handle.emit("workspace-modified", serde_json::json!({}));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::{group_cue::GroupCue, types::CueId};

    fn target_for(workspace: &Workspace, parent_id: Option<CueId>) -> ImportTarget {
        ImportTarget {
            created_at: workspace.metadata.created_at,
            file_path: workspace.file_path.clone(),
            active_list_id: workspace.active_cue_list_id,
            revision: workspace.revision,
            parent_id,
        }
    }

    #[test]
    fn import_target_rejects_workspace_changes_and_removed_parent() {
        let mut workspace = Workspace::new("Import test");
        let mut group = GroupCue::new();
        let group_id = group.id;
        workspace.active_cue_list_mut().unwrap().cues.push(Box::new(group));
        let target = target_for(&workspace, Some(group_id));
        validate_target(&workspace, &target).unwrap();

        workspace.active_cue_list_mut().unwrap().cues.clear();
        assert!(validate_target(&workspace, &target).is_err(), "removed import parent must reject the result");

        let target = target_for(&workspace, None);
        workspace.mark_modified();
        assert!(validate_target(&workspace, &target).is_err(), "a changed project revision must reject stale import output");
    }

    #[test]
    fn generated_import_paths_must_stay_under_the_uuid_folder() {
        let root = std::env::temp_dir().join(format!("qlisa-document-import-test-{}", Uuid::new_v4()));
        let outside = root.with_extension("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("page.png"), b"page").unwrap();
        std::fs::write(&outside, b"outside").unwrap();

        assert!(validate_generated_path(&root.join("page.png"), &root).is_ok());
        assert!(validate_generated_path(&outside, &root).is_err());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn as_slide_requires_matching_display_aspect() {
        let widescreen = crate::cue::types::PresentationVideoLayout {
            slide_width: 16,
            slide_height: 9,
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            rotation: 0,
            crop_left: 0.0,
            crop_top: 0.0,
            crop_right: 0.0,
            crop_bottom: 0.0,
        };
        assert!(presentation_layout_matches_video_aspect(widescreen, Some(16.0 / 9.0)).is_ok());
        assert!(presentation_layout_matches_video_aspect(widescreen, Some(4.0 / 3.0)).is_err());
        assert!(presentation_layout_matches_video_aspect(widescreen, None).is_err());
    }
}
