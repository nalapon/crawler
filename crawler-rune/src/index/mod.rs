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
    Loading { indexed: usize },
    Ready { indexed: usize },
    Failed(String),
}

pub struct Engine {
    next_id: u64,
    waker: Option<Waker>,
    #[allow(dead_code)]
    workers: Option<usize>,
}

impl Engine {
    pub fn new(workers: Option<usize>) -> Engine {
        Engine { next_id: 0, waker: None, workers }
    }

    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    pub fn search(&mut self, _query: Query) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// The newest response since the last call, if any.
    pub fn take(&mut self) -> Option<Response> {
        None
    }

    /// Records a successful open, for access-history scoring.
    pub fn record_access(&mut self, _path: &std::path::Path) {}

    pub fn idle(&mut self) {}

    pub fn suspend(&mut self) {}

    pub fn status(&self) -> Status {
        Status::Idle
    }

    /// Fields for the `--state-report` JSON.
    pub fn report(&self) -> serde_json::Value {
        serde_json::json!({})
    }
}
