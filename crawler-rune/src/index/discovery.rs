use super::snapshot::{BATCH, Snapshot, equal};
use foldhash::{HashMap, HashMapExt, HashSet, HashSetExt};
use objc2_foundation::{NSArray, NSNumber, NSString, NSURL, NSURLIsPackageKey};
use std::{
    collections::VecDeque,
    fs,
    hash::{BuildHasher, Hasher},
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
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
    pub retry: HashSet<String>,
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
    keep_contents: bool,
}
pub struct Paths {
    batches: Vec<Arc<Snapshot>>,
    hash: foldhash::fast::RandomState,
    locations: HashMap<u64, (u32, u16)>,
    collisions: HashMap<u64, Vec<(u32, u16)>>,
}
impl Paths {
    pub fn new(batches: &[Arc<Snapshot>]) -> Self {
        Self::selected(batches, None)
    }
    fn selected(batches: &[Arc<Snapshot>], affected: Option<&[Vec<u16>]>) -> Self {
        let mut this = Self {
            batches: batches.to_vec(),
            hash: Default::default(),
            locations: HashMap::new(),
            collisions: HashMap::new(),
        };
        for (b, batch) in batches.iter().enumerate() {
            let mut add = |r: u16| this.insert(batch.path(r as usize), (b as u32, r));
            if let Some(affected) = affected {
                affected[b].iter().copied().for_each(add);
            } else {
                for r in 0..batch.len() as u16 {
                    add(r);
                }
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
    pub fn without_existing(&self, batch: Arc<Snapshot>) -> Option<Arc<Snapshot>> {
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
        Some(batch)
    }
    pub fn append_unique(&mut self, batch: Arc<Snapshot>) -> Option<Arc<Snapshot>> {
        let batch = self.without_existing(batch)?;
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
) -> std::io::Result<Directory> {
    let mut out = Directory {
        rows: Vec::new(),
        children: Vec::new(),
        delayed: Vec::new(),
        excluded: Vec::new(),
        warning: None,
        keep_contents: false,
    };
    if unentered(root, relative) {
        out.excluded.push(relative.into());
        return Ok(out);
    }
    let entries = fs::read_dir(root.join(relative))?;
    for entry in entries {
        if cancel.load(Ordering::Relaxed) || directory_cancel.load(Ordering::Relaxed) {
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                if relative.is_empty() {
                    return Err(e);
                }
                out.keep_contents |= e.kind() == std::io::ErrorKind::TimedOut;
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
            Err(e) => {
                out.keep_contents |= e.kind() == std::io::ErrorKind::TimedOut;
                out.warning = Some(e.to_string());
            }
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
const READERS: usize = 8;
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Filesystem calls can stay blocked after their caller times out. Reserve a
/// worker until the call returns, across consecutive scans and scope checks.
#[derive(Default)]
pub struct ReadPool {
    pool: OnceLock<Result<rayon::ThreadPool, String>>,
    in_flight: Arc<AtomicUsize>,
}
struct ReadPermit(Arc<AtomicUsize>);
impl Drop for ReadPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
impl ReadPool {
    pub fn is_idle(&self) -> bool {
        self.in_flight.load(Ordering::Relaxed) == 0
    }

    pub fn submit(&self, read: impl FnOnce() + Send + 'static) -> Result<bool, String> {
        let pool = self
            .pool
            .get_or_init(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(READERS)
                    .thread_name(|i| format!("crawler-read-{i}"))
                    .build()
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)?;
        if self
            .in_flight
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < READERS).then_some(count + 1)
            })
            .is_err()
        {
            return Ok(false);
        }
        let permit = ReadPermit(Arc::clone(&self.in_flight));
        pool.spawn(move || {
            let _permit = permit;
            read();
        });
        Ok(true)
    }
}

enum ChangeKind {
    Ignored,
    Present { directory: bool, descend: bool },
    Hidden,
    Gone,
    Retry,
}
enum ChangeCheck {
    Path(ChangeKind),
    Done(HashSet<String>),
}

fn check_changes(
    root: &Path,
    changes: HashSet<String>,
    cancel: &Arc<AtomicBool>,
    pool: &ReadPool,
    mut checked: impl FnMut(&str, ChangeKind),
) -> Result<HashSet<String>, String> {
    let paths = Arc::new(
        changes
            .iter()
            .filter(|path| !parent_contains(path, &changes))
            .cloned()
            .collect::<Vec<_>>(),
    );
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let (tx, rx) = mpsc::sync_channel(16);
    let wait = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let root = root.to_owned();
        let job_paths = Arc::clone(&paths);
        let cancel = Arc::clone(cancel);
        let tx = tx.clone();
        if pool.submit(move || {
            let mut memo = HashMap::new();
            let mut excluded = HashSet::new();
            for path in job_paths.iter() {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let kind = if closed_ancestor(&root, path, &mut memo, &mut excluded) {
                    ChangeKind::Ignored
                } else {
                    match kind(&root.join(path)) {
                        Ok(Some((directory, descend))) => {
                            ChangeKind::Present { directory, descend }
                        }
                        Ok(None) => ChangeKind::Hidden,
                        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                            ChangeKind::Retry
                        }
                        Err(_) => ChangeKind::Gone,
                    }
                };
                if tx.send(ChangeCheck::Path(kind)).is_err() {
                    return;
                }
            }
            let _ = tx.send(ChangeCheck::Done(excluded));
        })? {
            break;
        }
        if wait.elapsed() >= READ_TIMEOUT {
            for path in paths.iter() {
                checked(path, ChangeKind::Retry);
            }
            return Ok(HashSet::new());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(tx);
    let mut at = 0;
    let mut last = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(ChangeCheck::Path(kind)) => {
                checked(&paths[at], kind);
                at += 1;
                last = Instant::now();
            }
            Ok(ChangeCheck::Done(excluded)) => return Ok(excluded),
            Err(mpsc::RecvTimeoutError::Timeout) if last.elapsed() < READ_TIMEOUT => continue,
            Err(_) => {
                for path in &paths[at..] {
                    checked(path, ChangeKind::Retry);
                }
                return Ok(HashSet::new());
            }
        }
    }
}

pub fn run(
    root: PathBuf,
    prefix: String,
    previous: Vec<Arc<Snapshot>>,
    changes: Option<HashSet<String>>,
    cancel: Arc<AtomicBool>,
    pool: Arc<ReadPool>,
    progress: impl Fn(Progress) + Send + Sync,
) {
    let result = scan(
        &root, &prefix, &previous, changes, &cancel, &pool, &progress,
    );
    if !cancel.load(Ordering::Relaxed) {
        progress(Progress::Finished(result));
    }
}
fn scan(
    root: &Path,
    prefix: &str,
    previous: &[Arc<Snapshot>],
    changes: Option<HashSet<String>>,
    cancel: &Arc<AtomicBool>,
    pool: &ReadPool,
    progress: &(impl Fn(Progress) + Send + Sync),
) -> Result<Scan, String> {
    // Reads and watcher paths are relative to this volume; snapshots always use Mac paths.
    let indexed_path = |path: &str| {
        if prefix.is_empty() {
            path.to_owned()
        } else if path.is_empty() {
            prefix.to_owned()
        } else {
            format!("{prefix}/{path}")
        }
    };
    let mut retry = HashSet::new();
    let mut excluded = HashSet::new();
    let mut pending = VecDeque::new();
    let mut deferred = VecDeque::new();
    let mut prefixes = HashSet::new();
    let mut seeds = Vec::new();
    let incremental = changes.is_some();
    if let Some(changes) = changes {
        let ancestor_exclusions = check_changes(root, changes, cancel, pool, |path, kind| {
            match kind {
                ChangeKind::Ignored => return,
                ChangeKind::Retry => {
                    retry.insert(path.to_owned());
                    return;
                }
                ChangeKind::Present { directory, descend } => {
                    seeds.push((path.to_owned(), directory));
                    if descend {
                        pending.push_back(path.to_owned());
                    }
                }
                ChangeKind::Hidden => {
                    excluded.insert(path.to_owned());
                }
                ChangeKind::Gone => {}
            }
            prefixes.insert(indexed_path(path));
        })?;
        excluded.extend(ancestor_exclusions);
        if prefixes.is_empty() {
            return Ok(Scan {
                batches: previous.to_vec(),
                changed: false,
                warning: (!retry.is_empty())
                    .then(|| "Some changed paths are still waiting for macOS".into()),
                excluded,
                retry,
            });
        }
    } else {
        pending.push_back(String::new());
    }
    let affected = if incremental {
        let mut blocks = Vec::with_capacity(previous.len());
        for batch in previous {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            blocks.push(batch.affected_rows(&prefixes));
        }
        Some(blocks)
    } else {
        None
    };
    let old = Paths::selected(previous, affected.as_deref());
    let mut seen: Vec<_> = previous
        .iter()
        .enumerate()
        .map(|(b, block)| match &affected {
            None => vec![false; block.len()],
            Some(rows) if rows[b].is_empty() => Vec::new(),
            Some(rows) => {
                let mut seen = vec![true; block.len()];
                for &r in &rows[b] {
                    seen[r as usize] = false;
                }
                seen
            }
        })
        .collect();
    enum ReadEvent {
        Rows(u64, Vec<(String, bool)>),
        Done(u64, std::io::Result<Directory>),
    }
    let (tx, rx) = mpsc::sync_channel(16);
    let mut running = HashMap::new();
    let mut waiting_for_worker = None;
    let mut serial = 0u64;
    let mut keep: HashSet<_> = retry.iter().map(|p| indexed_path(p)).collect();
    let mut warning =
        (!retry.is_empty()).then(|| "Some changed paths are still waiting for macOS".into());
    let mut added = Vec::new();
    let mut buffer = Vec::new();
    let mut replacements = Vec::new();
    let mut last_flush = Instant::now();
    let mut rows = seeds;
    loop {
        for (path, directory) in rows.drain(..) {
            let path = if prefix.is_empty() {
                path
            } else {
                indexed_path(&path)
            };
            if let Some((b, r)) = old.find(&path) {
                if previous[b].directory(r) == directory {
                    seen[b][r] = true;
                    continue;
                }
                // Replace the old kind atomically at completion. Only genuinely new paths
                // can be appended to the resident index before reconciliation finishes.
                replacements.push((path, directory));
                continue;
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
        while running.len() < READERS {
            let Some(path) = pending.pop_front().or_else(|| deferred.pop_front()) else {
                break;
            };
            let root = root.to_owned();
            let tx = tx.clone();
            let token = Arc::clone(cancel);
            let id = serial;
            let directory_cancel = Arc::new(AtomicBool::new(false));
            let read_cancel = Arc::clone(&directory_cancel);
            let read_path = path.clone();
            if pool.submit(move || {
                if !token.load(Ordering::Relaxed) && !read_cancel.load(Ordering::Relaxed) {
                    let result = read(&root, &read_path, &token, &read_cancel, |rows| {
                        tx.send(ReadEvent::Rows(id, rows)).is_ok()
                    });
                    let _ = tx.send(ReadEvent::Done(id, result));
                }
            })? {
                waiting_for_worker = None;
                serial += 1;
                running.insert(id, (path, Instant::now(), directory_cancel));
            } else {
                let waiting = waiting_for_worker.get_or_insert_with(Instant::now);
                if !running.is_empty() || waiting.elapsed() < READ_TIMEOUT {
                    pending.push_front(path);
                    break;
                }
                if path.is_empty() {
                    return Err(
                        "The search root is waiting for macOS to answer earlier reads".into(),
                    );
                }
                keep.insert(indexed_path(&path));
                retry.insert(path);
                warning = Some("Some folders are still waiting for macOS to answer".into());
            }
        }
        if running.is_empty() && pending.is_empty() && deferred.is_empty() {
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
                            if result.keep_contents {
                                keep.insert(indexed_path(&path));
                                retry.insert(path);
                            }
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
                                return Err(error.to_string());
                            }
                            keep.insert(indexed_path(&path));
                            if error.kind() == std::io::ErrorKind::TimedOut {
                                retry.insert(path);
                            }
                            warning = Some(format!("Some folders are unavailable: {error}"));
                        }
                    }
                }
            }
            Err(_) => {}
        }
        let timed_out: Vec<_> = running
            .iter()
            .filter(|(_, (_, at, _))| at.elapsed() > READ_TIMEOUT)
            .map(|(&id, _)| id)
            .collect();
        for id in timed_out {
            if let Some((path, _, directory_cancel)) = running.remove(&id) {
                directory_cancel.store(true, Ordering::Release);
                if path.is_empty() {
                    return Err("The search root did not answer within two seconds".into());
                }
                keep.insert(indexed_path(&path));
                retry.insert(path);
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
    added.extend(
        replacements
            .chunks(BATCH)
            .map(|rows| Arc::new(Snapshot::build(rows))),
    );
    let mut changed = !added.is_empty();
    for (b, batch) in previous.iter().enumerate() {
        if seen[b].is_empty() {
            batches.push(Arc::clone(batch));
            continue;
        }
        let retained: Vec<_> = (0..batch.len())
            .filter(|&r| {
                let path = batch.path(r);
                seen[b][r] || ancestors_contain(path, &keep)
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
        retry,
    })
}
pub fn ancestors_contain(path: &str, paths: &HashSet<String>) -> bool {
    !paths.is_empty() && (paths.contains(path) || parent_contains(path, paths))
}
pub fn parent_contains(path: &str, paths: &HashSet<String>) -> bool {
    if paths.is_empty() {
        return false;
    }
    // A slash is always a UTF-8 boundary. No Path component parsing or allocations.
    for (at, &byte) in path.as_bytes().iter().enumerate().rev() {
        if byte == b'/' && paths.contains(&path[..at]) {
            return true;
        }
    }
    paths.contains("")
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
