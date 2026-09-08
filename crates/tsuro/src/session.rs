use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use iced::clipboard;
use iced::event::{self, Event};
use iced::widget::scrollable;
use iced::window;
use iced::Task;
use tsuro_sign::{analyze_pdf, PdfAnalysis};

use crate::browse::{
    display_path, drop_recent, is_pdf, list_path, load_recents, merge_recents, parent_of,
    push_recent, read_recents, save_recents, EmptyState, FsEntry,
};
use crate::engine::PdfiumEngine;
use crate::kiri::Theme;
use crate::page::{
    EngineError, MediaBox, PageEngine, PageNo, PageSurface, Quad, Scale, TextLayer, Viewport,
};
use crate::prefs::{read_theme, save_theme};

pub(crate) const THUMB_WIDTH: f32 = 120.0;
pub(crate) const THUMB_ROW: f32 = 156.0;
const THUMB_VISIBLE: u32 = 8;
const THUMB_PREFETCH: u32 = 3;

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
    Empty(EmptyState),
    Loading {
        source: OpenSource,
        recents: Vec<PathBuf>,
        gen: u64,
        theme: Theme,
        /// DPR da janela (1.0 = sem Retina). Via `WindowScale`, como o tema.
        render_scale: f32,
    },
    Ready(Ready),
    Failed {
        source: OpenSource,
        message: String,
        recents: Vec<PathBuf>,
        gen: u64,
        theme: Theme,
        /// DPR da janela (1.0 = sem Retina). Via `WindowScale`, como o tema.
        render_scale: f32,
    },
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
    pub pages_open: bool,
    /// Tema Kiri — sobrevive a `begin_open`/`finish_open`/`close_document`.
    pub theme: Theme,
    /// Menu ⋯ aberto. Só existe em `Ready`; zera ao trocar de documento.
    pub overflow_open: bool,
    pub pages_scroll_y: f32,
    recents: Vec<PathBuf>,
    /// DPR da janela: bitmap sai em px físicos (zoom CSS × isto).
    pub render_scale: f32,
    open_gen: u64,
    surfaces: SurfaceCache,
    thumbs: SurfaceCache,
    inflight: HashSet<(u32, u16)>,
    failed: HashSet<(u32, u16)>,
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

    fn clear(&mut self) {
        self.entries.clear();
    }

    fn retain_pages(&mut self, mut keep: impl FnMut(u32) -> bool) {
        self.entries.retain(|&(page, _), _| keep(page));
    }
}

fn render_key(page: PageNo, scale: Scale) -> (u32, u16) {
    (page.index(), scale.key())
}

#[derive(Debug, Clone)]
pub enum Message {
    PickFile,
    FileDropped(PathBuf),
    Opened {
        gen: u64,
        result: Result<Ready, OpenError>,
    },
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
        surface: Option<PageSurface>,
    },
    ToggleSignatures,
    TogglePages,
    ToggleOverflow,
    SetTheme(Theme),
    /// Janela informou tamanho + identidade: ajusta viewport e reconsulta o DPR.
    WindowMetrics {
        width: f32,
        height: f32,
        id: window::Id,
    },
    /// Densidade da janela (device pixels por px CSS). 1.0 = sem Retina.
    WindowScale(f32),
    PagesScrolled(f32),
    BrowseTo(Option<PathBuf>),
    ListingReady {
        path: Option<PathBuf>,
        result: Result<Vec<FsEntry>, String>,
    },
    OpenRecent(PathBuf),
    RecentsReady(Vec<PathBuf>),
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
        Session::Empty(EmptyState {
            theme: read_theme(),
            render_scale: 1.0,
            ..EmptyState::default()
        })
    }

    pub fn open_path(path: PathBuf) -> Self {
        Session::Loading {
            source: OpenSource::Path(path),
            recents: read_recents(),
            gen: 1,
            theme: read_theme(),
            render_scale: 1.0,
        }
    }

    pub fn boot(self) -> (Self, Task<Message>) {
        match &self {
            Session::Loading { source, gen, .. } => {
                let source = source.clone();
                let gen = *gen;
                (
                    self,
                    Task::perform(open_ready(source), move |result| Message::Opened {
                        gen,
                        result,
                    }),
                )
            }
            Session::Empty(_) => (self, empty_tasks()),
            _ => (self, Task::none()),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::PickFile => match OpenSource::from_dialog() {
                None => Task::none(),
                Some(source) => self.begin_open(source),
            },
            Message::FileDropped(path) => {
                if !is_pdf(&path) {
                    Task::none()
                } else {
                    self.begin_open(OpenSource::Dropped(path))
                }
            }
            Message::OpenRecent(path) => {
                if !is_pdf(&path) {
                    Task::none()
                } else {
                    self.begin_open(OpenSource::Path(path))
                }
            }
            Message::Opened { gen, result } => {
                self.apply_open(gen, result);
                self.ready_followup()
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
                self.ready_followup()
            }
            Message::Close => self.close_document(),
            Message::SetPage(page) => {
                if let Session::Ready(ready) = self {
                    if page.index() < ready.pages.total {
                        ready.visible = page;
                    }
                }
                self.ready_followup()
            }
            Message::SetZoom(zoom) => {
                if let Session::Ready(ready) = self {
                    ready.zoom = zoom;
                    ready.overflow_open = false;
                }
                self.ready_followup()
            }
            Message::SetViewport(viewport) => {
                if let Session::Ready(ready) = self {
                    ready.viewport = viewport;
                }
                self.ensure_surface()
            }
            Message::WindowMetrics { width, height, id } => {
                if let Session::Ready(ready) = self {
                    ready.viewport = Viewport { width, height };
                }
                Task::batch([self.ensure_surface(), query_window_scale(id)])
            }
            Message::WindowScale(scale) => {
                if !scale.is_finite() || scale < 1.0 {
                    return Task::none();
                }
                let scale = scale.min(4.0);
                if (self.render_scale() - scale).abs() < 0.001 {
                    return Task::none();
                }
                self.set_render_scale(scale);
                self.ensure_surface()
            }
            Message::SearchChanged(query) => {
                if let Session::Ready(ready) = self {
                    ready.set_query(query);
                    if let Some(hit) = ready.search.hits.first() {
                        ready.visible = hit.page;
                    }
                }
                self.ready_followup()
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
                    ready.overflow_open = false;
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
                    let key = render_key(page, scale);
                    ready.inflight.remove(&key);
                    match surface {
                        Some(surface) => {
                            ready.failed.remove(&key);
                            let current = ready.page_scale(page);
                            if ready.visible == page && current == scale {
                                ready.surfaces.insert(page, scale, surface);
                            } else if ready.pages_open {
                                if let Some(media) = ready.loaded_media(page) {
                                    if ready.thumb_scale_for(media) == scale
                                        && ready.thumb_page_window().contains(&page)
                                    {
                                        ready.thumbs.insert(page, scale, surface);
                                    }
                                }
                            }
                        }
                        None => {
                            ready.failed.insert(key);
                        }
                    }
                    ready.evict_unused();
                }
                self.ensure_thumbs()
            }
            Message::ToggleSignatures => {
                if let Session::Ready(ready) = self {
                    ready.signatures_open = !ready.signatures_open;
                }
                Task::none()
            }
            Message::TogglePages => {
                let restore = if let Session::Ready(ready) = self {
                    ready.pages_open = !ready.pages_open;
                    if !ready.pages_open {
                        ready.thumbs.clear();
                    }
                    ready.failed.clear();
                    ready.pages_open.then_some(ready.pages_scroll_y)
                } else {
                    None
                };
                let mut tasks = vec![self.ready_followup()];
                if let Some(y) = restore {
                    tasks.push(scrollable::scroll_to(
                        crate::view::pages_scroll_id(),
                        scrollable::AbsoluteOffset { x: 0.0, y },
                    ));
                }
                Task::batch(tasks)
            }
            Message::ToggleOverflow => {
                if let Session::Ready(ready) = self {
                    ready.overflow_open = !ready.overflow_open;
                }
                Task::none()
            }
            Message::SetTheme(theme) => {
                self.set_theme(theme);
                self.close_overflow();
                let _ = save_theme(theme);
                Task::none()
            }
            Message::PagesScrolled(y) => {
                if let Session::Ready(ready) = self {
                    ready.pages_scroll_y = y;
                }
                self.ensure_thumbs()
            }
            Message::BrowseTo(path) => {
                if let Session::Empty(empty) = self {
                    empty.cwd = path.clone();
                    empty.listing_error = None;
                    return Task::perform(list_path(path.clone()), move |result| {
                        Message::ListingReady {
                            path: path.clone(),
                            result,
                        }
                    });
                }
                Task::none()
            }
            Message::ListingReady { path, result } => {
                if let Session::Empty(empty) = self {
                    if empty.cwd == path {
                        match result {
                            Ok(listing) => {
                                empty.listing = listing;
                                empty.listing_error = None;
                            }
                            Err(err) => {
                                empty.listing = Vec::new();
                                empty.listing_error = Some(err);
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::RecentsReady(recents) => {
                match self {
                    Session::Empty(empty) => empty.recents = recents,
                    Session::Loading { recents: slot, .. } => *slot = recents,
                    Session::Ready(_) | Session::Failed { .. } => {}
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> iced::Element<'_, Message> {
        crate::view::chrome(self, self.theme())
    }

    pub fn subscription(&self) -> iced::Subscription<Message> {
        event::listen_with(|event, _status, id| match event {
            Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
            Event::Window(window::Event::Opened { size, .. })
            | Event::Window(window::Event::Resized(size)) => Some(Message::WindowMetrics {
                width: size.width,
                height: (size.height - crate::view::CHROME_HEIGHT).max(1.0),
                id,
            }),
            _ => None,
        })
    }

    pub fn begin_open(&mut self, source: OpenSource) -> Task<Message> {
        let recents = self.recents();
        let theme = self.theme();
        let render_scale = self.render_scale();
        let mut gen = self.open_gen().wrapping_add(1);
        if gen == 0 {
            gen = 1;
        }
        *self = Session::Loading {
            source: source.clone(),
            recents,
            gen,
            theme,
            render_scale,
        };
        Task::perform(open_ready(source), move |result| Message::Opened {
            gen,
            result,
        })
    }

    pub fn finish_open(&mut self, result: Result<Ready, OpenError>) {
        let Some(gen) = self.loading_gen() else {
            return;
        };
        self.apply_open(gen, result);
    }

    fn apply_open(&mut self, gen: u64, result: Result<Ready, OpenError>) {
        match self {
            Session::Loading { gen: current, .. } if *current == gen => {}
            _ => return,
        }
        let recents = merge_recents(self.recents(), read_recents());
        let theme = self.theme();
        let render_scale = self.render_scale();
        match result {
            Ok(mut ready) => {
                let path = ready.source.path().to_path_buf();
                let recents = push_recent(recents, path);
                let _ = save_recents(&recents);
                ready.recents = recents;
                ready.theme = theme;
                ready.render_scale = render_scale;
                ready.open_gen = gen;
                ready.signatures_open = false;
                ready.pages_open = false;
                ready.pages_scroll_y = 0.0;
                ready.overflow_open = false;
                *self = Session::Ready(ready);
            }
            Err(err) => {
                let source = match self {
                    Session::Loading { source, .. } => source.clone(),
                    Session::Failed { source, .. } => source.clone(),
                    Session::Ready(r) => r.source.clone(),
                    Session::Empty(_) => return,
                };
                let recents = match &err {
                    OpenError::Io(_) => drop_recent(recents, source.path()),
                    _ => recents,
                };
                let _ = save_recents(&recents);
                *self = Session::Failed {
                    source,
                    message: err.to_string(),
                    recents,
                    gen,
                    theme,
                    render_scale,
                };
            }
        }
    }

    fn close_document(&mut self) -> Task<Message> {
        let recents = self.recents();
        let theme = self.theme();
        let open_gen = self.open_gen();
        let render_scale = self.render_scale();
        *self = Session::Empty(EmptyState {
            recents,
            theme,
            open_gen,
            render_scale,
            ..EmptyState::default()
        });
        empty_tasks()
    }

    fn recents(&self) -> Vec<PathBuf> {
        match self {
            Session::Empty(empty) => empty.recents.clone(),
            Session::Loading { recents, .. } | Session::Failed { recents, .. } => recents.clone(),
            Session::Ready(ready) => ready.recents.clone(),
        }
    }
    pub fn theme(&self) -> Theme {
        match self {
            Session::Empty(empty) => empty.theme,
            Session::Loading { theme, .. } | Session::Failed { theme, .. } => *theme,
            Session::Ready(ready) => ready.theme,
        }
    }

    fn set_theme(&mut self, theme: Theme) {
        match self {
            Session::Empty(empty) => empty.theme = theme,
            Session::Loading { theme: t, .. } | Session::Failed { theme: t, .. } => *t = theme,
            Session::Ready(ready) => ready.theme = theme,
        }
    }

    fn close_overflow(&mut self) {
        if let Session::Ready(ready) = self {
            ready.overflow_open = false;
        }
    }
    fn render_scale(&self) -> f32 {
        match self {
            Session::Empty(empty) => empty.render_scale,
            Session::Loading { render_scale, .. } | Session::Failed { render_scale, .. } => {
                *render_scale
            }
            Session::Ready(ready) => ready.render_scale,
        }
    }

    fn set_render_scale(&mut self, scale: f32) {
        match self {
            Session::Empty(empty) => empty.render_scale = scale,
            Session::Loading {
                render_scale: s, ..
            }
            | Session::Failed {
                render_scale: s, ..
            } => *s = scale,
            Session::Ready(ready) => ready.render_scale = scale,
        }
    }

    fn open_gen(&self) -> u64 {
        match self {
            Session::Empty(empty) => empty.open_gen,
            Session::Loading { gen, .. } | Session::Failed { gen, .. } => *gen,
            Session::Ready(ready) => ready.open_gen,
        }
    }

    fn loading_gen(&self) -> Option<u64> {
        match self {
            Session::Loading { gen, .. } => Some(*gen),
            _ => None,
        }
    }

    fn ready_followup(&mut self) -> Task<Message> {
        let data = self.ensure_page_data();
        let surface = self.ensure_surface();
        let thumbs = self.ensure_thumbs();
        Task::batch([data, surface, thumbs])
    }

    fn ensure_surface(&mut self) -> Task<Message> {
        let Session::Ready(ready) = self else {
            return Task::none();
        };
        ready.evict_unused();
        ready.request_render()
    }

    fn ensure_page_data(&self) -> Task<Message> {
        let Session::Ready(ready) = self else {
            return Task::none();
        };
        let mut pages = vec![ready.visible];
        if ready.pages_open {
            pages.extend(ready.thumb_page_window());
        }
        pages.sort_by_key(|p| p.index());
        pages.dedup();
        let tasks = pages
            .into_iter()
            .filter(|page| !ready.has_page_data(*page))
            .map(|page| page_data_task(ready.engine.clone(), page));
        Task::batch(tasks)
    }

    fn ensure_thumbs(&mut self) -> Task<Message> {
        let Session::Ready(ready) = self else {
            return Task::none();
        };
        ready.evict_unused();
        if !ready.pages_open {
            return Task::none();
        }
        let reading_ready = ready.has_page_data(ready.visible) || ready.visible_surface().is_some();
        if !reading_ready {
            return Task::none();
        }
        let mut tasks = Vec::new();
        for page in ready.thumb_page_window() {
            let Some(media) = ready.loaded_media(page) else {
                continue;
            };
            let scale = ready.thumb_scale_for(media);
            let key = render_key(page, scale);
            if ready.thumbs.get(page, scale).is_some()
                || ready.inflight.contains(&key)
                || ready.failed.contains(&key)
            {
                continue;
            }
            ready.inflight.insert(key);
            tasks.push(render_task(ready.engine.clone(), page, scale));
        }
        Task::batch(tasks)
    }
}

fn empty_tasks() -> Task<Message> {
    Task::batch([
        Task::perform(load_recents(), Message::RecentsReady),
        Task::perform(list_path(None), |result| Message::ListingReady {
            path: None,
            result,
        }),
    ])
}

fn page_data_task(engine: PdfiumEngine, page: PageNo) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || engine.page_data(page).map_err(|e| e.to_string()))
                .await
                .map_err(|e| e.to_string())?
        },
        move |result| Message::PageData { page, result },
    )
}

fn render_task(engine: PdfiumEngine, page: PageNo, scale: Scale) -> Task<Message> {
    Task::perform(
        async move {
            match tokio::task::spawn_blocking(move || engine.render(page, scale)).await {
                Ok(Ok(surface)) => (page, scale, Some(surface)),
                _ => (page, scale, None),
            }
        },
        move |(page, scale, surface)| Message::Rendered {
            page,
            scale,
            surface,
        },
    )
}
/// Pergunta o DPR da janela ao backend (precisa do `id` do evento).
fn query_window_scale(id: window::Id) -> Task<Message> {
    window::get_scale_factor(id).map(Message::WindowScale)
}

pub fn thumbnail_scale(media: MediaBox) -> Scale {
    Scale::from_factor((THUMB_WIDTH / media.width.max(1.0)).clamp(0.05, 2.0))
}

impl Ready {
    pub fn page_count(&self) -> u32 {
        self.pages.total
    }

    pub fn media(&self, page: PageNo) -> MediaBox {
        self.loaded_media(page).unwrap_or(MediaBox {
            width: 1.0,
            height: 1.0,
        })
    }

    /// Escala de render em px físicos: zoom CSS × DPR da janela.
    /// Guarda <1.0 (campo ainda desconhecido) como 1.0.
    fn page_scale(&self, page: PageNo) -> Scale {
        let css = self.zoom.scale(self.viewport, self.media(page)).factor();
        let dpr = if self.render_scale >= 1.0 {
            self.render_scale
        } else {
            1.0
        };
        Scale::from_factor(css * dpr)
    }

    fn thumb_scale_for(&self, media: MediaBox) -> Scale {
        let dpr = if self.render_scale >= 1.0 {
            self.render_scale
        } else {
            1.0
        };
        Scale::from_factor(thumbnail_scale(media).factor() * dpr)
    }

    fn loaded_media(&self, page: PageNo) -> Option<MediaBox> {
        self.pages.media.get(page.index() as usize).and_then(|m| *m)
    }

    fn has_page_data(&self, page: PageNo) -> bool {
        self.loaded_media(page).is_some()
            && matches!(self.pages.text.get(page.index() as usize), Some(Some(_)))
    }

    pub fn surface(&self, page: PageNo, scale: Scale) -> Option<&PageSurface> {
        self.surfaces.get(page, scale)
    }

    pub fn thumb_surface(&self, page: PageNo) -> Option<&PageSurface> {
        let media = self.loaded_media(page)?;
        self.thumbs.get(page, self.thumb_scale_for(media))
    }

    pub fn visible_surface(&self) -> Option<&PageSurface> {
        let scale = self.page_scale(self.visible);
        self.surface(self.visible, scale)
    }

    pub fn visible_render_failed(&self) -> bool {
        let scale = self.page_scale(self.visible);
        self.failed.contains(&render_key(self.visible, scale))
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn text_layers(&self) -> Vec<&TextLayer> {
        self.pages.text.iter().flatten().collect()
    }

    pub fn recents(&self) -> &[PathBuf] {
        &self.recents
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

    pub(crate) fn thumb_page_window(&self) -> Vec<PageNo> {
        let first = (self.pages_scroll_y / THUMB_ROW).floor().max(0.0) as u32;
        let start = first.saturating_sub(THUMB_PREFETCH);
        let end = (start + THUMB_VISIBLE + THUMB_PREFETCH * 2).min(self.pages.total);
        (start..end).map(PageNo::from_index).collect()
    }

    fn evict_unused(&mut self) {
        let visible = self.visible.index();
        self.surfaces.retain_pages(|page| page == visible);
        if !self.pages_open {
            self.thumbs.clear();
            return;
        }
        let keep: HashSet<u32> = self
            .thumb_page_window()
            .into_iter()
            .map(|page| page.index())
            .collect();
        self.thumbs.retain_pages(|page| keep.contains(&page));
    }

    fn request_render(&mut self) -> Task<Message> {
        let page = self.visible;
        let scale = self.page_scale(page);
        let key = render_key(page, scale);
        if self.surfaces.get(page, scale).is_some()
            || self.inflight.contains(&key)
            || self.failed.contains(&key)
        {
            return Task::none();
        }
        self.inflight.insert(key);
        render_task(self.engine.clone(), page, scale)
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
            pages_open: false,
            pages_scroll_y: 0.0,
            recents: Vec::new(),
            render_scale: 1.0,
            theme: Theme::Dark,
            overflow_open: false,
            open_gen: 0,
            surfaces: SurfaceCache::default(),
            thumbs: SurfaceCache::default(),
            inflight: HashSet::new(),
            failed: HashSet::new(),
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
            .field("signatures_open", &self.signatures_open)
            .field("pages_open", &self.pages_open)
            .finish()
    }
}

impl EmptyState {
    pub fn path_label(&self) -> String {
        display_path(self.cwd.as_deref())
    }

    pub fn parent(&self) -> Option<Option<PathBuf>> {
        match &self.cwd {
            None => None,
            Some(cwd) => Some(parent_of(cwd)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::Glyph;

    fn apply(session: &mut Session, message: Message) {
        let _ = session.update(message);
    }

    fn isolated<R>(f: impl FnOnce() -> R) -> R {
        let path = std::env::temp_dir().join(format!(
            "tsuro-session-recents-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let result = crate::browse::with_recents_path(path.clone(), f);
        let _ = std::fs::remove_file(path);
        result
    }

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

    #[test]
    fn empty_session_is_still_no_document() {
        assert!(matches!(Session::empty(), Session::Empty(_)));
        assert!(matches!(
            Session::open_path(PathBuf::from("/tmp/doc.pdf")),
            Session::Loading { .. }
        ));
    }

    #[test]
    fn listing_error_stays_empty() {
        let mut session = Session::empty();
        apply(
            &mut session,
            Message::ListingReady {
                path: None,
                result: Err("sem permissão".into()),
            },
        );
        match &session {
            Session::Empty(empty) => {
                assert_eq!(empty.listing_error.as_deref(), Some("sem permissão"));
                assert!(empty.listing.is_empty());
            }
            other => panic!("listing error left Empty, got {other:?}"),
        }
    }

    #[test]
    fn browse_into_folder_and_back() {
        let folder = PathBuf::from("/tmp/tsuro-docs");
        let mut session = Session::empty();
        apply(
            &mut session,
            Message::ListingReady {
                path: None,
                result: Ok(vec![FsEntry {
                    path: folder.clone(),
                    name: "docs".into(),
                    is_dir: true,
                }]),
            },
        );
        apply(&mut session, Message::BrowseTo(Some(folder.clone())));
        match &session {
            Session::Empty(empty) => assert_eq!(empty.cwd.as_deref(), Some(folder.as_path())),
            other => panic!("expected Empty after BrowseTo, got {other:?}"),
        }
        apply(
            &mut session,
            Message::ListingReady {
                path: Some(folder.clone()),
                result: Ok(vec![FsEntry {
                    path: folder.join("a.pdf"),
                    name: "a.pdf".into(),
                    is_dir: false,
                }]),
            },
        );
        match &session {
            Session::Empty(empty) => {
                assert_eq!(empty.listing.len(), 1);
                assert!(!empty.listing[0].is_dir);
            }
            other => panic!("expected listing, got {other:?}"),
        }
        apply(&mut session, Message::BrowseTo(None));
        match &session {
            Session::Empty(empty) => assert!(empty.cwd.is_none()),
            other => panic!("expected roots, got {other:?}"),
        }
    }

    #[test]
    fn open_path_skips_browser() {
        let session = Session::open_path(PathBuf::from("/tmp/direct.pdf"));
        assert!(matches!(session, Session::Loading { .. }));
        assert!(!matches!(session, Session::Empty(_)));
    }

    #[test]
    fn remember_recent_on_successful_open_and_close() {
        isolated(|| {
            let pdf = PathBuf::from("/tmp/remembered.pdf");
            let mut session = Session::Empty(EmptyState {
                recents: vec![PathBuf::from("/tmp/older.pdf")],
                ..EmptyState::default()
            });
            let _ = session.begin_open(OpenSource::Path(pdf.clone()));
            session.finish_open(Err(OpenError::Engine("sem motor no teste".into())));
            match &session {
                Session::Failed { recents, .. } => {
                    assert!(recents.contains(&PathBuf::from("/tmp/older.pdf")));
                }
                other => panic!("expected Failed, got {other:?}"),
            }
            let Some(ready) = sample_ready() else {
                return;
            };
            let mut session = Session::Empty(EmptyState {
                recents: vec![PathBuf::from("/tmp/older.pdf")],
                ..EmptyState::default()
            });
            let _ = session.begin_open(ready.source.clone());
            session.finish_open(Ok(ready));
            match &session {
                Session::Ready(ready) => {
                    assert!(!ready.signatures_open);
                    assert!(!ready.pages_open);
                    assert_eq!(
                        ready.recents.first(),
                        Some(&ready.source.path().to_path_buf())
                    );
                    assert!(ready.recents.contains(&PathBuf::from("/tmp/older.pdf")));
                }
                other => panic!("expected Ready, got {other:?}"),
            }
            apply(&mut session, Message::Close);
            match &session {
                Session::Empty(empty) => {
                    assert_eq!(
                        empty.recents.first().and_then(|p| p.file_name()),
                        sample_pdf().file_name()
                    );
                }
                other => panic!("Close should return to Empty, got {other:?}"),
            }
        });
    }

    #[test]
    fn dead_recent_drops_after_io_failure() {
        isolated(|| {
            let missing = PathBuf::from("/tmp/tsuro-missing-recent.pdf");
            let mut session = Session::Empty(EmptyState {
                recents: vec![missing.clone(), PathBuf::from("/tmp/keep.pdf")],
                ..EmptyState::default()
            });
            let _ = session.begin_open(OpenSource::Path(missing.clone()));
            session.finish_open(Err(OpenError::Io("arquivo em falta".into())));
            match &session {
                Session::Failed {
                    recents, message, ..
                } => {
                    assert!(message.contains("arquivo em falta"));
                    assert!(!recents.contains(&missing));
                    assert!(recents.contains(&PathBuf::from("/tmp/keep.pdf")));
                }
                other => panic!("expected Failed, got {other:?}"),
            }
        });
    }

    #[test]
    fn ready_panels_start_closed_and_toggle_independently() {
        let Some(ready) = sample_ready() else {
            return;
        };
        let mut session = Session::Ready(ready);
        match &session {
            Session::Ready(ready) => {
                assert!(!ready.signatures_open);
                assert!(!ready.pages_open);
            }
            _ => unreachable!(),
        }
        apply(&mut session, Message::ToggleSignatures);
        match &session {
            Session::Ready(ready) => {
                assert!(ready.signatures_open);
                assert!(!ready.pages_open);
            }
            _ => unreachable!(),
        }
        apply(&mut session, Message::TogglePages);
        match &session {
            Session::Ready(ready) => {
                assert!(ready.signatures_open);
                assert!(ready.pages_open);
            }
            _ => unreachable!(),
        }
        apply(&mut session, Message::ToggleSignatures);
        match &session {
            Session::Ready(ready) => {
                assert!(!ready.signatures_open);
                assert!(ready.pages_open);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn set_page_from_list_changes_visible() {
        let Some(ready) = sample_ready() else {
            return;
        };
        if ready.page_count() < 2 {
            return;
        }
        let mut session = Session::Ready(ready);
        apply(&mut session, Message::SetPage(PageNo::from_index(1)));
        match &session {
            Session::Ready(ready) => assert_eq!(ready.visible.index(), 1),
            _ => unreachable!(),
        }
    }

    #[test]
    fn close_and_reopen_do_not_inherit_panel_flags() {
        isolated(|| {
            let Some(ready) = sample_ready() else {
                return;
            };
            let path = ready.source.path().to_path_buf();
            let mut session = Session::Ready(ready);
            apply(&mut session, Message::ToggleSignatures);
            apply(&mut session, Message::TogglePages);
            apply(&mut session, Message::Close);
            assert!(matches!(session, Session::Empty(_)));
            let Some(ready) = sample_ready() else {
                return;
            };
            let _ = session.begin_open(OpenSource::Path(path));
            session.finish_open(Ok(ready));
            match &session {
                Session::Ready(ready) => {
                    assert!(!ready.signatures_open);
                    assert!(!ready.pages_open);
                }
                other => panic!("expected Ready, got {other:?}"),
            }
        });
    }

    #[test]
    fn finish_open_merges_disk_recents_when_memory_is_empty() {
        isolated(|| {
            save_recents(&[PathBuf::from("/tmp/disk.pdf")]).unwrap();
            let mut session = Session::Empty(EmptyState::default());
            let _ = session.begin_open(OpenSource::Path(PathBuf::from("/tmp/new.pdf")));
            session.finish_open(Err(OpenError::Engine("sem motor no teste".into())));
            match &session {
                Session::Failed { recents, .. } => {
                    assert!(recents.contains(&PathBuf::from("/tmp/disk.pdf")));
                }
                other => panic!("expected Failed, got {other:?}"),
            }
        });
    }

    #[test]
    fn recents_ready_fills_loading_session() {
        let mut session = Session::Loading {
            source: OpenSource::Path(PathBuf::from("/tmp/direct.pdf")),
            recents: Vec::new(),
            gen: 1,
            theme: Theme::Dark,
            render_scale: 1.0,
        };
        apply(
            &mut session,
            Message::RecentsReady(vec![PathBuf::from("/tmp/old.pdf")]),
        );
        match &session {
            Session::Loading { recents, .. } => {
                assert_eq!(recents, &vec![PathBuf::from("/tmp/old.pdf")]);
            }
            other => panic!("expected Loading, got {other:?}"),
        }
    }

    #[test]
    fn stale_opened_after_close_is_ignored() {
        isolated(|| {
            let mut session = Session::empty();
            let _ = session.begin_open(OpenSource::Path(PathBuf::from("/tmp/a.pdf")));
            let gen = match &session {
                Session::Loading { gen, .. } => *gen,
                other => panic!("expected Loading, got {other:?}"),
            };
            apply(&mut session, Message::Close);
            assert!(matches!(session, Session::Empty(_)));
            apply(
                &mut session,
                Message::Opened {
                    gen,
                    result: Err(OpenError::Engine("atrasado".into())),
                },
            );
            match &session {
                Session::Empty(_) => {}
                other => panic!("stale Opened must not leave Empty, got {other:?}"),
            }
        });
    }

    #[test]
    fn stale_opened_does_not_replace_newer_open() {
        isolated(|| {
            let mut session = Session::empty();
            let _ = session.begin_open(OpenSource::Path(PathBuf::from("/tmp/a.pdf")));
            let gen_a = match &session {
                Session::Loading { gen, .. } => *gen,
                other => panic!("expected Loading, got {other:?}"),
            };
            let _ = session.begin_open(OpenSource::Path(PathBuf::from("/tmp/b.pdf")));
            apply(
                &mut session,
                Message::Opened {
                    gen: gen_a,
                    result: Err(OpenError::Engine("A atrasado".into())),
                },
            );
            match &session {
                Session::Loading { source, .. } => {
                    assert_eq!(source.path(), PathBuf::from("/tmp/b.pdf").as_path());
                }
                other => panic!("A's result replaced B, got {other:?}"),
            }
        });
    }

    #[test]
    fn open_recent_ignores_non_pdf() {
        let mut session = Session::empty();
        apply(
            &mut session,
            Message::OpenRecent(PathBuf::from("/tmp/note.txt")),
        );
        assert!(matches!(session, Session::Empty(_)));
    }

    #[test]
    fn failed_render_is_not_cached() {
        let Some(ready) = sample_ready() else {
            return;
        };
        let page = ready.visible;
        let scale = ready.zoom.scale(ready.viewport, ready.media(page));
        let mut session = Session::Ready(ready);
        apply(
            &mut session,
            Message::Rendered {
                page,
                scale,
                surface: None,
            },
        );
        match &session {
            Session::Ready(ready) => {
                assert!(ready.surface(page, scale).is_none());
                assert!(ready.visible_render_failed());
            }
            other => panic!("expected Ready, got {other:?}"),
        }
        apply(
            &mut session,
            Message::SetViewport(Viewport {
                width: 960.0,
                height: 720.0,
            }),
        );
        match &session {
            Session::Ready(ready) => {
                assert!(ready.surface(page, scale).is_none());
                assert!(ready.visible_render_failed());
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    impl std::fmt::Debug for Session {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Session::Empty(empty) => f.debug_tuple("Empty").field(empty).finish(),
                Session::Loading { source, .. } => {
                    f.debug_struct("Loading").field("source", source).finish()
                }
                Session::Ready(ready) => f.debug_tuple("Ready").field(ready).finish(),
                Session::Failed { message, .. } => {
                    f.debug_struct("Failed").field("message", message).finish()
                }
            }
        }
    }

    fn sample_pdf() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../public/samples/guia-folio.pdf")
    }

    fn sample_ready() -> Option<Ready> {
        let path = sample_pdf();
        let bytes = std::fs::read(&path).ok()?;
        Document::from_bytes(OpenSource::Path(path), Arc::<[u8]>::from(bytes)).ok()
    }

    #[test]
    fn set_theme_updates_state_and_persists() {
        let prefs =
            std::env::temp_dir().join(format!("tsuro-prefs-unit-{}-theme", std::process::id()));
        crate::prefs::with_prefs_path(prefs.clone(), || {
            let _ = std::fs::remove_file(&prefs);
            let mut session = Session::empty();
            assert_eq!(session.theme(), Theme::Dark);
            apply(&mut session, Message::SetTheme(Theme::Light));
            assert_eq!(session.theme(), Theme::Light);
            assert_eq!(crate::prefs::read_theme(), Theme::Light);
            assert_eq!(Session::empty().theme(), Theme::Light);
            apply(&mut session, Message::SetTheme(Theme::Dark));
            assert_eq!(crate::prefs::read_theme(), Theme::Dark);
        });
        let _ = std::fs::remove_file(&prefs);
    }

    #[test]
    fn theme_survives_open_overflow_and_close() {
        let Some(ready) = sample_ready() else {
            return;
        };
        let mut session = Session::Empty(EmptyState {
            theme: Theme::Light,
            ..EmptyState::default()
        });
        let _ = session.begin_open(OpenSource::Path(sample_pdf()));
        let gen = match &session {
            Session::Loading { gen, .. } => *gen,
            other => panic!("expected Loading, got {other:?}"),
        };
        assert_eq!(session.theme(), Theme::Light);
        let _ = session.update(Message::Opened {
            gen,
            result: Ok(ready),
        });
        let Session::Ready(r) = &session else {
            panic!("expected Ready");
        };
        assert_eq!(r.theme, Theme::Light);
        assert!(!r.overflow_open);
        apply(&mut session, Message::ToggleOverflow);
        let Session::Ready(r) = &session else {
            panic!("expected Ready");
        };
        assert!(r.overflow_open);
        apply(&mut session, Message::SetZoom(Zoom::Width));
        let Session::Ready(r) = &session else {
            panic!("expected Ready");
        };
        assert!(!r.overflow_open);
        apply(&mut session, Message::Close);
        match &session {
            Session::Empty(empty) => assert_eq!(empty.theme, Theme::Light),
            other => panic!("expected Empty, got {other:?}"),
        }
        apply(&mut session, Message::ToggleOverflow);
        assert!(matches!(session, Session::Empty(_)));
    }

    #[test]
    fn window_scale_sets_dpr_and_ignores_garbage() {
        let mut session = Session::Empty(EmptyState {
            render_scale: 1.0,
            ..EmptyState::default()
        });
        apply(&mut session, Message::WindowScale(2.0));
        assert_eq!(session.render_scale(), 2.0);
        apply(&mut session, Message::WindowScale(f32::NAN));
        apply(&mut session, Message::WindowScale(0.0));
        assert_eq!(session.render_scale(), 2.0);
        apply(&mut session, Message::WindowScale(2.0));
        assert_eq!(session.render_scale(), 2.0);
    }

    #[test]
    fn page_scale_multiplies_css_zoom_by_dpr() {
        let Some(mut ready) = sample_ready() else {
            return;
        };
        let css = ready.zoom.scale(ready.viewport, ready.media(ready.visible));
        assert_eq!(ready.page_scale(ready.visible), css);
        ready.render_scale = 2.0;
        assert_eq!(
            ready.page_scale(ready.visible),
            Scale::from_factor(css.factor() * 2.0)
        );
        ready.render_scale = 0.0;
        assert_eq!(ready.page_scale(ready.visible), css);
    }

    #[test]
    fn render_scale_survives_open_and_close() {
        let Some(ready) = sample_ready() else {
            return;
        };
        let mut session = Session::Empty(EmptyState {
            render_scale: 2.0,
            ..EmptyState::default()
        });
        let _ = session.begin_open(OpenSource::Path(sample_pdf()));
        assert_eq!(session.render_scale(), 2.0);
        let gen = match &session {
            Session::Loading { gen, .. } => *gen,
            other => panic!("expected Loading, got {other:?}"),
        };
        let _ = session.update(Message::Opened {
            gen,
            result: Ok(ready),
        });
        match &session {
            Session::Ready(r) => assert_eq!(r.render_scale, 2.0),
            other => panic!("expected Ready, got {other:?}"),
        }
        apply(&mut session, Message::Close);
        match &session {
            Session::Empty(empty) => assert_eq!(empty.render_scale, 2.0),
            other => panic!("expected Empty, got {other:?}"),
        }
    }
}
