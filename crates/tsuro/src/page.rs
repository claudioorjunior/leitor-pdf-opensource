use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageNo(u32);

impl PageNo {
    pub fn first() -> Self {
        PageNo(0)
    }

    pub fn from_index(i: u32) -> Self {
        PageNo(i)
    }

    pub fn index(self) -> u32 {
        self.0
    }
}

/// PDF user space, 1 unit = 1 pt.
#[derive(Debug, Clone, Copy)]
pub struct MediaBox {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

/// Discrete render scale so cache keys are exact (thousandths).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Scale(u16);

impl Scale {
    pub fn from_factor(f: f32) -> Self {
        let q = (f * 1000.0).round().clamp(1.0, 65_000.0) as u16;
        Scale(q.max(1))
    }

    pub fn factor(self) -> f32 {
        self.0 as f32 / 1000.0
    }

    pub fn key(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Quad {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub x3: f32,
    pub y3: f32,
}

impl Quad {
    pub fn from_rect(left: f32, bottom: f32, right: f32, top: f32) -> Self {
        Self {
            x0: left,
            y0: bottom,
            x1: right,
            y1: bottom,
            x2: right,
            y2: top,
            x3: left,
            y3: top,
        }
    }

    pub fn contains(self, x: f32, y: f32) -> bool {
        let min_x = self.x0.min(self.x1).min(self.x2).min(self.x3);
        let max_x = self.x0.max(self.x1).max(self.x2).max(self.x3);
        let min_y = self.y0.min(self.y1).min(self.y2).min(self.y3);
        let max_y = self.y0.max(self.y1).max(self.y2).max(self.y3);
        x >= min_x && x <= max_x && y >= min_y && y <= max_y
    }

    pub fn union(self, other: Self) -> Self {
        let left = self
            .x0
            .min(self.x1)
            .min(self.x2)
            .min(self.x3)
            .min(other.x0)
            .min(other.x1)
            .min(other.x2)
            .min(other.x3);
        let right = self
            .x0
            .max(self.x1)
            .max(self.x2)
            .max(self.x3)
            .max(other.x0)
            .max(other.x1)
            .max(other.x2)
            .max(other.x3);
        let bottom = self
            .y0
            .min(self.y1)
            .min(self.y2)
            .min(self.y3)
            .min(other.y0)
            .min(other.y1)
            .min(other.y2)
            .min(other.y3);
        let top = self
            .y0
            .max(self.y1)
            .max(self.y2)
            .max(self.y3)
            .max(other.y0)
            .max(other.y1)
            .max(other.y2)
            .max(other.y3);
        Self::from_rect(left, bottom, right, top)
    }
}

#[derive(Debug, Clone)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Glyph {
    pub cluster: String,
    pub quad: Quad,
}

#[derive(Debug, Clone)]
pub struct TextLayer {
    pub page: PageNo,
    pub plain: String,
    pub glyphs: Vec<Glyph>,
}

impl TextLayer {
    pub fn hit(&self, page_pt: [f32; 2]) -> Option<usize> {
        let [x, y] = page_pt;
        self.glyphs.iter().position(|g| g.quad.contains(x, y))
    }

    pub fn slice(&self, range: crate::session::TextRange) -> String {
        let start = next_char_boundary(&self.plain, range.start.min(self.plain.len()));
        let end = next_char_boundary(&self.plain, range.end.min(self.plain.len()));
        if start >= end {
            String::new()
        } else {
            self.plain[start..end].to_string()
        }
    }
}

fn next_char_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    if s.is_char_boundary(i) {
        i
    } else {
        (i + 1..=s.len())
            .find(|n| s.is_char_boundary(*n))
            .unwrap_or(s.len())
    }
}

#[derive(Debug, Clone)]
pub struct PageSurface {
    pub bitmap: Bitmap,
    pub scale: Scale,
}

pub trait PageEngine: Send + Sync {
    fn open(bytes: Arc<[u8]>) -> Result<Self, EngineError>
    where
        Self: Sized;

    fn page_count(&self) -> u32;

    fn media(&self, page: PageNo) -> Result<MediaBox, EngineError>;

    fn render(&self, page: PageNo, scale: Scale, rotation: u8) -> Result<PageSurface, EngineError>;

    fn text_layer(&self, page: PageNo) -> Result<TextLayer, EngineError>;
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct EngineError(pub String);
