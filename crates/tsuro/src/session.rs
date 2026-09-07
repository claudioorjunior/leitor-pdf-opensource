use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use iced::clipboard;
use iced::event::{self, Event};
use iced::window;
use iced::Task;
use tsuro_sign::{analyze_pdf, PdfAnalysis};

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

    fn derive(query: &str, pages: &[Option<TextLayer>]) -> Self {
        let needle = query.to_lowercase();
        if needle.is_empty() {
            return Search {
                query: query.to_string(),
                hits: Vec::new(),
            };
        }
        let mut hits = Vec::new();
        // ponytail: busca cobre paginas carregadas; indexar resto em background se precisar
        for layer in pages.iter().flatten() {
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
    pub signatures_open: bool,
    surfaces: SurfaceCache,
    viewport: Viewport,
}

struct PageCatalog {
    total: u32,
    media: Vec<Option<MediaBox>>,
    text: Vec<Option<TextLayer>>,
}

impl Clone for PageCatalog {
    fn clone(&self) -> Self {
        Self {
            total: self.total,
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
    PageData {
        page: PageNo,
        result: Result<(MediaBox, TextLayer), String>,
    },
    Close,
    SetPage(PageNo),
    SetZoom(Zoom),
    SetViewport(Viewport),
    SearchChanged(String),
    PointerDown {
        page: PageNo,
        page_pt: [f32; 2],
    },
    PointerMove {
        page: PageNo,
        page_pt: [f32; 2],
    },
    PointerUp,
    CopySelection,
    Rendered {
        page: PageNo,
        scale: Scale,
        surface: PageSurface,
    },
    ToggleSignatures,
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
                Task::batch([self.ensure_page_data(), self.ensure_surface()])
            }
            Message::PageData { page, result } => {
                if let Session::Ready(ready) = self {
                    if let Ok((media, text)) = result {
                        let i = page.index() as usize;
                        if i < ready.pages.total as usize {
                            ready.pages.media[i] = Some(media);
                            ready.pages.text[i] = Some(text);
                            let q = ready.search.query().to_string();
                            if !q.is_empty() {
                                ready.set_query(q);
                            }
                        }
                    }
                }
                self.ensure_surface()
            }
            Message::Close => {
                *self = Session::Empty;
                Task::none()
            }
            Message::SetPage(page) => {
                if let Session::Ready(ready) = self {
                    if page.index() < ready.pages.total {
                        ready.visible = page;
                    }
                }
                Task::batch([self.ensure_page_data(), self.ensure_surface()])
            }
            Message::SetZoom(zoom) => {
                if let Session::Ready(ready) = self {
                    ready.zoom = zoom;
                }
                Task::batch([self.ensure_page_data(), self.ensure_surface()])
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
                Task::batch([self.ensure_page_data(), self.ensure_surface()])
            }
            Message::PointerDown { page, page_pt } => {
                if let Session::Ready(ready) = self {
                    if let Some(Some(layer)) = ready.pages.text.get(page.index() as usize) {
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
                            if let Some(Some(layer)) = ready.pages.text.get(page.index() as usize) {
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
            Message::ToggleSignatures => {
                if let Session::Ready(ready) = self {
                    ready.signatures_open = !ready.signatures_open;
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
            Ok(mut ready) => {
                ready.signatures_open = false;
                *self = Session::Ready(ready);
            }
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

    fn ensure_page_data(&self) -> Task<Message> {
        let Session::Ready(ready) = self else {
            return Task::none();
        };
        let page = ready.visible;
        let i = page.index() as usize;
        let loaded = ready.pages.media.get(i).and_then(|m| *m).is_some()
            && matches!(ready.pages.text.get(i), Some(Some(_)));
        if loaded {
            return Task::none();
        }
        let engine = ready.engine.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    engine.page_data(page).map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |result| Message::PageData { page, result },
        )
    }
}

impl Ready {
    pub fn page_count(&self) -> u32 {
        self.pages.total
    }

    pub fn media(&self, page: PageNo) -> MediaBox {
        self.pages
            .media
            .get(page.index() as usize)
            .and_then(|m| *m)
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

    pub fn text_layers(&self) -> Vec<&TextLayer> {
        self.pages.text.iter().flatten().collect()
    }

    pub fn selection_plain_text(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let layer = self.pages.text.get(sel.page.index() as usize)?.as_ref()?;
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
            async move {
                match tokio::task::spawn_blocking(move || engine.render(page, scale)).await {
                    Ok(Ok(surface)) => (page, scale, surface),
                    _ => (page, scale, empty_surface(page, scale)),
                }
            },
            move |(page, scale, surface)| Message::Rendered {
                page,
                scale,
                surface,
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
        let pages = PageCatalog::extract_first(&engine)?;
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
            signatures_open: false,
            surfaces: SurfaceCache::default(),
            viewport: Viewport {
                width: 960.0,
                height: 720.0,
            },
        })
    }
}

async fn open_ready(source: OpenSource) -> Result<Ready, OpenError> {
    let bytes = tokio::fs::read(source.path())
        .await
        .map_err(|e| OpenError::Io(e.to_string()))?;
    let bytes = Arc::<[u8]>::from(bytes);
    // Parse Pdfium + assinaturas fora do executor async: nada aqui pode
    // bloquear a janela.
    tokio::task::spawn_blocking(move || Document::from_bytes(source, bytes))
        .await
        .map_err(|e| OpenError::Engine(e.to_string()))?
}

impl PageCatalog {
    // ponytail: abre com a primeira pagina; resto por demanda em PageData
    fn extract_first(engine: &PdfiumEngine) -> Result<Self, OpenError> {
        let total = engine.page_count();
        let mut media: Vec<Option<MediaBox>> = vec![None; total as usize];
        let mut text: Vec<Option<TextLayer>> = vec![None; total as usize];
        if total > 0 {
            let (first_media, first_text) = engine
                .page_data(PageNo::first())
                .map_err(|e: EngineError| OpenError::Engine(e.to_string()))?;
            media[0] = Some(first_media);
            text[0] = Some(first_text);
        }
        Ok(PageCatalog { total, media, text })
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
        let hits = Search::derive("ação", &[Some(layer.clone())]);
        assert_eq!(hits.hits().len(), 1);
        let none = Search::derive("acao", &[Some(layer)]);
        assert!(none.hits().is_empty());
    }

    #[test]
    fn lazy_search_skips_unloaded_pages() {
        let layer = TextLayer {
            page: PageNo::first(),
            plain: "texto ação extra".into(),
            glyphs: vec![Glyph {
                cluster: "texto ação extra".into(),
                quad: Quad::from_rect(0.0, 0.0, 10.0, 10.0),
            }],
        };
        let mut pages: Vec<Option<TextLayer>> = vec![None; 200];
        pages[0] = Some(layer);
        let hits = Search::derive("ação", &pages);
        assert_eq!(hits.hits().len(), 1);
        assert_eq!(pages.len(), 200);
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

    fn apply(session: &mut Session, message: Message) {
        let _ = session.update(message);
    }

    fn sample_ready() -> Option<Ready> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../public/samples/guia-folio.pdf");
        let bytes = std::fs::read(&path).ok()?;
        Document::from_bytes(OpenSource::Path(path), Arc::<[u8]>::from(bytes)).ok()
    }

    #[test]
    fn ready_starts_with_signatures_closed() {
        let Some(ready) = sample_ready() else {
            return;
        };
        assert!(!ready.signatures_open);
        let mut session = Session::empty();
        session.finish_open(Ok(ready));
        match &session {
            Session::Ready(ready) => assert!(!ready.signatures_open),
            Session::Empty | Session::Loading { .. } | Session::Failed { .. } => {
                panic!("finish_open should become Ready")
            }
        }
    }

    #[test]
    fn toggle_signatures_flips_only_that_flag() {
        let Some(ready) = sample_ready() else {
            return;
        };
        let visible = ready.visible;
        let page_count = ready.page_count();
        let query = ready.search.query().to_string();
        let sigs = ready.signatures.signatures.len();
        let mut session = Session::Ready(ready);
        apply(&mut session, Message::ToggleSignatures);
        match &session {
            Session::Ready(ready) => {
                assert!(ready.signatures_open);
                assert_eq!(ready.visible, visible);
                assert_eq!(ready.page_count(), page_count);
                assert_eq!(ready.search.query(), query);
                assert_eq!(ready.signatures.signatures.len(), sigs);
                assert!(ready.selection.is_none());
            }
            Session::Empty | Session::Loading { .. } | Session::Failed { .. } => {
                panic!("toggle should stay Ready")
            }
        }
        apply(&mut session, Message::ToggleSignatures);
        match &session {
            Session::Ready(ready) => {
                assert!(!ready.signatures_open);
                assert_eq!(ready.visible, visible);
                assert_eq!(ready.page_count(), page_count);
            }
            Session::Empty | Session::Loading { .. } | Session::Failed { .. } => {
                panic!("toggle should stay Ready")
            }
        }
    }

    #[test]
    fn close_and_reopen_do_not_inherit_signatures_open() {
        let Some(ready) = sample_ready() else {
            return;
        };
        let mut session = Session::Ready(ready);
        apply(&mut session, Message::ToggleSignatures);
        match &session {
            Session::Ready(ready) => assert!(ready.signatures_open),
            Session::Empty | Session::Loading { .. } | Session::Failed { .. } => {
                panic!("expected Ready after toggle")
            }
        }
        apply(&mut session, Message::Close);
        assert!(matches!(session, Session::Empty));
        let Some(mut ready) = sample_ready() else {
            return;
        };
        ready.signatures_open = true;
        session.finish_open(Ok(ready));
        match &session {
            Session::Ready(ready) => assert!(!ready.signatures_open),
            Session::Empty | Session::Loading { .. } | Session::Failed { .. } => {
                panic!("finish_open should become Ready")
            }
        }
    }
}
