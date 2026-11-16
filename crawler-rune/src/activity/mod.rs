//! Activity: real libproc samples, the process table, the treemap with details, and the
//! process-name filter.

use rune::{Cx, El, KeyEvent, Tx, Waker, col, text};

use crate::app::{App, Paint};

#[derive(Default)]
pub struct State {}

pub fn setup(_app: &mut App, _waker: &Waker) {}

pub fn poll(_app: &mut App, _tx: &mut Tx) -> bool {
    false
}

pub fn on_key(_app: &mut App, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
    false
}

/// Activity's key hints for the status bar.
pub fn hints(_app: &App) -> String {
    String::new()
}

pub fn view(_app: &App, _cx: &mut Cx<App>, p: &Paint, _layer: &mut Vec<El>) -> El {
    col().key("activity").center().child(text("Activity").color(p.look.muted))
}
