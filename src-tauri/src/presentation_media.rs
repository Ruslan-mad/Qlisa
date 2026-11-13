use crate::cue::types::PresentationVideoLayout;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use url::Url;
use zip::ZipArchive;

const MAX_ZIP_ENTRIES: usize = 100_000;
const MAX_PACKAGE_UNCOMPRESSED: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MEDIA_BYTES: u64 = 512 * 1024 * 1024;
const MAX_XML_BYTES: u64 = 16 * 1024 * 1024;
const MAX_XML_DEPTH: usize = 128;
const MAX_XML_NODES: usize = 250_000;
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "webm", "mkv", "avi", "mpg", "mpeg", "wmv", "ogv",
];

#[derive(Debug, Clone)]
pub struct ExtractedPresentationVideo {
    pub slide_number: usize,
    pub name: String,
    pub status: PresentationMediaStatus,
    pub path: Option<PathBuf>,
    pub layout: Option<PresentationVideoLayout>,
    pub supported: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationMediaStatus {
    Added,
    Missing,
    Unsupported,
}

#[derive(Debug, Clone, Default)]
pub struct PresentationMediaSlide {
    pub slide_number: usize,
    pub videos: Vec<ExtractedPresentationVideo>,
}

#[derive(Debug, Clone, Default)]
pub struct PresentationMediaReport {
    pub slides: Vec<PresentationMediaSlide>,
    pub warnings: Vec<String>,
    pub report: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct XmlNode {
    name: String,
    attrs: HashMap<String, String>,
    children: Vec<XmlNode>,
}

#[derive(Debug, Clone)]
struct Relationship {
    target: String,
    external: bool,
    rel_type: String,
}

#[derive(Debug, Clone)]
enum MediaSource {
    PackagePart(String),
    ExternalPath(PathBuf),
    Web(String),
}

#[derive(Debug, Clone)]
struct ShapeVideo {
    refs: Vec<ShapeVideoRef>,
    x: Option<i64>,
    y: Option<i64>,
    cx: Option<i64>,
    cy: Option<i64>,
    rotation: Option<i64>,
    crop: [f64; 4],
    grouped: bool,
    has_video_object: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShapeVideoRefKind {
    Video,
    Media,
    Hyperlink,
}

#[derive(Debug, Clone)]
struct ShapeVideoRef {
    rel_id: String,
    kind: ShapeVideoRefKind,
}

#[derive(Debug, Clone)]
struct VideoSourceCandidate {
    key: String,
    source: MediaSource,
    display: String,
    name: String,
    extension: Option<&'static str>,
}

/// Extract video media referenced by a PPTX without opening external URLs.
/// Output names are generated locally; package paths are never used as output paths.
pub fn extract_presentation_media(
    source: &Path,
    destination: &Path,
) -> Result<PresentationMediaReport, String> {
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension == "ppt" {
        return Ok(PresentationMediaReport {
            slides: Vec::new(),
            warnings: vec!["Формат PPT не поддерживает извлечение видео. Сохраните документ как PPTX или добавьте видео вручную.".to_owned()],
            report: vec![
                "Из PPT импортируются только статические страницы; видео добавляются вручную или после конвертации в PPTX.".to_owned(),
            ],
        });
    }
    if extension != "pptx" {
        return Err("Выберите файл PPTX или PPT.".to_owned());
    }

    let file =
        File::open(source).map_err(|error| format!("Не удалось открыть презентацию: {error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("Некорректный архив PPTX: {error}"))?;
    let package_parts = validate_archive(&mut archive)?;
    let presentation_xml = read_xml_part(&mut archive, "ppt/presentation.xml")?;
    let presentation_rels = read_relationships(&mut archive, "ppt/_rels/presentation.xml.rels")?;
    let presentation = parse_xml(&presentation_xml)?;
    let (slide_width, slide_height) = slide_size(&presentation);
    let slide_rel_ids = ordered_slide_relationships(&presentation);

    fs::create_dir_all(destination)
        .map_err(|error| format!("Не удалось создать папку медиа презентации: {error}"))?;
    let mut report = PresentationMediaReport::default();
    let mut output_index = 0usize;
    let mut copied_media_cache: HashMap<String, PathBuf> = HashMap::new();

    for (slide_number, rel_id) in slide_rel_ids.iter().enumerate() {
        let Some(slide_relation) = presentation_rels.get(rel_id) else {
            report.warnings.push(format!(
                "Слайд {}: не найдена связь презентации {rel_id}.",
                slide_number + 1
            ));
            continue;
        };
        if slide_relation.external {
            report.warnings.push(format!(
                "Слайд {}: связь слайда указывает за пределы архива PPTX.",
                slide_number + 1
            ));
            continue;
        }
        let Some(slide_part) =
            normalize_package_target("ppt/presentation.xml", &slide_relation.target)
        else {
            report.warnings.push(format!(
                "Слайд {}: небезопасный путь слайда пропущен.",
                slide_number + 1
            ));
            continue;
        };
        if !package_parts.contains(&slide_part) {
            report.warnings.push(format!(
                "Слайд {}: файл слайда '{}' не найден.",
                slide_number + 1,
                slide_part
            ));
            continue;
        }
        let slide_bytes = read_xml_part(&mut archive, &slide_part)?;
        let slide = parse_xml(&slide_bytes)
            .map_err(|error| format!("Некорректный XML слайда {}: {error}", slide_number + 1))?;
        let slide_rels_part = relationships_part_path(&slide_part);
        let relationships = read_relationships(&mut archive, &slide_rels_part).unwrap_or_default();
        let shapes = collect_video_shapes(&slide);
        let mut slide_media = PresentationMediaSlide {
            slide_number: slide_number + 1,
            videos: Vec::new(),
        };

        for mut shape in shapes {
            let mut seen_sources = HashSet::new();
            let mut candidates = Vec::new();
            let mut missing_video_relation: Option<String> = None;
            let mut unresolved_video_reference: Option<(String, String)> = None;
            for shape_ref in &shape.refs {
                let Some(relationship) = relationships.get(&shape_ref.rel_id) else {
                    if shape_ref.kind == ShapeVideoRefKind::Video {
                        shape.has_video_object = true;
                        missing_video_relation.get_or_insert_with(|| shape_ref.rel_id.clone());
                    }
                    continue;
                };
                if !is_video_reference(shape_ref.kind, relationship) {
                    continue;
                }
                if matches!(
                    shape_ref.kind,
                    ShapeVideoRefKind::Video | ShapeVideoRefKind::Media
                ) {
                    shape.has_video_object = true;
                }
                let Some(media_source) = resolve_media_source(source, &slide_part, relationship)
                else {
                    unresolved_video_reference.get_or_insert_with(|| {
                        (relationship.target.clone(), file_name(&relationship.target))
                    });
                    continue;
                };
                let source_key = media_source_key(&media_source);
                if !seen_sources.insert(source_key.clone()) {
                    continue;
                }
                let display = media_source_display(&media_source);
                candidates.push(VideoSourceCandidate {
                    key: source_key,
                    name: file_name(&display),
                    display,
                    extension: media_extension(&media_source),
                    source: media_source,
                });
            }

            candidates.sort_by_key(|candidate| source_priority(candidate, &package_parts));
            let mut output_path = None;
            let mut failed_candidate: Option<(PresentationMediaStatus, String, String)> = None;
            for candidate in &candidates {
                if let Some(cached_path) = copied_media_cache.get(&candidate.key) {
                    output_path = Some((cached_path.clone(), candidate.clone()));
                    break;
                }
                if let MediaSource::Web(_) = candidate.source {
                    failed_candidate.get_or_insert_with(|| {
                        let error = if is_network_file_target(&candidate.display) {
                            format!(
                                "сетевой файл '{}' не проверялся; добавьте видео вручную",
                                candidate.display
                            )
                        } else {
                            format!("веб-видео '{}' не загружалось", candidate.display)
                        };
                        (
                            PresentationMediaStatus::Unsupported,
                            candidate.name.clone(),
                            error,
                        )
                    });
                    continue;
                }
                let Some(extension) = candidate.extension else {
                    failed_candidate.get_or_insert_with(|| {
                        (
                            PresentationMediaStatus::Unsupported,
                            candidate.name.clone(),
                            format!("неподдерживаемый формат видео '{}'", candidate.display),
                        )
                    });
                    continue;
                };
                let copied = match &candidate.source {
                    MediaSource::PackagePart(part) => copy_package_media(
                        &mut archive,
                        part,
                        destination,
                        slide_number + 1,
                        output_index,
                        extension,
                    ),
                    MediaSource::ExternalPath(path) => copy_external_media(
                        path,
                        destination,
                        slide_number + 1,
                        output_index,
                        extension,
                    ),
                    MediaSource::Web(_) => unreachable!(),
                };
                match copied {
                    Ok(path) => {
                        copied_media_cache.insert(candidate.key.clone(), path.clone());
                        output_path = Some((path, candidate.clone()));
                        break;
                    }
                    Err(CopyError::Missing) => {
                        failed_candidate.get_or_insert_with(|| {
                            (
                                PresentationMediaStatus::Missing,
                                candidate.name.clone(),
                                format!("видеофайл '{}' не найден", candidate.display),
                            )
                        });
                    }
                    Err(CopyError::TooLarge) => {
                        failed_candidate.get_or_insert_with(|| {
                            (
                                PresentationMediaStatus::Unsupported,
                                candidate.name.clone(),
                                format!(
                                    "видеофайл '{}' превышает допустимый размер извлечения",
                                    candidate.display
                                ),
                            )
                        });
                    }
                    Err(CopyError::Invalid(error)) => {
                        failed_candidate.get_or_insert_with(|| {
                            (
                                PresentationMediaStatus::Unsupported,
                                candidate.name.clone(),
                                format!(
                                    "не удалось извлечь видео '{}': {error}",
                                    candidate.display
                                ),
                            )
                        });
                    }
                }
            }

            let Some((output_path, chosen_source)) = output_path else {
                let failure = failed_candidate
                    .or_else(|| {
                        missing_video_relation.map(|rel_id| {
                            (
                                PresentationMediaStatus::Missing,
                                rel_id.clone(),
                                format!("не найдена связь с видео '{rel_id}'"),
                            )
                        })
                    })
                    .or_else(|| {
                        unresolved_video_reference.map(|(target, name)| {
                            (
                                PresentationMediaStatus::Unsupported,
                                name,
                                format!("неподдерживаемая ссылка на видео '{target}'"),
                            )
                        })
                    });
                if let Some((status, name, error)) = failure {
                    report.warnings.push(format!(
                        "Слайд {}: {error}; добавьте видео отдельным Video Cue.",
                        slide_number + 1
                    ));
                    slide_media.videos.push(ExtractedPresentationVideo {
                        slide_number: slide_number + 1,
                        name,
                        status,
                        path: None,
                        layout: None,
                        supported: false,
                        error: Some(error),
                    });
                }
                continue;
            };

            let (layout, supported, error) = shape_layout(&shape, slide_width, slide_height);
            if let Some(reason) = &error {
                report.warnings.push(format!(
                    "Слайд {}: видео '{}' {reason}.",
                    slide_number + 1,
                    chosen_source.display
                ));
            }
            report.report.push(format!(
                "Слайд {}: видео '{}' извлечено в '{}'.",
                slide_number + 1,
                chosen_source.display,
                output_path.display()
            ));
            slide_media.videos.push(ExtractedPresentationVideo {
                slide_number: slide_number + 1,
                name: chosen_source.name,
                status: PresentationMediaStatus::Added,
                path: Some(output_path),
                layout,
                supported,
                error,
            });
            output_index += 1;
        }
        if !slide_media.videos.is_empty() {
            report.slides.push(slide_media);
        }
    }

    Ok(report)
}

fn validate_archive<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<HashSet<String>, String> {
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err("В архиве PPTX слишком много файлов.".to_owned());
    }
    let mut total = 0u64;
    let mut parts = HashSet::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("Не удалось проверить файл PPTX: {error}"))?;
        let original_name = entry.name();
        if !safe_archive_name(original_name) {
            return Err(format!(
                "В архиве PPTX обнаружен небезопасный путь: {original_name}"
            ));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| "Размер распакованного PPTX превышает допустимый предел.".to_owned())?;
        if total > MAX_PACKAGE_UNCOMPRESSED {
            return Err("Размер распакованного PPTX превышает допустимый предел.".to_owned());
        }
        if !entry.is_dir() {
            parts.insert(original_name.replace('\\', "/"));
        }
    }
    Ok(parts)
}

fn safe_archive_name(name: &str) -> bool {
    let normalized = name.replace('\\', "/");
    !normalized.starts_with('/')
        && !normalized.split('/').any(|segment| segment == "..")
        && !normalized.contains(':')
}

fn read_xml_part<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>, String> {
    let mut entry = archive
        .by_name(name)
        .map_err(|error| format!("Часть PPTX '{name}' не найдена: {error}"))?;
    if entry.size() > MAX_XML_BYTES {
        return Err(format!(
            "XML-часть PPTX '{name}' превышает допустимый размер."
        ));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .take(MAX_XML_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Не удалось прочитать XML-часть PPTX '{name}': {error}"))?;
    if bytes.len() as u64 > MAX_XML_BYTES {
        return Err(format!(
            "XML-часть PPTX '{name}' превышает допустимый размер."
        ));
    }
    Ok(bytes)
}

fn parse_xml(bytes: &[u8]) -> Result<XmlNode, String> {
    let mut reader = Reader::from_reader(Cursor::new(bytes));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut stack: Vec<XmlNode> = Vec::new();
    let mut root = XmlNode::default();
    let mut node_count = 0usize;
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| format!("Ошибка чтения XML: {error}"))?
        {
            Event::Start(event) => {
                node_count += 1;
                if node_count > MAX_XML_NODES {
                    return Err("В XML презентации слишком много элементов.".to_owned());
                }
                if stack.len() >= MAX_XML_DEPTH {
                    return Err(
                        "Вложенность XML презентации превышает допустимую глубину.".to_owned()
                    );
                }
                stack.push(node_from_start(&event, &reader)?);
            }
            Event::Empty(event) => {
                node_count += 1;
                if node_count > MAX_XML_NODES {
                    return Err("В XML презентации слишком много элементов.".to_owned());
                }
                let node = node_from_start(&event, &reader)?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    root.children.push(node);
                }
            }
            Event::End(_) => {
                let node = stack
                    .pop()
                    .ok_or_else(|| "Неожиданный закрывающий тег XML.".to_owned())?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    root.children.push(node);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err("В XML остался незакрытый элемент.".to_owned());
    }
    Ok(root)
}

fn node_from_start(
    event: &BytesStart<'_>,
    reader: &Reader<Cursor<&[u8]>>,
) -> Result<XmlNode, String> {
    let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();
    let mut attrs = HashMap::new();
    for attr in event.attributes().with_checks(true) {
        let attr = attr.map_err(|error| format!("Некорректный атрибут XML: {error}"))?;
        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
        let value = attr
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| format!("Некорректное значение атрибута XML: {error}"))?
            .into_owned();
        attrs.insert(key, value);
    }
    Ok(XmlNode {
        name,
        attrs,
        children: Vec::new(),
    })
}

fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn find_nodes<'a>(node: &'a XmlNode, name: &str, result: &mut Vec<&'a XmlNode>) {
    if local_name(&node.name) == name {
        result.push(node);
    }
    for child in &node.children {
        find_nodes(child, name, result);
    }
}

fn attr<'a>(node: &'a XmlNode, qualified: &str) -> Option<&'a str> {
    node.attrs.get(qualified).map(String::as_str)
}

fn read_relationships<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    part: &str,
) -> Result<HashMap<String, Relationship>, String> {
    let bytes = read_xml_part(archive, part)?;
    let root = parse_xml(&bytes)?;
    let mut nodes = Vec::new();
    find_nodes(&root, "Relationship", &mut nodes);
    let mut result = HashMap::new();
    for node in nodes {
        let (Some(id), Some(target)) = (attr(node, "Id"), attr(node, "Target")) else {
            continue;
        };
        result.insert(
            id.to_owned(),
            Relationship {
                target: target.to_owned(),
                external: attr(node, "TargetMode")
                    .is_some_and(|mode| mode.eq_ignore_ascii_case("External")),
                rel_type: attr(node, "Type").unwrap_or_default().to_owned(),
            },
        );
    }
    Ok(result)
}

fn ordered_slide_relationships(root: &XmlNode) -> Vec<String> {
    let mut nodes = Vec::new();
    find_nodes(root, "sldId", &mut nodes);
    nodes
        .into_iter()
        .filter_map(|node| attr(node, "r:id").map(str::to_owned))
        .collect()
}

fn slide_size(root: &XmlNode) -> (u64, u64) {
    let mut nodes = Vec::new();
    find_nodes(root, "sldSz", &mut nodes);
    let Some(node) = nodes.first() else {
        return (0, 0);
    };
    (
        attr(node, "cx").and_then(|v| v.parse().ok()).unwrap_or(0),
        attr(node, "cy").and_then(|v| v.parse().ok()).unwrap_or(0),
    )
}

fn relationships_part_path(document_part: &str) -> String {
    let path = Path::new(document_part);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let file_name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
    format!("{}/{}/{}.rels", parent.display(), "_rels", file_name)
}

fn normalize_package_target(document_part: &str, target: &str) -> Option<String> {
    let target = target
        .split(['?', '#'])
        .next()
        .unwrap_or(target)
        .replace('\\', "/");
    let decoded = percent_decode(&target)?;
    let mut segments: Vec<&str> = if decoded.starts_with('/') {
        Vec::new()
    } else {
        document_part
            .rsplit_once('/')
            .map(|(parent, _)| parent.split('/').collect())
            .unwrap_or_default()
    };
    for segment in decoded.trim_start_matches('/').split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            value if value.contains(':') => return None,
            value => segments.push(value),
        }
    }
    if segments.is_empty() {
        None
    } else {
        Some(segments.join("/"))
    }
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = (bytes[index + 1] as char).to_digit(16)? as u8;
            let low = (bytes[index + 2] as char).to_digit(16)? as u8;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn collect_video_shapes(root: &XmlNode) -> Vec<ShapeVideo> {
    let mut output = Vec::new();
    collect_shapes(root, false, &mut output);
    output
}

fn collect_shapes(node: &XmlNode, in_group: bool, output: &mut Vec<ShapeVideo>) {
    let name = local_name(&node.name);
    let grouped = in_group || name == "grpSp";
    if matches!(name, "pic" | "sp" | "graphicFrame") {
        let mut shape = ShapeVideo {
            refs: Vec::new(),
            x: None,
            y: None,
            cx: None,
            cy: None,
            rotation: None,
            crop: [0.0; 4],
            grouped: in_group,
            has_video_object: false,
        };
        collect_shape_refs(node, &mut shape.refs);
        if !shape.refs.is_empty() {
            collect_shape_geometry(node, &mut shape);
            shape.grouped |= in_group;
            output.push(shape);
        }
    }
    for child in &node.children {
        collect_shapes(child, grouped, output);
    }
}

fn collect_shape_refs(node: &XmlNode, refs: &mut Vec<ShapeVideoRef>) {
    let name = local_name(&node.name);
    let (kind, keys): (Option<ShapeVideoRefKind>, &[&str]) = match name {
        "videoFile" => (
            Some(ShapeVideoRefKind::Video),
            &["r:link", "r:embed", "r:id"],
        ),
        "media" => (
            Some(ShapeVideoRefKind::Media),
            &["r:link", "r:embed", "r:id"],
        ),
        "hlinkClick" | "hlinkHover" => (Some(ShapeVideoRefKind::Hyperlink), &["r:id"]),
        _ => (None, &[]),
    };
    if let Some(kind) = kind {
        for key in keys {
            if let Some(rel_id) = attr(node, key) {
                refs.push(ShapeVideoRef {
                    rel_id: rel_id.to_owned(),
                    kind,
                });
            }
        }
    }
    for child in &node.children {
        collect_shape_refs(child, refs);
    }
}

fn collect_shape_geometry(node: &XmlNode, shape: &mut ShapeVideo) {
    let mut transforms = Vec::new();
    find_nodes(node, "xfrm", &mut transforms);
    if let Some(transform) = transforms.first() {
        shape.rotation = attr(transform, "rot").and_then(|value| value.parse::<i64>().ok());
        let mut offsets = Vec::new();
        let mut extents = Vec::new();
        find_nodes(transform, "off", &mut offsets);
        find_nodes(transform, "ext", &mut extents);
        if let Some(offset) = offsets.first() {
            shape.x = attr(offset, "x").and_then(|v| v.parse().ok());
            shape.y = attr(offset, "y").and_then(|v| v.parse().ok());
        }
        if let Some(extent) = extents.first() {
            shape.cx = attr(extent, "cx").and_then(|v| v.parse().ok());
            shape.cy = attr(extent, "cy").and_then(|v| v.parse().ok());
        }
    }
    let mut crops = Vec::new();
    find_nodes(node, "srcRect", &mut crops);
    if let Some(crop) = crops.first() {
        for (index, key) in ["l", "t", "r", "b"].into_iter().enumerate() {
            shape.crop[index] = attr(crop, key)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
                / 100_000.0;
        }
    }
}

fn resolve_media_source(
    source: &Path,
    slide_part: &str,
    relationship: &Relationship,
) -> Option<MediaSource> {
    let target = relationship.target.trim();
    if relationship.external {
        // Never let a media probe touch a network share. `metadata` and
        // `canonicalize` can block on UNC paths, so represent these as a
        // non-fetchable source before candidate prioritization.
        if target.starts_with("\\\\") || target.starts_with("//") {
            return Some(MediaSource::Web(target.to_owned()));
        }
        let windows_absolute = target.as_bytes().get(1) == Some(&b':')
            && target
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic);
        let native_target = PathBuf::from(target.replace('\\', std::path::MAIN_SEPARATOR_STR));
        if native_target.is_absolute() || windows_absolute {
            return Some(MediaSource::ExternalPath(native_target));
        }
        if let Ok(url) = Url::parse(target) {
            match url.scheme() {
                "http" | "https" | "ftp" => return Some(MediaSource::Web(target.to_owned())),
                "file" => {
                    if url.host_str().is_some_and(|host| {
                        !host.is_empty() && !host.eq_ignore_ascii_case("localhost")
                    }) {
                        return Some(MediaSource::Web(target.to_owned()));
                    }
                    let mut local_url = url.clone();
                    if local_url
                        .host_str()
                        .is_some_and(|host| host.eq_ignore_ascii_case("localhost"))
                    {
                        if local_url.set_host(None).is_err() {
                            return None;
                        }
                    }
                    return local_url.to_file_path().ok().map(MediaSource::ExternalPath);
                }
                _ => return None,
            }
        }
        if target.contains("://") {
            return None;
        }
        let decoded = percent_decode(target)?;
        let path = PathBuf::from(decoded.replace('\\', std::path::MAIN_SEPARATOR_STR));
        let resolved = if path.is_absolute() {
            path
        } else {
            source.parent()?.join(path)
        };
        return Some(MediaSource::ExternalPath(resolved));
    }
    let package_part = normalize_package_target(slide_part, target)?;
    Some(MediaSource::PackagePart(package_part))
}

fn media_source_key(source: &MediaSource) -> String {
    match source {
        MediaSource::PackagePart(part) => format!("package:{}", part.to_ascii_lowercase()),
        MediaSource::ExternalPath(path) => fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_owned())
            .to_string_lossy()
            .to_ascii_lowercase(),
        MediaSource::Web(url) => format!("web:{}", url.to_ascii_lowercase()),
    }
}

fn is_network_file_target(target: &str) -> bool {
    target.starts_with("\\\\")
        || target.starts_with("//")
        || Url::parse(target).ok().is_some_and(|url| {
            url.scheme() == "file"
                && url
                    .host_str()
                    .is_some_and(|host| !host.is_empty() && !host.eq_ignore_ascii_case("localhost"))
        })
}

fn media_source_display(source: &MediaSource) -> String {
    match source {
        MediaSource::PackagePart(part) => part.clone(),
        MediaSource::ExternalPath(path) => path.display().to_string(),
        MediaSource::Web(url) => url.clone(),
    }
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned()
}

fn media_extension(source: &MediaSource) -> Option<&'static str> {
    let path = match source {
        MediaSource::PackagePart(part) => Path::new(part),
        MediaSource::ExternalPath(path) => path.as_path(),
        MediaSource::Web(url) => Path::new(
            url.split(['?', '#'])
                .next()
                .unwrap_or(url)
                .rsplit('/')
                .next()
                .unwrap_or(""),
        ),
    };
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    VIDEO_EXTENSIONS
        .iter()
        .copied()
        .find(|candidate| *candidate == extension)
}

fn relationship_target_has_video_extension(target: &str) -> bool {
    let target = target.split(['?', '#']).next().unwrap_or(target);
    let path = Url::parse(target)
        .ok()
        .map(|url| url.path().to_owned())
        .unwrap_or_else(|| target.replace('\\', "/"));
    Path::new(&path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        })
}

fn is_video_reference(kind: ShapeVideoRefKind, relationship: &Relationship) -> bool {
    let rel_type = relationship.rel_type.to_ascii_lowercase();
    let target_has_video_extension = relationship_target_has_video_extension(&relationship.target);
    match kind {
        ShapeVideoRefKind::Video => true,
        ShapeVideoRefKind::Media => {
            !rel_type.contains("audio")
                && (rel_type.contains("video") || target_has_video_extension)
        }
        ShapeVideoRefKind::Hyperlink => rel_type.contains("video") || target_has_video_extension,
    }
}

fn source_priority(
    candidate: &VideoSourceCandidate,
    package_parts: &HashSet<String>,
) -> (bool, u8) {
    let available = match &candidate.source {
        MediaSource::PackagePart(part) => package_parts.contains(part),
        MediaSource::ExternalPath(path) => {
            fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
        }
        MediaSource::Web(_) => false,
    };
    let location_rank = match (&candidate.source, available) {
        (MediaSource::PackagePart(_), true) => 0,
        (MediaSource::ExternalPath(_), true) => 1,
        (MediaSource::Web(_), _) => 2,
        (MediaSource::PackagePart(_), false) => 3,
        (MediaSource::ExternalPath(_), false) => 4,
    };
    (candidate.extension.is_none(), location_rank)
}

#[derive(Debug)]
enum CopyError {
    Missing,
    TooLarge,
    Invalid(String),
}

fn copy_package_media<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    part: &str,
    destination: &Path,
    slide: usize,
    index: usize,
    extension: &str,
) -> Result<PathBuf, CopyError> {
    let mut entry = archive.by_name(part).map_err(|_| CopyError::Missing)?;
    if entry.size() > MAX_MEDIA_BYTES {
        return Err(CopyError::TooLarge);
    }
    let output_path =
        unique_output_path(destination, slide, index, extension).map_err(CopyError::Invalid)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .map_err(|error| CopyError::Invalid(error.to_string()))?;
    let result = io::copy(&mut entry.take(MAX_MEDIA_BYTES + 1), &mut output)
        .map_err(|error| CopyError::Invalid(error.to_string()));
    match result {
        Ok(size) if size <= MAX_MEDIA_BYTES => Ok(output_path),
        Ok(_) => {
            let _ = fs::remove_file(&output_path);
            Err(CopyError::TooLarge)
        }
        Err(error) => {
            let _ = fs::remove_file(&output_path);
            Err(error)
        }
    }
}

fn copy_external_media(
    source: &Path,
    destination: &Path,
    slide: usize,
    index: usize,
    extension: &str,
) -> Result<PathBuf, CopyError> {
    let metadata = fs::metadata(source).map_err(|_| CopyError::Missing)?;
    if !metadata.is_file() {
        return Err(CopyError::Missing);
    }
    if metadata.len() > MAX_MEDIA_BYTES {
        return Err(CopyError::TooLarge);
    }
    let mut input = File::open(source).map_err(|_| CopyError::Missing)?;
    let output_path =
        unique_output_path(destination, slide, index, extension).map_err(CopyError::Invalid)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .map_err(|error| CopyError::Invalid(error.to_string()))?;
    let result = io::copy(&mut input.take(MAX_MEDIA_BYTES + 1), &mut output)
        .map_err(|error| CopyError::Invalid(error.to_string()));
    match result {
        Ok(size) if size <= MAX_MEDIA_BYTES => Ok(output_path),
        Ok(_) => {
            let _ = fs::remove_file(&output_path);
            Err(CopyError::TooLarge)
        }
        Err(error) => {
            let _ = fs::remove_file(&output_path);
            Err(error)
        }
    }
}

fn unique_output_path(
    destination: &Path,
    slide: usize,
    index: usize,
    extension: &str,
) -> Result<PathBuf, String> {
    for suffix in 0..1000usize {
        let name = if suffix == 0 {
            format!("slide-{slide}-video-{index}.{extension}")
        } else {
            format!("slide-{slide}-video-{index}-{suffix}.{extension}")
        };
        let path = destination.join(name);
        if !path.exists() {
            return Ok(path);
        }
    }
    Err("Не удалось создать уникальное имя извлечённого видео.".to_owned())
}

fn shape_layout(
    shape: &ShapeVideo,
    slide_width: u64,
    slide_height: u64,
) -> (Option<PresentationVideoLayout>, bool, Option<String>) {
    if !shape.has_video_object {
        return (
            None,
            false,
            Some("это ссылка на видео; исходное размещение ролика не задано".to_owned()),
        );
    }
    let (Some(x), Some(y), Some(cx), Some(cy)) = (shape.x, shape.y, shape.cx, shape.cy) else {
        return (
            None,
            false,
            Some("не задано поддерживаемое положение на слайде".to_owned()),
        );
    };
    if slide_width == 0 || slide_height == 0 || cx <= 0 || cy <= 0 {
        return (
            None,
            false,
            Some("метаданные положения на слайде некорректны".to_owned()),
        );
    }
    let rotation = shape.rotation.unwrap_or(0);
    let signed_rotation_degrees = rotation as f64 / 60_000.0;
    let rotation_degrees = signed_rotation_degrees.rem_euclid(360.0);
    let layout = PresentationVideoLayout {
        slide_width,
        slide_height,
        x: x as f64 / slide_width as f64,
        y: y as f64 / slide_height as f64,
        width: cx as f64 / slide_width as f64,
        height: cy as f64 / slide_height as f64,
        rotation: rotation_degrees.round() as u32,
        crop_left: shape.crop[0],
        crop_top: shape.crop[1],
        crop_right: shape.crop[2],
        crop_bottom: shape.crop[3],
    };
    let reason = if shape.grouped {
        Some(
            "видео сгруппировано с другими объектами; исходное положение восстановить нельзя"
                .to_owned(),
        )
    } else if signed_rotation_degrees.abs() > 0.001 {
        Some("видео повёрнуто; исходное положение восстановить нельзя".to_owned())
    } else if shape.crop.iter().any(|value| value.abs() > 0.000_001) {
        Some("видео обрезано; исходное положение восстановить нельзя".to_owned())
    } else {
        None
    };
    (Some(layout), reason.is_none(), reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "qlisa-presentation-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_package(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        for (name, bytes) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn baseline_entries() -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("ppt/presentation.xml", br#"<p:presentation xmlns:p="p" xmlns:r="r"><p:sldIdLst><p:sldId id="3" r:id="rIdSecond"/><p:sldId id="4" r:id="rIdFirst"/></p:sldIdLst><p:sldSz cx="1000" cy="500"/></p:presentation>"#),
            ("ppt/_rels/presentation.xml.rels", br#"<Relationships><Relationship Id="rIdSecond" Type="slide" Target="slides/slide2.xml"/><Relationship Id="rIdFirst" Type="slide" Target="slides/slide1.xml"/></Relationships>"#),
            ("ppt/slides/slide2.xml", br#"<p:sld xmlns:p="p" xmlns:a="a" xmlns:r="r" xmlns:p14="p14"><p:spTree><p:pic><p:nvPicPr><p:nvPr><a:videoFile r:link="rIdVideoA"/><p:extLst><p:ext><p14:media r:embed="rIdVideoAlias"/></p:ext></p:extLst></p:nvPr></p:nvPicPr><p:spPr><a:xfrm><a:off x="100" y="50"/><a:ext cx="500" cy="250"/></a:xfrm></p:spPr></p:pic><p:pic><p:nvPicPr><p:nvPr><a:videoFile r:link="rIdVideoA"/></p:nvPr></p:nvPicPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="500" cy="250"/></a:xfrm></p:spPr></p:pic></p:spTree></p:sld>"#),
            ("ppt/slides/_rels/slide2.xml.rels", br#"<Relationships><Relationship Id="rIdVideoA" Type="video" Target="../media/clip.mp4"/><Relationship Id="rIdVideoAlias" Type="media" Target="../media/clip.mp4"/></Relationships>"#),
            ("ppt/slides/slide1.xml", br#"<p:sld xmlns:p="p" xmlns:a="a" xmlns:r="r" xmlns:p14="p14"><p:spTree><p:pic><p:nvPicPr><p:nvPr><a:hlinkClick r:id="rIdExternal"/><a:hlinkClick r:id="rIdWebsite"/></p:nvPr></p:nvPicPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="250" cy="250"/></a:xfrm></p:spPr></p:pic><p:pic><p:nvPicPr><p:nvPr><a:videoFile r:link="rIdMissing"/></p:nvPr></p:nvPicPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="200" cy="200"/></a:xfrm></p:spPr></p:pic><p:pic><p:nvPicPr><p:nvPr><a:videoFile r:link="rIdWeb"/></p:nvPr></p:nvPicPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="200" cy="200"/></a:xfrm></p:spPr></p:pic><p:pic><p:nvPicPr><p:nvPr><p14:media r:embed="rIdAudio"/></p:nvPr></p:nvPicPr></p:pic></p:spTree></p:sld>"#),
            ("ppt/slides/_rels/slide1.xml.rels", br#"<Relationships><Relationship Id="rIdExternal" Type="video" Target="assets/external.mp4" TargetMode="External"/><Relationship Id="rIdMissing" Type="video" Target="missing.mp4" TargetMode="External"/><Relationship Id="rIdWeb" Type="hyperlink" Target="https://example.invalid/video.mp4" TargetMode="External"/><Relationship Id="rIdWebsite" Type="hyperlink" Target="https://example.invalid" TargetMode="External"/><Relationship Id="rIdAudio" Type="audio" Target="../media/sound.mp3"/></Relationships>"#),
            ("ppt/media/clip.mp4", b"embedded-video"),
        ]
    }

    #[test]
    fn rejects_unc_and_remote_file_uris_without_creating_external_path_sources() {
        for target in [
            r"\\server\share\clip.mp4",
            "//server/share/clip.mp4",
            "file://server/share/clip.mp4",
        ] {
            let relation = Relationship {
                target: target.to_owned(),
                external: true,
                rel_type: "video".to_owned(),
            };
            let source = resolve_media_source(
                Path::new("C:/shows/deck.pptx"),
                "ppt/slides/slide1.xml",
                &relation,
            )
            .expect("network references remain reportable");
            assert!(
                matches!(source, MediaSource::Web(_)),
                "{target} must not become a filesystem path"
            );
            assert!(
                is_network_file_target(target),
                "{target} must be reported as a network file"
            );
            assert!(media_source_key(&source).starts_with("web:"));
            assert!(!matches!(source, MediaSource::ExternalPath(_)));
        }
    }

    #[test]
    fn preserves_slide_order_deduplicates_aliases_and_copies_embedded_and_relative_media() {
        let root = temp_dir("ordered");
        let source = root.join("deck.pptx");
        let destination = root.join("extracted");
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(root.join("assets/external.mp4"), b"external-video").unwrap();
        write_package(&source, &baseline_entries());

        let report = extract_presentation_media(&source, &destination).unwrap();

        assert_eq!(report.slides.len(), 2);
        assert_eq!(report.slides[0].slide_number, 1);
        assert_eq!(report.slides[0].videos.len(), 2);
        assert_eq!(
            report.slides[0].videos[0].status,
            PresentationMediaStatus::Added
        );
        assert_eq!(
            fs::read(report.slides[0].videos[0].path.as_ref().unwrap()).unwrap(),
            b"embedded-video"
        );
        assert_eq!(
            report.slides[1].slide_number, 2,
            "presentation.xml order overrides slide filenames"
        );
        assert_eq!(
            report.slides[1].videos.len(),
            3,
            "the next manifest slide has one external, one missing, and one web video"
        );
        assert_eq!(
            report.slides[1].videos[0].status,
            PresentationMediaStatus::Added
        );
        assert_eq!(
            fs::read(report.slides[1].videos[0].path.as_ref().unwrap()).unwrap(),
            b"external-video"
        );
        assert_eq!(
            report.slides[1].videos[1].status,
            PresentationMediaStatus::Missing
        );
        assert_eq!(
            report.slides[1].videos[2].status,
            PresentationMediaStatus::Unsupported
        );
        assert!(!report.slides[1].videos[0].supported);
        assert!(report.slides[1].videos[0].layout.is_none());
        assert!(report.slides[0].videos[0].supported);
        assert_eq!(
            report.slides[0].videos[0].path, report.slides[0].videos[1].path,
            "aliases on one shape deduplicate while another shape remains a cue"
        );
        let layout = report.slides[0].videos[0].layout.unwrap();
        assert!((layout.x - 0.1).abs() < 0.0001);
        assert!((layout.width - 0.5).abs() < 0.0001);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("missing.mp4")));
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("https://example.invalid/video.mp4")));
        assert!(report
            .warnings
            .iter()
            .all(|warning| !warning.contains("rIdWebsite") && !warning.contains("sound.mp3")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn marks_grouped_rotated_and_cropped_video_layout_as_unsupported() {
        let shape = ShapeVideo {
            refs: vec![],
            x: Some(0),
            y: Some(0),
            cx: Some(100),
            cy: Some(100),
            rotation: Some(5_400_000),
            crop: [0.0; 4],
            grouped: false,
            has_video_object: true,
        };
        let (_, supported, error) = shape_layout(&shape, 100, 100);
        assert!(!supported);
        assert!(error.unwrap().contains("повёрнуто"));
        let negative_rotation = ShapeVideo {
            rotation: Some(-5_400_000),
            ..shape.clone()
        };
        let (layout, supported, error) = shape_layout(&negative_rotation, 100, 100);
        assert!(!supported);
        assert_eq!(layout.unwrap().rotation, 270);
        assert!(error.unwrap().contains("повёрнуто"));
        let grouped = ShapeVideo {
            rotation: Some(0),
            grouped: true,
            ..shape.clone()
        };
        assert!(shape_layout(&grouped, 100, 100)
            .2
            .unwrap()
            .contains("сгруппировано"));
        let cropped = ShapeVideo {
            rotation: Some(0),
            crop: [0.1, 0.0, 0.0, 0.0],
            grouped: false,
            ..shape
        };
        assert!(shape_layout(&cropped, 100, 100)
            .2
            .unwrap()
            .contains("обрезано"));
    }

    #[test]
    fn rejects_malformed_xml_and_zip_path_traversal() {
        let root = temp_dir("unsafe");
        let malformed = root.join("malformed.pptx");
        let mut entries = baseline_entries();
        entries[0] = ("ppt/presentation.xml", b"<p:presentation>");
        write_package(&malformed, &entries);
        let result = extract_presentation_media(&malformed, &root.join("malformed-out"));
        assert!(result.is_err());

        let traversal = root.join("traversal.pptx");
        let mut entries = baseline_entries();
        entries.push(("../escape.mp4", b"escape"));
        write_package(&traversal, &entries);
        let result = extract_presentation_media(&traversal, &root.join("traversal-out"));
        assert!(result.unwrap_err().contains("небезопасный путь"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bounds_xml_nesting_depth() {
        let open = "<node>".repeat(MAX_XML_DEPTH + 1);
        let close = "</node>".repeat(MAX_XML_DEPTH + 1);
        let xml = format!("{open}{close}");
        assert!(parse_xml(xml.as_bytes())
            .unwrap_err()
            .contains("допустимую глубину"));
    }
}
