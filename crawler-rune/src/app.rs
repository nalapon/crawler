//! The whole app's state, its sections and the routing between them.
//!
//! Every module keeps its own state struct inside [`App`]. Handlers receive `&mut App`, so a
//! module reaches another only through that module's public functions (`crate::opening::open_file`,
//! `crate::transfers::start_copy`, …), never by editing its fields.

use std::path::PathBuf;

use rune::prefs::Prefs;
use rune::{Cx, El, Key, KeyEvent, Tx, Waker, col};
use rune_kit::Kit;

use crate::args::Launch;
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
    pub const ALL: [Section; 5] = [Section::Search, Section::Folders, Section::Activity, Section::Transfers, Section::Settings];

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
        Section::ALL.into_iter().find(|x| x.title().eq_ignore_ascii_case(s))
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
    Markdown,
    Pdf,
}

/// What every view needs to paint: the theme's roles and the rune-kit tokens built from them.
pub struct Paint {
    pub look: Look,
    pub kit: Kit,
}

pub struct App {
    pub launch: Launch,
    pub prefs: Prefs,
    pub waker: Option<Waker>,
    pub section: Section,
    /// The saved theme. `theme_preview` overrides it while the command palette previews one.
    pub theme: Theme,
    pub theme_preview: Option<Theme>,
    pub reader: Reader,
    /// A file to open on the first poll (`--open` or a path handed to the app).
    pub pending_open: Option<PathBuf>,
    /// The PDF whose successful display was last added to recent files.
    pub recorded_pdf: Option<PathBuf>,
    /// The last Escape's time in a reader, for the two-press close (700 ms).
    pub reader_escape_at: Option<f64>,
    pub clock: f64,
    pub toasts: Vec<shell::Toast>,
    pub show_help: bool,

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
        let prefs = if rune::tooling::headless_run().is_some() { Prefs::memory() } else { Prefs::open(PREFS_ID) };
        let saved: Theme = prefs.get("theme").unwrap_or_default();
        let theme = launch.theme.as_deref().and_then(Theme::parse).unwrap_or(saved);
        let section = launch.section.unwrap_or(if launch.folder.is_some() { Section::Folders } else { Section::Search });
        App {
            index: index::Engine::new(launch.search_workers),
            funnel: funnel::State::new(&launch, &prefs),
            opening: opening::State::new(&prefs),
            preview: preview::State::new(&prefs),
            pdf: pdf::State::default(),
            activity: activity::State::default(),
            transfers: transfers::State::default(),
            settings: settings::State::default(),
            launch,
            prefs,
            waker: None,
            section,
            theme,
            theme_preview: None,
            reader: Reader::None,
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
        let _ = self.prefs.set("theme", &theme);
    }

    pub fn show(&mut self, section: Section) {
        if self.section == section {
            return;
        }
        funnel::leaving(self, self.section);
        self.section = section;
        funnel::entering(self, section);
    }

    /// A short message at the bottom of the window that fades after a few seconds.
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(shell::Toast { text: text.into(), until: self.clock + 3.0 });
    }

    pub fn close_reader(&mut self) {
        match self.reader {
            Reader::None => return,
            Reader::Markdown => preview::reader_closed(self),
            Reader::Pdf => pdf::reader_closed(self),
        }
        self.reader = Reader::None;
        self.reader_escape_at = None;
    }
}

pub fn setup(app: &mut App, waker: Waker) {
    app.index.set_waker(waker.clone());
    app.waker = Some(waker.clone());
    funnel::setup(app, &waker);
    preview::setup(app, &waker);
    pdf::setup(app, &waker);
    activity::setup(app, &waker);
    transfers::setup(app, &waker);
    opening::setup(app, &waker);
    settings::setup(app, &waker);
    preview::set_editor_opener(app, opening::open_in_editor);
    preview::set_reader_display_observer(app, opening::record_recent);
    if let Some(path) = app.launch.open.clone().or_else(|| app.launch.paths.first().cloned()) {
        app.pending_open = Some(path);
    }
}

pub fn poll(app: &mut App, clock: f64, tx: &mut Tx) -> bool {
    app.clock = clock;
    let mut changed = false;
    if let Some(path) = app.pending_open.take() {
        opening::open_path(app, &path, tx);
        changed = true;
    }
    record_displayed_pdf(app);
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
    changed
}

/// Recent-file history counts a PDF once its first page has drawn, as the Swift reader does.
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
}

/// Global keys first, then the reader or the visible section.
pub fn on_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
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
        Reader::Markdown => preview::reader_key(app, ev, tx),
        Reader::Pdf => pdf::reader_key(app, ev, tx),
        Reader::None => false,
    };
    if handled {
        return true;
    }
    match ev.key {
        Key::Command('w') => app.close_reader(),
        Key::Text(ref t) if t.eq_ignore_ascii_case("x") && !ev.mods.cmd => app.close_reader(),
        Key::Escape => match app.reader_escape_at {
            Some(at) if app.clock - at <= 0.7 => app.close_reader(),
            _ => app.reader_escape_at = Some(app.clock),
        },
        _ => return false,
    }
    funnel::focus_section(app, tx);
    true
}

pub fn view(app: &App, cx: &mut Cx<App>) -> El {
    let look = app.look();
    let p = Paint { kit: look.kit(), look };
    let mut layer = Vec::new();
    let body = match app.reader {
        Reader::Markdown => preview::reader_view(app, cx, &p, &mut layer),
        Reader::Pdf => pdf::reader_view(app, cx, &p, &mut layer),
        Reader::None => {
            let content = match app.section {
                Section::Search | Section::Folders => funnel::view(app, cx, &p, &mut layer),
                Section::Activity => activity::view(app, cx, &p, &mut layer),
                Section::Transfers => transfers::view(app, cx, &p, &mut layer),
                Section::Settings => settings::view(app, cx, &p, &mut layer),
            };
            col()
                .key("workspace")
                .w_full()
                .h_full()
                .fill(look.background)
                .child(shell::section_bar(app, cx, &p, &mut layer))
                .child(content.grow(1.0).min_h(0.0).w_full())
                .child(shell::status_bar(app, &p))
        }
    };
    settings::overlays(app, cx, &p, &mut layer);
    shell::toasts(app, &p, &mut layer);
    rune_kit::root(body, layer)
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

#[allow(dead_code)]
pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
