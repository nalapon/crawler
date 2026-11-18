//! Search and Folders share the funnel, with independent sessions and one directory library.
mod bulk;
mod library;
pub mod model;
mod scroll;

use crate::app::{App, Paint, Section};
use crate::args::Launch;
use crate::index::{Query, Response, Scope};
use crate::opening::platform::{self, Volume};
use crate::shell::PlaceKind;
use library::{Library, Listing};
use model::{DocumentKind, FolderID, Item, ItemID, Sort, SortField, name};
use rune::prefs::Prefs;
use rune::{
    Cx, DragItem, El, GrowthSpec, Key, KeyEvent, Latest, Mods, ScrollSpec, TextEdit, Tx, Waker,
    col, el, row, text,
};
use rune_kit::{ScrollBar, TextInput};
use std::cell::Cell;
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::Duration;

const WIDE_PLACES: f32 = 1040.0;
const FOLDER_ROW_HEIGHT: f32 = 44.0;

struct Session {
    folder: FolderID,
    query: TextEdit,
    local: bool,
    scope: Option<FolderID>,
    scope_history: Vec<Option<FolderID>>,
    rows: Arc<Vec<Item>>,
    source_rows: Option<Arc<Vec<Item>>>,
    order: Option<Vec<usize>>,
    sort: Sort,
    selected: BTreeSet<ItemID>,
    focused: Option<ItemID>,
    focus_at: Option<usize>,
    neighbor: Option<Item>,
    anchor: Option<ItemID>,
    scroll: Rc<Cell<f32>>,
    saved_scroll: f32,
    chrome: Rc<scroll::Chrome>,
    query_focused: bool,
    request: Option<u64>,
    published_query: Option<u64>,
    usable: bool,
    complete: bool,
    pending: Option<Response>,
    last_publication: f64,
    has_nonempty: bool,
    scope_counts: Vec<(PathBuf, usize)>,
    revision: u64,
    scope_revision: u64,
    g_at: Option<f64>,
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
            order: None,
            sort: Sort::default(),
            selected: BTreeSet::new(),
            focused: None,
            focus_at: None,
            neighbor: None,
            anchor: None,
            scroll: Rc::new(Cell::new(0.0)),
            saved_scroll: 0.0,
            chrome: Rc::new(scroll::Chrome::default()),
            query_focused,
            request: None,
            published_query: None,
            usable: true,
            complete: false,
            pending: None,
            last_publication: 0.0,
            has_nonempty: false,
            scope_counts: Vec::new(),
            revision: 0,
            scope_revision: 0,
            g_at: None,
        }
    }
    fn searching(&self) -> bool {
        !self.query.text.trim().is_empty()
    }
    fn row(&self, at: usize) -> &Item {
        &self.rows[self.order.as_ref().map_or(at, |order| order[at])]
    }
    fn position(&self, id: &ItemID) -> Option<usize> {
        (0..self.rows.len()).find(|&at| &self.row(at).id == id)
    }
    fn focused_item(&self) -> Option<&Item> {
        self.neighbor
            .as_ref()
            .or_else(|| self.focus_at.map(|at| self.row(at)))
    }
    fn reconcile_selection(&mut self) {
        self.focus_at = self.focused.as_ref().and_then(|id| self.position(id));
        if self.focus_at.is_none() {
            self.focus_at = (!self.rows.is_empty()).then_some(0);
            self.focused = self.focus_at.map(|at| self.row(at).id.clone());
            self.selected.clear();
            self.anchor = self.focused.clone();
        } else if self.selected.len() > 1 {
            let ids: HashSet<_> = self.rows.iter().map(|i| &i.id).collect();
            self.selected.retain(|id| ids.contains(id));
        } else {
            self.selected.clear();
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

struct ParkedListing {
    folder: FolderID,
    rows: Arc<Vec<Item>>,
    view_id: u64,
}

impl ParkedListing {
    fn from_session(s: &Session, view_id: u64) -> Option<Self> {
        let count = s.rows.len();
        // Bound the extra Rune key strings and per-row index state, even for long paths.
        let bytes_per_row = (s.folder.0.as_os_str().len() + 256) * 3 + 96;
        (s.usable
            && !s.searching()
            && s.sort == Sort::default()
            && (1..=50_000).contains(&count)
            && count * bytes_per_row <= 48 * 1024 * 1024)
            .then(|| Self {
                folder: s.folder.clone(),
                rows: s.rows.clone(),
                view_id,
            })
    }
}

pub struct State {
    search: Session,
    folders: Session,
    library: Library,
    folder_view_id: u64,
    parked: Option<ParkedListing>,
    sorted: Latest<Vec<usize>>,
    sort_key: Option<(Sort, u64)>,
    child: Option<FolderID>,
    child_rows: Arc<Vec<Item>>,
    child_revision: u64,
    child_selected: Option<(ItemID, usize)>,
    child_scroll_reset: bool,
    child_chrome: Rc<scroll::Chrome>,
    context_chrome: Rc<scroll::Chrome>,
    pinned: Option<FolderID>,
    history: Vec<FolderID>,
    history_at: usize,
    folder_focus: VecDeque<(FolderID, ItemID, f32)>,
    folder_query_origin: Option<(Option<ItemID>, f32)>,
    reveal: Option<ItemID>,
    path_edit: Option<TextEdit>,
    drives_expanded: bool,
    recent: Latest<Vec<Item>>,
    recent_key: Vec<PathBuf>,
    recent_rows: Arc<Vec<Item>>,
    context: Latest<Context>,
    context_key: Option<PathBuf>,
    context_selected: Option<ItemID>,
    context_rows: Arc<Vec<Item>>,
    context_error: Option<String>,
    context_cache: Vec<Context>,
    volumes: Latest<Vec<Volume>>,
    mounted: Vec<Volume>,
    volume_events: mpsc::Receiver<()>,
    volume_sender: mpsc::Sender<()>,
    volume_observer: Option<platform::WorkspaceObserver>,
    waker: Option<Waker>,
    focus_pending: bool,
    restore_scroll: bool,
    browse_revision: u64,
    divider: Option<f32>,
    panel_width: f32,
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
            folder_view_id: 0,
            parked: None,
            sorted: Latest::default(),
            sort_key: None,
            child: None,
            child_rows: Arc::new(Vec::new()),
            child_revision: 0,
            child_selected: None,
            child_scroll_reset: false,
            child_chrome: Rc::new(scroll::Chrome::default()),
            context_chrome: Rc::new(scroll::Chrome::default()),
            pinned: prefs
                .get::<PathBuf>("pinnedFolder")
                .map(|p| FolderID::new(&p)),
            history: vec![FolderID::new(&folder)],
            history_at: 0,
            folder_focus: VecDeque::new(),
            folder_query_origin: None,
            reveal: None,
            path_edit: None,
            drives_expanded: false,
            recent: Latest::default(),
            recent_key: Vec::new(),
            recent_rows: Arc::new(Vec::new()),
            context: Latest::default(),
            context_key: None,
            context_selected: None,
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
            divider: None,
            panel_width: 0.0,
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
    if browsing(app) {
        folder_list_key(app.funnel.folder_view_id)
    } else {
        format!("{}-main-scroll", prefix(app))
    }
}

fn folder_list_key(view_id: u64) -> String {
    format!("folders-main-scroll-{view_id}")
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
    s.sorted.set_waker(waker.clone());
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
    app.funnel.path_edit = None;
    if matches!(from, Section::Search | Section::Folders) {
        app.funnel.parked = None;
        let s = app.funnel.session_mut(from);
        s.query_focused = s.query.focused;
        s.saved_scroll = s.scroll.get();
        s.chrome.hide_keyboard();
        app.index.idle();
        app.funnel.library.visible(Vec::new(), None, false);
        app.funnel.context.cancel();
        app.funnel.context_key = None;
        app.funnel.sorted.cancel();
        app.funnel.sort_key = None;
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
        tx.scroll_to_instant(list_key(app), 0.0, session(app).saved_scroll);
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
    open_folder_at(app, folder, None, tx);
}
fn open_folder_at(app: &mut App, folder: PathBuf, reveal: Option<ItemID>, tx: &mut Tx) {
    app.show(Section::Folders);
    browse(app, FolderID::new(&folder), true, reveal, tx);
}
fn browse(app: &mut App, folder: FolderID, record: bool, reveal: Option<ItemID>, tx: &mut Tx) {
    crate::opening::cancel_navigation(app);
    app.funnel.path_edit = None;
    if app.funnel.folders.folder != folder {
        let next_view = app
            .funnel
            .parked
            .as_ref()
            .filter(|parked| parked.folder == folder)
            .map_or(app.funnel.browse_revision + 1, |parked| parked.view_id);
        app.funnel.parked =
            ParkedListing::from_session(&app.funnel.folders, app.funnel.folder_view_id);
        app.funnel.folder_view_id = next_view;
    }
    if !app.funnel.folders.searching()
        && app.funnel.folders.usable
        && let Some(id) = app.funnel.folders.focused.clone()
    {
        let previous = &app.funnel.folders;
        app.funnel
            .folder_focus
            .retain(|(path, _, _)| *path != previous.folder);
        app.funnel
            .folder_focus
            .push_front((previous.folder.clone(), id, previous.scroll.get()));
        app.funnel.folder_focus.truncate(32);
    }
    let remembered = app
        .funnel
        .folder_focus
        .iter()
        .find(|(path, _, _)| *path == folder);
    let offset = remembered.map_or(0.0, |(_, _, y)| *y);
    app.funnel.folder_query_origin = None;
    app.funnel.reveal = reveal.clone();

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
    s.scope_history.clear();
    s.focused = reveal.or_else(|| remembered.map(|(_, id, _)| id.clone()));
    s.focus_at = None;
    s.neighbor = None;
    s.selected.clear();
    if let Some(id) = &s.focused {
        s.selected.insert(id.clone());
    }
    s.anchor = None;
    s.rows = Arc::new(Vec::new());
    s.source_rows = None;
    s.order = None;
    s.scroll.set(offset);
    s.saved_scroll = offset;
    s.usable = false;
    s.request = None;
    s.pending = None;
    s.chrome.reset_anchor();
    s.chrome.hide_keyboard();
    s.revision += 1;
    app.funnel.browse_revision += 1;
    app.mark_folder_input(app.funnel.browse_revision, tx);
    app.index.idle();
    prepare_browsing(app);
    tx.scroll_to_instant(list_key(app), 0.0, offset);
    app.funnel.restore_scroll = true;
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
        browse(app, folder, false, None, tx);
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
    if s.selected.len() <= 1 {
        return s.selected.iter().map(|i| i.0.clone()).collect();
    }
    (0..s.rows.len())
        .map(|at| s.row(at))
        .filter(|i| s.selected.contains(&i.id))
        .map(|i| i.id.0.clone())
        .collect()
}
pub fn selected_item(app: &App) -> Option<&Item> {
    if active(app) {
        session(app).focused_item()
    } else {
        None
    }
}
pub fn focused_path(app: &App) -> Option<PathBuf> {
    if !active(app) {
        return None;
    }
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
    active(app).then(|| session(app).folder.0.clone())
}
pub fn pin_current(app: &mut App) {
    pin_folder(app, session(app).folder.0.clone());
}
pub fn pin_folder(app: &mut App, path: PathBuf) {
    let folder = FolderID::new(&path);
    app.funnel.pinned = Some(folder.clone());
    if let Err(error) = app.set_preference("pinnedFolder", &folder.0) {
        app.toast(error.to_string());
    }
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

pub(crate) fn on_external_volume(app: &App, path: &Path) -> bool {
    app.funnel.mounted.iter().any(|v| path.starts_with(&v.path))
}

pub fn frame_search_result(app: &App) -> Option<u64> {
    let s = session(app);
    (active(app) && s.searching() && s.usable && !s.rows.is_empty())
        .then_some(s.published_query)
        .flatten()
}

pub fn frame_folder(app: &App) -> Option<(&Path, u64)> {
    (active(app) && browsing(app) && session(app).usable)
        .then(|| (session(app).folder.0.as_path(), app.funnel.browse_revision))
}

fn consume_local(app: &mut App) {
    let s = session_mut(app);
    if s.query.text.starts_with('$') {
        let text = s.query.text.trim_start_matches('$').to_owned();
        s.query = TextEdit::new(text);
        s.local = true;
        s.scope = None;
        s.scope_history.clear();
    }
    if s.query.text.is_empty() && !s.local {
        s.scope = None;
    }
}
fn query_changed(app: &mut App, tx: &mut Tx) {
    session_mut(app).query_focused = true;
    let was_local = session(app).local;
    let entered_prefix = session(app).query.text.starts_with('$');
    consume_local(app);
    if session(app).query.text.trim().is_empty() {
        if was_local && !entered_prefix {
            session_mut(app).local = false;
        }
        let s = session_mut(app);
        s.request = None;
        s.pending = None;
        s.usable = true;
        s.scope = None;
        s.scope_counts.clear();
        s.scope_revision += 1;
        s.scope_history.clear();
        s.neighbor = None;
        s.chrome.reset_anchor();
        s.revision += 1;
        app.index.idle();
        if app.section == Section::Folders
            && let Some((focus, offset)) = app.funnel.folder_query_origin.take()
        {
            let s = &mut app.funnel.folders;
            s.focused = focus;
            s.focus_at = None;
            s.source_rows = None;
            s.saved_scroll = offset;
            app.funnel.restore_scroll = true;
        }
        prepare_browsing(app);
    } else {
        submit_query(app);
        if let Some(id) = session(app).request {
            app.mark_search_input(id, tx);
        }
    }
}
fn submit_query(app: &mut App) {
    if !session(app).searching() {
        return;
    }
    if app.section == Section::Folders && app.funnel.folder_query_origin.is_none() {
        let s = &app.funnel.folders;
        app.funnel.folder_query_origin = Some((s.focused.clone(), s.scroll.get()));
    }
    app.funnel.parked = None;
    app.funnel.reveal = None;
    let root = search_root(app).0;
    let s = session_mut(app);
    if s.scope
        .as_ref()
        .is_some_and(|scope| !scope.0.starts_with(&root))
    {
        s.scope = None;
        s.scope_history.clear();
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
fn publish(app: &mut App, response: Response, tx: &mut Tx) -> bool {
    let id = response.id;
    let started = std::time::Instant::now();
    app.index.mark_stage(id, "publication_ms");
    let clock = app.clock;
    let s = session_mut(app);
    s.source_rows = None;
    let order_changed = s.order.take().is_some();
    let rows: Vec<_> = response.hits.iter().map(Item::from_hit).collect();
    let rows_changed = s.rows.as_ref() != &rows;
    let scopes_changed = !s
        .scope_counts
        .iter()
        .map(|(path, _)| path)
        .eq(response.scope_counts.iter().map(|(path, _)| path));
    let changed = order_changed
        || rows_changed
        || scopes_changed
        || !s.usable
        || s.complete != response.complete;
    if scopes_changed {
        s.scope_revision += 1;
    }
    s.scope_counts = response.scope_counts;
    if order_changed
        || rows_changed
            && !s
                .rows
                .iter()
                .map(|item| &item.id)
                .eq(rows.iter().map(|item| &item.id))
    {
        s.revision += 1;
    }
    if rows_changed {
        s.rows = Arc::new(rows);
    }
    // A newer immediate publication supersedes a response waiting in the 40 ms window.
    // Otherwise that older response can roll the rows and completion state backwards.
    s.pending = None;
    if s.published_query != Some(id) {
        s.neighbor = None;
    }
    s.usable = true;
    s.published_query = Some(id);
    s.complete = response.complete;
    s.has_nonempty |= !s.rows.is_empty();
    s.last_publication = clock;
    s.reconcile_selection();
    // Rune anchors inserted virtual rows automatically. Until the user navigates or scrolls,
    // keep the first result visible instead of following an early partial hit down the ranking.
    if !s.chrome.has_interacted() {
        tx.scroll_to_instant(list_key(app), 0.0, 0.0);
    }
    app.index.mark_stage(id, "first_rows_ms");
    app.index.trace_stage(
        id,
        "row_update_work_ms",
        started.elapsed().as_secs_f64() * 1000.0,
    );
    changed
}
fn prepare_browsing(app: &mut App) {
    if browsing(app) {
        sync_directory(app);
        sync_child(app);
    } else if !session(app).searching() {
        let rows = app.funnel.recent_rows.clone();
        let s = session_mut(app);
        if !Arc::ptr_eq(&s.rows, &rows) || s.order.is_some() {
            s.revision += 1;
        }
        s.order = None;
        s.rows = rows;
        s.usable = true;
        s.reconcile_selection();
    }
}
fn sync_directory(app: &mut App) {
    let folder = app.funnel.folders.folder.clone();
    let f = &mut app.funnel;
    let s = &mut f.folders;
    let Some(Listing::Ready(rows)) = f.library.listing(&folder) else {
        if s.source_rows.is_some() || !s.rows.is_empty() {
            s.source_rows = None;
            s.order = None;
            s.rows = Arc::default();
            s.focus_at = None;
            s.revision += 1;
        }
        f.sorted.cancel();
        f.sort_key = None;
        s.usable = false;
        return;
    };
    let source_changed = !s
        .source_rows
        .as_ref()
        .is_some_and(|old| Arc::ptr_eq(old, rows));
    if source_changed {
        s.source_rows = Some(rows.clone());
        s.order = None;
        s.rows = rows.clone();
        s.reconcile_selection();
        s.revision += 1;
    }
    s.usable = true;
    let totals = f.library.sizes(&folder);
    let size_revision = if s.sort.field == SortField::Size {
        totals.map_or(0, |(_, r)| r)
    } else {
        0
    };
    let key = (s.sort, size_revision);
    if source_changed || f.sort_key != Some(key) {
        f.sort_key = Some(key);
        if s.sort == Sort::default() {
            f.sorted.cancel();
            if s.order.take().is_some() {
                s.revision += 1;
                s.reconcile_selection();
            }
        } else {
            let sort = s.sort;
            let sizes = totals
                .filter(|_| sort.field == SortField::Size)
                .map(|(s, _)| s.clone());
            f.sorted.request(rows.clone(), move |source| {
                sort.order(&source, sizes.as_deref())
            });
        }
    }
    if let Some(order) = f.sorted.take() {
        s.order = Some(order);
        s.revision += 1;
        s.reconcile_selection();
    }
}
fn sync_child(app: &mut App) {
    let child = session(app)
        .focused_item()
        .filter(|i| i.kind == DocumentKind::Folder)
        .map(|i| FolderID(i.id.0.clone()));
    if app.funnel.child != child {
        app.funnel.child = child.clone();
        app.funnel.child_scroll_reset = true;
        app.funnel.child_selected = None;
    }
    let mut visible = Vec::new();
    let mut active_folder = None;
    if browsing(app) {
        let folder = session(app).folder.clone();
        visible.push(folder.clone());
        active_folder = Some(folder);
    }
    let rows = if let Some(child) = child {
        visible.push(child.clone());
        match app.funnel.library.listing(&child) {
            Some(Listing::Ready(rows)) => Some(rows.clone()),
            _ => None,
        }
    } else {
        None
    };
    if !rows
        .as_ref()
        .is_some_and(|rows| Arc::ptr_eq(rows, &app.funnel.child_rows))
        && (rows.is_some() || !app.funnel.child_rows.is_empty())
    {
        app.funnel.child_rows = rows.unwrap_or_default();
        app.funnel.child_revision += 1;
    }
    let external = active_folder
        .as_ref()
        .is_some_and(|f| on_external_volume(app, f.0.as_path()));
    app.funnel.library.visible(visible, active_folder, external);
}
fn update_context(app: &mut App) {
    if !session(app).usable {
        return;
    }
    if !session(app).searching() {
        if app.funnel.context_key.take().is_some() {
            app.funnel.context.cancel();
            app.funnel.context_selected = None;
        }
        return;
    }
    let selected = session(app).focused_item();
    let parent = selected.and_then(|i| i.id.0.parent());
    if parent == app.funnel.context_key.as_deref() {
        if selected.map(|i| &i.id) != app.funnel.context_selected.as_ref() {
            show_context_rows(app);
        }
        return;
    }
    let parent = parent.map(Path::to_owned);
    app.funnel.context_key = parent.clone();
    app.funnel.context_error = None;
    show_context_rows(app);
    if let Some(parent) = parent {
        app.funnel.context.request(parent, |parent| {
            let (rows, error) = match library::bounded_context(&parent) {
                Ok(rows) => (rows, None),
                Err(error) => (Vec::new(), Some(error)),
            };
            Context {
                rows: Arc::new(rows),
                parent,
                error,
            }
        });
    } else {
        app.funnel.context.cancel();
    }
}

// Keep the parent's bounded snapshot reusable when a different sibling is selected.
fn show_context_rows(app: &mut App) {
    let mut rows = app
        .funnel
        .context_cache
        .iter()
        .find(|c| Some(&c.parent) == app.funnel.context_key.as_ref())
        .map(|c| c.rows.clone())
        .unwrap_or_default();
    let selected = session(app).focused_item();
    if let Some(item) = selected
        && !rows.iter().any(|row| row.id == item.id)
    {
        Arc::make_mut(&mut rows).insert(0, item.clone());
    }
    app.funnel.context_selected = selected.map(|i| i.id.clone());
    app.funnel.context_rows = rows;
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
        refresh_recent_files(app);
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
        changed |= app.index.poll_status();
        changed |= directory_changed;
        if browsing(app) {
            let revision = session(app).revision;
            sync_directory(app);
            changed |= revision != session(app).revision;
            changed |= show_revealed_file(app, tx);
        }
        if let Some(response) = app.index.take() {
            if session(app).request == Some(response.id) && session(app).searching() {
                if (!session(app).has_nonempty && !response.hits.is_empty())
                    || app.clock - session(app).last_publication >= 0.040
                {
                    changed |= publish(app, response, tx);
                } else {
                    session_mut(app).pending = Some(response);
                }
            }
        }
        if session(app).pending.is_some() {
            let remaining = 0.040 - (app.clock - session(app).last_publication);
            if remaining <= 0.0 {
                if let Some(response) = session_mut(app).pending.take() {
                    changed |= publish(app, response, tx);
                }
            } else {
                tx.wake_after(Duration::from_secs_f64(remaining));
            }
        }
        sync_child(app);
        if std::mem::take(&mut app.funnel.child_scroll_reset) {
            app.funnel.child_chrome.hide_keyboard();
            tx.scroll_to_instant("child-scroll", 0.0, 0.0);
            changed = true;
        }
        update_context(app);
        if let Some(context) = app.funnel.context.take() {
            if app.funnel.context_key.as_ref() == Some(&context.parent) {
                changed = true;
                app.funnel.context_error = context.error.clone();
                if let Some(path) = focused_path(app) {
                    tx.scroll_into_view_in("context-scroll", ItemID::new(&path).key());
                }
                app.funnel
                    .context_cache
                    .retain(|c| c.parent != context.parent);
                app.funnel.context_cache.insert(0, context);
                app.funnel.context_cache.truncate(4);
                show_context_rows(app);
            }
        }
        if app.funnel.focus_pending {
            app.funnel.focus_pending = false;
            focus_section(app, tx);
        }
        if app.funnel.restore_scroll && (!browsing(app) || session(app).usable) {
            app.funnel.restore_scroll = false;
            tx.scroll_to_instant(list_key(app), 0.0, session(app).saved_scroll);
        }
    }
    if app.funnel.library.has_debounce() {
        tx.wake_after(Duration::from_millis(120));
    }
    changed
}

fn show_revealed_file(app: &mut App, tx: &mut Tx) -> bool {
    let Some(id) = app.funnel.reveal.as_ref() else {
        return false;
    };
    let s = session(app);
    if !s.usable || app.funnel.sorted.is_pending() {
        return false;
    }
    let Some(at) = s.position(id) else {
        if !app.funnel.library.is_reading(&s.folder) {
            app.funnel.reveal = None;
        }
        return false;
    };
    let key = id.key();
    let id = app.funnel.reveal.take().expect("pending reveal");
    let s = session_mut(app);
    let changed = s.focus_at != Some(at);
    s.focused = Some(id.clone());
    s.focus_at = Some(at);
    s.anchor = Some(id.clone());
    s.selected.clear();
    s.selected.insert(id);
    app.funnel.restore_scroll = false;
    tx.scroll_into_view_in(list_key(app), key);
    changed
}

fn select(app: &mut App, id: ItemID, mods: Mods, tx: &mut Tx, keyboard: bool) {
    if !session(app).usable {
        return;
    }
    if let Some(at) = session(app).position(&id) {
        select_at(app, at, mods, tx, keyboard);
    }
}
fn select_at(app: &mut App, at: usize, mods: Mods, tx: &mut Tx, keyboard: bool) {
    let multi = browsing(app);
    if multi {
        app.funnel.reveal = None;
    }
    let in_query = keyboard && session(app).query.focused;
    let s = session_mut(app);
    s.neighbor = None;
    let id = s.row(at).id.clone();
    if multi && mods.shift {
        let anchor = s.anchor.as_ref().and_then(|a| s.position(a)).unwrap_or(at);
        s.selected = (anchor.min(at)..=anchor.max(at))
            .map(|i| s.row(i).id.clone())
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
    s.focus_at = Some(at);
    s.chrome.mark_interacted();
    s.query_focused = in_query;
    if keyboard {
        session(app).chrome.hide_keyboard();
        session(app).chrome.mark_interacted();
        session(app)
            .chrome
            .reveal_row(at, row_height(app, List::Main), list_key(app), &id, tx);
    }
    if !in_query {
        tx.focus(format!("{}-main-list", prefix(app)), false);
    }
    sync_child(app);
    update_context(app);
    if session(app).searching() {
        tx.scroll_into_view_in("context-scroll", id.key());
    }
}
fn move_selection(app: &mut App, delta: isize, shift: bool, tx: &mut Tx) {
    let s = session(app);
    if !s.usable || s.rows.is_empty() {
        return;
    }
    let current = s.focus_at.unwrap_or(0);
    let at = current.saturating_add_signed(delta).min(s.rows.len() - 1);
    select_at(
        app,
        at,
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
            open_folder_at(app, parent.to_owned(), Some(ItemID::new(&path)), tx);
        }
    } else {
        let kind = session(app).focused_item().map(|i| i.kind);
        open_item(app, path, kind, tx);
    }
}
fn open_item(app: &mut App, path: PathBuf, kind: Option<DocumentKind>, tx: &mut Tx) {
    if kind == Some(DocumentKind::Folder) {
        crate::opening::cancel_navigation(app);
        crate::opening::dispatch(app, path, DocumentKind::Folder, tx);
    } else {
        crate::opening::open_path(app, &path, tx);
    }
}
fn enter_selected_folder(app: &mut App, tx: &mut Tx) {
    if !session(app).usable {
        return;
    }
    let item = session(app).focused_item();
    if let Some(item) = item.filter(|i| i.kind == DocumentKind::Folder) {
        open_item(app, item.id.0.clone(), Some(DocumentKind::Folder), tx);
    } else if !browsing(app)
        && let Some(path) = focused_path(app).and_then(|p| p.parent().map(Path::to_owned))
    {
        open_folder(app, path, tx);
    }
}

fn child_position(app: &App, id: &ItemID, index: usize) -> Option<usize> {
    let rows = &app.funnel.child_rows;
    if rows.get(index).is_some_and(|item| &item.id == id) {
        Some(index)
    } else {
        rows.iter().position(|item| &item.id == id)
    }
}

fn child_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !session(app).usable {
        return false;
    }
    let current = app
        .funnel
        .child_selected
        .as_ref()
        .and_then(|(id, at)| child_position(app, id, *at));
    let rows = &app.funnel.child_rows;
    match ev.key {
        Key::Up | Key::Down | Key::Home | Key::End if !rows.is_empty() => {
            let at = match ev.key {
                Key::Up => current.unwrap_or(0).saturating_sub(1),
                Key::Down => current.map_or(0, |at| (at + 1).min(rows.len() - 1)),
                Key::End => rows.len() - 1,
                _ => 0,
            };
            let id = rows[at].id.clone();
            tx.scroll_into_view_in("child-scroll", id.key());
            app.funnel.child_selected = Some((id, at));
        }
        Key::Enter | Key::Right if !rows.is_empty() => {
            let item = &rows[current.unwrap_or(0)];
            if ev.key == Key::Enter || item.kind == DocumentKind::Folder {
                open_item(app, item.id.0.clone(), Some(item.kind), tx);
            }
        }
        Key::Left | Key::Escape => tx.focus(format!("{}-main-list", prefix(app)), false),
        _ => return false,
    }
    true
}
fn parent(app: &mut App, tx: &mut Tx) {
    if session(app).searching() {
        if let Some(scope) = session(app).scope.clone() {
            let root = search_root(app).0;
            let p = scope
                .0
                .parent()
                .filter(|p| *p != root && p.starts_with(&root))
                .map(FolderID::new);
            session_mut(app).scope = p;
            submit_query(app);
        }
        return;
    }
    let old = session(app).folder.0.clone();
    if let Some(parent) = old.parent() {
        open_folder_at(app, parent.to_owned(), Some(ItemID::new(&old)), tx);
    }
}
fn half_page_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.mods.ctrl
        && !ev.mods.alt
        && let Key::Text(t) = &ev.key
    {
        if t.eq_ignore_ascii_case("d") || t.eq_ignore_ascii_case("u") {
            let count = session(app).chrome.half_page(row_height(app, List::Main));
            move_selection(
                app,
                if t.eq_ignore_ascii_case("d") {
                    count
                } else {
                    -count
                },
                false,
                tx,
            );
            return true;
        }
    }
    false
}
fn field_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if half_page_key(app, ev, tx) {
        return true;
    }
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

fn edit_path(app: &mut App, tx: &mut Tx) {
    let mut edit = TextEdit::new(session(app).folder.0.to_string_lossy());
    edit.set_selection(0, edit.text.len());
    app.funnel.path_edit = Some(edit);
    tx.focus_when_shown("folder-path", false);
}

fn refresh(app: &mut App) {
    let folder = session(app).folder.clone();
    crate::preview::invalidate_pane(app);
    app.funnel.library.refresh(&folder);
    if session(app).searching() {
        submit_query(app);
    }
}

pub fn on_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if half_page_key(app, ev, tx) {
        return true;
    }
    match &ev.key {
        Key::Command('[') if app.section == Section::Folders => {
            travel(app, -1, tx);
            return true;
        }
        Key::Command(']') if app.section == Section::Folders => {
            travel(app, 1, tx);
            return true;
        }
        Key::Command('l') | Key::Command('g')
            if app.section == Section::Folders
                && (ev.key == Key::Command('l') || ev.mods.shift) =>
        {
            edit_path(app, tx);
            return true;
        }
        Key::Command('r') => {
            refresh(app);
            return true;
        }
        _ => {}
    }
    if app.funnel.path_edit.is_some() || session(app).query.focused {
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
        Key::Text(t) if t == "/" || t == "i" => {
            session_mut(app).query_focused = true;
            tx.focus(query_key(app), false);
            true
        }
        Key::Text(t) if t == "G" => {
            move_selection(app, isize::MAX, false, tx);
            true
        }
        Key::Text(t) if t == "g" => {
            let clock = app.clock;
            if session(app).g_at.is_some_and(|at| clock - at <= 0.7) {
                session_mut(app).g_at = None;
                move_selection(app, isize::MIN, false, tx);
            } else {
                session_mut(app).g_at = Some(clock);
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
/// Status text for the status bar: the funnel state, then the spaced key-hint groups.
pub fn status_line(app: &App) -> String {
    const GAP: &str = "    ";
    let s = session(app);
    let hints = if s.query_focused {
        "↑/↓ Select    Esc Navigate    ↵ Open"
    } else if browsing(app) {
        "j/k Files    h/l Parent or child    / Search    Space Preview    ↵ Open"
    } else {
        "j/k Files    / Search    Space Preview    ↵ Open"
    };
    if browsing(app) {
        return match app.funnel.library.listing(&s.folder) {
            None | Some(Listing::Loading) => "Loading folder…".into(),
            Some(Listing::Failed(e)) => e.clone(),
            Some(Listing::Ready(_)) if s.selected.len() > 1 => {
                format!("{} selected{GAP}{hints}", s.selected.len())
            }
            Some(Listing::Ready(_)) => hints.into(),
        };
    }
    if !s.searching() {
        return hints.into();
    }
    if let crate::index::Status::Failed(error) = app.index.status() {
        return error;
    }
    let state = if !s.usable || s.rows.is_empty() && !s.complete {
        "Searching…".to_string()
    } else if !s.complete {
        format!("Searching… · {} shown", s.rows.len())
    } else if s.rows.len() >= 100 {
        "Showing the best 100 results · Narrow your search".to_string()
    } else {
        format!("{} results", s.rows.len())
    };
    format!("{state}{GAP}{hints}")
}

pub fn view(app: &App, cx: &mut Cx<App>, p: &Paint, _layer: &mut Vec<El>) -> El {
    let s = session(app);
    let folders = app.section == Section::Folders;
    let split = app.funnel.divider.unwrap_or(if s.searching() {
        0.43
    } else if folders {
        0.5
    } else {
        0.46
    });
    let mut body = col()
        .key("funnel")
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(p.look.background)
        .on_layout_tx(|a: &mut App, [_, _, width, _], tx| {
            if a.section != Section::Folders
                || (a.funnel.panel_width >= WIDE_PLACES) == (width >= WIDE_PLACES)
            {
                tx.live_only();
            }
            a.funnel.panel_width = width;
        });
    if folders {
        body = body.child(places(app, p));
    }
    let query = TextInput::new(&s.query)
        .placeholder("Find a file…")
        .label("Search files")
        .height(if folders { 28.0 } else { 32.0 })
        .text_style(p.look.font(if folders { 15.0 } else { 21.0 }, 400.0))
        .plain()
        .on_edit(|a: &mut App| &mut session_mut(a).query)
        .on_change(query_changed)
        .on_key(field_key)
        .build(&p.kit)
        .key(query_key(app))
        .w_full();
    let (_, root_title) = search_root(app);
    body = body.child(
        col()
            .key("search-field-band")
            .w_full()
            .h(if folders { 46.0 } else { 76.0 })
            .px(32.0)
            .center()
            .child(
                row()
                    .key("search-field-wrap")
                    .w_full()
                    .max_w(600.0)
                    .h(if folders { 34.0 } else { 48.0 })
                    .px(14.0)
                    .gap(8.0)
                    .items_center()
                    .border(1.0, if s.local { p.look.accent } else { p.look.muted })
                    .radius(if p.look.era() == crate::theme::Era::Modern {
                        8.0
                    } else {
                        0.0
                    })
                    .fill(if s.local {
                        p.look.accent.alpha(0.07)
                    } else {
                        p.look.background
                    })
                    .child(query.grow(1.0).min_w(0.0))
                    .child(
                        text(&root_title)
                            .key("search-scope-label")
                            .font(p.look.font(11.0, 500.0))
                            .color(if s.local { p.look.accent } else { p.look.muted })
                            .truncate()
                            .max_w(110.0)
                            .help(if s.local {
                                format!("Search in {}", model::display_path(&s.folder.0))
                            } else {
                                format!("Search in {root_title}")
                            })
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
    let detail = if s.searching() {
        let (root, title) = search_root(app);
        let relative = s
            .scope
            .as_ref()
            .and_then(|f| f.0.strip_prefix(&root).ok())
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| format!("  /  {}", p.display().to_string().replace('/', "  /  ")))
            .unwrap_or_default();
        Some(format!("in {title}{relative}"))
    } else {
        None
    };
    let main = col()
        .key(format!("{}-main-pane", prefix(app)))
        .grow(split)
        .min_w(180.0)
        .min_h(0.0)
        .h_full()
        .child(if browsing(app) {
            folder_title(app, p, &main_title)
        } else {
            crate::shell::pane_title(p, "main-title", &main_title, detail.as_deref(), true)
        })
        .child(main_pane_content(app, p));
    let mut panels = row()
        .key("funnel-panels")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], p.look.line);
    if s.searching() {
        panels = panels.child(
            col()
                .key("funnel-rail")
                .w_frac(0.23)
                .min_w(180.0)
                .max_w(250.0)
                .h_full()
                .min_h(0.0)
                .child(scope_rail(app, p).grow(0.44))
                .child(context_pane(app, p).grow(0.56)),
        );
    }
    panels = panels.child(main).child(split_handle(p, split));
    let selected = s.focused_item();
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
                None,
                false,
            ))
            .child(list_focus(
                app,
                List::Child,
                directory_content(app, p, folder, List::Child),
            ))
    } else if let Some(item) = selected {
        col()
            .key("preview-pane")
            .h_full()
            .min_h(0.0)
            .child(crate::shell::pane_title(
                p,
                "preview-title",
                &item.name(),
                Some(item.kind.label()),
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
    let right = right.grow(1.0 - split).min_w(180.0);
    body.child(panels.child(right))
}

fn split_handle(p: &Paint, split: f32) -> El {
    el().key("funnel-divider")
        .w(7.0)
        .h_full()
        .center()
        .cursor(rune::Cursor::ResizeColumn)
        .role("slider")
        .label("File pane width")
        .range_value((split * 100.0).round() as f64, 25.0, 75.0)
        .focusable(true)
        .hover(p.look.accent.alpha(0.18))
        .child(el().w(1.0).h_full().fill(p.look.line).inert())
        .on_drag(move |a: &mut App, drag, tx| match drag.phase {
            rune::Phase::Down => {
                drag.data = a.funnel.divider.unwrap_or(split);
                tx.live_only();
            }
            rune::Phase::Move => {
                let width = a.funnel.panel_width;
                let rail = if session(a).searching() {
                    (width * 0.23).clamp(180.0, 250.0)
                } else {
                    0.0
                };
                a.funnel.divider =
                    Some((drag.data + drag.dx / (width - rail).max(1.0)).clamp(0.25, 0.75));
            }
            rune::Phase::Up => tx.live_only(),
        })
        .on_step(move |a: &mut App, step, _| {
            a.funnel.divider =
                Some((a.funnel.divider.unwrap_or(split) + step as f32 * 0.04).clamp(0.25, 0.75));
        })
}

fn folder_title(app: &App, p: &Paint, name: &str) -> El {
    let s = session(app);
    let ink = crate::shell::pane_title_ink(p, true);
    let mut strip = crate::shell::pane_strip(p, "main-title", true);
    if let Some(parent_path) = s.folder.0.parent() {
        let label = format!("Open parent folder {}", model::display_path(parent_path));
        strip = strip.child(
            action_text(p, "parent-folder", &model::name(parent_path), ink)
                .max_w(85.0)
                .child(crate::shell::control_icon(
                    crate::shell::ControlIcon::Up,
                    ink,
                    11.0,
                ))
                .label(&label)
                .help(format!("{label} · ←"))
                .on_press_tx(parent),
        );
    }
    let direction = if s.sort.descending {
        "descending"
    } else {
        "ascending"
    };
    let label = format!("Sort by {}, {direction}", s.sort.field.title());
    let direction_icon = if s.sort.descending {
        crate::shell::ControlIcon::Down
    } else {
        crate::shell::ControlIcon::Up
    };
    strip
        .child(
            text(name)
                .font(p.look.font(12.0, 600.0))
                .color(ink)
                .grow(1.0)
                .min_w(0.0)
                .truncate()
                .help(model::display_path(&s.folder.0)),
        )
        .child(
            action_text(p, "folder-sort", s.sort.field.title(), ink)
                .child(crate::shell::control_icon(direction_icon, ink, 11.0))
                .label(&label)
                .help(label)
                .on_press_tx(|a: &mut App, tx| sort_menu(a, "folder-sort", tx)),
        )
}

fn action_text(p: &Paint, key: &str, label: &str, ink: rune::Color) -> El {
    p.kit.interactive(
        row()
            .key(key.to_owned())
            .role("button")
            .label(label.to_owned())
            .focusable(true)
            .h(23.0)
            .px(5.0)
            .gap(4.0)
            .min_w(0.0)
            .items_center()
            .child(
                text(label)
                    .font(p.look.font(11.0, 500.0))
                    .color(ink)
                    .min_w(0.0)
                    .truncate(),
            ),
    )
}
fn empty(p: &Paint, message: &str) -> El {
    col()
        .key(format!("empty-{message}"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .center()
        .padding(24.0)
        .child(
            text(message)
                .font(p.look.font(12.0, 400.0))
                .color(p.look.muted)
                .wrap(),
        )
}
fn main_content(app: &App, p: &Paint) -> El {
    if browsing(app) {
        return directory_content(app, p, &session(app).folder, List::Main);
    }
    if session(app).rows.is_empty() {
        if session(app).searching() {
            if let crate::index::Status::Failed(error) = app.index.status() {
                return empty(p, &error);
            }
        }
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

fn main_pane_content(app: &App, p: &Paint) -> El {
    let content = main_content(app, p);
    if !browsing(app) {
        return col()
            .key(format!("{}-content", prefix(app)))
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(list_focus(app, List::Main, content));
    }

    let mut list = list_focus(
        app,
        List::Main,
        folder_content(app.funnel.folder_view_id).child(content),
    );
    if let Some(parked) = &app.funnel.parked {
        let key = folder_list_key(parked.view_id);
        let rows = parked.rows.clone();
        let version = folder_rows_version(&rows);
        // Rune drops a virtual index when its stack unmounts. Keep one previous stack at
        // zero height with the same path and keys, without building file row views.
        let stack = col()
            .key(format!("{key}-rows"))
            .w_full()
            .growth(GrowthSpec::default())
            .virtual_rows(
                rows.len(),
                FOLDER_ROW_HEIGHT,
                move |_: &App, i| rows[i].id.key().into(),
                |_: &App, _| el().h(FOLDER_ROW_HEIGHT),
            )
            .virtual_rows_version(version);
        let viewport = col()
            .key(key.clone())
            .w_full()
            .h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(stack);
        list = list.child(
            folder_content(parked.view_id)
                .grow(0.0)
                .h(0.0)
                .max_h(0.0)
                .clip_rounded(0.0)
                .opacity(0.0)
                .inert()
                .a11y_hidden()
                .child(scroll::area(viewport, &key)),
        );
    }
    list
}

fn folder_content(view_id: u64) -> El {
    col()
        .key(format!("folder-content-{view_id}"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
}

fn list_focus(app: &App, list: List, content: El) -> El {
    let (key, label, focused) = match list {
        List::Main => (
            format!("{}-main-list", prefix(app)),
            "Files",
            session(app).focused.as_ref(),
        ),
        List::Child => (
            "child-list".into(),
            "Files in selected folder",
            app.funnel.child_selected.as_ref().map(|(id, _)| id),
        ),
        List::Context => return content,
    };
    let viewport = col()
        .key(key)
        .role("listbox")
        .label(label)
        .focusable(true)
        .no_focus_ring()
        .on_key(move |a: &mut App, ev, tx| match list {
            List::Child => child_key(a, ev, tx),
            _ => crate::app::on_key(a, ev, tx),
        })
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(content);
    match focused {
        Some(id) => viewport.active_descendant(id.key()),
        None => viewport,
    }
}
fn directory_content(app: &App, p: &Paint, folder: &FolderID, list: List) -> El {
    match app.funnel.library.listing(folder) {
        None | Some(Listing::Loading) => empty(p, "Loading folder…").key("folder-loading"),
        Some(Listing::Failed(error)) => empty(p, error).key("folder-error"),
        Some(Listing::Ready(rows)) if rows.is_empty() => {
            empty(p, "This folder is empty").key("folder-empty")
        }
        Some(Listing::Ready(_)) => file_list(app, p, list),
    }
}
fn rows(app: &App, list: List) -> &Arc<Vec<Item>> {
    match list {
        List::Main => &session(app).rows,
        List::Child => &app.funnel.child_rows,
        List::Context => &app.funnel.context_rows,
    }
}
fn row_at(app: &App, list: List, at: usize) -> &Item {
    if list == List::Main {
        session(app).row(at)
    } else {
        &rows(app, list)[at]
    }
}

fn folder_rows_version(rows: &Arc<Vec<Item>>) -> (usize, u64) {
    // A library snapshot is immutable, and the parked stack keeps its Arc alive across visits.
    (Arc::as_ptr(rows) as usize, 0)
}

fn file_list(app: &App, p: &Paint, list: List) -> El {
    let count = rows(app, list).len();
    let key = match list {
        List::Main => list_key(app),
        List::Child => "child-scroll".into(),
        List::Context => "context-scroll".into(),
    };
    // The theme's tokens are built once per list, not once per row.
    let paint = p.clone();
    let version = match list {
        List::Main if browsing(app) && session(app).sort == Sort::default() => {
            folder_rows_version(rows(app, list))
        }
        List::Main => (0, session(app).revision),
        List::Child => (0, app.funnel.child_revision),
        List::Context => unreachable!("context rows have their own list"),
    };
    let stack = col()
        .key(format!("{key}-rows"))
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            count,
            row_height(app, list),
            move |a: &App, i| row_at(a, list, i).id.0.to_string_lossy(),
            move |a: &App, i| file_row(a, &paint, list, row_at(a, list, i), i),
        )
        .virtual_rows_version(version);
    let mut viewport = col()
        .key(key.clone())
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .px(if list == List::Context {
            7.0
        } else if list == List::Child || browsing(app) {
            8.0
        } else {
            6.0
        })
        .py(scroll::LIST_PADDING)
        .child(stack);
    if list == List::Main {
        let scroll = session(app).scroll.clone();
        viewport = viewport.on_scroll_offset(move |pos| scroll.set(pos.offset[1]));
    }
    let chrome = match list {
        List::Main => session(app).chrome.clone(),
        List::Child => app.funnel.child_chrome.clone(),
        List::Context => app.funnel.context_chrome.clone(),
    };
    viewport = scroll::wrap(viewport, key, p.look.muted.alpha(0.55), chrome);
    if list == List::Main && browsing(app) {
        viewport = viewport.on_secondary_press(|a: &mut App, _, tx| sort_menu(a, list_key(a), tx));
    }
    viewport
}
fn file_row(app: &App, p: &Paint, list: List, item: &Item, index: usize) -> El {
    let id = item.id.clone();
    let click = id.clone();
    let open = id.clone();
    let drag = id.clone();
    let kind = item.kind;
    let selected = match list {
        List::Main => session(app).selected.contains(&id),
        List::Context => focused_path(app).as_ref() == Some(&id.0),
        List::Child => app
            .funnel
            .child_selected
            .as_ref()
            .is_some_and(|(selected, _)| selected == &id),
    };
    let ink = if selected {
        p.look.selection_text
    } else {
        p.look.text
    };
    let path_detail = list == List::Main && !browsing(app);
    let title = text(item.name().as_ref())
        .font(p.look.font(
            if path_detail { 14.0 } else { 11.0 },
            if path_detail { 500.0 } else { 400.0 },
        ))
        .color(ink)
        .spans(
            item.highlights
                .iter()
                .map(|r| rune::Span::new(r.start, r.end).bold().underline(ink))
                .collect::<Vec<_>>(),
        )
        .truncate()
        .w_full();
    // Keyed, so the selection highlight entering in front doesn't change who they are.
    let mut label = col()
        .key("file-label")
        .gap(1.0)
        .min_w(0.0)
        .grow(1.0)
        .child(title);
    if path_detail {
        label = label.child(
            text(
                item.id
                    .0
                    .parent()
                    .map(|p| {
                        if session(app).searching() {
                            p.to_string_lossy().trim_start_matches('/').to_owned() + "/"
                        } else {
                            model::display_path(&item.id.0)
                        }
                    })
                    .unwrap_or_default(),
            )
            .font(p.look.font(11.0, 400.0))
            .color(if selected { ink } else { p.look.muted })
            .truncate()
            .w_full()
            .spans(
                item.path_highlights
                    .iter()
                    .map(|r| rune::Span::new(r.start, r.end).bold())
                    .collect::<Vec<_>>(),
            ),
        );
    }
    let radius = if list == List::Context {
        5.0
    } else if path_detail {
        6.0
    } else if p.look.era() == crate::theme::Era::Modern {
        4.0
    } else {
        0.0
    };
    let inset_x = if list == List::Context {
        0.0
    } else if path_detail {
        2.0
    } else {
        1.0
    };
    let inset_y = if list == List::Context { 0.0 } else { 2.0 };
    // No travel between rows: a moving highlight is drawn above the row it lands on and hides
    // its name until it settles.
    let highlight = selected.then(|| {
        el().key("selection-highlight")
            .pin(Some(inset_x), Some(inset_y), Some(inset_x), Some(inset_y))
            .radius(radius)
            .fill(p.look.selection)
            .inert()
    });
    let mut r = row()
        .role("option")
        .focus_selected(selected)
        .label(if path_detail {
            model::display_path(&item.id.0)
        } else {
            item.name().into_owned()
        })
        .help(model::display_path(&item.id.0))
        .w_full()
        .h(row_height(app, list))
        .pl(if list == List::Context {
            39.0
        } else if path_detail {
            12.0
        } else {
            10.0
        })
        .pr(10.0)
        .gap(if path_detail { 12.0 } else { 10.0 })
        .radius(radius)
        .items_center()
        .hover(p.look.selection.alpha(0.55))
        .children(highlight)
        .child(
            crate::icons::file(
                &item.id.0,
                item.kind,
                p.look,
                selected,
                if list == List::Context { 16.0 } else { 24.0 },
            )
            .w(if list == List::Context {
                16.0
            } else if path_detail {
                24.0
            } else {
                22.0
            }),
        )
        .child(label);
    if list != List::Context && !path_detail {
        r = r.child(
            text(model::size_label(item.size.or_else(|| {
                let folder = if list == List::Main {
                    Some(&session(app).folder)
                } else {
                    app.funnel.child.as_ref()
                }?;
                app.funnel.library.sizes(folder)?.0.get(&item.id).copied()
            })))
            .key("file-size")
            .font(p.look.font(9.0, 400.0))
            .color(if selected { ink } else { p.look.muted }),
        );
    }
    r.on_press_tx(move |a: &mut App, tx| {
        if !session(a).usable {
            return;
        }
        match list {
            List::Main if index < session(a).rows.len() && session(a).row(index).id == click => {
                select_at(a, index, tx.mods(), tx, false)
            }
            List::Main => select(a, click.clone(), tx.mods(), tx, false),
            List::Context => {
                session_mut(a).neighbor = rows(a, list).iter().find(|i| i.id == click).cloned();
            }
            List::Child => {
                if let Some(at) = child_position(a, &click, index) {
                    a.funnel.child_selected = Some((click.clone(), at));
                    tx.focus("child-list", false);
                }
            }
        }
    })
    .on_double_press_tx(move |a: &mut App, tx| {
        if session(a).usable {
            open_item(a, open.0.clone(), Some(kind), tx);
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
fn row_height(app: &App, list: List) -> f32 {
    if list == List::Context {
        30.0
    } else if list == List::Main && !browsing(app) {
        58.0
    } else {
        FOLDER_ROW_HEIGHT
    }
}

fn narrow(app: &mut App, path: Option<FolderID>) {
    if path
        .as_ref()
        .is_some_and(|f| !f.0.starts_with(search_root(app).0))
        || session(app).scope == path
    {
        return;
    }
    let previous = session(app).scope.clone();
    session_mut(app).scope_history.push(previous);
    session_mut(app).scope = path;
    submit_query(app);
}
fn search_root(app: &App) -> (PathBuf, String) {
    let s = session(app);
    if s.local {
        return (
            s.folder.0.clone(),
            if s.folder.0 == crate::app::home() {
                "Home".into()
            } else {
                name(&s.folder.0)
            },
        );
    }
    if app.section == Section::Folders {
        if let Some(volume) = app
            .funnel
            .mounted
            .iter()
            .filter(|v| s.folder.0.starts_with(&v.path))
            .max_by_key(|v| v.path.components().count())
        {
            return (volume.path.clone(), volume.name.clone());
        }
    }
    (PathBuf::from("/"), "This Mac".into())
}
fn scope_rail(app: &App, p: &Paint) -> El {
    let s = session(app);
    let (root, root_title) = search_root(app);
    let look = p.look;
    let root_selected = s.scope.is_none();
    let subtitle = s
        .scope
        .as_ref()
        .map(|folder| name(&folder.0))
        .unwrap_or_else(|| root_title.clone());
    let mut scopes = col().key("scope-rows").w_full().child(
        rail_row(
            p,
            "scope-mac",
            &format!("All files in {root_title}"),
            &root,
            0,
            root_selected,
        )
        .role("option")
        .focus_selected(root_selected)
        .on_press(|a: &mut App| narrow(a, None)),
    );
    if !s.scope_history.is_empty() {
        scopes = scopes.child(
            row()
                .key("scope-back")
                .role("option")
                .label("Previous scope")
                .focusable(true)
                .w_full()
                .h(30.0)
                .px(9.0)
                .items_center()
                .child(
                    text("Previous scope")
                        .font(look.font(11.0, 400.0))
                        .color(look.text),
                )
                .on_press(|a: &mut App| {
                    let previous = session_mut(a).scope_history.pop().flatten();
                    session_mut(a).scope = previous;
                    submit_query(a);
                }),
        );
    }
    let paint = p.clone();
    scopes = scopes.child(
        col()
            .key("scope-children")
            .w_full()
            .growth(GrowthSpec::default())
            .virtual_rows(
                s.scope_counts.len(),
                30.0,
                |a: &App, i| session(a).scope_counts[i].0.to_string_lossy(),
                move |a: &App, i| {
                    let path = session(a).scope_counts[i].0.clone();
                    rail_row(
                        &paint,
                        &format!("scope-{}", path.display()),
                        &name(&path),
                        &path,
                        0,
                        false,
                    )
                    .role("option")
                    .on_press(move |a: &mut App| narrow(a, Some(FolderID::new(&path))))
                },
            )
            .virtual_rows_version(s.scope_revision),
    );
    col()
        .key("scope-rail")
        .w_full()
        .min_h(0.0)
        .fill(look.raised)
        .border_sides([0.0, 1.0, 0.0, 0.0], look.line)
        .child(crate::shell::pane_title(
            p,
            "scope-title",
            "Narrow the search",
            Some(&subtitle),
            false,
        ))
        .child(
            col()
                .key("scope-scroll")
                .role("listbox")
                .label("Search scope")
                .focus_group()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .px(7.0)
                .py(4.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(scopes)
                .scroll_bar(&p.kit),
        )
}
fn rail_row(p: &Paint, key: &str, title: &str, path: &Path, depth: usize, selected: bool) -> El {
    row()
        .key(key.to_owned())
        .role("button")
        .label(title.to_owned())
        .help(model::display_path(path))
        .focusable(true)
        .w_full()
        .h(30.0)
        .pl(9.0 + depth as f32 * 10.0)
        .pr(9.0)
        .gap(7.0)
        .items_center()
        .radius(5.0)
        .fill(if selected {
            p.look.selection
        } else {
            rune::Color::TRANSPARENT
        })
        .hover(p.look.selection.alpha(0.55))
        .child(crate::icons::file(
            path,
            DocumentKind::Folder,
            p.look,
            selected,
            16.0,
        ))
        .child(
            text(title)
                .font(p.look.font(11.0, if selected { 600.0 } else { 400.0 }))
                .color(if selected {
                    p.look.selection_text
                } else {
                    p.look.text
                })
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
}

fn context_ancestors(parent: &Path, root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![parent.to_owned()];
    let mut at = parent;
    for _ in 0..3 {
        if at == root {
            break;
        }
        if let Some(p) = at.parent() {
            paths.push(p.to_owned());
            at = p;
        } else {
            break;
        }
    }
    paths.reverse();
    paths
}
fn context_pane(app: &App, p: &Paint) -> El {
    let pane = col()
        .key("context-pane")
        .w_full()
        .min_h(0.0)
        .fill(p.look.raised)
        .border_sides([1.0, 1.0, 0.0, 0.0], p.look.line)
        .child(crate::shell::pane_title(
            p,
            "context-title",
            "Where it lives",
            app.funnel
                .context_key
                .as_ref()
                .map(|p| p.to_string_lossy())
                .as_deref(),
            false,
        ));
    if let Some(error) = &app.funnel.context_error {
        return pane.child(empty(p, error));
    }
    let ancestors = Arc::new(
        app.funnel
            .context_key
            .as_deref()
            .map(|parent| context_ancestors(parent, &search_root(app).0))
            .unwrap_or_default(),
    );
    let keys = ancestors.clone();
    let paint = p.clone();
    let count = ancestors.len() + app.funnel.context_rows.len();
    if count == 0 {
        return pane.child(empty(
            p,
            if app.funnel.context.is_pending() {
                "Loading nearby entries…"
            } else {
                "Select a result to see nearby files"
            },
        ));
    }
    let stack = col()
        .key("context-rows")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            count,
            30.0,
            move |a: &App, i| {
                if i < keys.len() {
                    format!("context-ancestor-{}", keys[i].display()).into()
                } else {
                    a.funnel.context_rows[i - keys.len()].id.0.to_string_lossy()
                }
            },
            move |a: &App, i| {
                if let Some(path) = ancestors.get(i) {
                    let path = path.clone();
                    let (root, root_title) = search_root(a);
                    let title = if path == root {
                        root_title
                    } else if i == 0 {
                        format!("… / {}", name(&path))
                    } else {
                        name(&path)
                    };
                    rail_row(
                        &paint,
                        &format!("context-ancestor-{}", path.display()),
                        &title,
                        &path,
                        i,
                        false,
                    )
                    .on_press(move |a: &mut App| narrow(a, Some(FolderID::new(&path))))
                } else {
                    file_row(
                        a,
                        &paint,
                        List::Context,
                        &a.funnel.context_rows[i - ancestors.len()],
                        i - ancestors.len(),
                    )
                }
            },
        )
        .virtual_rows_version((
            app.funnel.context_key.clone(),
            Arc::as_ptr(&app.funnel.context_rows) as usize,
        ));
    let viewport = col()
        .key("context-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .px(7.0)
        .py(4.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(stack);
    pane.child(scroll::wrap(
        viewport,
        "context-scroll".into(),
        p.look.muted.alpha(0.55),
        app.funnel.context_chrome.clone(),
    ))
}

fn place(app: &App, p: &Paint, key: &str, label: &str, path: PathBuf, kind: PlaceKind) -> El {
    let dest = path.clone();
    // A place lights only when it is exactly the current folder.
    let active = session(app).folder.0 == path;
    crate::shell::place_button(p, key, label, active, kind)
        .focusable(true)
        .max_w(132.0)
        .help(model::display_path(&path))
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
        .px(12.0)
        .gap(2.0)
        .items_center()
        .fill(
            if matches!(
                p.look.era(),
                crate::theme::Era::WindowsBevel | crate::theme::Era::UnixBevel
            ) {
                p.look.sidebar
            } else {
                p.look.background
            },
        )
        .border_sides([0.0, 0.0, 1.0, 0.0], p.look.separator());
    if let Some(edit) = &app.funnel.path_edit {
        return bar.child(
            TextInput::new(edit)
                .label("Folder path")
                .height(28.0)
                .on_edit(|a: &mut App| {
                    a.funnel
                        .path_edit
                        .as_mut()
                        .expect("folder path input is mounted")
                })
                .on_submit(|a: &mut App, tx| {
                    let Some(edit) = a.funnel.path_edit.take() else {
                        return;
                    };
                    let raw = &edit.text;
                    if raw.is_empty() {
                        focus_section(a, tx);
                        return;
                    }
                    let path = if raw == "~" {
                        crate::app::home()
                    } else if let Some(s) = raw.strip_prefix("~/") {
                        crate::app::home().join(s)
                    } else {
                        PathBuf::from(raw)
                    };
                    open_folder(a, path, tx);
                })
                .on_key(|a: &mut App, ev, tx| {
                    if ev.key == Key::Escape {
                        a.funnel.path_edit = None;
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
        .child(history_button(
            p,
            "folder-back",
            "Back",
            "Back · ⌘[",
            -1,
            app.funnel.history_at > 0,
        ))
        .child(
            history_button(
                p,
                "folder-forward",
                "Forward",
                "Forward · ⌘]",
                1,
                app.funnel.history_at + 1 < app.funnel.history.len(),
            )
            .mr(6.0),
        );
    let home = crate::app::home();
    bar = bar
        .child(place(
            app,
            p,
            "place-home",
            "Home",
            home.clone(),
            PlaceKind::Home,
        ))
        .child(place(
            app,
            p,
            "place-downloads",
            "Downloads",
            home.join("Downloads"),
            PlaceKind::Downloads,
        ))
        .child(place(
            app,
            p,
            "place-desktop",
            "Desktop",
            home.join("Desktop"),
            PlaceKind::Desktop,
        ));
    if let Some(pin) = &app.funnel.pinned {
        bar = bar.child(place(
            app,
            p,
            "place-pinned",
            &name(&pin.0),
            pin.0.clone(),
            PlaceKind::Pin,
        ));
    }
    let inline_drives = app.funnel.panel_width >= WIDE_PLACES && app.funnel.mounted.len() <= 2;
    if inline_drives {
        bar = bar.children(
            app.funnel
                .mounted
                .iter()
                .map(|volume| drive(app, p, volume)),
        );
    } else if !app.funnel.mounted.is_empty() {
        bar = bar.child(
            crate::shell::place_button(
                p,
                "connected-drives",
                "Drives",
                app.funnel.drives_expanded,
                PlaceKind::Drive,
            )
            .focusable(true)
            .help("Show connected drive destinations")
            .on_press(|a: &mut App| a.funnel.drives_expanded = !a.funnel.drives_expanded),
        );
    }
    let pinned = app.funnel.pinned.as_ref() == Some(&session(app).folder);
    let pin = crate::shell::place_button(p, "folder-pin", "Pin", false, PlaceKind::Pin)
        .label(if pinned {
            "Current folder is pinned"
        } else {
            "Pin current folder"
        })
        .help(if pinned {
            "Current folder is pinned"
        } else {
            "Pin current folder"
        })
        .focusable(!pinned)
        .opacity(if pinned { 0.35 } else { 1.0 })
        .disabled(pinned);
    bar = bar
        .child(el().grow(1.0))
        .child(
            crate::shell::shell_button(p, "folder-go", "Go to Folder", false)
                .focusable(true)
                .help("Go to Folder · ⌘L")
                .on_press_tx(edit_path),
        )
        .child(
            crate::shell::shell_button(p, "folder-refresh", "Refresh", false)
                .w(28.0)
                .focusable(true)
                .help("Refresh · ⌘R")
                .on_press(refresh),
        )
        .child(if pinned {
            pin
        } else {
            pin.on_press(pin_current)
        });
    if !inline_drives && app.funnel.drives_expanded && !app.funnel.mounted.is_empty() {
        col().w_full().child(bar).child(
            col()
                .key("drives-scroll")
                .w_full()
                .h(32.0)
                .scroll_x()
                .child(
                    row().px(12.0).gap(3.0).items_center().children(
                        app.funnel
                            .mounted
                            .iter()
                            .map(|volume| drive(app, p, volume)),
                    ),
                ),
        )
    } else {
        bar
    }
}

fn history_button(
    p: &Paint,
    key: &str,
    label: &str,
    help: &str,
    delta: isize,
    available: bool,
) -> El {
    let button = crate::shell::shell_button(p, key, label, false)
        .w(28.0)
        .focusable(available)
        .disabled(!available)
        .opacity(if available { 1.0 } else { 0.35 })
        .help(help.to_owned());
    if available {
        button.on_press_tx(move |a: &mut App, tx| travel(a, delta, tx))
    } else {
        button
    }
}

fn drive(app: &App, p: &Paint, volume: &Volume) -> El {
    let path = volume.path.clone();
    let mut controls = row()
        .key(format!("drive-{}", path.display()))
        .items_center()
        .child(place(
            app,
            p,
            &format!("volume-{}", path.display()),
            &volume.name,
            path.clone(),
            PlaceKind::Drive,
        ));
    if volume.ejectable {
        let label = format!("Eject {}", volume.name);
        controls = controls.child(
            crate::shell::shell_button(p, &format!("eject-{}", path.display()), &label, false)
                .focusable(true)
                .help(label)
                .on_press(move |a: &mut App| crate::opening::eject_volume(a, path.clone())),
        );
    }
    controls
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
            pin_folder(a, path);
        }
        Ok(None) => {}
        Err(error) => a.toast(error.to_string()),
    });
}

/// Numeric and path diagnostics for the shell's optional state report.
pub fn report(app: &App) -> serde_json::Value {
    fn session_report(s: &Session) -> serde_json::Value {
        serde_json::json!({"folder":s.folder.0,"query":s.query.text,"folder_mode":s.local,
            "scope":s.scope.as_ref().map(|f|&f.0), "focused":s.neighbor.as_ref().map(|i|&i.id.0).or_else(||s.focused.as_ref().map(|i|&i.0)),
            "scope_counts":s.scope_counts,
            "selection":s.selected.iter().map(|i|&i.0).collect::<Vec<_>>(),
            "usable":s.usable,"complete":s.complete,"query_id":s.request,"scroll":s.scroll.get(),
            "rows":(0..s.rows.len()).map(|at| &s.row(at).id.0).collect::<Vec<_>>()})
    }
    let mut report = session_report(session(app));
    report["search"] = session_report(&app.funnel.search);
    report["folders"] = session_report(&app.funnel.folders);
    report["history"] =
        serde_json::json!(app.funnel.history.iter().map(|f| &f.0).collect::<Vec<_>>());
    report["history_at"] = serde_json::json!(app.funnel.history_at);
    report["pinned"] = serde_json::json!(pinned_folder(app));
    report["path_editing"] = serde_json::json!(app.funnel.path_edit.is_some());
    report
}

fn sort_menu(app: &mut App, key: impl Into<rune::PopupAnchor>, tx: &mut Tx) {
    let mut menu = rune::PopupMenu::new();
    for (i, field) in SortField::ALL.into_iter().enumerate() {
        menu = menu.item(
            rune::PopupItem::new(i.to_string(), format!("Sort by {}", field.title()))
                .checked(session(app).sort.field == field),
        );
    }
    tx.popup_menu(menu, key, |a: &mut App, picked, _| {
        if let Some(field) = picked
            .and_then(|p| p.parse::<usize>().ok())
            .and_then(|i| SortField::ALL.get(i))
            .copied()
        {
            set_sort(a, field);
        }
    });
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
    sync_directory(app);
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
