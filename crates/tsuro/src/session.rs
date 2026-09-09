use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use iced::clipboard;
use iced::event::{self, Event};
use iced::keyboard::{self, key::Named, Key};
use iced::widget::{image, scrollable};
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
    EngineError, Glyph, MediaBox, PageEngine, PageNo, PageSurface, Quad, Scale, TextLayer, Viewport,
};
use crate::prefs::{read_theme, save_theme};
use crate::print::{present_print_pdf, print_document};

pub(crate) const THUMB_WIDTH: f32 = 120.0;
pub(crate) const THUMB_ROW: f32 = 156.0;
const THUMB_VISIBLE: u32 = 8;
const THUMB_PREFETCH: u32 = 3;

/// RGBA byte budget for neighbor/speculative page surfaces (`visible ± 1`).
/// The mandatory visible-page bitmap at its current scale is excluded.
const NEIGHBOR_CACHE_BUDGET: usize = 64 * 1024 * 1024;

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
        if query.is_empty() {
            return Search {
                query: query.to_string(),
                hits: Vec::new(),
            };
        }
        let mut hits = Vec::new();
        for layer in pages.iter().flatten() {
            hits.extend(find_hits(query, layer));
        }
        hits.sort_by_key(|hit| (hit.page.index(), hit.range.start));
        Search {
            query: query.to_string(),
            hits,
        }
    }

    fn extend_page(&mut self, layer: &TextLayer) {
        if self.query.is_empty() {
            return;
        }
        let page_idx = layer.page.index();
        self.hits.retain(|hit| hit.page != layer.page);
        let mut page_hits = find_hits(&self.query, layer);
        page_hits.sort_by_key(|hit| hit.range.start);
        let pos = self
            .hits
            .partition_point(|hit| (hit.page.index(), hit.range.start) < (page_idx, 0));
        self.hits.splice(pos..pos, page_hits);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LowerChar {
    lower: char,
    byte_start: usize,
    byte_end: usize,
}

fn lower_plain_map(plain: &str) -> Vec<LowerChar> {
    let mut mapped = Vec::new();
    for (byte_start, ch) in plain.char_indices() {
        let byte_end = byte_start + ch.len_utf8();
        for lower in ch.to_lowercase() {
            mapped.push(LowerChar {
                lower,
                byte_start,
                byte_end,
            });
        }
    }
    mapped
}

fn case_insensitive_byte_ranges(plain: &str, needle: &str) -> Vec<(usize, usize)> {
    let needle_chars: Vec<char> = needle.chars().flat_map(|ch| ch.to_lowercase()).collect();
    if needle_chars.is_empty() {
        return Vec::new();
    }
    let mapped = lower_plain_map(plain);
    if mapped.len() < needle_chars.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + needle_chars.len() <= mapped.len() {
        if mapped[i..i + needle_chars.len()]
            .iter()
            .zip(&needle_chars)
            .all(|(entry, &nc)| entry.lower == nc)
        {
            let start = mapped[i].byte_start;
            let end = mapped[i + needle_chars.len() - 1].byte_end;
            out.push((start, end));
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

fn find_hits(query: &str, layer: &TextLayer) -> Vec<Hit> {
    let ranges = case_insensitive_byte_ranges(&layer.plain, query);
    let mut hits = Vec::with_capacity(ranges.len());
    let mut glyph_idx = 0usize;
    let mut byte_cursor = 0usize;
    for (start, end) in ranges {
        let quad =
            quads_for_range_monotonic(&layer.glyphs, start, end, &mut glyph_idx, &mut byte_cursor);
        hits.push(Hit {
            page: layer.page,
            range: TextRange { start, end },
            quad,
        });
    }
    hits
}

fn quads_for_range_monotonic(
    glyphs: &[Glyph],
    start: usize,
    end: usize,
    glyph_idx: &mut usize,
    byte_cursor: &mut usize,
) -> Quad {
    while *glyph_idx < glyphs.len() {
        let next = *byte_cursor + glyphs[*glyph_idx].cluster.len();
        if next > start {
            break;
        }
        *byte_cursor = next;
        *glyph_idx += 1;
    }
    let mut acc: Option<Quad> = None;
    let mut cursor = *byte_cursor;
    for glyph in &glyphs[*glyph_idx..] {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavCmd {
    Previous,
    Next,
    First,
    Last,
    GoTo(PageNo),
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
    engine: PdfiumEngine,
    pages: PageCatalog,
    pub signatures: PdfAnalysis,
    pub zoom: Zoom,
    pub visible: PageNo,
    /// Draft 1-based page number shown in the nav pill.
    page_input: String,
    pub search: Search,
    pub selection: Option<Selection>,
    pub signatures_open: bool,
    pub pages_open: bool,
    /// Tema Kiri — sobrevive a `begin_open`/`finish_open`/`close_document`.
    pub theme: Theme,
    /// Menu ⋯ aberto. Só existe em `Ready`; zera ao trocar de documento.
    pub overflow_open: bool,
    /// Impressão em andamento (⋯ → Imprimir); trava novos `Print` até terminar.
    pub print_busy: bool,
    /// Erro da última impressão — banner no chrome, sem descarregar o documento.
    pub print_error: Option<String>,
    pub pages_scroll_y: f32,
    recents: Vec<PathBuf>,
    /// DPR da janela: bitmap sai em px físicos (zoom CSS × isto).
    pub render_scale: f32,
    open_gen: u64,
    /// Invalidates in-flight renders on nav/zoom/DPI changes.
    render_gen: u64,
    surfaces: SurfaceCache,
    thumbs: ThumbCache,
    render_inflight: HashSet<(u32, u16)>,
    render_inflight_gen: HashMap<(u32, u16), u64>,
    render_inflight_doc: HashMap<(u32, u16), u64>,
    failed: HashSet<(u32, u16)>,
    page_data_inflight: HashSet<u32>,
    page_data_failed: HashSet<u32>,
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

#[derive(Clone)]
pub(crate) struct CachedSurface {
    pub scale: Scale,
    pub image: image::Handle,
    rgba_bytes: usize,
}

impl CachedSurface {
    fn from_page(surface: PageSurface) -> Self {
        let rgba_bytes = surface.bitmap.rgba.len();
        let image = image::Handle::from_rgba(
            surface.bitmap.width,
            surface.bitmap.height,
            surface.bitmap.rgba,
        );
        Self {
            scale: surface.scale,
            image,
            rgba_bytes,
        }
    }

    fn byte_size(&self) -> usize {
        self.rgba_bytes
    }
}

#[derive(Clone, Default)]
struct SurfaceCache {
    pages: HashMap<u32, CachedSurface>,
}

impl SurfaceCache {
    fn get(&self, page: PageNo, scale: Scale) -> Option<&CachedSurface> {
        let cached = self.pages.get(&page.index())?;
        if cached.scale == scale {
            Some(cached)
        } else {
            None
        }
    }

    /// Stale-while-revalidate: devolve o bitmap armazenado enquanto o render da
    /// nova escala não chega (troca de zoom/viewport).
    fn fallback_for_page(&self, page: PageNo) -> Option<&CachedSurface> {
        self.pages.get(&page.index())
    }

    fn insert(&mut self, page: PageNo, _scale: Scale, surface: PageSurface) {
        self.pages
            .insert(page.index(), CachedSurface::from_page(surface));
    }

    fn retain_pages(&mut self, keep: &HashSet<u32>) {
        self.pages.retain(|page, _| keep.contains(page));
    }

    fn neighbor_bytes(&self, visible: u32) -> usize {
        self.pages
            .iter()
            .filter(|(page, _)| **page != visible)
            .map(|(_, surface)| surface.byte_size())
            .sum()
    }

    fn page_bytes(&self, page: u32) -> usize {
        self.pages
            .get(&page)
            .map(CachedSurface::byte_size)
            .unwrap_or(0)
    }

    fn enforce_neighbor_budget(&mut self, visible: u32, budget: usize) {
        while self.neighbor_bytes(visible) > budget {
            let victim = self
                .pages
                .iter()
                .filter(|(page, _)| **page != visible)
                .max_by_key(|(page, _)| page.abs_diff(visible))
                .map(|(page, _)| *page);
            let Some(page) = victim else {
                break;
            };
            self.pages.remove(&page);
        }
    }
}

#[derive(Clone, Default)]
struct ThumbCache {
    entries: HashMap<(u32, u16), CachedSurface>,
}

impl ThumbCache {
    fn get(&self, page: PageNo, scale: Scale) -> Option<&CachedSurface> {
        self.entries.get(&(page.index(), scale.key()))
    }

    fn insert(&mut self, page: PageNo, scale: Scale, surface: PageSurface) {
        let idx = page.index();
        self.entries.retain(|(page, _), _| *page != idx);
        self.entries
            .insert((idx, scale.key()), CachedSurface::from_page(surface));
    }

    fn clear(&mut self) {
        self.entries.clear();
    }

    fn retain_pages(&mut self, keep: &HashSet<u32>) {
        self.entries.retain(|(page, _), _| keep.contains(page));
    }
}

fn estimated_rgba_bytes(media: MediaBox, scale: Scale) -> usize {
    let w = (media.width.max(1.0) * scale.factor()).round().max(1.0) as u64;
    let h = (media.height.max(1.0) * scale.factor()).round().max(1.0) as u64;
    (w * h * 4).min(usize::MAX as u64) as usize
}

fn neighbor_page_set(visible: u32, total: u32) -> HashSet<u32> {
    let mut keep = HashSet::new();
    if total == 0 {
        return keep;
    }
    keep.insert(visible);
    if visible > 0 {
        keep.insert(visible - 1);
    }
    if visible + 1 < total {
        keep.insert(visible + 1);
    }
    keep
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
        doc_gen: u64,
        result: Result<(MediaBox, TextLayer), String>,
    },
    Close,
    Nav(NavCmd),
    PageInput(String),
    PageSubmit,
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
        doc_gen: u64,
        render_gen: u64,
        surface: Option<PageSurface>,
    },
    ToggleSignatures,
    TogglePages,
    ToggleOverflow,
    /// ⋯ → Imprimir: re-renderiza páginas em ~200 DPI e abre a folha nativa.
    Print,
    /// PDF de impressão pronto; `doc_gen` precisa casar com o `open_gen` do documento atual.
    PrintFinished {
        doc_gen: u64,
        result: Result<Vec<u8>, String>,
    },
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
                self.schedule_work()
            }
            Message::PageData {
                page,
                doc_gen,
                result,
            } => {
                if let Session::Ready(ready) = self {
                    if doc_gen != ready.open_gen {
                        return Task::none();
                    }
                    ready.page_data_inflight.remove(&page.index());
                    match result {
                        Ok((media, text)) => {
                            ready.page_data_failed.remove(&page.index());
                            let i = page.index() as usize;
                            if i < ready.pages.total as usize {
                                ready.pages.media[i] = Some(media);
                                if !ready.search.query().is_empty() {
                                    ready.search.extend_page(&text);
                                }
                                ready.pages.text[i] = Some(text);
                            }
                        }
                        Err(_) => {
                            ready.page_data_failed.insert(page.index());
                        }
                    }
                }
                self.schedule_work()
            }
            Message::Close => self.close_document(),
            Message::Nav(cmd) => {
                if let Session::Ready(ready) = self {
                    ready.apply_nav(cmd);
                }
                self.schedule_work()
            }
            Message::PageInput(draft) => {
                if let Session::Ready(ready) = self {
                    ready.page_input = draft;
                }
                Task::none()
            }
            Message::PageSubmit => {
                if let Session::Ready(ready) = self {
                    ready.submit_page_input();
                }
                self.schedule_work()
            }
            Message::SetZoom(zoom) => {
                if let Session::Ready(ready) = self {
                    ready.zoom = zoom;
                    ready.overflow_open = false;
                    ready.bump_render_gen();
                }
                self.schedule_work()
            }
            Message::SetViewport(viewport) => {
                if let Session::Ready(ready) = self {
                    ready.viewport = viewport;
                    ready.bump_render_gen();
                }
                self.schedule_work()
            }
            Message::WindowMetrics { width, height, id } => {
                if let Session::Ready(ready) = self {
                    ready.viewport = Viewport { width, height };
                    ready.bump_render_gen();
                }
                Task::batch([self.schedule_work(), query_window_scale(id)])
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
                if let Session::Ready(ready) = self {
                    ready.bump_render_gen();
                }
                self.schedule_work()
            }
            Message::SearchChanged(query) => {
                if let Session::Ready(ready) = self {
                    ready.set_query(query);
                    if let Some(hit) = ready.search.hits.first() {
                        ready.navigate_to(hit.page);
                    }
                }
                self.schedule_work()
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
                doc_gen,
                render_gen,
                surface,
            } => {
                if let Session::Ready(ready) = self {
                    if doc_gen != ready.open_gen {
                        return Task::none();
                    }
                    let key = render_key(page, scale);
                    ready.render_inflight.remove(&key);
                    ready.render_inflight_gen.remove(&key);
                    ready.render_inflight_doc.remove(&key);
                    if render_gen != ready.render_gen {
                        ready.evict_unused();
                        return self.schedule_work();
                    }
                    match surface {
                        Some(surface) => {
                            ready.failed.remove(&key);
                            let current = ready.page_scale(page);
                            if ready.visible == page && current == scale {
                                ready.surfaces.insert(page, scale, surface);
                            } else if ready.prefetch_target() == Some((page, scale)) {
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
                self.schedule_work()
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
                let mut tasks = vec![self.schedule_work()];
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
            Message::Print => {
                let Session::Ready(ready) = self else {
                    return Task::none();
                };
                if ready.print_busy {
                    return Task::none();
                }
                ready.overflow_open = false;
                ready.print_error = None;
                ready.print_busy = true;
                let engine = ready.engine.clone();
                let doc_gen = ready.open_gen;
                // Raster ~200 DPI fora da UI. A folha nativa abre em PrintFinished
                // (thread principal do iced) — AppKit exige isso.
                Task::perform(
                    async move {
                        let printed = tokio::task::spawn_blocking(
                            move || print_document(&engine),
                        )
                        .await;
                        match printed {
                            Ok(Ok(bytes)) => Ok(bytes),
                            Ok(Err(err)) => Err(err.to_string()),
                            Err(join) => Err(join.to_string()),
                        }
                    },
                    move |result| Message::PrintFinished { doc_gen, result },
                )
            }
            Message::PrintFinished { doc_gen, result } => {
                if let Session::Ready(ready) = self {
                    if doc_gen == ready.open_gen {
                        match result {
                            Ok(bytes) => {
                                let title = ready
                                    .source
                                    .path()
                                    .file_stem()
                                    .and_then(|stem| stem.to_str())
                                    .unwrap_or("documento");
                                match present_print_pdf(&bytes, title) {
                                    Ok(()) => ready.print_error = None,
                                    Err(err) => ready.print_error = Some(err.to_string()),
                                }
                            }
                            Err(err) => ready.print_error = Some(err),
                        }
                        ready.print_busy = false;
                    }
                }
                // Impressão não toca o cache de páginas: nada a reagendar.
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
                self.schedule_work()
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
        event::listen_with(|event, status, id| match event {
            Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
            Event::Window(window::Event::Opened { size, .. })
            | Event::Window(window::Event::Resized(size)) => Some(Message::WindowMetrics {
                width: size.width,
                height: (size.height - crate::view::CHROME_HEIGHT).max(1.0),
                id,
            }),
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                keyboard_message(key, modifiers, status)
            }
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
                ready.sync_page_input();
                ready.render_gen = 1;
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

    fn schedule_work(&mut self) -> Task<Message> {
        let Session::Ready(ready) = self else {
            return Task::none();
        };
        if !ready.page_data_inflight.is_empty() || !ready.render_inflight.is_empty() {
            return Task::none();
        }
        ready.evict_unused();
        if let Some(task) = ready.request_visible_render() {
            return task;
        }
        if let Some(page) = ready.next_page_data_target() {
            let doc_gen = ready.open_gen;
            ready.page_data_inflight.insert(page.index());
            return page_data_task(ready.engine.clone(), page, doc_gen);
        }
        if let Some(task) = ready.request_speculative_render() {
            return task;
        }
        ready.request_thumb_render()
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

fn page_data_task(engine: PdfiumEngine, page: PageNo, doc_gen: u64) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || engine.page_data(page).map_err(|e| e.to_string()))
                .await
                .map_err(|e| e.to_string())?
        },
        move |result| Message::PageData {
            page,
            doc_gen,
            result,
        },
    )
}

fn render_task(
    engine: PdfiumEngine,
    page: PageNo,
    scale: Scale,
    doc_gen: u64,
    render_gen: u64,
) -> Task<Message> {
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
            doc_gen,
            render_gen,
            surface,
        },
    )
}

pub(crate) fn keyboard_message(
    key: Key,
    modifiers: keyboard::Modifiers,
    status: event::Status,
) -> Option<Message> {
    if status != event::Status::Ignored {
        return None;
    }
    if modifiers.shift() || modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match key.as_ref() {
        Key::Named(Named::PageUp | Named::ArrowLeft) => Some(Message::Nav(NavCmd::Previous)),
        Key::Named(Named::PageDown | Named::ArrowRight) => Some(Message::Nav(NavCmd::Next)),
        Key::Named(Named::Home) => Some(Message::Nav(NavCmd::First)),
        Key::Named(Named::End) => Some(Message::Nav(NavCmd::Last)),
        _ => None,
    }
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

    pub fn page_input(&self) -> &str {
        &self.page_input
    }

    pub(crate) fn surface(&self, page: PageNo, scale: Scale) -> Option<&CachedSurface> {
        self.surfaces.get(page, scale)
    }

    pub(crate) fn thumb_surface(&self, page: PageNo) -> Option<&CachedSurface> {
        let media = self.loaded_media(page)?;
        self.thumbs.get(page, self.thumb_scale_for(media))
    }

    pub(crate) fn visible_surface(&self) -> Option<&CachedSurface> {
        let scale = self.page_scale(self.visible);
        self.surface(self.visible, scale)
            .or_else(|| self.surfaces.fallback_for_page(self.visible))
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

    fn sync_page_input(&mut self) {
        if self.pages.total == 0 {
            self.page_input.clear();
            return;
        }
        self.page_input = (self.visible.index() + 1).to_string();
    }

    fn bump_render_gen(&mut self) {
        self.render_gen = self.render_gen.wrapping_add(1);
        if self.render_gen == 0 {
            self.render_gen = 1;
        }
    }

    fn navigate_to(&mut self, page: PageNo) {
        if self.pages.total == 0 {
            self.sync_page_input();
            return;
        }
        let max = self.pages.total - 1;
        let idx = page.index().min(max);
        let target = PageNo::from_index(idx);
        if target != self.visible {
            self.visible = target;
            self.bump_render_gen();
        }
        self.sync_page_input();
    }

    fn apply_nav(&mut self, cmd: NavCmd) {
        match cmd {
            NavCmd::Previous => {
                let idx = self.visible.index().saturating_sub(1);
                self.navigate_to(PageNo::from_index(idx));
            }
            NavCmd::Next => {
                if self.pages.total == 0 {
                    self.sync_page_input();
                    return;
                }
                let max = self.pages.total - 1;
                let idx = (self.visible.index() + 1).min(max);
                self.navigate_to(PageNo::from_index(idx));
            }
            NavCmd::First => self.navigate_to(PageNo::first()),
            NavCmd::Last => {
                if self.pages.total == 0 {
                    self.sync_page_input();
                    return;
                }
                self.navigate_to(PageNo::from_index(self.pages.total - 1));
            }
            NavCmd::GoTo(page) => self.navigate_to(page),
        }
    }

    fn submit_page_input(&mut self) {
        let draft = self.page_input.trim();
        if draft.is_empty() {
            self.sync_page_input();
            return;
        }
        match draft.parse::<u32>() {
            Ok(0) => {
                if self.pages.total == 0 {
                    self.sync_page_input();
                    return;
                }
                self.navigate_to(PageNo::first());
            }
            Ok(one_based) if one_based >= 1 => {
                if self.pages.total == 0 {
                    self.sync_page_input();
                    return;
                }
                let idx = (one_based - 1).min(self.pages.total - 1);
                self.navigate_to(PageNo::from_index(idx));
            }
            _ => self.sync_page_input(),
        }
    }

    fn next_page_data_target(&self) -> Option<PageNo> {
        self.page_data_priority().into_iter().find(|page| {
            !self.has_page_data(*page)
                && !self.page_data_inflight.contains(&page.index())
                && !self.page_data_failed.contains(&page.index())
        })
    }

    fn page_data_priority(&self) -> Vec<PageNo> {
        let mut pages = vec![self.visible];
        let v = self.visible.index();
        if v + 1 < self.pages.total {
            pages.push(PageNo::from_index(v + 1));
        }
        if self.pages_open {
            for page in self.thumb_page_window() {
                if !pages.iter().any(|p| p.index() == page.index()) {
                    pages.push(page);
                }
            }
        }
        pages
    }

    fn prefetch_target(&self) -> Option<(PageNo, Scale)> {
        if self.pages.total == 0 {
            return None;
        }
        let next_idx = self.visible.index() + 1;
        if next_idx >= self.pages.total {
            return None;
        }
        let page = PageNo::from_index(next_idx);
        if !self.has_page_data(page) {
            return None;
        }
        Some((page, self.page_scale(page)))
    }

    fn evict_unused(&mut self) {
        let keep = neighbor_page_set(self.visible.index(), self.pages.total);
        self.surfaces.retain_pages(&keep);
        self.surfaces
            .enforce_neighbor_budget(self.visible.index(), NEIGHBOR_CACHE_BUDGET);
        if !self.pages_open {
            self.thumbs.clear();
            return;
        }
        let thumb_keep: HashSet<u32> = self
            .thumb_page_window()
            .into_iter()
            .map(|page| page.index())
            .collect();
        self.thumbs.retain_pages(&thumb_keep);
    }

    fn request_visible_render(&mut self) -> Option<Task<Message>> {
        if !self.render_inflight.is_empty() {
            return None;
        }
        if !self.has_page_data(self.visible) {
            return None;
        }
        let page = self.visible;
        let scale = self.page_scale(page);
        let key = render_key(page, scale);
        if self.surfaces.get(page, scale).is_some() || self.failed.contains(&key) {
            return None;
        }
        self.track_render(key);
        Some(render_task(
            self.engine.clone(),
            page,
            scale,
            self.open_gen,
            self.render_gen,
        ))
    }

    fn request_speculative_render(&mut self) -> Option<Task<Message>> {
        if !self.render_inflight.is_empty() {
            return None;
        }
        if self
            .surface(self.visible, self.page_scale(self.visible))
            .is_none()
            || !self.has_page_data(self.visible)
        {
            return None;
        }
        let Some((page, scale)) = self.prefetch_target() else {
            return None;
        };
        let key = render_key(page, scale);
        if self.surfaces.get(page, scale).is_some() || self.failed.contains(&key) {
            return None;
        }
        if !self.prefetch_fits_budget(page, scale) {
            return None;
        }
        self.track_render(key);
        Some(render_task(
            self.engine.clone(),
            page,
            scale,
            self.open_gen,
            self.render_gen,
        ))
    }

    fn request_thumb_render(&mut self) -> Task<Message> {
        if !self.render_inflight.is_empty() || !self.page_data_inflight.is_empty() {
            return Task::none();
        }
        if !self.pages_open {
            return Task::none();
        }
        let reading_ready = self.has_page_data(self.visible) || self.visible_surface().is_some();
        if !reading_ready {
            return Task::none();
        }
        for page in self.thumb_page_window() {
            let Some(media) = self.loaded_media(page) else {
                continue;
            };
            let scale = self.thumb_scale_for(media);
            let key = render_key(page, scale);
            if self.thumbs.get(page, scale).is_some() || self.failed.contains(&key) {
                continue;
            }
            self.track_render(key);
            return render_task(
                self.engine.clone(),
                page,
                scale,
                self.open_gen,
                self.render_gen,
            );
        }
        Task::none()
    }

    fn track_render(&mut self, key: (u32, u16)) {
        self.render_inflight.insert(key);
        self.render_inflight_gen.insert(key, self.render_gen);
        self.render_inflight_doc.insert(key, self.open_gen);
    }

    fn prefetch_fits_budget(&self, page: PageNo, scale: Scale) -> bool {
        let visible = self.visible.index();
        if page.index() == visible {
            return true;
        }
        let estimate = estimated_rgba_bytes(self.media(page), scale);
        let retained = self.surfaces.page_bytes(page.index());
        let neighbors = self.surfaces.neighbor_bytes(visible);
        neighbors.saturating_sub(retained).saturating_add(estimate) <= NEIGHBOR_CACHE_BUDGET
    }
}

pub type Document = Ready;

impl Document {
    fn from_bytes(source: OpenSource, bytes: Arc<[u8]>) -> Result<Ready, OpenError> {
        let engine =
            PdfiumEngine::open(bytes.clone()).map_err(|e| OpenError::Engine(e.to_string()))?;
        let pages = PageCatalog::extract_first(&engine)?;
        let signatures = analyze_pdf(bytes.as_ref()).map_err(|e| OpenError::Sign(e.to_string()))?;
        let mut ready = Ready {
            source,
            engine,
            pages,
            signatures,
            zoom: Zoom::Width,
            visible: PageNo::first(),
            page_input: String::new(),
            search: Search::derive("", &[]),
            selection: None,
            signatures_open: false,
            pages_open: false,
            pages_scroll_y: 0.0,
            recents: Vec::new(),
            render_scale: 1.0,
            theme: Theme::Dark,
            overflow_open: false,
            print_busy: false,
            print_error: None,
            open_gen: 0,
            render_gen: 1,
            surfaces: SurfaceCache::default(),
            thumbs: ThumbCache::default(),
            render_inflight: HashSet::new(),
            render_inflight_gen: HashMap::new(),
            render_inflight_doc: HashMap::new(),
            failed: HashSet::new(),
            page_data_inflight: HashSet::new(),
            page_data_failed: HashSet::new(),
            viewport: Viewport {
                width: 960.0,
                height: 720.0,
            },
        };
        ready.sync_page_input();
        Ok(ready)
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
    fn zoom_change_keeps_old_bitmap_until_rerender() {
        // Tela não apaga ao trocar o zoom: visible_surface devolve o bitmap
        // da escala antiga até o render da nova chegar.
        let Some(mut ready) = sample_ready() else {
            return;
        };
        let page = ready.visible;
        let old_scale = ready.page_scale(page);
        ready
            .surfaces
            .insert(page, old_scale, fake_surface(page, old_scale));
        assert!(ready.visible_surface().is_some());
        // Nova escala sem render: o cache não tem a chave exata…
        ready.render_scale = 2.0;
        let new_scale = ready.page_scale(page);
        assert_ne!(new_scale, old_scale);
        assert!(ready.surface(page, new_scale).is_none());
        // …mas a tela segue mostrando o bitmap antigo.
        assert!(ready.visible_surface().is_some());
    }

    fn fake_surface(page: PageNo, scale: Scale) -> PageSurface {
        let _ = page;
        PageSurface {
            bitmap: crate::page::Bitmap {
                width: 2,
                height: 2,
                rgba: vec![0; 16],
            },
            scale,
        }
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
    fn close_from_failed_returns_to_empty() {
        isolated(|| {
            let mut session = Session::Empty(EmptyState::default());
            let _ = session.begin_open(OpenSource::Path(PathBuf::from("/tmp/falha.pdf")));
            session.finish_open(Err(OpenError::Engine("motor quebrou".into())));
            assert!(matches!(session, Session::Failed { .. }));
            apply(&mut session, Message::Close);
            assert!(matches!(session, Session::Empty(_)));
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
        apply(
            &mut session,
            Message::Nav(NavCmd::GoTo(PageNo::from_index(1))),
        );
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
        let scale = ready.page_scale(page);
        let doc_gen = ready.open_gen;
        let render_gen = ready.render_gen;
        let mut session = Session::Ready(ready);
        apply(
            &mut session,
            Message::Rendered {
                page,
                scale,
                doc_gen,
                render_gen,
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

    #[test]
    fn page_submit_clamps_and_restores_invalid() {
        let Some(mut ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        let total = ready.page_count();
        ready.page_input = "9999".into();
        ready.submit_page_input();
        assert_eq!(ready.visible.index(), total - 1);
        assert_eq!(ready.page_input(), total.to_string());

        ready.page_input = "abc".into();
        ready.submit_page_input();
        assert_eq!(ready.page_input(), total.to_string());

        ready.page_input = "0".into();
        ready.submit_page_input();
        assert_eq!(ready.visible.index(), 0);
        assert_eq!(ready.page_input(), "1");

        ready.page_input = "  ".into();
        ready.submit_page_input();
        assert_eq!(ready.page_input(), "1");
    }

    #[test]
    fn page_submit_valid_and_external_nav_sync() {
        let Some(mut ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        if ready.page_count() < 3 {
            return;
        }
        ready.page_input = "2".into();
        ready.submit_page_input();
        assert_eq!(ready.visible.index(), 1);
        assert_eq!(ready.page_input(), "2");

        ready.navigate_to(PageNo::from_index(0));
        assert_eq!(ready.page_input(), "1");
    }

    #[test]
    fn keyboard_message_respects_focus_and_modifiers() {
        use iced::event::Status;
        use iced::keyboard::Modifiers;
        match keyboard_message(
            Key::Named(Named::PageDown),
            Modifiers::default(),
            Status::Ignored,
        ) {
            Some(Message::Nav(NavCmd::Next)) => {}
            other => panic!("expected PageDown -> Next, got {other:?}"),
        }
        match keyboard_message(
            Key::Named(Named::ArrowLeft),
            Modifiers::default(),
            Status::Ignored,
        ) {
            Some(Message::Nav(NavCmd::Previous)) => {}
            other => panic!("expected ArrowLeft -> Previous, got {other:?}"),
        }
        match keyboard_message(
            Key::Named(Named::ArrowRight),
            Modifiers::default(),
            Status::Ignored,
        ) {
            Some(Message::Nav(NavCmd::Next)) => {}
            other => panic!("expected ArrowRight -> Next, got {other:?}"),
        }
        assert!(keyboard_message(
            Key::Named(Named::Home),
            Modifiers::default(),
            Status::Captured
        )
        .is_none());
        assert!(
            keyboard_message(Key::Named(Named::End), Modifiers::SHIFT, Status::Ignored).is_none()
        );
    }

    #[test]
    fn stale_render_gen_is_ignored() {
        let Some(ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        let page = ready.visible;
        let scale = ready.page_scale(page);
        let mut session = Session::Ready(ready);
        apply(
            &mut session,
            Message::Rendered {
                page,
                scale,
                doc_gen: 1,
                render_gen: 0,
                surface: Some(fake_surface(page, scale)),
            },
        );
        let Session::Ready(ready) = &session else {
            panic!("expected Ready");
        };
        assert!(ready.surface(page, scale).is_none());
    }

    #[test]
    fn cache_replaces_scale_and_shows_stale_until_exact() {
        let Some(mut ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        let page = ready.visible;
        let old_scale = Scale::from_factor(1.0);
        let new_scale = Scale::from_factor(2.0);
        ready
            .surfaces
            .insert(page, old_scale, fake_surface(page, old_scale));
        assert!(ready.surface(page, old_scale).is_some());
        assert!(ready.visible_surface().is_some());
        ready
            .surfaces
            .insert(page, new_scale, fake_surface(page, new_scale));
        assert!(ready.surface(page, new_scale).is_some());
        assert!(ready.surface(page, old_scale).is_none());
    }

    #[test]
    fn evict_retains_visible_neighbors() {
        let Some(mut ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        if ready.page_count() < 3 {
            return;
        }
        let s0 = Scale::from_factor(1.0);
        ready.visible = PageNo::from_index(1);
        ready.surfaces.insert(
            PageNo::from_index(0),
            s0,
            fake_surface(PageNo::from_index(0), s0),
        );
        ready.surfaces.insert(
            PageNo::from_index(1),
            s0,
            fake_surface(PageNo::from_index(1), s0),
        );
        ready.surfaces.insert(
            PageNo::from_index(2),
            s0,
            fake_surface(PageNo::from_index(2), s0),
        );
        ready.evict_unused();
        assert!(ready.surface(PageNo::from_index(0), s0).is_some());
        assert!(ready.surface(PageNo::from_index(1), s0).is_some());
        assert!(ready.surface(PageNo::from_index(2), s0).is_some());
    }

    #[test]
    fn search_many_unicode_matches_share_glyph_walk() {
        let mut glyphs = Vec::new();
        let mut plain = String::new();
        for _ in 0..50 {
            plain.push_str("ação ");
            glyphs.push(Glyph {
                cluster: "ação ".into(),
                quad: Quad::from_rect(0.0, 0.0, 10.0, 10.0),
            });
        }
        let layer = TextLayer {
            page: PageNo::first(),
            plain,
            glyphs,
        };
        let hits = find_hits("ação", &layer);
        assert_eq!(hits.len(), 50);
    }

    #[test]
    fn stale_doc_gen_render_is_ignored() {
        let Some(ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        let page = ready.visible;
        let scale = ready.page_scale(page);
        let stale_doc = ready.open_gen.wrapping_add(1);
        let render_gen = ready.render_gen;
        let mut session = Session::Ready(ready);
        apply(
            &mut session,
            Message::Rendered {
                page,
                scale,
                doc_gen: stale_doc,
                render_gen,
                surface: Some(fake_surface(page, scale)),
            },
        );
        let Session::Ready(ready) = &session else {
            panic!("expected Ready");
        };
        assert!(ready.surface(page, scale).is_none());
    }

    #[test]
    fn page_data_failure_is_not_retried() {
        let Some(ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        if ready.page_count() < 2 {
            return;
        }
        let page = PageNo::from_index(1);
        let doc_gen = ready.open_gen;
        let mut session = Session::Ready(ready);
        apply(
            &mut session,
            Message::PageData {
                page,
                doc_gen,
                result: Err("boom".into()),
            },
        );
        let Session::Ready(ready) = &session else {
            panic!("expected Ready");
        };
        assert!(ready.page_data_failed.contains(&page.index()));
        assert!(
            ready.next_page_data_target().is_none() || ready.next_page_data_target() != Some(page)
        );
    }

    #[test]
    fn search_overlapping_ranges_keep_glyph_quads() {
        let layer = TextLayer {
            page: PageNo::first(),
            plain: "aa".into(),
            glyphs: vec![
                Glyph {
                    cluster: "a".into(),
                    quad: Quad::from_rect(0.0, 0.0, 1.0, 1.0),
                },
                Glyph {
                    cluster: "a".into(),
                    quad: Quad::from_rect(1.0, 0.0, 2.0, 1.0),
                },
            ],
        };
        let hits = find_hits("aa", &layer);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].quad.x1 > 1.0);
    }

    #[test]
    fn thumb_cache_drops_old_scale() {
        let Some(ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        let page = ready.visible;
        let s1 = Scale::from_factor(1.0);
        let s2 = Scale::from_factor(1.5);
        let mut thumbs = ThumbCache::default();
        thumbs.insert(page, s1, fake_surface(page, s1));
        thumbs.insert(page, s2, fake_surface(page, s2));
        assert!(thumbs.get(page, s1).is_none());
        assert!(thumbs.get(page, s2).is_some());
    }

    #[test]
    fn prefetch_budget_blocks_oversized_neighbor() {
        let Some(mut ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        if ready.page_count() < 2 {
            return;
        }
        ready.visible = PageNo::first();
        let next = PageNo::from_index(1);
        ready.pages.media[1] = Some(MediaBox {
            width: 2000.0,
            height: 2000.0,
        });
        let huge = Scale::from_factor(100.0);
        assert!(!ready.prefetch_fits_budget(next, huge));
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

    #[test]
    fn print_on_empty_is_noop() {
        let mut session = Session::empty();
        apply(&mut session, Message::Print);
        assert!(matches!(session, Session::Empty(_)));
        apply(
            &mut session,
            Message::PrintFinished {
                doc_gen: 0,
                result: Err("falha na impressão".into()),
            },
        );
        assert!(matches!(session, Session::Empty(_)));
    }

    #[test]
    fn print_from_ready_closes_overflow_and_marks_busy() {
        let Some(mut ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        ready.overflow_open = true;
        ready.print_error = Some("erro antigo".into());
        let mut session = Session::Ready(ready);
        apply(&mut session, Message::Print);
        let Session::Ready(r) = &session else {
            panic!("expected Ready, got {session:?}");
        };
        assert!(!r.overflow_open);
        assert!(r.print_busy);
        assert!(r.print_error.is_none());
    }

    #[test]
    fn second_print_while_busy_is_ignored() {
        let Some(ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        let mut session = Session::Ready(ready);
        apply(&mut session, Message::Print);
        let Session::Ready(r) = &session else {
            panic!("expected Ready, got {session:?}");
        };
        assert!(r.print_busy);
        // Usuário reabre o menu enquanto imprime: novo Print não pode mexer em nada.
        apply(&mut session, Message::ToggleOverflow);
        apply(&mut session, Message::Print);
        let Session::Ready(r) = &session else {
            panic!("expected Ready, got {session:?}");
        };
        assert!(r.print_busy);
        assert!(r.overflow_open);
        assert!(r.print_error.is_none());
    }

    #[test]
    fn print_finished_err_keeps_ready_and_reports_error() {
        let Some(mut ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        ready.print_busy = true;
        let doc_gen = ready.open_gen;
        let mut session = Session::Ready(ready);
        apply(
            &mut session,
            Message::PrintFinished {
                doc_gen,
                result: Err("falha na impressão".into()),
            },
        );
        let Session::Ready(r) = &session else {
            panic!("expected Ready, got {session:?}");
        };
        assert!(!r.print_busy);
        assert_eq!(r.print_error.as_deref(), Some("falha na impressão"));
    }

    #[test]
    fn print_finished_ok_clears_error_and_busy() {
        let Some(mut ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        ready.print_busy = true;
        ready.print_error = Some("erro antigo".into());
        let doc_gen = ready.open_gen;
        let mut session = Session::Ready(ready);
        apply(
            &mut session,
            Message::PrintFinished {
                doc_gen,
                result: Ok(b"%PDF-1.4".to_vec()),
            },
        );
        let Session::Ready(r) = &session else {
            panic!("expected Ready, got {session:?}");
        };
        assert!(!r.print_busy);
        assert!(r.print_error.is_none());
    }

    #[test]
    fn print_finished_with_stale_doc_gen_is_ignored() {
        let Some(mut ready) = sample_ready() else {
            panic!("fixture PDF required");
        };
        ready.print_busy = true;
        let stale = ready.open_gen.wrapping_add(1);
        let mut session = Session::Ready(ready);
        apply(
            &mut session,
            Message::PrintFinished {
                doc_gen: stale,
                result: Err("tarde demais".into()),
            },
        );
        let Session::Ready(r) = &session else {
            panic!("expected Ready, got {session:?}");
        };
        assert!(r.print_busy);
        assert!(r.print_error.is_none());
    }
}
