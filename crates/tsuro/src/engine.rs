use std::sync::Arc;

use pdfium_render::prelude::*;
use unicode_normalization::UnicodeNormalization;

use crate::page::{
    Bitmap, EngineError, Glyph, MediaBox, PageEngine, PageNo, PageSurface, Quad, Scale, TextLayer,
};

const PDFIUM_MISSING: &str =
    "Não foi possível carregar a biblioteca Pdfium (.dylib/.dll). Coloque-a na pasta do aplicativo ou instale-a no sistema.";

#[derive(Clone)]
pub struct PdfiumEngine {
    bytes: Arc<[u8]>,
    page_count: u32,
}

impl PdfiumEngine {
    fn bind() -> Result<Pdfium, EngineError> {
        let cwd = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("."));
        let bindings = cwd
            .or_else(|_| Pdfium::bind_to_system_library())
            .map_err(|_| EngineError(PDFIUM_MISSING.into()))?;
        Ok(Pdfium::new(bindings))
    }

    fn with_doc<T>(
        &self,
        f: impl FnOnce(&PdfDocument<'_>) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let pdfium = Self::bind()?;
        let document = pdfium
            .load_pdf_from_byte_vec(self.bytes.to_vec(), None)
            .map_err(|e| EngineError(format!("não foi possível abrir o PDF: {e}")))?;
        f(&document)
    }
}

impl PageEngine for PdfiumEngine {
    fn open(bytes: Arc<[u8]>) -> Result<Self, EngineError> {
        let pdfium = Self::bind()?;
        let document = pdfium
            .load_pdf_from_byte_vec(bytes.to_vec(), None)
            .map_err(|e| EngineError(format!("não foi possível abrir o PDF: {e}")))?;
        let page_count = u32::from(document.pages().len());
        Ok(Self { bytes, page_count })
    }

    fn page_count(&self) -> u32 {
        self.page_count
    }

    fn media(&self, page: PageNo) -> Result<MediaBox, EngineError> {
        self.with_doc(|document| {
            let page = document
                .pages()
                .get(page_index(page)?)
                .map_err(|e| EngineError(e.to_string()))?;
            Ok(MediaBox {
                width: page.width().value,
                height: page.height().value,
            })
        })
    }

    fn render(&self, page: PageNo, scale: Scale) -> Result<PageSurface, EngineError> {
        self.with_doc(|document| {
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
        })
    }

    fn text_layer(&self, page: PageNo) -> Result<TextLayer, EngineError> {
        self.with_doc(|document| {
            let pdf_page = document
                .pages()
                .get(page_index(page)?)
                .map_err(|e| EngineError(e.to_string()))?;
            text_layer_from_page(&pdf_page, page)
        })
    }
}

fn page_index(page: PageNo) -> Result<u16, EngineError> {
    u16::try_from(page.index()).map_err(|_| EngineError("página fora do intervalo".into()))
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
        let cluster: String = ch
            .unicode_string()
            .unwrap_or_default()
            .nfc()
            .collect();
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
