//! Transfers: drag-and-drop copies (copyfile(3) with byte progress, cancellation, never
//! replacing) with the Copies pane, and AirDrop beside it.

use std::path::PathBuf;

use rune::{Cx, El, KeyEvent, Tx, Waker, col, text};

use crate::app::{App, Paint};

#[derive(Default)]
pub struct State {}

pub fn setup(_app: &mut App, _waker: &Waker) {}

pub fn poll(_app: &mut App, _tx: &mut Tx) -> bool {
    false
}

/// Copies `sources` into the folder `dest` in the background. One copy runs at a time.
#[allow(dead_code)]
pub fn start_copy(_app: &mut App, _sources: Vec<PathBuf>, _dest: PathBuf, _tx: &mut Tx) {}

/// Stages files for AirDrop and shows Transfers (the command palette and Open menu use it).
#[allow(dead_code)]
pub fn airdrop(_app: &mut App, _files: Vec<PathBuf>, _tx: &mut Tx) {}

pub fn on_key(_app: &mut App, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
    false
}

/// The running copy for the status bar's right side: byte progress and Cancel. Clicking it
/// shows Transfers.
pub fn status(_app: &App, _p: &Paint) -> Option<El> {
    None
}

pub fn view(_app: &App, _cx: &mut Cx<App>, p: &Paint, _layer: &mut Vec<El>) -> El {
    col().key("transfers").center().child(text("Transfers").color(p.look.muted))
}
