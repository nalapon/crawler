use crate::error::Error;
use crate::index::constraints::{GlobPattern, compile_one, glob_matches_into};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use tracing::{debug, error};

/// Watcher subscription/watch id
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WatchId(pub u64);

pub(crate) type WatchCallback = Box<dyn Fn(WatchId, &[WatchEvent]) + Send + Sync>;

/// The kind of filesystem change.
///
/// Event kinds are normalized on a best-effort basis. Editors and operating
/// systems may represent the same operation with different native events.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEventKind {
    Created = 0,
    Modified = 1,
    Removed = 2,
    /// Individual events were lost; rescan the reported path.
    Rescan = 3,
    /// The file moved. `path` is the destination, `from` the source.
    Renamed = 4,
}

impl WatchEventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            WatchEventKind::Created => "created",
            WatchEventKind::Modified => "modified",
            WatchEventKind::Removed => "removed",
            WatchEventKind::Rescan => "rescan",
            WatchEventKind::Renamed => "renamed",
        }
    }
}

/// A single change notification delivered to subscribers.
#[derive(Debug, Clone)]
pub struct WatchEvent {
    /// Absolute affected path (the indexed base path for `Rescan`).
    pub path: PathBuf,
    pub kind: WatchEventKind,
    /// Source path, set only for [`WatchEventKind::Renamed`].
    pub from: Option<PathBuf>,
}

/// Per-subscription options.
#[derive(Debug, Clone, Default)]
pub struct WatchOptions {
    /// Additional glob or path-prefix exclusions.
    pub ignore: Vec<String>,
}

type WatchMask = u128;
const MAX_BATCH_EVENTS: usize = WatchMask::BITS as usize;

pub(crate) struct RawWatchEvent {
    pub(crate) path: PathBuf,
    pub(crate) kind: WatchEventKind,
    pub(crate) is_ignored: bool,
    pub(crate) from: Option<PathBuf>,
}

enum WatchMatcher {
    Glob(GlobPattern),
    Exact(PathBuf),
    Dir(PathBuf),
}

impl WatchMatcher {
    fn new(pattern: &str, base: &Path) -> Result<Self, Error> {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return Ok(WatchMatcher::Dir(PathBuf::new()));
        }

        let Some(relative) = relative_pattern(pattern, base) else {
            return Err(Error::InvalidGlobPattern {
                pattern: pattern.to_string(),
                reason: "watch patterns must be inside the indexed base path".into(),
            });
        };

        if fff_query_parser::glob_detect::has_wildcards(pattern) {
            let glob = relative.to_string_lossy().replace('\\', "/");
            return compile_one(&glob).map(WatchMatcher::Glob).ok_or_else(|| {
                Error::InvalidGlobPattern {
                    pattern: pattern.to_string(),
                    reason: "failed to compile glob".into(),
                }
            });
        }

        if base.join(&relative).is_dir() {
            return Ok(WatchMatcher::Dir(relative));
        }

        Ok(WatchMatcher::Exact(relative))
    }
}

#[derive(Default)]
struct SubIgnore {
    globs: Vec<GlobPattern>,
    prefixes: Vec<PathBuf>,
}

impl SubIgnore {
    fn prefix_matches(&self, path: &Path) -> bool {
        self.prefixes.iter().any(|prefix| path.starts_with(prefix))
    }
}

fn relative_pattern(pattern: &str, base: &Path) -> Option<PathBuf> {
    let expanded = crate::path_utils::expand_tilde(pattern);
    let relative = if expanded.is_absolute() || expanded.has_root() {
        match expanded.strip_prefix(base) {
            Ok(rel) => rel,
            // Windows: the caller may pass an 8.3 short-name or differently
            // cased path; canonicalize and retry before rejecting.
            Err(_) => {
                let canonical = crate::path_utils::canonicalize(&expanded).ok()?;
                return relative_from_canonical(&canonical, base);
            }
        }
    } else {
        &expanded
    };

    reject_parent_components(relative)
}

fn relative_from_canonical(canonical: &Path, base: &Path) -> Option<PathBuf> {
    let relative = canonical.strip_prefix(base).ok()?;
    reject_parent_components(relative)
}

fn reject_parent_components(path: &Path) -> Option<PathBuf> {
    if path
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        return None;
    }

    Some(path.components().collect())
}

fn resolve_sub_ignore(patterns: &[String], base: &Path) -> Result<SubIgnore, Error> {
    let mut ignore = SubIgnore::default();

    for pattern in patterns {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            continue;
        }
        let Some(relative) = relative_pattern(pattern, base) else {
            return Err(Error::InvalidGlobPattern {
                pattern: pattern.to_string(),
                reason: "ignore patterns must be inside the indexed base path".into(),
            });
        };

        if fff_query_parser::glob_detect::has_wildcards(pattern) {
            match compile_one(&relative.to_string_lossy().replace('\\', "/")) {
                Some(compiled) => ignore.globs.push(compiled),
                None => {
                    return Err(Error::InvalidGlobPattern {
                        pattern: pattern.to_string(),
                        reason: "failed to compile ignore glob".into(),
                    });
                }
            }
        } else {
            ignore.prefixes.push(relative);
        }
    }

    Ok(ignore)
}

struct WatchSub {
    id: WatchId,
    matcher: WatchMatcher,
    ignore: SubIgnore,
    callback: WatchCallback,
    active: AtomicBool,
    epoch: AtomicU64,
}

impl WatchSub {
    fn filter_mask(&self, paths: &[&str], scratch: &mut Vec<usize>) -> WatchMask {
        let mut mask = 0;

        match &self.matcher {
            WatchMatcher::Glob(g) => {
                scratch.clear();
                glob_matches_into(g, paths, scratch);
                for &index in scratch.iter() {
                    mask |= 1 << index;
                }
            }
            WatchMatcher::Dir(d) => {
                for (index, path) in paths.iter().enumerate() {
                    if Path::new(path).starts_with(d) {
                        mask |= 1 << index;
                    }
                }
            }
            WatchMatcher::Exact(p) => {
                for (index, path) in paths.iter().enumerate() {
                    if Path::new(path) == p {
                        mask |= 1 << index;
                    }
                }
            }
        }

        // Subtract per-subscription ignores from the match mask.
        for g in &self.ignore.globs {
            scratch.clear();
            glob_matches_into(g, paths, scratch);
            for &index in scratch.iter() {
                mask &= !(1 << index);
            }
        }
        if !self.ignore.prefixes.is_empty() {
            for (index, path) in paths.iter().enumerate() {
                if self.ignore.prefix_matches(Path::new(path)) {
                    mask &= !(1 << index);
                }
            }
        }

        mask
    }
}

struct CallbackDelivery {
    sub: Arc<WatchSub>,
    events: Vec<WatchEvent>,
    epoch: u64,
}

enum CallbackMessage {
    Deliver(Vec<CallbackDelivery>),
    // used to drain all the callbacks and close the sender right after
    Drain(mpsc::Sender<()>),
    Stop,
}

#[derive(Default)]
struct CallbackDispatcherState {
    sender: Option<mpsc::Sender<CallbackMessage>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Default)]
struct CallbackDispatcher {
    state: Mutex<CallbackDispatcherState>,
}

impl CallbackDispatcher {
    /// We have to use a separate thread becuause the callback is the actual C function pointer which
    /// can block on the user side, we can not allow our watcher logic to get into deadlocked state
    fn start(&self) -> Result<(), Error> {
        let mut state = self.state.lock();
        if state.sender.is_some() {
            return Ok(());
        }

        let (sender, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("fff-watch-callback".into())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    match message {
                        CallbackMessage::Deliver(deliveries) => {
                            for delivery in deliveries {
                                if !delivery.sub.active.load(Ordering::Acquire)
                                    || delivery.sub.epoch.load(Ordering::Acquire) != delivery.epoch
                                {
                                    continue;
                                }

                                let id = delivery.sub.id;
                                let result =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        (delivery.sub.callback)(id, &delivery.events)
                                    }));
                                if result.is_err() {
                                    error!(sub = id.0, "watch callback panicked");
                                }
                            }
                        }
                        CallbackMessage::Drain(done) => {
                            let _ = done.send(());
                        }
                        CallbackMessage::Stop => break,
                    }
                }
            })
            .map_err(Error::WatchDispatcherStart)?;

        state.sender = Some(sender);
        state.thread = Some(thread);
        Ok(())
    }

    fn deliver(&self, deliveries: Vec<CallbackDelivery>) {
        if deliveries.is_empty() {
            return;
        }
        let Some(sender) = self.state.lock().sender.clone() else {
            error!("watch callback dispatcher is not running");
            return;
        };
        if sender.send(CallbackMessage::Deliver(deliveries)).is_err() {
            error!("watch callback dispatcher stopped unexpectedly");
        }
    }

    fn drain(&self) {
        let (sender, is_dispatch_thread) = {
            let state = self.state.lock();
            let Some(sender) = state.sender.as_ref() else {
                return;
            };
            let is_dispatch_thread = state
                .thread
                .as_ref()
                .is_some_and(|thread| thread.thread().id() == std::thread::current().id());
            (sender.clone(), is_dispatch_thread)
        };

        if is_dispatch_thread {
            return;
        }

        let (done_tx, done_rx) = mpsc::channel();
        if sender.send(CallbackMessage::Drain(done_tx)).is_ok() {
            let _ = done_rx.recv();
        }
    }
}

impl Drop for CallbackDispatcher {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        if let Some(sender) = state.sender.take() {
            let _ = sender.send(CallbackMessage::Stop);
        }

        if let Some(thread) = state.thread.take()
            && thread.thread().id() != std::thread::current().id()
        {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
struct WatchRegistryState {
    subs: Vec<Arc<WatchSub>>,
    base_path: Option<PathBuf>,
    epoch: u64,
}

// External subscribers for one SharedFilePicker.
#[derive(Default)]
pub(crate) struct WatchRegistry {
    state: Mutex<WatchRegistryState>,
    dispatcher: CallbackDispatcher,
}

// Process-wide ids let FFI clients route all instances through one map.
static NEXT_WATCH_ID: AtomicU64 = AtomicU64::new(1);

impl WatchRegistry {
    #[inline]
    pub(crate) fn is_active(&self) -> bool {
        !self.state.lock().subs.is_empty()
    }

    pub(crate) fn subscribe(
        &self,
        base_path: &Path,
        pattern: &str,
        options: WatchOptions,
        callback: WatchCallback,
    ) -> Result<WatchId, Error> {
        let matcher = WatchMatcher::new(pattern, base_path)?;
        let ignore = resolve_sub_ignore(&options.ignore, base_path)?;

        let mut state = self.state.lock();
        if state.base_path.as_deref() != Some(base_path) {
            return Err(Error::WatchBaseChanged);
        }
        self.dispatcher.start()?;

        let id = WatchId(NEXT_WATCH_ID.fetch_add(1, Ordering::Relaxed));
        let sub = Arc::new(WatchSub {
            id,
            matcher,
            ignore,
            callback,
            active: AtomicBool::new(true),
            epoch: AtomicU64::new(state.epoch),
        });

        state.subs.push(sub);
        Ok(id)
    }

    pub(crate) fn unsubscribe(&self, id: WatchId) -> bool {
        let mut state = self.state.lock();
        let Some(idx) = state.subs.iter().position(|s| s.id == id) else {
            return false;
        };
        let sub = state.subs.swap_remove(idx);
        sub.active.store(false, Ordering::Release);
        drop(state);
        drop(sub);
        true
    }

    pub(crate) fn contains(&self, id: WatchId) -> bool {
        self.state.lock().subs.iter().any(|sub| sub.id == id)
    }

    pub(crate) fn shutdown(&self) {
        let mut state = self.state.lock();
        let subs = std::mem::take(&mut state.subs);
        for sub in &subs {
            sub.active.store(false, Ordering::Release);
        }
        drop(state);
    }

    pub(crate) fn shutdown_and_wait(&self) {
        self.shutdown();
        self.dispatcher.drain();
    }

    pub(crate) fn rebase(&self, base_path: &Path) {
        let mut state = self.state.lock();
        if state.base_path.as_deref() == Some(base_path) {
            return;
        }

        state.base_path = Some(base_path.to_path_buf());
        state.epoch = state.epoch.wrapping_add(1);
        for sub in &state.subs {
            sub.epoch.store(state.epoch, Ordering::Release);
        }
        drop(state);
        self.dispatcher.drain();
    }

    pub(crate) fn dispatch(&self, base_path: &Path, events: Vec<RawWatchEvent>) {
        if events.is_empty() {
            return;
        }

        let state = self.state.lock();
        if state.subs.is_empty() || state.base_path.as_deref() != Some(base_path) {
            return;
        }

        for batch in events.chunks(MAX_BATCH_EVENTS) {
            let mut paths = Vec::with_capacity(batch.len());
            let mut visible_mask = 0;
            let mut rescan_mask = 0;
            let has_renames = batch.iter().any(|event| event.from.is_some());
            // Parallel array of rename sources, so a subscription matching only
            // the pre-rename path still hears about the move.
            let mut from_paths = Vec::with_capacity(if has_renames { batch.len() } else { 0 });

            for (index, event) in batch.iter().enumerate() {
                let relative = event
                    .path
                    .strip_prefix(base_path)
                    .expect("watch event path must be inside the indexed base path");
                paths.push(relative.to_string_lossy().replace('\\', "/"));

                if has_renames {
                    let source = event
                        .from
                        .as_deref()
                        .and_then(|from| from.strip_prefix(base_path).ok())
                        .map(|rel| rel.to_string_lossy().replace('\\', "/"));
                    from_paths.push(source.unwrap_or_else(|| paths[index].clone()));
                }

                let bit = 1 << index;
                if event.kind == WatchEventKind::Rescan {
                    rescan_mask |= bit;
                } else if !event.is_ignored {
                    visible_mask |= bit;
                }
            }

            let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
            let from_refs: Vec<&str> = from_paths.iter().map(String::as_str).collect();
            let mut scratch = Vec::new();
            let mut deliveries = Vec::with_capacity(state.subs.len());
            for sub in &state.subs {
                let mut matched = sub.filter_mask(&path_refs, &mut scratch);
                if has_renames {
                    matched |= sub.filter_mask(&from_refs, &mut scratch);
                }
                let mut delivery_mask = (matched & visible_mask) | rescan_mask;
                if delivery_mask == 0 {
                    continue;
                }

                let mut filtered = Vec::with_capacity(delivery_mask.count_ones() as usize);
                while delivery_mask != 0 {
                    let index = delivery_mask.trailing_zeros() as usize;
                    let event = &batch[index];
                    filtered.push(WatchEvent {
                        path: event.path.clone(),
                        kind: event.kind,
                        from: event.from.clone(),
                    });
                    delivery_mask &= delivery_mask - 1;
                }

                debug!(
                    sub = sub.id.0,
                    count = filtered.len(),
                    "queueing watch events"
                );
                deliveries.push(CallbackDelivery {
                    sub: Arc::clone(sub),
                    events: filtered,
                    epoch: state.epoch,
                });
            }
            self.dispatcher.deliver(deliveries);
        }
    }

    // Signal that individual events were lost.
    pub(crate) fn dispatch_rescan(&self, base_path: &Path) {
        self.dispatch(
            base_path,
            vec![RawWatchEvent {
                path: base_path.to_path_buf(),
                kind: WatchEventKind::Rescan,
                is_ignored: false,
                from: None,
            }],
        );
    }
}

impl Drop for WatchRegistry {
    fn drop(&mut self) {
        self.shutdown();
    }
}

