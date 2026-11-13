//! Import PDF and PowerPoint files as ordered static PNG pages.
//!
//! The presentation cue owns playback and page advancement. This module only
//! prepares its page images and reports formats that were flattened.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PAGES: usize = 500;
const MAX_PAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TOTAL_PNG_BYTES: u64 = 512 * 1024 * 1024;
const MAX_RENDER_SIDE: u32 = 1920;
const BASE_PIXELS_PER_POINT: f64 = 2.0;
const OFFICE_TIMEOUT_SECS: u64 = 180;
const LIBREOFFICE_TIMEOUT_SECS: u64 = 180;

/// Pages rendered from one imported document, in original page/slide order.
pub struct PreparedDocument {
    pub title: String,
    pub pages: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Convert a PDF, PPT, or PPTX into PNG pages inside an empty import folder.
///
/// The caller owns `destination`. This function never overwrites existing
/// files there and removes its temporary outputs if any page fails.
pub fn prepare_document(source: &Path, destination: &Path) -> Result<PreparedDocument> {
    validate_source(source)?;
    validate_empty_destination(destination)?;

    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "pdf" | "ppt" | "pptx") {
        bail!("Поддерживаются только документы PDF, PPT и PPTX.");
    }

    // Give out-of-process PowerPoint a short physical path outside MSIX
    // virtualized AppData. We do not rely on the caller's logical AppData path.
    let (_stage, stage_root) = StageDirectory::create_temp("qld-")?;
    let stage_path = stage_root.join("ppt");
    fs::create_dir(&stage_path).context("Не удалось создать папку для страниц документа")?;
    let mut warnings = Vec::new();

    match extension.as_str() {
        "pdf" => render_pdf(source, &stage_path)?,
        "ppt" | "pptx" => {
            let office_stage = stage_path.join("powerpoint-output");
            fs::create_dir(&office_stage)?;
            match export_with_powerpoint(source, &office_stage) {
                Ok(()) => transfer_pages(&office_stage, &stage_path)?,
                Err(office_error) => {
                    let libreoffice_stage = stage_path.join("libreoffice-output");
                    fs::create_dir(&libreoffice_stage)?;
                    match convert_with_libreoffice(source, &libreoffice_stage) {
                        Ok(pdf_path) => {
                            let rendered_pages = libreoffice_stage.join("rendered-pages");
                            fs::create_dir(&rendered_pages)?;
                            render_pdf(&pdf_path, &rendered_pages)?;
                            let _ = fs::remove_file(pdf_path);
                            transfer_pages(&rendered_pages, &stage_path)?;
                            warnings.push(format!(
                                "PowerPoint не смог сохранить слайды: {office_error}. Использован резервный LibreOffice."
                            ));
                        }
                        Err(libreoffice_error) => {
                            bail!(
                                "PowerPoint не смог сохранить слайды: {office_error}\nРезервный LibreOffice недоступен: {libreoffice_error}"
                            );
                        }
                    }
                }
            }
            warnings.push(
                "Переходы, анимация и звук презентации не включены в статичные изображения."
                    .to_string(),
            );
        }
        _ => unreachable!("extension validated above"),
    }

    let staged_pages = collect_and_validate_pages(&stage_path)?;
    if staged_pages.is_empty() {
        bail!("Документ не содержит страниц для импорта.");
    }

    let title = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("Документ")
        .to_string();
    let mut pages: Vec<PathBuf> = Vec::with_capacity(staged_pages.len());
    for staged in staged_pages {
        let file_name = staged
            .file_name()
            .ok_or_else(|| anyhow!("Недопустимое имя временной страницы"))?;
        let final_path = destination.join(file_name);
        let publish_result = (|| -> Result<()> {
            let mut input = fs::File::open(&staged).context("Не удалось открыть PNG страницу")?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&final_path)
                .with_context(|| {
                    format!(
                        "Не удалось создать страницу импорта: {}",
                        final_path.display()
                    )
                })?;
            if let Err(error) = std::io::copy(&mut input, &mut output) {
                let _ = fs::remove_file(&final_path);
                return Err(error).context("Не удалось скопировать PNG страницу в папку импорта");
            }
            Ok(())
        })();
        if let Err(error) = publish_result {
            for published in &pages {
                let _ = fs::remove_file(published);
            }
            return Err(error).context("Не удалось опубликовать подготовленные страницы");
        }
        pages.push(final_path);
    }

    Ok(PreparedDocument {
        title,
        pages,
        warnings,
    })
}

fn transfer_pages(source: &Path, destination: &Path) -> Result<()> {
    let pages = collect_and_validate_pages(source)?;
    if pages.is_empty() {
        bail!("Конвертер не создал PNG страницы.");
    }
    for page in pages {
        let name = page
            .file_name()
            .ok_or_else(|| anyhow!("Недопустимое имя временной страницы"))?;
        fs::rename(&page, destination.join(name))
            .context("Не удалось перенести страницу из временного конвертера")?;
    }
    Ok(())
}

struct StageDirectory(PathBuf);

impl StageDirectory {
    fn create_temp(prefix: &str) -> Result<(Self, PathBuf)> {
        if !matches!(prefix, "qld-" | "qldi-") {
            bail!("Недопустимый префикс временной папки импорта.");
        }
        let temp_root = fs::canonicalize(std::env::temp_dir())
            .context("Не удалось проверить физическую папку TEMP")?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let folder_name = format!("{prefix}{}", &id[..12]);
        let candidate = temp_root.join(&folder_name);
        fs::create_dir(&candidate).context("Не удалось создать уникальную папку TEMP")?;
        let canonical = match fs::canonicalize(&candidate) {
            Ok(path) => path,
            Err(error) => {
                let _ = fs::remove_dir(&candidate);
                return Err(error).context("Не удалось проверить папку TEMP");
            }
        };
        let is_owned_direct_child = canonical.parent() == Some(temp_root.as_path())
            && canonical
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == folder_name && name.starts_with(prefix));
        if !is_owned_direct_child {
            // The candidate is still empty, so remove only this newly-created
            // directory entry. Never recursively clean an unverified path.
            let _ = fs::remove_dir(&candidate);
            bail!("Созданная папка TEMP вышла за проверенную границу.");
        }
        Ok((Self(canonical.clone()), canonical))
    }
}

impl Drop for StageDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn validate_source(source: &Path) -> Result<()> {
    let metadata = fs::metadata(source)
        .with_context(|| format!("Не удалось прочитать документ: {}", source.display()))?;
    if !metadata.is_file() {
        bail!("Путь документа не указывает на файл: {}", source.display());
    }
    if metadata.len() == 0 {
        bail!("Файл документа пуст: {}", source.display());
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        bail!("Размер документа превышает ограничение 512 МБ.");
    }
    Ok(())
}

fn validate_empty_destination(destination: &Path) -> Result<()> {
    let metadata = fs::metadata(destination)
        .with_context(|| format!("Папка импорта не создана: {}", destination.display()))?;
    if !metadata.is_dir() {
        bail!("Путь импорта не является папкой: {}", destination.display());
    }
    if fs::read_dir(destination)
        .context("Не удалось проверить папку импорта")?
        .next()
        .is_some()
    {
        bail!("Папка импорта должна быть пустой.");
    }
    Ok(())
}

fn canonical_external_path(path: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("Не удалось получить физический путь: {}", path.display()))?;
    #[cfg(windows)]
    {
        // COM and WinRT receive physical paths, not the app's MSIX-virtualized
        // logical AppData path or Win32's inconsistent verbatim prefix.
        let text = canonical.to_string_lossy();
        let normalized = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else if let Some(path) = text.strip_prefix(r"\\?\") {
            path.to_string()
        } else {
            text.into_owned()
        };
        return Ok(PathBuf::from(normalized));
    }
    #[cfg(not(windows))]
    Ok(canonical)
}

fn page_dimensions(width: f64, height: f64) -> Result<(u32, u32)> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        bail!("В документе найдена страница с неверным размером.");
    }
    let scale = BASE_PIXELS_PER_POINT.min(MAX_RENDER_SIDE as f64 / width.max(height));
    let pixel_width = (width * scale).round().clamp(1.0, MAX_RENDER_SIDE as f64) as u32;
    let pixel_height = (height * scale).round().clamp(1.0, MAX_RENDER_SIDE as f64) as u32;
    Ok((pixel_width, pixel_height))
}

fn page_file_name(one_based_index: usize) -> String {
    format!("page-{one_based_index:04}.png")
}

fn collect_and_validate_pages(stage: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(stage)
        .context("Не удалось прочитать временные страницы")?
        .collect::<std::io::Result<Vec<_>>>()
        .context("Не удалось прочитать запись временной страницы")?;
    let mut pages = entries
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        })
        .collect::<Vec<_>>();
    pages.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    if pages.len() > MAX_PAGES {
        bail!("В документе больше {MAX_PAGES} страниц.");
    }

    let mut total_bytes = 0_u64;
    for path in &pages {
        let metadata = fs::metadata(path).context("Не удалось проверить PNG страницу")?;
        if metadata.len() > MAX_PAGE_BYTES {
            bail!("PNG страница превышает ограничение 32 МБ.");
        }
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| anyhow!("Слишком большой объём страниц"))?;
        if total_bytes > MAX_TOTAL_PNG_BYTES {
            bail!("Суммарный размер страниц превышает ограничение 512 МБ.");
        }
        validate_png(path)?;
    }
    Ok(pages)
}

fn validate_png(path: &Path) -> Result<(u32, u32)> {
    let bytes =
        fs::read(path).with_context(|| format!("Не удалось прочитать PNG: {}", path.display()))?;
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        bail!(
            "Конвертер создал повреждённую PNG страницу: {}",
            path.display()
        );
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 || width > MAX_RENDER_SIDE || height > MAX_RENDER_SIDE {
        bail!(
            "PNG страница имеет недопустимый размер: {}×{}.",
            width,
            height
        );
    }
    Ok((width, height))
}

#[cfg(windows)]
fn render_pdf(source: &Path, destination: &Path) -> Result<()> {
    let source = source.to_path_buf();
    let destination = destination.to_path_buf();
    std::thread::Builder::new()
        .name("document-pdf-render".to_string())
        .spawn(move || render_pdf_on_mta(&source, &destination))
        .context("Не удалось запустить PDF renderer")?
        .join()
        .map_err(|_| anyhow!("PDF renderer неожиданно завершился"))?
}

#[cfg(not(windows))]
fn render_pdf(_source: &Path, _destination: &Path) -> Result<()> {
    bail!("Рендеринг PDF через Windows.Data.Pdf доступен только в Windows.")
}

#[cfg(windows)]
fn render_pdf_on_mta(source: &Path, destination: &Path) -> Result<()> {
    use windows::core::HSTRING;
    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows::Graphics::Imaging::BitmapEncoder;
    use windows::Storage::{
        StorageFile,
        Streams::{DataReader, InMemoryRandomAccessStream},
    };
    use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
    use windows::UI::Color;

    validate_source(source)?;
    let source = canonical_external_path(source)?;
    let destination = canonical_external_path(destination)?;
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { RoUninitialize() };
        }
    }

    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
        .context("Не удалось инициализировать Windows Runtime")?;
    let _apartment = Apartment;

    let path = HSTRING::from(source.to_string_lossy().as_ref());
    let file = StorageFile::GetFileFromPathAsync(&path)
        .context("Windows не смог открыть PDF")?
        .get()
        .context("Не удалось прочитать PDF")?;
    let document = PdfDocument::LoadFromFileAsync(&file)
        .context("Не удалось открыть PDF документ")?
        .get()
        .context("PDF не поддерживается, повреждён или защищён паролем")?;
    let page_count = document
        .PageCount()
        .context("Не удалось прочитать число страниц PDF")? as usize;
    validate_page_count(page_count)?;
    let mut total_bytes = 0_u64;

    for index in 0..page_count {
        let page = document
            .GetPage(index as u32)
            .with_context(|| format!("Не удалось открыть PDF страницу {}", index + 1))?;
        let size = page
            .Size()
            .with_context(|| format!("Не удалось прочитать размер PDF страницы {}", index + 1))?;
        let (width, height) = page_dimensions(size.Width as f64, size.Height as f64)?;
        let options =
            PdfPageRenderOptions::new().context("Не удалось создать настройки PDF рендеринга")?;
        options.SetDestinationWidth(width)?;
        options.SetDestinationHeight(height)?;
        options.SetBackgroundColor(Color {
            A: 255,
            R: 255,
            G: 255,
            B: 255,
        })?;
        options.SetBitmapEncoderId(BitmapEncoder::PngEncoderId()?)?;

        let stream =
            InMemoryRandomAccessStream::new().context("Не удалось создать поток PDF страницы")?;
        page.RenderWithOptionsToStreamAsync(&stream, &options)
            .with_context(|| format!("Не удалось отрисовать PDF страницу {}", index + 1))?
            .get()
            .with_context(|| {
                format!(
                    "PDF renderer завершился с ошибкой на странице {}",
                    index + 1
                )
            })?;
        let size = stream.Size()?;
        if size == 0 || size > MAX_PAGE_BYTES || size > u32::MAX as u64 {
            bail!("PDF страница {} превышает лимит размера PNG.", index + 1);
        }
        total_bytes = total_bytes
            .checked_add(size)
            .ok_or_else(|| anyhow!("Слишком большой объём страниц PDF"))?;
        if total_bytes > MAX_TOTAL_PNG_BYTES {
            bail!("Суммарный размер страниц PDF превышает ограничение 512 МБ.");
        }
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(size as u32)?.get()?;
        let mut bytes = vec![0_u8; size as usize];
        reader.ReadBytes(&mut bytes)?;
        let output = destination.join(page_file_name(index + 1));
        fs::write(output, bytes).context("Не удалось сохранить PNG страницу PDF")?;
        let _ = reader.Close();
        let _ = stream.Close();
        let _ = page.Close();
    }
    Ok(())
}

#[cfg(windows)]
fn validate_page_count(page_count: usize) -> Result<()> {
    if page_count == 0 {
        bail!("Документ не содержит страниц.");
    }
    if page_count > MAX_PAGES {
        bail!("В документе больше {MAX_PAGES} страниц.");
    }
    Ok(())
}

#[cfg(not(windows))]
fn validate_page_count(page_count: usize) -> Result<()> {
    if page_count == 0 || page_count > MAX_PAGES {
        bail!("Недопустимое число страниц: {page_count}.");
    }
    Ok(())
}

#[cfg(windows)]
fn export_with_powerpoint(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const WORKER: &str = r#"
$ErrorActionPreference = 'Stop'
$app = $null
$presentations = $null
$presentation = $null
$ownsApplication = $false
$ownsPresentation = $false
$settingsCaptured = $false
$originalAutomationSecurity = $null
$originalDisplayAlerts = $null
try {
  $utf8 = [System.Text.UTF8Encoding]::new($false)
  [Console]::OutputEncoding = $utf8
  [Console]::InputEncoding = $utf8
  $stdoutWriter = [System.IO.StreamWriter]::new([Console]::OpenStandardOutput(), $utf8)
  $stderrWriter = [System.IO.StreamWriter]::new([Console]::OpenStandardError(), $utf8)
  $stdoutWriter.AutoFlush = $true
  $stderrWriter.AutoFlush = $true
  [Console]::SetOut($stdoutWriter)
  [Console]::SetError($stderrWriter)
  $OutputEncoding = $utf8
  $existingPids = @(Get-Process -Name POWERPNT -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class QlisaDocumentNative {
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
}
'@
  $type = [Type]::GetTypeFromProgID('PowerPoint.Application')
  if ($null -eq $type) { throw 'Microsoft PowerPoint automation is not installed or registered' }
  if ($existingPids.Count -gt 0) {
    try { $app = [Runtime.InteropServices.Marshal]::GetActiveObject('PowerPoint.Application') }
    catch { throw 'QLISA_POWERPOINT_EXISTING_UNAVAILABLE' }
    # Reused applications are never assigned to the worker's process owner
    # record. The worker will close only the verified untitled presentation.
  } else {
    $app = [Activator]::CreateInstance($type)
    $ownerPid = [uint32]0
    [void][QlisaDocumentNative]::GetWindowThreadProcessId([IntPtr]$app.HWND, [ref]$ownerPid)
    if ($ownerPid -eq 0 -or $existingPids -contains [int]$ownerPid) {
      throw 'PowerPoint application owner could not be identified'
    }
    $ownerProcess = Get-Process -Id $ownerPid -ErrorAction Stop
    $ownerTicks = $ownerProcess.StartTime.ToUniversalTime().Ticks
    Set-Content -LiteralPath $env:QLISA_DOC_OWNER_FILE -Value "$ownerPid|$ownerTicks" -Encoding ASCII
    $ownsApplication = $true
  }

  # Do not set Application.Visible: PowerPoint may reject it. WithWindow=false
  # hides only the imported copy and leaves an existing application untouched.
  $originalAutomationSecurity = $app.AutomationSecurity
  $originalDisplayAlerts = $app.DisplayAlerts
  $settingsCaptured = $true
  $app.AutomationSecurity = 3
  $app.DisplayAlerts = 1
  $outputDirectory = [System.IO.Path]::GetFullPath($env:QLISA_DOC_DESTINATION)
  if (-not [System.IO.Directory]::Exists($outputDirectory)) {
    throw "PowerPoint output directory does not exist: $outputDirectory"
  }
  $writeProbe = Join-Path $outputDirectory ('.qlisa-write-' + [Guid]::NewGuid().ToString('N'))
  $probeStream = [System.IO.File]::Open($writeProbe, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
  $probeStream.Dispose()
  [System.IO.File]::Delete($writeProbe)
  $firstExportPath = Join-Path $outputDirectory 'page-0001.png'
  if ($firstExportPath.Length -gt 240) {
    throw "PowerPoint output path is too long ($($firstExportPath.Length) characters): $firstExportPath"
  }
  $presentations = $app.Presentations
  $existingPresentationSnapshot = @()
  $sourcePath = [System.IO.Path]::GetFullPath($env:QLISA_DOC_SOURCE).TrimEnd('\').ToUpperInvariant()
  foreach ($existingPresentation in $presentations) {
    $existingFullName = [string]$existingPresentation.FullName
    if (-not [string]::IsNullOrWhiteSpace($existingFullName)) {
      $existingPath = [System.IO.Path]::GetFullPath($existingFullName).TrimEnd('\').ToUpperInvariant()
      if ($existingPath -eq $sourcePath) { throw 'QLISA_POWERPOINT_SOURCE_ALREADY_OPEN' }
    }
    $existingPresentationSnapshot += [pscustomobject]@{
      Name = [string]$existingPresentation.Name
      FullName = $existingFullName
      Slides = [int]$existingPresentation.Slides.Count
      Saved = [bool]$existingPresentation.Saved
    }
    [void][Runtime.InteropServices.Marshal]::ReleaseComObject($existingPresentation)
  }
  $existingPresentationCount = [int]$presentations.Count
  # Always open a private UUID copy; never pass the user's source path to COM.
  $presentation = $presentations.Open($env:QLISA_DOC_SOURCE, $true, $false, $false)
  $openedPath = [System.IO.Path]::GetFullPath([string]$presentation.FullName).TrimEnd('\').ToUpperInvariant()
  if ($openedPath -ne $sourcePath) { throw 'PowerPoint opened a path other than the private import copy' }
  $ownsPresentation = $true
  if ([int]$presentations.Count -ne $existingPresentationCount + 1) {
    throw "PowerPoint did not add exactly one private copy ($existingPresentationCount -> $([int]$presentations.Count))"
  }
  $count = [int]$presentation.Slides.Count
  if ($count -lt 1 -or $count -gt 500) { throw "Invalid slide count: $count" }
  $slideWidth = [double]$presentation.PageSetup.SlideWidth
  $slideHeight = [double]$presentation.PageSetup.SlideHeight
  if ($slideWidth -le 0 -or $slideHeight -le 0) { throw 'Invalid slide dimensions' }
  $scale = [Math]::Min(2.0, 1920.0 / [Math]::Max($slideWidth, $slideHeight))
  $width = [int][Math]::Max(1, [Math]::Round($slideWidth * $scale))
  $height = [int][Math]::Max(1, [Math]::Round($slideHeight * $scale))
  $totalBytes = [int64]0
  for ($index = 1; $index -le $count; $index++) {
    $path = Join-Path $outputDirectory ("page-{0:D4}.png" -f $index)
    try {
      $presentation.Slides.Item($index).Export($path, 'PNG', $width, $height)
    } catch {
      throw "PowerPoint Slide.Export failed at '$path' ($($path.Length) characters): $($_.Exception.ToString())"
    }
    $pageBytes = (Get-Item -LiteralPath $path).Length
    if ($pageBytes -gt 33554432) { throw "Slide $index PNG exceeds 32 MB" }
    $totalBytes += $pageBytes
    if ($totalBytes -gt 536870912) { throw 'Exported slides exceed 512 MB total' }
  }
  $presentation.Close()
  $ownsPresentation = $false
  [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($presentation)
  $presentation = $null
  if ([int]$presentations.Count -ne $existingPresentationCount) {
    throw 'PowerPoint presentation count changed after closing the private copy'
  }
  $currentPresentationSnapshot = @()
  foreach ($existingPresentation in $presentations) {
    $currentPresentationSnapshot += [pscustomobject]@{
      Name = [string]$existingPresentation.Name
      FullName = [string]$existingPresentation.FullName
      Slides = [int]$existingPresentation.Slides.Count
      Saved = [bool]$existingPresentation.Saved
    }
    [void][Runtime.InteropServices.Marshal]::ReleaseComObject($existingPresentation)
  }
  $beforeState = @($existingPresentationSnapshot | ForEach-Object { "$($_.Name)|$($_.FullName)|$($_.Slides)|$($_.Saved)" } | Sort-Object) -join "`n"
  $afterState = @($currentPresentationSnapshot | ForEach-Object { "$($_.Name)|$($_.FullName)|$($_.Slides)|$($_.Saved)" } | Sort-Object) -join "`n"
  if ($afterState -ne $beforeState) {
    throw 'An existing PowerPoint presentation changed during import'
  }
  $app.AutomationSecurity = $originalAutomationSecurity
  $app.DisplayAlerts = $originalDisplayAlerts
  $settingsCaptured = $false
  [Console]::Out.WriteLine("QLISA_SLIDES=$count")
} catch {
  [Console]::Error.WriteLine([string]$_.Exception.ToString())
  exit 1
} finally {
  if ($ownsPresentation -and $null -ne $presentation) { try { $presentation.Close() } catch {} }
  if ($settingsCaptured -and $null -ne $app) {
    try { $app.AutomationSecurity = $originalAutomationSecurity } catch {}
    try { $app.DisplayAlerts = $originalDisplayAlerts } catch {}
  }
  if ($ownsApplication -and $null -ne $app) { try { $app.Quit() } catch {} }
  if ($ownsPresentation -and $null -ne $presentation) { try { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($presentation) } catch {} }
  if ($null -ne $presentations) { try { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($presentations) } catch {} }
  if ($null -ne $app) { try { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app) } catch {} }
}
"#;

    let temp = std::env::temp_dir().join(format!("qlisa-ppt-worker-{}.ps1", uuid::Uuid::new_v4()));
    let owner_file =
        std::env::temp_dir().join(format!("qlisa-ppt-owner-{}.txt", uuid::Uuid::new_v4()));
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("pptx");
    let private_copy = std::env::temp_dir().join(format!(
        "qlisa-ppt-source-{}.{}",
        uuid::Uuid::new_v4(),
        extension
    ));
    let mut private_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&private_copy)
        .context("Не удалось создать частную копию презентации")?;
    let _private_copy = RemoveFile(private_copy.clone());
    let mut source_file = fs::File::open(source)
        .context("Не удалось открыть исходную презентацию для копирования")?;
    std::io::copy(&mut source_file, &mut private_file)
        .context("Не удалось записать частную копию презентации")?;
    private_file
        .sync_all()
        .context("Не удалось завершить частную копию презентации")?;
    drop(private_file);
    let private_copy_for_office = canonical_external_path(&private_copy)?;
    let destination_for_office = canonical_external_path(destination)?;
    fs::write(&temp, WORKER).context("Не удалось создать скрытый PowerPoint worker")?;
    let _script = RemoveFile(temp.clone());
    let _owner_file = RemoveFile(owner_file.clone());

    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&temp)
        .env("QLISA_DOC_SOURCE", &private_copy_for_office)
        .env("QLISA_DOC_DESTINATION", &destination_for_office)
        .env("QLISA_DOC_OWNER_FILE", &owner_file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("Не удалось запустить PowerPoint worker")?;

    let deadline = Instant::now() + Duration::from_secs(OFFICE_TIMEOUT_SECS);
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context("Не удалось проверить PowerPoint worker")?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            terminate_owned_powerpoint(&owner_file);
            close_private_powerpoint_copy(&private_copy_for_office);
            bail!("PowerPoint не завершил экспорт за 3 минуты.");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let output = child
        .wait_with_output()
        .context("Не удалось получить ответ PowerPoint worker")?;
    terminate_owned_powerpoint(&owner_file);
    // The worker normally closes the private copy in its finally block. This
    // exact-path cleanup also covers crashes and nonzero exits without
    // touching any presentation supplied by the user.
    close_private_powerpoint_copy(&private_copy_for_office);
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !status.success() || !stdout.lines().any(|line| line.starts_with("QLISA_SLIDES=")) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("QLISA_POWERPOINT_EXISTING_UNAVAILABLE") {
            bail!("Не удалось безопасно подключиться к уже открытому Microsoft PowerPoint. Закройте PowerPoint и повторите импорт.");
        }
        if stderr.contains("QLISA_POWERPOINT_SOURCE_ALREADY_OPEN") {
            bail!("Эта презентация уже открыта в Microsoft PowerPoint. Закройте её и повторите импорт.");
        }
        if stderr.contains("Microsoft PowerPoint automation is not installed or registered") {
            bail!("Для импорта презентаций установите Microsoft PowerPoint или сохраните презентацию как PDF.");
        }
        let diagnostic = format!(
            "worker exit={:?}; stdout: {}; stderr: {}",
            status.code(),
            if stdout.trim().is_empty() {
                "(empty)"
            } else {
                stdout.trim()
            },
            if stderr.trim().is_empty() {
                "(empty)"
            } else {
                stderr.trim()
            }
        );
        bail!("Microsoft PowerPoint не смог экспортировать презентацию: {diagnostic}");
    }
    Ok(())
}

#[cfg(not(windows))]
fn export_with_powerpoint(_source: &Path, _destination: &Path) -> Result<()> {
    bail!("Экспорт PPT/PPTX через PowerPoint доступен только в Windows.");
}

#[cfg(windows)]
fn convert_with_libreoffice(source: &Path, stage: &Path) -> Result<PathBuf> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let executable =
        find_libreoffice().ok_or_else(|| anyhow!("Резервный LibreOffice не установлен."))?;
    let output_dir = stage.join("lo-pdf");
    fs::create_dir(&output_dir).context("Не удалось создать папку конвертации LibreOffice")?;
    let profile = stage.join("lo-profile");
    fs::create_dir(&profile).context("Не удалось создать профиль LibreOffice")?;
    let profile_uri = url::Url::from_directory_path(&profile)
        .map_err(|_| anyhow!("Не удалось подготовить изолированный профиль LibreOffice"))?;

    let mut child = Command::new(executable)
        .args([
            "--headless",
            "--nologo",
            "--nodefault",
            "--nolockcheck",
            "--nofirststartwizard",
        ])
        .arg(format!("-env:UserInstallation={profile_uri}"))
        .arg("--convert-to")
        .arg("pdf:impress_pdf_Export")
        .arg("--outdir")
        .arg(&output_dir)
        .arg(source)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("Не удалось запустить резервный LibreOffice")?;

    let deadline = Instant::now() + Duration::from_secs(LIBREOFFICE_TIMEOUT_SECS);
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context("Не удалось проверить LibreOffice")?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("LibreOffice не завершил конвертацию за 3 минуты.");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let output = child
        .wait_with_output()
        .context("Не удалось получить ответ LibreOffice")?;
    if !status.success() {
        bail!(
            "LibreOffice не смог открыть презентацию: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stem = source.file_stem().unwrap_or_default();
    let pdf_path = output_dir.join(format!("{}.pdf", stem.to_string_lossy()));
    if !pdf_path.is_file() {
        bail!("LibreOffice не создал PDF страницы презентации.");
    }
    Ok(pdf_path)
}

#[cfg(not(windows))]
fn convert_with_libreoffice(_source: &Path, _stage: &Path) -> Result<PathBuf> {
    bail!("LibreOffice headless доступен только в Windows в этом импортёре.");
}

#[cfg(windows)]
fn find_libreoffice() -> Option<PathBuf> {
    use std::process::Command;

    let mut candidates = Vec::new();
    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(variable).map(PathBuf::from) {
            candidates.push(root.join("LibreOffice/program/soffice.exe"));
            if let Ok(entries) = fs::read_dir(&root) {
                candidates.extend(
                    entries
                        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                        .filter(|path| {
                            path.file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(|name| name.starts_with("LibreOffice"))
                        })
                        .map(|path| path.join("program/soffice.exe")),
                );
            }
        }
    }
    if let Ok(output) = Command::new("where.exe").arg("soffice.exe").output() {
        if output.status.success() {
            candidates.extend(
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(PathBuf::from),
            );
        }
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

#[cfg(not(windows))]
fn find_libreoffice() -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn terminate_owned_powerpoint(owner_file: &Path) {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CLEANUP: &str = r#"
$parts = (Get-Content -LiteralPath $env:QLISA_DOC_OWNER_FILE -Raw -ErrorAction SilentlyContinue).Trim() -split '\|'
if ($parts.Count -ne 2) { exit 0 }
$process = Get-Process -Id ([int]$parts[0]) -ErrorAction SilentlyContinue
if ($null -ne $process -and $process.StartTime.ToUniversalTime().Ticks -eq [int64]$parts[1]) {
  try { $process.Kill(); [void]$process.WaitForExit(5000) } catch {}
}
"#;
    let _ = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", CLEANUP])
        .env("QLISA_DOC_OWNER_FILE", owner_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(windows)]
fn close_private_powerpoint_copy(private_copy: &Path) {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CLEANUP: &str = r#"
try { $app = [Runtime.InteropServices.Marshal]::GetActiveObject('PowerPoint.Application') }
catch { exit 0 }
$presentations = $null
try {
  $presentations = $app.Presentations
  $wanted = [System.IO.Path]::GetFullPath($env:QLISA_DOC_PRIVATE_COPY).TrimEnd('\').ToUpperInvariant()
  foreach ($presentation in $presentations) {
    $fullName = [string]$presentation.FullName
    if (-not [string]::IsNullOrWhiteSpace($fullName) -and
        [System.IO.Path]::GetFullPath($fullName).TrimEnd('\').ToUpperInvariant() -eq $wanted) {
      $presentation.Close()
      [void][Runtime.InteropServices.Marshal]::ReleaseComObject($presentation)
      break
    }
    [void][Runtime.InteropServices.Marshal]::ReleaseComObject($presentation)
  }
} catch {} finally {
  if ($null -ne $presentations) { try { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($presentations) } catch {} }
  if ($null -ne $app) { try { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($app) } catch {} }
}
"#;
    let Ok(mut child) = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-STA", "-Command", CLEANUP])
        .env("QLISA_DOC_PRIVATE_COPY", private_copy)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

#[cfg(not(windows))]
fn close_private_powerpoint_copy(_private_copy: &Path) {}

#[cfg(not(windows))]
fn terminate_owned_powerpoint(_owner_file: &Path) {}

struct RemoveFile(PathBuf);

impl Drop for RemoveFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_dimensions_preserve_aspect_and_cap_long_side() {
        let (width, height) = page_dimensions(612.0, 792.0).unwrap();
        assert_eq!((width, height), (1224, 1584));
        let (width, height) = page_dimensions(3000.0, 1000.0).unwrap();
        assert_eq!((width, height), (1920, 640));
        assert!(page_dimensions(0.0, 100.0).is_err());
    }

    #[test]
    fn page_names_sort_numerically_after_nine_pages() {
        let names = (1..=12).map(page_file_name).collect::<Vec<_>>();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert_eq!(names[0], "page-0001.png");
        assert_eq!(names[9], "page-0010.png");
        assert_eq!(page_file_name(10_000), "page-10000.png");
    }

    #[test]
    fn png_validation_rejects_invalid_and_overlarge_dimensions() {
        let path = std::env::temp_dir().join(format!("qlisa-doc-png-{}.png", uuid::Uuid::new_v4()));
        let mut bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        bytes.extend(1921_u32.to_be_bytes());
        bytes.extend(10_u32.to_be_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(validate_png(&path).is_err());
        let _ = fs::remove_file(path);
    }

    #[cfg(windows)]
    #[test]
    fn renders_twelve_page_pdf_from_unicode_path_without_modifying_source() {
        let (_root_guard, temp_root) = StageDirectory::create_temp("qldi-").unwrap();
        let root = temp_root.join(format!("qlisa документ {}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let source = root.join("пример #1.pdf");
        let destination = root.join("страницы");
        fs::create_dir(&destination).unwrap();
        fs::write(&source, twelve_page_pdf()).unwrap();
        let before = fs::read(&source).unwrap();

        let prepared = prepare_document(&source, &destination).unwrap();

        assert_eq!(prepared.title, "пример #1");
        assert_eq!(prepared.pages.len(), 12);
        assert_eq!(prepared.pages[9].file_name().unwrap(), "page-0010.png");
        assert!(prepared.pages.iter().all(|page| validate_png(page).is_ok()));
        assert_eq!(fs::read(&source).unwrap(), before);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires QLISA_TEST_PPTX_SOURCE and installed Microsoft PowerPoint"]
    fn imports_real_pptx_to_unicode_production_depth_destination() {
        use sha2::{Digest, Sha256};

        let fixture = std::env::var_os("QLISA_TEST_PPTX_SOURCE")
            .map(PathBuf::from)
            .expect("set QLISA_TEST_PPTX_SOURCE to a synthetic PPTX fixture");
        let (_root_guard, root) = StageDirectory::create_temp("qldi-").unwrap();

        let source = root.join("ТУРНИРНАЯ ТАБЛИЦА — длинное имя #1.pptx");
        fs::copy(&fixture, &source).unwrap();
        let source_before = fs::read(&source).unwrap();
        let source_hash_before = format!("{:x}", Sha256::digest(&source_before));
        let destination = root
            .join("AppData")
            .join("Roaming")
            .join("com.qlisa.app")
            .join("document-imports")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&destination).unwrap();

        // This models the old, deeply nested Office export path below a
        // production import destination. The current renderer must not use it.
        let old_export_path = destination
            .join(format!(".qlisa-import-{}", uuid::Uuid::new_v4()))
            .join("powerpoint-output")
            .join("page-0001.png");
        assert!(old_export_path.to_string_lossy().chars().count() >= 170);

        let temp_root = std::env::temp_dir();
        let mut before_staging = fs::read_dir(&temp_root)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("qld-"))
            })
            .collect::<Vec<_>>();
        before_staging.sort();

        let prepared = prepare_document(&source, &destination).unwrap();

        assert_eq!(prepared.title, "ТУРНИРНАЯ ТАБЛИЦА — длинное имя #1");
        assert!(!prepared.pages.is_empty());
        assert!(prepared.pages.iter().all(|page| {
            page.starts_with(&destination)
                && page
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".png"))
                && validate_png(page).is_ok()
        }));
        assert!(prepared
            .warnings
            .iter()
            .all(|warning| !warning.contains("LibreOffice")));
        assert_eq!(fs::read(&source).unwrap(), source_before);
        let source_hash_after = format!("{:x}", Sha256::digest(&fs::read(&source).unwrap()));
        assert_eq!(source_hash_after, source_hash_before);
        let destination_entries = fs::read_dir(&destination)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(destination_entries.len(), prepared.pages.len());
        assert!(destination_entries.iter().all(|path| path.is_file()));

        let mut after_staging = fs::read_dir(&temp_root)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("qld-"))
            })
            .collect::<Vec<_>>();
        after_staging.sort();
        assert_eq!(after_staging, before_staging, "short staging folder leaked");
        let dimensions = validate_png(&prepared.pages[0]).unwrap();
        println!(
            "PowerPoint import: {} pages, first page {}x{}, old destination path {} chars, source SHA256 {}",
            prepared.pages.len(),
            dimensions.0,
            dimensions.1,
            old_export_path.to_string_lossy().chars().count(),
            source_hash_after
        );
    }

    #[cfg(windows)]
    fn twelve_page_pdf() -> Vec<u8> {
        let page_count = 12_usize;
        let mut objects = Vec::new();
        objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_string());
        let kids = (0..page_count)
            .map(|index| format!("{} 0 R", 3 + index * 2))
            .collect::<Vec<_>>()
            .join(" ");
        objects.push(format!(
            "<< /Type /Pages /Kids [{kids}] /Count {page_count} >>"
        ));
        for index in 0..page_count {
            let page_object = 3 + index * 2;
            let contents_object = page_object + 1;
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> /Contents {contents_object} 0 R >>"
            ));
            let red = (index + 1) as f64 / page_count as f64;
            let content = format!("{red:.3} 0.25 0.75 rg 0 0 612 792 re f\n");
            objects.push(format!(
                "<< /Length {} >>\nstream\n{}endstream",
                content.len(),
                content
            ));
        }
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }
}
