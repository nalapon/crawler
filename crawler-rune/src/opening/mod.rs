//! Opening files: the Enter policy (PDF and Markdown/Word readers, editors for code and text,
//! macOS defaults otherwise), installed editor discovery, the Open menu and recent files.

use std::path::{Path, PathBuf};

use rune::prefs::Prefs;
use rune::{El, Tx, Waker, el};

use crate::app::{App, Paint};

/// An installed app that can edit text, from its bundle's declarations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    pub bundle_id: String,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Default)]
pub struct State {
    /// Discovered in the background; empty until then.
    pub editors: Vec<Editor>,
    /// `defaultTextEditorBundleID`.
    pub default_editor: Option<String>,
    /// `recentOpenedFiles`, newest first, at most eight shown.
    pub recent: Vec<PathBuf>,
}

impl State {
    pub fn new(_prefs: &Prefs) -> State {
        State::default()
    }
}

pub fn setup(_app: &mut App, _waker: &Waker) {}

pub fn poll(_app: &mut App, _tx: &mut Tx) -> bool {
    false
}

/// The Enter policy for a path: folders show in Folders, files open by kind.
pub fn open_path(_app: &mut App, _path: &Path, _tx: &mut Tx) {}

/// Choose the default text editor (Settings and the Open menu share it).
#[allow(dead_code)]
pub fn set_default_editor(app: &mut App, bundle_id: Option<String>) {
    app.opening.default_editor = bundle_id;
}

/// The title bar's Open dropdown for the current selection.
pub fn open_menu(_app: &App, _p: &Paint, _layer: &mut Vec<El>) -> El {
    el().key("open-menu")
}
