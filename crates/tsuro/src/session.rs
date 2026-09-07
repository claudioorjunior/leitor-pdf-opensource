use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use folio_sign::{analyze_pdf, PdfAnalysis};
use iced::clipboard;
use iced::event::{self, Event};
use iced::window;
use iced::Task;

use crate::engine::PdfiumEngine;
use crate::page::{
    EngineError, MediaBox, PageEngine, PageNo, PageSurface, Quad, Scale, TextLayer, Viewport,
};

#[derive(Debug, Clone)]
pub enum OpenSource {
    Path(PathBuf),
    Dropped(PathBuf),
}

impl OpenSource {
    pub fn path(&self) -> &std::path::Path {
        match self {
            OpenSource::Path(p) | OpenSource::Dropped(p) => p,
        }
    }

    pub fn from_dialog() -> Option<Self> {
        rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .pick_file()
            .map(OpenSource::Path)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ZoomFactor(f32);

impl ZoomFactor {
    pub fn new(raw: f32) -> Self {
        ZoomFactor(raw.clamp(0.25, 8.0))
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Zoom {
    Width,
    Page,
    Manual(ZoomFactor),
}

impl Zoom {
    pub fn scale(self, viewport: Viewport, media: MediaBox) -> Scale {
        let width = media.width.max(1.0);
        let height = media.height.max(1.0);
        let vw = viewport.width.max(1.0);
        let vh = viewport.height.max(1.0);
        let factor = match self {
            Zoom::Width => vw / width,
            Zoom::Page => (vw / width).min(vh / height),
            Zoom::Manual(z) => z.get(),
        };
        Scale::from_factor(factor)
    }
}

#[derive(Debug, Clone)]
pub struct Search {
    query: String,
    hits: Vec<Hit>,
}

impl Search {
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    fn derive(query: &str, pages: &[TextLayer]) -> Self {
        let needle = query.to_lowercase();
        if needle.is_empty() {
            return Search {
                query: query.to_string(),
                hits: Vec::new(),
            };
        }
        let mut hits = Vec::new();
        for layer in pages {
            let hay = layer.plain.to_lowercase();
            let mut from = 0;
            while let Some(rel) = hay[from..].find(&needle) {
                let start = from + rel;
                let end = start + needle.len();
                let quad = quads_for_range(layer, start, end);
                hits.push(Hit {
                    page: layer.page,
                    range: TextRange { start, end },
                    quad,
                });
                from = start + needle.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                if from >= hay.len() {
                    break;
                }
            }
        }
        Search {
            query: query.to_string(),
            hits,
        }
    }
}

fn quads_for_range(layer: &TextLayer, start: usize, end: usize) -> Quad {
    let mut acc: Option<Quad> = None;
    let mut cursor = 0usize;
    for glyph in &layer.glyphs {
        let next = cursor + glyph.cluster.len();
        if cursor < end && next > start {
            acc = Some(match acc {
                None => glyph.quad,
                Some(q) => q.union(glyph.quad),
            });
        }
        cursor = next;
        if cursor >= end {
            break;
        }
    }
    acc.unwrap_or(Quad::from_rect(0.0, 0.0, 0.0, 0.0))
}

#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub page: PageNo,
    pub range: TextRange,
    pub quad: Quad,
}

#[derive(Debug, Clone, Copy)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone)]
pub struct Selection {
    pub page: PageNo,
    pub range: TextRange,
}

pub enum Session {
    Empty,
    Loading { source: OpenSource },
    Ready(Ready),
    Failed { source: OpenSource, message: String },
}

#[derive(Clone)]
pub struct Ready {
    pub source: OpenSource,
    bytes: Arc<[u8]>,
    engine: PdfiumEngine,
    pages: PageCatalog,
    pub signatures: PdfAnalysis,
    pub zoom: Zoom,
    pub visible: PageNo,
    pub search: Search,
    pub selection: Option<Selection>,
    surfaces: SurfaceCache,
    viewport: Viewport,
}

struct PageCatalog {
    media: Vec<MediaBox>,
    text: Vec<TextLayer>,
}

impl Clone for PageCatalog {
    fn clone(&self) -> Self {
        Self {
            media: self.media.clone(),
            text: self.text.clone(),
        }
    }
}

#[derive(Clone, Default)]
struct SurfaceCache {
    entries: HashMap<(u32, u16), PageSurface>,
}

impl SurfaceCache {
    fn get(&self, page: PageNo, scale: Scale) -> Option<&PageSurface> {
        self.entries.get(&(page.index(), scale.key()))
    }

    fn insert(&mut self, page: PageNo, scale: Scale, surface: PageSurface) {
        self.entries.insert((page.index(), scale.key()), surface);
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    PickFile,
    FileDropped(PathBuf),
    Opened(Result<Ready, OpenError>),
    Close,
    SetPage(PageNo),
    SetZoom(Zoom),
    SetViewport(Viewport),
    SearchChanged(String),
    PointerDown { page: PageNo, page_pt: [f32; 2] },
    PointerMove { page: PageNo, page_pt: [f32; 2] },
    PointerUp,
    CopySelection,
    Rendered {
        page: PageNo,
        scale: Scale,
        surface: PageSurface,
    },
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum OpenError {
    #[error("não foi possível ler o arquivo: {0}")]
    Io(String),
    #[error("não foi possível abrir o PDF: {0}")]
    Engine(String),
    #[error("assinaturas: {0}")]
    Sign(String),
}

impl Session {
    pub fn empty() -> Self {
        Session::Empty
    }

    pub fn open_path(path: PathBuf) -> Self {
        Session::Loading {
            source: OpenSource::Path(path),
        }
    }

    pub fn boot(self) -> (Self, Task<Message>) {
        match &self {
            Session::Loading { source } => {
                let source = source.clone();
                (self, Task::perform(open_ready(source), Message::Opened))
            }
            _ => (self, Task::none()),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::PickFile => match OpenSource::from_dialog() {
                None => Task::none(),
                Some(source) => self.begin_open(source),
            },
            Message::FileDropped(path) => self.begin_open(OpenSource::Dropped(path)),
            Message::Opened(result) => {
                self.finish_open(result);
                self.ensure_surface()
            }
            Message::Close => {
                *self = Session::Empty;
                Task::none()
            }
            Message::SetPage(page) => {
                if let Session::Ready(ready) = self {
                    if (page.index() as usize) < ready.pages.media.len() {
                        ready.visible = page;
                    }
                }
                self.ensure_surface()
            }
            Message::SetZoom(zoom) => {
                if let Session::Ready(ready) = self {
                    ready.zoom = zoom;
                }
                self.ensure_surface()
            }
            Message::SetViewport(viewport) => {
                if let Session::Ready(ready) = self {
                    ready.viewport = viewport;
                }
                self.ensure_surface()
            }
            Message::SearchChanged(query) => {
                if let Session::Ready(ready) = self {
                    ready.set_query(query);
                    if let Some(hit) = ready.search.hits.first() {
                        ready.visible = hit.page;
                    }
                }
                self.ensure_surface()
            }
            Message::PointerDown { page, page_pt } => {
                if let Session::Ready(ready) = self {
                    if let Some(layer) = ready.pages.text.get(page.index() as usize) {
                        if let Some(i) = layer.hit(page_pt) {
                            let (start, end) = glyph_byte_range(layer, i);
                            ready.selection = Some(Selection {
                                page,
                                range: TextRange { start, end },
                            });
                        }
                    }
                }
                Task::none()
            }
            Message::PointerMove { page, page_pt } => {
                if let Session::Ready(ready) = self {
                    if let Some(sel) = ready.selection.as_mut() {
                        if sel.page == page {
                            if let Some(layer) = ready.pages.text.get(page.index() as usize) {
                                if let Some(i) = layer.hit(page_pt) {
                                    let (start, end) = glyph_byte_range(layer, i);
                                    sel.range.start = sel.range.start.min(start);
                                    sel.range.end = sel.range.end.max(end);
                                }
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::PointerUp => Task::none(),
            Message::CopySelection => {
                if let Session::Ready(ready) = self {
                    if let Some(text) = ready.selection_plain_text() {
                        return clipboard::write(text);
                    }
                }
                Task::none()
            }
            Message::Rendered {
                page,
                scale,
                surface,
            } => {
                if let Session::Ready(ready) = self {
                    let current = ready.zoom.scale(ready.viewport, ready.media(page));
                    if ready.visible == page && current == scale {
                        ready.surfaces.insert(page, scale, surface);
                    }
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> iced::Element<'_, Message> {
        crate::view::chrome(self)
    }

    pub fn subscription(&self) -> iced::Subscription<Message> {
        event::listen_with(|event, _status, _id| match event {
            Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
            Event::Window(window::Event::Resized(size)) => Some(Message::SetViewport(Viewport {
                width: size.width,
                height: (size.height - 96.0).max(1.0),
            })),
            _ => None,
        })
    }

    pub fn begin_open(&mut self, source: OpenSource) -> Task<Message> {
        *self = Session::Loading {
            source: source.clone(),
        };
        Task::perform(open_ready(source), Message::Opened)
    }

    pub fn finish_open(&mut self, result: Result<Ready, OpenError>) {
        match result {
            Ok(ready) => *self = Session::Ready(ready),
            Err(err) => {
                let source = match self {
                    Session::Loading { source } => source.clone(),
                    Session::Failed { source, .. } => source.clone(),
                    Session::Ready(r) => r.source.clone(),
                    Session::Empty => return,
                };
                *self = Session::Failed {
                    source,
                    message: err.to_string(),
                };
            }
        }
    }

    fn ensure_surface(&self) -> Task<Message> {
        let Session::Ready(ready) = self else {
            return Task::none();
        };
        ready.request_render()
    }
}

impl Ready {
    pub fn page_count(&self) -> u32 {
        self.pages.media.len() as u32
    }

    pub fn media(&self, page: PageNo) -> MediaBox {
        self.pages
            .media
            .get(page.index() as usize)
            .copied()
            .unwrap_or(MediaBox {
                width: 1.0,
                height: 1.0,
            })
    }

    pub fn surface(&self, page: PageNo, scale: Scale) -> Option<&PageSurface> {
        self.surfaces.get(page, scale)
    }

    pub fn visible_surface(&self) -> Option<&PageSurface> {
        let scale = self.zoom.scale(self.viewport, self.media(self.visible));
        self.surface(self.visible, scale)
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn text_layers(&self) -> &[TextLayer] {
        &self.pages.text
    }

    pub fn selection_plain_text(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let layer = self.pages.text.get(sel.page.index() as usize)?;
        let sliced = layer.slice(sel.range);
        if sliced.is_empty() {
            None
        } else {
            Some(sliced)
        }
    }

    pub fn set_query(&mut self, query: String) {
        self.search = Search::derive(&query, &self.pages.text);
    }

    fn request_render(&self) -> Task<Message> {
        let page = self.visible;
        let scale = self.zoom.scale(self.viewport, self.media(page));
        if self.surfaces.get(page, scale).is_some() {
            return Task::none();
        }
        let engine = self.engine.clone();
        Task::perform(
            async move { engine.render(page, scale) },
            move |result| match result {
                Ok(surface) => Message::Rendered {
                    page,
                    scale,
                    surface,
                },
                Err(_) => Message::Rendered {
                    page,
                    scale,
                    surface: empty_surface(page, scale),
                },
            },
        )
    }
}

fn empty_surface(page: PageNo, scale: Scale) -> PageSurface {
    PageSurface {
        bitmap: crate::page::Bitmap {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 0],
        },
        text: TextLayer {
            page,
            plain: String::new(),
            glyphs: Vec::new(),
        },
        scale,
    }
}

pub type Document = Ready;

impl Document {
    fn from_bytes(source: OpenSource, bytes: Arc<[u8]>) -> Result<Ready, OpenError> {
        let engine =
            PdfiumEngine::open(bytes.clone()).map_err(|e| OpenError::Engine(e.to_string()))?;
        let pages = PageCatalog::extract(&engine)?;
        let signatures = analyze_pdf(bytes.as_ref()).map_err(|e| OpenError::Sign(e.to_string()))?;
        Ok(Ready {
            source,
            bytes,
            engine,
            pages,
            signatures,
            zoom: Zoom::Width,
            visible: PageNo::first(),
            search: Search::derive("", &[]),
            selection: None,
            surfaces: SurfaceCache::default(),
            viewport: Viewport {
                width: 960.0,
                height: 720.0,
            },
        })
    }
}

async fn open_ready(source: OpenSource) -> Result<Ready, OpenError> {
    let bytes = std::fs::read(source.path()).map_err(|e| OpenError::Io(e.to_string()))?;
    Document::from_bytes(source, Arc::<[u8]>::from(bytes))
}

impl PageCatalog {
    fn extract(engine: &PdfiumEngine) -> Result<Self, OpenError> {
        let count = engine.page_count();
        let mut media = Vec::with_capacity(count as usize);
        let mut text = Vec::with_capacity(count as usize);
        for i in 0..count {
            let page = PageNo::from_index(i);
            media.push(
                engine
                    .media(page)
                    .map_err(|e: EngineError| OpenError::Engine(e.to_string()))?,
            );
            text.push(
                engine
                    .text_layer(page)
                    .map_err(|e: EngineError| OpenError::Engine(e.to_string()))?,
            );
        }
        Ok(PageCatalog { media, text })
    }
}

fn glyph_byte_range(layer: &TextLayer, index: usize) -> (usize, usize) {
    let mut cursor = 0usize;
    for (i, glyph) in layer.glyphs.iter().enumerate() {
        let next = cursor + glyph.cluster.len();
        if i == index {
            return (cursor, next);
        }
        cursor = next;
    }
    (0, 0)
}

impl std::fmt::Debug for Ready {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ready")
            .field("source", &self.source)
            .field("bytes", &self.bytes.len())
            .field("zoom", &self.zoom)
            .field("visible", &self.visible)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::Glyph;

    #[test]
    fn search_keeps_portuguese_accents() {
        let layer = TextLayer {
            page: PageNo::first(),
            plain: "texto ação extra".into(),
            glyphs: vec![Glyph {
                cluster: "texto ação extra".into(),
                quad: Quad::from_rect(0.0, 0.0, 10.0, 10.0),
            }],
        };
        let hits = Search::derive("ação", &[layer.clone()]);
        assert_eq!(hits.hits().len(), 1);
        let none = Search::derive("acao", &[layer]);
        assert!(none.hits().is_empty());
    }

    #[test]
    fn zoom_width_uses_media_box() {
        let media = MediaBox {
            width: 400.0,
            height: 800.0,
        };
        let viewport = Viewport {
            width: 800.0,
            height: 600.0,
        };
        let scale = Zoom::Width.scale(viewport, media);
        assert!((scale.factor() - 2.0).abs() < 0.002);
    }
}
