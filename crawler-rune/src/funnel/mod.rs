//! The scope funnel shared by Search and Folders: the query field, the results, "Narrow the
//! search" and "Where it lives", recent files at an empty Home, and the two folder panels with
//! the Folders places bar. Also the bounded directory library, watchers and folder sizes.

use std::path::PathBuf;

use rune::prefs::Prefs;
use rune::{Cx, El, KeyEvent, Tx, Waker, col, text};

use crate::app::{App, Paint, Section};
use crate::args::Launch;

#[derive(Default)]
pub struct State {}

impl State {
    pub fn new(_launch: &Launch, _prefs: &Prefs) -> State {
        State {}
    }
}

/// After `App::launch` builds the state: apply `--folder`, `--find` and `--search-root`.
pub fn launched(_app: &mut App) {}

pub fn setup(_app: &mut App, _waker: &Waker) {}

pub fn poll(_app: &mut App, _tx: &mut Tx) -> bool {
    false
}

/// Save the section's funnel session before another section shows.
pub fn leaving(_app: &mut App, _from: Section) {}

/// Restore the section's funnel session.
pub fn entering(_app: &mut App, _to: Section) {}

/// Put keyboard focus where the visible section wants it (after a switch or a reader closes).
pub fn focus_section(_app: &mut App, _tx: &mut Tx) {}

/// Show Search, optionally with a query, and focus its field (⌘P, `--find`).
pub fn show_search(app: &mut App, _query: Option<String>, _tx: &mut Tx) {
    app.show(Section::Search);
}

/// Show Folders at `folder`, recording it in Back/Forward history.
#[allow(dead_code)]
pub fn open_folder(app: &mut App, _folder: PathBuf, _tx: &mut Tx) {
    app.show(Section::Folders);
}

/// The visible selection (for the command palette, Open menu and AirDrop).
pub fn selection(_app: &App) -> Vec<PathBuf> {
    Vec::new()
}

/// The folder Folders is showing, or Search's browsing context.
pub fn current_folder(_app: &App) -> Option<PathBuf> {
    None
}

pub fn on_key(_app: &mut App, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
    false
}

/// Status text and key hints for the status bar.
pub fn status_line(_app: &App) -> String {
    String::new()
}

pub fn view(app: &App, _cx: &mut Cx<App>, p: &Paint, _layer: &mut Vec<El>) -> El {
    col().key("funnel").center().child(text(app.section.title()).color(p.look.muted))
}
