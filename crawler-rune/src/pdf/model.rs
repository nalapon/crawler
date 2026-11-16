use std::collections::BTreeMap;

use rune::{Live, Pixels};

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }

    pub fn union(self, b: Self) -> Self {
        let x = self.x.min(b.x);
        let y = self.y.min(b.y);
        Self {
            x,
            y,
            w: (self.x + self.w).max(b.x + b.w) - x,
            h: (self.y + self.h).max(b.y + b.h) - y,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize)]
pub enum Tool {
    #[default]
    Select,
    Highlight,
    Underline,
    Note,
}

impl Tool {
    pub const ALL: [Self; 4] = [Self::Select, Self::Highlight, Self::Underline, Self::Note];

    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Highlight => "Highlight",
            Self::Underline => "Underline",
            Self::Note => "Note",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum Ink {
    #[default]
    Yellow,
    Green,
    Blue,
    Pink,
}

impl Ink {
    pub const ALL: [Self; 4] = [Self::Yellow, Self::Green, Self::Blue, Self::Pink];
    pub fn rgb(self) -> u32 {
        match self {
            Self::Yellow => 0xffd11f,
            Self::Green => 0x40c773,
            Self::Blue => 0x4099ff,
            Self::Pink => 0xff669e,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Mark {
    pub id: u64,
    pub rects: Vec<Rect>,
    pub tool: Tool,
    pub color: u32,
    pub note: String,
}

#[derive(Clone, Debug)]
pub enum LinkTarget {
    Url(String),
    Page(usize),
}

#[derive(Clone, Debug)]
pub struct Link {
    pub rect: Rect,
    pub target: LinkTarget,
}

#[derive(Clone, Debug)]
pub struct Character {
    pub index: usize,
    pub rect: Rect,
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub text_ready: bool,
    pub size: [f64; 2],
    pub characters: Vec<Character>,
    pub marks: Vec<Mark>,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Summary {
    pub page_count: usize,
    pub first_size: [f64; 2],
    pub editable: bool,
    pub copying: bool,
    pub dirty: bool,
    pub undo: bool,
    pub redo: bool,
    pub supported_annotations: usize,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Metrics {
    pub first_bitmap_ms: Option<f64>,
    pub first_sharp_ms: Option<f64>,
    pub parse_ms: f64,
    pub max_raster_ms: f64,
    pub raster_ms: f64,
    pub renders: usize,
    pub cache_bytes: usize,
    pub cache_hits: usize,
    pub metadata_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RasterKey {
    pub page: usize,
    pub full_width: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The worker and the scene share these buffers. Replacing a bitmap never commits a view.
#[derive(Clone)]
pub struct Surface {
    pub pixels: Pixels,
    pub sharp: Pixels,
    pub crop: Live<[f32; 4]>,
    pub requested: Option<RasterKey>,
    pub epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            pixels: Pixels::new(),
            sharp: Pixels::new(),
            crop: Live::new([0.0; 4]),
            requested: None,
            epoch: Default::default(),
        }
    }
}

pub struct Source {
    pub summary: Option<Summary>,
    pub saved_path: Option<std::path::PathBuf>,
    pub pages: BTreeMap<usize, std::sync::Arc<Page>>,
    pub surfaces: BTreeMap<usize, Surface>,
    pub overlays: BTreeMap<usize, Vec<Live<[f32; 4]>>>,
    pub drawn: BTreeMap<usize, [f32; 4]>,
    pub metrics: Metrics,
    pub error: Option<String>,
    pub pending: usize,
    pub visible: std::ops::Range<usize>,
    pub started: std::time::Instant,
    pub revision: u64,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            summary: None,
            saved_path: None,
            pages: BTreeMap::new(),
            surfaces: BTreeMap::new(),
            overlays: BTreeMap::new(),
            drawn: BTreeMap::new(),
            metrics: Metrics::default(),
            error: None,
            pending: 0,
            visible: 0..1,
            started: std::time::Instant::now(),
            revision: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPoint {
    pub page: usize,
    pub character: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct TextRange {
    pub start: TextPoint,
    pub end: TextPoint,
}

impl TextRange {
    pub fn ordered(self) -> (TextPoint, TextPoint) {
        if self.start <= self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }
}

pub struct Stroke {
    pub selection: TextRange,
}
