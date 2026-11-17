use crate::{app::Paint, theme::Era};
use rune::{El, Icon, el, icon, row, text};

pub const COPY: Icon = Icon {
    name: "copy-name",
    view: 24.0,
    body: r#"<g fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"><rect x="8" y="8" width="12" height="14" rx="2"/><path d="M5 17H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v1"/></g>"#,
};
pub const CLOSE: Icon = Icon {
    name: "reader-close",
    view: 24.0,
    body: r#"<path d="m6 6 12 12M18 6 6 18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/>"#,
};
pub const BACK: Icon = Icon {
    name: "reader-back",
    view: 24.0,
    body: r#"<path d="m14 5-7 7 7 7" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
pub const SELECT: Icon = Icon {
    name: "pdf-select",
    view: 24.0,
    body: r#"<path d="m5 3 14 11-7 1-3 6Z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};
pub const HIGHLIGHT: Icon = Icon {
    name: "pdf-highlight",
    view: 24.0,
    body: r#"<g fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"><path d="m8 14 9-11 5 4-9 11Z M8 14l-3 5 4 2 4-3M2 22h10"/></g>"#,
};
pub const UNDERLINE: Icon = Icon {
    name: "pdf-underline",
    view: 24.0,
    body: r#"<path d="M6 3v9a6 6 0 0 0 12 0V3M4 22h16" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/>"#,
};
pub const NOTE: Icon = Icon {
    name: "pdf-note",
    view: 24.0,
    body: r#"<g fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"><path d="M4 3h16a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2h-9l-6 4v-4H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z M6 8h12M6 12h9"/></g>"#,
};
pub const DELETE: Icon = Icon {
    name: "pdf-delete",
    view: 24.0,
    body: r#"<g fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M3 5h18M9 5V2h6v3M6 5l1 17h10l1-17M10 9v9M14 9v9"/></g>"#,
};
pub const UNDO: Icon = Icon {
    name: "pdf-undo",
    view: 24.0,
    body: r#"<path d="m8 3-5 5 5 5M3 8h12a6 6 0 0 1 0 12h-3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
pub const REDO: Icon = Icon {
    name: "pdf-redo",
    view: 24.0,
    body: r#"<path d="m16 3 5 5-5 5M21 8H9a6 6 0 0 0 0 12h3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
pub const ERASE: Icon = Icon {
    name: "pdf-remove",
    view: 24.0,
    body: r#"<path d="m3 13 11-11 8 8L10 22H7l-5-5a3 3 0 0 1 1-4ZM8 8l9 9M10 22h12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};
pub const SAVE: Icon = Icon {
    name: "pdf-save",
    view: 24.0,
    body: r#"<path d="M12 2v13m-5-5 5 5 5-5M5 5H3v17h18V5h-2" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// Reader controls follow Swift's borderless ActionButton, including in retro themes.
pub fn button(
    p: &Paint,
    key: &str,
    title: &str,
    symbol: Option<&'static Icon>,
    active: bool,
) -> El {
    let ink = if active {
        p.look.selection_text
    } else {
        p.look.muted
    };
    let mut content = row().items_center().gap(4.0);
    if let Some(symbol) = symbol {
        content = content.child(icon(symbol, 14.0).color(ink));
    }
    if !title.is_empty() {
        content = content.child(text(title).font(p.look.font(11.0, 400.0)).color(ink));
    }
    p.kit
        .interactive(
            el().key(key.to_owned())
                .role("button")
                .label(title.to_owned())
                .h(30.0)
                .px(8.0)
                .center()
                .radius(if p.look.era() == Era::Modern {
                    4.0
                } else {
                    0.0
                })
                .fill(if active {
                    p.look.selection
                } else {
                    rune::Color::TRANSPARENT
                }),
        )
        .child(content)
}
