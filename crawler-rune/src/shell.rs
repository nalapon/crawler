//! The window's frame: the section bar across the top, the status bar along the bottom, the
//! title strip every pane starts with, and toasts.

use rune::{Cx, El, Transition, col, el, row, text};

use crate::app::{App, Paint, Section};
use crate::theme::Era;
use crate::{funnel, settings, transfers};

pub const SECTION_BAR_H: f32 = 40.0;
pub const STATUS_BAR_H: f32 = 24.0;
/// Room for the traffic lights at the bar's leading edge.
const LIGHTS_INSET: f32 = 80.0;

pub struct Toast {
    pub text: String,
    pub until: f64,
}

/// Search ⌘1, Folders ⌘2, Activity ⌘3, Transfers ⌘4 and Settings ⌘, after the window controls,
/// then the Open menu, the command palette and keyboard help.
pub fn section_bar(app: &App, cx: &mut Cx<App>, p: &Paint, layer: &mut Vec<El>) -> El {
    let inset = cx.titlebar_inset().max(LIGHTS_INSET);
    let mut buttons = row().key("sections").gap(4.0).items_center();
    for section in Section::ALL {
        buttons = buttons.child(
            shell_button(p, section.key(), section.title(), app.section == section)
                .help(format!("{} · ⌘{}", section.title(), section.shortcut()))
                .on_press_tx(move |a: &mut App, tx| {
                    a.show(section);
                    funnel::focus_section(a, tx);
                }),
        );
    }
    row()
        .key("section-bar")
        .w_full()
        .h(SECTION_BAR_H)
        .pl(inset)
        .pr(10.0)
        .items_center()
        .justify_between()
        .fill(p.look.sidebar)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.look.line)
        .window_drag()
        .child(buttons)
        .child(
            row()
                .key("section-bar-trailing")
                .gap(6.0)
                .items_center()
                .child(crate::opening::open_menu(app, p, layer))
                .child(settings::bar_buttons(app, p, layer)),
        )
}

/// A section or place button drawn in the current theme: a tinted block in modern themes, an
/// inverted block in System 7 and raised or pressed bevels in Windows Retro and Linux Retro.
pub fn shell_button(p: &Paint, key: &str, label: &str, active: bool) -> El {
    let l = &p.look;
    let base = el()
        .key(key.to_string())
        .role("button")
        .label(label.to_string())
        .focusable(false)
        .h(26.0)
        .px(8.0)
        .center();
    let (fill, ink) = match (l.era(), active) {
        (Era::Modern, true) => (l.selection, l.selection_text),
        (Era::Classic | Era::Terminal, true) => (l.accent, l.accent_text),
        (Era::WindowsBevel | Era::UnixBevel, true) => (l.raised, l.text),
        (Era::WindowsBevel | Era::UnixBevel, false) => (l.raised, l.text),
        (_, false) => (rune::Color::TRANSPARENT, l.muted),
    };
    let base = match l.era() {
        Era::Modern => base.radius(6.0),
        Era::WindowsBevel | Era::UnixBevel => bevel(base, p, active),
        Era::Classic | Era::Terminal => base,
    };
    let mut content = row().gap(5.0).items_center();
    if let Some(section) = Section::ALL.into_iter().find(|s| s.key() == key) {
        content = content.child(section_icon(section, ink));
    }
    content = content.child(text(label).font(p.look.font(11.0, 500.0)).color(ink));
    p.kit
        .interactive(base.fill(fill))
        .motion(rune::tok::motion::SNAPPY)
        .press_scale(0.98)
        .child(content)
}

/// A raised bevel, or a pressed one when `down`.
pub fn bevel(e: El, p: &Paint, down: bool) -> El {
    let unix = p.look.era() == Era::UnixBevel;
    let light = crate::theme::rgb(if unix { 0xf5f4ee } else { 0xffffff });
    let dark = crate::theme::rgb(if unix { 0x4c5249 } else { 0x0a0a0a });
    let inner_light = crate::theme::rgb(if unix { 0xdddcd3 } else { 0xdfdfdf });
    let inner_dark = crate::theme::rgb(if unix { 0x8f948a } else { 0x808080 });
    let (top_left, bottom_right, inner_top, inner_bottom) = if down {
        (dark, light, inner_dark, inner_light)
    } else {
        (light, dark, inner_light, inner_dark)
    };
    e.border_sides([1.0, 0.0, 0.0, 1.0], top_left)
        .child(
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .border_sides([0.0, 1.0, 1.0, 0.0], bottom_right)
                .inert(),
        )
        .child(
            el().pin(Some(1.0), Some(1.0), Some(1.0), Some(1.0))
                .border_sides([1.0, 0.0, 0.0, 1.0], inner_top)
                .child(
                    el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                        .border_sides([0.0, 1.0, 1.0, 0.0], inner_bottom)
                        .inert(),
                )
                .inert(),
        )
}

/// The strip every pane starts with. The active pane, where keyboard navigation goes, is drawn
/// the way its theme marks an active window. No focus rings around panes.
pub fn pane_title(p: &Paint, key: &str, title: &str, detail: Option<&str>, active: bool) -> El {
    let l = &p.look;
    let mut strip = row()
        .key(key.to_string())
        .w_full()
        .h(26.0)
        .px(10.0)
        .gap(8.0)
        .items_center()
        .border_sides([0.0, 0.0, 1.0, 0.0], l.line);
    let mut ink = l.text;
    strip = match (l.era(), active) {
        (Era::Classic, true) => {
            for y in [5.0, 8.0, 11.0, 14.0, 17.0, 20.0] {
                strip = strip.child(el().absolute(0.0, y).w_full().h(1.0).fill(l.text).inert());
            }
            strip.fill(l.raised)
        }
        (Era::WindowsBevel, true) => {
            ink = rune::Color::WHITE;
            strip.linear_gradient(
                std::f32::consts::FRAC_PI_2,
                &[(0.0, l.accent), (1.0, crate::theme::rgb(0x1084d0))],
            )
        }
        (Era::UnixBevel, true) => {
            ink = l.accent_text;
            strip.fill(l.accent)
        }
        (Era::Terminal, true) => {
            ink = l.accent_text;
            strip.fill(l.accent)
        }
        _ => strip.fill(l.raised),
    };
    if active && l.era() == Era::Modern {
        strip = strip.child(
            el().key("active-dot")
                .size(6.0, 6.0)
                .radius(3.0)
                .fill(l.accent)
                .transition(Transition::opacity()),
        );
    }
    let title = text(title).font(l.font(12.0, 600.0)).color(ink).truncate();
    strip = strip.child(if active && l.era() == Era::Classic {
        title.fill(l.raised).px(6.0)
    } else {
        title
    });
    if let Some(detail) = detail {
        let detail = text(detail)
            .font(l.font(11.0, 500.0))
            .color(if active && l.era() != Era::Modern {
                ink
            } else {
                l.muted
            })
            .truncate();
        strip = strip.child(if active && l.era() == Era::Classic {
            detail.fill(l.raised).px(6.0)
        } else {
            detail
        });
    }
    strip
}

/// The funnel's status and key hints (or Activity's) on the left, the running copy on the right.
pub fn status_bar(app: &App, p: &Paint) -> El {
    let left = match app.section {
        Section::Search | Section::Folders => funnel::status_line(app),
        Section::Activity => crate::activity::hints(app),
        Section::Transfers | Section::Settings => String::new(),
    };
    let mut bar = row()
        .key("status-bar")
        .w_full()
        .h(STATUS_BAR_H)
        .px(10.0)
        .gap(12.0)
        .items_center()
        .justify_between()
        .fill(p.look.sidebar)
        .border_sides([1.0, 0.0, 0.0, 0.0], p.look.line)
        .child(
            text(left)
                .key("status-left")
                .font(p.look.font(11.0, 500.0))
                .color(p.look.muted)
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        );
    if let Some(copy) = transfers::status(app, p) {
        bar = bar.child(copy);
    }
    bar
}

pub fn toasts(app: &App, p: &Paint, layer: &mut Vec<El>) {
    if app.toasts.is_empty() {
        return;
    }
    let mut stack = col()
        .key("toasts")
        .gap(6.0)
        .items_center()
        .pin(None, None, None, Some(STATUS_BAR_H + 12.0))
        .w_full();
    for (i, t) in app.toasts.iter().enumerate() {
        stack = stack.child(
            el().key(format!("toast-{i}"))
                .px(14.0)
                .py(8.0)
                .radius(8.0)
                .fill(p.look.raised)
                .border(1.0, p.look.line)
                .shadow(6.0, 18.0, rune::Color::BLACK.alpha(0.18))
                .transition(Transition::opacity().combined(Transition::offset(0.0, 8.0)))
                .child(
                    text(t.text.as_str())
                        .font(p.look.font(13.0, 400.0))
                        .color(p.look.text),
                ),
        );
    }
    layer.push(stack.inert());
}

#[derive(Clone, Copy)]
pub enum PlaceKind {
    Home,
    Downloads,
    Desktop,
    Folder,
    Pin,
    Drive,
}

/// Theme-coloured vector icons for the funnel's places bar.
pub fn place_icon(kind: PlaceKind, p: &Paint) -> El {
    let c = p.look.text;
    let shape = match kind {
        PlaceKind::Home => rune::Shape::new()
            .stroke(vec![[1.0, 7.0], [7.0, 2.0], [13.0, 7.0]], 1.2, c)
            .stroke(
                vec![[3.0, 6.0], [3.0, 13.0], [11.0, 13.0], [11.0, 6.0]],
                1.2,
                c,
            ),
        PlaceKind::Downloads => rune::Shape::new()
            .stroke(
                vec![[7.0, 1.0], [7.0, 9.0], [4.0, 6.0], [7.0, 9.0], [10.0, 6.0]],
                1.2,
                c,
            )
            .stroke(
                vec![[2.0, 10.0], [2.0, 13.0], [12.0, 13.0], [12.0, 10.0]],
                1.2,
                c,
            ),
        PlaceKind::Desktop => rune::Shape::new()
            .stroke(
                vec![
                    [1.0, 2.0],
                    [13.0, 2.0],
                    [13.0, 10.0],
                    [1.0, 10.0],
                    [1.0, 2.0],
                ],
                1.2,
                c,
            )
            .stroke(
                vec![[7.0, 10.0], [7.0, 13.0], [4.0, 13.0], [10.0, 13.0]],
                1.2,
                c,
            ),
        PlaceKind::Pin => rune::Shape::new()
            .stroke(
                vec![
                    [4.0, 2.0],
                    [11.0, 2.0],
                    [10.0, 7.0],
                    [12.0, 9.0],
                    [3.0, 9.0],
                    [5.0, 7.0],
                    [4.0, 2.0],
                ],
                1.2,
                c,
            )
            .stroke(vec![[7.0, 9.0], [7.0, 14.0]], 1.2, c),
        PlaceKind::Drive => rune::Shape::new()
            .stroke(
                vec![
                    [2.0, 2.0],
                    [12.0, 2.0],
                    [13.0, 12.0],
                    [1.0, 12.0],
                    [2.0, 2.0],
                ],
                1.2,
                c,
            )
            .stroke(vec![[3.0, 9.0], [11.0, 9.0]], 1.2, c),
        PlaceKind::Folder => rune::Shape::new().stroke(
            vec![
                [1.0, 3.0],
                [6.0, 3.0],
                [8.0, 5.0],
                [13.0, 5.0],
                [13.0, 12.0],
                [1.0, 12.0],
                [1.0, 3.0],
            ],
            1.2,
            c,
        ),
    };
    el().size(14.0, 14.0).shape(shape).inert()
}
fn section_icon(section: Section, c: rune::Color) -> El {
    let shape = match section {
        Section::Search => rune::Shape::new()
            .stroke(vec![[9.0, 9.0], [13.0, 13.0]], 1.3, c)
            .stroke(
                vec![
                    [6.0, 1.0],
                    [9.5, 2.5],
                    [11.0, 6.0],
                    [9.5, 9.5],
                    [6.0, 11.0],
                    [2.5, 9.5],
                    [1.0, 6.0],
                    [2.5, 2.5],
                    [6.0, 1.0],
                ],
                1.3,
                c,
            ),
        Section::Folders => rune::Shape::new().stroke(
            vec![
                [1.0, 3.0],
                [6.0, 3.0],
                [8.0, 5.0],
                [13.0, 5.0],
                [13.0, 12.0],
                [1.0, 12.0],
                [1.0, 3.0],
            ],
            1.2,
            c,
        ),
        Section::Activity => rune::Shape::new().stroke(
            vec![
                [0.0, 8.0],
                [3.0, 8.0],
                [5.0, 2.0],
                [8.0, 13.0],
                [10.0, 6.0],
                [12.0, 8.0],
                [14.0, 8.0],
            ],
            1.2,
            c,
        ),
        Section::Transfers => rune::Shape::new()
            .stroke(
                vec![[1.0, 5.0], [12.0, 5.0], [9.0, 2.0], [12.0, 5.0], [9.0, 8.0]],
                1.2,
                c,
            )
            .stroke(
                vec![
                    [13.0, 10.0],
                    [2.0, 10.0],
                    [5.0, 7.0],
                    [2.0, 10.0],
                    [5.0, 13.0],
                ],
                1.2,
                c,
            ),
        Section::Settings => rune::Shape::new()
            .stroke(vec![[1.0, 3.0], [13.0, 3.0]], 1.2, c)
            .stroke(vec![[1.0, 7.0], [13.0, 7.0]], 1.2, c)
            .stroke(vec![[1.0, 11.0], [13.0, 11.0]], 1.2, c)
            .stroke(vec![[4.0, 1.0], [4.0, 5.0]], 2.5, c)
            .stroke(vec![[10.0, 5.0], [10.0, 9.0]], 2.5, c)
            .stroke(vec![[6.0, 9.0], [6.0, 13.0]], 2.5, c),
    };
    el().size(14.0, 14.0).shape(shape).inert()
}
