//! File-opening policy, editor discovery and successful-open history.
pub(crate) mod platform;

use crate::app::{App, Paint};
use crate::funnel::model::{DocumentKind, Item, standardized};
use rune::prefs::Prefs;
use rune::{El, Latest, PopupItem, PopupMenu, Tx, Waker, row};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    pub bundle_id: String,
    pub name: String,
    pub path: PathBuf,
}
pub(crate) struct OpenResult {
    path: Option<PathBuf>,
    outcome: Result<(), String>,
}
struct Pending {
    path: PathBuf,
    kind: DocumentKind,
}
pub struct State {
    pub editors: Vec<Editor>,
    pub default_editor: Option<String>,
    pub recent: Vec<PathBuf>,
    discovery: Latest<Vec<Editor>>,
    inspections: Vec<Latest<Result<Item, String>>>,
    awaiting_editors: Vec<Pending>,
    tx: Sender<OpenResult>,
    rx: Receiver<OpenResult>,
    waker: Option<Waker>,
    scanned: bool,
    editor_events: Receiver<()>,
    editor_event_sender: Sender<()>,
    editor_observer: Option<platform::WorkspaceObserver>,
    last_scan: f64,
    ejection: Latest<Result<(), String>>,
}
impl Default for State {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let (editor_event_sender, editor_events) = mpsc::channel();
        Self {
            editors: Vec::new(),
            default_editor: None,
            recent: Vec::new(),
            discovery: Latest::default(),
            inspections: Vec::new(),
            awaiting_editors: Vec::new(),
            tx,
            rx,
            waker: None,
            scanned: false,
            editor_events,
            editor_event_sender,
            editor_observer: None,
            last_scan: f64::NEG_INFINITY,
            ejection: Latest::default(),
        }
    }
}
impl State {
    pub fn new(prefs: &Prefs) -> State {
        State {
            default_editor: prefs.get("defaultTextEditorBundleID"),
            recent: prefs
                .get::<Vec<PathBuf>>("recentOpenedFiles")
                .unwrap_or_default()
                .into_iter()
                .take(32)
                .collect(),
            ..State::default()
        }
    }
}
pub fn setup(app: &mut App, waker: &Waker) {
    app.opening.waker = Some(waker.clone());
    app.opening.discovery.set_waker(waker.clone());
    app.opening.ejection.set_waker(waker.clone());
    app.opening.editor_observer = Some(platform::observe_activation(
        app.opening.editor_event_sender.clone(),
        waker.clone(),
    ));
    refresh_editors(app);
}
pub fn refresh_editors(app: &mut App) {
    app.opening.discovery.request((), |_| discover());
}
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    if app.opening.editor_events.try_iter().next().is_some() {
        for _ in app.opening.editor_events.try_iter() {}
        if app.clock - app.opening.last_scan > 30.0 && !app.opening.discovery.is_pending() {
            refresh_editors(app);
        }
    }
    if let Some(result) = app.opening.ejection.take() {
        if let Err(error) = result {
            app.toast(error);
            changed = true;
        }
    }
    if let Some(editors) = app.opening.discovery.take() {
        changed |= app.section == crate::app::Section::Settings && editors != app.opening.editors;
        app.opening.editors = editors;
        app.opening.scanned = true;
        app.opening.last_scan = app.clock;
        let pending = std::mem::take(&mut app.opening.awaiting_editors);
        for p in pending {
            changed |= dispatch_visible(app, p.path, p.kind, tx);
        }
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
            Ok(item) => changed |= dispatch_visible(app, item.id.0, item.kind, tx),
            Err(e) => {
                app.toast(e);
                changed = true;
            }
        }
    }
    while let Ok(result) = app.opening.rx.try_recv() {
        match result.outcome {
            Ok(()) => {
                if let Some(path) = result.path {
                    record_recent(app, &path);
                    changed = true;
                }
            }
            Err(e) => {
                app.toast(format!("Could not open: {e}"));
                refresh_editors(app);
                changed = true;
            }
        }
    }
    changed
}
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
fn dispatch_visible(app: &mut App, path: PathBuf, kind: DocumentKind, tx: &mut Tx) -> bool {
    let before = (app.section, app.reader, app.toasts.len());
    dispatch(app, path, kind, tx);
    kind == DocumentKind::Folder || before != (app.section, app.reader, app.toasts.len())
}
fn dispatch(app: &mut App, path: PathBuf, kind: DocumentKind, tx: &mut Tx) {
    let result = match kind {
        DocumentKind::Folder => {
            crate::funnel::open_folder(app, path, tx);
            return;
        }
        DocumentKind::Pdf => Some(crate::pdf::open_reader(app, &path, tx)),
        DocumentKind::Markdown | DocumentKind::Word => {
            Some(crate::preview::open_reader(app, &path, tx))
        }
        DocumentKind::Text if !app.opening.scanned => {
            app.opening.awaiting_editors.push(Pending { path, kind });
            return;
        }
        DocumentKind::Text => {
            let editor = selected_editor(app).cloned();
            let Some(editor) = editor else {
                app.toast("No installed text editor was found. Install an editor, then refresh the list in Settings.");
                return;
            };
            if app
                .opening
                .default_editor
                .as_ref()
                .is_some_and(|id| id != &editor.bundle_id)
            {
                app.toast(format!(
                    "Your saved editor is unavailable. Opened with {}.",
                    editor.name
                ));
            }
            platform::open(
                &path,
                Some(&editor),
                app.opening.tx.clone(),
                app.opening.waker.clone(),
            );
            None
        }
        _ => {
            platform::open(
                &path,
                None,
                app.opening.tx.clone(),
                app.opening.waker.clone(),
            );
            None
        }
    };
    // Readers record history through record_recent only after their asynchronous load succeeds.
    match result {
        Some(Err(error)) => app.toast(error),
        Some(Ok(())) => crate::funnel::leaving(app, app.section),
        None => {}
    }
}
/// Reader modules may call this after asynchronous loading succeeds.
pub fn record_recent(app: &mut App, path: &Path) {
    let path = standardized(path);
    app.opening.recent.retain(|p| p != &path);
    app.opening.recent.insert(0, path.clone());
    app.opening.recent.truncate(32);
    let _ = app.prefs.set("recentOpenedFiles", &app.opening.recent);
    app.index.record_access(&path);
    crate::funnel::refresh_recent_files(app);
}
pub fn selected_editor(app: &App) -> Option<&Editor> {
    app.opening
        .editors
        .iter()
        .find(|e| Some(&e.bundle_id) == app.opening.default_editor.as_ref())
        .or_else(|| {
            app.opening
                .editors
                .iter()
                .find(|e| e.bundle_id == "com.apple.TextEdit")
        })
        .or_else(|| app.opening.editors.first())
}
pub fn set_default_editor(app: &mut App, bundle_id: Option<String>) {
    app.opening.default_editor = bundle_id;
    let _ = app
        .prefs
        .set("defaultTextEditorBundleID", &app.opening.default_editor);
}
/// Explicit editor action used by readers without returning through the Enter policy.
pub fn open_in_editor(app: &mut App, path: &Path, _tx: &mut Tx) {
    let editor = if matches!(
        DocumentKind::for_path(path, false),
        DocumentKind::Text | DocumentKind::Markdown
    ) {
        selected_editor(app).cloned()
    } else {
        None
    };
    platform::open(
        path,
        editor.as_ref(),
        app.opening.tx.clone(),
        app.opening.waker.clone(),
    );
}
pub fn open_menu(app: &App, p: &Paint, layer: &mut Vec<El>) -> El {
    let _ = layer;
    row()
        .key("open-controls")
        .items_center()
        .child(
            crate::shell::shell_button(p, "open-selected", "Open", false)
                .disabled(crate::funnel::selection(app).is_empty())
                .opacity(if crate::funnel::selection(app).is_empty() {
                    0.5
                } else {
                    1.0
                })
                .on_press_tx(|a: &mut App, tx| {
                    if let Some(path) = crate::funnel::focused_path(a) {
                        open_path(a, &path, tx);
                    }
                }),
        )
        .child(
            crate::shell::shell_button(p, "open-editor-menu", "⌄", false).on_press_tx(
                |a: &mut App, tx| {
                    refresh_editors(a);
                    let mut menu = PopupMenu::new()
                        .item(PopupItem::new("heading", "Default text editor").enabled(false));
                    for editor in &a.opening.editors {
                        menu = menu.item(
                            PopupItem::new(editor.bundle_id.clone(), editor.name.clone())
                                .checked(selected_editor(a) == Some(editor)),
                        );
                    }
                    if a.opening.editors.is_empty() {
                        menu = menu.item(
                            PopupItem::new("empty", "Finding installed editors…").enabled(false),
                        );
                    }
                    tx.popup_menu(menu, "open-editor-menu", |a: &mut App, picked, _| {
                        if let Some(id) = picked {
                            set_default_editor(a, Some(id.to_owned()));
                        }
                    });
                },
            ),
        )
}
fn discover() -> Vec<Editor> {
    let mut candidates: HashSet<_> = platform::registered_apps().into_iter().collect();
    let mut stack = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        crate::app::home().join("Applications"),
    ];
    while let Some(root) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.file_type() else {
                continue;
            };
            if meta.is_symlink() || entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.extension().is_some_and(|s| s == "app") {
                candidates.insert(path);
            } else if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    let mut sorted: Vec<_> = candidates.into_iter().collect();
    sorted.sort_by(|a, b| rune_kit::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    let mut editors = BTreeMap::new();
    for path in sorted {
        if let Some(editor) = read_editor(path) {
            editors.entry(editor.bundle_id.clone()).or_insert(editor);
        }
    }
    let mut editors: Vec<_> = editors.into_values().collect();
    editors.sort_by(|a, b| rune_kit::natural_cmp(&a.name, &b.name));
    editors
}
fn read_editor(path: PathBuf) -> Option<Editor> {
    use std::os::unix::fs::PermissionsExt;
    let value = plist::Value::from_file(path.join("Contents/Info.plist")).ok()?;
    let info = value.as_dictionary()?;
    let string = |key: &str| info.get(key).and_then(plist::Value::as_string);
    if string("CFBundlePackageType") != Some("APPL")
        || info
            .get("LSBackgroundOnly")
            .and_then(plist::Value::as_boolean)
            == Some(true)
    {
        return None;
    }
    let executable = path
        .join("Contents/MacOS")
        .join(string("CFBundleExecutable")?);
    if std::fs::metadata(executable).ok()?.permissions().mode() & 0o111 == 0 {
        return None;
    }
    let types = info.get("CFBundleDocumentTypes")?.as_array()?;
    let edits = types
        .iter()
        .filter_map(plist::Value::as_dictionary)
        .any(|d| {
            if d.get("CFBundleTypeRole").and_then(plist::Value::as_string) != Some("Editor") {
                return false;
            }
            let strings = |key: &str| -> Vec<&str> {
                d.get(key)
                    .and_then(plist::Value::as_array)
                    .map(|a| a.iter().filter_map(plist::Value::as_string).collect())
                    .unwrap_or_default()
            };
            if strings("CFBundleTypeExtensions").iter().any(|s| {
                matches!(
                    s.to_ascii_lowercase().as_str(),
                    "txt"
                        | "text"
                        | "swift"
                        | "js"
                        | "ts"
                        | "py"
                        | "go"
                        | "rs"
                        | "c"
                        | "cpp"
                        | "md"
                        | "markdown"
                )
            }) {
                return true;
            }
            let content = strings("LSItemContentTypes");
            if content.iter().any(|s| platform::source_type(s)) {
                return true;
            }
            content.iter().any(|s| {
                matches!(
                    *s,
                    "public.text"
                        | "public.plain-text"
                        | "public.utf8-plain-text"
                        | "public.utf16-plain-text"
                )
            }) && content.iter().all(|s| {
                matches!(
                    *s,
                    "public.text"
                        | "public.plain-text"
                        | "public.utf8-plain-text"
                        | "public.utf16-plain-text"
                        | "public.folder"
                        | "public.data"
                        | "public.content"
                )
            })
        });
    if !edits {
        return None;
    }
    Some(Editor {
        bundle_id: string("CFBundleIdentifier")?.to_owned(),
        name: string("CFBundleDisplayName")
            .or_else(|| string("CFBundleName"))
            .map(str::to_owned)
            .unwrap_or_else(|| crate::funnel::model::name(&path)),
        path,
    })
}

pub fn eject_volume(app: &mut App, path: PathBuf) {
    app.opening.ejection.request(path, |p| platform::eject(&p));
}

/// Explicit application choices still record only successful file opens.
pub fn open_with_application(app: &mut App, path: &Path, application: &Path, record: bool) {
    platform::open_application(
        path,
        application,
        record,
        app.opening.tx.clone(),
        app.opening.waker.clone(),
    );
}
pub fn open_terminal(app: &mut App, folder: &Path) {
    match platform::terminal_application() {
        Some(application) => open_with_application(app, folder, &application, false),
        None => app.toast("Terminal is unavailable."),
    }
}
pub fn reveal(paths: &[PathBuf]) {
    platform::reveal(paths);
}
