//! Shared PDF sources, background Core Graphics rasterization and PDFKit annotation editing.
mod bitmap;
mod model;
mod view;
mod worker;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use rune::{Cx, El, Key, KeyEvent, Live, TextEdit, Tx, Waker};

use crate::app::{App, Paint, Reader};
use crate::funnel::model::DocumentKind;
use model::*;
use worker::{Command, Edit, Event, Worker};

#[derive(PartialEq, Eq)]
enum CloseIntent {
    Reader,
    Open(PathBuf, DocumentKind),
    Window,
    Quit,
}

pub struct State {
    worker: Option<Worker>,
    sources: RefCell<HashMap<PathBuf, Source>>,
    preview_path: RefCell<Option<PathBuf>>,
    active: Option<PathBuf>,
    tool: Tool,
    ink: Ink,
    selected: Option<u64>,
    stroke: Option<Stroke>,
    selection: Option<TextRange>,
    note: TextEdit,
    note_open: bool,
    new_note: Option<TextRange>,
    close_prompt: Option<CloseIntent>,
    after_pending: Option<CloseIntent>,
    zoom: f32,
    pending_zoom: f32,
    zoom_live: Live<f32>,
    pan_live: Live<[f32; 2]>,
    pinch_at: Option<f64>,
    pinch_page: usize,
    viewport: Cell<[f32; 4]>,
    scroll: std::rc::Rc<Cell<[f32; 2]>>,
    epoch: Arc<AtomicU64>,
    max_scroll_ms: Cell<f64>,
    last_scroll: std::rc::Rc<Cell<Option<f64>>>,
    deferred: bool,
    scroll_clock: std::rc::Rc<Cell<f64>>,
    note_anchor: String,
    all_selected: bool,
    error: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            worker: None,
            sources: RefCell::new(HashMap::new()),
            preview_path: RefCell::new(None),
            active: None,
            tool: Tool::Select,
            ink: Ink::Yellow,
            selected: None,
            stroke: None,
            selection: None,
            note: TextEdit::new(""),
            note_open: false,
            new_note: None,
            close_prompt: None,
            after_pending: None,
            zoom: 1.0,
            pending_zoom: 1.0,
            zoom_live: Live::new(1.0),
            pan_live: Live::new([0.0; 2]),
            pinch_at: None,
            pinch_page: 0,
            viewport: Cell::new([0.0; 4]),
            scroll: Default::default(),
            epoch: Arc::new(AtomicU64::new(0)),
            max_scroll_ms: Cell::new(0.0),
            last_scroll: Default::default(),
            deferred: false,
            scroll_clock: Default::default(),
            note_anchor: "pdf-sheet-0".into(),
            all_selected: false,
            error: None,
        }
    }
}

pub fn setup(app: &mut App, waker: &Waker) {
    match Worker::start(waker.clone()) {
        Ok(worker) => app.pdf.worker = Some(worker),
        Err(error) => app.pdf.error = Some(error),
    }
}

impl State {
    fn send(&self, command: Command) {
        if let Some(worker) = &self.worker {
            let _ = worker.send.send(command);
        }
    }

    fn ensure(&self, path: &Path) -> PathBuf {
        let mut sources = self.sources.borrow_mut();
        if sources.get(path).is_some_and(|source| {
            source
                .saved_path
                .as_deref()
                .is_none_or(|saved| saved == path)
                || source.has_edits()
        }) {
            return path.into();
        }
        if let Some((key, _)) = sources
            .iter()
            .find(|(_, s)| s.saved_path.as_deref() == Some(path))
        {
            return key.clone();
        }
        // Only the selected document needs a clean source. Unresolved edits remain owned.
        let released: Vec<_> = sources
            .iter()
            .filter(|(_, s)| !s.has_edits())
            .map(|(path, _)| path.clone())
            .collect();
        for old in released {
            sources.remove(&old);
            self.send(Command::Release(old));
        }
        let mut source = Source::default();
        let surface = source.surfaces.entry(0).or_default();
        surface.poster_requested = true;
        let poster = surface.pixels.clone();
        let started = source.started;
        let owner = Arc::downgrade(&source.owner);
        sources.insert(path.into(), source);
        self.send(Command::Load(path.into(), started, poster, owner));
        path.into()
    }

    fn dirty(&self) -> bool {
        self.active
            .as_ref()
            .and_then(|path| self.sources.borrow().get(path).map(Source::has_edits))
            .unwrap_or(false)
    }

    fn operation_pending(&self) -> bool {
        self.active.as_ref().is_some_and(|path| {
            self.sources
                .borrow()
                .get(path)
                .is_some_and(|source| source.pending > 0)
        })
    }

    fn edit(&mut self, edit: Edit) {
        if self.after_pending.is_some() {
            return;
        }
        let Some(path) = self.active.clone() else {
            return;
        };
        if let Some(source) = self.sources.borrow_mut().get_mut(&path) {
            source.pending += 1;
            source.error = None;
        }
        self.send(Command::Edit { path, edit });
    }

    fn choose_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.selected = None;
        self.clear_stroke();
    }

    fn clear_stroke(&mut self) {
        self.stroke = None;
        for source in self.sources.borrow().values() {
            for overlays in source.overlays.values() {
                for live in overlays {
                    live.set([0.0; 4]);
                }
            }
        }
    }

    fn zoom_to(&mut self, zoom: f32, tx: &mut Tx) {
        self.zoom = zoom.clamp(0.25, 8.0);
        self.pending_zoom = self.zoom;
        self.zoom_live.set(1.0);
        self.pan_live.set([0.0; 2]);
        self.pinch_at = None;
        self.epoch.fetch_add(1, Ordering::Relaxed);
        for source in self.sources.borrow_mut().values_mut() {
            for surface in source.surfaces.values_mut() {
                surface.epoch.fetch_add(1, Ordering::Relaxed);
                surface.requested = None;
            }
        }
        tx.motion(rune::tok::motion::SNAPPY);
    }
}

pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    app.pdf.scroll_clock.set(app.clock);
    let mut changed = false;
    let events: Vec<_> = app
        .pdf
        .worker
        .as_ref()
        .map(|w| w.receive.try_iter().collect())
        .unwrap_or_default();
    for event in events {
        let visible = |path: &Path| {
            if app.reader == Reader::Pdf {
                app.pdf.active.as_deref() == Some(path)
            } else if app.reader == Reader::None {
                app.pdf.preview_path.borrow().as_deref() == Some(path)
            } else {
                false
            }
        };
        match event {
            Event::Loaded {
                owner,
                path,
                summary,
                metrics,
            } => {
                if owner.strong_count() > 0
                    && let Some(source) = app.pdf.sources.borrow_mut().get_mut(&path)
                {
                    source.summary = Some(summary);
                    source.metrics = metrics;
                    changed |= visible(&path);
                }
            }
            Event::Page {
                path,
                page,
                content,
                complete,
                summary,
                metrics,
            } => {
                if let Some(source) = app.pdf.sources.borrow_mut().get_mut(&path) {
                    source.metrics = metrics;
                    if !source.visible.contains(&page) {
                        continue;
                    }
                    let is_new = !source.pages.contains_key(&page);
                    let metadata_changed = complete
                        && source.pages.get(&page).is_some_and(|p| {
                            p.characters.len() != content.characters.len()
                                || p.marks.len() != content.marks.len()
                        });
                    if complete || is_new {
                        source.pages.insert(page, Arc::new(content));
                    }
                    let permissions_changed = source.summary.as_ref().is_some_and(|old| {
                        old.editable != summary.editable || old.copying != summary.copying
                    });
                    source.summary = Some(summary);
                    changed |=
                        visible(&path) && (is_new || metadata_changed || permissions_changed);
                }
            }
            Event::Edited {
                path,
                summary,
                pages,
            } => {
                if let Some(source) = app.pdf.sources.borrow_mut().get_mut(&path) {
                    source.pending = source.pending.saturating_sub(1);
                    source.summary = Some(summary);
                    source.pages = pages
                        .into_iter()
                        .map(|(index, page)| (index, Arc::new(page)))
                        .collect();
                    source.revision += 1;
                    changed |= visible(&path);
                }
            }
            Event::Saved {
                path,
                destination,
                summary,
            } => {
                if let Some(source) = app.pdf.sources.borrow_mut().get_mut(&path) {
                    source.pending = source.pending.saturating_sub(1);
                    source.summary = Some(summary);
                    source.saved_path = Some(destination);
                }
                app.toast("PDF saved");
                changed = true;
            }
            Event::Copied(text) => {
                tx.copy(text);
            }
            Event::Error {
                path,
                message,
                operation,
            } => {
                if let Some(source) = app.pdf.sources.borrow_mut().get_mut(&path) {
                    if operation {
                        source.pending = source.pending.saturating_sub(1);
                    }
                    source.error = Some(message);
                }
                if operation && app.pdf.active.as_ref() == Some(&path) {
                    app.pdf.after_pending = None;
                }
                changed = true;
            }
        }
    }
    if app.pdf.after_pending.is_some()
        && !app.pdf.operation_pending()
        && let Some(intent) = app.pdf.after_pending.take()
    {
        request_close(app, intent, tx);
        changed = true;
    }
    if let Some(at) = app.pdf.pinch_at {
        if app.clock - at >= 0.12 {
            let ratio = app.pdf.pending_zoom / app.pdf.zoom;
            let offset = app.pdf.scroll.get();
            let pan = app.pdf.pan_live.get();
            let viewport = app.pdf.viewport.get();
            let width = viewport[2];
            let height = viewport[3];
            let gap = 16.0 + 16.0 * app.pdf.pinch_page as f32;
            let next_y = (offset[1] * ratio + height * (ratio - 1.0) * 0.5 - pan[1]
                + gap * (1.0 - ratio))
                .max(0.0);
            let base_width = (width - 32.0).max(200.0);
            let old_left = (width - base_width * app.pdf.zoom).max(0.0) * 0.5;
            let new_left = (width - base_width * app.pdf.pending_zoom).max(0.0) * 0.5;
            let next_x = (offset[0] * ratio + width * (ratio - 1.0) * 0.5 - pan[0] + new_left
                - old_left * ratio)
                .max(0.0);
            let zoom = app.pdf.pending_zoom;
            app.pdf.zoom_to(zoom, tx);
            tx.scroll_to_instant("pdf-scroll", next_x, next_y);
            changed = true;
        } else {
            tx.wake_after(Duration::from_millis(125));
        }
    }
    let scrolling = app
        .pdf
        .last_scroll
        .get()
        .is_some_and(|at| app.clock - at < 0.09);
    if app.reader == Reader::Pdf && scrolling && (changed || app.pdf.deferred) {
        app.pdf.deferred = true;
        tx.wake_after(Duration::from_millis(100));
        changed = false;
    } else if app.pdf.deferred {
        app.pdf.deferred = false;
        changed = true;
    }
    changed
}

pub fn pane(app: &App, cx: &mut Cx<App>, p: &Paint, path: &Path, key: &str) -> El {
    let source = app.pdf.ensure(path);
    *app.pdf.preview_path.borrow_mut() = Some(source.clone());
    view::preview(app, cx, p, &source, key)
}

/// Hidden panes drop their view surfaces; the worker retains reusable bitmaps within its budget.
/// What the pane shows under an open reader: an empty panel. The reader owns the source's
/// surfaces meanwhile, and a surface drawn in two places takes only one of them, so the pane
/// neither requests nor draws a page until the reader's sheet is gone.
pub fn still(p: &Paint, key: &str) -> El {
    rune::col()
        .key(key.to_owned())
        .w_full()
        .h_full()
        .min_w(0.0)
        .fill(p.look.raised)
}
pub fn suspend_preview(app: &App) {
    for source in app.pdf.sources.borrow_mut().values_mut() {
        for surface in source.surfaces.values_mut() {
            surface.epoch.fetch_add(1, Ordering::Relaxed);
            surface.pixels.clear();
            surface.sharp.clear();
            surface.poster_requested = false;
            surface.requested = None;
        }
    }
}

pub fn release_previews(app: &mut App) {
    suspend_preview(app);
    let mut sources = app.pdf.sources.borrow_mut();
    let released: Vec<_> = sources
        .iter()
        .filter(|(_, s)| !s.has_edits())
        .map(|(path, _)| path.clone())
        .collect();
    for path in released {
        sources.remove(&path);
        app.pdf.send(Command::Release(path));
    }
    *app.pdf.preview_path.borrow_mut() = None;
}

pub fn open_reader(app: &mut App, path: &Path, tx: &mut Tx) -> Result<(), String> {
    if app.pdf.worker.is_none() {
        return Err(app
            .pdf
            .error
            .clone()
            .unwrap_or_else(|| "The PDF worker is unavailable".into()));
    }
    let source = app.pdf.ensure(path);
    // The pane's sharp crops are sized for the pane. Hide them, so the reader opens on the
    // whole-page poster and not on a crop drawn where the pane had it.
    if let Some(opened) = app.pdf.sources.borrow().get(&source) {
        for surface in opened.surfaces.values() {
            surface.crop.set([0.0; 4]);
        }
    }
    app.pdf.active = Some(source);
    app.pdf.zoom_to(1.0, tx);
    app.pdf.selected = None;
    app.pdf.selection = None;
    app.pdf.close_prompt = None;
    app.pdf.note_open = false;
    app.pdf.tool = Tool::Select;
    app.reader = Reader::Pdf;
    app.reader_escape_at = None;
    tx.focus("pdf-scroll", false);
    tx.scroll_to_instant("pdf-scroll", 0.0, 0.0);
    Ok(())
}

pub fn reader_view(app: &App, cx: &mut Cx<App>, p: &Paint, layer: &mut Vec<El>) -> El {
    view::reader(app, cx, p, layer)
}

pub fn reader_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if app.pdf.after_pending.is_some() {
        return true;
    }
    if ev.key == Key::Escape && tx.key_repeat() {
        return true;
    }
    if ev.key != Key::Escape {
        app.reader_escape_at = None;
    }
    if app.pdf.note_open {
        if ev.key == Key::Command('w') || ev.key == Key::Command('q') {
            request_close(
                app,
                if ev.key == Key::Command('q') {
                    CloseIntent::Quit
                } else {
                    CloseIntent::Reader
                },
                tx,
            );
            return true;
        }
        if ev.key == Key::Command('s') {
            commit_note(app);
            save(app, ev.mods.shift, tx);
            return true;
        }
        if ev.key == Key::Escape {
            app.pdf.note_open = false;
            app.pdf.new_note = None;
            return true;
        }
        return false;
    }
    if app.pdf.close_prompt.is_some() {
        return true;
    }
    if let Key::Command(c) = ev.key {
        match c.to_ascii_lowercase() {
            's' => {
                save(app, ev.mods.shift, tx);
                return true;
            }
            'z' => {
                app.pdf.edit(if ev.mods.shift {
                    Edit::Redo
                } else {
                    Edit::Undo
                });
                return true;
            }
            'c' => {
                if let Some(path) = app.pdf.active.clone() {
                    if app.pdf.all_selected {
                        app.pdf.send(Command::Copy {
                            path,
                            selection: None,
                        });
                    } else if let Some(selection) = app.pdf.selection {
                        app.pdf.send(Command::Copy {
                            path,
                            selection: Some(selection),
                        });
                    }
                }
                return true;
            }
            'a' => {
                app.pdf.all_selected = true;
                return true;
            }
            '+' | '=' => {
                app.pdf.zoom_to(app.pdf.zoom * 1.25, tx);
                return true;
            }
            '-' => {
                app.pdf.zoom_to(app.pdf.zoom / 1.25, tx);
                return true;
            }
            '0' => {
                app.pdf.zoom_to(1.0, tx);
                return true;
            }
            'q' => {
                request_close(app, CloseIntent::Quit, tx);
                return true;
            }
            _ => {}
        }
    }
    if ev.key == Key::Escape && app.pdf.tool != Tool::Select {
        app.pdf.choose_tool(Tool::Select);
        app.reader_escape_at = None;
        return true;
    }
    if matches!(ev.key, Key::Backspace | Key::Delete) {
        if let Some(id) = app.pdf.selected.take() {
            app.pdf.edit(Edit::Delete(id));
            return true;
        }
    }
    let close = matches!(ev.key, Key::Command('w'))
        || matches!(&ev.key, Key::Text(s) if s.eq_ignore_ascii_case("x"))
        || ev.key == Key::Escape && app.reader_escape_at.is_some_and(|at| app.clock - at <= 0.7);
    if close {
        request_close(app, CloseIntent::Reader, tx);
        return true;
    }
    if ev.key != Key::Escape {
        app.reader_escape_at = None;
    }
    false
}

pub fn reader_closed(app: &mut App) {
    app.pdf.epoch.fetch_add(1, Ordering::Relaxed);
    for source in app.pdf.sources.borrow_mut().values_mut() {
        for surface in source.surfaces.values_mut() {
            surface.epoch.fetch_add(1, Ordering::Relaxed);
            surface.requested = None;
        }
    }
    app.pdf.clear_stroke();
    app.pdf.note_open = false;
    let closed = app.pdf.active.take();
    // Hand the surfaces back to the visible pane only when it still shows this PDF.
    if !crate::preview::pane_selection(app).is_some_and(|(path, _)| Some(path) == closed.as_deref())
    {
        release_previews(app);
    }
}

/// Rune's current close callback cannot veto a native window close. The app's host should call
/// `can_close` before closing or quitting; the reader's own close routes already do so.
pub fn on_close(app: &mut App, tx: &mut Tx) {
    if app.pdf.dirty() {
        request_close(app, CloseIntent::Window, tx);
    } else {
        app.pdf.epoch.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn request_quit(app: &mut App, tx: &mut Tx) {
    request_close(app, CloseIntent::Quit, tx);
}

pub fn can_close(app: &mut App, tx: &mut Tx) -> bool {
    if !has_pending_changes(app) {
        return true;
    }
    request_close(app, CloseIntent::Window, tx);
    false
}

fn has_pending_changes(app: &App) -> bool {
    app.pdf.note_open || app.pdf.sources.borrow().values().any(Source::has_edits)
}

/// A Finder open may arrive even while the current reader has a save dialog.
pub(crate) fn defer_open(app: &mut App, path: &Path, kind: DocumentKind, tx: &mut Tx) -> bool {
    if !has_pending_changes(app) {
        return false;
    }
    request_close(app, CloseIntent::Open(path.to_owned(), kind), tx);
    true
}

fn request_close(app: &mut App, intent: CloseIntent, tx: &mut Tx) {
    let current = app
        .pdf
        .after_pending
        .as_ref()
        .or(app.pdf.close_prompt.as_ref());
    if matches!(current, Some(CloseIntent::Quit)) && intent != CloseIntent::Quit
        || matches!(current, Some(CloseIntent::Window))
            && matches!(intent, CloseIntent::Open(_, _) | CloseIntent::Reader)
    {
        return;
    }
    if !app.pdf.note_open && !app.pdf.dirty() && intent != CloseIntent::Reader {
        let next = app
            .pdf
            .sources
            .borrow()
            .iter()
            .find(|(_, source)| source.has_edits())
            .map(|(path, _)| path.clone());
        if let Some(path) = next
            && let Err(error) = open_reader(app, &path, tx)
        {
            app.toast(error);
            return;
        }
    }
    if app.pdf.operation_pending() {
        app.pdf.close_prompt = None;
        app.pdf.after_pending = Some(intent);
    } else if app.pdf.note_open || app.pdf.dirty() {
        app.pdf.close_prompt = Some(intent);
    } else {
        finish_close(app, intent, tx);
    }
}

fn finish_close(app: &mut App, intent: CloseIntent, tx: &mut Tx) {
    app.pdf.close_prompt = None;
    match intent {
        CloseIntent::Reader => {
            app.close_reader();
            if app.reader == Reader::Document {
                crate::preview::focus_reader(tx);
            } else {
                crate::funnel::focus_section(app, tx);
            }
        }
        CloseIntent::Open(path, kind) => {
            crate::opening::dispatch(app, path, kind, tx);
        }
        CloseIntent::Window => tx.close_window(),
        CloseIntent::Quit => tx.quit(),
    }
}

fn save(app: &mut App, as_copy: bool, tx: &mut Tx) {
    if app.pdf.note_open {
        commit_note(app);
    }
    let Some(path) = app.pdf.active.clone() else {
        return;
    };
    let current = app
        .pdf
        .sources
        .borrow()
        .get(&path)
        .and_then(|source| source.saved_path.clone())
        .unwrap_or_else(|| path.clone());
    if as_copy {
        let options = rune::FileDialogOptions {
            title: "Save PDF as".into(),
            directory: current.parent().map(Path::to_path_buf),
            file_name: current
                .file_name()
                .map(|s| s.to_string_lossy().into_owned()),
            ..Default::default()
        }
        .filter("PDF", &["pdf"]);
        if let Some(source) = app.pdf.sources.borrow_mut().get_mut(&path) {
            source.pending += 1;
        }
        tx.save_file(options, move |app: &mut App, result, _tx| {
            if let Some(source) = app.pdf.sources.borrow_mut().get_mut(&path) {
                source.pending = source.pending.saturating_sub(1);
            }
            match result {
                Ok(Some(destination)) => save_to(app, path.clone(), destination),
                Ok(None) => {}
                Err(error) => app.toast(error.to_string()),
            }
        });
    } else {
        save_to(app, path, current);
    }
}

fn save_to(app: &mut App, path: PathBuf, destination: PathBuf) {
    if let Some(source) = app.pdf.sources.borrow_mut().get_mut(&path) {
        source.pending += 1;
        source.error = None;
    }
    app.pdf.send(Command::Save { path, destination });
}

pub fn report(app: &App) -> serde_json::Value {
    let sources = app.pdf.sources.borrow();
    let source = app.pdf.active.as_ref().and_then(|p| sources.get(p));
    serde_json::json!({
        "pdf_reader_load_ms": source.and_then(|s| s.metrics.first_bitmap_ms),
        "pdf_reader_rendering": source.map(|s| &s.metrics),
        "pdf_reader_annotations": source.and_then(|s| s.summary.as_ref()),
        "pdf_retained_surfaces": source.map(|s| s.surfaces.len()),
        "pdf_retained_metadata": source.map(|s| s.pages.len()),
        "pdf_retained_range": source.map(|s| [s.visible.start, s.visible.end]),
        "pdf_peak_scroll_update_ms": app.pdf.max_scroll_ms.get(),
        "pdf_zoom": app.pdf.zoom,
        "pdf_tool": app.pdf.tool,
        "pdf_unsaved_close_prompt": app.pdf.close_prompt.is_some(),
        "pdf_pending_operations": source.map(|s| s.pending).unwrap_or(0),
        "pdf_error": source.and_then(|s| s.error.as_ref()).or(app.pdf.error.as_ref()),
    })
}

fn commit_note(app: &mut App) {
    let note = app.pdf.note.text.clone();
    if let Some(selection) = app.pdf.new_note.take() {
        app.pdf.edit(Edit::Add {
            selection,
            tool: Tool::Note,
            ink: app.pdf.ink,
            note,
        });
    } else if let Some(id) = app.pdf.selected {
        app.pdf.edit(Edit::Note { id, text: note });
    }
    app.pdf.note_open = false;
    app.pdf.clear_stroke();
}

/// Opening policy can record a recent file once a real bitmap has arrived.
/// The document the reader shows.
pub fn active_path(app: &App) -> Option<PathBuf> {
    app.pdf.active.clone()
}

pub fn is_ready(app: &App, path: &Path) -> bool {
    app.pdf.sources.borrow().iter().any(|(key, source)| {
        (key == path || source.saved_path.as_deref() == Some(path))
            && source.metrics.first_bitmap_ms.is_some()
            && source.error.is_none()
    })
}
