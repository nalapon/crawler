//! The Enter policy and successful-open history. Every file opens inside Crawler: there is no
//! external editor and no hand-off to another app. A file Crawler cannot draw stays closed.
pub(crate) mod platform;

use crate::app::App;
use crate::foreground_io::{Foreground, external_path};
use crate::funnel::model::{DocumentKind, Item, standardized};
use rune::prefs::Prefs;
use rune::{Latest, Tx, Waker};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
};

/// The outcome of handing a folder to Terminal.
pub(crate) type OpenResult = Result<(), String>;

type InspectedPaths = Vec<Result<Item, String>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenOrigin {
    Workspace,
    System,
}

pub struct State {
    pub recent: Vec<PathBuf>,
    inspection: Foreground<InspectedPaths>,
    mounted_inspection: Latest<InspectedPaths>,
    mounted_cancel: Option<Arc<AtomicBool>>,
    origin: Option<OpenOrigin>,
    tx: Sender<OpenResult>,
    rx: Receiver<OpenResult>,
    waker: Option<Waker>,
    ejection: Latest<Result<(), String>>,
}
impl State {
    pub fn new(prefs: &Prefs) -> State {
        let (tx, rx) = mpsc::channel();
        State {
            recent: prefs
                .get::<Vec<PathBuf>>("recentOpenedFiles")
                .unwrap_or_default()
                .into_iter()
                .take(32)
                .collect(),
            inspection: Foreground::new("crawler-open-inspection"),
            mounted_inspection: Latest::default(),
            mounted_cancel: None,
            origin: None,
            tx,
            rx,
            waker: None,
            ejection: Latest::default(),
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(cancel) = &self.mounted_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}
pub fn setup(app: &mut App, waker: &Waker) {
    app.opening.waker = Some(waker.clone());
    app.opening.mounted_inspection.set_waker(waker.clone());
    app.opening.ejection.set_waker(waker.clone());
}
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some(Err(error)) = app.opening.ejection.take() {
        app.toast(error);
        changed = true;
    }
    if let Some(results) = app
        .opening
        .inspection
        .take()
        .or_else(|| app.opening.mounted_inspection.take())
    {
        app.opening.origin = None;
        app.opening.mounted_cancel = None;
        for result in results {
            match result {
                Ok(item) => changed |= dispatch(app, item.id.0, item.kind, tx),
                Err(e) => {
                    app.toast(e);
                    changed = true;
                }
            }
        }
    }
    while let Ok(result) = app.opening.rx.try_recv() {
        if let Err(e) = result {
            app.toast(format!("Could not open: {e}"));
            changed = true;
        }
    }
    changed
}

pub(crate) fn cancel_navigation(app: &mut App) {
    if app.opening.origin == Some(OpenOrigin::Workspace) {
        cancel_inspection(app);
    }
}

fn cancel_inspection(app: &mut App) {
    app.opening.inspection.cancel();
    app.opening.mounted_inspection.cancel();
    if let Some(cancel) = app.opening.mounted_cancel.take() {
        cancel.store(true, Ordering::Relaxed);
    }
    app.opening.origin = None;
}

/// Opens `path` by its kind once its metadata has been read off the main thread.
pub fn open_path(app: &mut App, path: &Path, tx: &mut Tx) {
    inspect_paths(app, vec![standardized(path)], OpenOrigin::Workspace, tx);
}

/// Finder and Launch Services can hand over a batch. Keep its order in the one reader,
/// and let a newer explicit open supersede the pending batch.
pub fn open_external_paths(app: &mut App, mut paths: Vec<PathBuf>, tx: &mut Tx) {
    for path in &mut paths {
        *path = standardized(path);
    }
    inspect_paths(app, paths, OpenOrigin::System, tx);
}

pub(crate) fn mounted_path(app: &App, path: &Path) -> bool {
    external_path(path) || crate::funnel::on_external_volume(app, path)
}

fn inspect_paths(app: &mut App, paths: Vec<PathBuf>, origin: OpenOrigin, tx: &mut Tx) {
    let Some(path) = paths.last() else { return };
    app.mark_open_input(path, tx);
    cancel_inspection(app);
    app.opening.origin = Some(origin);
    if paths.iter().any(|path| mounted_path(app, path)) {
        let cancel = Arc::new(AtomicBool::new(false));
        app.opening.mounted_cancel = Some(cancel.clone());
        app.opening
            .mounted_inspection
            .request(paths, move |paths| inspect(paths, &cancel));
    } else if let Err(error) = app
        .opening
        .inspection
        .request(app.opening.waker.clone(), move |cancel| {
            inspect(paths, cancel)
        })
    {
        app.opening.origin = None;
        app.toast(format!("Could not open: {error}"));
    }
}

fn inspect(paths: Vec<PathBuf>, cancel: &AtomicBool) -> InspectedPaths {
    paths
        .into_iter()
        .take_while(|_| !cancel.load(Ordering::Relaxed))
        .map(|path| {
            Item::read(&path).map_err(|error| format!("Could not open {}: {error}", path.display()))
        })
        .collect()
}
/// Returns whether the view changed. Kinds Crawler has no reader for do nothing.
pub(crate) fn dispatch(app: &mut App, path: PathBuf, kind: DocumentKind, tx: &mut Tx) -> bool {
    if matches!(
        kind,
        DocumentKind::Audio | DocumentKind::Package | DocumentKind::Other(_)
    ) {
        return false;
    }
    if crate::pdf::defer_open(app, &path, kind, tx) {
        return true;
    }
    let result = match kind {
        DocumentKind::Folder => {
            if app.reader != crate::app::Reader::None {
                crate::preview::reader_closed(app);
                crate::pdf::reader_closed(app);
                app.reader = crate::app::Reader::None;
            }
            app.finish_leaving();
            crate::funnel::open_folder(app, path, tx);
            return true;
        }
        DocumentKind::Pdf => {
            app.finish_leaving();
            crate::pdf::open_reader(app, &path, tx)
        }
        DocumentKind::Markdown
        | DocumentKind::Word
        | DocumentKind::Sheet
        | DocumentKind::PowerPoint
        | DocumentKind::Text
        | DocumentKind::Image
        | DocumentKind::Video => {
            app.finish_leaving();
            let replacing_pdf = app.reader == crate::app::Reader::Pdf;
            let result = crate::preview::open_reader(app, &path, tx);
            if replacing_pdf && result.is_ok() {
                crate::pdf::reader_closed(app);
            }
            result
        }
        DocumentKind::Audio | DocumentKind::Package | DocumentKind::Other(_) => return false,
    };
    // Readers record history through record_recent only after their asynchronous load succeeds.
    match result {
        Err(error) => app.toast(error),
        Ok(()) => {
            crate::funnel::leaving(app, app.section);
            // The open lands from a worker's result, not from the key that asked for it, and
            // Rune mounts those at once. The reader's sheet should slide in all the same.
            tx.animated();
        }
    }
    true
}
/// Reader modules call this after asynchronous loading succeeds.
pub fn record_recent(app: &mut App, path: &Path) {
    let path = standardized(path);
    if app.opening.recent.first() != Some(&path) {
        app.opening.recent.retain(|p| p != &path);
        app.opening.recent.insert(0, path.clone());
        app.opening.recent.truncate(32);
        let recent = app.opening.recent.clone();
        let _ = app.set_preference("recentOpenedFiles", &recent);
    }
    app.index.record_access(&path);
    crate::funnel::refresh_recent_files(app);
}

pub fn eject_volume(app: &mut App, path: PathBuf) {
    app.opening.ejection.request(path, |p| platform::eject(&p));
}
pub fn open_terminal(app: &mut App, folder: &Path) {
    match platform::terminal_application() {
        Some(application) => platform::open_application(
            folder,
            &application,
            app.opening.tx.clone(),
            app.opening.waker.clone(),
        ),
        None => app.toast("Terminal is unavailable."),
    }
}
pub fn reveal(paths: &[PathBuf]) {
    platform::reveal(paths);
}
