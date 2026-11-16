use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Instant;

use rune::tok::type_;
use rune::{Color, Cx, Drag, El, GrowthSpec, Live, Phase, Tx, col, el, row, text};
use rune_kit::{Dialog, TextArea};

use crate::app::{App, Paint};
use crate::shell::{pane_title, shell_button};

use super::{
    CloseIntent, State,
    model::*,
    request_close, save,
    worker::{Command, Edit},
};

fn paint(app: &App) -> Paint {
    let look = app.look();
    Paint {
        kit: look.kit(),
        look,
    }
}

pub fn reader(app: &App, cx: &mut Cx<App>, p: &Paint, layer: &mut Vec<El>) -> El {
    let Some(path) = app.pdf.active.as_ref() else {
        return col().key("pdf-reader");
    };
    let filename = app
        .pdf
        .destination
        .as_deref()
        .unwrap_or(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "PDF".into());
    let summary = app
        .pdf
        .sources
        .borrow()
        .get(path)
        .and_then(|s| s.summary.clone());
    let dirty = summary.as_ref().is_some_and(|s| s.dirty);
    let editable = summary.as_ref().is_some_and(|s| s.editable);
    let no_text_layer = app.pdf.sources.borrow().get(path).is_some_and(|s| {
        s.pages
            .get(&s.visible.start)
            .is_some_and(|p| p.text_ready && p.characters.is_empty())
    });
    let status = app
        .pdf
        .sources
        .borrow()
        .get(path)
        .map(|s| {
            if s.pending > 0 {
                "Working…".into()
            } else if let Some(error) = &s.error {
                error.clone()
            } else if no_text_layer {
                "This page has no text layer".into()
            } else if !editable {
                "Annotations unavailable".into()
            } else {
                format!(
                    "{} pages · {}",
                    summary.as_ref().map(|s| s.page_count).unwrap_or(0),
                    if dirty {
                        "Unsaved changes"
                    } else {
                        "Drag over text"
                    }
                )
            }
        })
        .unwrap_or_else(|| "Loading PDF…".into());
    let title = format!("{filename}{}", if dirty { " •" } else { "" });
    let copy_name = filename.clone();
    let header = row()
        .key("pdf-header")
        .w_full()
        .h(48.0)
        .pl(cx.titlebar_inset().max(80.0))
        .pr(12.0)
        .gap(6.0)
        .items_center()
        .fill(p.look.sidebar)
        .window_drag()
        .child(
            pane_title(p, "pdf-title", &title, None, true)
                .grow(1.0)
                .min_w(0.0),
        )
        .child(
            shell_button(p, "pdf-copy-name", "Copy name", false)
                .help("Copy file name with extension")
                .on_press_tx(move |_: &mut App, tx| tx.copy(copy_name.clone())),
        )
        .child(
            text("Esc twice to return")
                .font(type_::LABEL)
                .color(p.look.muted),
        )
        .child(
            shell_button(p, "close-pdf", "Close", false)
                .on_press_tx(|a: &mut App, tx| request_close(a, CloseIntent::Reader, tx)),
        );
    let mut tools = row()
        .key("pdf-tools-row")
        .min_w(1160.0)
        .h(38.0)
        .px(8.0)
        .gap(3.0)
        .items_center()
        .fill(p.look.raised)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.look.line);
    for tool in Tool::ALL {
        tools = tools.child(
            shell_button(
                p,
                &format!("pdf-tool-{}", tool.label().to_ascii_lowercase()),
                tool.label(),
                app.pdf.tool == tool,
            )
            .disabled(tool != Tool::Select && (!editable || no_text_layer))
            .on_press(move |a: &mut App| a.pdf.choose_tool(tool)),
        );
    }
    for ink in Ink::ALL {
        tools = tools.child(
            el().key(format!("pdf-color-{ink:?}"))
                .role("button")
                .label(format!("{ink:?} annotation color"))
                .size(18.0, 18.0)
                .radius(9.0)
                .fill(crate::theme::rgb(ink.rgb()))
                .border(if app.pdf.ink == ink { 2.0 } else { 0.0 }, p.look.text)
                .disabled(!editable)
                .on_press(move |a: &mut App| {
                    a.pdf.ink = ink;
                    if let Some(id) = a.pdf.selected {
                        a.pdf.edit(Edit::Color { id, ink });
                    }
                }),
        );
    }
    tools = tools
        .child(
            shell_button(p, "pdf-delete", "Delete", false)
                .disabled(!editable || app.pdf.selected.is_none())
                .on_press(|a: &mut App| {
                    if let Some(id) = a.pdf.selected.take() {
                        a.pdf.edit(Edit::Delete(id));
                    }
                }),
        )
        .child(
            shell_button(p, "pdf-undo", "Undo", false)
                .disabled(!summary.as_ref().is_some_and(|s| s.undo))
                .on_press(|a: &mut App| a.pdf.edit(Edit::Undo)),
        )
        .child(
            shell_button(p, "pdf-redo", "Redo", false)
                .disabled(!summary.as_ref().is_some_and(|s| s.redo))
                .on_press(|a: &mut App| a.pdf.edit(Edit::Redo)),
        )
        .child(
            shell_button(p, "pdf-remove-all", "Remove annotations", false)
                .disabled(!editable)
                .on_press(|a: &mut App| {
                    a.pdf.choose_tool(Tool::Select);
                    a.pdf.edit(Edit::RemoveAll);
                }),
        )
        .child(el().grow(1.0))
        .child(
            shell_button(p, "pdf-zoom-out", "−", false)
                .on_press_tx(|a: &mut App, tx| a.pdf.zoom_to(a.pdf.zoom / 1.1, tx)),
        )
        .child(
            shell_button(
                p,
                "pdf-zoom-reset",
                &format!("{}%", (app.pdf.zoom * 100.0).round()),
                false,
            )
            .on_press_tx(|a: &mut App, tx| a.pdf.zoom_to(1.0, tx)),
        )
        .child(
            shell_button(p, "pdf-zoom-in", "+", false)
                .on_press_tx(|a: &mut App, tx| a.pdf.zoom_to(a.pdf.zoom * 1.1, tx)),
        )
        .child(
            shell_button(p, "pdf-save-as", "Save as…", false)
                .disabled(!editable)
                .on_press_tx(|a: &mut App, tx| save(a, true, tx)),
        )
        .child(
            shell_button(p, "pdf-save", "Save", false)
                .disabled(!editable || !dirty)
                .on_press_tx(|a: &mut App, tx| save(a, false, tx)),
        );
    let tools = col()
        .disabled(app.pdf.closing_save.is_some() || app.pdf.close_after_discard.is_some())
        .key("pdf-tools")
        .w_full()
        .h(38.0)
        .scroll_x()
        .child(tools);
    overlays(app, p, layer);
    let viewport = pages(app, cx, p, path);
    col()
        .key("pdf-reader")
        .w_full()
        .h_full()
        .fill(p.look.background)
        .child(header)
        .child(tools)
        .child(viewport.grow(1.0).min_h(0.0))
        .child(
            row()
                .key("pdf-status")
                .w_full()
                .h(24.0)
                .px(12.0)
                .items_center()
                .fill(p.look.sidebar)
                .child(
                    text(status)
                        .font(type_::LABEL)
                        .color(p.look.muted)
                        .truncate(),
                ),
        )
}

pub fn preview(app: &App, cx: &mut Cx<App>, p: &Paint, path: &Path, key: &str) -> El {
    let size = cx.layout_size(key).unwrap_or([480.0, 600.0]);
    let summary = app
        .pdf
        .sources
        .borrow()
        .get(path)
        .and_then(|s| s.summary.clone());
    let error = app
        .pdf
        .sources
        .borrow()
        .get(path)
        .and_then(|s| s.error.clone());
    let Some(summary) = summary else {
        return col()
            .key(key.to_string())
            .w_full()
            .h_full()
            .center()
            .child(text(error.unwrap_or_else(|| "Loading PDF…".into())).color(p.look.muted));
    };
    let width = (size[0] - 32.0)
        .max(100.0)
        .min((size[1] - 32.0).max(100.0) * (summary.first_size[0] / summary.first_size[1]) as f32);
    let page = page(app, p, path, 0, width, cx.env().scale, true);
    let path = path.to_path_buf();
    col()
        .key(key.to_string())
        .w_full()
        .h_full()
        .items_center()
        .pt(16.0)
        .fill(p.look.sidebar)
        .child(page.on_double_press_tx(move |a: &mut App, tx| {
            if let Err(error) = super::open_reader(a, &path, tx) {
                a.toast(error);
            }
        }))
}

fn pages(app: &App, cx: &mut Cx<App>, p: &Paint, path: &Path) -> El {
    let width = cx.layout_size("pdf-scroll").map(|s| s[0]).unwrap_or(1180.0);
    let device_scale = cx.env().scale;
    let summary = app
        .pdf
        .sources
        .borrow()
        .get(path)
        .and_then(|s| s.summary.clone());
    let Some(summary) = summary else {
        let error = app
            .pdf
            .sources
            .borrow()
            .get(path)
            .and_then(|s| s.error.clone());
        return col()
            .key("pdf-scroll")
            .w_full()
            .h_full()
            .center()
            .child(text(error.unwrap_or_else(|| "Loading PDF…".into())).color(p.look.muted));
    };
    let page_width = (width - 48.0).clamp(200.0, 900.0) * app.pdf.zoom;
    let estimate = (summary.first_size[1] / summary.first_size[0]) as f32 * page_width + 24.0;
    let build_path = path.to_path_buf();
    let visible_path = path.to_path_buf();
    let offset = app.pdf.scroll.clone();
    let last_scroll = app.pdf.last_scroll.clone();
    let scroll_clock = app.pdf.scroll_clock.clone();
    let retain_worker = app.pdf.worker.as_ref().map(|w| w.send.clone());
    let rows = col()
        .key("pdf-pages")
        .w(page_width.max(width))
        .items_center()
        .growth(GrowthSpec::default())
        .virtual_rows(
            summary.page_count,
            estimate,
            |_: &App, i| format!("pdf-page-{i}").into(),
            move |a: &App, i| {
                let p = paint(a);
                col()
                    .key(format!("pdf-page-{i}"))
                    .w_full()
                    .items_center()
                    .pt(12.0)
                    .pb(12.0)
                    .child(page(a, &p, &build_path, i, page_width, device_scale, false))
            },
        )
        .virtual_rows_version((path.to_path_buf(), summary.page_count))
        .on_visible_rows(move |a: &mut App, visible| {
            let started = Instant::now();
            if let Some(source) = a.pdf.sources.borrow_mut().get_mut(&visible_path) {
                let retained =
                    visible.start.saturating_sub(1)..(visible.end + 1).min(summary.page_count);
                source.visible = retained.clone();
                source.surfaces.retain(|i, surface| {
                    if retained.contains(i) {
                        true
                    } else {
                        surface.epoch.fetch_add(1, Ordering::Relaxed);
                        false
                    }
                });
                source.overlays.retain(|i, _| retained.contains(i));
                source.drawn.retain(|i, _| retained.contains(i));
                // Geometry is tiny. Text and annotations stay only around the viewport.
                let anchor = a.pdf.stroke.as_ref().map(|s| s.selection.start.page);
                source
                    .pages
                    .retain(|i, _| retained.contains(i) || Some(*i) == anchor);
            }
            if let Some(worker) = &retain_worker {
                let _ = worker.send(Command::Retain {
                    path: visible_path.clone(),
                    pages: visible.start.saturating_sub(1)
                        ..(visible.end + 1).min(summary.page_count),
                });
            }
            a.pdf.max_scroll_ms.set(
                a.pdf
                    .max_scroll_ms
                    .get()
                    .max(started.elapsed().as_secs_f64() * 1000.0),
            );
            false
        });
    col()
        .key("pdf-scroll")
        .w_full()
        .h_full()
        .min_h(0.0)
        .focusable(true)
        .fill(p.look.sidebar)
        .scroll_xy()
        .scroll_anchor()
        .live_scale(&app.pdf.zoom_live)
        .live_offset(&app.pdf.pan_live)
        .on_scroll_offset(move |position| {
            if offset.get() != position.offset {
                last_scroll.set(Some(scroll_clock.get()));
            }
            offset.set(position.offset);
        })
        .on_layout_tx(|a: &mut App, rect, tx| {
            a.pdf.viewport.set(rect);
            tx.live_only();
        })
        .on_wheel(|_: &mut App, _, tx| {
            tx.live_only();
            false
        })
        .on_magnify(|a: &mut App, e, tx| {
            let old = a.pdf.pending_zoom / a.pdf.zoom;
            let next = (a.pdf.pending_zoom * (1.0 + e.delta)).clamp(0.25, 8.0);
            let ratio = next / a.pdf.zoom;
            let viewport = a.pdf.viewport.get();
            let pan = a.pdf.pan_live.get();
            let center = [viewport[2] * 0.5, viewport[3] * 0.5];
            let point = [
                center[0] + (e.x - center[0]) * old + pan[0],
                center[1] + (e.y - center[1]) * old + pan[1],
            ];
            if let Some(path) = &a.pdf.active {
                if let Some(source) = a.pdf.sources.borrow().get(path) {
                    let window_y = viewport[1] + point[1];
                    if let Some((page, _)) = source.drawn.iter().min_by(|(_, a), (_, b)| {
                        let distance =
                            |r: &[f32; 4]| (r[1] - window_y).max(0.0).max(window_y - r[1] - r[3]);
                        distance(a).total_cmp(&distance(b))
                    }) {
                        a.pdf.pinch_page = *page;
                    }
                }
            }
            a.pdf.pan_live.set([
                point[0] - center[0] - (point[0] - center[0] - pan[0]) * ratio / old,
                point[1] - center[1] - (point[1] - center[1] - pan[1]) * ratio / old,
            ]);
            a.pdf.pending_zoom = next;
            a.pdf.zoom_live.set(ratio);
            a.pdf.pinch_at = Some(a.clock);
            tx.wake_after(std::time::Duration::from_millis(125));
            tx.live_only();
            true
        })
        .child(rows)
}

fn page(
    app: &App,
    p: &Paint,
    path: &Path,
    index: usize,
    width: f32,
    device_scale: f32,
    preview: bool,
) -> El {
    let (size, content, surface, overlays) = {
        let mut sources = app.pdf.sources.borrow_mut();
        let Some(source) = sources.get_mut(path) else {
            return el();
        };
        let size = source
            .pages
            .get(&index)
            .map(|p| p.size)
            .or_else(|| source.summary.as_ref().map(|s| s.first_size))
            .unwrap_or([612.0, 792.0]);
        let content = source.pages.get(&index).cloned();
        let surface = source.surfaces.entry(index).or_default().clone();
        let overlays = source
            .overlays
            .entry(index)
            .or_insert_with(|| (0..128).map(|_| Live::new([0.0; 4])).collect())
            .clone();
        (size, content, surface, overlays)
    };
    let height = (size[1] / size[0]) as f32 * width;
    let render_path = path.to_path_buf();
    let drag_path = path.to_path_buf();
    let mut page = el()
        .key(format!("pdf-sheet-{index}"))
        .role("document")
        .label(format!("PDF page {}", index + 1))
        .size(width, height)
        .fill(Color::WHITE)
        .shadow(2.0, 8.0, Color::BLACK.alpha(0.16))
        .child(
            el().key("bitmap")
                .w_full()
                .h_full()
                .pixels(&surface.pixels)
                .inert(),
        )
        .child(
            el().key("sharp-bitmap")
                .absolute(0.0, 0.0)
                .size(0.0, 0.0)
                .pixels(&surface.sharp)
                .live_rect(&surface.crop)
                .inert(),
        )
        .on_drawn_layout_tx(move |a: &mut App, rect, tx| {
            if let Some(source) = a.pdf.sources.borrow_mut().get_mut(&render_path) {
                source.drawn.insert(index, rect);
            }
            request_raster(&a.pdf, &render_path, index, rect, device_scale, preview);
            tx.live_only();
        });
    if let Some(content) = content {
        let scale = width as f64 / size[0];
        for mark in &content.marks {
            for (i, rect) in mark.rects.iter().enumerate() {
                let r = scaled(*rect, scale);
                let selected = app.pdf.selected == Some(mark.id);
                let mut overlay = el()
                    .key(format!("pdf-mark-{}-{i}", mark.id))
                    .absolute(r[0], r[1])
                    .size(r[2], r[3]);
                match mark.tool {
                    Tool::Highlight => {
                        overlay = overlay.fill(crate::theme::rgb(mark.color).alpha(if selected {
                            0.48
                        } else {
                            0.32
                        }));
                    }
                    Tool::Underline => {
                        overlay = overlay.border_sides(
                            [0.0, 0.0, if selected { 3.0 } else { 1.5 }, 0.0],
                            crate::theme::rgb(mark.color),
                        );
                    }
                    Tool::Note => {
                        let mut dots = row().absolute(0.0, (r[3] - 2.0).max(0.0)).h(2.0).gap(3.0);
                        for _ in 0..((r[2] / 5.0).ceil() as usize).min(2048) {
                            dots = dots.child(
                                el().size(2.0, 2.0)
                                    .radius(1.0)
                                    .fill(crate::theme::rgb(mark.color)),
                            );
                        }
                        overlay = overlay.child(dots).help(if mark.note.is_empty() {
                            "Click to edit note"
                        } else {
                            &mark.note
                        });
                    }
                    Tool::Select => {}
                }
                page = page.child(overlay);
            }
        }
        for (i, link) in content.links.iter().enumerate() {
            let r = scaled(link.rect, scale);
            let target = link.target.clone();
            let title = match &target {
                LinkTarget::Url(url) => url.clone(),
                LinkTarget::Page(i) => format!("Go to page {}", i + 1),
            };
            page = page.child(
                el().key(format!("pdf-link-{i}"))
                    .role("link")
                    .label(title.clone())
                    .help(title)
                    .absolute(r[0], r[1])
                    .size(r[2], r[3])
                    .on_press_tx(move |a: &mut App, tx| {
                        if a.pdf.tool != Tool::Select {
                            return;
                        }
                        match &target {
                            LinkTarget::Url(url) => {
                                if url.starts_with("https://")
                                    || url.starts_with("http://")
                                    || url.starts_with("mailto:")
                                {
                                    let _ = tx.open_url(url);
                                }
                            }
                            LinkTarget::Page(index) => {
                                tx.scroll_into_view(format!("pdf-page-{index}"))
                            }
                        }
                    }),
            );
        }
    }
    if !preview {
        let fill = if app.pdf.tool == Tool::Select {
            p.look.accent.alpha(0.25)
        } else {
            crate::theme::rgb(app.pdf.ink.rgb()).alpha(0.35)
        };
        for (i, live) in overlays.iter().enumerate() {
            page = page.child(
                el().key(format!("pdf-live-selection-{index}-{i}"))
                    .absolute(0.0, 0.0)
                    .size(0.0, 0.0)
                    .fill(fill)
                    .live_rect(live)
                    .inert(),
            );
        }
        page = page
            .on_drag(move |a: &mut App, drag, tx| drag_text(a, &drag_path, index, drag, width, tx));
    }
    page
}

fn scaled(rect: Rect, scale: f64) -> [f32; 4] {
    [
        (rect.x * scale) as f32,
        (rect.y * scale) as f32,
        (rect.w * scale) as f32,
        (rect.h * scale) as f32,
    ]
}

/// A low-resolution whole page appears first. Sharp rendering covers only the viewport's
/// intersection with the page, with a margin, and stays below 16 million pixels per request.
fn request_raster(
    state: &State,
    path: &Path,
    index: usize,
    drawn: [f32; 4],
    device_scale: f32,
    preview: bool,
) {
    let started = Instant::now();
    let viewport = state.viewport.get();
    let mut sources = state.sources.borrow_mut();
    let Some(source) = sources.get_mut(path) else {
        return;
    };
    if !preview && !source.visible.contains(&index) {
        return;
    }
    let size = source
        .pages
        .get(&index)
        .map(|p| p.size)
        .or_else(|| source.summary.as_ref().map(|s| s.first_size))
        .unwrap_or([612.0, 792.0]);
    let surface = source.surfaces.entry(index).or_default();
    let generation = state.epoch.load(Ordering::Relaxed);
    if surface.pixels.size().0 == 0 && surface.requested.is_none() {
        let full_width = 480;
        let height = (full_width as f64 * size[1] / size[0]).ceil().min(8192.0) as u32;
        let key = RasterKey {
            page: index,
            full_width,
            x: 0,
            y: 0,
            width: full_width,
            height,
        };
        state.send(Command::Render {
            path: path.into(),
            key,
            pixels: surface.pixels.clone(),
            epoch: state.epoch.clone(),
            generation,
            sharp: false,
            crop: None,
        });
        surface.requested = Some(key);
    }
    if drawn[2] <= 0.0 || drawn[3] <= 0.0 {
        return;
    }
    let intersects =
        preview || drawn[1] + drawn[3] >= viewport[1] && drawn[1] <= viewport[1] + viewport[3];
    if !intersects {
        return;
    }
    let full_width = (drawn[2] * device_scale).ceil().clamp(1.0, 65536.0) as u32;
    let full_height = (full_width as f64 * size[1] / size[0]).ceil() as u32;
    let scale = full_width as f32 / drawn[2];
    let x = if preview {
        0
    } else {
        (((viewport[0] - drawn[0] - 64.0).max(0.0) * scale) as u32 / 128) * 128
    };
    let y = if preview {
        0
    } else {
        (((viewport[1] - drawn[1] - 128.0).max(0.0) * scale) as u32 / 256) * 256
    };
    let width = if preview {
        full_width.min(4096)
    } else {
        ((viewport[2] + 128.0) * scale).ceil() as u32
    }
    .min(full_width.saturating_sub(x))
    .min(4096);
    let height = if preview {
        full_height.min(4096)
    } else {
        ((viewport[3] + 384.0) * scale).ceil() as u32
    }
    .min(full_height.saturating_sub(y))
    .min(4096);
    if width == 0 || height == 0 {
        return;
    }
    let key = RasterKey {
        page: index,
        full_width,
        x,
        y,
        width,
        height,
    };
    if surface.requested != Some(key) {
        let rect = [
            x as f32 / scale,
            y as f32 / scale,
            width as f32 / scale,
            height as f32 / scale,
        ];
        let generation = surface.epoch.fetch_add(1, Ordering::Relaxed) + 1;
        state.send(Command::Render {
            path: path.into(),
            key,
            pixels: surface.sharp.clone(),
            epoch: surface.epoch.clone(),
            generation,
            sharp: true,
            crop: Some((surface.crop.clone(), rect)),
        });
        surface.requested = Some(key);
    }
    state.max_scroll_ms.set(
        state
            .max_scroll_ms
            .get()
            .max(started.elapsed().as_secs_f64() * 1000.0),
    );
}

fn nearest(page: &Page, x: f64, y: f64) -> Option<usize> {
    page.characters
        .iter()
        .min_by(|a, b| {
            let distance = |r: Rect| {
                let dx = (r.x - x).max(0.0).max(x - r.x - r.w);
                let dy = (r.y - y).max(0.0).max(y - r.y - r.h);
                dy * dy * 16.0 + dx * dx
            };
            distance(a.rect).total_cmp(&distance(b.rect))
        })
        .map(|c| c.index)
}

fn selection_rects(page: &Page, from: usize, to: usize) -> Vec<Rect> {
    let mut rects: Vec<Rect> = Vec::new();
    for character in page
        .characters
        .iter()
        .filter(|c| (from.min(to)..=from.max(to)).contains(&c.index))
    {
        if let Some(last) = rects
            .last_mut()
            .filter(|r| (r.y - character.rect.y).abs() < character.rect.h * 0.4)
        {
            *last = last.union(character.rect);
        } else {
            rects.push(character.rect);
        }
    }
    rects
}

fn drag_text(app: &mut App, path: &Path, index: usize, drag: &Drag, width: f32, tx: &mut Tx) {
    let content = app
        .pdf
        .sources
        .borrow()
        .get(path)
        .and_then(|s| s.pages.get(&index))
        .cloned();
    let Some(page) = content else {
        tx.live_only();
        return;
    };
    let scale = width as f64 / page.size[0];
    let point = [drag.x as f64 / scale, drag.y as f64 / scale];
    if drag.phase == Phase::Down {
        app.pdf.clear_stroke();
        app.pdf.all_selected = false;
        if let Some(character) = nearest(&page, point[0], point[1]) {
            let start = TextPoint {
                page: index,
                character,
            };
            app.pdf.stroke = Some(Stroke {
                selection: TextRange { start, end: start },
            });
        }
    }
    let window = drag.window_point();
    if let Some(source) = app.pdf.sources.borrow().get(path) {
        let end_page = source
            .drawn
            .iter()
            .filter(|(i, _)| {
                source
                    .pages
                    .get(i)
                    .is_some_and(|p| !p.characters.is_empty())
            })
            .min_by(|(_, a), (_, b)| {
                let distance =
                    |r: &[f32; 4]| (r[1] - window[1]).max(0.0).max(window[1] - r[1] - r[3]);
                distance(a).total_cmp(&distance(b))
            });
        if let Some((index, rect)) = end_page {
            if let Some(page) = source.pages.get(index) {
                let scale = rect[2] as f64 / page.size[0];
                let end = nearest(
                    page,
                    (window[0] - rect[0]) as f64 / scale,
                    (window[1] - rect[1]) as f64 / scale,
                );
                if let (Some(stroke), Some(character)) = (app.pdf.stroke.as_mut(), end) {
                    stroke.selection.end = TextPoint {
                        page: *index,
                        character,
                    };
                }
            }
        }
        if let Some(stroke) = &app.pdf.stroke {
            let (start, end) = stroke.selection.ordered();
            for (index, overlays) in &source.overlays {
                let rects = if (start.page..=end.page).contains(index) {
                    source
                        .pages
                        .get(index)
                        .map(|p| {
                            selection_rects(
                                p,
                                if *index == start.page {
                                    start.character
                                } else {
                                    0
                                },
                                if *index == end.page {
                                    end.character
                                } else {
                                    usize::MAX
                                },
                            )
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let page_scale = source
                    .drawn
                    .get(index)
                    .zip(source.pages.get(index))
                    .map(|(r, p)| r[2] as f64 / p.size[0])
                    .unwrap_or(scale);
                for (i, live) in overlays.iter().enumerate() {
                    let rect = rects
                        .get(i)
                        .map(|r| {
                            let mut r = scaled(*r, page_scale);
                            if matches!(app.pdf.tool, Tool::Underline | Tool::Note) {
                                r[1] += r[3] - 2.0;
                                r[3] = 2.0;
                            }
                            r
                        })
                        .unwrap_or([0.0; 4]);
                    live.set(rect);
                }
            }
        }
    }
    if drag.phase != Phase::Up {
        let viewport = app.pdf.viewport.get();
        let offset = app.pdf.scroll.get();
        if window[1] > viewport[1] + viewport[3] - 24.0 {
            tx.scroll_to_y("pdf-scroll", offset[1] + 120.0);
            tx.drag_repeat_after(std::time::Duration::from_millis(16));
        } else if window[1] < viewport[1] + 24.0 {
            tx.scroll_to_y("pdf-scroll", (offset[1] - 120.0).max(0.0));
            tx.drag_repeat_after(std::time::Duration::from_millis(16));
        }
        tx.live_only();
        return;
    }
    if drag.cancelled() {
        app.pdf.clear_stroke();
        tx.live_only();
        return;
    }
    let click = drag.dx.abs() + drag.dy.abs() < 4.0;
    if click && app.pdf.tool == Tool::Select {
        app.pdf.clear_stroke();
        if let Some(mark) = page
            .marks
            .iter()
            .find(|m| m.rects.iter().any(|r| r.contains(point[0], point[1])))
        {
            app.pdf.selected = Some(mark.id);
            if mark.tool == Tool::Note || !mark.note.is_empty() {
                app.pdf.note_anchor = format!("pdf-mark-{}-0", mark.id);
                app.pdf.note = rune::TextEdit::new(&mark.note);
                app.pdf.note_open = true;
                app.pdf.new_note = None;
                tx.focus("pdf-note-text", false);
            }
        } else {
            app.pdf.selected = None;
            app.pdf.selection = None;
        }
        return;
    }
    if let Some(stroke) = app.pdf.stroke.as_ref() {
        let selection = stroke.selection;
        app.pdf.selection = Some(selection);
        match app.pdf.tool {
            Tool::Select => {
                tx.live_only();
            }
            Tool::Note => {
                app.pdf.note_anchor = format!("pdf-live-selection-{}-0", selection.end.page);
                app.pdf.new_note = Some(selection);
                app.pdf.note = rune::TextEdit::new("");
                app.pdf.note_open = true;
                tx.focus("pdf-note-text", false);
            }
            tool => {
                app.pdf.edit(Edit::Add {
                    selection,
                    tool,
                    ink: app.pdf.ink,
                    note: String::new(),
                });
                app.pdf.clear_stroke();
            }
        }
    }
}

fn overlays(app: &App, p: &Paint, layer: &mut Vec<El>) {
    if app.pdf.note_open {
        let editor = TextArea::new(&app.pdf.note)
            .placeholder("Write a note")
            .on_edit(|a: &mut App| &mut a.pdf.note)
            .build(&p.kit)
            .key("pdf-note-text")
            .w_full()
            .h(140.0);
        let body = col()
            .key("pdf-note-popover")
            .role("dialog")
            .label("PDF note")
            .w(320.0)
            .padding(12.0)
            .gap(12.0)
            .child(text("PDF note").font(type_::LABEL).color(p.look.text))
            .child(editor)
            .child(
                row()
                    .justify_end()
                    .gap(8.0)
                    .child(
                        shell_button(p, "pdf-note-cancel", "Cancel", false).on_press(
                            |a: &mut App| {
                                a.pdf.note_open = false;
                                a.pdf.new_note = None;
                                a.pdf.clear_stroke();
                            },
                        ),
                    )
                    .child(
                        shell_button(p, "pdf-note-save", "Done", true)
                            .on_press(|a: &mut App| super::commit_note(a)),
                    ),
            )
            .modal(false, |a: &mut App| {
                a.pdf.note_open = false;
                a.pdf.new_note = None;
                a.pdf.clear_stroke();
            });
        let body = rune_kit::popover::raised(&p.kit, body, p.kit.radius.sm);
        layer.push(rune_kit::popover::anchored(
            &p.kit,
            body,
            &app.pdf.note_anchor,
            rune_kit::Placement::BottomStart,
            8.0,
        ));
    }
    if let Some(intent) = app.pdf.close_prompt {
        Dialog::new("pdf-unsaved", "Save changes to this PDF?")
            .description("Your annotations have not been saved.")
            .open(true)
            .fit()
            .click_outside(false)
            .on_close(|a: &mut App| a.pdf.close_prompt = None)
            .child(
                row()
                    .padding(16.0)
                    .gap(8.0)
                    .justify_end()
                    .child(
                        shell_button(p, "pdf-close-discard", "Discard changes", false).on_press_tx(
                            move |a: &mut App, tx| {
                                a.pdf.close_after_discard = Some(intent);
                                a.pdf.close_prompt = None;
                                a.pdf.edit(Edit::Discard);
                                a.pdf.note_open = false;
                                a.pdf.clear_stroke();
                                tx.wake_after(std::time::Duration::from_millis(1));
                            },
                        ),
                    )
                    .child(
                        shell_button(p, "pdf-close-cancel", "Cancel", false)
                            .on_press(|a: &mut App| a.pdf.close_prompt = None),
                    )
                    .child(shell_button(p, "pdf-close-save", "Save", true).on_press_tx(
                        move |a: &mut App, tx| {
                            a.pdf.close_after_save = Some(intent);
                            a.pdf.close_prompt = None;
                            save(a, false, tx);
                        },
                    )),
            )
            .build(&p.kit, layer);
    }
}
