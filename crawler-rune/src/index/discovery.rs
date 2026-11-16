use super::snapshot::{BATCH, Snapshot, equal};
use objc2_foundation::{NSArray, NSNumber, NSString, NSURL, NSURLIsPackageKey};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    hash::{BuildHasher, Hasher},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use unicode_normalization::UnicodeNormalization;

pub struct Scan {
    pub batches: Vec<Arc<Snapshot>>,
    pub changed: bool,
    pub warning: Option<String>,
    pub excluded: HashSet<String>,
}
pub enum Progress {
    Batch(Arc<Snapshot>),
    Finished(Result<Scan, String>),
}
struct Directory {
    rows: Vec<(String, bool)>,
    children: Vec<String>,
    delayed: Vec<String>,
    excluded: Vec<String>,
    warning: Option<String>,
}
pub struct Paths {
    batches: Vec<Arc<Snapshot>>,
    hash: std::collections::hash_map::RandomState,
    locations: HashMap<u64, (u32, u16)>,
    collisions: HashMap<u64, Vec<(u32, u16)>>,
}
impl Paths {
    pub fn new(batches: &[Arc<Snapshot>]) -> Self {
        let mut this = Self {
            batches: batches.to_vec(),
            hash: Default::default(),
            locations: HashMap::new(),
            collisions: HashMap::new(),
        };
        for (batch, value) in batches.iter().enumerate() {
            for row in 0..value.len() {
                this.insert(value.path(row), (batch as u32, row as u16));
            }
        }
        this
    }
    fn fingerprint(&self, path: &str) -> u64 {
        let mut hash = self.hash.build_hasher();
        if path.is_ascii() {
            hash.write(path.as_bytes());
        } else {
            for c in path.nfc() {
                hash.write_u32(u32::from(c));
            }
        }
        hash.finish()
    }
    pub fn find(&self, path: &str) -> Option<(usize, usize)> {
        let hash = self.fingerprint(path);
        let first = self.locations.get(&hash)?;
        std::iter::once(first)
            .chain(self.collisions.get(&hash).into_iter().flatten())
            .find_map(|&(b, r)| {
                equal(self.batches[b as usize].path(r as usize), path)
                    .then_some((b as usize, r as usize))
            })
    }
    fn insert(&mut self, path: &str, location: (u32, u16)) {
        let hash = self.fingerprint(path);
        match self.locations.entry(hash) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(location);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                self.collisions.entry(hash).or_default().push(location)
            }
        }
    }
    pub fn append_unique(&mut self, batch: Arc<Snapshot>) -> Option<Arc<Snapshot>> {
        let rows: Vec<_> = (0..batch.len())
            .filter(|&r| self.find(batch.path(r)).is_none())
            .collect();
        if rows.is_empty() {
            return None;
        }
        let batch = if rows.len() == batch.len() {
            batch
        } else {
            Arc::new(Snapshot::build(
                &rows
                    .into_iter()
                    .map(|r| (batch.path(r).to_owned(), batch.directory(r)))
                    .collect::<Vec<_>>(),
            ))
        };
        let id = self.batches.len();
        for row in 0..batch.len() {
            self.insert(batch.path(row), (id as u32, row as u16));
        }
        self.batches.push(Arc::clone(&batch));
        Some(batch)
    }
}
fn unentered(root: &Path, relative: &str) -> bool {
    root == Path::new("/") && matches!(relative, "dev" | "System/Volumes")
}
fn package(path: &Path) -> bool {
    let Some(path) = path.to_str() else {
        return false;
    };
    objc2::rc::autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        // These Foundation keys are immutable process-lifetime constants.
        let key = unsafe { NSURLIsPackageKey };
        let keys = NSArray::from_slice(&[key]);
        url.resourceValuesForKeys_error(&keys)
            .ok()
            .and_then(|v| v.objectForKey(key))
            .and_then(|v| v.downcast_ref::<NSNumber>().map(NSNumber::boolValue))
            .unwrap_or(false)
    })
}
fn kind(path: &Path) -> std::io::Result<Option<(bool, bool)>> {
    use std::os::macos::fs::MetadataExt;
    let meta = fs::symlink_metadata(path)?;
    if meta.st_flags() & libc::UF_HIDDEN != 0 {
        return Ok(None);
    }
    let link = meta.file_type().is_symlink();
    let target = if link { fs::metadata(path).ok() } else { None };
    let directory = target.as_ref().unwrap_or(&meta).is_dir() && !package(path);
    Ok(Some((directory, directory && !link)))
}
fn read(
    root: &Path,
    relative: &str,
    cancel: &AtomicBool,
    directory_cancel: &AtomicBool,
    emit: impl Fn(Vec<(String, bool)>) -> bool,
) -> Result<Directory, String> {
    let mut out = Directory {
        rows: Vec::new(),
        children: Vec::new(),
        delayed: Vec::new(),
        excluded: Vec::new(),
        warning: None,
    };
    if unentered(root, relative) {
        out.excluded.push(relative.into());
        return Ok(out);
    }
    let entries = fs::read_dir(root.join(relative)).map_err(|e| e.to_string())?;
    for entry in entries {
        if cancel.load(Ordering::Relaxed) || directory_cancel.load(Ordering::Relaxed) {
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                out.warning = Some(e.to_string());
                continue;
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        if path.len() >= fff_search::constants::PATH_BUF_SIZE {
            continue;
        }
        match kind(&entry.path()) {
            Ok(Some((directory, descend))) => {
                out.rows.push((path.clone(), directory));
                if out.rows.len() == BATCH && !emit(std::mem::take(&mut out.rows)) {
                    break;
                }
                if descend {
                    if ["node_modules", "vendor", "Pods", "Carthage"].contains(&name.as_str()) {
                        out.delayed.push(path);
                    } else {
                        out.children.push(path);
                    }
                }
            }
            Ok(None) => out.excluded.push(path),
            Err(e) => out.warning = Some(e.to_string()),
        }
    }
    Ok(out)
}
fn closed_ancestor(
    root: &Path,
    path: &str,
    memo: &mut HashMap<String, bool>,
    excluded: &mut HashSet<String>,
) -> bool {
    let mut parent = String::new();
    let parts: Vec<_> = path.split('/').collect();
    for name in parts.iter().take(parts.len().saturating_sub(1)) {
        if !parent.is_empty() {
            parent.push('/');
        }
        parent.push_str(name);
        if let Some(closed) = memo.get(&parent) {
            if *closed {
                return true;
            }
            continue;
        }
        let hidden_name = name.starts_with('.') || unentered(root, &parent);
        let closed = hidden_name
            || match kind(&root.join(&parent)) {
                Ok(Some((_, descend))) => !descend,
                Ok(None) => {
                    excluded.insert(parent.clone());
                    true
                }
                Err(_) => false,
            };
        if hidden_name {
            excluded.insert(parent.clone());
        }
        memo.insert(parent.clone(), closed);
        if closed {
            return true;
        }
    }
    false
}
pub fn run(
    root: PathBuf,
    previous: Vec<Arc<Snapshot>>,
    changes: Option<HashSet<String>>,
    cancel: Arc<AtomicBool>,
    progress: impl Fn(Progress) + Send + Sync,
) {
    let result = scan(&root, &previous, changes, &cancel, &progress);
    if !cancel.load(Ordering::Relaxed) {
        progress(Progress::Finished(result));
    }
}
fn scan(
    root: &Path,
    previous: &[Arc<Snapshot>],
    changes: Option<HashSet<String>>,
    cancel: &Arc<AtomicBool>,
    progress: &(impl Fn(Progress) + Send + Sync),
) -> Result<Scan, String> {
    let old = Paths::new(previous);
    let mut seen: Vec<Vec<bool>> = previous.iter().map(|b| vec![false; b.len()]).collect();
    let mut excluded = HashSet::new();
    let mut pending = VecDeque::new();
    let mut deferred = VecDeque::new();
    let mut prefixes = HashSet::new();
    let mut memo = HashMap::new();
    let mut seeds = Vec::new();
    if let Some(changes) = &changes {
        for path in changes {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if closed_ancestor(root, path, &mut memo, &mut excluded) {
                continue;
            }
            if Path::new(path)
                .ancestors()
                .skip(1)
                .filter_map(Path::to_str)
                .any(|a| changes.contains(a))
            {
                continue;
            }
            prefixes.insert(path.clone());
            match kind(&root.join(path)) {
                Ok(Some((directory, descend))) => {
                    seeds.push((path.clone(), directory));
                    if descend {
                        pending.push_back(path.clone());
                    }
                }
                Ok(None) => {
                    excluded.insert(path.clone());
                }
                Err(_) => {}
            }
        }
        if prefixes.is_empty() {
            return Ok(Scan {
                batches: previous.to_vec(),
                changed: false,
                warning: None,
                excluded,
            });
        }
    } else {
        pending.push_back(String::new());
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .thread_name(|i| format!("crawler-read-{i}"))
        .build()
        .map_err(|e| e.to_string())?;
    enum ReadEvent {
        Rows(u64, Vec<(String, bool)>),
        Done(u64, Result<Directory, String>),
    }
    let (tx, rx) = mpsc::sync_channel(16);
    let outstanding = Arc::new(AtomicUsize::new(0));
    let mut running = HashMap::new();
    let mut serial = 0u64;
    let mut keep = HashSet::new();
    let mut warning = None;
    let mut added = Vec::new();
    let mut buffer = Vec::new();
    let mut replacement = false;
    let mut last_flush = Instant::now();
    let mut rows = seeds;
    loop {
        for (path, directory) in rows.drain(..) {
            if let Some((b, r)) = old.find(&path) {
                if previous[b].directory(r) == directory {
                    seen[b][r] = true;
                    continue;
                }
                replacement = true;
            }
            buffer.push((path, directory));
            if buffer.len() == BATCH {
                let batch = Arc::new(Snapshot::build(&buffer));
                buffer.clear();
                progress(Progress::Batch(Arc::clone(&batch)));
                added.push(batch);
                last_flush = Instant::now();
            }
        }
        if !buffer.is_empty() && last_flush.elapsed() >= Duration::from_millis(40) {
            let batch = Arc::new(Snapshot::build(&buffer));
            buffer.clear();
            progress(Progress::Batch(Arc::clone(&batch)));
            added.push(batch);
            last_flush = Instant::now();
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        while running.len() < 8 {
            let Some(path) = pending.pop_front().or_else(|| deferred.pop_front()) else {
                break;
            };
            // Do not accumulate queued reads behind filesystem calls that never return.
            if outstanding.load(Ordering::Relaxed) >= 32 {
                keep.insert(path);
                warning = Some("Some folders are still waiting for macOS to answer".into());
                continue;
            }
            let outstanding = Arc::clone(&outstanding);
            outstanding.fetch_add(1, Ordering::Relaxed);
            let root = root.to_owned();
            let tx = tx.clone();
            let token = Arc::clone(cancel);
            let id = serial;
            serial += 1;
            let directory_cancel = Arc::new(AtomicBool::new(false));
            running.insert(
                id,
                (path.clone(), Instant::now(), Arc::clone(&directory_cancel)),
            );
            pool.spawn(move || {
                if !token.load(Ordering::Relaxed) && !directory_cancel.load(Ordering::Relaxed) {
                    let result = read(&root, &path, &token, &directory_cancel, |rows| {
                        tx.send(ReadEvent::Rows(id, rows)).is_ok()
                    });
                    let _ = tx.send(ReadEvent::Done(id, result));
                }
                outstanding.fetch_sub(1, Ordering::Relaxed);
            });
        }
        if running.is_empty() {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(ReadEvent::Rows(id, chunk)) => {
                if let Some((_, at, _)) = running.get_mut(&id) {
                    *at = Instant::now();
                    rows = chunk;
                }
            }
            Ok(ReadEvent::Done(id, result)) => {
                if let Some((path, _, _)) = running.remove(&id) {
                    match result {
                        Ok(result) => {
                            rows = result.rows;
                            pending.extend(result.children);
                            deferred.extend(result.delayed);
                            excluded.extend(result.excluded);
                            if result.warning.is_some() {
                                warning = result.warning;
                            }
                        }
                        Err(error) => {
                            if path.is_empty() {
                                return Err(error);
                            }
                            keep.insert(path);
                            warning = Some(format!("Some folders are unavailable: {error}"));
                        }
                    }
                }
            }
            Err(_) => {}
        }
        let timed_out: Vec<_> = running
            .iter()
            .filter(|(_, (_, at, _))| at.elapsed() > Duration::from_secs(2))
            .map(|(&id, _)| id)
            .collect();
        for id in timed_out {
            if let Some((path, _, directory_cancel)) = running.remove(&id) {
                directory_cancel.store(true, Ordering::Release);
                if path.is_empty() {
                    return Err("The search root did not answer within two seconds".into());
                }
                keep.insert(path);
                warning = Some("Some folders did not answer within two seconds".into());
            }
        }
    }
    if !buffer.is_empty() {
        let batch = Arc::new(Snapshot::build(&buffer));
        progress(Progress::Batch(Arc::clone(&batch)));
        added.push(batch);
    }
    let mut batches = Vec::new();
    let mut changed = replacement || !added.is_empty();
    for (b, batch) in previous.iter().enumerate() {
        let retained: Vec<_> = (0..batch.len())
            .filter(|&r| {
                let path = batch.path(r);
                seen[b][r]
                    || ancestors_contain(path, &keep)
                    || (changes.is_some() && !ancestors_contain(path, &prefixes))
            })
            .collect();
        if retained.len() == batch.len() {
            batches.push(Arc::clone(batch));
        } else {
            changed = true;
            if !retained.is_empty() {
                batches.push(Arc::new(Snapshot::build(
                    &retained
                        .into_iter()
                        .map(|r| (batch.path(r).to_owned(), batch.directory(r)))
                        .collect::<Vec<_>>(),
                )));
            }
        }
    }
    batches.extend(added);
    Ok(Scan {
        batches,
        changed,
        warning,
        excluded,
    })
}
pub fn ancestors_contain(path: &str, paths: &HashSet<String>) -> bool {
    paths.contains(path)
        || Path::new(path)
            .ancestors()
            .skip(1)
            .filter_map(Path::to_str)
            .any(|p| paths.contains(p))
}
pub fn alias_scope(scope: &Path) -> bool {
    fs::canonicalize(scope).is_ok_and(|resolved| resolved != scope)
}

pub fn warm(root: &Path, folders: &[PathBuf], cancel: &AtomicBool, emit: impl Fn(Arc<Snapshot>)) {
    for folder in folders {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(relative) = folder.strip_prefix(root) else {
            continue;
        };
        let Some(relative) = relative.to_str() else {
            continue;
        };
        if closed_ancestor(
            root,
            &format!("{relative}/child"),
            &mut HashMap::new(),
            &mut HashSet::new(),
        ) {
            continue;
        }
        let Ok(entries) = fs::read_dir(folder) else {
            continue;
        };
        let mut rows = Vec::new();
        for entry in entries.take(BATCH).flatten() {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if let Ok(Some((directory, _))) = kind(&entry.path()) {
                rows.push((
                    if relative.is_empty() {
                        name
                    } else {
                        format!("{relative}/{name}")
                    },
                    directory,
                ));
            }
        }
        if !rows.is_empty() {
            emit(Arc::new(Snapshot::build(&rows)));
        }
    }
}
