//! All Cocoa PDF objects are created, used and dropped on this thread. No PDFView or PDFKit
//! page rendering is used. Only owned geometry, text metadata and Rune pixel handles cross it.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::time::Instant;

use objc2::{
    AnyThread,
    rc::{Retained, autoreleasepool},
};
use objc2_app_kit::{NSColor, NSColorSpace};
use objc2_core_foundation::{CFRetained, CFURL, CGAffineTransform, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGContext, CGPDFBox, CGPDFDocument, CGPDFPage};
use objc2_foundation::{NSArray, NSCopying, NSPoint, NSString, NSURL, NSValue};
use objc2_pdf_kit::{
    PDFActionGoTo, PDFActionURL, PDFAnnotation, PDFAnnotationKeyName, PDFDocument,
};
use rune::{Live, NativeSurface, Waker};

use super::bitmap::Bitmap;

use super::model::*;

const CACHE_LIMIT: usize = 128 * 1024 * 1024;
const MAX_PIXELS: usize = 16 * 1024 * 1024;

pub enum Edit {
    Add {
        selection: TextRange,
        tool: Tool,
        ink: Ink,
        note: String,
    },
    Delete(u64),
    Note {
        id: u64,
        text: String,
    },
    Color {
        id: u64,
        ink: Ink,
    },
    Undo,
    Redo,
    RemoveAll,
    Discard,
}

pub enum Command {
    Load(PathBuf, Instant, NativeSurface),
    Retain {
        path: PathBuf,
        pages: std::ops::Range<usize>,
    },
    Render {
        path: PathBuf,
        key: RasterKey,
        pixels: NativeSurface,
        epoch: Arc<AtomicU64>,
        generation: u64,
        sharp: bool,
        crop: Option<(Live<[f32; 4]>, [f32; 4])>,
    },
    Edit {
        path: PathBuf,
        edit: Edit,
    },
    Save {
        serial: u64,
        path: PathBuf,
        destination: PathBuf,
    },
    Copy {
        path: PathBuf,
        selection: Option<TextRange>,
    },
    Release(PathBuf),
}

pub enum Event {
    Loaded {
        path: PathBuf,
        summary: Summary,
        metrics: Metrics,
    },
    Page {
        path: PathBuf,
        page: usize,
        content: Page,
        complete: bool,
        summary: Summary,
        metrics: Metrics,
    },
    Edited {
        path: PathBuf,
        summary: Summary,
        pages: BTreeMap<usize, Page>,
    },
    Saved {
        serial: u64,
        path: PathBuf,
        destination: PathBuf,
        summary: Summary,
    },
    Copied(String),
    Error {
        path: PathBuf,
        message: String,
        operation: bool,
    },
}

#[derive(Clone)]
pub struct WorkerSender {
    commands: mpsc::Sender<Work>,
    events: mpsc::Sender<Event>,
    waker: Waker,
}

impl WorkerSender {
    pub fn send(&self, command: Command) -> Result<(), mpsc::SendError<Command>> {
        self.commands
            .send(Work {
                command,
                events: self.events.clone(),
                waker: self.waker.clone(),
            })
            .map_err(|error| mpsc::SendError(error.0.command))
    }
}

struct Work {
    command: Command,
    events: mpsc::Sender<Event>,
    waker: Waker,
}

pub struct Worker {
    pub send: WorkerSender,
    pub receive: mpsc::Receiver<Event>,
}

impl Worker {
    pub fn start(waker: Waker) -> Result<Self, String> {
        // Both windows use one document owner and one 128 MiB bitmap budget.
        static COMMANDS: OnceLock<Result<mpsc::Sender<Work>, String>> = OnceLock::new();
        let commands = COMMANDS
            .get_or_init(|| {
                let (send, commands) = mpsc::channel::<Work>();
                std::thread::Builder::new()
                    .name("crawler-pdf".into())
                    .spawn(move || {
                        let mut documents = HashMap::<PathBuf, Document>::new();
                        let mut owners = HashMap::<PathBuf, usize>::new();
                        while let Ok(work) = commands.recv() {
                            autoreleasepool(|_| {
                                match &work.command {
                                    Command::Load(path, _, _) => {
                                        *owners.entry(path.clone()).or_default() += 1
                                    }
                                    Command::Release(path) => {
                                        if let Some(count) = owners.get_mut(path) {
                                            *count = count.saturating_sub(1);
                                            if *count > 0 {
                                                return;
                                            }
                                        }
                                        owners.remove(path);
                                    }
                                    _ => {}
                                }
                                let result = handle(work.command, &mut documents);
                                while documents
                                    .values()
                                    .map(|d| d.metrics.cache_bytes)
                                    .sum::<usize>()
                                    > CACHE_LIMIT
                                {
                                    let Some(doc) = documents
                                        .values_mut()
                                        .filter(|d| !d.lru.is_empty())
                                        .min_by_key(|d| d.started)
                                    else {
                                        break;
                                    };
                                    if let Some(oldest) = doc.lru.pop_front() {
                                        if let Some(image) = doc.cache.remove(&oldest) {
                                            doc.metrics.cache_bytes -= image.bytes;
                                        }
                                    }
                                }
                                for mut event in result {
                                    match &mut event {
                                        Event::Loaded { path, metrics, .. }
                                        | Event::Page { path, metrics, .. } => {
                                            if let Some(doc) = documents.get(path) {
                                                metrics.cache_bytes = doc.metrics.cache_bytes;
                                            }
                                        }
                                        _ => {}
                                    }
                                    if work.events.send(event).is_err() {
                                        break;
                                    }
                                    (work.waker)();
                                }
                            });
                        }
                    })
                    .map_err(|e| e.to_string())?;
                Ok(send)
            })
            .as_ref()
            .map_err(Clone::clone)?
            .clone();
        let (events, receive) = mpsc::channel();
        Ok(Self {
            send: WorkerSender {
                commands,
                events,
                waker,
            },
            receive,
        })
    }
}

fn handle(command: Command, documents: &mut HashMap<PathBuf, Document>) -> Vec<Event> {
    let (path, operation) = match &command {
        Command::Load(path, _, _) | Command::Release(path) => (path.clone(), false),
        Command::Render { path, .. } => (path.clone(), false),
        Command::Copy { path, .. } | Command::Retain { path, .. } => (path.clone(), false),
        Command::Edit { path, .. } | Command::Save { path, .. } => (path.clone(), true),
    };
    let error_path = path.clone();
    let result = (|| -> Result<Vec<Event>, String> {
        if let Command::Release(_) = command {
            documents.remove(&path);
            return Ok(Vec::new());
        }
        if !documents.contains_key(&path) {
            let mut doc = Document::open(&path)?;
            if let Command::Load(_, requested, _) = &command {
                doc.started = *requested;
            }
            documents.insert(path.clone(), doc);
        }
        let doc = documents
            .get_mut(&path)
            .ok_or("PDF worker lost its document")?;
        match command {
            Command::Load(_, _, poster) => {
                let size = doc.geometry(0)?.size;
                let key = RasterKey {
                    page: 0,
                    full_width: 480,
                    x: 0,
                    y: 0,
                    width: 480,
                    height: (480.0 * size[1] / size[0]).ceil().min(8192.0) as u32,
                };
                let bitmap = doc.render(key)?;
                poster.push(bitmap.pixels, 0)?;
                doc.rendered(false);
                Ok(vec![Event::Loaded {
                    path,
                    summary: doc.summary(),
                    metrics: doc.metrics.clone(),
                }])
            }
            Command::Render {
                key,
                pixels,
                epoch,
                generation,
                sharp,
                crop,
                ..
            } => {
                if epoch.load(Ordering::Relaxed) != generation {
                    return Ok(Vec::new());
                }
                let cached = doc.cache.contains_key(&key);
                let bitmap = doc.render(key)?;
                if epoch.load(Ordering::Relaxed) != generation {
                    if !cached {
                        doc.evict(key);
                    }
                    return Ok(Vec::new());
                }
                // The scene buffer is stable, so bitmap replacement requires no view commit.
                pixels.push(bitmap.pixels, generation)?;
                if let Some((live, rect)) = crop {
                    live.set(rect);
                }
                doc.rendered(sharp);
                let content = if sharp {
                    doc.content(key.page)?
                } else {
                    Page {
                        size: doc.geometry(key.page)?.size,
                        ..Page::default()
                    }
                };
                Ok(vec![Event::Page {
                    path,
                    page: key.page,
                    content,
                    complete: sharp,
                    summary: doc.summary(),
                    metrics: doc.metrics.clone(),
                }])
            }
            Command::Edit { edit, .. } => {
                doc.edit(edit)?;
                let pages = doc.refresh_loaded()?;
                Ok(vec![Event::Edited {
                    path,
                    summary: doc.summary(),
                    pages,
                }])
            }
            Command::Save {
                destination,
                serial,
                ..
            } => {
                doc.save(&destination)?;
                Ok(vec![Event::Saved {
                    serial,
                    path,
                    destination,
                    summary: doc.summary(),
                }])
            }
            Command::Copy { selection, .. } => Ok(vec![Event::Copied(doc.copy(selection)?)]),
            Command::Retain { pages, .. } => {
                doc.loaded.retain(|i, _| pages.contains(i));
                Ok(Vec::new())
            }
            Command::Release(_) => Ok(Vec::new()),
        }
    })();
    result.unwrap_or_else(|message| {
        vec![Event::Error {
            path: error_path,
            message,
            operation,
        }]
    })
}

#[derive(Clone, PartialEq, Eq)]
struct Version {
    dev: u64,
    inode: u64,
    len: u64,
    modified: i64,
    nanos: i64,
}

impl Version {
    fn read(path: &Path) -> Result<Self, String> {
        let m = fs::metadata(path).map_err(|e| e.to_string())?;
        Ok(Self {
            dev: m.dev(),
            inode: m.ino(),
            len: m.len(),
            modified: m.mtime(),
            nanos: m.mtime_nsec(),
        })
    }
}

struct Change {
    page: usize,
    annotation: Retained<PDFAnnotation>,
    added: bool,
}

struct Action(Vec<Change>);

struct Geometry {
    size: [f64; 2],
    transform: CGAffineTransform,
}

impl Geometry {
    fn point(&self, p: CGPoint) -> [f64; 2] {
        let m = self.transform;
        [
            m.a * p.x + m.c * p.y + m.tx,
            self.size[1] - (m.b * p.x + m.d * p.y + m.ty),
        ]
    }
    fn rect(&self, r: CGRect) -> Rect {
        let a = self.point(r.origin);
        let b = self.point(CGPoint::new(
            r.origin.x + r.size.width,
            r.origin.y + r.size.height,
        ));
        Rect {
            x: a[0].min(b[0]),
            y: a[1].min(b[1]),
            w: (b[0] - a[0]).abs(),
            h: (b[1] - a[1]).abs(),
        }
    }
}

struct Document {
    raster: CFRetained<CGPDFDocument>,
    text: Option<Retained<PDFDocument>>,
    path: PathBuf,
    version: Version,
    started: Instant,
    geometry: BTreeMap<usize, Geometry>,
    loaded: BTreeMap<usize, Page>,
    annotations: HashMap<usize, u64>,
    next_id: u64,
    history: Vec<Action>,
    cursor: usize,
    saved_cursor: Option<usize>,
    editable: bool,
    copying: bool,
    cache: HashMap<RasterKey, Bitmap>,
    lru: VecDeque<RasterKey>,
    metrics: Metrics,
}

impl Document {
    fn open(path: &Path) -> Result<Self, String> {
        let started = Instant::now();
        let url = CFURL::from_file_path(path).ok_or("Invalid PDF path")?;
        let raster = CGPDFDocument::with_url(Some(&url))
            .ok_or("Unable to read this PDF. It may be damaged or inaccessible.")?;
        if !CGPDFDocument::is_unlocked(Some(&raster)) {
            return Err(
                "This PDF is password protected. Open it in a PDF application to unlock it.".into(),
            );
        }
        if CGPDFDocument::number_of_pages(Some(&raster)) == 0 {
            return Err("This PDF contains no pages".into());
        }
        let mut doc = Self {
            raster,
            text: None,
            path: path.into(),
            version: Version::read(path)?,
            started,
            geometry: BTreeMap::new(),
            loaded: BTreeMap::new(),
            annotations: HashMap::new(),
            next_id: 1,
            history: Vec::new(),
            cursor: 0,
            saved_cursor: Some(0),
            editable: false,
            copying: false,
            cache: HashMap::new(),
            lru: VecDeque::new(),
            metrics: Metrics::default(),
        };
        doc.geometry(0)?;
        doc.metrics.parse_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(doc)
    }

    fn rendered(&mut self, sharp: bool) {
        let elapsed = self.started.elapsed().as_secs_f64() * 1000.0;
        if self.metrics.first_bitmap_ms.is_none() {
            self.metrics.first_bitmap_ms = Some(elapsed);
            eprintln!(
                "PDF Performance first_bitmap_ms={elapsed:.3} parse_ms={:.3}",
                self.metrics.parse_ms
            );
        }
        if sharp && self.metrics.first_sharp_ms.is_none() {
            self.metrics.first_sharp_ms = Some(elapsed);
            eprintln!(
                "PDF Performance first_sharp_ms={elapsed:.3} max_raster_ms={:.3}",
                self.metrics.max_raster_ms
            );
        }
    }

    fn text(&mut self) -> Result<Retained<PDFDocument>, String> {
        if let Some(pdf) = &self.text {
            return Ok(pdf.clone());
        }
        let url = NSURL::from_file_path(&self.path).ok_or("Invalid PDF path")?;
        // SAFETY: This PDFDocument is confined to this worker; the URL is retained for this call.
        let pdf = unsafe { PDFDocument::initWithURL(PDFDocument::alloc(), &url) }
            .ok_or("Unable to read PDF text metadata")?;
        // SAFETY: Querying the worker-owned document does not touch AppKit views.
        unsafe {
            self.editable = !pdf.isEncrypted() && pdf.allowsCommenting();
            self.copying = pdf.allowsCopying();
        }
        self.text = Some(pdf.clone());
        Ok(pdf)
    }

    fn summary(&self) -> Summary {
        Summary {
            page_count: CGPDFDocument::number_of_pages(Some(&self.raster)),
            first_size: self
                .geometry
                .get(&0)
                .map(|g| g.size)
                .unwrap_or([612.0, 792.0]),
            editable: self.editable,
            copying: self.copying,
            dirty: self.saved_cursor != Some(self.cursor),
            undo: self.cursor > 0,
            redo: self.cursor < self.history.len(),
            supported_annotations: self.loaded.values().map(|p| p.marks.len()).sum(),
        }
    }

    fn geometry(&mut self, index: usize) -> Result<&Geometry, String> {
        if !self.geometry.contains_key(&index) {
            let page =
                CGPDFDocument::page(Some(&self.raster), index + 1).ok_or("Invalid PDF page")?;
            let crop = CGPDFPage::box_rect(Some(&page), CGPDFBox::CropBox);
            let rotated = CGPDFPage::rotation_angle(Some(&page)).rem_euclid(180) == 90;
            let size = if rotated {
                [crop.size.height, crop.size.width]
            } else {
                [crop.size.width, crop.size.height]
            };
            if !size.iter().all(|v| v.is_finite() && *v > 0.0 && *v < 1e7) {
                return Err("Invalid PDF page dimensions".into());
            }
            let transform = CGPDFPage::drawing_transform(
                Some(&page),
                CGPDFBox::CropBox,
                CGRect::new(CGPoint::ZERO, CGSize::new(size[0], size[1])),
                0,
                true,
            );
            self.geometry.insert(index, Geometry { size, transform });
        }
        self.geometry
            .get(&index)
            .ok_or_else(|| "Invalid PDF geometry".into())
    }

    fn evict(&mut self, key: RasterKey) {
        if let Some(bitmap) = self.cache.remove(&key) {
            self.metrics.cache_bytes -= bitmap.bytes;
        }
        self.lru.retain(|entry| *entry != key);
    }

    fn render(&mut self, key: RasterKey) -> Result<Bitmap, String> {
        if let Some(image) = self.cache.get(&key) {
            self.metrics.cache_hits += 1;
            self.lru.retain(|k| *k != key);
            self.lru.push_back(key);
            return Ok(image.clone());
        }
        let started = Instant::now();
        let width = key.width as usize;
        let height = key.height as usize;
        if width == 0 || height == 0 || width > 8192 || height > 8192 || width > MAX_PIXELS / height
        {
            return Err("PDF crop exceeds the bitmap memory limit".into());
        }
        let geometry = self.geometry(key.page)?;
        let transform = geometry.transform;
        let scale = key.full_width as f64 / geometry.size[0];
        let full_height = geometry.size[1] * scale;
        let page =
            CGPDFDocument::page(Some(&self.raster), key.page + 1).ok_or("Invalid PDF page")?;
        let bitmap = Bitmap::draw(width, height, |context| {
            CGContext::translate_ctm(
                Some(context),
                -(key.x as f64),
                -(full_height - (key.y + key.height) as f64),
            );
            CGContext::scale_ctm(Some(context), scale, scale);
            CGContext::concat_ctm(Some(context), transform);
            CGContext::draw_pdf_page(Some(context), Some(&page));
        })?;
        let cost = started.elapsed().as_secs_f64() * 1000.0;
        self.metrics.max_raster_ms = self.metrics.max_raster_ms.max(cost);
        self.metrics.raster_ms += cost;
        self.metrics.renders += 1;
        let bytes = bitmap.bytes;
        while self.metrics.cache_bytes + bytes > CACHE_LIMIT {
            let Some(oldest) = self.lru.pop_front() else {
                break;
            };
            if let Some(image) = self.cache.remove(&oldest) {
                self.metrics.cache_bytes -= image.bytes;
            }
        }
        // A resized or zoomed page no longer uses sharp crops at its previous resolution.
        if key.full_width != 480 {
            let obsolete: Vec<_> = self
                .cache
                .keys()
                .filter(|old| {
                    old.page == key.page
                        && old.full_width != 480
                        && old.full_width != key.full_width
                })
                .copied()
                .collect();
            for old in obsolete {
                self.evict(old);
            }
        }
        self.cache.insert(key, bitmap.clone());
        self.lru.push_back(key);
        self.metrics.cache_bytes += bytes;
        Ok(bitmap)
    }

    fn content(&mut self, index: usize) -> Result<Page, String> {
        if let Some(page) = self.loaded.get(&index) {
            return Ok(page.clone());
        }
        let started = Instant::now();
        let pdf = self.text()?;
        // SAFETY: All PDFKit access is serialized on this worker, with checked page indexes.
        let page = unsafe { pdf.pageAtIndex(index) }.ok_or("Invalid PDF text page")?;
        let mut content = Page {
            text_ready: true,
            size: self.geometry(index)?.size,
            ..Page::default()
        };
        unsafe {
            if self.copying {
                let count = page.numberOfCharacters().min(100_000);
                content.characters.reserve(count);
                for i in 0..count {
                    let rect = self
                        .geometry(index)?
                        .rect(page.characterBoundsAtIndex(i as isize));
                    if rect.w > 0.0 && rect.h > 0.0 {
                        content.characters.push(Character { index: i, rect });
                    }
                }
            }
            for annotation in page.annotations().iter() {
                if !annotation.shouldDisplay() {
                    continue;
                }
                let kind = annotation
                    .r#type()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if kind == "Link" {
                    let target = annotation.action().and_then(|a| {
                        if let Some(url) = a.downcast_ref::<PDFActionURL>() {
                            url.URL()
                                .and_then(|u| u.absoluteString())
                                .map(|s| LinkTarget::Url(s.to_string()))
                        } else if let Some(go) = a.downcast_ref::<PDFActionGoTo>() {
                            go.destination()
                                .page()
                                .map(|p| LinkTarget::Page(pdf.indexForPage(&p)))
                        } else {
                            None
                        }
                    });
                    if let Some(target) = target {
                        content.links.push(Link {
                            rect: self.geometry(index)?.rect(annotation.bounds()),
                            target,
                        });
                    }
                }
                if let Some(tool) = supported(&kind) {
                    let pointer = Retained::as_ptr(&annotation) as usize;
                    let id = *self.annotations.entry(pointer).or_insert_with(|| {
                        let id = self.next_id;
                        self.next_id += 1;
                        id
                    });
                    let note = annotation
                        .contents()
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    let text_note = annotation
                        .valueForAnnotationKey(&NSString::from_str("/CrawlerTextNote"))
                        .and_then(|v| v.downcast::<objc2_foundation::NSNumber>().ok())
                        .is_some_and(|n| n.boolValue());
                    let tool = if (tool == Tool::Underline || tool == Tool::Highlight)
                        && (text_note
                            || !note.is_empty()
                            || annotation
                                .valueForAnnotationKey(PDFAnnotationKeyName)
                                .is_some_and(|v| {
                                    v.downcast_ref::<NSString>()
                                        .is_some_and(|s| s.to_string().starts_with("crawler-note-"))
                                }))
                    {
                        Tool::Note
                    } else {
                        tool
                    };
                    let mut rects = Vec::new();
                    if let Some(points) = annotation.quadrilateralPoints() {
                        let points: Vec<_> = points.iter().map(|p| p.pointValue()).collect();
                        for quad in points.chunks_exact(4) {
                            let origin = annotation.bounds().origin;
                            let xs = quad
                                .iter()
                                .map(|p| p.x + origin.x)
                                .fold(f64::INFINITY, f64::min);
                            let ys = quad
                                .iter()
                                .map(|p| p.y + origin.y)
                                .fold(f64::INFINITY, f64::min);
                            let x1 = quad
                                .iter()
                                .map(|p| p.x + origin.x)
                                .fold(f64::NEG_INFINITY, f64::max);
                            let y1 = quad
                                .iter()
                                .map(|p| p.y + origin.y)
                                .fold(f64::NEG_INFINITY, f64::max);
                            rects.push(self.geometry(index)?.rect(CGRect::new(
                                CGPoint::new(xs, ys),
                                CGSize::new(x1 - xs, y1 - ys),
                            )));
                        }
                    }
                    if rects.is_empty() {
                        rects.push(self.geometry(index)?.rect(annotation.bounds()));
                    }
                    let color = annotation
                        .color()
                        .colorUsingColorSpace(&NSColorSpace::sRGBColorSpace());
                    let color = color
                        .map(|c| {
                            ((c.redComponent() * 255.0) as u32) << 16
                                | ((c.greenComponent() * 255.0) as u32) << 8
                                | (c.blueComponent() * 255.0) as u32
                        })
                        .unwrap_or(0xffd11f);
                    content.marks.push(Mark {
                        id,
                        rects,
                        tool,
                        color,
                        note,
                    });
                }
            }
        }
        self.metrics.metadata_ms += started.elapsed().as_secs_f64() * 1000.0;
        self.loaded.insert(index, content.clone());
        Ok(content)
    }

    fn refresh_loaded(&mut self) -> Result<BTreeMap<usize, Page>, String> {
        let indices: Vec<_> = self.loaded.keys().copied().collect();
        self.loaded.clear();
        for index in indices {
            self.content(index)?;
        }
        Ok(self.loaded.clone())
    }

    fn perform(&self, action: &Action, forward: bool) -> Result<(), String> {
        let pdf = self
            .text
            .as_ref()
            .ok_or("PDF text metadata is unavailable")?;
        for change in &action.0 {
            // SAFETY: The page and retained annotation belong to this document and worker.
            unsafe {
                let page = pdf
                    .pageAtIndex(change.page)
                    .ok_or("Invalid annotation page")?;
                if change.added == forward {
                    page.addAnnotation(&change.annotation);
                } else {
                    page.removeAnnotation(&change.annotation);
                }
            }
        }
        Ok(())
    }

    fn commit(&mut self, action: Action) -> Result<(), String> {
        if action.0.is_empty() {
            return Ok(());
        }
        self.perform(&action, true)?;
        if self.saved_cursor.is_some_and(|s| s > self.cursor) {
            self.saved_cursor = None;
        }
        self.history.truncate(self.cursor);
        self.history.push(action);
        self.cursor += 1;
        if self.history.len() > 100 {
            self.history.remove(0);
            self.cursor -= 1;
            self.saved_cursor = self.saved_cursor.and_then(|c| c.checked_sub(1));
        }
        Ok(())
    }

    fn find(&mut self, id: u64) -> Result<(usize, Retained<PDFAnnotation>), String> {
        let pdf = self.text()?;
        for index in self.loaded.keys() {
            // SAFETY: The loaded page indices and annotation map come from this PDFDocument.
            unsafe {
                if let Some(page) = pdf.pageAtIndex(*index) {
                    for annotation in page.annotations().iter() {
                        if self
                            .annotations
                            .get(&(Retained::as_ptr(&annotation) as usize))
                            == Some(&id)
                        {
                            let flags = annotation
                                .valueForAnnotationKey(&NSString::from_str("/F"))
                                .and_then(|v| v.downcast::<objc2_foundation::NSNumber>().ok())
                                .map(|n| n.unsignedIntValue())
                                .unwrap_or(0);
                            if flags & 64 != 0 {
                                return Err("This annotation is read-only".into());
                            }
                            return Ok((*index, annotation));
                        }
                    }
                }
            }
        }
        Err("The selected annotation is no longer available".into())
    }

    fn edit(&mut self, edit: Edit) -> Result<(), String> {
        let pdf = self.text()?;
        if !self.editable {
            return Err(
                "This PDF does not allow annotations. Encrypted PDFs are read-only.".into(),
            );
        }
        // SAFETY: These calls operate only on the worker-owned PDFDocument and retained
        // annotations. PDFKit supplies the selection bounds in original page coordinates.
        unsafe {
            match edit {
                Edit::Add {
                    selection: range,
                    tool,
                    ink,
                    note,
                } => {
                    if tool == Tool::Select {
                        return Ok(());
                    }
                    let selection = self
                        .selection(Some(range))?
                        .ok_or("Select text to annotate")?;
                    let mut changes = Vec::new();
                    for page_ref in selection.pages().iter() {
                        let page = pdf.indexForPage(&page_ref);
                        let bounds = selection.boundsForPage(&page_ref);
                        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
                            continue;
                        }
                        let subtype = NSString::from_str(if tool == Tool::Highlight {
                            "Highlight"
                        } else {
                            "Underline"
                        });
                        let annotation = PDFAnnotation::initWithBounds_forType_withProperties(
                            PDFAnnotation::alloc(),
                            bounds,
                            &subtype,
                            None,
                        );
                        let mut points = Vec::new();
                        for line in selection.selectionsByLine().iter() {
                            if !line.pages().iter().any(|p| pdf.indexForPage(&p) == page) {
                                continue;
                            }
                            let rect = line.boundsForPage(&page_ref);
                            let x = rect.origin.x - bounds.origin.x;
                            let y = rect.origin.y - bounds.origin.y;
                            let w = rect.size.width;
                            let h = rect.size.height;
                            points.extend([
                                NSValue::valueWithPoint(NSPoint::new(x, y + h)),
                                NSValue::valueWithPoint(NSPoint::new(x + w, y + h)),
                                NSValue::valueWithPoint(NSPoint::new(x, y)),
                                NSValue::valueWithPoint(NSPoint::new(x + w, y)),
                            ]);
                        }
                        annotation
                            .setQuadrilateralPoints(Some(&NSArray::from_retained_slice(&points)));
                        let color = ink.rgb();
                        annotation.setColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
                            ((color >> 16) & 255) as f64 / 255.0,
                            ((color >> 8) & 255) as f64 / 255.0,
                            (color & 255) as f64 / 255.0,
                            1.0,
                        ));
                        if tool == Tool::Note {
                            annotation.setContents(Some(&NSString::from_str(&note)));
                            annotation.setBoolean_forAnnotationKey(
                                true,
                                &NSString::from_str("/CrawlerTextNote"),
                            );
                            let name = NSString::from_str(&format!(
                                "crawler-note-{}-{page}",
                                self.next_id
                            ));
                            annotation.setValue_forAnnotationKey(&name, PDFAnnotationKeyName);
                        }
                        changes.push(Change {
                            page,
                            annotation,
                            added: true,
                        });
                    }
                    self.commit(Action(changes))?;
                }
                Edit::Delete(id) => {
                    let (page, annotation) = self.find(id)?;
                    self.commit(Action(vec![Change {
                        page,
                        annotation,
                        added: false,
                    }]))?;
                }
                Edit::Note { id, text } => {
                    let (page, original) = self.find(id)?;
                    let replacement = original.copy();
                    replacement.setContents(Some(&NSString::from_str(&text)));
                    self.commit(Action(vec![
                        Change {
                            page,
                            annotation: original,
                            added: false,
                        },
                        Change {
                            page,
                            annotation: replacement,
                            added: true,
                        },
                    ]))?;
                }
                Edit::Color { id, ink } => {
                    let (page, original) = self.find(id)?;
                    let replacement = original.copy();
                    let color = ink.rgb();
                    replacement.setColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
                        ((color >> 16) & 255) as f64 / 255.0,
                        ((color >> 8) & 255) as f64 / 255.0,
                        (color & 255) as f64 / 255.0,
                        1.0,
                    ));
                    self.commit(Action(vec![
                        Change {
                            page,
                            annotation: original,
                            added: false,
                        },
                        Change {
                            page,
                            annotation: replacement,
                            added: true,
                        },
                    ]))?;
                }
                Edit::Undo if self.cursor > 0 => {
                    self.perform(&self.history[self.cursor - 1], false)?;
                    self.cursor -= 1;
                }
                Edit::Redo if self.cursor < self.history.len() => {
                    self.perform(&self.history[self.cursor], true)?;
                    self.cursor += 1;
                }
                Edit::RemoveAll => {
                    let mut changes = Vec::new();
                    for index in 0..pdf.pageCount() {
                        if let Some(page) = pdf.pageAtIndex(index) {
                            for annotation in page.annotations().iter() {
                                if annotation
                                    .r#type()
                                    .is_some_and(|s| supported(&s.to_string()).is_some())
                                {
                                    changes.push(Change {
                                        page: index,
                                        annotation,
                                        added: false,
                                    });
                                }
                            }
                        }
                    }
                    self.commit(Action(changes))?;
                }
                Edit::Discard => {
                    if let Some(saved) = self.saved_cursor {
                        while self.cursor > saved {
                            self.perform(&self.history[self.cursor - 1], false)?;
                            self.cursor -= 1;
                        }
                        while self.cursor < saved {
                            self.perform(&self.history[self.cursor], true)?;
                            self.cursor += 1;
                        }
                        self.history.truncate(self.cursor);
                    } else {
                        self.text = None;
                        self.text()?;
                        self.annotations.clear();
                        self.history.clear();
                        self.cursor = 0;
                        self.saved_cursor = Some(0);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn selection(
        &mut self,
        selection: Option<TextRange>,
    ) -> Result<Option<Retained<objc2_pdf_kit::PDFSelection>>, String> {
        let pdf = self.text()?;
        if !self.copying {
            return Err("This PDF does not allow copying text".into());
        }
        // SAFETY: PDFKit's page points and selection metadata stay on this worker.
        unsafe {
            if let Some(selection) = selection {
                let start = pdf
                    .pageAtIndex(selection.start.page)
                    .ok_or("Invalid selection page")?;
                let end = pdf
                    .pageAtIndex(selection.end.page)
                    .ok_or("Invalid selection page")?;
                if selection.start.character >= start.numberOfCharacters()
                    || selection.end.character >= end.numberOfCharacters()
                {
                    return Err("This page has no selectable text".into());
                }
                Ok(pdf.selectionFromPage_atPoint_toPage_atPoint(
                    &start,
                    character_center(&start, selection.start.character),
                    &end,
                    character_center(&end, selection.end.character),
                ))
            } else {
                Ok(pdf.selectionForEntireDocument())
            }
        }
    }

    fn copy(&mut self, selection: Option<TextRange>) -> Result<String, String> {
        // SAFETY: The returned selection and string are used only on their owning worker.
        Ok(unsafe {
            self.selection(selection)?
                .and_then(|s| s.string())
                .map(|s| s.to_string())
                .unwrap_or_default()
        })
    }

    fn save(&mut self, destination: &Path) -> Result<(), String> {
        let destination = if destination.exists() {
            fs::canonicalize(destination).map_err(|e| e.to_string())?
        } else {
            let parent = fs::canonicalize(
                destination
                    .parent()
                    .ok_or("The save destination has no parent folder")?,
            )
            .map_err(|e| e.to_string())?;
            parent.join(
                destination
                    .file_name()
                    .ok_or("The save destination has no file name")?,
            )
        };
        let destination = destination.as_path();
        let pdf = self.text()?;
        if !self.editable {
            return Err("This PDF is read-only".into());
        }
        let current = fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        let same = destination == current;
        if same && Version::read(destination)? != self.version {
            return Err("The PDF changed on disk. Use Save as to keep your annotations.".into());
        }
        // SAFETY: PDFKit serializes this document on its owning worker. No PDFKit drawing.
        let data = unsafe { pdf.dataRepresentation() }
            .ok_or("The PDF could not be saved. Your edits are still available.")?;
        let parent = destination
            .parent()
            .ok_or("The save destination has no parent folder")?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temporary = parent.join(format!(
            ".crawler-pdf-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| -> Result<(), String> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|e| e.to_string())?;
            file.write_all(&data.to_vec()).map_err(|e| e.to_string())?;
            if let Ok(metadata) = fs::metadata(destination) {
                file.set_permissions(metadata.permissions())
                    .map_err(|e| e.to_string())?;
            }
            file.sync_all().map_err(|e| e.to_string())?;
            if same && Version::read(destination)? != self.version {
                return Err(
                    "The PDF changed on disk. Use Save as to keep your annotations.".into(),
                );
            }
            fs::rename(&temporary, destination).map_err(|e| e.to_string())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        self.path = destination.into();
        self.version = Version::read(destination)?;
        self.saved_cursor = Some(self.cursor);
        Ok(())
    }
}

fn supported(kind: &str) -> Option<Tool> {
    match kind {
        "Highlight" => Some(Tool::Highlight),
        "Underline" => Some(Tool::Underline),
        "Text" => Some(Tool::Note),
        _ => None,
    }
}

/// PDFKit's character-bounds indices exclude synthesized line separators, while selection
/// ranges use NSString positions. Select by PDFKit's page points to preserve its text order.
unsafe fn character_center(page: &objc2_pdf_kit::PDFPage, index: usize) -> NSPoint {
    let r = unsafe { page.characterBoundsAtIndex(index as isize) };
    NSPoint::new(
        r.origin.x + r.size.width * 0.5,
        r.origin.y + r.size.height * 0.5,
    )
}
