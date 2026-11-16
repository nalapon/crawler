//! Shared document previews (code with Tree-sitter-grade highlighting, Markdown, plain text,
//! images, video, Quick Look thumbnails and metadata), the Space preview window, and the
//! full-window Markdown reader that also shows Word documents.

use std::path::Path;

use rune::prefs::Prefs;
use rune::{Cx, El, KeyEvent, Tx, Waker, col, text};

use crate::app::{App, Paint};

#[derive(Default)]
pub struct State {}

impl State {
    pub fn new(_prefs: &Prefs) -> State {
        State::default()
    }
}

pub fn setup(_app: &mut App, _waker: &Waker) {}

pub fn poll(_app: &mut App, _tx: &mut Tx) -> bool {
    false
}

/// The preview panel for `path` inside a pane. `key` scopes element keys.
#[allow(dead_code)]
pub fn pane(_app: &App, _cx: &mut Cx<App>, p: &Paint, path: &Path, key: &str) -> El {
    col().key(key.to_string()).center().child(text(path.display().to_string()).color(p.look.muted))
}

/// Opens Markdown or Word in the reader (`app.reader = Reader::Markdown`).
#[allow(dead_code)]
pub fn open_reader(_app: &mut App, _path: &Path, _tx: &mut Tx) -> Result<(), String> {
    Err("Not ported yet".into())
}

/// Opens the Space preview window for `path`, or closes it if it is open.
#[allow(dead_code)]
pub fn toggle_space(_app: &mut App, _path: &Path, _tx: &mut Tx) {}

pub fn reader_view(_app: &App, _cx: &mut Cx<App>, p: &Paint, _layer: &mut Vec<El>) -> El {
    col().key("markdown-reader").w_full().h_full().fill(p.look.background)
}

pub fn reader_key(_app: &mut App, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
    false
}

pub fn reader_closed(_app: &mut App) {}
