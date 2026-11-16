//! Shared, bounded file previews, the Space window, and the Markdown/Word reader.
mod content;
mod native;
mod word;

use crate::app::{App, Paint, Reader, Section};
use crate::theme::Theme;
use content::{Budget, Document, Format, Request};
use rune::prefs::Prefs;
use rune::tok::{Family, TextStyle, motion, type_};
use rune::{
    AppHandle, AppSpec, Cx, El, GrowthSpec, ImageCache, Key, KeyEvent, Latest, Player, ScrollSpec,
    SurfaceFit, TextEdit, Transition, Tx, Waker, WindowHandle, WindowOptions, WindowSpec, col, el,
    rich, row, text,
};
use rune_kit::{Markdown, MarkdownStyle, TextInput, syntax::SyntaxTheme};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

type EditorOpener = fn(&mut App, &Path, &mut Tx);
type ReaderDisplayed = fn(&mut App, &Path);

#[derive(Default)]
struct Slot {
    path: Option<PathBuf>,
    task: Latest<Result<Arc<Document>, String>>,
    generation: Arc<AtomicU64>,
    document: Option<Arc<Document>>,
    error: Option<String>,
    player: Option<Player>,
    started: bool,
    playing_requested: Cell<bool>,
    muted: bool,
    displayed_second: u64,
}
impl Slot {
    fn request(&mut self, path: &Path, budget: Budget, cache: &Option<Arc<ImageCache>>) {
        if self.path.as_deref() == Some(path) {
            return;
        }
        self.cancel();
        self.path = Some(path.to_owned());
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
        match result {
            Ok(document) => {
                if document.format == Format::Video {
                    self.player = Some(Player::file(&document.path));
                }
                self.document = Some(document);
            }
            Err(error) => self.error = Some(error),
        }
        true
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
    pending_position: Option<Position>,
    find_open: bool,
    find_edit: TextEdit,
    find_task: Latest<Vec<content::Found>>,
    found: Vec<content::Found>,
    current_match: usize,
    space: Option<(AppHandle<App>, WindowHandle)>,
    space_parent: Option<AppHandle<App>>,
    space_open: Arc<AtomicBool>,
    space_window: bool,
    space_path: Option<PathBuf>,
    editor_opener: Option<EditorOpener>,
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
            editor_opener: None,
            reader_displayed: None,
            did_display: false,
        }
    }
}
enum Position {
    Block(usize),
    Anchor(String),
}

pub fn setup(app: &mut App, waker: &Waker) {
    app.preview.cache = Some(Arc::new(ImageCache::new(waker.clone())));
    app.preview.pane.borrow().task.set_waker(waker.clone());
    app.preview.reader.task.set_waker(waker.clone());
    app.preview.find_task.set_waker(waker.clone());
}

/// Connect the opening module's editor/default-app policy without duplicating app discovery.
pub fn set_editor_opener(app: &mut App, opener: EditorOpener) {
    app.preview.editor_opener = Some(opener);
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
        let landed = app.preview.pane.borrow_mut().take();
        changed |= landed;
        if landed
            && let Some(player) = &app.preview.pane.borrow().player
            && let Some(waker) = &app.waker
        {
            player.set_waker(waker.clone());
        }
        let mut pane = app.preview.pane.borrow_mut();
        if let Some(player) = &pane.player
            && matches!(player.state(), rune::PlayerState::Playing)
        {
            let second = player.time().max(0.0) as u64;
            tx.wake_after(std::time::Duration::from_millis(500));
            if second != pane.displayed_second {
                pane.displayed_second = second;
                changed = true;
            }
        }
    } else {
        let mut pane = app.preview.pane.borrow_mut();
        pane.pause();
        if pane.task.is_pending() {
            pane.cancel();
            pane.path = None;
        }
    }
    if app.reader == Reader::Markdown {
        let loaded = app.preview.reader.take();
        changed |= loaded;
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
                            d.blocks
                                .iter()
                                .position(|b| b.heading.as_deref() == Some(anchor.as_str()))
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
    if Format::of(path) == Format::Pdf {
        return crate::pdf::pane(app, cx, p, path, key);
    }
    let suspended = !app.preview.space_window
        && (app.reader != Reader::None || app.preview.space_open.load(Ordering::Relaxed));
    let mut panel = col()
        .key(key.to_owned())
        .w_full()
        .h_full()
        .min_w(0.0)
        .fill(p.look.background)
        .child(crate::shell::pane_title(
            p,
            &format!("{key}-title"),
            "Preview",
            None,
            false,
        ));
    if suspended {
        return panel.child(message("Preview is open in another window", p));
    }
    let mut slot = app.preview.pane.borrow_mut();
    slot.request(path, Budget::Preview, &app.preview.cache);
    if let Some(error) = &slot.error {
        return panel.child(message(error, p));
    }
    let Some(doc) = slot.document.clone() else {
        return panel.child(message("Loading preview…", p));
    };
    let content = match doc.format {
        Format::Markdown | Format::Text => document_view(app, p, doc.clone(), key, false),
        Format::Video => video_view(p, &slot, key),
        Format::Image | Format::Word | Format::Thumbnail => picture_view(p, &doc),
        Format::Pdf => el(),
    };
    let modified = doc
        .modified
        .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|v| format!(" · Modified {}", date(v.as_secs())))
        .unwrap_or_default();
    panel = panel
        .child(
            content
                .key(format!("{key}-content-{}", path.display()))
                .grow(1.0)
                .min_h(0.0)
                .transition(Transition::opacity())
                .motion(motion::SNAPPY),
        )
        .child(
            col()
                .w_full()
                .px(16.0)
                .py(12.0)
                .gap(5.0)
                .border_sides([1.0, 0.0, 0.0, 0.0], p.look.line)
                .child(
                    text(doc.name())
                        .font(type_::BODY)
                        .color(p.look.text)
                        .truncate(),
                )
                .child(
                    text(format!(
                        "{}{}",
                        doc.bytes
                            .map(byte_size)
                            .unwrap_or_else(|| "Size unknown".into()),
                        modified
                    ))
                    .font(type_::LABEL)
                    .color(p.look.muted)
                    .truncate(),
                ),
        );
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
fn video_view(p: &Paint, slot: &Slot, key: &str) -> El {
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
            .on_press(toggle_play),
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
                .on_drag(|a: &mut App, drag, _| {
                    if drag.phase == rune::Phase::Up
                        && !drag.cancelled()
                        && let Some(player) = &a.preview.pane.borrow().player
                        && let Some(duration) = player.duration()
                    {
                        player
                            .seek(f64::from((drag.x / drag.w.max(1.0)).clamp(0.0, 1.0)) * duration);
                    }
                })
                .on_key(|a: &mut App, event, _| {
                    if let Some(player) = &a.preview.pane.borrow().player {
                        match event.key {
                            Key::Left => player.seek(player.time() - 5.0),
                            Key::Right => player.seek(player.time() + 5.0),
                            Key::Home => player.seek(0.0),
                            Key::End => player.seek(player.duration().unwrap_or(0.0)),
                            _ => return false,
                        }
                        true
                    } else {
                        false
                    }
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
            .on_press(|a: &mut App| {
                let mut slot = a.preview.pane.borrow_mut();
                slot.muted = !slot.muted;
                if let Some(player) = &slot.player {
                    player.set_muted(slot.muted || !slot.started);
                }
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
fn toggle_play(app: &mut App) {
    let mut slot = app.preview.pane.borrow_mut();
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

/// Markdown and Word share one reader, with workspace state retained underneath.
pub fn open_reader(app: &mut App, path: &Path, tx: &mut Tx) -> Result<(), String> {
    if !matches!(Format::of(path), Format::Markdown | Format::Word) {
        return Err("This file does not open in the Markdown reader.".into());
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
        .request(path, Budget::Reader, &app.preview.cache);
    // A complete pane read already contains the whole document and can be shared immediately.
    if let Some(doc) = app
        .preview
        .pane
        .borrow()
        .document
        .as_ref()
        .filter(|d| d.path == path && !d.truncated && d.format == Format::Markdown)
    {
        app.preview.reader.cancel();
        app.preview.reader.document = Some(doc.clone());
    }
    app.preview.find_task.cancel();
    app.preview.found.clear();
    app.preview.pending_position = Some(position);
    app.preview.visible_block = 0;
    app.preview.did_display = false;
    app.reader = Reader::Markdown;
    app.reader_escape_at = None;
    tx.focus("markdown-reader", false);
}

pub fn reader_view(app: &App, _cx: &mut Cx<App>, p: &Paint, _layer: &mut Vec<El>) -> El {
    let path = app.preview.reader.path.as_deref().unwrap_or(Path::new(""));
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned();
    let copied = name.clone();
    let word = Format::of(path) == Format::Word;
    let header = row()
        .key("markdown-reader-header")
        .w_full()
        .h(48.0)
        .px(12.0)
        .gap(8.0)
        .items_center()
        .fill(p.look.raised)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.look.line)
        .child(
            crate::shell::shell_button(p, "markdown-back", "‹", false)
                .on_press_tx(|a: &mut App, tx: &mut Tx| back(a, tx)),
        )
        .child(
            text(name)
                .font(type_::BODY)
                .color(p.look.text)
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(
            crate::shell::shell_button(p, "markdown-copy-name", "Copy name", false)
                .on_press_tx(move |_: &mut App, tx: &mut Tx| tx.copy(copied.clone())),
        )
        .child(
            crate::shell::shell_button(p, "markdown-zoom-out", "−", false)
                .on_press(|a: &mut App| zoom(a, -1)),
        )
        .child(
            crate::shell::shell_button(
                p,
                "markdown-zoom-reset",
                &format!("{}%", app.preview.scale),
                false,
            )
            .on_press(|a: &mut App| zoom(a, 0)),
        )
        .child(
            crate::shell::shell_button(p, "markdown-zoom-in", "+", false)
                .on_press(|a: &mut App| zoom(a, 1)),
        )
        .child(
            crate::shell::shell_button(p, "markdown-find", "Find", false)
                .on_press_tx(|a: &mut App, tx: &mut Tx| show_find(a, tx)),
        )
        .child(
            crate::shell::shell_button(
                p,
                "markdown-edit",
                if word {
                    "Open in app"
                } else {
                    "Open in editor"
                },
                false,
            )
            .on_press_tx(|a: &mut App, tx: &mut Tx| open_external(a, tx)),
        )
        .child(
            crate::shell::shell_button(p, "markdown-close", "×", false).on_press_tx(
                |a: &mut App, tx| {
                    a.close_reader();
                    crate::funnel::focus_section(a, tx);
                },
            ),
        );
    let mut view = col()
        .key("markdown-reader")
        .focusable(true)
        .w_full()
        .h_full()
        .fill(p.look.background)
        .child(header);
    if app.preview.find_open {
        view = view.child(
            row()
                .key("markdown-find-bar")
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
                        .key("markdown-find-input"),
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
                    crate::shell::shell_button(p, "markdown-find-prev", "Previous", false)
                        .on_press_tx(|a: &mut App, tx: &mut Tx| next_match(a, true, tx)),
                )
                .child(
                    crate::shell::shell_button(p, "markdown-find-next", "Next", false)
                        .on_press_tx(|a: &mut App, tx: &mut Tx| next_match(a, false, tx)),
                )
                .child(
                    crate::shell::shell_button(p, "markdown-find-close", "Done", false)
                        .on_press_tx(|a: &mut App, tx: &mut Tx| hide_find(a, tx)),
                ),
        );
    }
    let body = if let Some(doc) = &app.preview.reader.document {
        document_view(app, p, doc.clone(), "reader", true)
    } else if let Some(error) = &app.preview.reader.error {
        message(error, p)
    } else {
        message("Loading document…", p)
    };
    view.child(body.grow(1.0).min_h(0.0))
}

fn document_view(app: &App, p: &Paint, document: Arc<Document>, key: &str, reader: bool) -> El {
    let count = document.blocks.len();
    let truncated = document.truncated;
    let budget = document.budget;
    let prefix = key.to_owned();
    let build_prefix = prefix.clone();
    let row_prefix = prefix.clone();
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
    );
    let mut rows = col()
        .selection_group(true)
        .key(format!("{key}-blocks"))
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            count,
            if document.format == Format::Text {
                20.0
            } else {
                70.0
            },
            move |_: &App, i| block_key(&row_prefix, i).into(),
            move |a: &App, i| {
                let look = a.look();
                let paint = Paint {
                    kit: look.kit(),
                    look,
                };
                let Some(block) = document.blocks.get(i) else {
                    return el();
                };
                let style = markdown_style(&paint, scale, reader);
                let content = if block.code {
                    rich(
                        if block.source.is_empty() {
                            " ".to_owned()
                        } else {
                            block.source.clone()
                        },
                        SyntaxTheme::from_kit(&paint.kit).spans(&block.highlights, 0),
                    )
                    .font(
                        TextStyle::new(if reader { 14.0 * scale } else { 12.0 }, 400.0)
                            .family(Family::SystemMono),
                    )
                    .color(look.text)
                    .wrap()
                    .selectable(true)
                    .w_full()
                } else {
                    let images = document.images.clone();
                    let fenced = document.fenced.clone();
                    let source_path = document.path.clone();
                    let highlights: Vec<_> = if reader && a.preview.find_open {
                        a.preview
                            .found
                            .iter()
                            .enumerate()
                            .filter(|(_, f)| f.block == i)
                            .map(|(index, f)| {
                                (
                                    f.ranges.clone(),
                                    rune_kit::markdown::Highlight::new()
                                        .current_match(index == a.preview.current_match)
                                        .key(format!("reader-match-{index}")),
                                )
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    Markdown::new(block.source.clone())
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
                        .render(&style)
                        .selection_group(false)
                        .w_full()
                };
                let max_width = if reader { 70.0 * 8.1 * scale } else { f32::MAX };
                col()
                    .w_full()
                    .items_center()
                    .py(if block.code { 0.0 } else { 8.0 })
                    .px(if reader { 28.0 } else { 14.0 })
                    .child(content.max_w(max_width).motion(motion::SNAPPY))
                    .key(block_key(&build_prefix, i))
            },
        )
        .virtual_rows_version(revision);
    if reader {
        rows = rows.on_visible_rows(|a: &mut App, range| {
            a.preview.visible_block = range.start;
            false
        });
    }
    let mut body = col()
        .key(format!("{key}-scroll"))
        .w_full()
        .h_full()
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(rows);
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
    with_notice(body, truncated, budget, p)
}
fn with_notice(body: El, truncated: bool, budget: Budget, p: &Paint) -> El {
    if truncated {
        col()
            .w_full()
            .h_full()
            .child(body.grow(1.0).min_h(0.0))
            .child(
                text(budget.notice())
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
            Theme::System7 => Family::Named("New York"),
            Theme::WindowsRetro => Family::Named("Verdana"),
            Theme::LinuxRetro => Family::Named("Charter"),
            Theme::Terminal => Family::Named("IBM Plex Mono"),
            _ => Family::Sans,
        })
        .lh(size * 1.6)
}
fn markdown_style(p: &Paint, scale: f32, reader: bool) -> MarkdownStyle {
    let size = if reader { 16.0 * scale } else { 13.0 };
    let mut style = MarkdownStyle::from_kit(&p.kit)
        .text(p.look.text)
        .muted(p.look.muted)
        .link(p.look.accent)
        .body_font(reading_font(p.look.theme, size))
        .code_block_font(
            TextStyle::new(size * 0.88, 400.0)
                .family(Family::SystemMono)
                .lh(size * 1.45),
        )
        .code_block(p.look.raised, p.look.line)
        .text_width(if reader { size * 35.4 } else { 400.0 });
    for level in 1..=6 {
        style = style.heading_font(
            level,
            TextStyle {
                weight: 650.0,
                ..reading_font(
                    p.look.theme,
                    size * match level {
                        1 => 1.9,
                        2 => 1.5,
                        3 => 1.25,
                        _ => 1.05,
                    },
                )
            },
        );
    }
    style
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
        if matches!(Format::of(&path), Format::Markdown | Format::Word) {
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
    app.reader_escape_at = None;
    tx.focus_when_shown("input", false);
}
fn hide_find(app: &mut App, tx: &mut Tx) {
    app.preview.find_open = false;
    app.preview.find_task.cancel();
    app.preview.found.clear();
    tx.focus("markdown-reader", false);
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
fn open_external(app: &mut App, tx: &mut Tx) {
    if let Some(path) = app.preview.reader.path.clone() {
        if let Some(opener) = app.preview.editor_opener {
            opener(app, &path, tx);
        } else {
            let _ = tx.open_url(&file_url(&path));
        }
    }
}
fn file_url(path: &Path) -> String {
    format!(
        "file://{}",
        path.to_string_lossy()
            .bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            })
            .collect::<String>()
    )
}

pub fn reader_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if !matches!(event.key, Key::Escape) {
        app.reader_escape_at = None;
    }
    if matches!(event.key, Key::Escape) && tx.key_repeat() {
        return true;
    }
    match &event.key {
        Key::Command('f') => show_find(app, tx),
        Key::Command('g') => next_match(app, event.mods.shift, tx),
        Key::Command('[') => back(app, tx),
        Key::Command('+') | Key::Command('=') => zoom(app, 1),
        Key::Command('-') => zoom(app, -1),
        Key::Command('0') => zoom(app, 0),
        Key::Escape if app.preview.find_open => hide_find(app, tx),
        Key::Text(value) if value.eq_ignore_ascii_case("e") && !app.preview.find_open => {
            open_external(app, tx)
        }
        // The app routes global reader keys before focused text fields.
        Key::Text(_) | Key::Backspace | Key::Delete if app.preview.find_open => return false,
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
    app.preview.history.clear();
    app.preview.pending_position = None;
}

/// Register a retained secondary window. Its own state gives it independent scrolling and
/// decoding, while the shared flag suspends the workspace's player until Space closes.
pub fn with_space_window(mut host: rune::App<App>) -> rune::App<App> {
    let main_handle = host.handle();
    let open = Arc::new(AtomicBool::new(false));
    let spec = AppSpec::<App> {
        setup: Some(space_setup),
        poll: Some(space_poll),
        on_key: Some(space_key),
        ..AppSpec::new("Preview", (780.0, 660.0), App::preview_window, space_view)
    };
    let space_host = rune::App::new(spec).on_close(space_closed);
    let (space_app, space_window) = host.window(
        WindowSpec::from_app("space-preview", space_host).options(WindowOptions {
            visible: false,
            min_size: Some((420.0, 320.0)),
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
    main_handle.update(move |a, _| {
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
    let target = path.to_owned();
    let theme = app.theme_preview.unwrap_or(app.theme);
    let opener = app.preview.editor_opener;
    let shared = {
        let pane = app.preview.pane.borrow();
        pane.document
            .as_ref()
            .filter(|d| d.path == path)
            .map(|d| (d.clone(), pane.player.clone(), pane.muted))
    };
    handle.update(move |a, _| {
        a.preview.space_path = Some(target.clone());
        a.theme = theme;
        a.preview.editor_opener = opener;
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
fn space_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
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
