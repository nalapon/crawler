//! The resident Mac-wide fuzzy index (FFF): discovery, the path-snapshot cache, FSEvents
//! catch-up, the query cache and scoring. The funnel talks to it only through [`Engine`].
//!
//! Contract with `crate::funnel` (frozen; additions are fine, changes are not):
//! - `search(query)` starts a query and returns its id. Results arrive through `take()` in
//!   `poll`, possibly several times per query (progressive), the last with `complete: true`.
//! - Searching never blocks: discovery and cache loading start on the first nonempty query.
//! - `idle()` is called when the user leaves Search or clears the query: expensive work stops
//!   at once, batches stay resident for 60 s, then buffers drop and watchers stop.
//! - `suspend()` stops everything (window closed).

use std::path::PathBuf;

use rune::Waker;

/// Which part of the index a query may return.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// This Mac: everything under `/`.
    Mac,
    /// A subtree (a "Narrow the search" choice, a `$` current-folder search or a drive).
    Under(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Query {
    /// Normalized query text (trimmed, as typed otherwise).
    pub text: String,
    pub scope: Scope,
    /// A mounted volume being searched; its batches supplement the Mac cache.
    pub volume: Option<PathBuf>,
    /// `$` current-folder mode: a prefix filter on `scope`, same matcher.
    pub folder_mode: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub path: PathBuf,
    pub is_dir: bool,
    pub score: i64,
    /// Byte ranges of the matched characters in the file name (top 100 only).
    pub name_highlights: Vec<std::ops::Range<usize>>,
    /// Byte ranges of the matched characters in the parent path (top 100 only).
    pub dir_highlights: Vec<std::ops::Range<usize>>,
}

#[derive(Clone, Debug, Default)]
pub struct Response {
    pub id: u64,
    /// At most the top 100 hits, best first.
    pub hits: Vec<Hit>,
    /// Matches per immediate child of the scope (diagnostic; not shown as numbers).
    pub scope_counts: Vec<(PathBuf, usize)>,
    /// Diagnostic total; never shown in the UI.
    pub total: usize,
    /// True once the index is complete for this query.
    pub complete: bool,
}

/// What the status bar says about the index.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Loading {
        indexed: usize,
    },
    Ready {
        indexed: usize,
    },
    Failed(String),
}
impl Status {
    /// Indexed counts are diagnostic. Only a failure change needs a view update.
    fn same_visible_state(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Failed(previous), Self::Failed(next)) => previous == next,
            (Self::Failed(_), _) | (_, Self::Failed(_)) => false,
            _ => true,
        }
    }
}

mod cache;
mod discovery;
mod matching;
mod performance;
mod snapshot;
mod watcher;
mod worker;

use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Instant,
};

struct Shared {
    response: Option<Response>,
    status: Status,
    report: serde_json::Value,
    waker: Option<Waker>,
}
pub struct Engine {
    next_id: u64,
    interval: performance::Interval,
    waker: Option<Waker>,
    commands: mpsc::Sender<worker::Message>,
    shared: Arc<Mutex<Shared>>,
    status: Status,
    diagnostics: bool,
    generation: Arc<AtomicU64>,
    /// What the worker thread needs, held until the first search or recorded open starts it.
    /// An app that never searches (the Space preview window, a launch that only browses)
    /// never spawns the index thread or its matching pool.
    idle: Option<Start>,
}
struct Start {
    workers: usize,
    root: PathBuf,
    support: PathBuf,
    receive: mpsc::Receiver<worker::Message>,
}
impl Engine {
    pub fn new(workers: Option<usize>) -> Engine {
        Self::with_root(workers, PathBuf::from("/"), cache::support_dir())
    }

    /// Isolated roots and caches for headless diagnostics. The app uses `new`, rooted at `/`.
    pub fn with_root(workers: Option<usize>, root: PathBuf, support: PathBuf) -> Engine {
        let workers = workers
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map_or(1, |n| n.get())
                    .min(12)
            })
            .max(1);
        let shared = Arc::new(Mutex::new(Shared {
            response: None,
            status: Status::Idle,
            report: serde_json::json!({"workers": workers}),
            waker: None,
        }));
        let (commands, receive) = mpsc::channel();
        Engine {
            next_id: 0,
            interval: performance::Interval::default(),
            waker: None,
            commands,
            shared,
            status: Status::Idle,
            diagnostics: false,
            generation: Arc::new(AtomicU64::new(0)),
            idle: Some(Start {
                workers,
                root,
                support,
                receive,
            }),
        }
    }
    pub fn enable_diagnostics(&mut self) {
        self.diagnostics = true;
    }
    /// Spawns the worker thread the first time it has something to do.
    fn start(&mut self) {
        let Some(start) = self.idle.take() else {
            return;
        };
        let state = Arc::clone(&self.shared);
        let token = Arc::clone(&self.generation);
        let sender = self.commands.clone();
        let diagnostics = self.diagnostics;
        let spawned = std::thread::Builder::new()
            .name("crawler-index".into())
            .spawn(move || {
                worker::run(
                    start.workers,
                    start.root,
                    start.support,
                    sender,
                    start.receive,
                    state,
                    token,
                    diagnostics,
                )
            });
        if let Err(error) = spawned
            && let Ok(mut state) = self.shared.lock()
        {
            state.status = Status::Failed(error.to_string());
            self.status = state.status.clone();
        }
    }
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker.clone());
        if let Ok(mut state) = self.shared.lock() {
            state.waker = Some(waker);
        }
    }
    pub fn search(&mut self, mut query: Query) -> u64 {
        let started = Instant::now();
        self.next_id += 1;
        self.generation.store(self.next_id, Ordering::Release);
        query.text = query.text.trim().to_owned();
        let signpost = if query.text.is_empty() {
            self.interval.cancel();
            0
        } else {
            self.start();
            self.interval.begin(self.next_id)
        };
        let _ = self.commands.send(worker::Message::Search {
            id: self.next_id,
            signpost,
            query,
            started,
            enqueue_ms: started.elapsed().as_secs_f64() * 1000.0,
        });
        self.next_id
    }
    /// Moves at most 100 owned hits out of a mailbox. It never waits for a worker lock.
    pub fn take(&mut self) -> Option<Response> {
        let response = match self.shared.try_lock() {
            Ok(mut state) => state.response.take()?,
            Err(std::sync::TryLockError::WouldBlock) => {
                // A response may already be waiting behind a diagnostic/status update.
                if let Some(waker) = &self.waker {
                    waker();
                }
                return None;
            }
            Err(std::sync::TryLockError::Poisoned(_)) => return None,
        };
        (response.id == self.next_id && self.generation.load(Ordering::Acquire) == self.next_id)
            .then_some(response)
    }
    pub fn record_access(&mut self, path: &Path) {
        // The worker owns the access history file, so an open must reach it.
        self.start();
        let _ = self.commands.send(worker::Message::Access(path.to_owned()));
    }
    pub fn idle(&mut self) {
        self.interval.cancel();
        self.generation.store(0, Ordering::Release);
        if self.idle.is_some() {
            return;
        }
        let _ = self.commands.send(worker::Message::Idle);
    }
    pub fn suspend(&mut self) {
        self.interval.cancel();
        self.generation.store(0, Ordering::Release);
        if self.idle.is_some() {
            return;
        }
        let _ = self.commands.send(worker::Message::Suspend);
    }
    pub fn poll_status(&mut self) -> bool {
        match self.shared.try_lock() {
            Ok(shared) => {
                let changed = !self.status.same_visible_state(&shared.status);
                self.status = shared.status.clone();
                changed
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                if let Some(waker) = &self.waker {
                    waker();
                }
                false
            }
            Err(std::sync::TryLockError::Poisoned(_)) => false,
        }
    }
    pub fn status(&self) -> Status {
        self.status.clone()
    }
    pub fn report(&self) -> serde_json::Value {
        self.shared
            .lock()
            .map(|s| s.report.clone())
            .unwrap_or_default()
    }

    /// Measures a stage against the query's dispatch time. Call the title stage from paint.
    pub fn mark_stage(&mut self, id: u64, stage: &'static str) {
        if let Some(elapsed) = self.interval.elapsed(id) {
            self.trace_stage(id, stage, elapsed);
        }
    }

    /// The funnel can attach row preparation and actual title-paint timings to the numeric trace.
    /// A window-update notification is not a draw measurement.
    pub fn trace_stage(&mut self, id: u64, stage: &'static str, milliseconds: f64) {
        if stage == "first_title_draw_ms" {
            self.interval.drawn(id);
        }
        if !self.diagnostics {
            return;
        }
        let _ = self.commands.send(worker::Message::Trace {
            id,
            stage,
            milliseconds,
        });
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.generation.store(0, Ordering::Release);
        let _ = self.commands.send(worker::Message::Shutdown);
    }
}
