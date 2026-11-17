//! The Enter policy and successful-open history. Every file opens inside Crawler: there is no
//! external editor and no hand-off to another app. A file Crawler cannot draw stays closed.
pub(crate) mod platform;

use crate::app::App;
use crate::funnel::model::{DocumentKind, Item, standardized};
use rune::prefs::Prefs;
use rune::{Latest, Tx, Waker};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

/// The outcome of handing a folder to Terminal.
pub(crate) type OpenResult = Result<(), String>;

pub struct State {
    pub recent: Vec<PathBuf>,
    inspections: Vec<Latest<Result<Item, String>>>,
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
            inspections: Vec::new(),
            tx,
            rx,
            waker: None,
            ejection: Latest::default(),
        }
    }
}
pub fn setup(app: &mut App, waker: &Waker) {
    app.opening.waker = Some(waker.clone());
    app.opening.ejection.set_waker(waker.clone());
}
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some(Err(error)) = app.opening.ejection.take() {
        app.toast(error);
        changed = true;
    }
    let results: Vec<_> = app
        .opening
        .inspections
        .iter()
        .filter_map(Latest::take)
        .collect();
    app.opening.inspections.retain(Latest::is_pending);
    for result in results {
        match result {
            Ok(item) => changed |= dispatch(app, item.id.0, item.kind, tx),
            Err(e) => {
                app.toast(e);
                changed = true;
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
/// Opens `path` by its kind once its metadata has been read off the main thread.
pub fn open_path(app: &mut App, path: &Path, _tx: &mut Tx) {
    let task = Latest::default();
    if let Some(waker) = &app.opening.waker {
        task.set_waker(waker.clone());
    }
    task.request(standardized(path), |p| {
        Item::read(&p).map_err(|e| format!("Could not open {}: {e}", p.display()))
    });
    app.opening.inspections.push(task);
}
/// Returns whether the view changed. Kinds Crawler has no reader for do nothing.
fn dispatch(app: &mut App, path: PathBuf, kind: DocumentKind, tx: &mut Tx) -> bool {
    let result = match kind {
        DocumentKind::Folder => {
            crate::funnel::open_folder(app, path, tx);
            return true;
        }
        DocumentKind::Pdf => crate::pdf::open_reader(app, &path, tx),
        DocumentKind::Markdown
        | DocumentKind::Word
        | DocumentKind::Sheet
        | DocumentKind::Text
        | DocumentKind::Image
        | DocumentKind::Video => crate::preview::open_reader(app, &path, tx),
        DocumentKind::Audio | DocumentKind::Package | DocumentKind::Other => return false,
    };
    // Readers record history through record_recent only after their asynchronous load succeeds.
    match result {
        Err(error) => app.toast(error),
        Ok(()) => crate::funnel::leaving(app, app.section),
    }
    true
}
/// Reader modules call this after asynchronous loading succeeds.
pub fn record_recent(app: &mut App, path: &Path) {
    let path = standardized(path);
    app.opening.recent.retain(|p| p != &path);
    app.opening.recent.insert(0, path.clone());
    app.opening.recent.truncate(32);
    let _ = app.prefs.set("recentOpenedFiles", &app.opening.recent);
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
