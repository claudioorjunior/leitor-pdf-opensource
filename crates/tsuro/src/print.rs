//! PDF de impressão: jobs em ~200 DPI + montagem via `pdf-writer`.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use miniz_oxide::deflate::compress_to_vec_zlib;
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref};

use crate::page::{Bitmap, MediaBox, PageEngine, PageNo, Scale};

/// DPI de impressão da v1 (user space PDF = 72 DPI).
pub const PRINT_DPI: f32 = 200.0;
const PDF_USER_SPACE_DPI: f32 = 72.0;
static PRINT_FILE_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct PrintError(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintJob {
    pub page: PageNo,
    pub scale: Scale,
}

pub fn print_scale(dpi: f32) -> Scale {
    Scale::from_factor(dpi / PDF_USER_SPACE_DPI)
}

/// N páginas → N jobs na ordem, sem IO.
pub fn print_pages(page_count: u32, dpi: f32) -> Vec<PrintJob> {
    let scale = print_scale(dpi);
    (0..page_count)
        .map(|index| PrintJob {
            page: PageNo::from_index(index),
            scale,
        })
        .collect()
}

pub fn print_document(engine: &impl PageEngine) -> Result<Vec<u8>, PrintError> {
    let jobs = print_pages(engine.page_count(), PRINT_DPI);
    assemble_print_pages(
        jobs.len(),
        jobs.into_iter().map(|job| {
            let media = engine
                .media(job.page)
                .map_err(|err| PrintError(err.to_string()))?;
            let surface = engine
                .render(job.page, job.scale)
                .map_err(|err| PrintError(err.to_string()))?;
            Ok((surface.bitmap, media))
        }),
    )
}

#[cfg(test)]
pub fn assemble_print_pdf(pages: &[(Bitmap, MediaBox)]) -> Result<Vec<u8>, PrintError> {
    assemble_print_pages(
        pages.len(),
        pages
            .iter()
            .map(|(bitmap, media)| Ok((bitmap.clone(), *media))),
    )
}

fn assemble_print_pages<I>(count: usize, pages: I) -> Result<Vec<u8>, PrintError>
where
    I: IntoIterator<Item = Result<(Bitmap, MediaBox), PrintError>>,
{
    if count == 0 {
        return Err(PrintError("nenhuma página para imprimir".into()));
    }

    let mut pdf = Pdf::new();
    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let mut next = 3;
    let mut alloc = || {
        let id = Ref::new(next);
        next += 1;
        id
    };

    let slots: Vec<(Ref, Ref, Ref)> = (0..count).map(|_| (alloc(), alloc(), alloc())).collect();
    let kids: Vec<Ref> = slots.iter().map(|slot| slot.0).collect();
    let page_count = i32::try_from(count)
        .map_err(|_| PrintError("documento grande demais para imprimir".into()))?;

    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id).kids(kids).count(page_count);

    let mut pages = pages.into_iter();
    for (index, slot) in slots.into_iter().enumerate() {
        let (bitmap, media) = pages
            .next()
            .ok_or_else(|| PrintError("página em falta".into()))??;
        write_print_page(&mut pdf, page_tree_id, slot, index, &bitmap, media)?;
    }

    Ok(pdf.finish())
}

fn write_print_page(
    pdf: &mut Pdf,
    page_tree_id: Ref,
    slot: (Ref, Ref, Ref),
    index: usize,
    bitmap: &Bitmap,
    media: MediaBox,
) -> Result<(), PrintError> {
    let (page_id, image_id, content_id) = slot;
    let width_pt = finite_positive(media.width)?;
    let height_pt = finite_positive(media.height)?;
    let name = format!("Im{}", index + 1);
    let image_name = Name(name.as_bytes());

    let mut page = pdf.page(page_id);
    page.media_box(Rect::new(0.0, 0.0, width_pt, height_pt));
    page.parent(page_tree_id);
    page.contents(content_id);
    page.resources().x_objects().pair(image_name, image_id);
    page.finish();

    let rgb = rgba_to_rgb(&bitmap.rgba, bitmap.width, bitmap.height)?;
    let compressed = compress_to_vec_zlib(&rgb, 6);
    let width =
        i32::try_from(bitmap.width).map_err(|_| PrintError("largura de bitmap inválida".into()))?;
    let height =
        i32::try_from(bitmap.height).map_err(|_| PrintError("altura de bitmap inválida".into()))?;
    let mut image = pdf.image_xobject(image_id, &compressed);
    image.filter(Filter::FlateDecode);
    image.width(width);
    image.height(height);
    image.color_space().device_rgb();
    image.bits_per_component(8);
    image.finish();

    let mut content = Content::new();
    content.save_state();
    content.transform([width_pt, 0.0, 0.0, height_pt, 0.0, 0.0]);
    content.x_object(image_name);
    content.restore_state();
    pdf.stream(content_id, &content.finish());
    Ok(())
}

pub fn write_and_open_print_pdf(bytes: &[u8], source: &Path) -> Result<PathBuf, PrintError> {
    let mut last_err = None;
    for _ in 0..8 {
        let path = print_temp_path(source);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(err) = file.write_all(bytes) {
                    let _ = std::fs::remove_file(&path);
                    return Err(PrintError(format!(
                        "não foi possível gravar o PDF de impressão: {err}"
                    )));
                }
                open_with_system_viewer(&path)?;
                return Ok(path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                last_err = Some(err);
            }
            Err(err) => {
                return Err(PrintError(format!(
                    "não foi possível gravar o PDF de impressão: {err}"
                )));
            }
        }
    }
    Err(PrintError(format!(
        "não foi possível gravar o PDF de impressão: {}",
        last_err
            .map(|err| err.to_string())
            .unwrap_or_else(|| "arquivo temporário em uso".into())
    )))
}

pub fn print_temp_path(source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("documento")
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .take(40)
        .collect::<String>();
    let stem = if stem.is_empty() {
        "documento".to_string()
    } else {
        stem
    };
    let seq = PRINT_FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "tsuro-print-{}-{}-{}.pdf",
        stem,
        std::process::id(),
        seq
    ))
}

pub fn open_with_system_viewer(path: &Path) -> Result<(), PrintError> {
    #[cfg(target_os = "macos")]
    {
        // Não usar `open arquivo.pdf`: o handler padrão pode ser um editor
        // (PDFgear, Adobe, etc.). Imprimir da v1 é o Preview, onde o usuário
        // confirma a impressora com Cmd+P.
        if run_viewer(macos_preview_command(path))? {
            return Ok(());
        }
        let mut fallback = Command::new("open");
        fallback.arg(path);
        if run_viewer(fallback)? {
            return Ok(());
        }
        return Err(PrintError(
            "não foi possível abrir o Preview para imprimir".into(),
        ));
    }
    #[cfg(not(target_os = "macos"))]
    {
        run_viewer(default_viewer_command(path)).and_then(|ok| {
            if ok {
                Ok(())
            } else {
                Err(PrintError("não foi possível abrir o visualizador".into()))
            }
        })
    }
}

#[cfg(target_os = "macos")]
fn macos_preview_command(path: &Path) -> Command {
    let mut command = Command::new("open");
    command.args(["-b", "com.apple.Preview"]);
    command.arg(path);
    command
}

#[cfg(not(target_os = "macos"))]
fn default_viewer_command(path: &Path) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", ""]);
        command.arg(path);
        command
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut command = Command::new("xdg-open");
        command.arg(path);
        command
    }
}

fn run_viewer(mut command: Command) -> Result<bool, PrintError> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match command.status() {
        Ok(status) => Ok(status.success()),
        Err(err) => Err(PrintError(format!(
            "não foi possível abrir o visualizador: {err}"
        ))),
    }
}

fn finite_positive(value: f32) -> Result<f32, PrintError> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(PrintError("MediaBox inválida".into()))
    }
}

fn rgba_to_rgb(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, PrintError> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| PrintError("bitmap grande demais".into()))?;
    let expected = pixels
        .checked_mul(4)
        .ok_or_else(|| PrintError("bitmap grande demais".into()))?;
    if rgba.len() != expected {
        return Err(PrintError("bitmap RGBA incompleto".into()));
    }
    let mut rgb = Vec::with_capacity(pixels * 3);
    for pixel in rgba.chunks_exact(4) {
        let alpha = u16::from(pixel[3]);
        if alpha == 255 {
            rgb.extend_from_slice(&pixel[..3]);
        } else if alpha == 0 {
            rgb.extend_from_slice(&[255, 255, 255]);
        } else {

[Showing lines 1-300 of 449. Use :301 to continue]