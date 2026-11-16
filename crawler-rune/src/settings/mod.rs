//! Settings, the context-aware command palette and the keyboard shortcut sheet.
mod platform;
use crate::{
    app::{App, Paint, Section},
    funnel, opening, preview, shell,
    theme::{Era, Theme},
    transfers,
};
use rune::{
    Cx, El, FileDialogOptions, Key, KeyEvent, Latest, TextEdit, Tx, Waker, col, el, row, text,
};
use rune_kit::{Dialog, Select, SelectAppearance, TextInput};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Commands,
    Themes,
    Editors,
    FolderEditors,
    Drives,
}
#[derive(Clone)]
enum Action {
    Section(Section),
    Folder(PathBuf),
    Open,
    Preview,
    CopyPath(bool),
    Reveal,
    AirDrop,
    Page(Page),
    Theme(Theme),
    Editor(PathBuf),
    Terminal,
    ChooseFolder,
    Pin,
    ChoosePin,
    Refresh,
    Help,
}
#[derive(Clone)]
struct Command {
    id: String,
    title: String,
    detail: String,
    aliases: &'static str,
    action: Action,
}
pub struct State {
    open: bool,
    page: Page,
    query: TextEdit,
    selection: usize,
    files: Vec<PathBuf>,
    folder: Option<PathBuf>,
    context: platform::Context,
    request: Latest<platform::Context>,
    font_theme: Option<Theme>,
    editor_open: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            page: Page::Commands,
            query: TextEdit::new(""),
            selection: 0,
            files: vec![],
            folder: None,
            context: platform::Context::default(),
            request: Latest::default(),
            font_theme: None,
            editor_open: false,
        }
    }
}
pub fn setup(app: &mut App, waker: &Waker) {
    app.settings.request.set_waker(waker.clone());
}
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let theme = app.theme_preview.unwrap_or(app.theme);
    // Global font slots also cover controls drawn by other workers.
    if app.settings.font_theme != Some(theme) {
        app.settings.font_theme = Some(theme);
        let family = match theme {
            Theme::System7 => "Geneva",
            Theme::WindowsRetro => "Tahoma",
            Theme::LinuxRetro => "Helvetica",
            Theme::Terminal => "IBM Plex Mono",
            _ => "Inter",
        };
        tx.set_font_families(family, "IBM Plex Mono");
    }
    let mut changed = false;
    if let Some(context) = app.settings.request.take() {
        app.settings.context = context;
        changed = app.settings.open;
    }
    changed
}
fn matches(query: &str, title: &str) -> bool {
    let title = title.to_lowercase();
    query.to_lowercase().split_whitespace().all(|word| {
        let mut chars = word.chars();
        let mut next = chars.next();
        for c in title.chars() {
            if Some(c) == next {
                next = chars.next();
            }
        }
        next.is_none()
    })
}
fn commands(app: &App) -> Vec<Command> {
    let s = &app.settings;
    let mut commands = Vec::new();
    let mut add = |id: String, title: String, detail: String, aliases, action| {
        commands.push(Command {
            id,
            title,
            detail,
            aliases,
            action,
        })
    };
    match s.page {
        Page::Themes => {
            for theme in Theme::ALL {
                add(
                    format!("theme-{theme:?}"),
                    theme.name().into(),
                    if theme == app.theme {
                        "Current".into()
                    } else {
                        theme.detail().into()
                    },
                    "appearance skin color",
                    Action::Theme(theme),
                );
            }
        }
        Page::Drives => {
            for (path, name, _) in funnel::connected_volumes(app) {
                add(
                    path.display().to_string(),
                    name,
                    path.display().to_string(),
                    "volume disk",
                    Action::Folder(path.clone()),
                );
            }
        }
        Page::Editors => {
            for application in &s.context.applications {
                add(
                    application.path.display().to_string(),
                    application.name.clone(),
                    String::new(),
                    "app editor",
                    Action::Editor(application.path.clone()),
                );
            }
        }
        Page::FolderEditors => {
            for editor in &app.opening.editors {
                add(
                    editor.bundle_id.clone(),
                    editor.name.clone(),
                    String::new(),
                    "editor",
                    Action::Editor(editor.path.clone()),
                );
            }
        }
        Page::Commands => {
            for section in Section::ALL {
                if section != app.section {
                    add(
                        section.key().into(),
                        format!("Go to {}", section.title()),
                        format!("⌘{}", section.shortcut()),
                        "section navigate",
                        Action::Section(section),
                    );
                }
            }
            for (id, title, path) in [
                ("home", "Go to Home folder", crate::app::home()),
                (
                    "downloads",
                    "Go to Downloads",
                    crate::app::home().join("Downloads"),
                ),
                (
                    "desktop",
                    "Go to Desktop",
                    crate::app::home().join("Desktop"),
                ),
            ] {
                add(
                    id.into(),
                    title.into(),
                    String::new(),
                    "navigate folders",
                    Action::Folder(path),
                );
            }
            if let Some(pin) = funnel::pinned_folder(app) {
                add(
                    "pinned".into(),
                    format!("Go to pinned folder: {}", basename(&pin)),
                    String::new(),
                    "favorite folders",
                    Action::Folder(pin),
                );
            }
            add(
                "drives".into(),
                "Go to connected drive…".into(),
                String::new(),
                "volume disk locations",
                Action::Page(Page::Drives),
            );
            add(
                "path".into(),
                "Go to folder…".into(),
                "⌘L".into(),
                "go location path",
                Action::ChooseFolder,
            );
            if let Some(file) = s.files.first() {
                let name = basename(file);
                add(
                    "open".into(),
                    format!("Open {name}"),
                    "↵".into(),
                    "file folder",
                    Action::Open,
                );
                if !s.context.selected_is_folder {
                    add(
                        "open-with".into(),
                        format!("Open {name} with…"),
                        String::new(),
                        "open app editor",
                        Action::Page(Page::Editors),
                    );
                    add(
                        "preview".into(),
                        format!("Quick Look {name}"),
                        "Space".into(),
                        "preview",
                        Action::Preview,
                    );
                }
                add(
                    "copy-path".into(),
                    format!("Copy path to {name}"),
                    String::new(),
                    "absolute clipboard",
                    Action::CopyPath(false),
                );
                if s.folder.is_some() {
                    add(
                        "relative-path".into(),
                        format!("Copy relative path to {name}"),
                        String::new(),
                        "clipboard",
                        Action::CopyPath(true),
                    );
                }
                add(
                    "reveal".into(),
                    format!("Reveal {name} in Finder"),
                    String::new(),
                    "show file",
                    Action::Reveal,
                );
                if !s.context.selected_is_folder || s.files.len() > 1 {
                    add(
                        "airdrop".into(),
                        if s.files.len() > 1 {
                            format!("Send {} selected items with AirDrop", s.files.len())
                        } else {
                            format!("Send {name} with AirDrop")
                        },
                        String::new(),
                        "share transfers",
                        Action::AirDrop,
                    );
                }
            }
            if let Some(folder) = &s.folder {
                if let Some(parent) = folder.parent() {
                    add(
                        "parent".into(),
                        "Go to parent folder".into(),
                        String::new(),
                        "up",
                        Action::Folder(parent.to_path_buf()),
                    );
                }
                add(
                    "refresh".into(),
                    "Refresh folder".into(),
                    "⌘R".into(),
                    "reload",
                    Action::Refresh,
                );
                add(
                    "pin".into(),
                    format!("Pin {}", basename(folder)),
                    String::new(),
                    "favorite current folder places",
                    Action::Pin,
                );
                add(
                    "terminal".into(),
                    "Open current folder in Terminal".into(),
                    String::new(),
                    "shell console",
                    Action::Terminal,
                );
                add(
                    "folder-editor".into(),
                    "Open current folder in editor…".into(),
                    String::new(),
                    "code project",
                    Action::Page(Page::FolderEditors),
                );
            }
            add(
                "choose-pin".into(),
                "Change pinned folder…".into(),
                String::new(),
                "favorite",
                Action::ChoosePin,
            );
            add(
                "theme".into(),
                "Change theme…".into(),
                String::new(),
                "skin appearance color dark light",
                Action::Page(Page::Themes),
            );
            add(
                "help".into(),
                "Keyboard shortcuts".into(),
                "⌘?".into(),
                "help keys",
                Action::Help,
            );
        }
    }
    commands.retain(|c| matches(&s.query.text, &format!("{} {}", c.title, c.aliases)));
    commands
}
fn basename(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
fn open_palette(app: &mut App, tx: &mut Tx) {
    app.show_help = false;
    app.settings.open = true;
    app.settings.page = Page::Commands;
    app.settings.query = TextEdit::new("");
    app.settings.selection = 0;
    app.settings.files = funnel::selection(app);
    app.settings.folder = funnel::current_folder(app);
    app.settings.context = platform::Context::default();
    app.settings
        .request
        .request(app.settings.files.first().cloned(), platform::context);
    tx.focus_when_shown("command-query", false);
}
fn close_palette(app: &mut App) {
    app.settings.open = false;
    app.theme_preview = None;
    app.settings.request.cancel();
}
fn preview_selection(app: &mut App, tx: &mut Tx) {
    let choices = commands(app);
    app.settings.selection = app.settings.selection.min(choices.len().saturating_sub(1));
    if app.settings.page == Page::Themes {
        app.theme_preview = choices.get(app.settings.selection).and_then(|c| {
            if let Action::Theme(t) = c.action {
                Some(t)
            } else {
                None
            }
        });
        tx.color_motion(rune::tok::motion::CALM);
    }
    if let Some(c) = choices.get(app.settings.selection) {
        tx.scroll_into_view_in("command-results", format!("command-{}", c.id));
    }
}
fn set_page(app: &mut App, page: Page, tx: &mut Tx) {
    app.settings.page = page;
    app.settings.query = TextEdit::new("");
    app.settings.selection = 0;
    app.theme_preview = None;
    preview_selection(app, tx);
    tx.focus("command-query", false);
}
fn activate(app: &mut App, tx: &mut Tx) {
    let Some(c) = commands(app).get(app.settings.selection).cloned() else {
        return;
    };
    if let Action::Page(page) = c.action {
        set_page(app, page, tx);
        return;
    }
    let files = app.settings.files.clone();
    let folder = app.settings.folder.clone();
    let folder_editor = app.settings.page == Page::FolderEditors;
    close_palette(app);
    match c.action {
        Action::Section(section) => {
            app.show(section);
            funnel::focus_section(app, tx);
        }
        Action::Folder(path) => funnel::open_folder(app, path, tx),
        Action::Open => {
            if let Some(file) = files.first() {
                opening::open_path(app, file, tx);
            }
        }
        Action::Preview => {
            if let Some(file) = files.first() {
                preview::toggle_space(app, file, tx);
            }
        }
        Action::CopyPath(relative) => {
            if let Some(file) = files.first() {
                let path = if relative {
                    folder
                        .as_deref()
                        .map(|base| relative_path(base, file))
                        .unwrap_or_else(|| file.display().to_string())
                } else {
                    file.display().to_string()
                };
                tx.copy(path);
                app.toast("Path copied.");
            }
        }
        Action::Reveal => opening::reveal(&files),
        Action::AirDrop => transfers::airdrop(app, files, tx),
        Action::Theme(theme) => {
            tx.color_motion(rune::tok::motion::CALM);
            app.set_theme(theme);
        }
        Action::Editor(editor) => {
            if let Some(path) = if folder_editor {
                folder
            } else {
                files.first().cloned()
            } {
                opening::open_with_application(app, &path, &editor, !folder_editor);
            }
        }
        Action::Terminal => {
            if let Some(folder) = folder {
                opening::open_terminal(app, &folder);
            }
        }
        Action::ChooseFolder => tx.open_folder(
            FileDialogOptions {
                title: "Go to folder".into(),
                directory: folder,
                ..Default::default()
            },
            |a: &mut App, result, tx| match result {
                Ok(Some(path)) => funnel::open_folder(a, path, tx),
                Ok(None) => {}
                Err(error) => a.toast(error.to_string()),
            },
        ),
        Action::ChoosePin => funnel::choose_pinned_folder(app, tx),
        Action::Pin => {
            if let Some(folder) = folder {
                funnel::pin_folder(app, folder);
            }
        }
        Action::Refresh => {
            funnel::on_key(
                app,
                &KeyEvent {
                    key: Key::Command('r'),
                    mods: rune::Mods {
                        cmd: true,
                        ..Default::default()
                    },
                },
                tx,
            );
        }
        Action::Help => app.show_help = true,
        Action::Page(_) => {}
    }
}
fn relative_path(base: &Path, path: &Path) -> String {
    let a: Vec<_> = base.components().collect();
    let b: Vec<_> = path.components().collect();
    let common = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
    let parts = std::iter::repeat_n("..".to_string(), a.len() - common)
        .chain(
            b[common..]
                .iter()
                .map(|p| p.as_os_str().to_string_lossy().into_owned()),
        )
        .collect::<Vec<_>>();
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}
fn palette_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match ev.key {
        Key::Escape => {
            if app.settings.page == Page::Commands {
                close_palette(app);
            } else {
                set_page(app, Page::Commands, tx);
            }
        }
        Key::Enter => activate(app, tx),
        Key::Down => {
            app.settings.selection =
                (app.settings.selection + 1).min(commands(app).len().saturating_sub(1));
            preview_selection(app, tx);
        }
        Key::Up => {
            app.settings.selection = app.settings.selection.saturating_sub(1);
            preview_selection(app, tx);
        }
        _ => return false,
    }
    true
}
pub fn on_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.is_command('k') || (ev.is_command('p') && ev.mods.shift) {
        if app.settings.open {
            close_palette(app);
        } else {
            open_palette(app, tx);
        }
        return true;
    }
    if ev.is_command('?') || (ev.is_command('/') && ev.mods.shift) {
        close_palette(app);
        app.show_help = !app.show_help;
        return true;
    }
    if app.show_help {
        if ev.key == Key::Escape {
            app.show_help = false;
        }
        return true;
    }
    if app.settings.open {
        palette_key(app, ev, tx);
        return true;
    }
    false
}
pub fn bar_buttons(_app: &App, p: &Paint, _layer: &mut Vec<El>) -> El {
    row()
        .key("bar-buttons")
        .gap(4.0)
        .child(
            shell::shell_button(p, "commands-button", "Commands", false)
                .help("Commands · ⌘K")
                .on_press_tx(open_palette),
        )
        .child(
            shell::shell_button(p, "help-button", "?", false)
                .help("Keyboard shortcuts · ⌘?")
                .on_press(|a: &mut App| a.show_help = !a.show_help),
        )
}
pub fn overlays(app: &App, cx: &mut Cx<App>, p: &Paint, layer: &mut Vec<El>) {
    if app.settings.open {
        let title = match app.settings.page {
            Page::Commands => "Commands",
            Page::Themes => "Choose a theme",
            Page::Editors => "Open with",
            Page::FolderEditors => "Choose editor",
            Page::Drives => "Connected drives",
        };
        let input = TextInput::new(&app.settings.query)
            .placeholder(if app.settings.page == Page::Commands {
                "What would you like to do?"
            } else {
                "Filter choices…"
            })
            .label(title)
            .height(34.0)
            .on_edit(|a: &mut App| &mut a.settings.query)
            .on_change(|a: &mut App, tx| {
                a.settings.selection = 0;
                preview_selection(a, tx);
            })
            .on_key(palette_key)
            .build(&p.kit)
            .key("command-query");
        let mut rows = col().key("command-rows").w_full().gap(4.0);
        let choices = commands(app);
        for (i, c) in choices.iter().enumerate() {
            let active = i == app.settings.selection;
            rows = rows.child(
                p.kit
                    .interactive(
                        row()
                            .key(format!("command-{}", c.id))
                            .role("option")
                            .label(&c.title)
                            .w_full()
                            .h(42.0)
                            .px(12.0)
                            .gap(12.0)
                            .items_center()
                            .radius(p.kit.radius.sm)
                            .fill(if active {
                                p.look.selection
                            } else {
                                rune::Color::TRANSPARENT
                            })
                            .child(
                                text(&c.title)
                                    .font(p.look.font(14.0, 500.0))
                                    .color(if active {
                                        p.look.selection_text
                                    } else {
                                        p.look.text
                                    })
                                    .truncate()
                                    .grow(1.0)
                                    .min_w(0.0),
                            )
                            .child(text(&c.detail).font(p.look.font(11.0, 400.0)).color(
                                if active {
                                    p.look.selection_text
                                } else {
                                    p.look.muted
                                },
                            )),
                    )
                    .on_hover_change(move |a: &mut App, hover, tx| {
                        if hover {
                            a.settings.selection = i;
                            preview_selection(a, tx);
                        }
                    })
                    .on_press_tx(move |a: &mut App, tx| {
                        a.settings.selection = i;
                        activate(a, tx);
                    }),
            );
        }
        if choices.is_empty() {
            let empty = match app.settings.page {
                Page::Drives if funnel::connected_volumes(app).is_empty() => "No connected drives",
                Page::FolderEditors if app.opening.editors.is_empty() => {
                    "No installed editors support opening this folder."
                }
                Page::Editors if app.settings.context.applications.is_empty() => {
                    "No installed apps support this file."
                }
                _ => "No matches",
            };
            rows = rows.child(
                text(empty)
                    .font(p.kit.ty.ui)
                    .color(p.look.muted)
                    .padding(12.0),
            );
        }
        let body = col()
            .w_full()
            .min_h(0.0)
            .gap(10.0)
            .px(16.0)
            .pb(12.0)
            .child(input)
            .child(
                col()
                    .key("command-results")
                    .h(220.0)
                    .scroll(rune::ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(rows),
            )
            .child(
                text("↑ ↓ Select    ↵ Run    Esc Back / close")
                    .font(p.look.font(11.0, 400.0))
                    .color(p.look.muted),
            );
        Dialog::new("commands", title)
            .open(true)
            .width(660.0)
            .fit()
            .on_close(close_palette)
            .child(body)
            .build(&p.kit, layer);
    }
    if app.show_help {
        let height = cx.layout_size("workspace").map_or(760.0, |size| size[1]);
        help_sheet(app, p, layer, (height - 140.0).max(180.0));
    }
}
fn help_sheet(app: &App, p: &Paint, layer: &mut Vec<El>, max_height: f32) {
    let mut entries = vec![
        ("⌘1 … ⌘4", "Search, Folders, Activity, Transfers"),
        ("⌘,", "Settings"),
        ("⌘K / ⌘⇧P", "Commands"),
    ];
    if app.section == Section::Activity {
        entries.extend([
            ("↑ ↓ / j k", "Select process or box"),
            ("Tab", "Switch process list / map"),
            ("↵ / l / →", "Expand group / explore box"),
            ("h / ←", "Collapse group / go back"),
            ("gg / G", "First / last item"),
            ("⌃D / ⌃U", "Half page of items"),
            ("/ or i", "Focus process filter"),
            ("Esc", "Leave filter / go back"),
            ("Space", "Pause / resume sampling"),
            ("1 / 2 / 3", "Map memory / CPU / disk"),
            ("m / c / d", "Sort memory / CPU / disk"),
            ("Repeat sort key", "Reverse sort order"),
            ("⌘R", "Refresh sample"),
            ("⌘I", "Inspect process / group"),
            ("⌘⌫", "Quit selected process…"),
        ]);
    } else {
        entries.extend([
            ("⌘P", "Search This Mac"),
            ("↑ ↓ / j k", "Select file"),
            ("← → / h l", "Parent folder / open folder"),
            ("⇧↑ ⇧↓ / ⇧J ⇧K", "Extend selection in Folders"),
            ("⌘A", "Select all in Folders"),
            ("⌃D / ⌃U", "Half page of results"),
            ("gg / G", "First / last result"),
            ("/ or i", "Focus search"),
            ("$", "Search the current folder"),
            ("↵", "Open file / enter folder"),
            ("⌘↵", "Show in Folders"),
            ("Space", "Preview file"),
            ("Esc Esc", "Close the reader"),
            ("⌘[ / ⌘]", "Back / forward in Folders"),
            ("⌘L", "Go to a folder path"),
            ("⌘R", "Refresh"),
        ]);
    }
    entries.push(("⌘?", "Toggle this sheet"));
    let mut body = col()
        .key("shortcut-entries")
        .max_h(max_height)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .gap(5.0)
        .px(18.0)
        .pb(18.0);
    for (keys, action) in entries {
        body = body.child(
            row()
                .gap(12.0)
                .items_center()
                .child(
                    text(keys)
                        .w(140.0)
                        .font(p.look.font(11.0, 500.0))
                        .color(p.look.text),
                )
                .child(
                    text(action)
                        .font(p.look.font(11.0, 400.0))
                        .color(p.look.muted),
                ),
        );
    }
    Dialog::new("shortcuts", "Keyboard shortcuts")
        .open(true)
        .width(480.0)
        .fit()
        .on_close(|a: &mut App| a.show_help = false)
        .child(body)
        .build(&p.kit, layer);
}

fn theme_tile(theme: Theme, selected: bool, p: &Paint) -> El {
    let look = theme.look();
    let mut miniature = col()
        .key("miniature")
        .size(72.0, 44.0)
        .radius(if theme.era() == Era::Modern { 4.0 } else { 0.0 })
        .fill(look.background)
        .border(1.0, look.line)
        .child(
            row()
                .h(10.0)
                .w_full()
                .fill(look.sidebar)
                .px(4.0)
                .items_center()
                .child(el().size(14.0, 4.0).fill(look.selection)),
        );
    let mut lines = col().grow(1.0).padding(4.0).gap(4.0);
    for width in [28.0, 28.0, 28.0, 18.0] {
        lines = lines.child(el().w(width).h(2.0).fill(look.text.alpha(0.7)));
    }
    miniature = miniature.child(
        row()
            .grow(1.0)
            .child(
                col()
                    .w(28.0)
                    .h_full()
                    .padding(3.0)
                    .gap(4.0)
                    .border_sides([0.0, 1.0, 0.0, 0.0], look.line)
                    .child(el().w_full().h(5.0).fill(look.selection))
                    .child(el().size(18.0, 2.0).fill(look.muted))
                    .child(el().size(18.0, 2.0).fill(look.muted)),
            )
            .child(lines),
    );
    p.kit
        .interactive(
            row()
                .key(format!("setting-theme-{theme:?}"))
                .role("radio")
                .label(theme.name())
                .h(64.0)
                .grow(1.0)
                .min_w(0.0)
                .px(10.0)
                .gap(12.0)
                .items_center()
                .radius(p.kit.radius.md)
                .fill(if selected {
                    p.look.selection
                } else {
                    rune::Color::TRANSPARENT
                })
                .border(if selected { 2.0 } else { 0.0 }, p.look.accent)
                .child(miniature)
                .child(
                    col()
                        .min_w(0.0)
                        .gap(3.0)
                        .child(
                            text(theme.name())
                                .font(p.look.font(12.0, 600.0))
                                .color(if selected {
                                    p.look.selection_text
                                } else {
                                    p.look.text
                                })
                                .truncate(),
                        )
                        .child(
                            text(theme.detail())
                                .font(p.look.font(11.0, 400.0))
                                .color(if selected {
                                    p.look.selection_text
                                } else {
                                    p.look.muted
                                })
                                .truncate(),
                        ),
                ),
        )
        .press_scale(0.98)
        .on_press_tx(move |a: &mut App, tx| {
            tx.color_motion(rune::tok::motion::CALM);
            a.set_theme(theme);
        })
}
pub fn view(app: &App, cx: &mut Cx<App>, p: &Paint, layer: &mut Vec<El>) -> El {
    let heading = |title: &str, detail: &str| {
        col()
            .gap(8.0)
            .child(
                text(title)
                    .font(p.look.font(15.0, 600.0))
                    .color(p.look.text),
            )
            .child(
                text(detail)
                    .font(p.look.font(12.0, 400.0))
                    .color(p.look.muted)
                    .wrap(),
            )
    };
    let mut content=col().key("settings-content").w_full().max_w(972.0).padding(36.0).gap(12.0)
        .child(heading("Theme","Colours, controls and lines for the whole window. Retro themes use square edges and classic bevels."));
    let width = cx
        .layout_size("settings-scroll")
        .map_or(1180.0, |size| size[0]);
    let columns = (((width - 72.0).clamp(560.0, 900.0) + 12.0) / 212.0) as usize;
    for themes in Theme::ALL.chunks(columns) {
        let mut line = row().w_full().gap(12.0);
        for theme in themes {
            line = line.child(theme_tile(*theme, *theme == app.theme, p));
        }
        for _ in themes.len()..columns {
            line = line.child(el().grow(1.0));
        }
        content = content.child(line);
    }
    content = content.child(
        heading(
            "Default text editor",
            "Code and plain text open with this app, as do files you send from the reader with E.",
        )
        .mt(24.0),
    );
    let mut options = vec!["System default".to_string()];
    options.extend(app.opening.editors.iter().map(|e| e.name.clone()));
    let current = app
        .opening
        .default_editor
        .as_ref()
        .and_then(|id| app.opening.editors.iter().find(|e| &e.bundle_id == id))
        .map_or("System default", |e| e.name.as_str());
    let editors = Select::new("default-editor", &options)
        .label("Default text editor")
        .appearance(SelectAppearance::Field)
        .width(300.0)
        .current(Some(current))
        .open(app.settings.editor_open)
        .on_toggle(|a: &mut App| a.settings.editor_open = !a.settings.editor_open)
        .on_close(|a: &mut App| a.settings.editor_open = false)
        .on_choose(|a: &mut App, name, _| {
            let bundle = a
                .opening
                .editors
                .iter()
                .find(|e| e.name == name)
                .map(|e| e.bundle_id.clone());
            opening::set_default_editor(a, bundle);
            a.settings.editor_open = false;
        })
        .build(&p.kit, layer);
    content=content.child(editors)
        .child(text(if app.opening.editors.is_empty() { "No text editor found. Installed editor discovery runs in the background.".into() } else if app.opening.default_editor.as_ref().is_some_and(|id|!app.opening.editors.iter().any(|e|&e.bundle_id==id)) { "Your saved editor is unavailable. Choose another editor above.".into() } else { format!("{} installed editors · This choice applies only in Crawler.",app.opening.editors.len()) }).font(p.look.font(11.0,400.0)).color(p.look.muted).wrap())
        .child(heading("PDFs, Markdown and Word","PDFs, Markdown and Word documents open in Crawler's reader. Images and video use the macOS default apps.").mt(24.0));
    col()
        .key("settings")
        .min_h(0.0)
        .child(shell::pane_title(
            p,
            "settings-title",
            "Settings",
            None,
            true,
        ))
        .child(
            col()
                .key("settings-scroll")
                .grow(1.0)
                .min_h(0.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(content),
        )
}

pub fn palette_visible(app: &App) -> bool {
    app.settings.open
}
