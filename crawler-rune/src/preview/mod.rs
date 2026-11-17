//! Shared, bounded file previews, the Space window and the document reader: Markdown, Word,
//! spreadsheets, code, plain text, images and video. PDFs have their own reader in `crate::pdf`.
mod content;
pub(crate) mod controls;
mod native;
mod sheet;
mod word;

use crate::app::{App, Paint, Reader, Section};
use crate::theme::Theme;
use content::{Body, Budget, Document, Format, Request};
use rune::prefs::Prefs;
use rune::tok::{Family, TextStyle, motion, type_};
use rune::{
    AppHandle, AppSpec, Cx, El, GrowthSpec, ImageCache, Key, KeyEvent, Latest, Player, ScrollSpec,
    SurfaceFit, TextEdit, Transition, Tx, Waker, WindowHandle, WindowOptions, WindowSpec, col, el,
    rich, row, text,
};
use rune_kit::{
    Markdown, MarkdownStyle, TextInput,
    image_zoom::{Stage, StageZoom},
    syntax::SyntaxTheme,
};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

type ReaderDisplayed = fn(&mut App, &Path);

#[derive(Default)]
struct Slot {
    path: Option<PathBuf>,
    /// The workbook sheet requested with `path`.
    sheet: usize,
    task: Latest<Result<Arc<Document>, String>>,
    generation: Arc<AtomicU64>,
    document: Option<Arc<Document>>,
    error: Option<String>,
    player: Option<Player>,
    started: bool,
    playing_requested: Cell<bool>,
    muted: bool,
    displayed_second: u64,
    requested_at: Option<std::time::Instant>,
    load_ms: Option<f64>,
}
impl Slot {
    fn request(
        &mut self,
        path: &Path,
        sheet: usize,
        budget: Budget,
        cache: &Option<Arc<ImageCache>>,
    ) {
        if self.path.as_deref() == Some(path) && self.sheet == sheet {
            return;
        }
        self.cancel();
        self.requested_at = Some(std::time::Instant::now());
        self.load_ms = None;
        self.path = Some(path.to_owned());
        self.sheet = sheet;
        self.document = None;
        self.error = None;
        self.started = false;
        self.muted = false;
        self.displayed_second = 0;
        let Some(cache) = cache else {
            self.path = None;
            return;
        };
        let revision = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.task.request(
            Request {
                path: path.to_owned(),
                budget,
                sheet,
                revision,
                generation: self.generation.clone(),
                cache: cache.clone(),
            },
            content::load,
        );
    }
    fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.task.cancel();
        if let Some(player) = self.player.take() {
            player.pause();
        }
    }
    fn take(&mut self) -> bool {
        let Some(result) = self.task.take() else {
            return false;
        };
        self.load_ms = self
            .requested_at
            .map(|at| at.elapsed().as_secs_f64() * 1000.0);
        match result {
            Ok(document) => self.adopt(document),
            Err(error) => self.error = Some(error),
        }
        true
    }
    /// Shows a loaded document. Video gets its player here, so playback stays opt-in.
    fn adopt(&mut self, document: Arc<Document>) {
        if document.format == Format::Video {
            self.player = Some(Player::file(&document.path));
        }
        self.document = Some(document);
    }
    /// Keeps a playing video's clock current. Returns whether the shown second changed.
    fn tick(&mut self, landed: bool, waker: &Option<Waker>, tx: &mut Tx) -> bool {
        let Some(player) = &self.player else {
            return false;
        };
        if landed && let Some(waker) = waker {
            player.set_waker(waker.clone());
        }
        if !matches!(player.state(), rune::PlayerState::Playing) {
            return false;
        }
        tx.wake_after(std::time::Duration::from_millis(500));
        let second = player.time().max(0.0) as u64;
        let changed = second != self.displayed_second;
        self.displayed_second = second;
        changed
    }
    fn pause(&self) {
        if self.playing_requested.replace(false)
            && let Some(player) = &self.player
        {
            player.pause();
        }
    }
}

pub struct State {
    pane: RefCell<Slot>,
    reader: Slot,
    cache: Option<Arc<ImageCache>>,
    scale: u16,
    pinch: f32,
    history: Vec<(PathBuf, usize)>,
    visible_block: usize,
    visible_end: usize,
    pending_position: Option<Position>,
    find_open: bool,
    find_edit: TextEdit,
    find_task: Latest<Vec<content::Found>>,
    found: Vec<content::Found>,
    current_match: usize,
    space: Option<(AppHandle<App>, WindowHandle)>,
    space_parent: Option<AppHandle<App>>,
    space_open: Arc<AtomicBool>,
    pub(crate) space_window: bool,
    space_path: Option<PathBuf>,
    /// The heading list beside Markdown and Word documents.
    outline_open: bool,
    /// Whether code and text lines wrap at the window's edge.
    wrap: bool,
    goto_open: bool,
    goto_edit: TextEdit,
    image_zoom: StageZoom,
    reader_displayed: Option<ReaderDisplayed>,
    did_display: bool,
}
impl Default for State {
    fn default() -> Self {
        Self::new(&Prefs::memory())
    }
}
impl State {
    pub fn new(prefs: &Prefs) -> Self {
        let saved: f64 = prefs.get("markdownReaderScale").unwrap_or(1.0);
        let scale = if saved.is_finite() {
            ((saved * 10.0).round() * 10.0).clamp(70.0, 200.0) as u16
        } else {
            100
        };
        Self {
            pane: RefCell::new(Slot::default()),
            reader: Slot::default(),
            cache: None,
            scale,
            pinch: 0.0,
            history: Vec::new(),
            visible_block: 0,
            visible_end: 0,
            pending_position: None,
            find_open: false,
            find_edit: TextEdit::new(""),
            find_task: Latest::default(),
            found: Vec::new(),
            current_match: 0,
            space: None,
            space_parent: None,
            space_open: Arc::default(),
            space_window: false,
            space_path: None,
            outline_open: prefs.get("readerOutline").unwrap_or(false),
            wrap: prefs.get("readerWrapLines").unwrap_or(true),
            goto_open: false,
            goto_edit: TextEdit::new(""),
            image_zoom: StageZoom::default(),
            reader_displayed: None,
            did_display: false,
        }
    }
}
enum Position {
    Block(usize),
    Anchor(String),
}

/// New York is a hidden UI font. Rune's named-family lookup cannot discover its files.
pub fn fonts() -> &'static [&'static [u8]] {
    use std::sync::OnceLock;
    static MAPPINGS: OnceLock<Vec<memmap2::Mmap>> = OnceLock::new();
    static FONTS: OnceLock<Vec<&'static [u8]>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mapped = MAPPINGS.get_or_init(|| {
            [
                "/System/Library/Fonts/NewYork.ttf",
                "/System/Library/Fonts/NewYorkItalic.ttf",
            ]
            .into_iter()
            .filter_map(|path| {
                let file = std::fs::File::open(path).ok()?;
                // SAFETY: These system font files are read-only on macOS's sealed volume.
                unsafe { memmap2::Mmap::map(&file) }.ok()
            })
            .collect()
        });
        crate::theme::FONTS
            .iter()
            .copied()
            .chain(mapped.iter().map(|m| &m[..]))
            .collect()
    })
}

pub fn setup(app: &mut App, waker: &Waker) {
    app.preview.cache = Some(Arc::new(ImageCache::new(waker.clone())));
    app.preview.pane.borrow().task.set_waker(waker.clone());
    app.preview.reader.task.set_waker(waker.clone());
    app.preview.find_task.set_waker(waker.clone());
}

/// The opening module records recent files only after a document loads successfully.
pub fn set_reader_display_observer(app: &mut App, observer: ReaderDisplayed) {
    app.preview.reader_displayed = Some(observer);
}

pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let preview_visible = app.preview.space_window
        || (app.reader == Reader::None
            && matches!(app.section, Section::Search | Section::Folders)
            && !app.preview.space_open.load(Ordering::Relaxed));
    let mut changed = false;
    if preview_visible {
        let mut pane = app.preview.pane.borrow_mut();
        let landed = pane.take();
        changed |= landed | pane.tick(landed, &app.waker, tx);
    } else {
        let mut pane = app.preview.pane.borrow_mut();
        pane.pause();
        if pane.task.is_pending() {
            pane.cancel();
            pane.path = None;
        }
    }
    if app.reader == Reader::Document {
        let loaded = app.preview.reader.take();
        changed |= loaded | app.preview.reader.tick(loaded, &app.waker, tx);
        if loaded {
            app.preview.found.clear();
            app.preview.find_task.cancel();
        }
        if let Some(doc) = app.preview.reader.document.clone() {
            if !app.preview.did_display {
                app.preview.did_display = true;
                if let Some(observer) = app.preview.reader_displayed {
                    observer(app, &doc.path);
                }
            }
            if app.preview.find_open {
                let needle = app.preview.find_edit.text.clone();
                let key = (doc.path.clone(), doc.revision, needle.clone());
                app.preview
                    .find_task
                    .request_if_changed(&key, move |_| content::find(&doc, &needle));
            }
            if let Some(found) = app.preview.find_task.take() {
                app.preview.found = found;
                app.preview.current_match = 0;
                reveal_match(app, tx);
                changed = true;
            }
            if let Some(position) = app.preview.pending_position.take() {
                let index = match position {
                    Position::Block(i) => Some(i),
                    Position::Anchor(anchor) => {
                        app.preview.reader.document.as_ref().and_then(|d| {
                            d.blocks().iter().position(|b| {
                                b.heading.as_ref().is_some_and(|h| h.slug == anchor)
                            })
                        })
                    }
                };
                if let Some(i) = index {
                    tx.scroll_into_view(block_key("reader", i));
                }
            }
        }
    }
    changed
}

/// The shared preview panel. Starting a new demand supersedes the old worker immediately.
pub fn pane(app: &App, cx: &mut Cx<App>, p: &Paint, path: &Path, key: &str) -> El {
    let suspended = !app.preview.space_window
        && (app.reader != Reader::None || app.preview.space_open.load(Ordering::Relaxed));
    if Format::of(path) == Format::Pdf && !suspended {
        return crate::pdf::pane(app, cx, p, path, key);
    }
    let mut panel = col()
        .key(key.to_owned())
        .w_full()
        .h_full()
        .min_w(0.0)
        .fill(p.look.raised);
    if suspended {
        return panel.child(message("Preview is open in another window", p));
    }
    let mut slot = app.preview.pane.borrow_mut();
    slot.request(path, 0, Budget::Preview, &app.preview.cache);
    if let Some(error) = &slot.error {
        return panel.child(message(error, p));
    }
    let Some(doc) = slot.document.clone() else {
        return panel.child(message("Loading preview…", p));
    };
    let content = match (&doc.body, doc.format) {
        (Body::Blocks(_) | Body::Text(_), _) => document_view(app, p, doc.clone(), key, false),
        (Body::Sheet(_), _) => sheet_view(app, p, doc.clone(), key, false),
        (Body::None, Format::Video) => video_view(p, &slot, key, false),
        (Body::None, Format::Pdf) => el(),
        (Body::None, _) => picture_view(p, &doc),
    };
    let modified = doc
        .modified
        .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|v| format!(" · Modified {}", date(v.as_secs())))
        .unwrap_or_default();
    let status = if doc.truncated {
        doc.budget.notice(doc.format).to_owned()
    } else {
        summary(&doc)
    };
    panel = panel.child(
        content
            .key(format!("{key}-content-{}", path.display()))
            .grow(1.0)
            .min_h(0.0)
            .transition(Transition::opacity())
            .motion(motion::SNAPPY),
    );
    if doc.format != Format::Video {
        panel = panel.child(
            row()
                .w_full()
                .h(30.0)
                .px(12.0)
                .items_center()
                .fill(p.look.raised)
                .help(format!(
                    "{} · {}{}",
                    doc.name(),
                    doc.bytes
                        .map(byte_size)
                        .unwrap_or_else(|| "Size unknown".into()),
                    modified
                ))
                .child(
                    text(status)
                        .font(p.look.font(10.0, 400.0))
                        .color(p.look.muted)
                        .truncate(),
                ),
        );
    }
    panel
}
fn picture_view(p: &Paint, doc: &Document) -> El {
    match &doc.thumbnail {
        Some(image) if image.error().is_none() => el().w_full().h_full().px(18.0).py(18.0).child(
            el().w_full()
                .h_full()
                .pixels_fit(image.pixels(), SurfaceFit::Fit),
        ),
        _ => {
            let icon = doc
                .icon
                .as_ref()
                .map(|i| {
                    el().size(64.0, 64.0)
                        .pixels_fit(i.pixels(), SurfaceFit::Fit)
                })
                .unwrap_or_else(|| {
                    text("▤")
                        .font(TextStyle::new(56.0, 400.0))
                        .color(p.look.muted)
                });
            col()
                .w_full()
                .h_full()
                .center()
                .gap(12.0)
                .child(icon)
                .child(
                    text("No thumbnail available")
                        .font(type_::BODY)
                        .color(p.look.muted),
                )
        }
    }
}
/// The pane's or the reader's video slot, for handlers that only know which one drew them.
fn video_slot<R>(app: &mut App, reader: bool, f: impl FnOnce(&mut Slot) -> R) -> R {
    if reader {
        f(&mut app.preview.reader)
    } else {
        f(&mut app.preview.pane.borrow_mut())
    }
}
fn video_view(p: &Paint, slot: &Slot, key: &str, reader: bool) -> El {
    let Some(player) = &slot.player else {
        return message("Preparing video…", p);
    };
    let playing = matches!(player.state(), rune::PlayerState::Playing);
    let duration = player.duration().unwrap_or(0.0);
    let time = player.time();
    let progress = if duration > 0.0 {
        (time / duration).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let controls = row()
        .gap(8.0)
        .px(10.0)
        .py(10.0)
        .items_center()
        .child(
            crate::shell::shell_button(
                p,
                &format!("{key}-play"),
                if playing { "Pause" } else { "Play" },
                false,
            )
            .on_press(move |a: &mut App| video_slot(a, reader, toggle_play)),
        )
        .child(
            el().key(format!("{key}-seek"))
                .role("slider")
                .label("Video position")
                .focusable(true)
                .grow(1.0)
                .min_w(24.0)
                .h(22.0)
                .justify_center()
                .child(
                    el().w_full()
                        .h(4.0)
                        .fill(p.look.line)
                        .radius(2.0)
                        .child(el().w_frac(progress).h(4.0).fill(p.look.accent)),
                )
                .on_drag(move |a: &mut App, drag, _| {
                    if drag.phase == rune::Phase::Up && !drag.cancelled() {
                        let at = f64::from((drag.x / drag.w.max(1.0)).clamp(0.0, 1.0));
                        video_slot(a, reader, |slot| {
                            if let Some(player) = &slot.player
                                && let Some(duration) = player.duration()
                            {
                                player.seek(at * duration);
                            }
                        });
                    }
                })
                .on_key(move |a: &mut App, event, _| {
                    video_slot(a, reader, |slot| {
                        let Some(player) = &slot.player else {
                            return false;
                        };
                        match event.key {
                            Key::Left => player.seek(player.time() - 5.0),
                            Key::Right => player.seek(player.time() + 5.0),
                            Key::Home => player.seek(0.0),
                            Key::End => player.seek(player.duration().unwrap_or(0.0)),
                            _ => return false,
                        }
                        true
                    })
                }),
        )
        .child(
            text(format!("{} / {}", clock(time), clock(duration)))
                .font(type_::LABEL)
                .color(p.look.muted),
        )
        .child(
            crate::shell::shell_button(
                p,
                &format!("{key}-mute"),
                if slot.muted { "Unmute" } else { "Mute" },
                false,
            )
            .on_press(move |a: &mut App| {
                video_slot(a, reader, |slot| {
                    slot.muted = !slot.muted;
                    if let Some(player) = &slot.player {
                        player.set_muted(slot.muted || !slot.started);
                    }
                })
            }),
        );
    let mut view = col().w_full().h_full().child(
        el().w_full()
            .grow(1.0)
            .min_h(0.0)
            .video(player, SurfaceFit::Fit),
    );
    if let rune::PlayerState::Failed(error) = player.state() {
        view = view.child(
            text(error)
                .font(type_::BODY)
                .color(p.look.muted)
                .wrap()
                .px(12.0)
                .py(12.0),
        );
    }
    view.child(controls)
}
fn toggle_play(slot: &mut Slot) {
    slot.started = true;
    if let Some(player) = &slot.player {
        if matches!(player.state(), rune::PlayerState::Playing) {
            slot.playing_requested.set(false);
            player.pause();
        } else {
            slot.playing_requested.set(true);
            player.set_muted(slot.muted);
            player.play();
        }
    }
}
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Every readable format shares one reader, with workspace state retained underneath.
pub fn open_reader(app: &mut App, path: &Path, tx: &mut Tx) -> Result<(), String> {
    if !Format::of(path).readable() {
        return Err("Crawler has no reader for this file.".into());
    }
    app.preview.history.clear();
    navigate(app, path, Position::Block(0), tx);
    Ok(())
}
fn navigate(app: &mut App, path: &Path, position: Position, tx: &mut Tx) {
    app.preview.pane.borrow().pause();
    app.preview.reader.cancel();
    app.preview.reader.path = None;
    app.preview
        .reader
        .request(path, 0, Budget::Reader, &app.preview.cache);
    // A complete pane read already holds the whole document and can be shared at once.
    // Pictures are decoded smaller for the pane, so the reader reads those again.
    let shared = app
        .preview
        .pane
        .borrow()
        .document
        .as_ref()
        .filter(|d| d.path == path && !d.truncated && !matches!(d.body, Body::None))
        .cloned();
    if let Some(document) = shared {
        app.preview.reader.cancel();
        app.preview.reader.adopt(document);
    }
    app.preview.find_task.cancel();
    app.preview.found.clear();
    app.preview.pending_position = Some(position);
    app.preview.visible_block = 0;
    app.preview.visible_end = 0;
    app.preview.did_display = false;
    app.preview.goto_open = false;
    app.preview.image_zoom = StageZoom::default();
    app.reader = Reader::Document;
    app.reader_escape_at = None;
    tx.focus(READER, false);
}
/// Returns to the document a PDF was opened from (a link in Markdown), if there is one.
pub fn resume_reader(app: &mut App) -> bool {
    let waiting = app.preview.reader.path.is_some();
    if waiting {
        app.reader = Reader::Document;
    }
    waiting
}
/// Gives the keyboard back to the reader, after a PDF opened from it closes.
pub fn focus_reader(tx: &mut Tx) {
    tx.focus(READER, false);
}
/// Whether a reader text field has the keyboard, so letters are text rather than commands.
pub fn typing(app: &App) -> bool {
    app.preview.find_open || app.preview.goto_open
}

const READER: &str = "document-reader";

pub fn reader_view(app: &App, cx: &mut Cx<App>, p: &Paint, _layer: &mut Vec<El>) -> El {
    let reader = &app.preview.reader;
    let mut view = col()
        .key(READER)
        .focusable(true)
        .w_full()
        .h_full()
        .fill(p.look.raised)
        .child(reader_header(app, cx, p));
    if app.preview.find_open {
        view = view.child(find_bar(app, p));
    }
    if app.preview.goto_open {
        view = view.child(goto_bar(app, p));
    }
    let body = match &reader.document {
        None => message(
            reader.error.as_deref().unwrap_or("Loading document…"),
            p,
        ),
        Some(doc) => match (&doc.body, doc.format) {
            (Body::Blocks(_), _) if app.preview.outline_open => row()
                .w_full()
                .h_full()
                .child(outline(app, p, doc))
                .child(
                    document_view(app, p, doc.clone(), "reader", true)
                        .grow(1.0)
                        .min_w(0.0),
                ),
            (Body::Blocks(_) | Body::Text(_), _) => {
                document_view(app, p, doc.clone(), "reader", true)
            }
            (Body::Sheet(_), _) => sheet_view(app, p, doc.clone(), "reader", true),
            (Body::None, Format::Video) => video_view(p, reader, "reader-video", true),
            (Body::None, _) => image_view(app, cx, p, doc),
        },
    };
    view.child(body.grow(1.0).min_h(0.0))
}

/// One header for every format: back, the name, where you are, then only the controls the
/// open document can use.
fn reader_header(app: &App, cx: &mut Cx<App>, p: &Paint) -> El {
    let reader = &app.preview.reader;
    let path = reader.path.as_deref().unwrap_or(Path::new(""));
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned();
    let doc = reader.document.as_deref();
    let body = doc.map(|d| &d.body);
    let mut header = row()
        .key("reader-header")
        .w_full()
        .h(48.0)
        .pl(cx.titlebar_inset().max(88.0))
        .pr(14.0)
        .gap(4.0)
        .items_center()
        .fill(p.look.raised)
        .window_drag()
        .border_sides([0.0, 0.0, 1.0, 0.0], p.look.line);
    if !app.preview.history.is_empty() {
        header = header.child(
            controls::button(p, "reader-back", "", Some(&controls::BACK), false)
                .w(32.0)
                .label("Back · ⌘[")
                .on_press_tx(|a: &mut App, tx| back(a, tx)),
        );
    }
    let place = match (doc, body) {
        (Some(doc), Some(Body::Blocks(_) | Body::Text(_) | Body::Sheet(_))) => {
            let rows = doc.rows().max(1);
            format!("{}%", (app.preview.visible_end * 100 / rows).min(100))
        }
        (Some(doc), _) => doc
            .thumbnail
            .as_ref()
            .and_then(|image| image.size())
            .map(|[w, h]| format!("{w} × {h}"))
            .unwrap_or_default(),
        _ => String::new(),
    };
    header = header
        .child(
            text(name)
                .shrink()
                .font(p.look.font(12.0, 500.0))
                .color(p.look.text)
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(
            text(if app.reader_escape_at.is_some() {
                "Press Esc again to close".to_owned()
            } else {
                place
            })
            .font(p.look.font(11.0, 400.0))
            .color(p.look.muted)
            .align(rune::Align::End),
        );
    if matches!(body, Some(Body::Blocks(_))) {
        header = header.child(
            controls::button(
                p,
                "reader-outline-toggle",
                "Outline",
                None,
                app.preview.outline_open,
            )
            .help("Headings · O")
            .on_press(toggle_outline),
        );
    }
    if matches!(body, Some(Body::Text(_))) {
        header = header.child(
            controls::button(p, "reader-wrap", "Wrap", None, app.preview.wrap)
                .help("Wrap long lines · W")
                .on_press(toggle_wrap),
        );
    }
    if matches!(body, Some(Body::Text(_) | Body::Sheet(_))) {
        let sheet = matches!(body, Some(Body::Sheet(_)));
        header = header.child(
            controls::button(
                p,
                "reader-goto",
                if sheet { "Go to row" } else { "Go to line" },
                None,
                app.preview.goto_open,
            )
            .help("⌘L")
            .on_press_tx(|a: &mut App, tx| show_goto(a, tx)),
        );
    }
    if matches!(body, Some(Body::Blocks(_) | Body::Text(_))) {
        header = header
            .child(
                controls::button(p, "reader-zoom-out", "−", None, false)
                    .w(30.0)
                    .help("Zoom out · ⌘−")
                    .on_press(|a: &mut App| zoom(a, -1)),
            )
            .child(
                controls::button(
                    p,
                    "reader-zoom-reset",
                    &format!("{}%", app.preview.scale),
                    None,
                    false,
                )
                .w(52.0)
                .help("Actual size · ⌘0")
                .on_press(|a: &mut App| zoom(a, 0)),
            )
            .child(
                controls::button(p, "reader-zoom-in", "+", None, false)
                    .w(30.0)
                    .help("Zoom in · ⌘+")
                    .on_press(|a: &mut App| zoom(a, 1)),
            );
    }
    if matches!(
        body,
        Some(Body::Blocks(_) | Body::Text(_) | Body::Sheet(_))
    ) {
        header = header.child(
            controls::button(p, "reader-find", "Find", None, app.preview.find_open)
                .help("⌘F")
                .on_press_tx(|a: &mut App, tx| show_find(a, tx)),
        );
    }
    header.child(
        controls::button(p, "reader-close", "", Some(&controls::CLOSE), false)
            .w(32.0)
            .label("Close document")
            .help("Esc twice, X or ⌘W")
            .on_press_tx(|a: &mut App, tx| {
                a.close_reader();
                crate::funnel::focus_section(a, tx);
            }),
    )
}

fn find_bar(app: &App, p: &Paint) -> El {
    row()
        .key("reader-find-bar")
        .w_full()
        .px(16.0)
        .py(6.0)
        .gap(8.0)
        .items_center()
        .fill(p.look.raised)
        .child(
            TextInput::new(&app.preview.find_edit)
                .placeholder("Find in document")
                .on_edit(|a: &mut App| &mut a.preview.find_edit)
                .on_key(find_key)
                .build(&p.kit)
                .key("reader-find-input"),
        )
        .child(
            text(if app.preview.found.is_empty() {
                "0 matches".into()
            } else {
                format!(
                    "{} of {}",
                    app.preview.current_match + 1,
                    app.preview.found.len()
                )
            })
            .font(type_::LABEL)
            .color(p.look.muted),
        )
        .child(
            crate::shell::shell_button(p, "reader-find-prev", "Previous", false)
                .on_press_tx(|a: &mut App, tx: &mut Tx| next_match(a, true, tx)),
        )
        .child(
            crate::shell::shell_button(p, "reader-find-next", "Next", false)
                .on_press_tx(|a: &mut App, tx: &mut Tx| next_match(a, false, tx)),
        )
        .child(
            crate::shell::shell_button(p, "reader-find-close", "Done", false)
                .on_press_tx(|a: &mut App, tx: &mut Tx| hide_find(a, tx)),
        )
}
fn goto_bar(app: &App, p: &Paint) -> El {
    let rows = app
        .preview
        .reader
        .document
        .as_ref()
        .map_or(0, |d| d.rows());
    row()
        .key("reader-goto-bar")
        .w_full()
        .px(16.0)
        .py(6.0)
        .gap(8.0)
        .items_center()
        .fill(p.look.raised)
        .child(
            TextInput::new(&app.preview.goto_edit)
                .placeholder("Number, then Return")
                .on_edit(|a: &mut App| &mut a.preview.goto_edit)
                .on_key(goto_key)
                .build(&p.kit)
                .key("reader-goto-input"),
        )
        .child(
            text(format!("of {rows}"))
                .font(type_::LABEL)
                .color(p.look.muted),
        )
        .child(
            crate::shell::shell_button(p, "reader-goto-close", "Done", false)
                .on_press_tx(|a: &mut App, tx: &mut Tx| hide_goto(a, tx)),
        )
}

/// Headings of a Markdown or Word document. The one being read is marked.
fn outline(app: &App, p: &Paint, doc: &Document) -> El {
    let blocks = doc.blocks();
    let reading = blocks
        .iter()
        .take(app.preview.visible_block + 1)
        .rposition(|b| b.heading.is_some());
    let mut list = col()
        .key("reader-outline-rows")
        .w_full()
        .px(8.0)
        .py(12.0)
        .gap(1.0);
    let headings = blocks
        .iter()
        .enumerate()
        .filter_map(|(i, block)| Some((i, block.heading.as_ref()?)))
        .take(500);
    let mut empty = true;
    for (i, heading) in headings {
        empty = false;
        let current = reading == Some(i);
        list = list.child(
            p.kit
                .interactive(
                    el().key(format!("reader-outline-{i}"))
                        .role("button")
                        .label(heading.title.clone())
                        .w_full()
                        .py(4.0)
                        .pl(8.0 + f32::from(heading.level.saturating_sub(1).min(4)) * 12.0)
                        .pr(8.0)
                        .radius(if p.look.era() == crate::theme::Era::Modern {
                            4.0
                        } else {
                            0.0
                        })
                        .fill(if current {
                            p.look.selection
                        } else {
                            rune::Color::TRANSPARENT
                        }),
                )
                .child(
                    text(heading.title.clone())
                        .font(p.look.font(12.0, if heading.level <= 1 { 600.0 } else { 400.0 }))
                        .color(if current {
                            p.look.selection_text
                        } else if heading.level <= 2 {
                            p.look.text
                        } else {
                            p.look.muted
                        })
                        .truncate()
                        .w_full(),
                )
                .on_press_tx(move |_: &mut App, tx: &mut Tx| {
                    tx.scroll_into_view(block_key("reader", i));
                }),
        );
    }
    let panel = col()
        .key("reader-outline")
        .w(248.0)
        .h_full()
        .min_h(0.0)
        .border_sides([0.0, 1.0, 0.0, 0.0], p.look.line);
    if empty {
        return panel.child(message("No headings", p));
    }
    panel
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(list)
}

/// A picture fitted to the window. A click zooms to its pixels, a pinch or the controls zoom
/// further and a drag pans.
fn image_view(app: &App, cx: &mut Cx<App>, p: &Paint, doc: &Document) -> El {
    let Some(image) = doc.thumbnail.as_ref().filter(|i| i.error().is_none()) else {
        return picture_view(p, doc);
    };
    let size = cx.layout_size("reader-stage").unwrap_or([900.0, 640.0]);
    // Decoded pixels are shown at two per point, the density of the displays Crawler runs on.
    let natural = image
        .size()
        .map_or([1.0, 1.0], |[w, h]| [w as f32 / 2.0, h as f32 / 2.0]);
    el().key("reader-stage").w_full().h_full().child(
        Stage::new(
            |a: &mut App| &mut a.preview.image_zoom,
            natural,
            el().pixels(image.pixels()),
        )
        .icons(&controls::ZOOM_IN, &controls::ZOOM_OUT, &controls::FIT)
        .build(&app.preview.image_zoom, &p.kit, size),
    )
}

/// Matches of the open find in row `row`, with each one's index among all matches.
fn row_matches(app: &App, row: usize) -> impl Iterator<Item = (usize, &content::Found)> {
    let found = &app.preview.found;
    let first = found.partition_point(|f| f.block < row);
    found[first..]
        .iter()
        .take_while(move |f| f.block == row)
        .enumerate()
        .map(move |(n, f)| (first + n, f))
}

/// Markdown blocks or text lines as virtual rows: only the rows on screen are built.
fn document_view(app: &App, p: &Paint, document: Arc<Document>, key: &str, reader: bool) -> El {
    let count = document.rows();
    let truncated = document.truncated;
    let budget = document.budget;
    let format = document.format;
    let plain = matches!(document.body, Body::Text(_));
    let wrap = !plain || !reader || app.preview.wrap;
    let scale = if reader {
        app.preview.scale as f32 / 100.0
    } else {
        1.0
    };
    let revision = (
        document.path.clone(),
        document.revision,
        p.look.theme,
        app.preview.scale,
        wrap,
    );
    // The theme's styles are built once per view, not once per row.
    let look = p.look;
    let style = markdown_style(p, scale, reader);
    let syntax = syntax_theme(p);
    let digits = count.max(1).ilog10() as f32 + 1.0;
    let row_prefix = key.to_owned();
    let build_prefix = key.to_owned();
    let mut rows = col()
        .selection_group(true)
        .key(format!("{key}-blocks"))
        .w_full()
        .growth(GrowthSpec::default())
        .pt(if plain {
            8.0
        } else if reader {
            (17.0 * scale).round() * 2.5
        } else {
            16.0
        })
        .pb(16.0)
        .virtual_rows(
            count,
            if !plain {
                70.0
            } else if reader {
                20.0 * scale
            } else {
                16.0
            },
            move |_: &App, i| block_key(&row_prefix, i).into(),
            move |a: &App, i| {
                let row = match &document.body {
                    Body::Text(text) => {
                        text_row(a, text, i, look, syntax, scale, digits, reader, wrap)
                    }
                    Body::Blocks(blocks) => match blocks.get(i) {
                        Some(block) => block_row(a, &document, block, i, &style, scale, reader),
                        None => el(),
                    },
                    Body::Sheet(_) | Body::None => el(),
                };
                row.key(block_key(&build_prefix, i))
            },
        )
        .virtual_rows_version(revision);
    if reader {
        rows = rows.on_visible_rows(|a: &mut App, range| {
            let changed = a.preview.visible_end != range.end
                || (a.preview.outline_open && a.preview.visible_block != range.start);
            a.preview.visible_block = range.start;
            a.preview.visible_end = range.end;
            changed
        });
    }
    let mut body = col()
        .key(format!("{key}-scroll"))
        .w_full()
        .h_full()
        .min_h(0.0);
    body = if wrap {
        body.scroll(ScrollSpec {
            stick_bottom: false,
        })
    } else {
        body.scroll_xy()
    };
    let mut body = body.child(rows);
    if reader {
        body = body.on_magnify(|a: &mut App, event, _tx| {
            a.preview.pinch += event.delta;
            if a.preview.pinch.abs() >= 0.1 {
                let steps = (a.preview.pinch / 0.1) as i16;
                a.preview.pinch -= steps as f32 * 0.1;
                zoom(a, steps);
            }
            true
        });
    }
    // A pane names the cut in its own status line.
    with_notice(body, reader && truncated, budget, format, p)
}

/// One line of code or plain text beside its number.
#[allow(clippy::too_many_arguments)]
fn text_row(
    app: &App,
    text: &content::Text,
    line: usize,
    look: crate::theme::Look,
    syntax: SyntaxTheme,
    scale: f32,
    digits: f32,
    reader: bool,
    wrap: bool,
) -> El {
    let (size, height) = if reader {
        (14.0 * scale, 20.0 * scale)
    } else {
        (12.0, 16.0)
    };
    let source = text.line(line);
    let mut content = rich(
        if source.is_empty() {
            " ".to_owned()
        } else {
            source.to_owned()
        },
        syntax.spans(&text.highlights(line), 0),
    )
    .font(
        TextStyle::new(size, 400.0)
            .family(Family::SystemMono)
            .lh(height),
    )
    .color(look.text)
    .selectable(true);
    if reader && app.preview.find_open {
        content = content.text_highlights(row_matches(app, line).flat_map(|(index, found)| {
            let current = index == app.preview.current_match;
            found.ranges.iter().map(move |range| {
                rune::TextHighlight::new(
                    range.clone(),
                    look.accent.alpha(if current { 0.55 } else { 0.25 }),
                )
                .key(format!("reader-match-{index}"))
            })
        }));
    }
    let number = rune::text((line + 1).to_string())
        .w(digits * size * 0.62 + 28.0)
        .pr(14.0)
        .align(rune::Align::End)
        .font(
            TextStyle::new(size * 0.85, 400.0)
                .family(Family::SystemMono)
                .lh(height),
        )
        .color(look.muted);
    let line = row().w_full().items_start();
    if wrap {
        line.child(number)
            .child(content.wrap().grow(1.0).min_w(0.0).pr(19.0))
    } else {
        // The number stays put while the code scrolls sideways under it.
        line.child(number.sticky_left(0.0).fill(look.raised))
            .child(content.pr(19.0))
    }
}

/// One top-level Markdown block.
fn block_row(
    app: &App,
    document: &Arc<Document>,
    block: &content::Block,
    index: usize,
    style: &MarkdownStyle,
    scale: f32,
    reader: bool,
) -> El {
    let images = document.images.clone();
    let fenced = document.fenced.clone();
    let source_path = document.path.clone();
    let highlights: Vec<_> = if reader && app.preview.find_open {
        row_matches(app, index)
            .map(|(n, found)| {
                (
                    found.ranges.clone(),
                    rune_kit::markdown::Highlight::new()
                        .current_match(n == app.preview.current_match)
                        .key(format!("reader-match-{n}")),
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let content = Markdown::new(block.source.clone())
        .selectable(true)
        .tasks_read_only()
        .highlights(highlights)
        .highlight(move |_, code| {
            fenced
                .get(code.trim_end_matches(['\n', '\r']))
                .cloned()
                .unwrap_or_default()
        })
        .images(move |source| images.get(source).cloned())
        .on_link_tx(move |a: &mut App, destination, tx| {
            link(a, &source_path, destination, reader, tx)
        })
        .render(style)
        .selection_group(false)
        .w_full();
    let max_width = if reader {
        (17.0 * scale).round() * 40.0 - 10.0
    } else {
        f32::MAX
    };
    col()
        .w_full()
        .items_center()
        .pt(if reader { 16.0 * scale } else { 8.0 })
        .px(if reader {
            (17.0 * scale).round() * 2.0
        } else {
            19.0
        })
        .child(content.max_w(max_width).motion(motion::SNAPPY))
}

const SHEET_ROW: f32 = 24.0;
const SHEET_GUTTER: f32 = 56.0;

/// A sheet as a grid that scrolls both ways. The column letters and the first row stay at the
/// top, the row numbers at the left, and only the rows on screen are built.
fn sheet_view(app: &App, p: &Paint, document: Arc<Document>, key: &str, reader: bool) -> El {
    let Body::Sheet(book) = &document.body else {
        return el();
    };
    let sheet = &book.sheet;
    if sheet.rows() == 0 {
        return message("This sheet is empty", p);
    }
    let look = p.look;
    let width = SHEET_GUTTER + sheet.widths.iter().sum::<f32>();
    let mut letters = row().w(width).h(SHEET_ROW).child(
        el().w(SHEET_GUTTER)
            .h_full()
            .sticky_left(0.0)
            .fill(look.raised),
    );
    for (column, column_width) in sheet.widths.iter().enumerate() {
        letters = letters.child(
            row().w(*column_width).h_full().center().child(
                text(column_name(column))
                    .font(look.font(10.0, 500.0))
                    .color(look.muted),
            ),
        );
    }
    let header = col()
        .key(format!("{key}-sheet-header"))
        .w(width)
        .sticky_top(0.0)
        .fill(look.raised)
        .border_sides([0.0, 0.0, 1.0, 0.0], look.line)
        .child(letters)
        .child(sheet_row(app, &document, 0, look, key, reader));
    let revision = (
        document.path.clone(),
        document.revision,
        look.theme,
        book.active,
    );
    let rows_document = document.clone();
    let row_prefix = key.to_owned();
    let build_prefix = key.to_owned();
    let mut rows = col()
        .key(format!("{key}-sheet-rows"))
        .w(width)
        .growth(GrowthSpec::default())
        .virtual_rows(
            sheet.rows() - 1,
            SHEET_ROW,
            move |_: &App, i| block_key(&row_prefix, i + 1).into(),
            move |a: &App, i| sheet_row(a, &rows_document, i + 1, look, &build_prefix, reader),
        )
        .virtual_rows_version(revision);
    if reader {
        rows = rows.on_visible_rows(|a: &mut App, range| {
            let changed = a.preview.visible_end != range.end + 1;
            a.preview.visible_block = range.start + 1;
            a.preview.visible_end = range.end + 1;
            changed
        });
    }
    let grid = col()
        .key(format!("{key}-scroll"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll_xy()
        .child(col().w(width).child(header).child(rows));
    let mut view = col().w_full().h_full().child(grid);
    if reader && book.names.len() > 1 {
        let mut tabs = row().gap(2.0).items_center();
        for (index, name) in book.names.iter().enumerate() {
            tabs = tabs.child(
                controls::button(
                    p,
                    &format!("reader-sheet-tab-{index}"),
                    name,
                    None,
                    index == book.active,
                )
                .on_press_tx(move |a: &mut App, tx: &mut Tx| show_sheet(a, index, tx)),
            );
        }
        view = view.child(
            row()
                .key("reader-sheet-tabs")
                .w_full()
                .h(38.0)
                .px(10.0)
                .items_center()
                .border_sides([1.0, 0.0, 0.0, 0.0], look.line)
                .scroll_x()
                .child(tabs),
        );
    }
    with_notice(
        view,
        reader && document.truncated,
        document.budget,
        document.format,
        p,
    )
}
fn sheet_row(
    app: &App,
    document: &Document,
    index: usize,
    look: crate::theme::Look,
    prefix: &str,
    reader: bool,
) -> El {
    let Body::Sheet(book) = &document.body else {
        return el();
    };
    let sheet = &book.sheet;
    let matched = (reader && app.preview.find_open)
        .then(|| row_matches(app, index).next())
        .flatten()
        .map(|(n, _)| n == app.preview.current_match);
    let fill = match matched {
        Some(true) => look.accent.alpha(0.4),
        Some(false) => look.accent.alpha(0.16),
        None => rune::Color::TRANSPARENT,
    };
    let rule = look.line.alpha(0.5);
    let mut line = row()
        .key(block_key(prefix, index))
        .w_full()
        .h(SHEET_ROW)
        .fill(fill)
        .border_sides([0.0, 0.0, 1.0, 0.0], rule)
        .child(
            row()
                .w(SHEET_GUTTER)
                .h_full()
                .pr(10.0)
                .items_center()
                .justify_end()
                .sticky_left(0.0)
                .fill(look.raised)
                .child(
                    text((index + 1).to_string())
                        .font(look.font(10.0, 400.0))
                        .color(look.muted),
                ),
        );
    // The first row is usually the column titles.
    let font = look.font(12.0, if index == 0 { 600.0 } else { 400.0 });
    for (column, width) in sheet.widths.iter().enumerate() {
        let value = sheet.cell(index, column);
        let mut cell = row()
            .w(*width)
            .h_full()
            .px(8.0)
            .items_center()
            .border_sides([0.0, 1.0, 0.0, 0.0], rule);
        if !value.is_empty() {
            let number = value.parse::<f64>().is_ok();
            cell = cell.child(
                text(value.to_owned())
                    .font(font.clone())
                    .color(look.text)
                    .truncate()
                    .w_full()
                    .align(if number {
                        rune::Align::End
                    } else {
                        rune::Align::Start
                    })
                    .selectable(true),
            );
        }
        line = line.child(cell);
    }
    line
}
/// A, B … Z, AA, AB …
fn column_name(mut column: usize) -> String {
    let mut name = Vec::new();
    loop {
        name.push(b'A' + (column % 26) as u8);
        if column < 26 {
            break;
        }
        column = column / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}
fn show_sheet(app: &mut App, sheet: usize, tx: &mut Tx) {
    let Some(path) = app.preview.reader.path.clone() else {
        return;
    };
    app.preview
        .reader
        .request(&path, sheet, Budget::Reader, &app.preview.cache);
    app.preview.find_task.cancel();
    app.preview.found.clear();
    app.preview.visible_block = 0;
    app.preview.visible_end = 0;
    tx.scroll_to_instant("reader-scroll", 0.0, 0.0);
}

fn summary(doc: &Document) -> String {
    match &doc.body {
        Body::Blocks(_) if doc.format == Format::Word => "Word document".into(),
        Body::Blocks(_) => "Markdown".into(),
        Body::Text(text) => format!(
            "{} lines · {}",
            text.lines(),
            content::extension(&doc.path).to_uppercase()
        ),
        Body::Sheet(book) => format!(
            "{} rows · {} columns",
            book.sheet.rows(),
            book.sheet.columns
        ),
        Body::None => String::new(),
    }
}
fn with_notice(body: El, truncated: bool, budget: Budget, format: Format, p: &Paint) -> El {
    if truncated {
        col()
            .w_full()
            .h_full()
            .child(body.grow(1.0).min_h(0.0))
            .child(
                text(budget.notice(format))
                    .font(type_::LABEL)
                    .color(p.look.muted)
                    .wrap()
                    .px(8.0)
                    .py(8.0),
            )
    } else {
        body
    }
}
fn reading_font(theme: Theme, size: f32) -> TextStyle {
    TextStyle::new(size, 400.0)
        .family(match theme {
            Theme::System7 => Family::Named(".New York"),
            Theme::WindowsRetro => Family::Named("Verdana"),
            Theme::LinuxRetro => Family::Named("Charter"),
            Theme::Terminal => Family::Named("IBM Plex Mono"),
            _ => Family::Sans,
        })
        .lh(size * 1.6)
}
fn markdown_style(p: &Paint, scale: f32, reader: bool) -> MarkdownStyle {
    let size = if reader { (17.0 * scale).round() } else { 13.0 };
    let mut style = MarkdownStyle::from_kit(&p.kit)
        .text(p.look.text)
        .muted(p.look.muted)
        .link(crate::theme::rgb(if p.look.dark {
            0x419cff
        } else {
            0x0068da
        }))
        .syntax_theme(syntax_theme(p))
        .body_font(reading_font(p.look.theme, size).lh(if reader {
            size * 1.62
        } else {
            size * 1.4
        }))
        .code_block_font(
            TextStyle::new(size * 0.88, 400.0)
                .family(Family::SystemMono)
                .lh(size * 1.45),
        )
        .code_block(p.look.line.alpha(0.32), rune::Color::TRANSPARENT)
        .code_block_padding((size * 0.9).round(), (size * 0.9).round())
        .inline_code_bg(p.look.line.alpha(0.45))
        .text_width(if reader { size * 40.0 } else { 400.0 });
    for level in 1..=6 {
        style = style.heading_font(
            level,
            TextStyle {
                weight: 650.0,
                ..reading_font(
                    p.look.theme,
                    (size
                        * match level {
                            1 => 1.9,
                            2 => 1.5,
                            3 => 1.25,
                            _ => 1.1,
                        })
                    .round(),
                )
                .lh((size
                    * match level {
                        1 => 1.9,
                        2 => 1.5,
                        3 => 1.25,
                        _ => 1.1,
                    })
                .round()
                    * 1.2)
            },
        );
    }
    style
}

fn syntax_theme(p: &Paint) -> SyntaxTheme {
    use rune_kit::syntax::Syntax;
    let color = |light, dark| crate::theme::rgb(if p.look.dark { dark } else { light });
    SyntaxTheme::from_kit(&p.kit)
        .with(Syntax::Comment, color(0x78836b, 0x9ba891))
        .with(Syntax::String, color(0x9b3434, 0xeda4a4))
        .with(Syntax::Regex, color(0x9b3434, 0xeda4a4))
        .with(Syntax::Keyword, color(0x9a328d, 0xe8a4dc))
        .with(Syntax::Number, color(0x7653b5, 0xbfa5f0))
        .with(Syntax::Type, color(0x236e80, 0x85c9d7))
        .with(Syntax::Function, color(0x2462a6, 0x8abaf0))
        .with(Syntax::Property, color(0x8b6518, 0xdec185))
        .with(Syntax::Attribute, color(0x8b6518, 0xdec185))
        .with(Syntax::Punctuation, p.look.muted)
        .with(Syntax::Operator, p.look.muted)
        .with(Syntax::Variable, p.look.text)
        .with(Syntax::Parameter, p.look.text)
}
fn block_key(prefix: &str, index: usize) -> String {
    format!("{prefix}-block-{index}")
}
fn message(value: &str, p: &Paint) -> El {
    col().w_full().h_full().center().px(24.0).py(24.0).child(
        text(value.to_owned())
            .font(type_::BODY)
            .color(p.look.muted)
            .wrap(),
    )
}

fn link(app: &mut App, base: &Path, destination: &str, reader: bool, tx: &mut Tx) {
    let fragment = destination
        .split_once('#')
        .and_then(|(_, value)| content::percent_decode(value));
    if let Some(anchor) = destination
        .strip_prefix('#')
        .and_then(content::percent_decode)
    {
        if reader {
            app.preview.pending_position = Some(Position::Anchor(anchor));
            if let Some(w) = &app.waker {
                w();
            }
        }
        return;
    }
    if let Some(path) = content::local_path(base, destination) {
        if Format::of(&path).readable() {
            if app.preview.space_window
                && let Some(parent) = &app.preview.space_parent
            {
                parent.update(move |a, tx| {
                    navigate(
                        a,
                        &path,
                        fragment.map(Position::Anchor).unwrap_or(Position::Block(0)),
                        tx,
                    )
                });
                tx.close_window();
                return;
            }
            if let Some(current) = app.preview.reader.path.clone() {
                app.preview
                    .history
                    .push((current, app.preview.visible_block));
            }
            navigate(
                app,
                &path,
                fragment.map(Position::Anchor).unwrap_or(Position::Block(0)),
                tx,
            );
        } else {
            crate::opening::open_path(app, &path, tx);
        }
    } else if destination.starts_with("https://")
        || destination.starts_with("http://")
        || destination.starts_with("mailto:")
    {
        let _ = tx.open_url(destination);
    }
}
fn back(app: &mut App, tx: &mut Tx) {
    if let Some((path, block)) = app.preview.history.pop() {
        navigate(app, &path, Position::Block(block), tx);
    }
}
fn zoom(app: &mut App, steps: i16) {
    let scale = if steps == 0 {
        100
    } else {
        (app.preview.scale as i16 + steps * 10).clamp(70, 200) as u16
    };
    if scale != app.preview.scale {
        app.preview.scale = scale;
        if app.preview.find_open
            && let Some(found) = app.preview.found.get(app.preview.current_match)
        {
            app.preview.pending_position = Some(Position::Block(found.block));
        }
        let _ = app
            .prefs
            .set("markdownReaderScale", &(scale as f64 / 100.0));
    }
}
fn show_find(app: &mut App, tx: &mut Tx) {
    app.preview.find_open = true;
    app.preview.goto_open = false;
    app.reader_escape_at = None;
    tx.focus_when_shown("input", false);
}
fn hide_find(app: &mut App, tx: &mut Tx) {
    app.preview.find_open = false;
    app.preview.find_task.cancel();
    app.preview.found.clear();
    tx.focus(READER, false);
}
fn show_goto(app: &mut App, tx: &mut Tx) {
    app.preview.goto_open = true;
    app.preview.find_open = false;
    app.preview.goto_edit = TextEdit::new("");
    app.reader_escape_at = None;
    tx.focus_when_shown("input", false);
}
fn hide_goto(app: &mut App, tx: &mut Tx) {
    app.preview.goto_open = false;
    tx.focus(READER, false);
}
/// Scrolls to the typed line or sheet row, counted from one.
fn goto_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    match event.key {
        Key::Escape => hide_goto(app, tx),
        Key::Enter => {
            let rows = app
                .preview
                .reader
                .document
                .as_ref()
                .map_or(0, |d| d.rows());
            if let Ok(number) = app.preview.goto_edit.text.trim().parse::<usize>()
                && rows > 0
            {
                tx.scroll_into_view(block_key("reader", number.clamp(1, rows) - 1));
            }
            hide_goto(app, tx);
        }
        _ => return false,
    }
    true
}
fn toggle_outline(app: &mut App) {
    app.preview.outline_open = !app.preview.outline_open;
    let _ = app.prefs.set("readerOutline", &app.preview.outline_open);
}
fn toggle_wrap(app: &mut App) {
    app.preview.wrap = !app.preview.wrap;
    let _ = app.prefs.set("readerWrapLines", &app.preview.wrap);
}
fn reveal_match(app: &App, tx: &mut Tx) {
    if let Some(found) = app.preview.found.get(app.preview.current_match) {
        tx.scroll_into_view(block_key("reader", found.block));
        tx.scroll_into_view(format!("reader-match-{}", app.preview.current_match));
    }
}
fn next_match(app: &mut App, previous: bool, tx: &mut Tx) {
    let count = app.preview.found.len();
    if count == 0 {
        return;
    }
    app.preview.current_match =
        (app.preview.current_match + if previous { count - 1 } else { 1 }) % count;
    reveal_match(app, tx);
}
fn find_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    match event.key {
        Key::Escape => hide_find(app, tx),
        Key::Enter => next_match(app, event.mods.shift, tx),
        _ => return false,
    }
    true
}
pub fn reader_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if !matches!(event.key, Key::Escape) {
        app.reader_escape_at = None;
    }
    if matches!(event.key, Key::Escape) && tx.key_repeat() {
        return true;
    }
    let body = app.preview.reader.document.as_ref().map(|d| &d.body);
    let (blocks, plain, sheet, picture) = (
        matches!(body, Some(Body::Blocks(_))),
        matches!(body, Some(Body::Text(_))),
        matches!(body, Some(Body::Sheet(_))),
        matches!(body, Some(Body::None)),
    );
    let step = match &event.key {
        Key::Command('+') | Key::Command('=') => Some(1),
        Key::Command('-') => Some(-1),
        Key::Command('0') => Some(0),
        _ => None,
    };
    if let Some(step) = step {
        if picture {
            match step {
                0 => app.preview.image_zoom.reset(tx),
                1 => app.preview.image_zoom.zoom_by(1.25, tx),
                _ => app.preview.image_zoom.zoom_by(0.8, tx),
            }
        } else if blocks || plain {
            zoom(app, step);
        }
        return true;
    }
    match &event.key {
        Key::Command('f') if blocks || plain || sheet => show_find(app, tx),
        Key::Command('g') => next_match(app, event.mods.shift, tx),
        Key::Command('l') if plain || sheet => show_goto(app, tx),
        Key::Command('[') => back(app, tx),
        Key::Escape if app.preview.find_open => hide_find(app, tx),
        Key::Escape if app.preview.goto_open => hide_goto(app, tx),
        // Letters belong to an open text field before they are commands.
        Key::Text(_) | Key::Backspace | Key::Delete if typing(app) => return false,
        Key::Text(value) if value.eq_ignore_ascii_case("o") && blocks => toggle_outline(app),
        Key::Text(value) if value.eq_ignore_ascii_case("w") && plain => toggle_wrap(app),
        _ => return false,
    }
    true
}
pub fn reader_closed(app: &mut App) {
    app.preview.reader.cancel();
    app.preview.reader = Slot::default();
    if let Some(w) = &app.waker {
        app.preview.reader.task.set_waker(w.clone());
    }
    app.preview.find_task.cancel();
    app.preview.found.clear();
    app.preview.find_open = false;
    app.preview.goto_open = false;
    app.preview.image_zoom = StageZoom::default();
    app.preview.history.clear();
    app.preview.pending_position = None;
}

/// Register a retained secondary window. Its own state gives it independent scrolling and
/// decoding, while the shared flag suspends the workspace's player until Space closes.
pub fn with_space_window(mut host: rune::App<App>) -> rune::App<App> {
    let main_handle = host.handle();
    let open = Arc::new(AtomicBool::new(false));
    let spec = AppSpec::<App> {
        fonts: fonts(),
        families: ("System Font", "ui-monospace"),
        setup: Some(space_setup),
        poll: Some(space_poll),
        on_key: Some(space_key),
        ..AppSpec::new("Preview", (640.0, 720.0), App::preview_window, space_view)
    };
    // A hidden window still owns a renderer. Create Space only when it is opened.
    let space_host = rune::App::new(spec)
        .chrome(rune::Chrome {
            full_size_content: false,
            ..rune::Chrome::DEFAULT
        })
        .windowless()
        .release_gpu_when_closed()
        .on_close(space_closed)
        .menu_item(
            rune::MenuItem::new("Quit Crawler", space_quit)
                .id("crawler-quit")
                .key_equivalent("cmd+q".parse().expect("valid quit shortcut")),
        );
    let (space_app, space_window) = host.window(
        WindowSpec::from_app("space-preview", space_host).options(WindowOptions {
            visible: false,
            min_size: Some((420.0, 360.0)),
            ..Default::default()
        }),
    );
    let shared = open.clone();
    let parent = main_handle.clone();
    space_app.update(move |a, _| {
        a.preview.space_window = true;
        a.preview.space_open = shared;
        a.preview.space_parent = Some(parent);
    });
    let host_handle = main_handle.clone();
    main_handle.update(move |a, _| {
        a.host = Some(host_handle);
        a.preview.space = Some((space_app, space_window));
        a.preview.space_open = open;
    });
    host
}
pub fn toggle_space(app: &mut App, path: &Path, _tx: &mut Tx) {
    let Some((handle, window)) = &app.preview.space else {
        app.toast("The preview window has not been initialized.");
        return;
    };
    if app.preview.space_open.swap(true, Ordering::Relaxed) {
        window.close();
        return;
    }
    app.preview.pane.borrow().pause();
    if Format::of(path) == Format::Pdf {
        crate::pdf::suspend_preview(app);
    }
    let target = path.to_owned();
    let theme = app.theme_preview.unwrap_or(app.theme);
    let shared = {
        let pane = app.preview.pane.borrow();
        pane.document
            .as_ref()
            .filter(|d| d.path == path)
            .map(|d| (d.clone(), pane.player.clone(), pane.muted))
    };
    handle.update(move |a, tx| {
        tx.set_title(
            target
                .file_name()
                .unwrap_or(target.as_os_str())
                .to_string_lossy()
                .into_owned(),
        );
        a.preview.space_path = Some(target.clone());
        a.theme = theme;
        let mut pane = a.preview.pane.borrow_mut();
        pane.cancel();
        pane.path = None;
        pane.document = None;
        pane.error = None;
        if let Some((document, player, muted)) = shared {
            pane.path = Some(target);
            pane.document = Some(document);
            pane.player = player;
            pane.muted = muted;
            if let Some(player) = &pane.player
                && let Some(waker) = &a.waker
            {
                player.set_waker(waker.clone());
            }
        }
    });
    window.open();
}
fn space_setup(app: &mut App, waker: Waker) {
    app.waker = Some(waker.clone());
    setup(app, &waker);
    crate::pdf::setup(app, &waker);
}
fn space_poll(app: &mut App, clock: f64, tx: &mut Tx) -> bool {
    app.clock = clock;
    poll(app, tx) | crate::pdf::poll(app, tx)
}
fn space_view(app: &App, cx: &mut Cx<App>) -> El {
    let look = app.look();
    let p = Paint {
        kit: look.kit(),
        look,
    };
    if let Some(path) = &app.preview.space_path {
        pane(app, cx, &p, path, "space-preview-pane")
    } else {
        message("Select a file to preview", &p)
    }
}
fn space_quit(app: &mut App, _tx: &mut Tx) {
    if let Some(parent) = &app.preview.space_parent {
        parent.open();
        parent.update(crate::app::request_quit);
    }
}
fn space_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if event.key == Key::Command('q') {
        space_quit(app, tx);
        return true;
    }
    if matches!(event.key, Key::Escape | Key::Command('w'))
        || matches!(&event.key, Key::Text(t) if t == " ")
    {
        app.preview.space_open.store(false, Ordering::Relaxed);
        app.preview.pane.borrow_mut().cancel();
        tx.close_window();
        true
    } else {
        false
    }
}
fn byte_size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let (divisor, unit) = if bytes < 1_000_000 {
        (1000.0, "KB")
    } else if bytes < 1_000_000_000 {
        (1_000_000.0, "MB")
    } else {
        (1_000_000_000.0, "GB")
    };
    format!("{:.1} {unit}", bytes as f64 / divisor)
}
fn date(seconds: u64) -> String {
    // libc's thread-safe formatter avoids querying file metadata in the view.
    let seconds = seconds as libc::time_t;
    let mut broken = std::mem::MaybeUninit::<libc::tm>::uninit();
    unsafe {
        if libc::localtime_r(&seconds, broken.as_mut_ptr()).is_null() {
            return String::new();
        }
        let value = broken.assume_init();
        format!(
            "{:04}-{:02}-{:02}",
            value.tm_year + 1900,
            value.tm_mon + 1,
            value.tm_mday
        )
    }
}

fn space_closed(app: &mut App, _tx: &mut Tx) {
    app.preview.space_open.store(false, Ordering::Relaxed);
    app.preview.pane.borrow_mut().cancel();
    app.preview.pane.borrow_mut().document = None;
    app.preview.pane.borrow_mut().path = None;
    app.preview.space_path = None;
    crate::pdf::release_previews(app);
    if let Some(parent) = &app.preview.space_parent {
        parent.update(|a, _| {
            let pane = a.preview.pane.borrow();
            pane.pause();
            if let Some(player) = &pane.player
                && let Some(waker) = &a.waker
            {
                player.set_waker(waker.clone());
            }
        });
    }
}

pub fn main_closed(app: &mut App, tx: &mut Tx) {
    app.preview.pane.borrow_mut().cancel();
    app.preview.reader.cancel();
    app.preview.find_task.cancel();
    if let Some((_, window)) = &app.preview.space {
        window.close();
    }
    crate::app::on_close(app, tx);
}

/// Readiness reflects completed background reads, not compositor presentation.
pub fn report(app: &App) -> serde_json::Value {
    fn state(slot: &Slot) -> &'static str {
        if slot.error.is_some() {
            "failed"
        } else if slot.document.is_some() {
            "ready"
        } else if slot.task.is_pending() {
            "loading"
        } else {
            "closed"
        }
    }
    let pane = app.preview.pane.borrow();
    let reader = &app.preview.reader;
    let path = if app.preview.space_window {
        app.preview.space_path.clone()
    } else if app.reader == Reader::None
        && matches!(app.section, Section::Search | Section::Folders)
    {
        crate::funnel::focused_path(app)
    } else {
        None
    };
    let preview_state = if let Some(path) = path.as_deref().filter(|p| Format::of(p) == Format::Pdf)
    {
        if crate::pdf::is_ready(app, path) {
            "ready"
        } else {
            "loading"
        }
    } else if path.as_deref() == pane.path.as_deref() {
        state(&pane)
    } else {
        "closed"
    };
    serde_json::json!({
        "file_search_preview_state":preview_state, "file_search_preview_path":path,
        "preview_load_ms":pane.load_ms, "preview_error":pane.error,
        "video_seconds":pane.player.as_ref().map(|p| p.time()),
        "markdown_reader_visible":app.reader == Reader::Document,
        "reader_format":reader.document.as_ref().map(|d| format!("{:?}", d.format)),
        "reader_rows":reader.document.as_ref().map(|d| d.rows()),
        "reader_truncated":reader.document.as_ref().map(|d| d.truncated),
        "markdown_reader_path":reader.path, "markdown_reader_state":state(reader),
        "markdown_reader_load_ms":reader.load_ms, "markdown_reader_error":reader.error,
        "markdown_reader_scale":app.preview.scale, "markdown_reader_history":app.preview.history,
        "space_open":app.preview.space_open.load(Ordering::Relaxed)
    })
}
