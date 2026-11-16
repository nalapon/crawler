//! PDFs: the full-window reader, the pane preview, page rasterization off the main thread and
//! annotations (highlight, underline, notes) with Undo/Redo and Save.

use std::path::Path;

use rune::{Cx, El, KeyEvent, Tx, Waker, col};

use crate::app::{App, Paint};

#[derive(Default)]
pub struct State {}

pub fn setup(_app: &mut App, _waker: &Waker) {}

pub fn poll(_app: &mut App, _tx: &mut Tx) -> bool {
    false
}

/// The PDF preview panel inside a pane (shares its source and bitmap cache with the reader).
#[allow(dead_code)]
pub fn pane(_app: &App, _cx: &mut Cx<App>, p: &Paint, _path: &Path, key: &str) -> El {
    col().key(key.to_string()).fill(p.look.background)
}

/// Opens the reader (`app.reader = Reader::Pdf`).
#[allow(dead_code)]
pub fn open_reader(_app: &mut App, _path: &Path, _tx: &mut Tx) -> Result<(), String> {
    Err("Not ported yet".into())
}

pub fn reader_view(_app: &App, _cx: &mut Cx<App>, p: &Paint, _layer: &mut Vec<El>) -> El {
    col().key("pdf-reader").w_full().h_full().fill(p.look.background)
}

pub fn reader_key(_app: &mut App, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
    false
}

/// Called when X, Escape-Escape or ⌘W closes the reader. Unsaved changes must be handled first:
/// set `app.reader` back yourself and return early from `reader_key` to keep it open.
pub fn reader_closed(_app: &mut App) {}

/// The window is closing: resolve unsaved changes and in-flight saves.
pub fn on_close(_app: &mut App, _tx: &mut Tx) {}
