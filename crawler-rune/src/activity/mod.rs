//! Real process sampling, a virtualized table and a navigable resource map.
mod model;
mod sampling;

use crate::app::{App, Paint, Reader, Section};
use crate::shell;
use model::{ListRow, Metric, Node};
use rune::{Align, Cx, El, Key, KeyEvent, Latest, TextEdit, Tx, Waker, col, el, row, text};
use rune_kit::{
    Cell, Dialog, MenuItem, RollingText, SortDir, Table, TableColumn, TableState, TableStyle,
    TextInput,
};
use sampling::{Process, Sample};
use std::{borrow::Cow, collections::HashSet, time::Duration};

pub struct State {
    sample: Sample,
    request: Latest<Sample>,
    control: Latest<Result<(), String>>,
    next_sample: f64,
    active: bool,
    paused: bool,
    filter: TextEdit,
    table: TableState,
    rows: Vec<ListRow>,
    expanded: HashSet<String>,
    metric: Metric,
    roots: Vec<Node>,
    scope: Vec<String>,
    map_selection: Option<String>,
    map_focused: bool,
    pending_g: bool,
    inspect: bool,
    quit: Option<std::sync::Arc<Process>>,
    menu: Option<rune::PointerPress>,
}
impl Default for State {
    fn default() -> Self {
        let mut table = TableState::new().key("process-table");
        table.set_sort("memory", SortDir::Descending);
        Self {
            sample: Sample::default(),
            request: Latest::default(),
            control: Latest::default(),
            next_sample: 0.0,
            active: false,
            paused: false,
            filter: TextEdit::new(""),
            table,
            rows: Vec::new(),
            expanded: HashSet::new(),
            metric: Metric::Memory,
            roots: Vec::new(),
            scope: Vec::new(),
            map_selection: None,
            map_focused: false,
            pending_g: false,
            inspect: false,
            quit: None,
            menu: None,
        }
    }
}
pub fn setup(app: &mut App, waker: &Waker) {
    app.activity.request.set_waker(waker.clone());
    app.activity.control.set_waker(waker.clone());
}

pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let s = &mut app.activity;
    let active = app.section == Section::Activity && app.reader == Reader::None;
    if !active {
        if s.active {
            s.request.cancel();
            s.sample.counters.clear();
        }
        s.active = false;
        return false;
    }
    if !s.active {
        s.next_sample = app.clock;
        s.active = true;
    }
    let mut changed = false;
    if let Some(sample) = s.request.take() {
        changed = s.sample.processes != sample.processes
            || s.sample.error != sample.error
            || s.sample.unavailable != sample.unavailable;
        s.sample = sample;
        s.next_sample = app.clock + 1.0;
        if changed {
            rebuild(s);
        }
    }
    if !s.paused {
        if !s.request.is_pending() && app.clock >= s.next_sample {
            let previous = std::mem::take(&mut s.sample.counters);
            s.request.request(previous, sampling::read);
        }
        if !s.request.is_pending() {
            tx.wake_after(Duration::from_secs_f64(
                (s.next_sample - app.clock).max(0.05),
            ));
        }
    }
    if let Some(result) = app.activity.control.take() {
        app.toast(match result {
            Ok(()) => "The quit request was sent.".into(),
            Err(e) => e,
        });
        changed = true;
    }
    changed
}

fn rebuild(s: &mut State) {
    let groups = model::groups(&s.sample.processes, &s.filter.text);
    s.expanded.retain(|id| groups.iter().any(|g| &g.id == id));
    let (column, dir) = s
        .table
        .sort()
        .map(|(c, d)| (c.to_owned(), d))
        .unwrap_or(("memory".into(), SortDir::Descending));
    let sort_rows = |rows: &mut Vec<ListRow>| {
        rows.sort_by(|a, b| {
            let order = match column.as_str() {
                "name" => rune_kit::natural_cmp(&a.name, &b.name),
                _ => {
                    let m = if column == "cpu" {
                        Metric::Cpu
                    } else if column == "disk" {
                        Metric::Disk
                    } else {
                        Metric::Memory
                    };
                    a.value(m)
                        .unwrap_or(-1.0)
                        .total_cmp(&b.value(m).unwrap_or(-1.0))
                }
            };
            dir.apply(order).then_with(|| a.id.cmp(&b.id))
        })
    };
    let mut parents: Vec<_> = groups
        .iter()
        .map(|g| ListRow {
            id: format!("group:{}", g.id),
            group: g.id.clone(),
            name: g.name.clone(),
            subtitle: if g.processes.len() == 1 {
                format!("PID {}", g.processes[0].id.pid)
            } else {
                format!("{} processes", g.processes.len())
            },
            processes: g.processes.clone(),
            child: false,
        })
        .collect();
    sort_rows(&mut parents);
    let mut rows = Vec::new();
    for parent in parents {
        if s.expanded.contains(&parent.group) && parent.processes.len() > 1 {
            let mut children: Vec<_> = parent
                .processes
                .iter()
                .map(|p| ListRow {
                    id: p.id.key(),
                    group: parent.group.clone(),
                    name: p.name.clone(),
                    subtitle: format!("PID {}", p.id.pid),
                    processes: vec![p.clone()],
                    child: true,
                })
                .collect();
            sort_rows(&mut children);
            rows.push(parent);
            rows.extend(children);
        } else {
            rows.push(parent);
        }
    }
    s.rows = rows;
    if !s
        .rows
        .iter()
        .any(|r| Some(r.id.as_str()) == s.table.cursor())
    {
        if let Some(first) = s.rows.first() {
            s.table.select(first.id.clone());
        } else {
            s.table.clear_selection();
        }
    }
    s.roots = model::roots(
        groups
            .into_iter()
            .map(|group| {
                let children = model::forest(&group.processes, s.metric);
                Node::group(format!("group:{}", group.id), group.name, children)
            })
            .collect(),
    );
    let old_scope = std::mem::take(&mut s.scope);
    for id in old_scope {
        if visible(s)
            .iter()
            .any(|n| n.id == id && !n.visible().is_empty())
        {
            s.scope.push(id);
        } else {
            break;
        }
    }
}
fn visible(s: &State) -> &[Node] {
    let mut nodes = s.roots.as_slice();
    for id in &s.scope {
        let Some(found) = nodes.iter().find(|n| n.id == *id) else {
            return &[];
        };
        nodes = found.visible();
    }
    nodes
}
fn find_node<'a>(nodes: &'a [Node], id: &str) -> Option<&'a Node> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        if let Some(found) = find_node(node.visible(), id) {
            return Some(found);
        }
    }
    None
}
fn node_path(nodes: &[Node], id: &str) -> Option<Vec<String>> {
    for n in nodes {
        if n.id == id {
            return Some(vec![n.id.clone()]);
        }
        if let Some(mut path) = node_path(n.visible(), id) {
            path.insert(0, n.id.clone());
            return Some(path);
        }
    }
    None
}
fn selected(s: &State) -> Option<ListRow> {
    if s.map_focused {
        s.map_selection
            .as_ref()
            .and_then(|id| find_node(visible(s), id))
            .map(|node| {
                let mut processes = Vec::new();
                node.members(&mut processes);
                ListRow {
                    id: node.id.clone(),
                    group: String::new(),
                    name: node.name.clone(),
                    subtitle: format!("{} processes", processes.len()),
                    processes,
                    child: false,
                }
            })
    } else {
        s.rows
            .iter()
            .find(|r| Some(r.id.as_str()) == s.table.cursor())
            .cloned()
    }
}
fn select_map(s: &mut State, id: String) {
    s.map_focused = true;
    s.map_selection = Some(id.clone());
    if let Some(node) = find_node(visible(s), &id) {
        let row = s.rows.iter().find(|r| r.id == id).or_else(|| {
            let mut members = Vec::new();
            node.members(&mut members);
            members.first().and_then(|p| {
                s.rows
                    .iter()
                    .find(|r| r.processes.len() == 1 && r.processes[0].id == p.id)
            })
        });
        if let Some(row) = row {
            s.table.select(row.id.clone());
        }
    }
}
fn toggle_pause(app: &mut App) {
    let s = &mut app.activity;
    s.paused = !s.paused;
    s.request.cancel();
    s.sample.counters.clear();
    s.next_sample = app.clock;
}
fn explore(s: &mut State) {
    if s.map_focused {
        if let Some(id) = s.map_selection.as_ref() {
            let nodes = visible(s);
            if find_node(nodes, id).is_some_and(|n| !n.visible().is_empty()) {
                if let Some(path) = node_path(nodes, id) {
                    s.scope.extend(path);
                    s.map_selection = visible(s).first().map(|n| n.id.clone());
                }
            }
        }
    } else if let Some(row) = selected(s) {
        if row.processes.len() > 1 {
            if !s.expanded.remove(&row.group) {
                s.expanded.insert(row.group);
            }
            rebuild(s);
        } else if let Some(process) = row.processes.first() {
            let group_id = format!("group:{}", row.group);
            if let Some(path) = node_path(&s.roots, &group_id) {
                let process_id = process.id.key();
                s.scope = path;
                let nodes = visible(s);
                let id = nodes
                    .iter()
                    .find(|n| n.id == process_id)
                    .or_else(|| nodes.first())
                    .map(|n| n.id.clone());
                if let Some(id) = id {
                    select_map(s, id);
                }
            }
        }
    }
}
fn back(s: &mut State) {
    if s.map_focused {
        s.scope.pop();
        s.map_selection = visible(s).first().map(|n| n.id.clone());
    } else if let Some(row) = selected(s) {
        s.expanded.remove(&row.group);
        s.table.select(format!("group:{}", row.group));
        rebuild(s);
    }
}
fn move_selection(s: &mut State, amount: isize, edge: Option<bool>, tx: &mut Tx) {
    if s.map_focused {
        let nodes = visible(s);
        if nodes.is_empty() {
            return;
        }
        let current = nodes
            .iter()
            .position(|n| Some(&n.id) == s.map_selection.as_ref())
            .unwrap_or(0);
        let at = edge.map_or_else(
            || current.saturating_add_signed(amount).min(nodes.len() - 1),
            |end| if end { nodes.len() - 1 } else { 0 },
        );
        s.map_selection = Some(nodes[at].id.clone());
    } else {
        if s.rows.is_empty() {
            return;
        }
        let current = s
            .rows
            .iter()
            .position(|r| Some(r.id.as_str()) == s.table.cursor())
            .unwrap_or(0);
        let at = edge.map_or_else(
            || current.saturating_add_signed(amount).min(s.rows.len() - 1),
            |end| if end { s.rows.len() - 1 } else { 0 },
        );
        s.table.reveal(s.rows[at].id.clone(), tx);
    }
}
pub fn on_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if app.activity.quit.is_some() || app.activity.inspect {
        if ev.key == Key::Escape {
            app.activity.quit = None;
            app.activity.inspect = false;
        }
        return true;
    }
    if tx.focused() == Some("process-filter") {
        return false;
    }
    let ch = match &ev.key {
        Key::Text(t) => t.as_str(),
        _ => "",
    };
    match &ev.key {
        Key::Command('r') => {
            app.activity.request.cancel();
            app.activity.next_sample = app.clock;
        }
        Key::Command('i') => app.activity.inspect = true,
        Key::Backspace if ev.mods.cmd => {
            app.activity.quit = selected(&app.activity)
                .and_then(|r| (r.processes.len() == 1).then(|| r.processes[0].clone()))
                .filter(|p| p.id.pid > 1 && p.id.started > 0);
            if app.activity.quit.is_none() {
                app.toast("Expand the group and select an individual process.");
            }
        }
        Key::Tab => {
            app.activity.map_focused = !app.activity.map_focused;
            if app.activity.map_focused {
                let nodes = visible(&app.activity);
                let id = nodes
                    .iter()
                    .find(|n| Some(n.id.as_str()) == app.activity.table.cursor())
                    .or_else(|| nodes.first())
                    .map(|n| n.id.clone());
                if let Some(id) = id {
                    select_map(&mut app.activity, id);
                }
            }
            tx.blur();
        }
        Key::Down => move_selection(&mut app.activity, 1, None, tx),
        Key::Up => move_selection(&mut app.activity, -1, None, tx),
        Key::Home => move_selection(&mut app.activity, 0, Some(false), tx),
        Key::End => move_selection(&mut app.activity, 0, Some(true), tx),
        Key::PageDown => move_selection(&mut app.activity, 12, None, tx),
        Key::PageUp => move_selection(&mut app.activity, -12, None, tx),
        Key::Enter | Key::Right => explore(&mut app.activity),
        Key::Left => back(&mut app.activity),
        Key::Escape => {
            if !app.activity.scope.is_empty() {
                back(&mut app.activity);
            } else if !app.activity.filter.text.is_empty() {
                app.activity.filter = TextEdit::new("");
                rebuild(&mut app.activity);
            } else {
                app.activity.map_focused = false;
            }
        }
        _ if ch == "/" || ch == "i" => tx.focus("process-filter", false),
        _ if ch == " " => toggle_pause(app),
        _ if ch == "j" => move_selection(&mut app.activity, 1, None, tx),
        _ if ch == "k" => move_selection(&mut app.activity, -1, None, tx),
        _ if ch == "l" => explore(&mut app.activity),
        _ if ch == "h" => back(&mut app.activity),
        _ if ch == "G" => move_selection(&mut app.activity, 0, Some(true), tx),
        _ if ch == "g" => {
            if app.activity.pending_g {
                move_selection(&mut app.activity, 0, Some(false), tx);
            }
            app.activity.pending_g = !app.activity.pending_g;
            return true;
        }
        _ if ev.mods.ctrl && (ch == "d" || ch == "u") => {
            move_selection(&mut app.activity, if ch == "d" { 8 } else { -8 }, None, tx)
        }
        _ if ["1", "2", "3"].contains(&ch) => {
            app.activity.metric = if ch == "1" {
                Metric::Memory
            } else if ch == "2" {
                Metric::Cpu
            } else {
                Metric::Disk
            };
            rebuild(&mut app.activity);
        }
        _ if ["m", "c", "d"].contains(&ch) => {
            let column = if ch == "m" {
                "memory"
            } else if ch == "c" {
                "cpu"
            } else {
                "disk"
            };
            let dir = if app.activity.table.sort().is_some_and(|(c, _)| c == column) {
                app.activity.table.next_sort(column)
            } else {
                SortDir::Descending
            };
            app.activity.table.set_sort(column, dir);
            rebuild(&mut app.activity);
        }
        _ => return false,
    }
    app.activity.pending_g = false;
    true
}
pub fn hints(_app: &App) -> String {
    "j/k Move   Tab Panel   Enter/l Explore   h Back   / Filter   Space Pause   ⌘I Inspect   ⌘⌫ Quit   1/2/3 Map   m/c/d Sort".into()
}

fn tiles(
    nodes: &[Node],
    s: &State,
    p: &Paint,
    width: f64,
    height: f64,
    depth: usize,
    parent_color: Option<rune::Color>,
) -> El {
    if nodes.is_empty() {
        return el().grow(1.0).center().child(
            text("No measured usage")
                .font(p.kit.ty.ui)
                .color(p.look.muted),
        );
    }
    if nodes.len() == 1 {
        let n = &nodes[0];
        let id = n.id.clone();
        let double = id.clone();
        let chosen = if s.map_focused {
            s.map_selection.as_ref() == Some(&n.id)
        } else {
            s.table.cursor() == Some(n.id.as_str())
        };
        let palette = if p.look.dark {
            [0x2c4058, 0x2b453b, 0x4b4030, 0x42364f, 0x503931, 0x2b444b]
        } else {
            [0xd9e9fc, 0xdbece4, 0xeee2cd, 0xe8dff4, 0xf5dfd9, 0xd8ebef]
        };
        let hash = n
            .name
            .bytes()
            .fold(0_u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        let fill = if p.look.era() == crate::theme::Era::Classic {
            p.look.background
        } else {
            parent_color
                .unwrap_or_else(|| crate::theme::rgb(palette[hash as usize % palette.len()]))
        };
        let container = !s.scope.is_empty()
            && depth == 0
            && !n.visible().is_empty()
            && width > 110.0
            && height > 95.0;
        let mut content = col()
            .key(n.id.clone())
            .min_w(0.0)
            .min_h(0.0)
            .grow(1.0)
            .padding(8.0)
            .gap(3.0)
            .clip_rounded(0.0)
            .fill(fill)
            .radius(if p.look.era() == crate::theme::Era::Classic {
                0.0
            } else {
                5.0
            })
            .border(
                if chosen { 2.0 } else { 1.0 },
                if chosen { p.look.accent } else { p.look.line },
            )
            .role("button")
            .label(format!("{}: {}", n.name, s.metric.format(Some(n.value))))
            .child(
                text(&n.name)
                    .font(p.look.font(12.0, 600.0))
                    .color(p.look.text)
                    .truncate(),
            )
            .child(
                text(s.metric.format(Some(n.value)))
                    .font(p.look.font(11.0, 400.0))
                    .color(p.look.text)
                    .truncate(),
            );
        if container {
            content = content.child(tiles(
                n.visible(),
                s,
                p,
                (width - 16.0).max(1.0),
                (height - 44.0).max(1.0),
                depth + 1,
                Some(fill.mix(p.look.raised, 0.17)),
            ));
        }
        return p
            .kit
            .interactive(content)
            .on_press_tx(move |a: &mut App, tx| {
                select_map(&mut a.activity, id.clone());
                tx.blur();
            })
            .on_double_press(move |a: &mut App| {
                select_map(&mut a.activity, double.clone());
                explore(&mut a.activity);
            });
    }
    let total: f64 = nodes.iter().map(|n| n.value).sum();
    let mut sum = 0.0;
    let mut cut = 1;
    for (i, n) in nodes.iter().enumerate().take(nodes.len() - 1) {
        sum += n.value;
        cut = i + 1;
        if sum >= total / 2.0 {
            break;
        }
    }
    let fraction = (sum / total).clamp(0.01, 0.99);
    let horizontal = width >= height;
    let stack = if horizontal { row() } else { col() };
    stack
        .min_w(0.0)
        .min_h(0.0)
        .grow(1.0)
        .gap(3.0)
        .child(
            tiles(
                &nodes[..cut],
                s,
                p,
                if horizontal { width * fraction } else { width },
                if horizontal {
                    height
                } else {
                    height * fraction
                },
                depth,
                parent_color,
            )
            .grow(fraction as f32),
        )
        .child(
            tiles(
                &nodes[cut..],
                s,
                p,
                if horizontal {
                    width * (1.0 - fraction)
                } else {
                    width
                },
                if horizontal {
                    height
                } else {
                    height * (1.0 - fraction)
                },
                depth,
                parent_color,
            )
            .grow((1.0 - fraction) as f32),
        )
}

pub fn view(app: &App, cx: &mut Cx<App>, p: &Paint, layer: &mut Vec<El>) -> El {
    let s = &app.activity;
    let summary = s.sample.error.clone().unwrap_or_else(|| {
        if s.sample.processes.is_empty() {
            "Reading processes…".into()
        } else {
            format!(
                "{} processes · {} · {} readings unavailable",
                s.sample.processes.len(),
                if s.paused { "Paused" } else { "Live · 1 s" },
                s.sample.unavailable
            )
        }
    });
    let filter = TextInput::new(&s.filter)
        .placeholder("Filter processes · /")
        .leading_icon(p.kit.glyph.search)
        .leading_icon_size(13.0)
        .label("Filter processes by name or PID")
        .height(28.0)
        .width(Some(260.0))
        .on_edit(|a: &mut App| &mut a.activity.filter)
        .on_change(|a: &mut App, _| {
            a.activity.scope.clear();
            a.activity.map_selection = None;
            rebuild(&mut a.activity);
        })
        .on_key(|_: &mut App, e, tx| {
            if e.key == Key::Escape || e.key == Key::Enter {
                tx.blur();
                true
            } else {
                false
            }
        })
        .build(&p.kit)
        .key("process-filter");
    let toolbar = row().key("activity-toolbar").w_full().h(44.0).px(16.0).gap(12.0).items_center().border_sides([0.0,0.0,1.0,0.0],p.look.line)
        .child(text(summary).font(p.look.font(12.0,400.0)).color(p.look.muted).truncate().grow(1.0).min_w(0.0).help(format!("{:.1} ms to sample. CPU uses 100% per core. Memory includes compressed memory.",s.sample.milliseconds)))
        .child(filter).child(shell::shell_button(p,"activity-pause",if s.paused { "Resume" } else { "Pause" },s.paused).w(92.0).on_press(toggle_pause));
    let mut style = TableStyle::from_kit(&p.kit);
    style.row_height = 42.0;
    style.header_height = 28.0;
    style.selected = p.look.selection;
    style.text = p.look.text;
    let kit = p.kit.clone();
    let look = p.look;
    let table = Table::new(
        |a: &App| &a.activity.table,
        |a: &mut App| &mut a.activity.table,
        |a: &App| a.activity.rows.len(),
        |a: &App, i| Cow::Borrowed(a.activity.rows[i].id.as_str()),
    )
    .name("Running processes")
    .column(
        TableColumn::new("name", "Process")
            .flex(2.0)
            .min(160.0)
            .sortable(),
    )
    .column(
        TableColumn::new("cpu", "CPU")
            .width(76.0)
            .min(60.0)
            .align(Align::End)
            .sortable(),
    )
    .column(
        TableColumn::new("memory", "Memory")
            .width(92.0)
            .min(70.0)
            .align(Align::End)
            .sortable(),
    )
    .column(
        TableColumn::new("disk", "Disk/s")
            .width(92.0)
            .min(70.0)
            .align(Align::End)
            .sortable(),
    )
    .cell(move |a: &App, i, column| {
        let r = &a.activity.rows[i];
        let selected = a.activity.table.is_selected(&r.id);
        let ink = if selected {
            look.selection_text
        } else {
            look.text
        };
        if column == "name" {
            let disclosure = if r.child {
                "    "
            } else if r.processes.len() > 1 {
                if a.activity.expanded.contains(&r.group) {
                    "⌄ "
                } else {
                    "› "
                }
            } else {
                ""
            };
            Cell::el(cell_context(
                rune::memo(
                    format!("{}-name", r.id),
                    (&r.name, &r.subtitle, disclosure, selected, look.theme),
                    || {
                        col()
                            .min_w(0.0)
                            .gap(1.0)
                            .child(
                                text(format!("{disclosure}{}", r.name))
                                    .font(look.font(12.0, 500.0))
                                    .color(ink)
                                    .truncate(),
                            )
                            .child(
                                text(&r.subtitle)
                                    .font(look.font(10.0, 400.0))
                                    .color(if selected { ink } else { look.muted })
                                    .truncate(),
                            )
                    },
                ),
                &r.id,
            ))
        } else {
            let m = if column == "cpu" {
                Metric::Cpu
            } else if column == "disk" {
                Metric::Disk
            } else {
                Metric::Memory
            };
            let formatted = if m == Metric::Disk && r.value(m) == Some(0.0) {
                String::new()
            } else {
                m.format(r.value(m))
            };
            Cell::el(cell_context(
                rune::memo(
                    format!("{}-{column}-cell", r.id),
                    (&formatted, selected, look.theme),
                    || {
                        RollingText::new(format!("{}-{column}-cell", r.id), &formatted)
                            .value(r.value(m).unwrap_or(0.0))
                            .font(look.font(11.0, 400.0).features("'tnum' 1"))
                            .color(ink)
                            .spring(rune::tok::motion::SNAPPY)
                            .build(&kit)
                    },
                ),
                &r.id,
            ))
        }
    })
    .on_sort(|a: &mut App, _, _, _| rebuild(&mut a.activity))
    .on_select(|a: &mut App, _, _| a.activity.map_focused = false)
    .on_open(|a: &mut App, _, _| explore(&mut a.activity))
    .style(style)
    .build(app, &p.kit)
    .on_key(on_key)
    .grow(1.0)
    .min_h(0.0);
    let list = col()
        .key("process-list")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .border_sides([0.0, 1.0, 0.0, 0.0], p.look.line)
        .child(shell::pane_title(
            p,
            "process-title",
            "Applications & processes",
            None,
            !s.map_focused,
        ))
        .child(table);
    let mut metrics = row()
        .key("map-controls")
        .w(182.0)
        .h(24.0)
        .radius(if p.look.era() == crate::theme::Era::Modern {
            5.0
        } else {
            0.0
        })
        .fill(p.look.raised);
    for m in Metric::ALL {
        let active = s.metric == m;
        let mut segment = el()
            .key(m.column())
            .role("radio")
            .label(m.title())
            .grow(1.0)
            .h(24.0)
            .center()
            .radius(if p.look.era() == crate::theme::Era::Modern {
                4.0
            } else {
                0.0
            });
        if active {
            let mut indicator = el()
                .key("metric-indicator")
                .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .radius(if p.look.era() == crate::theme::Era::Modern {
                    4.0
                } else {
                    0.0
                })
                .fill(p.look.background)
                .border(1.0, p.look.line)
                .inert();
            if matches!(
                p.look.era(),
                crate::theme::Era::WindowsBevel | crate::theme::Era::UnixBevel
            ) {
                indicator = shell::bevel(indicator, p, true).fill(p.look.raised);
            }
            segment = segment.child(indicator);
        }
        metrics = metrics.child(
            p.kit
                .interactive(segment)
                .press_scale(0.98)
                .child(
                    text(m.title())
                        .key("metric-label")
                        .font(p.look.font(11.0, 500.0))
                        .color(p.look.text),
                )
                .on_press(move |a: &mut App| {
                    a.activity.metric = m;
                    rebuild(&mut a.activity);
                }),
        );
    }
    let back_label = if s.scope.is_empty() {
        "All applications".to_string()
    } else {
        format!(
            "Back to {}",
            if s.scope.len() == 1 {
                "all applications"
            } else {
                "parent"
            }
        )
    };
    let back_link = row()
        .key("map-back-row")
        .w_full()
        .h(32.0)
        .px(6.0)
        .items_center()
        .child(
            shell::shell_button(p, "map-back", &back_label, false)
                .disabled(s.scope.is_empty())
                .opacity(if s.scope.is_empty() { 0.55 } else { 1.0 })
                .on_press(|a: &mut App| back(&mut a.activity)),
        );
    let detail = selected(s);
    let mut details = col()
        .key("process-details")
        .h(125.0)
        .px(12.0)
        .py(10.0)
        .gap(5.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], p.look.line);
    if let Some(r) = detail {
        details = details
            .child(
                text(&r.name)
                    .font(p.look.font(13.0, 600.0))
                    .color(p.look.text)
                    .truncate(),
            )
            .child(
                text(format!(
                    "{} memory   {} CPU   {} disk",
                    Metric::Memory.format(r.value(Metric::Memory)),
                    Metric::Cpu.format(r.value(Metric::Cpu)),
                    Metric::Disk.format(r.value(Metric::Disk))
                ))
                .font(p.look.font(12.0, 400.0))
                .color(p.look.text)
                .truncate(),
            );
        let path = if r.processes.len() == 1 {
            let proc = &r.processes[0];
            format!(
                "PID {} · Parent {} · {} threads · {}",
                proc.id.pid,
                proc.parent,
                proc.threads.map_or("—".into(), |v| v.to_string()),
                if proc.executable.is_empty() {
                    "Executable unavailable"
                } else {
                    &proc.executable
                }
            )
        } else {
            format!(
                "{} · Enter to expand; double-click the box to explore.",
                r.subtitle
            )
        };
        details = details.child(
            text(path.clone())
                .font(p.look.font(11.0, 400.0))
                .color(p.look.muted)
                .truncate()
                .help(path),
        );
    } else {
        details = details.child(
            text(if s.rows.is_empty() {
                "No processes match"
            } else {
                "Select a process or a box"
            })
            .font(p.kit.ty.ui)
            .color(p.look.muted),
        );
    }
    details = details.child(
        text("Process breakdown. Libraries inside a process require a runtime profiler.")
            .font(p.look.font(10.0, 400.0))
            .color(p.look.muted)
            .truncate(),
    );
    let map_size = cx.layout_size("treemap").unwrap_or([560.0, 440.0]);
    let map = col()
        .key("resource-map")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .child(
            el().w_full()
                .h(30.0)
                .child(shell::pane_title(
                    p,
                    "map-title",
                    &format!("{} by application", s.metric.title()),
                    None,
                    s.map_focused,
                ))
                .child(metrics.pin(None, Some(3.0), Some(8.0), None)),
        )
        .child(back_link)
        .child(
            text(if s.metric == Metric::Memory {
                "Area = physical footprint · Includes compressed memory"
            } else {
                "Area = measured usage · Double-click to explore"
            })
            .font(p.look.font(11.0, 400.0))
            .color(p.look.muted)
            .px(12.0)
            .pb(8.0),
        )
        .child(
            col()
                .key("treemap")
                .grow(1.0)
                .min_h(0.0)
                .padding(8.0)
                .child(tiles(
                    visible(s),
                    s,
                    p,
                    map_size[0] as f64 - 16.0,
                    map_size[1] as f64 - 16.0,
                    0,
                    None,
                )),
        )
        .child(details);
    activity_dialogs(app, p, layer);
    col()
        .key("activity")
        .min_h(0.0)
        .child(toolbar)
        .child(row().grow(1.0).min_h(0.0).child(list).child(map))
}

fn cell_context(content: El, id: &str) -> El {
    let id = id.to_string();
    content
        .w_full()
        .on_secondary_press(move |a: &mut App, point, tx| {
            a.activity.table.select(id.clone());
            a.activity.map_focused = false;
            a.activity.menu = Some(point);
            tx.blur();
        })
}
fn activity_dialogs(app: &App, p: &Paint, layer: &mut Vec<El>) {
    let selection = selected(&app.activity);
    let actionable = selection.as_ref().is_some_and(|r| {
        r.processes.len() == 1 && r.processes[0].id.pid > 1 && r.processes[0].id.started > 0
    });
    let mut menu = rune_kit::context_menu("process-menu", el(), app.activity.menu)
        .on_close(|a: &mut App| a.activity.menu = None)
        .item(
            MenuItem::new("Inspect…")
                .shortcut("⌘I")
                .on_select(|a: &mut App| {
                    a.activity.menu = None;
                    a.activity.inspect = true;
                }),
        );
    if selection.as_ref().is_some_and(|r| r.processes.len() > 1) {
        menu = menu.item(
            MenuItem::new("Expand / collapse processes").on_select(|a: &mut App| {
                a.activity.menu = None;
                explore(&mut a.activity);
            }),
        );
    }
    let trigger = menu
        .separator()
        .item(
            MenuItem::new("Quit process…")
                .shortcut("⌘⌫")
                .disabled(!actionable)
                .on_select(|a: &mut App| {
                    a.activity.menu = None;
                    a.activity.quit =
                        selected(&a.activity).and_then(|r| r.processes.first().cloned());
                }),
        )
        .build(&p.kit, layer);
    layer.push(trigger);

    if app.activity.inspect {
        let contents=selected(&app.activity).map(|r| r.processes.iter().map(|p|format!("{} · PID {} · Parent {}\nMemory: {}   CPU: {}\nRead: {}   Write: {}\n{}",p.name,p.id.pid,p.parent,Metric::Memory.format(p.memory),Metric::Cpu.format(p.cpu),Metric::Disk.format(p.read),Metric::Disk.format(p.written),p.executable)).collect::<Vec<_>>().join("\n\n")).unwrap_or_else(||"Select a process or group to inspect.".into());
        Dialog::new("process-inspection", "Process inspection")
            .open(true)
            .width(600.0)
            .description("Memory is physical footprint, including compressed memory.")
            .on_close(|a: &mut App| a.activity.inspect = false)
            .child(
                col()
                    .h(240.0)
                    .padding(16.0)
                    .scroll(rune::ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(
                        text(contents)
                            .selectable(true)
                            .font(p.kit.ty.ui)
                            .color(p.look.text)
                            .wrap(),
                    ),
            )
            .build(&p.kit, layer);
    }
    if let Some(proc) = &app.activity.quit {
        let mut actions = row().gap(8.0).padding(16.0);
        actions = actions.child(
            shell::shell_button(p, "quit-cancel", "Cancel", false)
                .on_press(|a: &mut App| a.activity.quit = None),
        );
        for (force, label) in [(false, "Quit"), (true, "Force Quit")] {
            actions = actions.child(
                shell::shell_button(p, if force { "force-quit" } else { "quit" }, label, false)
                    .on_press(move |a: &mut App| {
                        if let Some(proc) = a.activity.quit.take() {
                            a.activity.control.request((proc.id, force), |(id, force)| {
                                sampling::terminate(id, force)
                            });
                        }
                    }),
            );
        }
        Dialog::new("quit-process",format!("Quit {} (PID {})?",proc.name,proc.id.pid)).open(true).fit().description("Quit requests that the process exit. Force Quit stops it immediately and may lose unsaved work.").on_close(|a: &mut App| a.activity.quit=None).child(actions).build(&p.kit,layer);
    }
}

pub fn report(app: &App) -> serde_json::Value {
    let s = &app.activity;
    serde_json::json!({"active":s.active,"paused":s.paused,"filter":s.filter.text,
        "sample_ms":s.sample.milliseconds,"unavailable":s.sample.unavailable,"error":s.sample.error,
        "process_count":s.sample.processes.len(),"processes":s.sample.processes.iter().map(|p| {
            serde_json::json!({"pid":p.id.pid,"started":p.id.started,"name":p.name,
                "memory_bytes":p.memory,"cpu_percent":p.cpu,"read_bytes_per_second":p.read,
                "written_bytes_per_second":p.written,"threads":p.threads})
        }).collect::<Vec<_>>()})
}
