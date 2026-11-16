//! Search and Folders share the funnel, with independent sessions and one directory library.
mod library;
pub mod model;
mod scroll;

use crate::app::{App, Paint, Section};
use crate::args::Launch;
use crate::index::{Query, Response, Scope};
use crate::opening::platform::{self, Volume};
use library::{Library, Listing};
use model::{DocumentKind, FolderID, Item, ItemID, Sort, SortField, name, standardized};
use rune::prefs::Prefs;
use rune::{
    Cx, DragItem, El, GrowthSpec, Key, KeyEvent, Latest, Mods, ScrollSpec, TextEdit, Transition,
    Tx, Waker, col, el, row, text,
};
use rune_kit::{ScrollBar, TextInput};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::Duration;

struct Session {
    folder: FolderID,
    query: TextEdit,
    local: bool,
    scope: Option<FolderID>,
    scope_history: Vec<Option<FolderID>>,
    rows: Arc<Vec<Item>>,
    source_rows: Option<Arc<Vec<Item>>>,
    sort: Sort,
    selected: BTreeSet<ItemID>,
    focused: Option<ItemID>,
    neighbor: Option<Item>,
    anchor: Option<ItemID>,
    scroll: Rc<Cell<f32>>,
    saved_scroll: f32,
    chrome: Rc<scroll::Chrome>,
    query_focused: bool,
    request: Option<u64>,
    usable: bool,
    complete: bool,
    pending: Option<Response>,
    last_publication: f64,
    has_nonempty: bool,
    scope_children: Vec<PathBuf>,
    revision: u64,
}
impl Session {
    fn new(folder: PathBuf, query_focused: bool) -> Self {
        Self {
            folder: FolderID::new(&folder),
            query: TextEdit::new(""),
            local: false,
            scope: None,
            scope_history: Vec::new(),
            rows: Arc::new(Vec::new()),
            source_rows: None,
            sort: Sort::default(),
            selected: BTreeSet::new(),
            focused: None,
            neighbor: None,
            anchor: None,
            scroll: Rc::new(Cell::new(0.0)),
            saved_scroll: 0.0,
            chrome: Rc::new(scroll::Chrome::default()),
            query_focused,
            request: None,
            usable: true,
            complete: false,
            pending: None,
            last_publication: 0.0,
            has_nonempty: false,
            scope_children: Vec::new(),
            revision: 0,
        }
    }
    fn searching(&self) -> bool {
        !self.query.text.trim().is_empty()
    }
    fn reconcile_selection(&mut self) {
        let ids: HashSet<_> = self.rows.iter().map(|i| &i.id).collect();
        self.selected.retain(|id| ids.contains(id));
        if !self.focused.as_ref().is_some_and(|id| ids.contains(id)) {
            self.focused = self.rows.first().map(|i| i.id.clone());
            self.selected.clear();
            if let Some(id) = &self.focused {
                self.selected.insert(id.clone());
            }
            self.anchor = self.focused.clone();
        }
        if let Some(id) = &self.focused {
            self.selected.insert(id.clone());
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum List {
    Main,
    Child,
    Context,
}
struct Context {
    parent: PathBuf,
    rows: Arc<Vec<Item>>,
    error: Option<String>,
}

pub struct State {
    search: Session,
    folders: Session,
    library: Library,
    child: Option<FolderID>,
    child_rows: Arc<Vec<Item>>,
    child_chrome: Rc<scroll::Chrome>,
    context_chrome: Rc<scroll::Chrome>,
    pinned: Option<FolderID>,
    history: Vec<FolderID>,
    history_at: usize,
    folder_focus: BTreeMap<FolderID, ItemID>,
    path_editing: bool,
    path_edit: TextEdit,
    recent: Latest<Vec<Item>>,
    recent_key: Vec<PathBuf>,
    recent_rows: Arc<Vec<Item>>,
    context: Latest<Context>,
    context_key: Option<PathBuf>,
    context_rows: Arc<Vec<Item>>,
    context_error: Option<String>,
    context_cache: Vec<Context>,
    volumes: Latest<Vec<Volume>>,
    mounted: Vec<Volume>,
    volume_events: mpsc::Receiver<()>,
    volume_sender: mpsc::Sender<()>,
    volume_observer: Option<platform::VolumeObserver>,
    waker: Option<Waker>,
    focus_pending: bool,
    restore_scroll: bool,
    browse_revision: u64,
    in_workspace: bool,
}
impl State {
    pub fn new(launch: &Launch, prefs: &Prefs) -> Self {
        let (volume_sender, volume_events) = mpsc::channel();
        let folder = launch
            .folder
            .clone()
            .or_else(|| launch.search_root.clone())
            .unwrap_or_else(crate::app::home);
        Self {
            search: Session::new(
                launch.search_root.clone().unwrap_or_else(crate::app::home),
                true,
            ),
            folders: Session::new(folder.clone(), false),
            library: Library::default(),
            child: None,
            child_rows: Arc::new(Vec::new()),
            child_chrome: Rc::new(scroll::Chrome::default()),
            context_chrome: Rc::new(scroll::Chrome::default()),
            pinned: prefs
                .get::<PathBuf>("pinnedFolder")
                .map(|p| FolderID::new(&p)),
            history: vec![FolderID::new(&folder)],
            history_at: 0,
            folder_focus: BTreeMap::new(),
            path_editing: false,
            path_edit: TextEdit::new(""),
            recent: Latest::default(),
            recent_key: Vec::new(),
            recent_rows: Arc::new(Vec::new()),
            context: Latest::default(),
            context_key: None,
            context_rows: Arc::new(Vec::new()),
            context_error: None,
            context_cache: Vec::new(),
            volumes: Latest::default(),
            mounted: Vec::new(),
            volume_sender,
            volume_events,
            volume_observer: None,
            waker: None,
            focus_pending: true,
            restore_scroll: false,
            browse_revision: 0,
            in_workspace: false,
        }
    }
    fn session(&self, section: Section) -> &Session {
        if section == Section::Folders {
            &self.folders
        } else {
            &self.search
        }
    }
    fn session_mut(&mut self, section: Section) -> &mut Session {
        if section == Section::Folders {
            &mut self.folders
        } else {
            &mut self.search
        }
    }
}
fn session(app: &App) -> &Session {
    app.funnel.session(app.section)
}
fn session_mut(app: &mut App) -> &mut Session {
    app.funnel.session_mut(app.section)
}
fn browsing(app: &App) -> bool {
    app.section == Section::Folders && !session(app).searching()
}
fn active(app: &App) -> bool {
    matches!(app.section, Section::Search | Section::Folders)
        && app.reader == crate::app::Reader::None
}
fn prefix(app: &App) -> &'static str {
    if app.section == Section::Folders {
        "folders"
    } else {
        "search"
    }
}
fn query_key(app: &App) -> String {
    format!("{}-query", prefix(app))
}
fn list_key(app: &App) -> String {
    format!("{}-main-scroll", prefix(app))
}

pub fn launched(app: &mut App) {
    if let Some(query) = &app.launch.find {
        app.section = Section::Search;
        app.funnel.search.query = TextEdit::new(query.clone());
    }
    consume_local(app);
}
pub fn setup(app: &mut App, waker: &Waker) {
    app.funnel.in_workspace = active(app);
    let s = &mut app.funnel;
    s.waker = Some(waker.clone());
    s.library.setup(waker);
    s.recent.set_waker(waker.clone());
    s.context.set_waker(waker.clone());
    s.volumes.set_waker(waker.clone());
    s.volumes.request((), |_| platform::volumes());
    s.volume_observer = Some(platform::observe_volumes(
        s.volume_sender.clone(),
        waker.clone(),
    ));
    if active(app) {
        if session(app).searching() {
            submit_query(app);
        } else {
            prepare_browsing(app);
        }
    }
}
pub fn leaving(app: &mut App, from: Section) {
    app.funnel.in_workspace = false;
    if matches!(from, Section::Search | Section::Folders) {
        let s = app.funnel.session_mut(from);
        s.query_focused = s.query.focused;
        s.saved_scroll = s.scroll.get();
        s.chrome.hide_keyboard();
        app.index.idle();
        app.funnel.library.visible(Vec::new(), None);
        app.funnel.context.cancel();
    }
}
pub fn entering(app: &mut App, to: Section) {
    app.funnel.in_workspace = matches!(to, Section::Search | Section::Folders);
    if matches!(to, Section::Search | Section::Folders) {
        app.funnel.focus_pending = true;
        app.funnel.restore_scroll = true;
        if session(app).searching() {
            submit_query(app);
        } else {
            prepare_browsing(app);
        }
    }
}
pub fn focus_section(app: &mut App, tx: &mut Tx) {
    if !active(app) {
        return;
    }
    if !app.funnel.in_workspace {
        app.funnel.in_workspace = true;
        if session(app).searching() {
            submit_query(app);
        } else {
            prepare_browsing(app);
        }
    }
    if session(app).query_focused {
        tx.focus(query_key(app), false);
    } else {
        tx.focus(format!("{}-main-list", prefix(app)), false);
    }
}
pub fn show_search(app: &mut App, query: Option<String>, tx: &mut Tx) {
    app.show(Section::Search);
    app.funnel.search.query_focused = true;
    if let Some(query) = query {
        app.funnel.search.query = TextEdit::new(query);
        query_changed(app, tx);
    }
    tx.focus(query_key(app), false);
}
pub fn open_folder(app: &mut App, folder: PathBuf, tx: &mut Tx) {
    app.show(Section::Folders);
    browse(app, FolderID::new(&folder), true, tx);
}
fn browse(app: &mut App, folder: FolderID, record: bool, tx: &mut Tx) {
    if let Some(id) = app.funnel.folders.focused.clone() {
        app.funnel
            .folder_focus
            .insert(app.funnel.folders.folder.clone(), id);
        while app.funnel.folder_focus.len() > 32 {
            if let Some(key) = app
                .funnel
                .folder_focus
                .keys()
                .find(|f| **f != folder)
                .cloned()
            {
                app.funnel.folder_focus.remove(&key);
            } else {
                break;
            }
        }
    }
    let remembered = app.funnel.folder_focus.get(&folder).cloned();

    if record && app.funnel.history.get(app.funnel.history_at) != Some(&folder) {
        app.funnel.history.truncate(app.funnel.history_at + 1);
        app.funnel.history.push(folder.clone());
        app.funnel.history_at = app.funnel.history.len() - 1;
    }
    let s = &mut app.funnel.folders;
    s.folder = folder;
    s.query = TextEdit::new("");
    s.query_focused = false;
    s.local = false;
    s.scope = None;
    s.focused = remembered.clone();
    s.neighbor = None;
    s.selected.clear();
    if let Some(id) = remembered {
        s.selected.insert(id);
    }
    s.anchor = None;
    s.rows = Arc::new(Vec::new());
    s.source_rows = None;
    s.scroll.set(0.0);
    s.saved_scroll = 0.0;
    s.chrome.hide_keyboard();
    s.revision += 1;
    app.funnel.browse_revision += 1;
    app.index.idle();
    prepare_browsing(app);
    tx.scroll_to_instant(list_key(app), 0.0, 0.0);
    tx.focus(format!("{}-main-list", prefix(app)), false);
}
fn travel(app: &mut App, delta: isize, tx: &mut Tx) {
    let at = app
        .funnel
        .history_at
        .saturating_add_signed(delta)
        .min(app.funnel.history.len().saturating_sub(1));
    if at == app.funnel.history_at {
        return;
    }
    app.funnel.history_at = at;
    if let Some(folder) = app.funnel.history.get(at).cloned() {
        browse(app, folder, false, tx);
    }
}
pub fn selection(app: &App) -> Vec<PathBuf> {
    if !active(app) {
        return Vec::new();
    }
    let s = session(app);
    if !s.usable {
        return Vec::new();
    }
    if let Some(item) = &s.neighbor {
        return vec![item.id.0.clone()];
    }
    s.rows
        .iter()
        .filter(|i| s.selected.contains(&i.id))
        .map(|i| i.id.0.clone())
        .collect()
}
pub fn focused_path(app: &App) -> Option<PathBuf> {
    let s = session(app);
    s.usable
        .then(|| {
            s.neighbor
                .as_ref()
                .map(|i| i.id.0.clone())
                .or_else(|| s.focused.as_ref().map(|i| i.0.clone()))
        })
        .flatten()
}
pub fn current_folder(app: &App) -> Option<PathBuf> {
    Some(session(app).folder.0.clone())
}
pub fn pin_current(app: &mut App) {
    let folder = session(app).folder.clone();
    app.funnel.pinned = Some(folder.clone());
    let _ = app.prefs.set("pinnedFolder", &folder.0);
}
pub fn pinned_folder(app: &App) -> Option<PathBuf> {
    app.funnel.pinned.as_ref().map(|f| f.0.clone())
}
pub fn connected_volumes(app: &App) -> Vec<(PathBuf, String, bool)> {
    app.funnel
        .mounted
        .iter()
        .map(|v| (v.path.clone(), v.name.clone(), v.ejectable))
        .collect()
}

fn consume_local(app: &mut App) {
    let s = session_mut(app);
    if s.query.text.starts_with('$') {
        let text = s.query.text.trim_start_matches('$').to_owned();
        s.query = TextEdit::new(text);
        s.local = true;
        s.scope = None;
    }
    if s.query.text.is_empty() && !s.local {
        s.scope = None;
    }
}
fn query_changed(app: &mut App, _tx: &mut Tx) {
    session_mut(app).query_focused = true;
    let was_local = session(app).local;
    consume_local(app);
    if session(app).query.text.trim().is_empty() {
        if was_local {
            session_mut(app).local = false;
        }
        let s = session_mut(app);
        s.request = None;
        s.pending = None;
        s.usable = true;
        s.scope = None;
        s.scope_children.clear();
        s.revision += 1;
        app.index.idle();
        prepare_browsing(app);
    } else {
        submit_query(app);
    }
}
fn submit_query(app: &mut App) {
    if !session(app).searching() {
        return;
    }
    let s = session(app);
    let volume = app
        .funnel
        .mounted
        .iter()
        .filter(|v| s.folder.0.starts_with(&v.path))
        .max_by_key(|v| v.path.components().count())
        .map(|v| v.path.clone())
        .filter(|_| app.section == Section::Folders);
    let scope = if s.local {
        Scope::Under(s.scope.as_ref().unwrap_or(&s.folder).0.clone())
    } else if let Some(scope) = &s.scope {
        Scope::Under(scope.0.clone())
    } else if let Some(volume) = &volume {
        Scope::Under(volume.clone())
    } else {
        Scope::Mac
    };
    let query = Query {
        text: s.query.text.trim().to_owned(),
        scope,
        volume,
        folder_mode: s.local,
    };
    let id = app.index.search(query);
    let s = session_mut(app);
    s.request = Some(id);
    s.usable = false;
    s.complete = false;
    s.pending = None;
    s.has_nonempty = false;
}
fn publish(app: &mut App, response: Response) -> bool {
    let clock = app.clock;
    let s = session_mut(app);
    let rows: Vec<_> = response.hits.iter().map(Item::from_hit).collect();
    let changed = s.rows.as_ref() != &rows || !s.usable || s.complete != response.complete;
    let scopes = response.scope_counts.into_iter().map(|(p, _)| p).collect();
    let changed = changed || s.scope_children != scopes;
    s.scope_children = scopes;
    if s.rows.as_ref() != &rows {
        s.rows = Arc::new(rows);
        s.revision += 1;
    }
    s.usable = true;
    s.complete = response.complete;
    s.has_nonempty |= !s.rows.is_empty();
    s.last_publication = clock;
    s.reconcile_selection();
    changed
}
fn prepare_browsing(app: &mut App) {
    if browsing(app) {
        let folder = app.funnel.folders.folder.clone();
        app.funnel.library.ensure(&folder, false);
        sync_directory(app);
    } else if !session(app).searching() {
        let rows = app.funnel.recent_rows.clone();
        let s = session_mut(app);
        s.rows = rows;
        s.usable = true;
        s.reconcile_selection();
    }
}
fn sync_directory(app: &mut App) {
    let folder = app.funnel.folders.folder.clone();
    if let Some(Listing::Ready(rows)) = app.funnel.library.listing(&folder) {
        let rows = rows.clone();
        let s = &mut app.funnel.folders;
        if !s
            .source_rows
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, &rows))
        {
            s.source_rows = Some(rows.clone());
            s.rows = if s.sort == Sort::default() {
                rows
            } else {
                let mut rows = rows.as_ref().clone();
                s.sort.apply(&mut rows);
                Arc::new(rows)
            };
            s.revision += 1;
            s.reconcile_selection();
        }
        s.usable = true;
    }
}
fn sync_child(app: &mut App) {
    let child = session(app)
        .neighbor
        .as_ref()
        .or_else(|| {
            session(app)
                .focused
                .as_ref()
                .and_then(|id| session(app).rows.iter().find(|i| &i.id == id))
        })
        .filter(|i| i.kind == DocumentKind::Folder)
        .map(|i| FolderID::new(&i.id.0));
    if app.funnel.child != child {
        app.funnel.child = child.clone();
        app.funnel.child_rows = Arc::new(Vec::new());
    }
    let mut visible = Vec::new();
    let mut active_folder = None;
    if browsing(app) {
        let folder = session(app).folder.clone();
        visible.push(folder.clone());
        active_folder = Some(folder);
    }
    if let Some(child) = child {
        app.funnel.library.ensure(&child, false);
        visible.push(child.clone());
        if let Some(Listing::Ready(rows)) = app.funnel.library.listing(&child) {
            app.funnel.child_rows = rows.clone();
        }
    }
    app.funnel.library.visible(visible, active_folder);
}
fn update_context(app: &mut App) {
    if !session(app).usable {
        return;
    }
    if !session(app).searching() {
        app.funnel.context.cancel();
        app.funnel.context_key = None;
        return;
    }
    let parent = focused_path(app).and_then(|p| p.parent().map(Path::to_owned));
    if parent == app.funnel.context_key {
        return;
    }
    app.funnel.context_key = parent.clone();
    app.funnel.context_error = None;
    let Some(parent) = parent else {
        app.funnel.context_rows = Arc::new(Vec::new());
        return;
    };
    if let Some(cached) = app.funnel.context_cache.iter().find(|c| c.parent == parent) {
        app.funnel.context_rows = cached.rows.clone();
    } else {
        app.funnel.context_rows = Arc::new(Vec::new());
    }
    let selected = session(app)
        .neighbor
        .as_ref()
        .or_else(|| {
            session(app)
                .focused
                .as_ref()
                .and_then(|id| session(app).rows.iter().find(|i| &i.id == id))
        })
        .cloned();
    app.funnel
        .context
        .request((parent, selected), |(parent, selected)| {
            let (mut rows, error) = match library::bounded_context(&parent) {
                Ok(rows) => (rows, None),
                Err(error) => (Vec::new(), Some(error)),
            };
            if let Some(item) = selected {
                if !rows.iter().any(|i| i.id == item.id) {
                    rows.insert(0, item);
                }
            }
            Context {
                rows: Arc::new(rows),
                parent,
                error,
            }
        });
}
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let visible = active(app);
    let mut changed = false;
    if let Some(volumes) = app.funnel.volumes.take() {
        let updated = app.funnel.mounted != volumes;
        changed |= visible && app.section == Section::Folders && updated;
        app.funnel.mounted = volumes;
        if visible && updated && app.section == Section::Folders && session(app).searching() {
            submit_query(app);
        }
    }
    if app.funnel.volume_events.try_iter().next().is_some() {
        for _ in app.funnel.volume_events.try_iter() {}
        app.funnel.volumes.request((), |_| platform::volumes());
    }
    if visible && app.funnel.recent_key != app.opening.recent {
        app.funnel.recent_key = app.opening.recent.clone();
        app.funnel
            .recent
            .request(app.opening.recent.clone(), |paths| {
                paths
                    .iter()
                    .filter_map(|p| Item::read(p).ok())
                    .filter(|i| i.kind != DocumentKind::Folder)
                    .take(8)
                    .collect()
            });
    }
    if let Some(rows) = app.funnel.recent.take() {
        let rows = Arc::new(rows);
        let updated = app.funnel.recent_rows != rows;
        app.funnel.recent_rows = rows;
        if visible && !browsing(app) && !session(app).searching() {
            prepare_browsing(app);
            changed |= updated;
        }
    }
    let directory_changed = app.funnel.library.poll();
    if visible {
        changed |= directory_changed;
        if browsing(app) {
            sync_directory(app);
        }
        if let Some(response) = app.index.take() {
            if session(app).request == Some(response.id) && session(app).searching() {
                if (!session(app).has_nonempty && !response.hits.is_empty())
                    || app.clock - session(app).last_publication >= 0.040
                {
                    changed |= publish(app, response);
                } else {
                    session_mut(app).pending = Some(response);
                }
            }
        }
        if session(app).pending.is_some() {
            let remaining = 0.040 - (app.clock - session(app).last_publication);
            if remaining <= 0.0 {
                if let Some(response) = session_mut(app).pending.take() {
                    changed |= publish(app, response);
                }
            } else {
                tx.wake_after(Duration::from_secs_f64(remaining));
            }
        }
        let old_child = app.funnel.child.clone();
        sync_child(app);
        if old_child != app.funnel.child {
            app.funnel.child_chrome.hide_keyboard();
            tx.scroll_to_instant("child-scroll", 0.0, 0.0);
            changed = true;
        }
        update_context(app);
        if let Some(context) = app.funnel.context.take() {
            if app.funnel.context_key.as_ref() == Some(&context.parent) {
                changed |= app.funnel.context_rows != context.rows;
                app.funnel.context_rows = context.rows.clone();
                app.funnel.context_error = context.error.clone();
                app.funnel
                    .context_cache
                    .retain(|c| c.parent != context.parent);
                app.funnel.context_cache.insert(0, context);
                app.funnel.context_cache.truncate(4);
            }
        }
        if app.funnel.focus_pending {
            app.funnel.focus_pending = false;
            focus_section(app, tx);
        }
        if app.funnel.restore_scroll {
            app.funnel.restore_scroll = false;
            tx.scroll_to_instant(list_key(app), 0.0, session(app).saved_scroll);
        }
    }
    if app.funnel.library.has_debounce() {
        tx.wake_after(Duration::from_millis(120));
    }
    changed
}

fn select(app: &mut App, id: ItemID, mods: Mods, tx: &mut Tx, keyboard: bool) {
    if !session(app).usable {
        return;
    }
    let multi = browsing(app);
    let s = session_mut(app);
    s.neighbor = None;
    let Some(at) = s.rows.iter().position(|i| i.id == id) else {
        return;
    };
    if multi && mods.shift {
        let anchor = s
            .anchor
            .as_ref()
            .and_then(|a| s.rows.iter().position(|i| &i.id == a))
            .unwrap_or(at);
        s.selected = s.rows[anchor.min(at)..=anchor.max(at)]
            .iter()
            .map(|i| i.id.clone())
            .collect();
    } else if multi && mods.cmd {
        if !s.selected.remove(&id) {
            s.selected.insert(id.clone());
        }
        s.anchor = Some(id.clone());
    } else {
        s.selected.clear();
        s.selected.insert(id.clone());
        s.anchor = Some(id.clone());
    }
    s.focused = Some(id.clone());
    s.query_focused = false;
    if keyboard {
        session(app).chrome.hide_keyboard();
        tx.scroll_into_view_in(list_key(app), id.key());
    }
    tx.focus(format!("{}-main-list", prefix(app)), false);
    sync_child(app);
    update_context(app);
}
fn move_selection(app: &mut App, delta: isize, shift: bool, tx: &mut Tx) {
    let s = session(app);
    if !s.usable || s.rows.is_empty() {
        return;
    }
    let current = s
        .focused
        .as_ref()
        .and_then(|id| s.rows.iter().position(|i| &i.id == id))
        .unwrap_or(0);
    let at = current.saturating_add_signed(delta).min(s.rows.len() - 1);
    let id = s.rows[at].id.clone();
    select(
        app,
        id,
        Mods {
            shift,
            ..Mods::default()
        },
        tx,
        true,
    );
}
fn activate(app: &mut App, tx: &mut Tx, reveal: bool) {
    let Some(path) = focused_path(app) else {
        return;
    };
    if reveal {
        if let Some(parent) = path.parent() {
            open_folder(app, parent.to_owned(), tx);
            let id = ItemID::new(&path);
            session_mut(app).focused = Some(id.clone());
            session_mut(app).selected.insert(id);
        }
    } else {
        crate::opening::open_path(app, &path, tx);
    }
}
fn enter_selected_folder(app: &mut App, tx: &mut Tx) {
    if !session(app).usable {
        return;
    }
    let item = session(app).neighbor.as_ref().or_else(|| {
        session(app)
            .focused
            .as_ref()
            .and_then(|id| session(app).rows.iter().find(|i| &i.id == id))
    });
    if let Some(item) = item.filter(|i| i.kind == DocumentKind::Folder) {
        open_folder(app, item.id.0.clone(), tx);
    }
}
fn parent(app: &mut App, tx: &mut Tx) {
    if session(app).searching() {
        if let Some(scope) = session(app).scope.clone() {
            let p = scope.0.parent().map(FolderID::new);
            session_mut(app).scope = p;
            submit_query(app);
        }
        return;
    }
    let old = session(app).folder.0.clone();
    if let Some(parent) = old.parent() {
        open_folder(app, parent.to_owned(), tx);
        let id = ItemID::new(&old);
        let s = session_mut(app);
        s.focused = Some(id.clone());
        s.selected.clear();
        s.selected.insert(id);
    }
}
fn field_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match &ev.key {
        Key::Escape => {
            session_mut(app).query_focused = false;
            tx.focus(format!("{}-main-list", prefix(app)), false);
            true
        }
        Key::Down | Key::Up => {
            move_selection(app, if ev.key == Key::Down { 1 } else { -1 }, false, tx);
            true
        }
        Key::Enter => {
            activate(app, tx, ev.mods.cmd);
            true
        }
        Key::Backspace if session(app).query.text.is_empty() && session(app).local => {
            session_mut(app).local = false;
            true
        }
        Key::Text(t) if t == "$" && session(app).query.text.is_empty() => {
            session_mut(app).local = true;
            true
        }
        _ => false,
    }
}
pub fn on_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match &ev.key {
        Key::Command('[') if app.section == Section::Folders => {
            travel(app, -1, tx);
            return true;
        }
        Key::Command(']') if app.section == Section::Folders => {
            travel(app, 1, tx);
            return true;
        }
        Key::Command('l') if app.section == Section::Folders => {
            app.funnel.path_editing = true;
            app.funnel.path_edit = TextEdit::new(session(app).folder.0.to_string_lossy());
            let n = app.funnel.path_edit.text.len();
            app.funnel.path_edit.set_selection(0, n);
            tx.focus("folder-path", false);
            return true;
        }
        Key::Command('r') => {
            let folder = session(app).folder.clone();
            app.funnel.library.refresh(&folder);
            if session(app).searching() {
                submit_query(app);
            }
            return true;
        }
        _ => {}
    }
    if app.funnel.path_editing
        || tx
            .focused()
            .is_some_and(|f| f.contains("query") || f.contains("folder-path"))
    {
        return false;
    }
    match &ev.key {
        Key::Command('a') if browsing(app) => {
            let s = session_mut(app);
            s.selected = s.rows.iter().map(|i| i.id.clone()).collect();
            true
        }
        Key::Down | Key::Up => {
            move_selection(
                app,
                if ev.key == Key::Down { 1 } else { -1 },
                ev.mods.shift,
                tx,
            );
            true
        }
        Key::Text(t) if matches!(t.as_str(), "j" | "J" | "k" | "K") => {
            move_selection(
                app,
                if t.eq_ignore_ascii_case("j") { 1 } else { -1 },
                ev.mods.shift || t == "J" || t == "K",
                tx,
            );
            true
        }
        Key::Home | Key::End => {
            move_selection(
                app,
                if ev.key == Key::Home {
                    isize::MIN
                } else {
                    isize::MAX
                },
                ev.mods.shift,
                tx,
            );
            true
        }
        Key::Enter => {
            activate(app, tx, ev.mods.cmd);
            true
        }
        Key::Right => {
            enter_selected_folder(app, tx);
            true
        }
        Key::Left => {
            parent(app, tx);
            true
        }
        Key::Text(t) if t == "l" => {
            enter_selected_folder(app, tx);
            true
        }
        Key::Text(t) if t == "h" => {
            parent(app, tx);
            true
        }
        Key::Text(t) if t == " " => {
            if let Some(path) = focused_path(app) {
                crate::preview::toggle_space(app, &path, tx);
            }
            true
        }
        Key::Escape => {
            let s = session_mut(app);
            if s.selected.len() > 1 {
                s.selected.clear();
                if let Some(id) = &s.focused {
                    s.selected.insert(id.clone());
                }
            } else if s.searching() {
                s.query = TextEdit::new("");
                query_changed(app, tx);
            } else {
                s.query_focused = true;
                tx.focus(query_key(app), false);
            }
            true
        }
        Key::Text(t) if !ev.mods.cmd && !ev.mods.ctrl => {
            let s = session_mut(app);
            s.query_focused = true;
            if t == "$" && s.query.text.is_empty() {
                s.local = true;
            } else {
                s.query.insert(t);
                query_changed(app, tx);
            }
            tx.focus(query_key(app), false);
            true
        }
        _ => false,
    }
}
pub fn status_line(app: &App) -> String {
    let s = session(app);
    if browsing(app) {
        let state = app.funnel.library.listing(&s.folder);
        return match state {
            None | Some(Listing::Loading) => "Loading folder…".into(),
            Some(Listing::Failed(e)) => e.clone(),
            Some(Listing::Ready(rows)) => format!(
                "{} items{} · ↑↓ select · Return open · Space preview · ⌘R refresh",
                rows.len(),
                if s.selected.len() > 1 {
                    format!(" · {} selected", s.selected.len())
                } else {
                    String::new()
                }
            ),
        };
    }
    if s.searching() {
        if let crate::index::Status::Failed(error) = app.index.status() {
            return error;
        }
        format!(
            "{} · ↑↓ select · Return open · Space preview",
            if !s.usable {
                "Searching…"
            } else if !s.complete {
                "Searching…"
            } else {
                "Search ready"
            }
        )
    } else {
        "Recent files · Type to search · $ search this folder · Return open · Space preview".into()
    }
}

pub fn view(app: &App, cx: &mut Cx<App>, p: &Paint, _layer: &mut Vec<El>) -> El {
    let s = session(app);
    let mut body = col()
        .key("funnel")
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(p.look.background);
    if app.section == Section::Folders {
        body = body.child(places(app, p));
    }
    let query = TextInput::new(&s.query)
        .placeholder("Search files…")
        .label("Search files")
        .height(32.0)
        .plain()
        .on_edit(|a: &mut App| &mut session_mut(a).query)
        .on_change(query_changed)
        .on_key(field_key)
        .build(&p.kit)
        .key(query_key(app))
        .w_full();
    let scope_label = if s.local {
        format!("In {}", name(&s.folder.0))
    } else if let Some(scope) = &s.scope {
        format!("In {}", scope.0.display())
    } else {
        "This Mac".into()
    };
    body = body.child(
        col()
            .key("search-field-band")
            .w_full()
            .h(76.0)
            .px(32.0)
            .center()
            .child(
                row()
                    .key("search-field-wrap")
                    .w_full()
                    .max_w(600.0)
                    .h(48.0)
                    .px(14.0)
                    .gap(8.0)
                    .items_center()
                    .border(1.0, p.look.line)
                    .radius(if p.look.era() == crate::theme::Era::Modern {
                        8.0
                    } else {
                        0.0
                    })
                    .fill(if s.local {
                        p.look.selection.alpha(0.45)
                    } else {
                        p.look.raised
                    })
                    .child(query.grow(1.0).min_w(0.0))
                    .child(
                        text(scope_label)
                            .key("search-scope-label")
                            .font(rune::tok::type_::LABEL)
                            .color(p.look.muted)
                            .truncate()
                            .max_w(120.0)
                            .inert(),
                    ),
            ),
    );
    let main_title = if browsing(app) {
        name(&s.folder.0)
    } else if s.searching() {
        "Results".into()
    } else {
        "Recently opened".into()
    };
    let detail = if browsing(app) {
        s.folder.0.parent().map(|p| p.display().to_string())
    } else {
        None
    };
    let main = col()
        .key(format!("{}-main-pane", prefix(app)))
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .h_full()
        .child(crate::shell::pane_title(
            p,
            "main-title",
            &main_title,
            detail.as_deref(),
            true,
        ))
        .child(
            col()
                .key(if browsing(app) {
                    format!("folder-content-{}", s.folder.0.display())
                } else {
                    format!("{}-content", prefix(app))
                })
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .transition(Transition::opacity().combined(Transition::offset(14.0, 0.0)))
                .motion(rune::tok::motion::SNAPPY)
                .child(main_content(app, p)),
        );
    let mut panels = row().key("funnel-panels").w_full().grow(1.0).min_h(0.0);
    if s.searching() {
        panels = panels.child(
            col()
                .key("funnel-rail")
                .w(240.0)
                .h_full()
                .min_h(0.0)
                .child(scope_rail(app, p).grow(0.44))
                .child(context_pane(app, p).grow(0.56)),
        );
    }
    panels = panels
        .child(main)
        .child(el().w(1.0).h_full().fill(p.look.line));
    let selected = s.neighbor.as_ref().or_else(|| {
        s.focused
            .as_ref()
            .and_then(|id| s.rows.iter().find(|i| &i.id == id))
    });
    let right = if app.funnel.child.is_some() {
        let folder = app.funnel.child.as_ref().expect("child was checked");
        col()
            .key("child-pane")
            .h_full()
            .min_h(0.0)
            .child(crate::shell::pane_title(
                p,
                "child-title",
                &name(&folder.0),
                folder.0.parent().map(|p| p.to_string_lossy()).as_deref(),
                false,
            ))
            .child(directory_content(app, p, folder, List::Child))
    } else if let Some(item) = selected {
        col()
            .key("preview-pane")
            .h_full()
            .min_h(0.0)
            .child(crate::shell::pane_title(
                p,
                "preview-title",
                &item.name,
                None,
                false,
            ))
            .child(
                crate::preview::pane(app, cx, p, &item.id.0, "funnel-preview")
                    .grow(1.0)
                    .min_h(0.0)
                    .w_full(),
            )
    } else {
        col()
            .key("preview-pane")
            .h_full()
            .min_h(0.0)
            .child(crate::shell::pane_title(
                p,
                "preview-title",
                "Preview",
                None,
                false,
            ))
            .child(empty(p, "Select a file to preview"))
    };
    let right = right.grow(1.0).min_w(0.0);
    body.child(panels.child(right))
}
fn empty(p: &Paint, message: &str) -> El {
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .center()
        .padding(24.0)
        .child(
            text(message)
                .font(rune::tok::type_::BODY)
                .color(p.look.muted)
                .wrap(),
        )
}
fn main_content(app: &App, p: &Paint) -> El {
    if browsing(app) {
        return directory_content(app, p, &session(app).folder, List::Main);
    }
    if session(app).rows.is_empty() {
        return empty(
            p,
            if session(app).searching() {
                if session(app).complete {
                    "No matching files"
                } else {
                    "Searching…"
                }
            } else {
                "Files you open will appear here"
            },
        );
    }
    file_list(app, p, List::Main)
}
fn directory_content(app: &App, p: &Paint, folder: &FolderID, list: List) -> El {
    match app.funnel.library.listing(folder) {
        None | Some(Listing::Loading) => empty(p, "Loading folder…").key("folder-loading"),
        Some(Listing::Failed(error)) => empty(p, error).key("folder-error"),
        Some(Listing::Ready(rows)) if rows.is_empty() => {
            empty(p, "This folder is empty").key("folder-empty")
        }
        Some(Listing::Ready(_)) => {
            let content = file_list(app, p, list);
            if list == List::Main {
                col()
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .child(sort_header(app, p))
                    .child(content)
            } else {
                content
            }
        }
    }
}
fn rows(app: &App, list: List) -> &Arc<Vec<Item>> {
    match list {
        List::Main => &session(app).rows,
        List::Child => &app.funnel.child_rows,
        List::Context => &app.funnel.context_rows,
    }
}
fn file_list(app: &App, p: &Paint, list: List) -> El {
    let count = rows(app, list).len();
    let key = match list {
        List::Main => list_key(app),
        List::Child => "child-scroll".into(),
        List::Context => "context-scroll".into(),
    };
    let look = p.look;
    let version = Arc::as_ptr(rows(app, list)) as usize;
    let stack = col()
        .key(format!("{key}-rows"))
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            count,
            32.0,
            move |a: &App, i| rows(a, list)[i].id.key().into(),
            move |a: &App, i| {
                file_row(
                    a,
                    &Paint {
                        look,
                        kit: look.kit(),
                    },
                    list,
                    &rows(a, list)[i],
                )
            },
        )
        .virtual_rows_version(version);
    let mut viewport = col()
        .key(key)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(stack);
    if list == List::Main {
        let scroll = session(app).scroll.clone();
        viewport = viewport.on_scroll_offset(move |pos| scroll.set(pos.offset[1]));
        viewport = viewport.key(list_key(app));
    }
    let chrome = match list {
        List::Main => session(app).chrome.clone(),
        List::Child => app.funnel.child_chrome.clone(),
        List::Context => app.funnel.context_chrome.clone(),
    };
    viewport = scroll::wrap(
        viewport,
        match list {
            List::Main => list_key(app),
            List::Child => "child-scroll".into(),
            List::Context => "context-scroll".into(),
        },
        p.look.muted.alpha(0.55),
        chrome,
    );
    if list == List::Main {
        // A separate focus node gives the query and rows clear keyboard ownership.
        viewport = col()
            .key(format!("{}-main-list", prefix(app)))
            .role("listbox")
            .label("Files")
            .focusable(true)
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(viewport);
    }
    viewport
}
fn file_row(app: &App, p: &Paint, list: List, item: &Item) -> El {
    let id = item.id.clone();
    let click = id.clone();
    let open = id.clone();
    let drag = id.clone();
    let selected = match list {
        List::Main => session(app).selected.contains(&id),
        List::Context => focused_path(app).as_ref() == Some(&id.0),
        List::Child => false,
    };
    let ink = if selected {
        p.look.selection_text
    } else {
        p.look.text
    };
    let single = list == List::Main && selected && session(app).selected.len() == 1;
    let mut r = row()
        .role("option")
        .focus_selected(selected)
        .label(item.name.clone())
        .w_full()
        .h(32.0)
        .px(10.0)
        .gap(8.0)
        .items_center()
        .fill(if selected && !single {
            p.look.selection
        } else {
            rune::Color::TRANSPARENT
        })
        .hover(p.look.selection.alpha(0.55))
        .motion(rune::tok::motion::SNAPPY)
        .children(if single {
            vec![
                el().key("selection-highlight")
                    .matched(format!("{}-selection-highlight", prefix(app)))
                    .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .fill(p.look.selection)
                    .motion(rune::tok::motion::SNAPPY)
                    .inert(),
            ]
        } else {
            Vec::new()
        })
        .child(file_icon(p, item.kind, selected))
        .child(
            text(&item.name)
                .font(rune::tok::type_::BODY)
                .color(ink)
                .spans(
                    item.highlights
                        .iter()
                        .map(|r| rune::Span::new(r.start, r.end).bold())
                        .collect::<Vec<_>>(),
                )
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        );
    if item.symlink {
        r = r.child(text("↗").font(rune::tok::type_::LABEL).color(p.look.muted));
    }
    if list != List::Context {
        r = r.child(
            text(model::size_label(item.size))
                .font(rune::tok::type_::LABEL)
                .color(if selected { ink } else { p.look.muted }),
        );
    }
    if item.kind == DocumentKind::Folder {
        r = r.child(text("›").color(if selected { ink } else { p.look.muted }));
    }
    r.transition(Transition::opacity())
        .on_press_tx(move |a: &mut App, tx| {
            if !session(a).usable {
                return;
            }
            match list {
                List::Main => select(a, click.clone(), tx.mods(), tx, false),
                List::Context => {
                    session_mut(a).neighbor = rows(a, list).iter().find(|i| i.id == click).cloned();
                }
                List::Child => {
                    if let Some(parent) = click.0.parent() {
                        open_folder(a, parent.to_owned(), tx);
                        select(a, click.clone(), Mods::default(), tx, false);
                    }
                }
            }
        })
        .on_double_press_tx(move |a: &mut App, tx| {
            if session(a).usable {
                crate::opening::open_path(a, &open.0, tx);
            }
        })
        .drag_out(move |a: &App| {
            if !session(a).usable {
                return None;
            }
            let files = if list == List::Main && session(a).selected.contains(&drag) {
                selection(a)
            } else {
                vec![drag.0.clone()]
            };
            Some(
                files
                    .into_iter()
                    .fold(DragItem::new().within_app(), |d, p| d.file(p)),
            )
        })
}
pub fn file_icon(p: &Paint, kind: DocumentKind, selected: bool) -> El {
    let ink = if selected {
        p.look.selection_text
    } else {
        p.look.accent
    };
    if kind == DocumentKind::Folder {
        el().key("file-icon")
            .size(18.0, 16.0)
            .child(
                el().pin(Some(0.0), Some(1.0), None, None)
                    .size(8.0, 5.0)
                    .radius(1.0)
                    .fill(ink.alpha(0.8)),
            )
            .child(
                el().pin(Some(0.0), Some(4.0), None, None)
                    .size(18.0, 12.0)
                    .radius(2.0)
                    .fill(ink.alpha(0.75)),
            )
    } else {
        let mark = match kind {
            DocumentKind::Pdf => "P",
            DocumentKind::Markdown => "M",
            DocumentKind::Word => "W",
            DocumentKind::Text => "‹›",
            DocumentKind::Image => "◇",
            DocumentKind::Video => "▸",
            DocumentKind::Audio => "♪",
            DocumentKind::Package => "▧",
            _ => "",
        };
        el().key("file-icon")
            .size(16.0, 19.0)
            .radius(2.0)
            .border(1.0, ink.alpha(0.6))
            .center()
            .child(text(mark).font(rune::tok::type_::LABEL).color(ink))
    }
}
fn narrow(app: &mut App, path: Option<FolderID>) {
    let previous = session(app).scope.clone();
    session_mut(app).scope_history.push(previous);
    session_mut(app).scope = path;
    submit_query(app);
}
fn scope_rail(app: &App, p: &Paint) -> El {
    let s = session(app);
    let root_title = if s.local {
        name(&s.folder.0)
    } else {
        "This Mac".into()
    };
    let look = p.look;
    let mut rail = col()
        .key("scope-rail")
        .w_full()
        .min_h(0.0)
        .fill(p.look.sidebar)
        .border_sides([0.0, 1.0, 0.0, 0.0], p.look.line)
        .child(crate::shell::pane_title(
            p,
            "scope-title",
            "Narrow the search",
            None,
            false,
        ))
        .child(
            crate::shell::shell_button(p, "scope-mac", &root_title, s.scope.is_none()).on_press(
                |a: &mut App| {
                    narrow(a, None);
                },
            ),
        );
    if s.scope.is_some() {
        rail = rail.child(
            crate::shell::shell_button(p, "scope-back", "Broaden", false).on_press(
                |a: &mut App| {
                    let previous = session_mut(a).scope_history.pop().flatten();
                    session_mut(a).scope = previous;
                    submit_query(a);
                },
            ),
        );
    }
    let scopes = col()
        .key("scope-rows")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            s.scope_children.len(),
            30.0,
            |a: &App, i| session(a).scope_children[i].to_string_lossy(),
            move |a: &App, i| {
                let path = session(a).scope_children[i].clone();
                let p = Paint {
                    look,
                    kit: look.kit(),
                };
                row()
                    .w_full()
                    .h(30.0)
                    .px(10.0)
                    .gap(7.0)
                    .items_center()
                    .hover(look.selection.alpha(0.5))
                    .child(file_icon(&p, DocumentKind::Folder, false))
                    .child(
                        text(name(&path))
                            .font(rune::tok::type_::LABEL)
                            .color(look.text)
                            .truncate()
                            .grow(1.0)
                            .min_w(0.0),
                    )
                    .on_press(move |a: &mut App| narrow(a, Some(FolderID::new(&path))))
            },
        );
    rail.child(
        col()
            .key("scope-scroll")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(scopes)
            .scroll_bar(&p.kit),
    )
}
fn context_pane(app: &App, p: &Paint) -> El {
    let mut pane = col()
        .key("context-pane")
        .w_full()
        .min_h(0.0)
        .border_sides([1.0, 1.0, 0.0, 0.0], p.look.line)
        .child(crate::shell::pane_title(
            p,
            "context-title",
            "Where it lives",
            None,
            false,
        ));
    if let Some(parent) = &app.funnel.context_key {
        let mut paths = vec![parent.clone()];
        let mut at = parent.as_path();
        for _ in 0..2 {
            if let Some(p) = at.parent() {
                paths.push(p.to_owned());
                at = p;
            } else {
                break;
            }
        }
        paths.reverse();
        for (i, path) in paths.into_iter().enumerate() {
            let title = if path == Path::new("/") {
                "This Mac".into()
            } else {
                name(&path)
            };
            pane = pane.child(
                row()
                    .key(format!("context-ancestor-{}", path.display()))
                    .w_full()
                    .h(26.0)
                    .pl(10.0 + i as f32 * 9.0)
                    .pr(8.0)
                    .hover(p.look.selection.alpha(0.5))
                    .child(
                        text(title)
                            .font(rune::tok::type_::LABEL)
                            .color(p.look.muted)
                            .truncate(),
                    )
                    .on_press(move |a: &mut App| narrow(a, Some(FolderID::new(&path)))),
            );
        }
    }
    if let Some(error) = &app.funnel.context_error {
        return pane.child(empty(p, error));
    }
    pane.child(if app.funnel.context_rows.is_empty() {
        empty(
            p,
            if app.funnel.context.is_pending() {
                "Loading nearby entries…"
            } else {
                "Select a result to see nearby files"
            },
        )
    } else {
        file_list(app, p, List::Context)
    })
}
fn place(app: &App, p: &Paint, key: &str, label: &str, path: PathBuf) -> El {
    let dest = path.clone();
    let active = session(app).folder.0 == path;
    crate::shell::shell_button(p, key, label, active)
        .on_press_tx(move |a: &mut App, tx| open_folder(a, path.clone(), tx))
        .on_drop_files(move |a: &mut App, files, tx| {
            crate::transfers::start_copy(a, files.to_vec(), dest.clone(), tx)
        })
}
fn places(app: &App, p: &Paint) -> El {
    let mut bar = row()
        .key("places-bar")
        .w_full()
        .h(36.0)
        .px(8.0)
        .gap(3.0)
        .items_center()
        .fill(p.look.sidebar)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.look.line);
    if app.funnel.path_editing {
        return bar.child(
            TextInput::new(&app.funnel.path_edit)
                .label("Folder path")
                .height(28.0)
                .on_edit(|a: &mut App| &mut a.funnel.path_edit)
                .on_submit(|a: &mut App, tx| {
                    let raw = a.funnel.path_edit.text.clone();
                    let path = if raw == "~" {
                        crate::app::home()
                    } else if let Some(s) = raw.strip_prefix("~/") {
                        crate::app::home().join(s)
                    } else {
                        PathBuf::from(raw)
                    };
                    a.funnel.path_editing = false;
                    open_folder(a, standardized(&path), tx);
                })
                .on_key(|a: &mut App, ev, tx| {
                    if ev.key == Key::Escape {
                        a.funnel.path_editing = false;
                        focus_section(a, tx);
                        true
                    } else {
                        false
                    }
                })
                .build(&p.kit)
                .key("folder-path")
                .grow(1.0),
        );
    }
    bar = bar
        .child(
            crate::shell::shell_button(p, "folder-back", "‹", false)
                .opacity(if app.funnel.history_at > 0 { 1.0 } else { 0.35 })
                .help("Back · ⌘[")
                .on_press_tx(|a: &mut App, tx| travel(a, -1, tx)),
        )
        .child(
            crate::shell::shell_button(p, "folder-forward", "›", false)
                .opacity(if app.funnel.history_at + 1 < app.funnel.history.len() {
                    1.0
                } else {
                    0.35
                })
                .help("Forward · ⌘]")
                .on_press_tx(|a: &mut App, tx| travel(a, 1, tx)),
        );
    let home = crate::app::home();
    bar = bar
        .child(place(app, p, "place-home", "Home", home.clone()))
        .child(place(
            app,
            p,
            "place-downloads",
            "Downloads",
            home.join("Downloads"),
        ))
        .child(place(
            app,
            p,
            "place-desktop",
            "Desktop",
            home.join("Desktop"),
        ));
    if let Some(pin) = &app.funnel.pinned {
        bar = bar.child(place(app, p, "place-pinned", &name(&pin.0), pin.0.clone()));
    }
    for volume in &app.funnel.mounted {
        bar = bar.child(place(
            app,
            p,
            &format!("volume-{}", volume.path.display()),
            &volume.name,
            volume.path.clone(),
        ));
        if volume.ejectable {
            let path = volume.path.clone();
            bar = bar.child(
                crate::shell::shell_button(p, &format!("eject-{}", path.display()), "⏏", false)
                    .on_press(move |a: &mut App| {
                        // Ejection may block on a mounted device; it shares the background opening queue.
                        eject_volume(a, path.clone());
                    }),
            );
        }
    }
    bar.child(el().grow(1.0))
        .child(
            crate::shell::shell_button(p, "folder-refresh", "↻", false)
                .help("Refresh · ⌘R")
                .on_press(|a: &mut App| {
                    let folder = a.funnel.folders.folder.clone();
                    a.funnel.library.refresh(&folder);
                }),
        )
        .child(
            crate::shell::shell_button(p, "folder-pin", "Pin", false)
                .help("Pin current folder")
                .on_press(pin_current),
        )
}
fn eject_volume(app: &mut App, path: PathBuf) {
    crate::opening::eject_volume(app, path);
}

/// A native picker for the palette's pin-folder action.
pub fn choose_pinned_folder(app: &mut App, tx: &mut Tx) {
    let options = rune::FileDialogOptions {
        title: "Pin a folder".into(),
        directory: current_folder(app),
        ..Default::default()
    };
    tx.open_folder(options, |a: &mut App, result, _| match result {
        Ok(Some(path)) => {
            let folder = FolderID::new(&path);
            a.funnel.pinned = Some(folder.clone());
            let _ = a.prefs.set("pinnedFolder", &folder.0);
        }
        Ok(None) => {}
        Err(error) => a.toast(error.to_string()),
    });
}

/// Numeric and path diagnostics for the shell's optional state report.
pub fn report(app: &App) -> serde_json::Value {
    let s = session(app);
    serde_json::json!({"folder":s.folder.0,"query":s.query.text,"folder_mode":s.local,"scope":s.scope.as_ref().map(|f|&f.0),"selection":selection(app),"focused":focused_path(app),"usable":s.usable,"complete":s.complete,"rows":s.rows.iter().map(|i|&i.id.0).collect::<Vec<_>>(),"history":app.funnel.history.iter().map(|f|&f.0).collect::<Vec<_>>(),"history_at":app.funnel.history_at})
}

fn sort_header(app: &App, p: &Paint) -> El {
    row()
        .key("folder-columns")
        .w_full()
        .h(24.0)
        .px(10.0)
        .items_center()
        .justify_between()
        .fill(p.look.raised)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.look.line)
        .child(
            text("Name")
                .font(rune::tok::type_::LABEL)
                .color(p.look.muted)
                .grow(1.0)
                .on_press(|a: &mut App| set_sort(a, SortField::Name)),
        )
        .child(
            crate::shell::shell_button(
                p,
                "folder-sort",
                &format!(
                    "Sort by {} {}",
                    session(app).sort.field.title(),
                    if session(app).sort.descending {
                        "↓"
                    } else {
                        "↑"
                    }
                ),
                false,
            )
            .h(22.0)
            .on_press_tx(|a: &mut App, tx| {
                let mut menu = rune::PopupMenu::new();
                for (i, field) in SortField::ALL.into_iter().enumerate() {
                    menu = menu.item(
                        rune::PopupItem::new(i.to_string(), field.title())
                            .checked(session(a).sort.field == field),
                    );
                }
                tx.popup_menu(menu, "folder-sort", |a: &mut App, picked, _| {
                    if let Some(field) = picked
                        .and_then(|p| p.parse::<usize>().ok())
                        .and_then(|i| SortField::ALL.get(i))
                        .copied()
                    {
                        set_sort(a, field);
                    }
                });
            }),
        )
}
fn set_sort(app: &mut App, field: SortField) {
    let s = session_mut(app);
    s.sort = Sort {
        field,
        descending: if field == s.sort.field {
            !s.sort.descending
        } else {
            matches!(
                field,
                SortField::Modified | SortField::Created | SortField::Size
            )
        },
    };
    if let Some(source) = &s.source_rows {
        let mut rows = source.as_ref().clone();
        s.sort.apply(&mut rows);
        s.rows = Arc::new(rows);
        s.revision += 1;
    }
}

/// Called after a successful open so the empty Home view refreshes on that worker's wake.
pub fn refresh_recent_files(app: &mut App) {
    app.funnel.recent_key = app.opening.recent.clone();
    app.funnel
        .recent
        .request(app.opening.recent.clone(), |paths| {
            paths
                .iter()
                .filter_map(|p| Item::read(p).ok())
                .filter(|i| i.kind != DocumentKind::Folder)
                .take(8)
                .collect()
        });
}
