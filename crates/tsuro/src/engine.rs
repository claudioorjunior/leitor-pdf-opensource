use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

use pdfium_render::prelude::*;
use unicode_normalization::UnicodeNormalization;

use crate::page::{
    Bitmap, EngineError, Glyph, MediaBox, Outline, OutlineItem, PageEngine, PageNo, PageSurface,
    Quad, Scale, TextLayer,
};

const PDFIUM_MISSING: &str =
    "Não foi possível carregar a biblioteca Pdfium (.dylib/.dll). Coloque-a na pasta do aplicativo ou instale-a no sistema.";
const WORKER_GONE: &str = "motor PDF encerrado";
const RENDER_TOO_LARGE: &str = "página grande demais para renderizar nesta escala";
const RENDER_INVALID: &str = "dimensão de render inválida";

/// Lado máximo em px. A4 a 8× (teto de zoom da UI) fica em ~4760×6736.
const MAX_RENDER_SIDE: u32 = 16_384;
/// Teto de pixels RGBA (bytes = este valor × 4). 64M ≈ 256 MiB; A4 a 8× ≈ 32M.
const MAX_RENDER_PIXELS: u32 = 64_000_000;

// O documento Pdfium fica aberto numa thread dedicada: bind + parse acontecem
// uma vez por arquivo, e cada operação vira uma ida-e-volta leve pelo canal.
// `PdfDocument` toma emprestado o `Pdfium`, então os dois vivem como locais
// na mesma função da worker — nunca atravessam threads.
#[derive(Clone)]
pub struct PdfiumEngine {
    shared: Arc<Shared>,
}

struct Shared {
    requests: mpsc::Sender<Request>,
    page_count: u32,
}

enum Request {
    PageData {
        page: PageNo,
        reply: mpsc::Sender<Result<(MediaBox, TextLayer), EngineError>>,
    },
    Render {
        page: PageNo,
        scale: Scale,
        /// Quartos de volta horários da vista (0..=3); impressão usa 0.
        rotation: u8,
        reply: mpsc::Sender<Result<PageSurface, EngineError>>,
    },
    /// Lê o outline (bookmarks) do documento. Read-only: não altera o
    /// comportamento interno do Pdfium, apenas percorre a árvore existente.
    Outline {
        reply: mpsc::Sender<Result<Option<Outline>, EngineError>>,
    },
}

impl PdfiumEngine {
    fn bind() -> Result<Pdfium, EngineError> {
        // Bundle .app, depois a lib ao lado do binário. Nunca o cwd:
        // um PDF numa pasta com libpdfium plantada não deve ser carregado.
        for path in pdfium_library_candidates() {
            if let Ok(bindings) = Pdfium::bind_to_library(&path) {
                return Ok(Pdfium::new(bindings));
            }
        }
        Pdfium::bind_to_system_library()
            .map(Pdfium::new)
            .map_err(|_| EngineError(PDFIUM_MISSING.into()))
    }

    fn call<T>(
        &self,
        make: impl FnOnce(mpsc::Sender<Result<T, EngineError>>) -> Request,
    ) -> Result<T, EngineError> {
        let (tx, rx) = mpsc::channel();
        self.shared
            .requests
            .send(make(tx))
            .map_err(|_| EngineError(WORKER_GONE.into()))?;
        rx.recv().map_err(|_| EngineError(WORKER_GONE.into()))?
    }

    // Media + texto numa única ida à worker (antes eram dois reloads).
    pub fn page_data(&self, page: PageNo) -> Result<(MediaBox, TextLayer), EngineError> {
        self.call(|reply| Request::PageData { page, reply })
    }

    /// Lê o outline (bookmarks) do documento. Read-only sobre `PdfDocument`;
    /// não altera o estado interno do Pdfium.
    pub fn outline(&self) -> Result<Option<Outline>, EngineError> {
        self.call(|reply| Request::Outline { reply })
    }
}

impl PageEngine for PdfiumEngine {
    /// Abre um documento numa worker thread própria. Só um engine vivo por
    /// vez: um segundo `bind` com outro worker ativo trava (limite do Pdfium,
    /// não deste código) — o app sempre derruba o `Ready` anterior ao abrir.
    fn open(bytes: Arc<[u8]>) -> Result<Self, EngineError> {
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, EngineError>>();
        let owned = bytes.to_vec();
        std::thread::Builder::new()
            .name("tsuro-pdfium".into())
            .spawn(move || {
                let pdfium = match PdfiumEngine::bind() {
                    Ok(pdfium) => pdfium,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                let document = match pdfium.load_pdf_from_byte_vec(owned, None) {
                    Ok(document) => document,
                    Err(e) => {
                        let _ = ready_tx.send(Err(EngineError(format!(
                            "não foi possível abrir o PDF: {e}"
                        ))));
                        return;
                    }
                };
                let count = u32::from(document.pages().len());
                if ready_tx.send(Ok(count)).is_err() {
                    return;
                }
                for request in req_rx {
                    match request {
                        Request::PageData { page, reply } => {
                            let _ = reply.send(page_data_from_doc(&document, page));
                        }
                        Request::Render {
                            page,
                            scale,
                            rotation,
                            reply,
                        } => {
                            let _ = reply.send(render_from_doc(&document, page, scale, rotation));
                        }
                        Request::Outline { reply } => {
                            let _ = reply.send(outline_from_doc(&document));
                        }
                    }
                }
            })
            .map_err(|e| EngineError(format!("não foi possível iniciar o motor PDF: {e}")))?;
        let page_count = ready_rx
            .recv()
            .map_err(|_| EngineError(WORKER_GONE.into()))??;
        Ok(Self {
            shared: Arc::new(Shared {
                requests: req_tx,
                page_count,
            }),
        })
    }

    fn page_count(&self) -> u32 {
        self.shared.page_count
    }

    fn media(&self, page: PageNo) -> Result<MediaBox, EngineError> {
        self.page_data(page).map(|(media, _)| media)
    }

    fn render(&self, page: PageNo, scale: Scale, rotation: u8) -> Result<PageSurface, EngineError> {
        self.call(|reply| Request::Render {
            page,
            scale,
            rotation,
            reply,
        })
    }

    fn text_layer(&self, page: PageNo) -> Result<TextLayer, EngineError> {
        self.page_data(page).map(|(_, text)| text)
    }
}

fn page_data_from_doc(
    document: &PdfDocument<'_>,
    page: PageNo,
) -> Result<(MediaBox, TextLayer), EngineError> {
    let pdf_page = document
        .pages()
        .get(page_index(page)?)
        .map_err(|e| EngineError(e.to_string()))?;
    let media = MediaBox {
        width: pdf_page.width().value,
        height: pdf_page.height().value,
    };
    let text = text_layer_from_page(&pdf_page, page)?;
    Ok((media, text))
}

/// Percorre recursivamente a árvore de bookmarks do Pdfium e produz
/// `Option<Outline>`: `None` quando não há bookmark raiz, `Some` quando há.
/// Read-only sobre o `PdfDocument` — não altera estado interno do Pdfium.
fn outline_from_doc(document: &PdfDocument<'_>) -> Result<Option<Outline>, EngineError> {
    let total = document.pages().len() as u32;
    // `root()` é o primeiro bookmark de topo (não um contêiner): o nível
    // superior é ele mais `iter_siblings()` (que pula o próprio nó).
    let Some(first) = document.bookmarks().root() else {
        return Ok(None);
    };
    Ok(Some(Outline {
        items: std::iter::once(first.clone())
            .chain(first.iter_siblings())
            .map(|child| outline_node(&child, total))
            .collect(),
    }))
}

fn outline_node(bookmark: &PdfBookmark<'_>, total: u32) -> OutlineItem {
    let page = bookmark
        .destination()
        .and_then(|dest| dest.page_index().ok())
        .filter(|&idx| (idx as u32) < total)
        .map(|idx| PageNo::from_index(idx as u32))
        .unwrap_or_else(PageNo::first);
    let title = bookmark
        .title()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(String::new);
    let children = bookmark
        .iter_direct_children()
        .map(|child| outline_node(&child, total))
        .collect();
    OutlineItem {
        title,
        page,
        children,
    }
}

#[derive(Debug, Clone, Copy)]
struct RenderTarget {
    width: i32,
    height: i32,
}

fn finite_positive(value: f32) -> Result<f32, EngineError> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(EngineError(RENDER_INVALID.into()))
    }
}

fn px_from_f32(value: f32) -> Result<u32, EngineError> {
    if !value.is_finite() {
        return Err(EngineError(RENDER_INVALID.into()));
    }
    let rounded = value.round();
    if !rounded.is_finite() {
        return Err(EngineError(RENDER_INVALID.into()));
    }
    if rounded < 0.0 {
        return Err(EngineError(RENDER_INVALID.into()));
    }
    if rounded < 1.0 {
        return Ok(1);
    }
    if rounded > MAX_RENDER_SIDE as f32 {
        return Err(EngineError(RENDER_TOO_LARGE.into()));
    }
    // Já limitado a [1, 16384]; f32 representa estes inteiros com exatidão.
    #[allow(clippy::cast_possible_truncation)]
    let px = rounded as u16;
    Ok(u32::from(px))
}

fn effective_scale(target_px: u32, page_pt: f32) -> Result<f32, EngineError> {
    let scale = target_px as f32 / page_pt;
    if scale.is_finite() && scale > 0.0 {
        Ok(scale)
    } else {
        Err(EngineError(RENDER_INVALID.into()))
    }
}

/// Largura e altura em px para o render, *antes* de chamar o Pdfium.
/// Recusa NaN/inf/≤0, lado acima do teto, bitmap RGBA que não cabe, ou
/// escala efetiva `px / pts` não-finita (página subnormal: 1×1 passaria no
/// teto, mas o pdfium-render faz `target / source` em f32 → inf).
fn render_target_px(page_w: f32, page_h: f32, factor: f32) -> Result<RenderTarget, EngineError> {
    let page_w = finite_positive(page_w)?;
    let page_h = finite_positive(page_h)?;
    let factor = finite_positive(factor)?;

    let width_f = page_w * factor;
    let height_f = page_h * factor;
    if !width_f.is_finite() || !height_f.is_finite() {
        return Err(EngineError(RENDER_INVALID.into()));
    }

    let width = px_from_f32(width_f)?;
    let height = px_from_f32(height_f)?;
    if width > MAX_RENDER_SIDE || height > MAX_RENDER_SIDE {
        return Err(EngineError(RENDER_TOO_LARGE.into()));
    }

    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| EngineError(RENDER_TOO_LARGE.into()))?;
    if pixels > MAX_RENDER_PIXELS {
        return Err(EngineError(RENDER_TOO_LARGE.into()));
    }

    let bytes = u64::from(pixels)
        .checked_mul(4)
        .ok_or_else(|| EngineError(RENDER_TOO_LARGE.into()))?;
    let _: usize = usize::try_from(bytes).map_err(|_| EngineError(RENDER_TOO_LARGE.into()))?;

    // pdfium-render `apply_to_page` faz `(target as f32) / source` e, se inf,
    // aloca i32::MAX². Recusar *antes* de pedir o bitmap.
    let _ = effective_scale(width, page_w)?;
    let _ = effective_scale(height, page_h)?;

    Ok(RenderTarget {
        width: i32::try_from(width).map_err(|_| EngineError(RENDER_TOO_LARGE.into()))?,
        height: i32::try_from(height).map_err(|_| EngineError(RENDER_TOO_LARGE.into()))?,
    })
}

fn render_from_doc(
    document: &PdfDocument<'_>,
    page: PageNo,
    scale: Scale,
    rotation: u8,
) -> Result<PageSurface, EngineError> {
    let pdf_page = document
        .pages()
        .get(page_index(page)?)
        .map_err(|e| EngineError(e.to_string()))?;
    // Vista girada 90°/270° troca largura ↔ altura antes do alvo em px.
    let swap = rotation & 1 == 1;
    let (page_w, page_h) = if swap {
        (pdf_page.height().value, pdf_page.width().value)
    } else {
        (pdf_page.width().value, pdf_page.height().value)
    };
    let target = render_target_px(page_w, page_h, scale.factor())?;
    // Tamanho fixo: o pdfium-render não divide por MediaBox (evita inf em
    // página subnormal mesmo se o helper falhar em silêncio).
    let config = PdfRenderConfig::new()
        .set_fixed_size(target.width, target.height)
        .rotate(rotation_for(rotation), false);
    let bitmap = pdf_page
        .render_with_config(&config)
        .map_err(|e| EngineError(e.to_string()))?;
    let width = bitmap.width() as u32;
    let height = bitmap.height() as u32;
    let rgba = bitmap.as_rgba_bytes();
    Ok(PageSurface {
        bitmap: Bitmap {
            width,
            height,
            rgba,
        },
        scale,
    })
}

fn page_index(page: PageNo) -> Result<u16, EngineError> {
    u16::try_from(page.index()).map_err(|_| EngineError("página fora do intervalo".into()))
}

/// Quartos de volta horários da vista → rotação do Pdfium (também horária).
fn rotation_for(quarter_turns: u8) -> PdfPageRenderRotation {
    match quarter_turns & 3 {
        0 => PdfPageRenderRotation::None,
        1 => PdfPageRenderRotation::Degrees90,
        2 => PdfPageRenderRotation::Degrees180,
        _ => PdfPageRenderRotation::Degrees270,
    }
}

fn pdfium_library_candidates() -> Vec<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| pdfium_candidates_for(&exe))
        .unwrap_or_default()
}

fn pdfium_candidates_for(exe: &Path) -> Vec<PathBuf> {
    let mut out = vec![frameworks_lib_path(exe)];
    if let Some(dir) = exe.parent() {
        out.push(dir.join(Pdfium::pdfium_platform_library_name()));
    }
    out
}

fn frameworks_lib_path(exe: &Path) -> PathBuf {
    let dir = exe.parent().unwrap_or_else(|| Path::new(""));
    dir.join("../Frameworks")
        .join(Pdfium::pdfium_platform_library_name())
}

fn text_layer_from_page(page: &PdfPage<'_>, page_no: PageNo) -> Result<TextLayer, EngineError> {
    let text = match page.text() {
        Ok(text) => text,
        Err(_) => {
            return Ok(TextLayer {
                page: page_no,
                plain: String::new(),
                glyphs: Vec::new(),
            });
        }
    };
    let chars = text.chars();

    let mut plain = String::new();
    let mut glyphs = Vec::new();
    for ch in chars.iter() {
        let cluster: String = ch.unicode_string().unwrap_or_default().nfc().collect();
        if cluster.is_empty() {
            continue;
        }
        let quad = quad_from_char(&ch);
        plain.push_str(&cluster);
        glyphs.push(Glyph { cluster, quad });
    }
    let plain: String = plain.nfc().collect();
    Ok(TextLayer {
        page: page_no,
        plain,
        glyphs,
    })
}

fn quad_from_char(ch: &PdfPageTextChar<'_>) -> Quad {
    match ch.tight_bounds() {
        Ok(rect) => Quad::from_rect(
            rect.left().value,
            rect.bottom().value,
            rect.right().value,
            rect.top().value,
        ),
        Err(_) => Quad::from_rect(0.0, 0.0, 0.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn rotation_for_maps_quarter_turns_clockwise() {
        assert_eq!(rotation_for(0), PdfPageRenderRotation::None);
        assert_eq!(rotation_for(1), PdfPageRenderRotation::Degrees90);
        assert_eq!(rotation_for(2), PdfPageRenderRotation::Degrees180);
        assert_eq!(rotation_for(3), PdfPageRenderRotation::Degrees270);
        assert_eq!(rotation_for(5), PdfPageRenderRotation::Degrees90);
    }

    #[test]
    fn engine_handle_is_send_sync_for_tasks() {
        // A sessão move clones do engine para `Task::perform` + `spawn_blocking`.
        assert_send_sync::<PdfiumEngine>();
    }

    #[test]
    fn frameworks_path_points_at_bundle_lib() {
        let exe = Path::new("/Applications/TsuroPDF.app/Contents/MacOS/TsuroPDF");
        let got = frameworks_lib_path(exe);
        assert_eq!(
            got.parent().and_then(|p| p.file_name()),
            Some(std::ffi::OsStr::new("Frameworks"))
        );
        assert_eq!(
            got.file_name(),
            Some(Pdfium::pdfium_platform_library_name().as_os_str())
        );
    }

    #[test]
    fn pdfium_candidates_stay_next_to_the_binary() {
        let exe = Path::new("/Applications/TsuroPDF.app/Contents/MacOS/TsuroPDF");
        let got = pdfium_candidates_for(exe);
        assert!(got.iter().all(|p| p != Path::new(".") && p.is_absolute()));
        assert!(got.iter().any(|p| p
            .parent()
            .and_then(|d| d.file_name())
            .is_some_and(|n| n == "Frameworks")));
        assert!(got
            .iter()
            .any(|p| p.parent() == Some(Path::new("/Applications/TsuroPDF.app/Contents/MacOS"))));
    }

    #[test]
    fn open_propagates_worker_errors_without_hanging() {
        // Bytes vazios nunca abrem: sem Pdfium, falha no bind; com Pdfium,
        // falha no parse. Em ambos os casos o handshake da worker responde.
        let bytes: Arc<[u8]> = Arc::from(Vec::new());
        let result = PdfiumEngine::open(bytes);
        assert!(result.is_err(), "bytes vazios devem falhar");
    }

    fn assert_too_large(page_w: f32, page_h: f32, factor: f32) {
        let err = render_target_px(page_w, page_h, factor).unwrap_err();
        assert_eq!(err.0, RENDER_TOO_LARGE);
    }

    fn assert_invalid(page_w: f32, page_h: f32, factor: f32) {
        let err = render_target_px(page_w, page_h, factor).unwrap_err();
        assert_eq!(err.0, RENDER_INVALID);
    }

    #[test]
    fn render_target_rejects_narrow_tall_page() {
        // Largura abaixo do teto; altura estoura o lado.
        assert_too_large(10.0, 100_000.0, 1.0);
    }

    #[test]
    fn render_target_rejects_wide_short_page() {
        assert_too_large(100_000.0, 10.0, 1.0);
    }

    #[test]
    fn render_target_rejects_pixel_cap_even_when_sides_fit() {
        // 8001×8000 = 64_008_000 > 64M, ambos os lados < 16384.
        assert_too_large(8_001.0, 8_000.0, 1.0);
    }

    #[test]
    fn render_target_accepts_limits() {
        let side = render_target_px(MAX_RENDER_SIDE as f32, 1.0, 1.0).unwrap();
        assert_eq!(side.width, MAX_RENDER_SIDE as i32);
        assert_eq!(side.height, 1);

        let pixels = render_target_px(8_000.0, 8_000.0, 1.0).unwrap();
        assert_eq!(pixels.width, 8_000);
        assert_eq!(pixels.height, 8_000);

        // A4 a 8×, zoom máximo da UI, tem de caber.
        let a4 = render_target_px(595.0, 842.0, 8.0).unwrap();
        assert_eq!(a4.width, 4_760);
        assert_eq!(a4.height, 6_736);
    }

    #[test]
    fn render_target_rejects_non_finite_and_non_positive() {
        assert_invalid(f32::NAN, 100.0, 1.0);
        assert_invalid(100.0, f32::NAN, 1.0);
        assert_invalid(100.0, 100.0, f32::NAN);
        assert_invalid(f32::INFINITY, 100.0, 1.0);
        assert_invalid(100.0, f32::NEG_INFINITY, 1.0);
        assert_invalid(100.0, 100.0, 0.0);
        assert_invalid(0.0, 100.0, 1.0);
        assert_invalid(100.0, 0.0, 1.0);
        assert_invalid(-10.0, 100.0, 1.0);
        assert_invalid(100.0, -10.0, 1.0);
        assert_invalid(100.0, 100.0, -1.0);
        // Produto explode para inf sem que cada argumento seja inf.
        assert_invalid(1e30, 1.0, 1e10);
    }

    #[test]
    fn render_target_rejects_subnormal_page_that_overflows_scale() {
        // 1×1 passaria no teto de px, mas target/page_w em f32 é inf.
        assert_invalid(1e-40, 1e-40, 1.0);
        assert_invalid(1e-40, 100.0, 1.0);
        assert_invalid(100.0, 1e-40, 1.0);
    }
}
