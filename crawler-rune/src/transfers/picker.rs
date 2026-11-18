use crate::funnel::model::DocumentKind;
use std::{
    borrow::Cow,
    collections::VecDeque,
    ffi::CString,
    fs,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

struct Watch {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<()>,
}
impl Drop for Watch {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Watch {
    fn start(path: PathBuf, waker: Waker) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
                return;
            };
            let fd = unsafe { libc::open(path.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
            if fd < 0 {
                return;
            }
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            let kq = unsafe { libc::kqueue() };
            if kq < 0 {
                return;
            }
            let kq = unsafe { OwnedFd::from_raw_fd(kq) };
            let change = libc::kevent {
                ident: fd.as_raw_fd() as libc::uintptr_t,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: libc::NOTE_WRITE
                    | libc::NOTE_RENAME
                    | libc::NOTE_DELETE
                    | libc::NOTE_ATTRIB,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            if unsafe {
                libc::kevent(
                    kq.as_raw_fd(),
                    &change,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            } < 0
            {
                return;
            }
            while !stop.load(Ordering::Relaxed) {
                let timeout = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 200_000_000,
                };
                let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
                let n = unsafe {
                    libc::kevent(
                        kq.as_raw_fd(),
                        std::ptr::null(),
                        0,
                        event.as_mut_ptr(),
                        1,
                        &timeout,
                    )
                };
                if n > 0 {
                    if tx.send(()).is_err() {
                        break;
                    }
                    waker();
                }
                if n < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                {
                    break;
                }
            }
        });
        Self { cancel, rx }
    }
}
use crate::{
    app::{App, Paint},
    shell,
};
use rune::{Cx, El, Key, Latest, TextEdit, Tx, Waker, col, row, text};
use rune_kit::{Cell, Dialog, Table, TableColumn, TableState, TextInput};

#[derive(Clone, PartialEq, Eq)]
struct Entry {
    id: String,
    path: PathBuf,
    name: String,
    kind: DocumentKind,
    bytes: u64,
    modified: Option<std::time::SystemTime>,
}
#[derive(Default)]
pub struct Picker {
    pub open: bool,
    pub send_after: bool,
    waker: Option<Waker>,
    watch: Option<Watch>,
    folder: PathBuf,
    path: TextEdit,
    filter: TextEdit,
    rows: Vec<Entry>,
    cache: VecDeque<(PathBuf, Vec<Entry>)>,
    visible: Vec<usize>,
    table: TableState,
    cursor_index: std::cell::Cell<Option<usize>>,
    read: Latest<Result<Vec<Entry>, String>>,
    error: Option<String>,
}
impl super::State {
    pub(crate) fn picker_preview(&self) -> Option<(&Path, Option<(u64, std::time::SystemTime)>)> {
        if !self.picker.open {
            return None;
        }
        let entry = self.picker.selected_file()?;
        Some((&entry.path, entry.modified.map(|time| (entry.bytes, time))))
    }
}

impl Picker {
    fn selected_entry(&self) -> Option<&Entry> {
        let id = self.table.cursor()?;
        if let Some(row) = self.cursor_index.get().and_then(|i| self.rows.get(i))
            && row.id == id
        {
            return Some(row);
        }
        let at = self.rows.iter().position(|entry| entry.id == id)?;
        self.cursor_index.set(Some(at));
        self.rows.get(at)
    }
    fn selected_file(&self) -> Option<&Entry> {
        self.selected_entry()
            .filter(|entry| entry.kind != DocumentKind::Folder)
    }
    pub fn setup(&mut self, waker: &Waker) {
        self.read = Latest::default().debounce(std::time::Duration::from_millis(120));
        self.read.set_waker(waker.clone());
        self.waker = Some(waker.clone());
    }
    pub fn show(&mut self, send_after: bool) {
        self.open = true;
        self.send_after = send_after;
        if self.folder.as_os_str().is_empty() {
            self.folder = crate::app::home().join("Downloads");
        }
        self.navigate(self.folder.clone());
    }
    fn navigate(&mut self, path: PathBuf) {
        self.folder = path;
        self.path = TextEdit::new(self.folder.to_string_lossy());
        self.filter = TextEdit::new("");
        self.watch = self
            .waker
            .as_ref()
            .map(|w| Watch::start(self.folder.clone(), w.clone()));
        self.rows = self
            .cache
            .iter()
            .find(|(path, _)| path == &self.folder)
            .map(|(_, rows)| rows.clone())
            .unwrap_or_default();
        self.filter_rows();
        self.table = TableState::new().multi_select(true);
        self.error = None;
        self.refresh();
    }
    fn refresh(&mut self) {
        let folder = self.folder.clone();
        self.read.request(folder, |folder| {
            let entries = fs::read_dir(folder).map_err(|e| e.to_string())?;
            let mut rows = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| e.to_string())?;
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
                let package = metadata.is_dir() && crate::opening::platform::is_package(&path);
                rows.push(Entry {
                    id: path.to_string_lossy().into_owned(),
                    kind: if package {
                        DocumentKind::Package
                    } else {
                        DocumentKind::for_path(&path, metadata.is_dir())
                    },
                    path,
                    name,
                    bytes: metadata.len(),
                    modified: metadata.modified().ok(),
                });
            }
            rows.sort_by(|a, b| {
                (b.kind == DocumentKind::Folder)
                    .cmp(&(a.kind == DocumentKind::Folder))
                    .then_with(|| rune_kit::natural_cmp(&a.name, &b.name))
            });
            Ok(rows)
        });
    }
    fn filter_rows(&mut self) {
        let query = self.filter.text.to_lowercase();
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.name.to_lowercase().contains(&query).then_some(i))
            .collect();
    }
    fn selected_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| self.table.is_selected(&r.id))
            .count()
    }
    pub fn poll(&mut self) -> bool {
        if self
            .watch
            .as_ref()
            .is_some_and(|w| w.rx.try_iter().count() > 0)
        {
            self.refresh();
        }
        let Some(result) = self.read.take() else {
            return false;
        };
        match result {
            Ok(rows) => {
                if self.rows == rows && self.error.is_none() {
                    return false;
                }
                self.cache.retain(|(folder, _)| folder != &self.folder);
                self.cache.push_back((self.folder.clone(), rows.clone()));
                if self.cache.len() > 32 {
                    self.cache.pop_front();
                }
                self.rows = rows;
                self.error = None;
            }
            Err(e) => {
                if self.rows.is_empty() && self.error.as_ref() == Some(&e) {
                    return false;
                }
                self.rows.clear();
                self.error = Some(e);
            }
        }
        self.filter_rows();
        self.open
    }
    pub fn close(&mut self) {
        self.open = false;
        self.watch = None;
        self.read.cancel();
    }
}
fn finish(app: &mut App, tx: &mut Tx) {
    let picker = &mut app.transfers.picker;
    let files = picker
        .rows
        .iter()
        .filter(|r| picker.table.is_selected(&r.id))
        .map(|r| r.path.clone())
        .collect::<Vec<_>>();
    if files.is_empty() {
        return;
    }
    let send = picker.send_after;
    picker.close();
    super::airdrop(app, files, tx);
    if send {
        super::send(app);
    }
}
pub fn overlay(app: &App, cx: &mut Cx<App>, p: &Paint, layer: &mut Vec<El>) {
    let s = &app.transfers.picker;
    if !s.open {
        return;
    }
    let mut places = row().gap(6.0);
    for (label, folder) in [
        (
            "Parent",
            s.folder.parent().unwrap_or(&s.folder).to_path_buf(),
        ),
        ("Downloads", crate::app::home().join("Downloads")),
        ("Desktop", crate::app::home().join("Desktop")),
        ("Home", crate::app::home()),
    ] {
        places = places.child(
            shell::shell_button(p, &format!("picker-{label}"), label, false)
                .on_press(move |a: &mut App| a.transfers.picker.navigate(folder.clone())),
        );
    }
    let path = TextInput::new(&s.path)
        .label("Folder path")
        .width(None)
        .on_edit(|a: &mut App| &mut a.transfers.picker.path)
        .on_key(|a: &mut App, key, _| {
            if key.key == Key::Enter {
                let value = a.transfers.picker.path.text.clone();
                let path = if value == "~" {
                    crate::app::home()
                } else if let Some(rest) = value.strip_prefix("~/") {
                    crate::app::home().join(rest)
                } else {
                    PathBuf::from(value)
                };
                a.transfers.picker.navigate(path);
                true
            } else {
                false
            }
        })
        .build(&p.kit)
        .key("airdrop-picker-path");
    let filter = TextInput::new(&s.filter)
        .placeholder("Filter this folder…")
        .width(None)
        .on_edit(|a: &mut App| &mut a.transfers.picker.filter)
        .on_change(|a: &mut App, _| a.transfers.picker.filter_rows())
        .build(&p.kit)
        .key("airdrop-picker-filter");
    let look = p.look;
    let list_height =
        (cx.layout("workspace").map(|r| r[3]).unwrap_or(760.0) - 305.0).clamp(120.0, 275.0);
    let table = Table::new(
        |a: &App| &a.transfers.picker.table,
        |a: &mut App| &mut a.transfers.picker.table,
        |a: &App| a.transfers.picker.visible.len(),
        |a: &App, i| {
            Cow::Borrowed(
                a.transfers.picker.rows[a.transfers.picker.visible[i]]
                    .id
                    .as_str(),
            )
        },
    )
    .name("Files to send")
    .column(TableColumn::new("name", "Name").flex(1.0))
    .cell(move |a: &App, i, _| {
        let r = &a.transfers.picker.rows[a.transfers.picker.visible[i]];
        let ink = if a.transfers.picker.table.is_selected(&r.id) {
            look.selection_text
        } else {
            look.text
        };
        Cell::el(
            row()
                .gap(8.0)
                .items_center()
                .child(crate::icons::file(
                    &r.path,
                    r.kind,
                    look,
                    a.transfers.picker.table.is_selected(&r.id),
                    16.0,
                ))
                .child(
                    text(r.name.clone())
                        .font(rune::tok::type_::LABEL)
                        .color(ink)
                        .truncate(),
                ),
        )
    })
    .on_open(|a: &mut App, id, tx| {
        let row = a
            .transfers
            .picker
            .rows
            .iter()
            .find(|r| r.id == id)
            .map(|r| (r.kind == DocumentKind::Folder, r.path.clone()));
        if let Some((directory, path)) = row {
            if directory {
                a.transfers.picker.navigate(path);
            } else {
                finish(a, tx);
            }
        }
    })
    .build(app, &p.kit)
    .grow(1.0)
    .min_h(0.0)
    .min_w(0.0);
    let selected = s.selected_entry();
    let preview = if let Some(entry) = selected.filter(|r| r.kind != DocumentKind::Folder) {
        crate::preview::pane(app, cx, p, &entry.path, "airdrop-picker-preview")
            .w(210.0)
            .h_full()
    } else {
        col()
            .w(210.0)
            .h_full()
            .padding(14.0)
            .gap(12.0)
            .fill(p.look.raised)
            .child(
                text(selected.map(|r| r.name.as_str()).unwrap_or("No selection"))
                    .font(rune::tok::type_::LABEL)
                    .color(p.look.text)
                    .wrap(),
            )
            .child(
                text(if selected.is_some() {
                    "Folder"
                } else {
                    "Select files or folders to send."
                })
                .color(p.look.muted)
                .wrap(),
            )
    };
    let body = col()
        .padding(16.0)
        .gap(10.0)
        .child(places)
        .child(path)
        .child(filter)
        .child(row().h(list_height).gap(12.0).child(table).child(preview))
        .child(
            text(s.error.as_deref().unwrap_or(if s.read.is_pending() {
                "Loading…"
            } else if s.visible.is_empty() {
                "No matching files"
            } else {
                "Command-click or Shift-click to select more."
            }))
            .color(p.look.muted)
            .wrap(),
        )
        .child(
            row()
                .justify_between()
                .items_center()
                .child(text(format!("{} selected", s.selected_count())).color(p.look.muted))
                .child(
                    row()
                        .gap(8.0)
                        .child(
                            shell::shell_button(p, "airdrop-picker-cancel", "Cancel", false)
                                .on_press(|a: &mut App| a.transfers.picker.close()),
                        )
                        .child(
                            shell::shell_button(
                                p,
                                "airdrop-picker-use",
                                "Use selected files",
                                s.selected_count() > 0,
                            )
                            .on_press_tx(finish),
                        ),
                ),
        );
    Dialog::new("airdrop-picker", "Choose files to send")
        .open(true)
        .width(780.0)
        .fit()
        .click_outside(false)
        .on_close(|a: &mut App| a.transfers.picker.close())
        .child(body)
        .build(&p.kit, layer);
}
