//! Settings (theme and default text editor), the command palette (⌘K, ⇧⌘P) and the keyboard
//! shortcut sheet.

use rune::{Cx, El, KeyEvent, Tx, col, el, text};

use crate::app::{App, Paint};

#[derive(Default)]
pub struct State {}

/// Palette and help keys, before the section sees the key.
pub fn on_key(_app: &mut App, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
    false
}

/// The command palette and keyboard help buttons at the section bar's trailing end.
pub fn bar_buttons(_app: &App, _p: &Paint, _layer: &mut Vec<El>) -> El {
    el().key("bar-buttons")
}

/// The command palette and the shortcut sheet, when open.
pub fn overlays(_app: &App, _cx: &mut Cx<App>, _p: &Paint, _layer: &mut Vec<El>) {}

pub fn view(_app: &App, _cx: &mut Cx<App>, p: &Paint, _layer: &mut Vec<El>) -> El {
    col().key("settings").center().child(text("Settings").color(p.look.muted))
}
