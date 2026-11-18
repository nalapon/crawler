//! The whole app's state, its sections and the routing between them.
//!
//! Every module keeps its own state struct inside [`App`]. Handlers receive `&mut App`, so a
//! module reaches another only through that module's public functions (`crate::opening::open_path`,
//! `crate::transfers::start_copy`, …), never by editing its fields.

use std::path::PathBuf;

use rune::prefs::Prefs;
use rune::{Cx, El, Key, KeyEvent, Tx, Waker, col};
use rune_kit::Kit;

use crate::args::Launch;
use crate::preferences::Preferences;
use crate::theme::{Look, Theme};
use crate::{activity, funnel, index, opening, pdf, preview, settings, shell, transfers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Section {
    #[default]
    Search,
    Folders,
    Activity,
    Transfers,
    Settings,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Search,
        Section::Folders,
        Section::Activity,
        Section::Transfers,
        Section::Settings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Search => "Search",
            Section::Folders => "Folders",
            Section::Activity => "Activity",
            Section::Transfers => "Transfers",
            Section::Settings => "Settings",
        }
    }

    /// The ⌘ key that shows it: ⌘1–⌘4 and ⌘, for Settings.
    pub fn shortcut(self) -> char {
        match self {
            Section::Search => '1',
            Section::Folders => '2',
            Section::Activity => '3',
            Section::Transfers => '4',
            Section::Settings => ',',
        }
    }

    pub fn parse(s: &str) -> Option<Section> {
        Section::ALL
            .into_iter()
            .find(|x| x.title().eq_ignore_ascii_case(s))
    }

    pub fn key(self) -> &'static str {
        match self {
            Section::Search => "section-search",
            Section::Folders => "section-folders",
            Section::Activity => "section-activity",
            Section::Transfers => "section-transfers",
            Section::Settings => "section-settings",
        }
    }
}

/// A full-window reader covering the workspace. Its module owns the document state; this only
/// says which one is up. Closing restores the workspace and its focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Reader {
    #[default]
    None,
    /// Markdown, Word, spreadsheets, PowerPoint, code, text, images and video.
    Document,
    Pdf,
}

/// What every view needs to paint: the theme's roles and the rune-kit tokens built from them.
#[derive(Clone)]
pub struct Paint {
    pub look: Look,
    pub kit: Kit,
}

pub struct App {
    pub launch: Launch,
    prefs: Preferences,
    pub waker: Option<Waker>,
    pub section: Section,
    /// The saved theme. `theme_preview` overrides it while the command palette previews one.
    pub theme: Theme,
    pub theme_preview: Option<Theme>,
    pub reader: Reader,
    /// A closed reader whose sheet is still sliding out, and the clock time at which its
    /// state is released. It is drawn but takes no input: `reader` is already `None`.
    leaving: Option<(Reader, f64)>,
    /// A file to open on the first poll (`--open` or a path handed to the app).
    pub pending_open: Option<PathBuf>,
    /// The PDF whose successful display was last added to recent files.
    pub recorded_pdf: Option<PathBuf>,
    /// The last Escape's time in a reader, for the two-press close (700 ms).
    pub reader_escape_at: Option<f64>,
    pub clock: f64,
    pub toasts: Vec<shell::Toast>,
    pub show_help: bool,
    pub host: Option<rune::AppHandle<App>>,
    reports: Reports,
    pub(crate) frame_inputs: crate::frame_timing::Inputs,

    pub index: index::Engine,
    pub funnel: funnel::State,
    pub opening: opening::State,
    pub preview: preview::State,
    pub pdf: pdf::State,
    pub activity: activity::State,
    pub transfers: transfers::State,
    pub settings: settings::State,
}

pub const PREFS_ID: &str = "studio.crawler.rune";

impl App {
    pub fn launch() -> App {
        let mut app = Self::new(Launch::parse());
        funnel::launched(&mut app);
        app
    }

    /// A secondary preview needs the shared document services, without launching a funnel.
    pub fn preview_window() -> App {
        Self::new(Launch::default())
    }

    fn new(launch: Launch) -> App {
        // Headless runs keep their preferences apart from the real ones.
        let isolated = std::env::var_os("RUNE_PREFS_DIR").is_some()
            || std::env::var_os("RUNE_DIRS_ROOT").is_some();
        let prefs = if rune::tooling::headless_run().is_some() && !isolated {
            Prefs::memory()
        } else {
            let mut prefs = Prefs::open(PREFS_ID);
            crate::swift_settings::import(&mut prefs);
            prefs
        };
        let saved: Theme = prefs.get("theme").unwrap_or_default();
        let theme = launch
            .theme
            .as_deref()
            .and_then(Theme::parse)
            .unwrap_or(saved);
        let section = launch.section.unwrap_or(if launch.folder.is_some() {
            Section::Folders
        } else {
            Section::Search
        });
        let mut index = index::Engine::new(launch.search_workers);
        if launch.state_report.is_some() {
            index.enable_diagnostics();
        }
        App {
            reports: Reports::new(&launch),
            frame_inputs: Default::default(),
            host: None,
            index,
            funnel: funnel::State::new(&launch, &prefs),
            opening: opening::State::new(&prefs),
            preview: preview::State::new(&prefs),
            pdf: pdf::State::default(),
            activity: activity::State::default(),
            transfers: transfers::State::default(),
            settings: settings::State::default(),
            launch,
            prefs: Preferences::new(prefs),
            waker: None,
            section,
            theme,
            theme_preview: None,
            reader: Reader::None,
            leaving: None,
            pending_open: None,
            recorded_pdf: None,
            reader_escape_at: None,
            clock: 0.0,
            toasts: Vec::new(),
            show_help: false,
        }
    }

    pub fn look(&self) -> Look {
        self.theme_preview.unwrap_or(self.theme).look()
    }

    /// Confirms a theme and persists it under `theme`.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.theme_preview = None;
        let _ = self.set_preference("theme", &theme);
    }

    pub fn set_preference<T: serde::Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), rune::prefs::PrefsError> {
        self.prefs.set(key, value, self.waker.clone())
    }

    pub fn mark_search_input(&mut self, query: u64, tx: &Tx) {
        if self.launch.frame_timing {
            self.frame_inputs.search = tx.input_time().map(|at| (query, at));
        }
    }

    pub fn mark_folder_input(&mut self, visit: u64, tx: &Tx) {
        if self.launch.frame_timing {
            self.frame_inputs.folder = tx.input_time().map(|at| (visit, at));
        }
    }

    pub fn mark_open_input(&mut self, path: &std::path::Path, tx: &Tx) {
        if self.launch.frame_timing {
            self.frame_inputs.open = tx.input_time().map(|at| (path.to_owned(), at));
        }
    }

    pub(crate) fn state_started(&self) -> std::time::Instant {
        self.reports.started
    }

    pub fn show(&mut self, section: Section) {
        if self.section == section {
            return;
        }
        opening::cancel_navigation(self);
        funnel::leaving(self, self.section);
        self.section = section;
        funnel::entering(self, section);
    }

    /// A short message at the bottom of the window that fades after a few seconds.
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(shell::Toast {
            text: text.into(),
            until: self.clock + 3.0,
        });
    }

    /// Closes the open reader. The workspace takes input again at once; the reader's own
    /// state lives on until its sheet has slid out (`leaving`).
    pub fn close_reader(&mut self) {
        let closing = self.reader;
        if closing == Reader::None {
            return;
        }
        self.finish_leaving();
        self.reader = Reader::None;
        self.reader_escape_at = None;
        // A PDF opened from a link in a document returns to that document, with no slide.
        if closing == Reader::Pdf && preview::resume_reader(self) {
            pdf::reader_closed(self);
            return;
        }
        self.leaving = Some((closing, self.clock + SHEET_EXIT));
    }

    /// Releases a closed reader's state, once its sheet is off screen or another one opens.
    pub fn finish_leaving(&mut self) {
        if let Some((reader, _)) = self.leaving.take() {
            match reader {
                Reader::Document => preview::reader_closed(self),
                Reader::Pdf => pdf::reader_closed(self),
                Reader::None => {}
            }
        }
    }

    /// Whether the PDF reader is drawn, open or sliding out, rather than a pane's preview.
    pub fn pdf_sheet_shown(&self) -> bool {
        self.reader == Reader::Pdf || matches!(self.leaving, Some((Reader::Pdf, _)))
    }

    /// Whether a reader's sheet is on screen, open or sliding out.
    pub fn sheet_shown(&self) -> bool {
        self.reader != Reader::None || self.leaving.is_some()
    }
}

/// Seconds a closed reader stays drawn while its sheet slides out.
const SHEET_EXIT: f64 = 0.4;

pub fn setup(app: &mut App, waker: Waker) {
    app.index.set_waker(waker.clone());
    app.waker = Some(waker.clone());
    crate::documents::set_waker(waker.clone());
    funnel::setup(app, &waker);
    preview::setup(app, &waker);
    pdf::setup(app, &waker);
    activity::setup(app, &waker);
    transfers::setup(app, &waker);
    opening::setup(app, &waker);
    settings::setup(app, &waker);
    preview::set_reader_display_observer(app, opening::record_recent);
    if let Some(path) = app
        .launch
        .open
        .clone()
        .or_else(|| app.launch.paths.first().cloned())
    {
        app.pending_open = Some(path);
    }
    app.reports.setup_ms = Some(app.reports.started.elapsed().as_secs_f64() * 1000.0);
}

pub fn poll(app: &mut App, clock: f64, tx: &mut Tx) -> bool {
    app.clock = clock;
    let mut changed = false;
    if let Some(path) = app.pending_open.take() {
        opening::open_external_paths(app, vec![path], tx);
        changed = true;
    }
    let documents = crate::documents::take();
    if !documents.is_empty() {
        opening::open_external_paths(app, documents, tx);
        changed = true;
    }
    record_displayed_pdf(app);
    if let Some((_, until)) = app.leaving {
        if clock >= until {
            app.finish_leaving();
            changed = true;
        } else {
            tx.wake_after(std::time::Duration::from_secs_f64(until - clock));
        }
    }
    let before = app.toasts.len();
    app.toasts.retain(|t| t.until > clock);
    changed |= app.toasts.len() != before;
    if let Some(next) = app.toasts.iter().map(|t| t.until).reduce(f64::min) {
        tx.wake_after(std::time::Duration::from_secs_f64((next - clock).max(0.0)));
    }
    changed |= funnel::poll(app, tx);
    changed |= opening::poll(app, tx);
    changed |= settings::poll(app, tx);
    changed |= preview::poll(app, tx);
    changed |= pdf::poll(app, tx);
    changed |= activity::poll(app, tx);
    changed |= transfers::poll(app, tx);
    if let Some(error) = app.prefs.take_error() {
        app.toast(error);
        changed = true;
    }
    poll_reports(app, tx);
    changed
}

/// Record a PDF after its first bitmap is ready, once per reader visit.
fn record_displayed_pdf(app: &mut App) {
    let shown = match app.reader {
        Reader::Pdf => pdf::active_path(app).filter(|p| pdf::is_ready(app, p)),
        _ => None,
    };
    match shown {
        Some(path) if app.recorded_pdf.as_ref() != Some(&path) => {
            opening::record_recent(app, &path);
            app.recorded_pdf = Some(path);
        }
        Some(_) => {}
        None if app.reader != Reader::Pdf => app.recorded_pdf = None,
        None => {}
    }
}

pub fn on_close(app: &mut App, tx: &mut Tx) {
    pdf::on_close(app, tx);
    transfers::on_close(app);
    app.index.suspend();
    app.prefs.finish();
}

/// Global keys first, then the reader or the visible section.
pub fn on_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key == Key::Command('q') {
        request_quit(app, tx);
        return true;
    }
    if app.reader != Reader::None {
        return reader_key(app, ev, tx);
    }
    if settings::on_key(app, ev, tx) {
        return true;
    }
    if let Key::Command(c) = ev.key {
        if !ev.mods.shift {
            if let Some(section) = Section::ALL.into_iter().find(|s| s.shortcut() == c) {
                app.show(section);
                funnel::focus_section(app, tx);
                return true;
            }
        }
        if c == 'p' && !ev.mods.shift {
            funnel::show_search(app, None, tx);
            return true;
        }
    }
    match app.section {
        Section::Search | Section::Folders => funnel::on_key(app, ev, tx),
        Section::Activity => activity::on_key(app, ev, tx),
        Section::Transfers => transfers::on_key(app, ev, tx),
        Section::Settings => false,
    }
}

/// Close keys shared by every reader: X, ⌘W, or two non-repeated Escapes within 700 ms.
fn reader_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let handled = match app.reader {
        Reader::Document => preview::reader_key(app, ev, tx),
        Reader::Pdf => pdf::reader_key(app, ev, tx),
        Reader::None => false,
    };
    if handled {
        return true;
    }
    let escape = match ev.key {
        Key::Command('w') => false,
        Key::Text(ref t)
            if t.eq_ignore_ascii_case("x") && !ev.mods.cmd && !preview::typing(app) =>
        {
            false
        }
        Key::Escape => true,
        _ => return false,
    };
    let now = tx.clock();
    if escape && app.reader_escape_at.is_none_or(|at| now - at > 0.7) {
        app.reader_escape_at = Some(now);
    } else {
        app.close_reader();
    }
    if app.reader == Reader::Document {
        preview::focus_reader(tx);
    } else {
        funnel::focus_section(app, tx);
    }
    true
}

pub fn view(app: &App, cx: &mut Cx<App>) -> El {
    let look = app.look();
    let p = Paint {
        kit: look.kit(),
        look,
    };
    let mut layer = Vec::new();
    let content = match app.section {
        Section::Search | Section::Folders => funnel::view(app, cx, &p, &mut layer),
        Section::Activity => activity::view(app, cx, &p, &mut layer),
        Section::Transfers => transfers::view(app, cx, &p, &mut layer),
        Section::Settings => settings::view(app, cx, &p, &mut layer),
    };
    // Nothing in the workspace animates: layout and colour changes land at once and what
    // it mounts appears whole. Rune would spring every change and fade in every new
    // element, which hid a row's name under a travelling highlight, crossfaded previews
    // and held back the PDF pane's sharp render for 0.7 s after each key.
    let mut body = col()
        .key("workspace")
        .motion(rune::Spring::NONE)
        .instant_changes()
        .w_full()
        .h_full()
        .fill(look.background)
        .child(shell::section_bar(app, cx, &p, &mut layer))
        .child(content.grow(1.0).min_h(0.0).w_full())
        .child(shell::status_bar(app, &p));
    // A reader is a sheet over the workspace, which stays mounted underneath. Opening slides
    // the sheet in from the right over the workspace; closing slides it back and uncovers it.
    // Rune fades whatever it unmounts, so the way out is an offset on a sheet that stays
    // mounted until it is off screen (`App::leaving`), not an exit transition.
    let leaving = app.reader == Reader::None;
    let shown = if leaving {
        app.leaving.map_or(Reader::None, |(reader, _)| reader)
    } else {
        app.reader
    };
    let mut above = Vec::new();
    let reader = match shown {
        Reader::Document => Some(preview::reader_view(app, cx, &p, &mut above)),
        Reader::Pdf => Some(pdf::reader_view(app, cx, &p, &mut above)),
        Reader::None => None,
    };
    if let Some(reader) = reader {
        if !leaving {
            // The covered workspace takes no input while the sheet is up.
            body = body.inert();
        }
        let width = cx.layout_size("workspace").map_or(1180.0, |size| size[0]);
        let mut sheet = rune::el()
            .key("reader-sheet")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .transition(rune::Transition::asymmetric(
                rune::Transition::move_edge(rune::Edge::Trailing)
                    .springs(rune::tok::motion::pair::GLIDE),
                rune::Transition::none(),
            ))
            .offset(if leaving { width } else { 0.0 }, 0.0)
            .offset_motion(rune::tok::motion::GLIDE_EXIT)
            // Only the sheet travels. What it holds lands at once, so a page that first lays
            // out at the pane's width doesn't spring to the window's while it slides.
            .child(reader.w_full().h_full().instant_changes());
        if leaving {
            sheet = sheet.inert();
        }
        layer.push(sheet);
        // What the reader floats (note popovers, prompts) stays above its sheet.
        layer.extend(above);
    }
    settings::overlays(app, cx, &p, &mut layer);
    shell::toasts(app, &p, &mut layer);
    rune_kit::root(body, layer).on_drawn_layout_tx(|a: &mut App, _, tx| {
        if a.reports.first_layout_ms.is_none() {
            a.reports.first_layout_ms = Some(a.reports.started.elapsed().as_secs_f64() * 1000.0);
            if a.reports.sender.is_some() {
                a.reports.next = 0.0;
                poll_reports(a, tx);
            }
        }
        tx.live_only();
    })
}

/// Named states for `--scene`. Each module's own launch flags cover most checks.
pub const SCENES: &[(&str, fn(&mut App))] = &[
    ("search", |a| a.section = Section::Search),
    ("folders", |a| a.section = Section::Folders),
    ("activity", |a| a.section = Section::Activity),
    ("transfers", |a| a.section = Section::Transfers),
    ("settings", |a| a.section = Section::Settings),
    ("help", |a| a.show_help = true),
];

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// A close hides Rune's window before it can be vetoed. Keep its runtime alive and reopen
/// it with the PDF prompt before cancelling any work. A clean close ends the app.
pub fn window_closed(app: &mut App, tx: &mut Tx) {
    if !pdf::can_close(app, tx) {
        if let Some(host) = &app.host {
            host.open();
        }
        return;
    }
    tx.quit();
}

pub fn request_quit(app: &mut App, tx: &mut Tx) {
    pdf::request_quit(app, tx);
}

struct Reports {
    sender: Option<std::sync::mpsc::SyncSender<serde_json::Value>>,
    worker: Option<std::thread::JoinHandle<()>>,
    next: f64,
    started: std::time::Instant,
    setup_ms: Option<f64>,
    first_layout_ms: Option<f64>,
    pending: Option<serde_json::Value>,
}
impl Reports {
    fn new(launch: &Launch) -> Self {
        let mut reports = Self {
            sender: None,
            worker: None,
            next: 0.0,
            started: std::time::Instant::now(),
            setup_ms: None,
            first_layout_ms: None,
            pending: None,
        };
        if launch.state_report.is_none() && launch.launch_report.is_none() {
            return reports;
        }
        let state_path = launch.state_report.clone();
        let event_path = launch.event_log.clone();
        let mut launch_path = launch.launch_report.clone();
        let (sender, receive) = std::sync::mpsc::sync_channel::<serde_json::Value>(1);
        match std::thread::Builder::new()
            .name("crawler-reports".into())
            .spawn(move || {
                use std::io::Write;
                let mut previous = serde_json::Value::Null;
                while let Ok(mut state) = receive.recv() {
                    if state == previous {
                        continue;
                    }
                    previous = state.clone();
                    if state["launch"]["setup_ms"].is_number() {
                        if let Some(path) = launch_path.take() {
                            if let Err(error) = write_report(&path, &state["launch"]) {
                                eprintln!("crawler: launch report: {error}");
                            }
                        }
                    }
                    state["report_epoch_seconds"] = serde_json::json!(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs_f64()
                    );
                    if let Some(path) = &state_path {
                        if let Err(error) = write_report(path, &state) {
                            eprintln!("crawler: state report: {error}");
                        }
                        if let Some(path) = &event_path {
                            let append = (|| -> std::io::Result<()> {
                                // Deliberately do not create a missing event log.
                                let mut file =
                                    std::fs::OpenOptions::new().append(true).open(path)?;
                                serde_json::to_writer(&mut file, &state)?;
                                file.write_all(b"\n")
                            })();
                            if let Err(error) = append {
                                eprintln!("crawler: event log: {error}");
                            }
                        }
                    }
                }
            }) {
            Ok(worker) => {
                reports.sender = Some(sender);
                reports.worker = Some(worker);
            }
            Err(error) => eprintln!("crawler: start report writer: {error}"),
        }
        reports
    }
}
impl Drop for Reports {
    fn drop(&mut self) {
        // A one-frame headless snapshot may end immediately after drawn-layout callbacks.
        // Flush its latest snapshot here; handlers and normal polls never wait for disk IO.
        if let Some(state) = self.pending.take()
            && let Some(sender) = &self.sender
        {
            let _ = sender.send(state);
        }
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn write_report(path: &std::path::Path, state: &serde_json::Value) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut file, state)?;
    file.write_all(b"\n")?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
fn poll_reports(app: &mut App, tx: &mut Tx) {
    if app.reports.sender.is_none() {
        return;
    }
    if app.launch.state_report.is_none() && app.reports.setup_ms.is_none() {
        return;
    }
    tx.wake_after(std::time::Duration::from_millis(250));
    if app.clock < app.reports.next {
        return;
    }
    app.reports.next = app.clock + 0.25;
    let funnel = funnel::report(app);
    let index = app.index.report();
    let status = match app.index.status() {
        index::Status::Idle => "idle".to_owned(),
        index::Status::Loading { indexed } => format!("loading {indexed}"),
        index::Status::Ready { indexed } => format!("ready {indexed}"),
        index::Status::Failed(error) => format!("failed: {error}"),
    };
    let mut state = serde_json::json!({
        "section":app.section.title(),"theme":app.theme_preview.unwrap_or(app.theme).name(),
        "folder":funnel["folders"]["folder"],"folder_history":{
            "paths":funnel["history"],"at":funnel["history_at"]},
        "funnel":funnel,"index":index,"activity_monitor":activity::report(app),
        "shortcuts_visible":app.show_help,"palette_visible":settings::palette_visible(app),
        "reader":format!("{:?}",app.reader),"recent_opened_files":app.opening.recent,
        "file_search_engine":"fff","file_search_status":status,
        "file_search_engine_revision":index["engine_revision"],
        "file_search_first_result_ms":index["first_result_ms"],
        "file_search_workers":index["workers"],"file_search_scope_counts":funnel["scope_counts"],
        "file_search_query_cache":{"entries":index["query_cache_entries"],"hits":index["query_cache_hits"]},
        "file_search_cache_load_ms":index["cache_load_ms"],"file_search_index_ms":index["index_ms"],
        "file_search_query":funnel["query"],"file_search_mode":if funnel["folder_mode"]==true {"folder"} else {"computer"},
        "file_search_scope":funnel["scope"],"file_search_results":funnel["rows"],
        "file_search_selected":funnel["focused"],"file_search_indexed":index["indexed_paths"],
        "file_search_matches":index["total_matches"],"file_search_performance":index["stages"],
        "file_search_catch_up":index["file_search_catch_up"],
        "pdf_reader_visible":app.reader==Reader::Pdf,"pdf_reader_path":pdf::active_path(app),
        "launch":{"pid":std::process::id(),"setup_ms":app.reports.setup_ms,"first_drawn_layout_ms":app.reports.first_layout_ms,
            "note":"App-side setup readiness; drawn layout is optional. Excludes compositor presentation and asynchronous file loading."}
    });
    state
        .as_object_mut()
        .unwrap()
        .extend(pdf::report(app).as_object().unwrap().clone());
    state
        .as_object_mut()
        .unwrap()
        .extend(preview::report(app).as_object().unwrap().clone());
    if let Some(sender) = &app.reports.sender {
        app.reports.pending = match sender.try_send(state) {
            Err(std::sync::mpsc::TrySendError::Full(state)) => Some(state),
            _ => None,
        };
    }
    if app.launch.state_report.is_none() {
        app.reports.sender.take();
    }
}

/// Rune preserves AppKit's default terminate item. Remove that duplicate so both the
/// visible Quit command and its equivalent go through our annotation guard.
pub fn install_quit_menu() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let application = NSApplication::sharedApplication(mtm);
    let Some(menu) = application
        .mainMenu()
        .and_then(|m| m.itemAtIndex(0))
        .and_then(|i| i.submenu())
    else {
        return;
    };
    for index in (0..menu.numberOfItems()).rev() {
        if let Some(item) = menu.itemAtIndex(index) {
            if item
                .action()
                .is_some_and(|action| action == objc2::sel!(terminate:))
            {
                menu.removeItemAtIndex(index);
            }
        }
    }
}
