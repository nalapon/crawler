//! The window's frame: the section bar across the top, the status bar along the bottom, the
//! title strip every pane starts with, and toasts.

use rune::{Cx, El, Transition, col, el, row, text};

use crate::app::{App, Paint, Section};
use crate::theme::Era;
use crate::{funnel, settings, transfers};

pub const SECTION_BAR_H: f32 = 46.0;
pub const STATUS_BAR_H: f32 = 26.0;
/// Room for the traffic lights at the bar's leading edge.
const LIGHTS_INSET: f32 = 86.0;

pub struct Toast {
    pub text: String,
    pub until: f64,
}

/// Search ⌘1, Folders ⌘2, Activity ⌘3, Transfers ⌘4 and Settings ⌘, after the window controls,
/// then the command palette and keyboard help.
pub fn section_bar(app: &App, cx: &mut Cx<App>, p: &Paint, layer: &mut Vec<El>) -> El {
    let inset = cx.titlebar_inset().max(LIGHTS_INSET);
    let mut buttons = row()
        .key("sections")
        .gap(
            if matches!(p.look.era(), Era::WindowsBevel | Era::UnixBevel) {
                4.0
            } else {
                2.0
            },
        )
        .items_center();
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
        .fill(p.look.chrome())
        .border_sides([0.0, 0.0, 1.0, 0.0], p.look.separator())
        .window_drag()
        .child(buttons)
        .child(
            row()
                .key("section-bar-trailing")
                .gap(6.0)
                .items_center()
                .child(settings::bar_buttons(app, p, layer)),
        )
}

/// A section or place button drawn in the current theme: a tinted block in modern themes, an
/// inverted block in System 7 and raised or pressed bevels in Windows Retro and Linux Retro.
pub fn shell_button(p: &Paint, key: &str, label: &str, active: bool) -> El {
    shell_button_with_icon(p, key, label, active, None)
}

pub fn place_button(p: &Paint, key: &str, label: &str, active: bool, kind: PlaceKind) -> El {
    shell_button_with_icon(p, key, label, active, Some(kind))
}

fn shell_button_with_icon(
    p: &Paint,
    key: &str,
    label: &str,
    active: bool,
    place: Option<PlaceKind>,
) -> El {
    let l = &p.look;
    let section = Section::ALL.into_iter().find(|s| s.key() == key);
    let is_shell = section.is_some() || place.is_some();
    let ink = if active {
        l.selection_text
    } else if !is_shell || l.era() == Era::Modern {
        l.muted
    } else {
        l.text
    };
    let fill = if active {
        l.selection
    } else if is_shell && matches!(l.era(), Era::WindowsBevel | Era::UnixBevel) {
        l.raised
    } else {
        rune::Color::TRANSPARENT
    };
    let mut base = el()
        .key(key.to_string())
        .role("button")
        .label(label.to_string())
        .focusable(false)
        .h(28.0)
        .px(if place.is_some() || section.is_some() {
            11.0
        } else {
            8.0
        })
        .center();
    base = match l.era() {
        Era::Modern => base.radius(6.0),
        Era::WindowsBevel | Era::UnixBevel if is_shell => bevel(base, p, active),
        Era::WindowsBevel | Era::UnixBevel => base,
        Era::Classic | Era::Terminal => base,
    };
    // The indicator does not travel between sections: a travelling one is drawn above the
    // button it lands on and hides its label until it settles.
    if active && section.is_some() {
        let inset = if matches!(l.era(), Era::WindowsBevel | Era::UnixBevel) {
            2.0
        } else {
            0.0
        };
        base = base.child(
            el().key("section-indicator")
                .pin(Some(inset), Some(inset), Some(inset), Some(inset))
                .radius(if l.era() == Era::Modern { 6.0 } else { 0.0 })
                .fill(fill)
                .inert(),
        );
    } else {
        base = base.fill(fill);
    }
    let mut content = row().gap(4.0).items_center();
    if place.is_some() {
        content = content.min_w(0.0).shrink();
    }
    if let Some(kind) = place {
        content = content.child(shell_icon(p, kind.into(), active, 18.0));
    }
    if let Some(section) = section {
        content = content.child(shell_icon(p, section.into(), active, 18.0));
    }
    let action = match key {
        "folder-back" | "map-back" => Some(ControlIcon::Back),
        "folder-forward" => Some(ControlIcon::Forward),
        "folder-refresh" | "airdrop-refresh" => Some(ControlIcon::Refresh),
        "activity-pause" => Some(if active {
            ControlIcon::Play
        } else {
            ControlIcon::Pause
        }),
        "airdrop-choose" => Some(ControlIcon::Add),
        "airdrop-send" => Some(ControlIcon::Send),
        "airdrop-clear" => Some(ControlIcon::Close),
        "commands-button" => Some(ControlIcon::Command),
        "help-button" => Some(ControlIcon::Help),
        _ => None,
    };
    if key.starts_with("eject-") {
        content = content.child(shell_icon(p, ShellIcon::Eject, false, 18.0));
    } else if let Some(icon) = action {
        content = content.child(control_icon(icon, ink, 14.0));
    }
    let icon_only = key.starts_with("eject-")
        || matches!(
            key,
            "folder-back" | "folder-forward" | "folder-refresh" | "commands-button" | "help-button"
        );
    if !icon_only {
        let title = text(label)
            .font(l.font(
                if section.is_some() || place.is_some() {
                    12.0
                } else {
                    11.0
                },
                if active { 600.0 } else { 500.0 },
            ))
            .color(ink);
        content = content.child(if place.is_some() {
            title.truncate().min_w(0.0).shrink()
        } else {
            title
        });
    }
    // A button's own size and colours land at once: its label is bolder when active, and a
    // spring there slides every neighbour.
    p.kit
        .interactive(base)
        .motion(rune::Spring::NONE)
        .press_scale(0.98)
        .child(content)
}

/// Small action symbols share a fourteen-point coordinate system.
#[derive(Clone, Copy)]
pub enum ControlIcon {
    Back,
    Forward,
    Up,
    Down,
    Refresh,
    Pause,
    Play,
    Add,
    Send,
    Close,
    Command,
    Help,
}
pub fn control_icon(kind: ControlIcon, ink: rune::Color, size: f32) -> El {
    use ControlIcon::*;
    let lines: Vec<Vec<[f32; 2]>> = match kind {
        Back => vec![vec![[9., 2.], [4., 7.], [9., 12.]]],
        Forward => vec![vec![[5., 2.], [10., 7.], [5., 12.]]],
        Up => vec![vec![[3., 9.], [7., 5.], [11., 9.]]],
        Down => vec![vec![[3., 5.], [7., 9.], [11., 5.]]],
        Pause => vec![vec![[5., 3.], [5., 11.]], vec![[9., 3.], [9., 11.]]],
        Play => vec![vec![[4., 2.], [11., 7.], [4., 12.], [4., 2.]]],
        Add => vec![vec![[7., 2.], [7., 12.]], vec![[2., 7.], [12., 7.]]],
        Send => vec![
            vec![[7., 12.], [7., 2.], [3., 6.]],
            vec![[7., 2.], [11., 6.]],
        ],
        Close => vec![vec![[3., 3.], [11., 11.]], vec![[3., 11.], [11., 3.]]],
        Refresh => vec![
            vec![
                [11., 5.],
                [9., 2.],
                [5., 2.],
                [2., 5.],
                [2., 9.],
                [5., 12.],
                [9., 12.],
                [12., 9.],
            ],
            vec![[11., 1.], [11., 5.], [7., 5.]],
        ],
        Help => vec![
            circle_points(7., 7., 5.5),
            vec![[5., 5.], [6., 4.], [8., 4.], [9., 5.], [9., 6.], [7., 8.]],
            vec![[7., 10.], [7., 10.5]],
        ],
        Command => vec![vec![
            [5., 5.],
            [3., 5.],
            [2., 4.],
            [2., 3.],
            [3., 2.],
            [4., 2.],
            [5., 3.],
            [5., 11.],
            [4., 12.],
            [3., 12.],
            [2., 11.],
            [2., 10.],
            [3., 9.],
            [11., 9.],
            [12., 10.],
            [12., 11.],
            [11., 12.],
            [10., 12.],
            [9., 11.],
            [9., 3.],
            [10., 2.],
            [11., 2.],
            [12., 3.],
            [12., 4.],
            [11., 5.],
            [5., 5.],
        ]],
    };
    let mut shape = rune::Shape::new();
    for line in lines {
        shape = shape.stroke(
            line.into_iter()
                .map(|[x, y]| [x * size / 14., y * size / 14.])
                .collect::<Vec<_>>(),
            1.1,
            ink,
        );
    }
    el().size(size, size).shape(shape).inert()
}
fn circle_points(x: f32, y: f32, radius: f32) -> Vec<[f32; 2]> {
    (0..=32)
        .map(|i| {
            let a = i as f32 * std::f32::consts::TAU / 32.;
            [x + radius * a.cos(), y + radius * a.sin()]
        })
        .collect()
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
    let ink = pane_title_ink(p, active);
    let mut strip = pane_strip(p, key, active);
    if l.era() == Era::Modern {
        strip = strip.child(
            el().size(6.0, 6.0)
                .radius(3.0)
                .fill(if active { l.accent } else { l.line })
                .inert(),
        );
    }
    let mut content = row()
        .gap(8.0)
        .items_center()
        .shrink()
        // The detail elides first; the title keeps up to ~220 pt.
        .child(
            text(title)
                .font(l.font(12.0, 600.0))
                .color(ink)
                .truncate()
                .min_w((title.chars().count() as f32 * 7.5).min(220.0)),
        );
    if let Some(detail) = detail {
        content = content.child(
            text(detail)
                .font(l.font(11.0, 400.0))
                .color(ink.alpha(0.72))
                .truncate()
                .shrink(),
        );
    }
    if active && l.era() == Era::Classic {
        content = content.fill(l.background).px(6.0);
    }
    strip.child(content)
}

/// Shared strip decoration for pane owners that compose their own title content.
pub fn pane_strip(p: &Paint, key: &str, active: bool) -> El {
    let l = &p.look;
    let mut strip = row()
        .key(key.to_string())
        .w_full()
        .h(30.0)
        .px(12.0)
        .gap(8.0)
        .items_center()
        .border_sides([0.0, 0.0, 1.0, 0.0], l.separator());
    strip = match (l.era(), active) {
        (Era::Classic, true) => {
            for y in (8..23).step_by(2) {
                strip = strip.child(
                    el().pin(Some(4.0), Some(y as f32), Some(4.0), None)
                        .h(1.0)
                        .fill(l.text)
                        .inert(),
                );
            }
            strip.fill(l.background)
        }
        (Era::WindowsBevel, true) => strip.linear_gradient(
            std::f32::consts::FRAC_PI_2,
            &[(0.0, l.accent), (1.0, crate::theme::rgb(0x1084d0))],
        ),
        (Era::WindowsBevel, false) => strip.fill(crate::theme::rgb(0x808080)),
        (Era::UnixBevel | Era::Terminal, true) => strip.fill(l.accent),
        (Era::Terminal, false) => strip.fill(l.sidebar),
        _ => strip.fill(l.raised),
    };
    strip
}
pub fn pane_title_ink(p: &Paint, active: bool) -> rune::Color {
    match (p.look.era(), active) {
        (Era::WindowsBevel, true) => rune::Color::WHITE,
        (Era::Classic, false) => crate::theme::rgb(0x808080),
        (Era::WindowsBevel, false) => crate::theme::rgb(0xd4d0c8),
        (Era::Terminal | Era::UnixBevel, true) => p.look.accent_text,
        (_, true) => p.look.text,
        (_, false) => p.look.muted,
    }
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
        .border_sides([1.0, 0.0, 0.0, 0.0], p.look.separator())
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
                .w(450.0)
                .min_h(43.0)
                .px(14.0)
                .py(12.0)
                .radius(if p.look.era() == Era::Modern {
                    8.0
                } else {
                    0.0
                })
                .fill(p.look.raised)
                .border(1.0, p.look.line)
                .shadow(6.0, 18.0, rune::Color::BLACK.alpha(0.18))
                .transition(Transition::opacity().combined(Transition::offset(0.0, 8.0)))
                .child(
                    text(t.text.as_str())
                        .font(p.look.font(12.0, 400.0))
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

/// The section and place artwork at 2x, laid out as twelve roles by eleven themes and two states.
/// Keeping its native symbols and pixel artwork avoids substituting a different icon family.
#[derive(Clone, Copy)]
pub enum ShellIcon {
    Home,
    Folder,
    Downloads,
    Desktop,
    Pin,
    Drive,
    AirDrop,
    Activity,
    Preferences,
    Search = 10,
    Eject,
}
impl From<PlaceKind> for ShellIcon {
    fn from(kind: PlaceKind) -> Self {
        match kind {
            PlaceKind::Home => Self::Home,
            PlaceKind::Downloads => Self::Downloads,
            PlaceKind::Desktop => Self::Desktop,
            PlaceKind::Folder => Self::Folder,
            PlaceKind::Pin => Self::Pin,
            PlaceKind::Drive => Self::Drive,
        }
    }
}
impl From<Section> for ShellIcon {
    fn from(section: Section) -> Self {
        match section {
            Section::Search => Self::Search,
            Section::Folders => PlaceKind::Folder.into(),
            Section::Activity => Self::Activity,
            Section::Transfers => Self::AirDrop,
            Section::Settings => Self::Preferences,
        }
    }
}
pub fn shell_icon(p: &Paint, kind: ShellIcon, selected: bool, size: f32) -> El {
    let theme = crate::theme::Theme::ALL
        .iter()
        .position(|t| *t == p.look.theme)
        .unwrap_or(0);
    let index = (theme * 2 + usize::from(selected)) * 12 + kind as usize;
    match shell_artwork().get(index) {
        Some(pixels) => el().size(size, size).pixels(pixels).inert(),
        None => el().size(size, size).inert(),
    }
}
fn shell_artwork() -> &'static [rune::Pixels] {
    static ARTWORK: std::sync::OnceLock<Vec<rune::Pixels>> = std::sync::OnceLock::new();
    ARTWORK.get_or_init(|| {
        use objc2_app_kit::{NSBitmapFormat, NSBitmapImageRep};
        use objc2_foundation::NSData;
        let data = NSData::with_bytes(include_bytes!("shell-icons.png"));
        let Some(bitmap) = NSBitmapImageRep::imageRepWithData(&data) else {
            return Vec::new();
        };
        let stride = bitmap.bytesPerRow() as usize;
        if bitmap.pixelsWide() != 480
            || bitmap.pixelsHigh() != 880
            || bitmap.samplesPerPixel() != 4
            || bitmap.bitsPerSample() != 8
            || bitmap.bitmapData().is_null()
        {
            return Vec::new();
        }
        // SAFETY: the decoded bitmap owns all 880 rows for this scope; stride comes from AppKit.
        let bytes = unsafe { std::slice::from_raw_parts(bitmap.bitmapData(), stride * 880) };
        let straight = bitmap
            .bitmapFormat()
            .contains(NSBitmapFormat::AlphaNonpremultiplied);
        (0..264)
            .map(|index| {
                let mut rgba = Vec::with_capacity(40 * 40 * 4);
                for y in 0..40 {
                    let start = (index / 12 * 40 + y) * stride + (index % 12 * 40) * 4;
                    rgba.extend_from_slice(&bytes[start..start + 160]);
                }
                if !straight {
                    for px in rgba.chunks_exact_mut(4) {
                        let alpha = u32::from(px[3]);
                        for channel in &mut px[..3] {
                            *channel = (u32::from(*channel) * 255 + alpha / 2)
                                .checked_div(alpha)
                                .unwrap_or(0)
                                .min(255) as u8;
                        }
                    }
                }
                let pixels = rune::Pixels::new();
                pixels.set(40, 40, rgba);
                pixels
            })
            .collect()
    })
}
