use super::model::{DocumentKind, FolderID, Item, ItemID};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rune::{Latest, Waker};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
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
    read: Latest<Result<Vec<Item>, String>>,
    used: u64,
    again: bool,
}
struct WatchSet {
    _watcher: RecommendedWatcher,
}

pub struct Library {
    entries: HashMap<FolderID, Entry>,
    tick: u64,
    visible: Vec<FolderID>,
    waker: Option<Waker>,
    watches: Latest<Option<WatchSet>>,
    watching: Option<WatchSet>,
    events_tx: mpsc::Sender<PathBuf>,
    events: mpsc::Receiver<PathBuf>,
    dirty: HashMap<FolderID, Instant>,
    sizes: Latest<(FolderID, Vec<(ItemID, Option<u64>)>)>,
    size_cancel: Arc<AtomicBool>,
    size_folder: Option<FolderID>,
}
impl Default for Library {
    fn default() -> Self {
        let (events_tx, events) = mpsc::channel();
        Self {
            entries: HashMap::new(),
            tick: 0,
            visible: Vec::new(),
            waker: None,
            watches: Latest::default(),
            watching: None,
            events_tx,
            events,
            dirty: HashMap::new(),
            sizes: Latest::default(),
            size_cancel: Arc::new(AtomicBool::new(false)),
            size_folder: None,
        }
    }
}
impl Library {
    pub fn setup(&mut self, waker: &Waker) {
        self.waker = Some(waker.clone());
        self.watches.set_waker(waker.clone());
        self.sizes.set_waker(waker.clone());
    }
    pub fn listing(&self, folder: &FolderID) -> Option<&Listing> {
        self.entries.get(folder).map(|e| &e.listing)
    }
    pub fn ensure(&mut self, folder: &FolderID, refresh: bool) {
        self.tick += 1;
        if let Some(entry) = self.entries.get_mut(folder) {
            entry.used = self.tick;
            if refresh {
                if entry.read.is_pending() {
                    entry.again = true;
                } else {
                    entry.read.request(folder.clone(), |f| read_directory(&f.0));
                }
            }
            return;
        }
        let read = Latest::default();
        if let Some(waker) = &self.waker {
            read.set_waker(waker.clone());
        }
        read.request(folder.clone(), |f| read_directory(&f.0));
        self.entries.insert(
            folder.clone(),
            Entry {
                listing: Listing::Loading,
                read,
                used: self.tick,
                again: false,
            },
        );
    }
    pub fn visible(&mut self, folders: Vec<FolderID>, active: Option<FolderID>) {
        if self.visible != folders {
            let newly_visible: Vec<_> = folders
                .iter()
                .filter(|f| !self.visible.contains(f))
                .cloned()
                .collect();
            self.visible = folders.clone();
            for folder in newly_visible {
                self.ensure(&folder, true);
            }
            self.watching = None;
            let tx = self.events_tx.clone();
            let wake = self.waker.clone();
            self.watches.request(folders, move |folders| {
                // FSEvents reports canonical mount paths, while navigation keeps the entered alias.
                let aliases: Vec<_> = folders
                    .iter()
                    .map(|f| {
                        (
                            std::fs::canonicalize(&f.0).unwrap_or_else(|_| f.0.clone()),
                            f.0.clone(),
                        )
                    })
                    .collect();
                let mut watcher = notify::recommended_watcher(
                    move |event: Result<notify::Event, notify::Error>| {
                        if let Ok(event) = event {
                            for path in event.paths {
                                for (canonical, entered) in &aliases {
                                    if let Ok(relative) = path.strip_prefix(canonical) {
                                        let _ = tx.send(entered.join(relative));
                                    }
                                }
                            }
                            if let Some(w) = &wake {
                                w();
                            }
                        }
                    },
                )
                .ok()?;
                for f in folders {
                    let _ = watcher.watch(&f.0, RecursiveMode::NonRecursive);
                }
                Some(WatchSet { _watcher: watcher })
            });
        }
        if self.size_folder != active {
            self.size_cancel.store(true, Ordering::Relaxed);
            self.sizes.cancel();
            self.size_cancel = Arc::new(AtomicBool::new(false));
            self.size_folder = active;
            self.start_sizes();
        }
    }
    fn start_sizes(&mut self) {
        let Some(folder) = self.size_folder.clone() else {
            return;
        };
        let Some(Listing::Ready(items)) = self.listing(&folder) else {
            return;
        };
        let paths: Vec<_> = items
            .iter()
            .filter(|i| {
                matches!(i.kind, DocumentKind::Folder | DocumentKind::Package) && !i.symlink
            })
            .map(|i| i.id.clone())
            .collect();
        let cancel = self.size_cancel.clone();
        self.sizes.request((folder, paths), move |(folder, paths)| {
            let mut totals = Vec::new();
            for path in paths {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let bytes = total_size(&path.0, &cancel).ok();
                totals.push((path, bytes));
            }
            (folder, totals)
        });
    }
    pub fn refresh(&mut self, folder: &FolderID) {
        self.ensure(folder, true);
        if self.size_folder.as_ref() == Some(folder) {
            self.size_cancel.store(true, Ordering::Relaxed);
            self.size_cancel = Arc::new(AtomicBool::new(false));
            self.sizes.cancel();
        }
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(watches) = self.watches.take() {
            self.watching = watches;
        }
        while let Ok(path) = self.events.try_recv() {
            for f in &self.visible {
                if path == f.0 || path.parent() == Some(f.0.as_path()) {
                    self.dirty.insert(f.clone(), Instant::now());
                }
            }
        }
        let refresh: Vec<_> = self
            .dirty
            .iter()
            .filter(|(_, at)| at.elapsed() >= Duration::from_millis(120))
            .map(|(f, _)| f.clone())
            .collect();
        for f in refresh {
            self.dirty.remove(&f);
            self.refresh(&f);
        }
        let mut sizes_needed = false;
        for (folder, entry) in &mut self.entries {
            if let Some(result) = entry.read.take() {
                let listing = match result {
                    Ok(items) => Listing::Ready(Arc::new(items)),
                    Err(error) => Listing::Failed(error),
                };
                if listing != entry.listing {
                    entry.listing = listing;
                    changed |= self.visible.contains(folder);
                }
                sizes_needed |= self.size_folder.as_ref() == Some(folder);
                if entry.again {
                    entry.again = false;
                    entry.read.request(folder.clone(), |f| read_directory(&f.0));
                }
            }
        }
        if sizes_needed {
            self.start_sizes();
        }
        if let Some((folder, totals)) = self.sizes.take() {
            if let Some(Entry {
                listing: Listing::Ready(items),
                ..
            }) = self.entries.get_mut(&folder)
            {
                let totals: HashMap<_, _> = totals.into_iter().collect();
                let rows = Arc::make_mut(items);
                let mut updated = false;
                for row in rows {
                    if let Some(size) = totals.get(&row.id) {
                        if row.size != *size {
                            row.size = *size;
                            updated = true;
                        }
                    }
                }
                changed |= updated && self.visible.contains(&folder);
            }
        }
        while self.entries.len() > 32 {
            let victim = self
                .entries
                .iter()
                .filter(|(f, e)| !self.visible.contains(f) && !e.read.is_pending())
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
fn read_directory(path: &Path) -> Result<Vec<Item>, String> {
    let entries =
        std::fs::read_dir(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let mut rows = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        // A vanished entry is a directory race; missing metadata stays unknown.
        let row = Item::read(&path).unwrap_or_else(|_| Item {
            id: ItemID::new(&path),
            name: super::model::name(&path),
            kind: DocumentKind::Other,
            size: None,
            modified: None,
            created: None,
            symlink: false,
            highlights: Vec::new(),
        });
        rows.push(row);
    }
    rows.sort_by(|a, b| {
        (b.kind == DocumentKind::Folder)
            .cmp(&(a.kind == DocumentKind::Folder))
            .then_with(|| rune_kit::natural_cmp(&a.name, &b.name))
    });
    Ok(rows)
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
            result.push(item);
        }
        if result.len() == 80 {
            break;
        }
    }
    result.sort_by(|a, b| {
        (b.kind == DocumentKind::Folder)
            .cmp(&(a.kind == DocumentKind::Folder))
            .then_with(|| rune_kit::natural_cmp(&a.name, &b.name))
    });
    Ok(result)
}
