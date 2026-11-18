use super::bulk::Packages;
use super::model::{DocumentKind, FolderID, Item, ItemID};
use crate::foreground_io::{Foreground, external_path};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rune::{Latest, Waker};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Listing {
    Loading,
    Ready(Arc<Vec<Item>>),
    Failed(String),
}
struct Entry {
    listing: Listing,
    packages: Option<Arc<Packages>>,
    read_started: Option<Instant>,
    sizes: Arc<HashMap<ItemID, u64>>,
    size_revision: u64,
    size_order_changed: bool,
    used: u64,
}
#[derive(Default)]
struct ReadSlot {
    folder: Option<FolderID>,
    foreground: bool,
    work: Latest<Snapshot>,
    cancelled: Arc<AtomicBool>,
    failed_to_start: Option<Snapshot>,
    reading: bool,
    again: Option<Refresh>,
}
struct Snapshot {
    listing: Listing,
    packages: Option<Arc<Packages>>,
    started: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Refresh {
    ReusePackages,
    RecheckPackages,
}

struct ReadRequest {
    folder: FolderID,
    previous: Listing,
    packages: Option<Arc<Packages>>,
}

impl ReadSlot {
    fn cancel(&mut self, reserved: &Foreground<Snapshot>) {
        self.failed_to_start = None;
        self.reading = false;
        if self.foreground {
            reserved.cancel();
        } else {
            self.cancelled.store(true, Ordering::Relaxed);
            self.work.cancel();
        }
    }

    fn take(&mut self, reserved: &Foreground<Snapshot>) -> Option<Snapshot> {
        let snapshot = self.failed_to_start.take().or_else(|| {
            if self.foreground {
                reserved.take()
            } else {
                self.work.take()
            }
        })?;
        self.reading = false;
        Some(snapshot)
    }
}

pub struct Library {
    entries: HashMap<FolderID, Entry>,
    tick: u64,
    visible: Vec<FolderID>,
    reads: [ReadSlot; 2],
    foreground: Foreground<Snapshot>,
    waker: Option<Waker>,
    watches: Option<mpsc::Sender<Vec<FolderID>>>,
    events: Arc<Mutex<HashMap<FolderID, Instant>>>,
    dirty: HashMap<FolderID, Instant>,
    sizes: Latest<()>,
    size_send: mpsc::Sender<(u64, FolderID, ItemID, Option<u64>)>,
    size_results: mpsc::Receiver<(u64, FolderID, ItemID, Option<u64>)>,
    size_cancel: Arc<AtomicBool>,
    size_folder: Option<FolderID>,
    size_generation: u64,
}
impl Default for Library {
    fn default() -> Self {
        let (size_send, size_results) = mpsc::channel();
        Self {
            entries: HashMap::new(),
            tick: 0,
            visible: Vec::new(),
            waker: None,
            watches: None,
            reads: Default::default(),
            foreground: Foreground::new("crawler-folder-read"),
            events: Arc::default(),
            dirty: HashMap::new(),
            sizes: Latest::default(),
            size_send,
            size_results,
            size_cancel: Arc::new(AtomicBool::new(false)),
            size_folder: None,
            size_generation: 0,
        }
    }
}
impl Drop for Library {
    fn drop(&mut self) {
        self.size_cancel.store(true, Ordering::Relaxed);
        for slot in &mut self.reads {
            slot.cancel(&self.foreground);
        }
    }
}
impl Library {
    pub fn setup(&mut self, waker: &Waker) {
        self.waker = Some(waker.clone());
        for slot in &self.reads {
            slot.work.set_waker(waker.clone());
        }
        self.sizes.set_waker(waker.clone());
    }
    pub fn listing(&self, folder: &FolderID) -> Option<&Listing> {
        self.entries.get(folder).map(|e| &e.listing)
    }
    pub fn is_reading(&self, folder: &FolderID) -> bool {
        self.reads
            .iter()
            .any(|slot| slot.reading && slot.folder.as_ref() == Some(folder))
    }
    pub fn sizes(&self, folder: &FolderID) -> Option<(&Arc<HashMap<ItemID, u64>>, u64)> {
        self.entries
            .get(folder)
            .map(|e| (&e.sizes, e.size_revision))
    }

    pub fn visible(&mut self, folders: Vec<FolderID>, active: Option<FolderID>, external: bool) {
        let local = active
            .as_ref()
            .filter(|f| !external && !external_path(&f.0));
        let current_local = self
            .reads
            .iter()
            .filter(|s| s.foreground)
            .find_map(|s| s.folder.as_ref());
        if self.visible != folders || current_local != local {
            self.dirty.retain(|folder, _| folders.contains(folder));
            for slot in &mut self.reads {
                if slot
                    .folder
                    .as_ref()
                    .is_some_and(|f| !folders.contains(f) || slot.foreground != (Some(f) == local))
                {
                    slot.cancel(&self.foreground);
                    slot.folder = None;
                    slot.foreground = false;
                    slot.again = None;
                }
            }
            for folder in &folders {
                if self.reads.iter().any(|s| s.folder.as_ref() == Some(folder)) {
                    continue;
                }
                let Some(slot) = self.reads.iter_mut().find(|s| s.folder.is_none()) else {
                    continue;
                };
                self.tick += 1;
                let entry = self.entries.entry(folder.clone()).or_insert_with(|| Entry {
                    listing: Listing::Loading,
                    packages: None,
                    read_started: None,
                    sizes: Arc::default(),
                    size_revision: 0,
                    size_order_changed: false,
                    used: self.tick,
                });
                entry.used = self.tick;
                slot.folder = Some(folder.clone());
                slot.foreground = Some(folder) == local;
                request_listing(
                    slot,
                    &mut self.foreground,
                    folder,
                    entry,
                    Refresh::ReusePackages,
                    &self.waker,
                );
            }
            if self.watches.is_none() && !folders.is_empty() {
                self.watches = Some(watch_folders(self.events.clone(), self.waker.clone()));
            }
            if let Some(watches) = &self.watches {
                let _ = watches.send(folders.clone());
            }
            self.visible = folders;
        }
        if self.size_folder != active {
            self.size_generation += 1;
            self.size_cancel.store(true, Ordering::Relaxed);
            self.sizes.cancel();
            self.size_cancel = Arc::new(AtomicBool::new(false));
            self.size_folder = active;
            if self
                .size_folder
                .as_ref()
                .is_some_and(|f| !self.is_reading(f))
            {
                self.start_sizes();
            }
        }
    }
    fn start_sizes(&mut self) {
        let Some(folder) = self.size_folder.clone() else {
            return;
        };
        let Some(Listing::Ready(items)) = self.listing(&folder) else {
            return;
        };
        let items = items.clone();
        self.size_generation += 1;
        let generation = self.size_generation;
        self.size_cancel.store(true, Ordering::Relaxed);
        self.size_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.size_cancel.clone();
        let send = self.size_send.clone();
        let wake = self.waker.clone();
        self.sizes.request((folder, items), move |(folder, items)| {
            let mut qos = libc::qos_class_t::QOS_CLASS_DEFAULT;
            let mut priority = 0;
            // SAFETY: these pointers address initialized local values. QoS changes apply only
            // to this worker, and its previous class is restored before the shared task returns.
            let restore = unsafe {
                let known =
                    libc::pthread_get_qos_class_np(libc::pthread_self(), &mut qos, &mut priority)
                        == 0;
                if known {
                    libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
                }
                known
            };
            let mut last_wake = None;
            for item in items.iter() {
                if !matches!(item.kind, DocumentKind::Folder | DocumentKind::Package)
                    || item.symlink
                {
                    continue;
                }
                let path = &item.id;
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let bytes = total_size(&path.0, &cancel).ok();
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let _ = send.send((generation, folder.clone(), path.clone(), bytes));
                if let Some(wake) = &wake
                    && last_wake.is_none_or(|at: Instant| at.elapsed() >= Duration::from_millis(16))
                {
                    wake();
                    last_wake = Some(Instant::now());
                }
            }
            if restore {
                // SAFETY: qos and priority came from this same worker before the scan.
                unsafe {
                    libc::pthread_set_qos_class_self_np(qos, priority);
                }
            }
        });
    }
    pub fn refresh(&mut self, folder: &FolderID) {
        if let Some(entry) = self.entries.get_mut(folder) {
            entry.packages = None;
        }
        self.refresh_listing(folder, Refresh::RecheckPackages);
    }
    fn refresh_listing(&mut self, folder: &FolderID, refresh: Refresh) {
        let Some(slot) = self
            .reads
            .iter_mut()
            .find(|s| s.folder.as_ref() == Some(folder))
        else {
            return;
        };
        if slot.reading {
            slot.again = slot.again.max(Some(refresh));
        } else if let Some(entry) = self.entries.get(folder) {
            request_listing(
                slot,
                &mut self.foreground,
                folder,
                entry,
                refresh,
                &self.waker,
            );
        }
        if self.size_folder.as_ref() == Some(folder) {
            self.size_generation += 1;
            self.size_cancel.store(true, Ordering::Relaxed);
            self.size_cancel = Arc::new(AtomicBool::new(false));
            self.sizes.cancel();
        }
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for (folder, at) in self.events.lock().unwrap().drain() {
            if self.visible.contains(&folder) {
                self.dirty.insert(folder, at);
            }
        }
        let mut sizes_needed = false;
        for slot in &mut self.reads {
            if !slot.reading {
                continue;
            }
            let Some(Snapshot {
                listing,
                packages,
                started,
            }) = slot.take(&self.foreground)
            else {
                continue;
            };
            let Some(folder) = &slot.folder else { continue };
            let entry = self
                .entries
                .get_mut(folder)
                .expect("a visible read has a cache entry");
            entry.read_started = Some(started);
            // A queued explicit refresh invalidated package knowledge even if the older read
            // settled first. Keep it invalid if navigation cancels the queued read.
            entry.packages = if slot.again == Some(Refresh::RecheckPackages) {
                None
            } else {
                packages
            };
            let same = match (&listing, &entry.listing) {
                (Listing::Ready(a), Listing::Ready(b)) => Arc::ptr_eq(a, b),
                (Listing::Failed(a), Listing::Failed(b)) => a == b,
                _ => false,
            };
            if !same {
                entry.listing = listing;
                entry.sizes = Arc::default();
                entry.size_revision += 1;
                changed = true;
            }
            if let Some(refresh) = slot.again.take() {
                let folder = folder.clone();
                request_listing(
                    slot,
                    &mut self.foreground,
                    &folder,
                    entry,
                    refresh,
                    &self.waker,
                );
            } else {
                sizes_needed |= self.size_folder.as_ref() == Some(folder);
            }
        }
        let mut refresh = Vec::new();
        self.dirty.retain(|folder, at| {
            if at.elapsed() < Duration::from_millis(120) {
                return true;
            }
            if self
                .entries
                .get(folder)
                .and_then(|e| e.read_started)
                .is_none_or(|start| start < *at)
            {
                refresh.push(folder.clone());
            }
            false
        });
        for folder in refresh {
            sizes_needed &= self.size_folder.as_ref() != Some(&folder);
            self.refresh_listing(&folder, Refresh::ReusePackages);
        }
        if sizes_needed {
            self.start_sizes();
        }
        let scan_completed = self.sizes.take().is_some();
        while let Ok((generation, folder, item, size)) = self.size_results.try_recv() {
            if generation != self.size_generation {
                continue;
            }
            if let Some(entry) = self.entries.get_mut(&folder) {
                let sizes = Arc::make_mut(&mut entry.sizes);
                let updated = match size {
                    Some(bytes) => sizes.insert(item, bytes) != Some(bytes),
                    None => sizes.remove(&item).is_some(),
                };
                entry.size_order_changed |= updated;
                changed |= updated;
            }
        }
        // Only the size ordering needs another sort once all totals for this scan landed.
        if scan_completed
            && let Some(entry) = self
                .size_folder
                .as_ref()
                .and_then(|f| self.entries.get_mut(f))
            && std::mem::take(&mut entry.size_order_changed)
        {
            entry.size_revision += 1;
        }
        while self.entries.len() > 32 {
            let victim = self
                .entries
                .iter()
                .filter(|(f, _)| !self.visible.contains(f))
                .min_by_key(|(_, e)| e.used)
                .map(|(f, _)| f.clone());
            if let Some(f) = victim {
                self.entries.remove(&f);
            } else {
                break;
            }
        }
        changed
    }
    pub fn has_debounce(&self) -> bool {
        !self.dirty.is_empty()
    }
}
fn request_listing(
    slot: &mut ReadSlot,
    foreground: &mut Foreground<Snapshot>,
    folder: &FolderID,
    entry: &Entry,
    refresh: Refresh,
    wake: &Option<Waker>,
) {
    slot.reading = true;
    let input = ReadRequest {
        folder: folder.clone(),
        previous: entry.listing.clone(),
        packages: match refresh {
            Refresh::ReusePackages => entry.packages.clone(),
            Refresh::RecheckPackages => None,
        },
    };
    if slot.foreground {
        if let Err(error) =
            foreground.request(wake.clone(), move |cancel| read_snapshot(input, cancel))
        {
            slot.failed_to_start = Some(Snapshot {
                started: Instant::now(),
                packages: None,
                listing: Listing::Failed(format!("Could not read {}: {error}", folder.0.display())),
            });
            if let Some(wake) = wake {
                wake();
            }
        }
    } else {
        slot.cancelled.store(true, Ordering::Relaxed);
        slot.cancelled = Arc::default();
        let cancel = slot.cancelled.clone();
        slot.work
            .request(input, move |input| read_snapshot(input, &cancel));
    }
}
fn read_snapshot(request: ReadRequest, cancel: &AtomicBool) -> Snapshot {
    let started = Instant::now();
    let (listing, packages) =
        match read_directory(&request.folder.0, request.packages.as_deref(), cancel) {
            Ok((items, packages)) => {
                let listing = if !cancel.load(Ordering::Relaxed)
                    && matches!(&request.previous, Listing::Ready(old) if **old == items)
                {
                    request.previous
                } else {
                    Listing::Ready(Arc::new(items))
                };
                (listing, packages.map(Arc::new))
            }
            Err(error) => (Listing::Failed(error), None),
        };
    Snapshot {
        listing,
        packages,
        started,
    }
}

// notify joins the FSEvents thread on drop. Own both registration and teardown here.
fn watch_folders(
    events: Arc<Mutex<HashMap<FolderID, Instant>>>,
    wake: Option<Waker>,
) -> mpsc::Sender<Vec<FolderID>> {
    let (tx, rx) = mpsc::channel::<Vec<FolderID>>();
    std::thread::spawn(move || {
        let mut watching: HashMap<FolderID, RecommendedWatcher> = HashMap::new();
        while let Ok(mut folders) = rx.recv() {
            for newer in rx.try_iter() {
                folders = newer;
            }
            watching.retain(|f, _| folders.contains(f));
            for folder in folders {
                if watching.contains_key(&folder) {
                    continue;
                }
                let path = std::fs::canonicalize(&folder.0).unwrap_or_else(|_| folder.0.clone());
                let key = folder.clone();
                let callback_events = events.clone();
                let callback_wake = wake.clone();
                let Ok(mut watcher) =
                    notify::recommended_watcher(move |event: Result<notify::Event, _>| {
                        if event.is_ok() {
                            note_change(&callback_events, callback_wake.as_ref(), key.clone());
                        }
                    })
                else {
                    continue;
                };
                if watcher.watch(&path, RecursiveMode::NonRecursive).is_ok() {
                    // A read may have started before the watcher could see changes. Reconcile
                    // once the watch is installed unless a newer read already covers that gap.
                    note_change(&events, wake.as_ref(), folder.clone());
                    watching.insert(folder, watcher);
                }
            }
        }
    });
    tx
}
fn note_change(events: &Mutex<HashMap<FolderID, Instant>>, wake: Option<&Waker>, folder: FolderID) {
    let mut events = events.lock().unwrap();
    let notify = events.is_empty();
    events.insert(folder, Instant::now());
    drop(events);
    if notify && let Some(wake) = wake {
        wake();
    }
}

fn read_directory(
    path: &Path,
    previous: Option<&Packages>,
    cancel: &AtomicBool,
) -> Result<(Vec<Item>, Option<Packages>), String> {
    let check = || {
        if cancel.load(Ordering::Relaxed) {
            Err("Folder read cancelled".to_owned())
        } else {
            Ok(())
        }
    };
    check()?;
    let (mut rows, packages) = match super::bulk::read(path, previous, cancel) {
        Ok((rows, packages)) => (rows, Some(packages)),
        Err(_) => {
            check()?;
            // Some volumes cannot vend bulk attributes. Restart with a fresh directory handle.
            let entries = std::fs::read_dir(path)
                .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
            let mut rows = Vec::new();
            for entry in entries {
                check()?;
                let entry = entry.map_err(|e| e.to_string())?;
                let name = entry.file_name();
                if !name.as_encoded_bytes().starts_with(b".") {
                    push_entry(&mut rows, path, &name);
                }
            }
            (rows, None)
        }
    };
    check()?;
    // Group similar names in-place before the more expensive localized sort. Skip this
    // for lossy names so their stable display-order ties retain enumeration order.
    if rows.len() >= 1024
        && rows
            .iter()
            .all(|row| row.id.0.file_name().is_some_and(|n| n.to_str().is_some()))
    {
        rows.sort_unstable_by(|a, b| {
            (b.kind == DocumentKind::Folder)
                .cmp(&(a.kind == DocumentKind::Folder))
                .then_with(|| {
                    numeric_bytes(
                        a.id.0.file_name().unwrap().as_encoded_bytes(),
                        b.id.0.file_name().unwrap().as_encoded_bytes(),
                    )
                })
        });
    }
    check()?;
    rows.sort_by(|a, b| {
        (b.kind == DocumentKind::Folder)
            .cmp(&(a.kind == DocumentKind::Folder))
            .then_with(|| rune_kit::natural_cmp(&a.name(), &b.name()))
    });
    Ok((rows, packages))
}
// This only seeds the adaptive sort: Rune still decides the displayed order in every locale.
fn numeric_bytes(mut a: &[u8], mut b: &[u8]) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn number(s: &[u8]) -> (&[u8], &[u8]) {
        let (mut digits, rest) = s.split_at(
            s.iter()
                .position(|b| !b.is_ascii_digit())
                .unwrap_or(s.len()),
        );
        while let [b'0', tail @ ..] = digits {
            digits = tail;
        }
        (digits, rest)
    }
    while let ([x, xs @ ..], [y, ys @ ..]) = (a, b) {
        let by = if x.is_ascii_digit() && y.is_ascii_digit() {
            let (left, rest_a) = number(a);
            let (right, rest_b) = number(b);
            (a, b) = (rest_a, rest_b);
            left.len().cmp(&right.len()).then_with(|| left.cmp(right))
        } else {
            (a, b) = (xs, ys);
            x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase())
        };
        if by != Ordering::Equal {
            return by;
        }
    }
    a.len().cmp(&b.len())
}

pub(super) fn push_entry(rows: &mut Vec<Item>, parent: &Path, name: &std::ffi::OsStr) {
    let row = Item::read_entry(parent.join(name)).unwrap_or_else(|_| Item {
        id: ItemID(parent.join(name)),
        kind: DocumentKind::Other(super::model::FileType::Unknown),
        size: None,
        modified: None,
        created: None,
        symlink: false,
        hidden: false,
        highlights: Vec::new(),
        path_highlights: Vec::new(),
    });
    if !row.hidden {
        rows.push(row);
    }
}

fn total_size(path: &Path, cancel: &AtomicBool) -> Result<u64, ()> {
    let mut stack = vec![path.to_owned()];
    let mut total = 0u64;
    while let Some(path) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err(());
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| ())?;
        if metadata.is_symlink() {
            continue;
        }
        if metadata.is_file() {
            total = total.checked_add(metadata.len()).ok_or(())?;
        } else if metadata.is_dir() {
            for entry in std::fs::read_dir(path).map_err(|_| ())? {
                if cancel.load(Ordering::Relaxed) {
                    return Err(());
                }
                stack.push(entry.map_err(|_| ())?.path());
            }
        }
    }
    Ok(total)
}

pub fn bounded_context(path: &Path) -> Result<Vec<Item>, String> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if let Ok(item) = Item::read(&entry.path()) {
            if !item.hidden {
                result.push(item);
            }
        }
        if result.len() == 80 {
            break;
        }
    }
    result.sort_by(|a, b| {
        (b.kind == DocumentKind::Folder)
            .cmp(&(a.kind == DocumentKind::Folder))
            .then_with(|| rune_kit::natural_cmp(&a.name(), &b.name()))
    });
    Ok(result)
}
