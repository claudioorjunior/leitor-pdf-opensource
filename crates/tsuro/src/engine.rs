use std::sync::{mpsc, Arc};

use pdfium_render::prelude::*;
use unicode_normalization::UnicodeNormalization;

use crate::page::{
    Bitmap, EngineError, Glyph, MediaBox, PageEngine, PageNo, PageSurface, Quad, Scale, TextLayer,
};

const PDFIUM_MISSING: &str =
    "Não foi possível carregar a biblioteca Pdfium (.dylib/.dll). Coloque-a na pasta do aplicativo ou instale-a no sistema.";
const WORKER_GONE: &str = "motor PDF encerrado";

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
        reply: mpsc::Sender<Result<PageSurface, EngineError>>,
    },
}

impl PdfiumEngine {
    fn bind() -> Result<Pdfium, EngineError> {
        // Bundle .app carrega a lib de Contents/Frameworks sem depender do cwd.
        let bundled = bundled_library_path().and_then(|path| Pdfium::bind_to_library(path).ok());
        let local =
            || Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(".")).ok();
        let bindings = bundled
            .or_else(local)
            .or_else(|| Pdfium::bind_to_system_library().ok())
            .ok_or_else(|| EngineError(PDFIUM_MISSING.into()))?;
        Ok(Pdfium::new(bindings))
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
}

impl PageEngine for PdfiumEngine {
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
                        Request::Render { page, scale, reply } => {
                            let _ = reply.send(render_from_doc(&document, page, scale));
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

    fn render(&self, page: PageNo, scale: Scale) -> Result<PageSurface, EngineError> {
        self.call(|reply| Request::Render { page, scale, reply })
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

fn render_from_doc(
    document: &PdfDocument<'_>,
    page: PageNo,
    scale: Scale,
) -> Result<PageSurface, EngineError> {
    let pdf_page = document
        .pages()
        .get(page_index(page)?)
        .map_err(|e| EngineError(e.to_string()))?;
    let factor = scale.factor();
    let target_width = (pdf_page.width().value * factor).round().max(1.0) as i32;
    let config = PdfRenderConfig::new().set_target_width(target_width);
    let bitmap = pdf_page
        .render_with_config(&config)
        .map_err(|e| EngineError(e.to_string()))?;
    let width = bitmap.width() as u32;
    let height = bitmap.height() as u32;
    let rgba = rgba_from_bitmap(&bitmap);
    let text = text_layer_from_page(&pdf_page, page)?;
    Ok(PageSurface {
        bitmap: Bitmap {
            width,
            height,
            rgba,
        },
        text,
        scale,
    })
}

fn page_index(page: PageNo) -> Result<u16, EngineError> {
    u16::try_from(page.index()).map_err(|_| EngineError("página fora do intervalo".into()))
}

fn bundled_library_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(frameworks_lib_path(exe.as_path()))
}

fn frameworks_lib_path(exe: &std::path::Path) -> std::path::PathBuf {
    exe.parent()
        .map(|dir| {
            dir.join("../Frameworks")
                .join(Pdfium::pdfium_platform_library_name())
        })
        .unwrap_or_else(|| Pdfium::pdfium_platform_library_name_at_path("."))
}

fn rgba_from_bitmap(bitmap: &PdfBitmap<'_>) -> Vec<u8> {
    bitmap.as_rgba_bytes().to_vec()
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
    fn engine_handle_is_send_sync_for_tasks() {
        // A sessão move clones do engine para `Task::perform` + `spawn_blocking`.
        assert_send_sync::<PdfiumEngine>();
    }

    #[test]
    fn frameworks_path_points_at_bundle_lib() {
        let exe = std::path::Path::new("/Applications/Tsuro.app/Contents/MacOS/tsuro");
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
    fn open_propagates_worker_errors_without_hanging() {
        // Bytes vazios nunca abrem: sem Pdfium, falha no bind; com Pdfium,
        // falha no parse. Em ambos os casos o handshake da worker responde.
        let bytes: Arc<[u8]> = Arc::from(Vec::new());
        let result = PdfiumEngine::open(bytes);
        assert!(result.is_err(), "bytes vazios devem falhar");
    }
}
