//! PDF de impressão: seleção (intervalo/cópias/orientação) + montagem via `pdf-writer`.
//!
//! Intervalo, cópias e orientação vão assados no PDF: cada página única vira um
//! Image XObject compartilhado pelas suas cópias.

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
/// Teto do stepper de cópias: limita tempo de montagem e tamanho do PDF.
pub const MAX_COPIES: u32 = 99;
static PRINT_FILE_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct PrintError(pub String);

/// Intervalo do diálogo de impressão (v1: sem lista livre tipo "1-3, 5").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrintRange {
    #[default]
    All,
    Current(PageNo),
    FromTo {
        from: PageNo,
        to: PageNo,
    },
}

/// Orientação do diálogo; `Auto` mantém cada página como está.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrintOrientation {
    #[default]
    Auto,
    Portrait,
    Landscape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintSelection {
    pub range: PrintRange,
    pub copies: u32,
    pub orientation: PrintOrientation,
}

pub fn print_scale(dpi: f32) -> Scale {
    Scale::from_factor(dpi / PDF_USER_SPACE_DPI)
}

/// Monta o PDF da seleção do diálogo: filtra o intervalo, aplica a orientação e
/// repete cada página `copies` vezes (cópias assadas no PDF, 1..=MAX_COPIES).
pub fn print_selection_pdf(
    engine: &impl PageEngine,
    selection: PrintSelection,
) -> Result<Vec<u8>, PrintError> {
    let pages = resolve_range(selection.range, engine.page_count())?;
    let scale = print_scale(PRINT_DPI);
    let mut rendered = Vec::with_capacity(pages.len());
    for page in pages {
        let media = engine
            .media(page)
            .map_err(|err| PrintError(err.to_string()))?;
        let surface = engine
            .render(page, scale)
            .map_err(|err| PrintError(err.to_string()))?;
        rendered.push(apply_orientation(
            &surface.bitmap,
            media,
            selection.orientation,
        )?);
    }
    assemble_print_pages(&rendered, selection.copies)
}

/// Intervalo → páginas em ordem; valida contra o total (`PageNo` é 0-based).
pub fn resolve_range(range: PrintRange, page_count: u32) -> Result<Vec<PageNo>, PrintError> {
    if page_count == 0 {
        return Err(PrintError("nenhuma página para imprimir".into()));
    }
    let valid = |page: PageNo| page.index() < page_count;
    match range {
        PrintRange::All => Ok((0..page_count).map(PageNo::from_index).collect()),
        PrintRange::Current(page) if valid(page) => Ok(vec![page]),
        PrintRange::FromTo { from, to }
            if valid(from) && valid(to) && from.index() <= to.index() =>
        {
            Ok((from.index()..=to.index())
                .map(PageNo::from_index)
                .collect())
        }
        _ => Err(PrintError("intervalo de páginas inválido".into())),
    }
}

/// Retrato gira páginas paisagem (e vice-versa); quadrada e `Auto` não mexem.
fn apply_orientation(
    bitmap: &Bitmap,
    media: MediaBox,
    orientation: PrintOrientation,
) -> Result<(Bitmap, MediaBox), PrintError> {
    let landscape = media.width > media.height;
    let portrait = media.height > media.width;
    let rotate = matches!(
        (orientation, landscape, portrait),
        (PrintOrientation::Portrait, true, _) | (PrintOrientation::Landscape, _, true)
    );
    if !rotate {
        return Ok((bitmap.clone(), media));
    }
    let (rgba, width, height) = rotate_rgba_90_cw(&bitmap.rgba, bitmap.width, bitmap.height)?;
    Ok((
        Bitmap {
            width,
            height,
            rgba,
        },
        MediaBox {
            width: media.height,
            height: media.width,
        },
    ))
}

fn rotate_rgba_90_cw(
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<(Vec<u8>, u32, u32), PrintError> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| PrintError("bitmap grande demais".into()))?;
    if rgba.len() != expected || width == 0 || height == 0 {
        return Err(PrintError("bitmap RGBA incompleto".into()));
    }
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; expected];
    for y in 0..w {
        for x in 0..h {
            let src = ((h - 1 - x) * w + y) * 4;
            let dst = (y * h + x) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    Ok((out, height, width))
}

#[cfg(test)]
pub fn assemble_test_pdf(pages: &[(Bitmap, MediaBox)], copies: u32) -> Result<Vec<u8>, PrintError> {
    assemble_print_pages(pages, copies)
}

/// Páginas únicas → 1 Image XObject cada, compartilhado pelas `copies` cópias.
fn assemble_print_pages(pages: &[(Bitmap, MediaBox)], copies: u32) -> Result<Vec<u8>, PrintError> {
    if pages.is_empty() {
        return Err(PrintError("nenhuma página para imprimir".into()));
    }
    let copies = copies.clamp(1, MAX_COPIES) as usize;
    let total = pages
        .len()
        .checked_mul(copies)
        .ok_or_else(|| PrintError("documento grande demais para imprimir".into()))?;
    let total_count = i32::try_from(total)
        .map_err(|_| PrintError("documento grande demais para imprimir".into()))?;

    let mut pdf = Pdf::new();
    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let mut next = 3;
    let mut alloc = || {
        let id = Ref::new(next);
        next += 1;
        id
    };

    let image_ids: Vec<Ref> = (0..pages.len()).map(|_| alloc()).collect();
    let slots: Vec<(Ref, Ref)> = (0..total).map(|_| (alloc(), alloc())).collect();
    let kids: Vec<Ref> = slots.iter().map(|slot| slot.0).collect();

    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id).kids(kids).count(total_count);

    for (index, (bitmap, _)) in pages.iter().enumerate() {
        write_print_image(&mut pdf, image_ids[index], bitmap)?;
    }
    for (slot_index, (page_id, content_id)) in slots.into_iter().enumerate() {
        let unique = slot_index / copies;
        write_print_page(
            &mut pdf,
            page_tree_id,
            page_id,
            content_id,
            image_ids[unique],
            unique,
            &pages[unique].1,
        )?;
    }

    Ok(pdf.finish())
}

fn write_print_image(pdf: &mut Pdf, image_id: Ref, bitmap: &Bitmap) -> Result<(), PrintError> {
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
    Ok(())
}

fn write_print_page(
    pdf: &mut Pdf,
    page_tree_id: Ref,
    page_id: Ref,
    content_id: Ref,
    image_id: Ref,
    image_index: usize,
    media: &MediaBox,
) -> Result<(), PrintError> {
    let width_pt = finite_positive(media.width)?;
    let height_pt = finite_positive(media.height)?;
    let name = format!("Im{}", image_index + 1);
    let image_name = Name(name.as_bytes());

    let mut page = pdf.page(page_id);
    page.media_box(Rect::new(0.0, 0.0, width_pt, height_pt));
    page.parent(page_tree_id);
    page.contents(content_id);
    page.resources().x_objects().pair(image_name, image_id);
    page.finish();

    let mut content = Content::new();
    content.save_state();
    content.transform([width_pt, 0.0, 0.0, height_pt, 0.0, 0.0]);
    content.x_object(image_name);
    content.restore_state();
    pdf.stream(content_id, &content.finish());
    Ok(())
}

/// Rota de fuga do diálogo ("Abrir PDF"): grava o PDF da seleção em temp e abre
/// no visualizador padrão, em todas as plataformas.
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
                open_fallback_viewer(&path)?;
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

fn open_fallback_viewer(path: &Path) -> Result<(), PrintError> {
    let mut command = {
        #[cfg(target_os = "windows")]
        {
            let mut command = Command::new("cmd");
            command.args(["/C", "start", ""]);
            command.arg(path);
            command
        }
        #[cfg(target_os = "macos")]
        {
            let mut command = Command::new("open");
            command.arg(path);
            command
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let mut command = Command::new("xdg-open");
            command.arg(path);
            command
        }
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|err| PrintError(format!("não foi possível abrir o visualizador: {err}")))
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(PrintError("não foi possível abrir o visualizador".into()))
            }
        })
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
            let inv = 255 - alpha;
            rgb.push(((u16::from(pixel[0]) * alpha + 255 * inv) / 255) as u8);
            rgb.push(((u16::from(pixel[1]) * alpha + 255 * inv) / 255) as u8);
            rgb.push(((u16::from(pixel[2]) * alpha + 255 * inv) / 255) as u8);
        }
    }
    Ok(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::PdfiumEngine;
    use tsuro_sign::analyze_pdf;

    fn solid_rgba(width: u32, height: u32, rgb: [u8; 3]) -> Bitmap {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        Bitmap {
            width,
            height,
            rgba,
        }
    }

    fn media_boxes(bytes: &[u8]) -> Vec<(f32, f32)> {
        let text = String::from_utf8_lossy(bytes);
        let mut out = Vec::new();
        let mut rest = text.as_ref();
        while let Some(at) = rest.find("/MediaBox") {
            rest = &rest[at + "/MediaBox".len()..];
            let Some(open) = rest.find('[') else {
                break;
            };
            let Some(close) = rest[open + 1..].find(']') else {
                break;
            };
            let inner = rest[open + 1..open + 1 + close].split_whitespace();
            let nums: Vec<f32> = inner.filter_map(|part| part.parse().ok()).collect();
            if nums.len() == 4 {
                out.push((nums[2] - nums[0], nums[3] - nums[1]));
            }
            rest = &rest[open + 1 + close..];
        }
        out
    }

    #[test]
    fn print_scale_matches_dpi_over_user_space() {
        let scale = print_scale(PRINT_DPI);
        assert!((scale.factor() - PRINT_DPI / PDF_USER_SPACE_DPI).abs() < 0.002);
    }

    #[test]
    fn assemble_print_pdf_keeps_count_and_mediabox() {
        let pages = [
            (
                solid_rgba(8, 12, [255, 0, 0]),
                MediaBox {
                    width: 200.0,
                    height: 300.0,
                },
            ),
            (
                solid_rgba(10, 6, [0, 0, 255]),
                MediaBox {
                    width: 400.0,
                    height: 240.0,
                },
            ),
        ];
        let bytes = assemble_test_pdf(&pages, 1).expect("pdf");
        let analysis = analyze_pdf(&bytes).expect("parse");
        assert_eq!(analysis.page_count_hint, Some(2));
        assert!(analysis.signatures.is_empty());
        let boxes = media_boxes(&bytes);
        assert_eq!(boxes.len(), 2);
        assert!((boxes[0].0 - 200.0).abs() < 0.01 && (boxes[0].1 - 300.0).abs() < 0.01);
        assert!((boxes[1].0 - 400.0).abs() < 0.01 && (boxes[1].1 - 240.0).abs() < 0.01);
    }

    #[test]
    fn assemble_print_pdf_rejects_empty() {
        assert!(assemble_test_pdf(&[], 1).is_err());
    }

    #[test]
    fn copies_expand_pages_and_clamp() {
        let pages = [(
            solid_rgba(4, 4, [0, 255, 0]),
            MediaBox {
                width: 100.0,
                height: 100.0,
            },
        )];
        let bytes = assemble_test_pdf(&pages, 3).expect("pdf");
        let analysis = analyze_pdf(&bytes).expect("parse");
        assert_eq!(analysis.page_count_hint, Some(3));
        assert_eq!(media_boxes(&bytes).len(), 3);
        // 0 prende em 1; acima do teto prende em MAX_COPIES.
        let one = assemble_test_pdf(&pages, 0).expect("pdf");
        assert_eq!(media_boxes(&one).len(), 1);
        let many = assemble_test_pdf(&pages, MAX_COPIES + 50).expect("pdf");
        assert_eq!(media_boxes(&many).len(), MAX_COPIES as usize);
    }

    #[test]
    fn resolve_range_covers_all_current_and_from_to() {
        let all = resolve_range(PrintRange::All, 3).expect("all");
        assert_eq!(
            all.iter().map(|p| p.index()).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        let current =
            resolve_range(PrintRange::Current(PageNo::from_index(1)), 3).expect("current");
        assert_eq!(
            current.iter().map(|p| p.index()).collect::<Vec<_>>(),
            vec![1]
        );
        let span = resolve_range(
            PrintRange::FromTo {
                from: PageNo::from_index(1),
                to: PageNo::from_index(2),
            },
            3,
        )
        .expect("span");
        assert_eq!(
            span.iter().map(|p| p.index()).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(resolve_range(PrintRange::All, 0).is_err());
        assert!(resolve_range(PrintRange::Current(PageNo::from_index(5)), 3).is_err());
        assert!(resolve_range(
            PrintRange::FromTo {
                from: PageNo::from_index(2),
                to: PageNo::from_index(1),
            },
            3,
        )
        .is_err());
        assert!(resolve_range(
            PrintRange::FromTo {
                from: PageNo::from_index(0),
                to: PageNo::from_index(9),
            },
            3,
        )
        .is_err());
    }

    #[test]
    fn orientation_rotates_bitmap_and_media() {
        let wide = solid_rgba(4, 2, [255, 0, 0]);
        let media = MediaBox {
            width: 400.0,
            height: 200.0,
        };
        // Paisagem pede paisagem: nada muda.
        let (same, same_media) =
            apply_orientation(&wide, media, PrintOrientation::Landscape).expect("same");
        assert_eq!((same.width, same.height), (4, 2));
        assert_eq!((same_media.width, same_media.height), (400.0, 200.0));
        // Retrato gira: 4x2 vira 2x4, MediaBox troca.
        let (tall, tall_media) =
            apply_orientation(&wide, media, PrintOrientation::Portrait).expect("rot");
        assert_eq!((tall.width, tall.height), (2, 4));
        assert_eq!((tall_media.width, tall_media.height), (200.0, 400.0));
        // Quadrada nunca gira.
        let square = solid_rgba(3, 3, [0, 0, 255]);
        let sq_media = MediaBox {
            width: 100.0,
            height: 100.0,
        };
        let (kept, _) =
            apply_orientation(&square, sq_media, PrintOrientation::Portrait).expect("sq");
        assert_eq!((kept.width, kept.height), (3, 3));
    }

    #[test]
    fn rotate_90_cw_maps_corners() {
        // 2x1: [A][B] vira 1x2 com A em cima (horário).
        let rgba = vec![1, 0, 0, 255, 2, 0, 0, 255];
        let (out, w, h) = rotate_rgba_90_cw(&rgba, 2, 1).expect("rot");
        assert_eq!((w, h), (1, 2));
        assert_eq!(out, vec![1, 0, 0, 255, 2, 0, 0, 255]);
        // 1x2: [A]/[B] vira 2x1 [B][A].
        let rgba = vec![1, 0, 0, 255, 2, 0, 0, 255];
        let (out, w, h) = rotate_rgba_90_cw(&rgba, 1, 2).expect("rot");
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, vec![2, 0, 0, 255, 1, 0, 0, 255]);
        assert!(rotate_rgba_90_cw(&[0u8; 3], 1, 1).is_err());
    }

    #[test]
    fn transparent_pixels_flatten_to_white() {
        let mut rgba = vec![0, 0, 0, 0, 10, 20, 30, 255];
        let rgb = rgba_to_rgb(&rgba, 2, 1).expect("rgb");
        assert_eq!(rgb, vec![255, 255, 255, 10, 20, 30]);
        rgba.truncate(3);
        assert!(rgba_to_rgb(&rgba, 1, 1).is_err());
    }

    #[test]
    fn print_temp_path_sanitizes_stem() {
        let path = print_temp_path(Path::new("/tmp/Contrato (final).PDF"));
        let name = path.file_name().and_then(|n| n.to_str()).unwrap();
        assert!(name.starts_with("tsuro-print-Contratofinal-"));
        assert!(name.ends_with(".pdf"));
    }

    #[test]
    fn print_selection_from_fixture_expands_range_and_copies() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../public/samples/guia-folio.pdf");
        let bytes = std::fs::read(&path).expect("fixture PDF required");
        let engine =
            PdfiumEngine::open(std::sync::Arc::<[u8]>::from(bytes)).expect("fixture PDF required");
        let last = engine.page_count().saturating_sub(1).min(1);
        let span = last + 1;
        let printed = print_selection_pdf(
            &engine,
            PrintSelection {
                range: PrintRange::FromTo {
                    from: PageNo::from_index(0),
                    to: PageNo::from_index(last),
                },
                copies: 2,
                orientation: PrintOrientation::Auto,
            },
        )
        .expect("selection pdf");
        let analysis = analyze_pdf(&printed).expect("parse");
        assert_eq!(analysis.page_count_hint, Some(span * 2));
    }

    #[test]
    fn print_document_from_fixture_roundtrips_pages() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../public/samples/guia-folio.pdf");
        let bytes = std::fs::read(&path).expect("fixture PDF required");
        let engine =
            PdfiumEngine::open(std::sync::Arc::<[u8]>::from(bytes)).expect("fixture PDF required");
        let printed = print_selection_pdf(
            &engine,
            PrintSelection {
                range: PrintRange::All,
                copies: 1,
                orientation: PrintOrientation::Auto,
            },
        )
        .expect("print pdf");
        let analysis = analyze_pdf(&printed).expect("parse printed pdf");
        assert_eq!(analysis.page_count_hint, Some(engine.page_count()));
        let boxes = media_boxes(&printed);
        assert_eq!(boxes.len(), engine.page_count() as usize);
        for (index, (width, height)) in boxes.into_iter().enumerate() {
            let media = engine
                .media(PageNo::from_index(index as u32))
                .expect("media");
            assert!(
                (width - media.width).abs() < 0.5 && (height - media.height).abs() < 0.5,
                "page {index} mediabox {width}x{height} vs {}x{}",
                media.width,
                media.height
            );
        }
    }
}
