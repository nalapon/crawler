use super::{
    Query, Response, Scope, Shared, Status,
    cache::{self, Event, History, Reader, Visit},
    discovery::{self, Paths, Progress},
    matching::{self, Matches},
    performance,
    snapshot::{self, Snapshot},
    watcher::{self, Events, Watcher},
};
use foldhash::{HashSet, HashSetExt};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

pub(super) enum Message {
    Search {
        id: u64,
        signpost: u64,
        query: Query,
        started: Instant,
        enqueue_ms: f64,
    },
    Access(PathBuf),
    Idle,
    Suspend,
    Shutdown,
    Trace {
        id: u64,
        stage: &'static str,
        milliseconds: f64,
    },
    WarmBatch {
        job: u64,
        batch: Arc<Snapshot>,
    },
    CacheBatch {
        job: u64,
        position: usize,
        batch: Arc<Snapshot>,
    },
    CacheDone {
        job: u64,
        event: Option<Event>,
        valid: bool,
        milliseconds: f64,
        warning: Option<String>,
    },
    Scan {
        job: u64,
        supplement: bool,
        progress: Progress,
    },
    WatchStarted {
        job: u64,
        ready: bool,
    },
    Events {
        job: u64,
        events: Events,
    },
    SupplementEvents {
        job: u64,
        events: Events,
    },
    AliasResolved {
        job: u64,
        is_alias: bool,
    },
    Scored {
        epoch: u64,
        response: Response,
        final_pass: bool,
        fuzzy: bool,
        /// A final pass hands back what it scored, for the next pass to extend.
        partial: Option<Box<Partial>>,
    },
    Saved {
        job: u64,
        event: Option<Event>,
        milliseconds: f64,
        error: Option<String>,
    },
}
/// The matches of the last finished pass and exactly which batches they cover. When the next
/// pass has the same query over the same batches plus new ones at the end, it scores only the
/// new ones and merges: the top 100, the counts and the total come out the same as a full pass.
pub(super) struct Partial {
    key: Key,
    exclude: Option<PathBuf>,
    fallback: bool,
    literal: bool,
    mac: Vec<Arc<Snapshot>>,
    supplement: Vec<Arc<Snapshot>>,
    matches: Matches,
}
/// Whether `old` is the start of `new`, batch for batch.
fn extends(old: &[Arc<Snapshot>], new: &[Arc<Snapshot>]) -> bool {
    old.len() <= new.len() && old.iter().zip(new).all(|(a, b)| Arc::ptr_eq(a, b))
}
#[derive(Clone, PartialEq, Eq)]
struct Key {
    query: Query,
    boosts: Vec<(String, i16)>,
}
struct Active {
    id: u64,
    signpost: u64,
    key: Key,
    started: Instant,
    first_result: bool,
}
struct Supplement {
    path: PathBuf,
    batches: Vec<Arc<Snapshot>>,
    complete: bool,
    cancel: Arc<AtomicBool>,
    job: u64,
    // The event stream survives individual scans of the same mounted volume.
    watch: (u64, Watcher),
    pending: HashSet<String>,
    pending_full: bool,
    failure: Option<String>,
}
struct AliasProbe {
    path: PathBuf,
    state: AliasState,
}
enum AliasState {
    Checking { job: u64, until: Instant },
    Resolved { is_alias: bool },
    Unavailable(String),
}
impl AliasProbe {
    fn deadline(&self) -> Option<Instant> {
        match self.state {
            AliasState::Checking { until, .. } => Some(until),
            _ => None,
        }
    }
}
struct ScanJob {
    id: u64,
    cancel: Arc<AtomicBool>,
    through: Option<Event>,
    start: Instant,
    incremental: bool,
}
struct Replay {
    start: Instant,
}
struct Worker {
    root: PathBuf,
    physical_root: PathBuf,
    support: PathBuf,
    cache_file: PathBuf,
    sender: Sender<Message>,
    shared: Arc<Mutex<Shared>>,
    generation: Arc<AtomicU64>,
    epoch: Arc<AtomicU64>,
    pool: Option<rayon::ThreadPool>,
    read_pool: Arc<discovery::ReadPool>,
    workers: usize,
    active: Option<Active>,
    history: History,
    batches: Vec<Arc<Snapshot>>,
    unique: Option<Paths>,
    complete: bool,
    failure: Option<String>,
    cache_loaded: bool,
    cache_legacy: bool,
    cache_identity: Option<cache::Identity>,
    loaded_blocks: HashSet<usize>,
    loading: bool,
    cache_cancel: Arc<AtomicBool>,
    cache_job: u64,
    cache_expected: Option<Event>,
    cache_priority: Option<Arc<Mutex<Option<String>>>>,
    scan: Option<ScanJob>,
    supplement: Option<Supplement>,
    alias_probe: Option<AliasProbe>,
    watch: Option<Watcher>,
    watch_job: u64,
    watch_active: bool,
    watch_start_until: Option<Instant>,
    replay: Option<Replay>,
    pending: HashSet<String>,
    excluded: HashSet<String>,
    pending_full: bool,
    refresh_at: Option<Instant>,
    event: Option<Event>,
    saved: Option<Event>,
    latest_event: u64,
    idle_at: Option<Instant>,
    serial: u64,
    scoring: bool,
    score_at: Option<Instant>,
    dirty: bool,
    partial: Option<Box<Partial>>,
    queries: VecDeque<(Key, Response, bool)>,
    report: Value,
    samples: VecDeque<Value>,
    diagnostics: bool,
    next_report_at: Instant,
    save_cancel: Arc<AtomicBool>,
    save_lock: Arc<Mutex<()>>,
    supplement_refresh_at: Option<Instant>,
    retry: HashSet<String>,
    retry_at: Option<Instant>,
    retry_delay: Duration,
    save_at: Option<Instant>,
    saving: Option<u64>,
}
pub(super) fn run(
    workers: usize,
    root: PathBuf,
    support: PathBuf,
    sender: Sender<Message>,
    receive: Receiver<Message>,
    shared: Arc<Mutex<Shared>>,
    generation: Arc<AtomicU64>,
    diagnostics: bool,
) {
    let mut worker = Worker {
        physical_root: fs::canonicalize(&root).unwrap_or_else(|_| root.clone()),
        root,
        cache_file: support.join("mac-paths.fff4.bin"),
        history: cache::history(&support.join("access-history.json")),
        support,
        sender,
        shared,
        generation,
        epoch: Arc::new(AtomicU64::new(0)),
        pool: None,
        read_pool: Arc::default(),
        workers,
        active: None,
        batches: Vec::new(),
        unique: None,
        complete: false,
        failure: None,
        cache_loaded: false,
        cache_legacy: false,
        cache_identity: None,
        loaded_blocks: HashSet::new(),
        loading: false,
        cache_cancel: Arc::new(AtomicBool::new(false)),
        cache_job: 0,
        cache_expected: None,
        cache_priority: None,
        scan: None,
        supplement: None,
        alias_probe: None,
        watch: None,
        watch_job: 0,
        watch_active: false,
        watch_start_until: None,
        replay: None,
        pending: HashSet::new(),
        excluded: HashSet::new(),
        pending_full: false,
        refresh_at: None,
        event: None,
        saved: None,
        latest_event: 0,
        idle_at: None,
        serial: 0,
        scoring: false,
        score_at: None,
        dirty: false,
        partial: None,
        queries: VecDeque::new(),
        samples: VecDeque::new(),
        diagnostics,
        next_report_at: Instant::now(),
        report: json!({"workers": workers, "engine_revision": snapshot::REVISION, "cache_loaded": false, "query_cache_hits": 0, "engine_passes": 0}),
        save_cancel: Arc::new(AtomicBool::new(false)),
        save_lock: Arc::new(Mutex::new(())),
        supplement_refresh_at: None,
        retry: HashSet::new(),
        retry_at: None,
        retry_delay: Duration::from_secs(2),
        save_at: None,
        saving: None,
    };
    loop {
        let message = if let Some(at) = worker.deadline() {
            receive.recv_timeout(at.saturating_duration_since(Instant::now()))
        } else {
            receive
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        match message {
            Ok(Message::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                worker.release();
                break;
            }
            Ok(message) => worker.message(message),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        worker.tick();
    }
}
impl Worker {
    fn deadline(&self) -> Option<Instant> {
        [
            self.score_at
                .filter(|_| !self.scoring && self.active.is_some()),
            self.watch_start_until,
            self.idle_at.map(|at| at + Duration::from_secs(60)),
            self.replay
                .as_ref()
                .map(|r| r.start + Duration::from_secs(30)),
            self.refresh_at.filter(|_| self.can_scan()),
            self.supplement_refresh_at
                .filter(|_| self.can_refresh_supplement()),
            self.alias_probe.as_ref().and_then(AliasProbe::deadline),
            self.retry_at.filter(|_| self.can_scan()),
            self.save_at.filter(|_| self.can_save()),
        ]
        .into_iter()
        .flatten()
        .min()
    }
    fn can_refresh_supplement(&self) -> bool {
        self.active.is_some()
            && self
                .supplement
                .as_ref()
                .is_some_and(|s| s.complete || s.failure.is_some())
    }
    fn can_scan(&self) -> bool {
        self.active.is_some()
            && !self.loading
            && self.scan.is_none()
            && self.replay.is_none()
            && self.watch_start_until.is_none()
    }
    fn can_save(&self) -> bool {
        self.can_scan() && self.complete && self.retry.is_empty() && self.saving.is_none()
    }
    fn next(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }
    fn update_report(&mut self) {
        self.report["indexed_paths"] = json!(self.batches.iter().map(|b| b.len()).sum::<usize>());
        let supplement_bytes = self
            .supplement
            .as_ref()
            .map_or(0, |s| s.batches.iter().map(|b| b.bytes()).sum::<usize>());
        self.report["resident_bytes"] =
            json!(self.batches.iter().map(|b| b.bytes()).sum::<usize>() + supplement_bytes);
        self.report["supplemental_paths"] = json!(
            self.supplement.as_ref().map_or(0, |s| s
                .batches
                .iter()
                .map(|b| b.len())
                .sum::<usize>())
        );
        self.report["retry_paths"] = json!(self.retry.len());
        self.report["query_cache_entries"] = json!(self.queries.len());
        self.report["physical_footprint_bytes"] = json!(physical_footprint());
        self.report["cache_loaded"] = json!(self.cache_loaded);
        self.report["indexed_event_id"] = json!(self.event.map(|event| event.id));
        self.report["latest_event_id"] = json!(self.latest_event);
        self.report["stages"] = json!(self.samples);
    }
    fn notify(&mut self, status: Option<Status>, response: Option<Response>) {
        let now = Instant::now();
        let report_changed = self.diagnostics
            && (now >= self.next_report_at
                || response.is_some()
                || status
                    .as_ref()
                    .is_some_and(|s| matches!(s, Status::Idle | Status::Failed(_))));
        if report_changed {
            self.update_report();
            self.next_report_at = now + Duration::from_millis(100);
        }
        let waker = if let Ok(mut state) = self.shared.lock() {
            let changed = status
                .as_ref()
                .is_some_and(|s| !s.same_visible_state(&state.status))
                || response.as_ref().is_some_and(|r| {
                    state.response.as_ref().is_none_or(|old| {
                        old.id != r.id
                            || old.hits != r.hits
                            || old.complete != r.complete
                            || old.scope_counts != r.scope_counts
                    })
                });
            if let Some(status) = status {
                state.status = status;
            }
            if let Some(response) = response {
                state.response = Some(response);
            }
            if report_changed {
                state.report = self.report.clone();
            }
            changed.then(|| state.waker.clone()).flatten()
        } else {
            None
        };
        if let Some(waker) = waker {
            waker();
        }
    }
    fn status(&mut self) {
        let indexed = self.batches.iter().map(|b| b.len()).sum::<usize>()
            + self
                .supplement
                .as_ref()
                .map_or(0, |s| s.batches.iter().map(|b| b.len()).sum());
        let status = if self.active.is_none() {
            Status::Idle
        } else if let Some(error) = self
            .failure
            .as_ref()
            .or_else(|| self.supplement.as_ref().and_then(|s| s.failure.as_ref()))
            .or_else(|| match &self.alias_probe.as_ref()?.state {
                AliasState::Unavailable(error) => Some(error),
                _ => None,
            })
        {
            Status::Failed(error.clone())
        } else if self.ready() {
            Status::Ready { indexed }
        } else {
            Status::Loading { indexed }
        };
        self.notify(Some(status), None);
    }
    fn ready(&self) -> bool {
        self.complete
            && !self.loading
            && self.replay.is_none()
            && self.scan.is_none()
            && !self.pending_full
            && self.pending.is_empty()
            && self.retry.is_empty()
            && self
                .alias_probe
                .as_ref()
                .is_none_or(|probe| matches!(probe.state, AliasState::Resolved { .. }))
            && self
                .supplement
                .as_ref()
                .is_none_or(|s| s.complete && s.pending.is_empty() && !s.pending_full)
    }
    fn invalidate(&mut self) {
        self.queries.clear();
        self.dirty = true;
    }
    fn message(&mut self, message: Message) {
        match message {
            Message::Search {
                id,
                signpost,
                mut query,
                started,
                enqueue_ms,
            } => {
                if self.generation.load(Ordering::Acquire) != id {
                    return;
                }
                query.text = snapshot::folded(query.text.trim());
                if query.text.is_empty() {
                    self.pause();
                    return;
                }
                if let Some(priority) = &self.cache_priority
                    && let Ok(mut pending) = priority.lock()
                {
                    *pending = (query.text.len() >= 3).then(|| query.text.clone());
                }
                let was_idle = self.active.is_none();
                self.idle_at = None;
                self.report["idle_released"] = json!(false);
                self.epoch.fetch_add(1, Ordering::AcqRel);
                self.scoring = false;
                self.score_at = None;
                let key = Key {
                    query,
                    boosts: cache::boosts(&self.history),
                };
                if self.diagnostics {
                    let dispatch_ms = started.elapsed().as_secs_f64() * 1000.0;
                    self.samples.push_back(json!({
                        "sequence": id,
                        "input_enqueue_ms": enqueue_ms,
                        "input_dispatch_ms": dispatch_ms,
                    }));
                    if self.samples.len() > 32 {
                        self.samples.pop_front();
                    }
                }
                self.active = Some(Active {
                    id,
                    signpost,
                    key,
                    started,
                    first_result: false,
                });
                self.ensure_supplement();
                if !self.cache_loaded && !self.loading {
                    self.load();
                } else if was_idle && (self.pending_full || !self.pending.is_empty()) {
                    self.refresh_at = Some(Instant::now());
                } else if !self.complete && self.scan.is_none() && !self.loading {
                    self.begin_scan(None);
                }
                if self.ready()
                    && let Some(position) = self
                        .queries
                        .iter()
                        .position(|(key, _, _)| Some(key) == self.active.as_ref().map(|a| &a.key))
                {
                    if let Some((key, mut response, fuzzy)) = self.queries.remove(position) {
                        response.id = id;
                        self.report["query_cache_hits"] =
                            json!(self.report["query_cache_hits"].as_u64().unwrap_or(0) + 1);
                        self.report["last_query_cache_hit"] = json!(true);
                        self.published(response.clone(), true);
                        self.queries.push_front((key, response, fuzzy));
                    }
                } else {
                    self.score();
                }
                self.status();
            }
            Message::Access(path) => {
                let Some(path) = path.to_str().filter(|p| p.starts_with('/')) else {
                    return;
                };
                let now = cache::now();
                let visit = self.history.entry(path.to_owned()).or_insert(Visit {
                    count: 0,
                    opened: now,
                });
                visit.count = (visit.count + 1).min(100);
                visit.opened = now;
                if self.history.len() > 1000 {
                    if let Some(oldest) = self
                        .history
                        .iter()
                        .min_by(|a, b| a.1.opened.total_cmp(&b.1.opened))
                        .map(|(p, _)| p.clone())
                    {
                        self.history.remove(&oldest);
                    }
                }
                self.invalidate();
                if let Ok(bytes) = serde_json::to_vec(&self.history) {
                    if let Err(error) =
                        cache::atomic_write(&self.support.join("access-history.json"), |file| {
                            file.write_all(&bytes)
                        })
                    {
                        self.report["history_error"] = json!(error.to_string());
                    }
                }
                if let Some(active) = &mut self.active {
                    active.key.boosts = cache::boosts(&self.history);
                }
                self.restart();
            }
            Message::Idle => self.pause(),
            Message::Suspend => {
                self.active = None;
                self.release();
            }
            Message::Trace {
                id,
                stage,
                milliseconds,
            } => {
                if matches!(
                    stage,
                    "first_rows_ms"
                        | "publication_ms"
                        | "row_update_work_ms"
                        | "first_title_draw_ms"
                        | "title_draw_work_ms"
                ) && milliseconds.is_finite()
                    && let Some(sample) = self
                        .samples
                        .iter_mut()
                        .find(|s| s["sequence"].as_u64() == Some(id))
                {
                    if matches!(
                        stage,
                        "first_rows_ms" | "publication_ms" | "first_title_draw_ms"
                    ) {
                        if sample.get(stage).is_none() {
                            sample[stage] = json!(milliseconds.max(0.0));
                        }
                    } else {
                        sample[stage] = json!(milliseconds.max(0.0));
                    }
                    // Preserve the last draw stamp even when no further index work follows.
                    self.next_report_at = Instant::now();
                    self.notify(None, None);
                }
            }
            Message::WarmBatch { job, batch }
                if self.active.is_some()
                    && job == self.cache_job
                    && !self.complete
                    && self.scan.is_none()
                    && self.loaded_blocks.is_empty() =>
            {
                self.append(batch);
            }
            Message::CacheBatch {
                job,
                position,
                batch,
            } if self.loading && job == self.cache_job => {
                if !self.loaded_blocks.insert(position) {
                    return;
                }
                // Saved batches are disjoint. Only the small warm listing can overlap;
                // do not build a second million-row table while restoring the cache.
                let batch = self.unique.as_ref().map_or_else(
                    || Some(Arc::clone(&batch)),
                    |warm| warm.without_existing(Arc::clone(&batch)),
                );
                if let Some(batch) = batch {
                    self.batches.push(batch);
                    self.invalidate();
                    self.schedule_index_score();
                    self.status();
                }
            }
            Message::CacheDone {
                job,
                event,
                valid,
                milliseconds,
                warning,
            } if self.loading && job == self.cache_job => {
                self.loading = false;
                self.cache_priority = None;
                self.cache_loaded = true;
                self.unique = None;
                self.report["cache_load_ms"] = json!(milliseconds);
                if let Some(warning) = warning {
                    self.report["cache_error"] = json!(warning);
                }
                self.dirty = true;
                if event != self.cache_expected || (!valid && self.cache_expected.is_some()) {
                    self.fallback("cache changed");
                } else if self.replay.is_none() && self.watch_start_until.is_none() {
                    let changes = if self.pending_full {
                        None
                    } else {
                        Some(std::mem::take(&mut self.pending))
                    };
                    self.begin_scan(changes);
                }
                self.schedule_index_score();
                self.status();
            }
            Message::Scan {
                job,
                supplement: false,
                progress,
            } if self.scan.as_ref().is_some_and(|s| s.id == job) => match progress {
                Progress::Batch(batch) => {
                    self.batches.push(batch);
                    self.invalidate();
                    self.schedule_index_score();
                    self.status();
                }
                Progress::Finished(result) => {
                    let Some(scan) = self.scan.take() else {
                        return;
                    };
                    match result {
                        Ok(scan_result) => {
                            let changed = scan_result.changed
                                || !same_batches(&self.batches, &scan_result.batches);
                            self.batches = scan_result.batches;
                            self.unique = None;
                            self.excluded.extend(scan_result.excluded);
                            self.complete = true;
                            self.retry.extend(scan_result.retry);
                            if self.retry.is_empty() {
                                self.event = scan.through;
                                self.retry_delay = Duration::from_secs(2);
                            } else {
                                self.retry_at
                                    .get_or_insert(Instant::now() + self.retry_delay);
                                self.retry_delay =
                                    (self.retry_delay * 2).min(Duration::from_secs(60));
                            }
                            self.report["index_ms"] =
                                json!(scan.start.elapsed().as_secs_f64() * 1000.0);
                            self.report["warning"] = json!(scan_result.warning);
                            if changed {
                                self.invalidate();
                            }
                            let behind = self
                                .event
                                .map_or(0.0, |e| e.date - self.saved.map_or(0.0, |s| s.date));
                            if changed || !scan.incremental || behind > 3600.0 || self.cache_legacy
                            {
                                if scan.incremental {
                                    self.queue_save();
                                } else {
                                    self.save_at = Some(Instant::now());
                                }
                            }
                            self.restart();
                            self.status();
                        }
                        Err(error) => {
                            self.report["scan_error"] = json!(error);
                            self.failure = Some(error);
                            self.status();
                        }
                    }
                }
            },
            Message::Scan {
                job,
                supplement: true,
                progress,
            } if self.active.is_some()
                && self
                    .supplement
                    .as_ref()
                    .is_some_and(|s| s.job == job && !s.cancel.load(Ordering::Acquire)) =>
            {
                let Some(supplement) = &mut self.supplement else {
                    return;
                };
                match progress {
                    Progress::Batch(batch) => {
                        supplement.batches.push(batch);
                        self.invalidate();
                        if !self.scoring {
                            self.score();
                        }
                    }
                    Progress::Finished(result) => {
                        supplement.complete = true;
                        match result {
                            Ok(result) => {
                                if !result.retry.is_empty() {
                                    supplement.pending.extend(result.retry);
                                    self.supplement_refresh_at =
                                        Some(Instant::now() + Duration::from_secs(5));
                                }
                                let batches = result.batches;
                                if same_batches(&supplement.batches, &batches) {
                                    self.status();
                                    if self.scoring {
                                        self.dirty = true;
                                    } else {
                                        self.score();
                                    }
                                    return;
                                }
                                supplement.batches = batches;
                            }
                            Err(error) => {
                                supplement.complete = false;
                                supplement.failure = Some(error.clone());
                                self.report["supplement_error"] = json!(error);
                                self.notify(Some(Status::Failed(error)), None);
                                return;
                            }
                        };
                        self.invalidate();
                        self.restart();
                        self.status();
                    }
                }
            }
            Message::WatchStarted { job, ready }
                if job == self.watch_job && self.watch_start_until.is_some() =>
            {
                self.watch_started(ready)
            }
            Message::Events { job, events } if job == self.watch_job => self.events(events),
            Message::SupplementEvents { job, events }
                if self.supplement.as_ref().is_some_and(|s| s.watch.0 == job) =>
            {
                let Some(supplement) = &mut self.supplement else {
                    return;
                };
                supplement.pending_full |= events.full_scan;
                for path in events.paths {
                    if path.starts_with(&self.support) {
                        continue;
                    }
                    let Ok(relative) = path.strip_prefix(&events.root) else {
                        continue;
                    };
                    let Some(relative) = relative.to_str() else {
                        continue;
                    };
                    if relative.is_empty() {
                        supplement.pending_full = true;
                    } else if !relative.split('/').any(|p| p.starts_with('.')) {
                        supplement.pending.insert(relative.to_owned());
                    }
                }
                if supplement.pending.len() > 50_000 {
                    supplement.pending_full = true;
                    supplement.pending.clear();
                }
                if supplement.pending_full || !supplement.pending.is_empty() {
                    self.supplement_refresh_at = Some(Instant::now() + Duration::from_millis(220));
                }
            }
            Message::AliasResolved { job, is_alias }
                if self.active.is_some()
                    && self.alias_probe.as_ref().is_some_and(|probe| {
                        matches!(probe.state, AliasState::Checking { job: current, .. } if current == job)
                    }) =>
            {
                if let Some(probe) = &mut self.alias_probe {
                    probe.state = AliasState::Resolved { is_alias };
                }
                self.ensure_supplement();
                if self.scoring {
                    self.dirty = true;
                } else {
                    self.score();
                }
                self.status();
            }
            Message::Scored {
                epoch,
                mut response,
                final_pass,
                fuzzy,
                partial,
            } if epoch == self.epoch.load(Ordering::Acquire)
                && self.active.as_ref().is_some_and(|a| a.id == response.id) =>
            {
                if final_pass {
                    self.scoring = false;
                    self.partial = partial;
                }
                if final_pass && self.dirty {
                    self.schedule_index_score();
                    return;
                }
                response.complete &= self.ready();
                let cacheable = response.complete && final_pass;
                if cacheable && let Some(active) = &self.active {
                    self.queries.retain(|(key, _, _)| key != &active.key);
                    self.queries
                        .push_front((active.key.clone(), response.clone(), fuzzy));
                    self.queries.truncate(8);
                }
                self.report["fuzzy_fallback"] = json!(fuzzy);
                self.published(response, false);
            }
            Message::Saved {
                job,
                event,
                milliseconds,
                error,
            } if self.saving == Some(job) => {
                self.saving = None;
                self.report["cache_save_ms"] = json!(milliseconds);
                self.report["cache_save_error"] = json!(error);
                if error.is_none() {
                    self.saved = event;
                    self.cache_legacy = false;
                }
                self.next_report_at = Instant::now();
                self.notify(None, None);
            }
            _ => {}
        }
    }
    fn published(&mut self, response: Response, cache_hit: bool) {
        if let Some(active) = &mut self.active {
            performance::published(active.signpost, response.complete, cache_hit);
            let ms = active.started.elapsed().as_secs_f64() * 1000.0;
            if let Some(sample) = self.samples.back_mut() {
                if sample.get("first_publish_ms").is_none() {
                    sample["first_publish_ms"] = json!(ms);
                }
                if !response.hits.is_empty() && !active.first_result {
                    active.first_result = true;
                    sample["first_result_ms"] = json!(ms);
                    self.report["first_result_ms"] = json!(ms);
                }
                if response.complete {
                    sample["complete_publish_ms"] = json!(ms);
                }
                sample["cache_hit"] = json!(cache_hit);
            }
        }
        self.report["total_matches"] = json!(response.total);
        self.notify(None, Some(response));
    }
    fn append(&mut self, batch: Arc<Snapshot>) {
        if self.unique.is_none() {
            self.unique = Some(Paths::new(&self.batches));
        }
        let addition = self
            .unique
            .as_mut()
            .and_then(|paths| paths.append_unique(batch));
        if let Some(addition) = addition {
            self.batches.push(addition);
            self.invalidate();
            self.schedule_index_score();
            self.status();
        }
    }
    fn load(&mut self) {
        let failure = match fs::metadata(&self.root) {
            Ok(metadata) if metadata.is_dir() => None,
            Ok(_) => Some("The search root is not a folder".to_owned()),
            Err(error) => Some(error.to_string()),
        };
        if let Some(error) = failure {
            self.failure = Some(error.clone());
            self.report["root_error"] = json!(error);
            self.status();
            return;
        }
        self.failure = None;
        self.report["root_error"] = Value::Null;
        self.physical_root = fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        self.loading = true;
        self.cache_cancel = Arc::new(AtomicBool::new(false));
        self.cache_job = self.next();
        let root = self.root.clone();
        let folders = if root == Path::new("/") {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.clone());
            vec![home.clone(), home.join("Downloads"), home.join("Desktop")]
        } else {
            vec![root.clone()]
        };
        let token = Arc::clone(&self.cache_cancel);
        let tx = self.sender.clone();
        let job = self.cache_job;
        let pool = Arc::clone(&self.read_pool);
        let _ = std::thread::Builder::new()
            .name("crawler-warm-paths".into())
            .spawn(move || {
                let _ = pool.submit(move || {
                    discovery::warm(&root, &folders, &token, |batch| {
                        let _ = tx.send(Message::WarmBatch { job, batch });
                    });
                });
            });
        let reader = Reader::open(&self.cache_file, &self.root).map(|mut reader| {
            if self.cache_identity != Some(reader.identity) {
                // Only an atomically replaced cache invalidates a partial restore.
                // The visible Response keeps its hits until the new query publishes.
                self.batches.clear();
                self.unique = None;
                self.loaded_blocks.clear();
                self.invalidate();
            }
            self.cache_identity = Some(reader.identity);
            self.report["cache_reused_batches"] = json!(self.loaded_blocks.len());
            reader.skip_loaded(&self.loaded_blocks);
            reader
        });
        self.cache_legacy = reader.as_ref().is_ok_and(|r| r.legacy);
        self.saved = reader.as_ref().ok().and_then(|r| r.event);
        self.report["cache_version"] = json!(if self.cache_legacy { 4 } else { 5 });
        self.report["saved_event_id"] = json!(self.saved.map(|e| e.id));
        self.cache_expected = self.saved;
        self.event = self.saved;
        let saved = self.saved.filter(|e| {
            cache::now() - e.date < 86400.0
                && e.date <= cache::now() + 60.0
                && e.id <= watcher::current()
        });
        self.start_watcher(saved.map(|s| s.id));
        if let Some(saved) = saved {
            self.replay = Some(Replay {
                start: Instant::now(),
            });
            self.report["file_search_catch_up"] = json!({"method": "replay", "saved_hours_ago": (cache::now() - saved.date) / 3600.0});
        } else {
            self.pending_full = true;
            self.report["file_search_catch_up"] = json!({"method": "walk", "reason": if self.saved.is_none() { "no saved event" } else if saved.is_none() { "saved event too old" } else { "history unavailable" }});
        }
        let cancel = Arc::clone(&self.cache_cancel);
        let tx = self.sender.clone();
        let job = self.cache_job;
        let loaders = self.workers.min(8);
        let priority = Arc::new(Mutex::new(self.active.as_ref().and_then(|active| {
            let text = &active.key.query.text;
            (text.len() >= 3).then(|| text.clone())
        })));
        self.cache_priority = Some(Arc::clone(&priority));
        let _ = std::thread::Builder::new()
            .name("crawler-cache-load".into())
            .spawn(move || {
                let start = Instant::now();
                let mut event = None;
                let mut valid = false;
                let mut warning = None;
                if let Err(error) = &reader
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    warning = Some(error.to_string());
                }
                if let Ok(reader) = reader {
                    event = reader.event;
                    // Blocks are independent: verify and decode them on several threads.
                    // The queue keeps the query's priority order for arrival.
                    let (decoder, blocks) = reader.split();
                    let queue = Mutex::new((blocks, start));
                    let failure = Mutex::new(None::<String>);
                    std::thread::scope(|scope| {
                        for _ in 0..loaders {
                            let tx = tx.clone();
                            let (decoder, queue, failure, cancel, priority) =
                                (&decoder, &queue, &failure, &cancel, &priority);
                            scope.spawn(move || {
                                loop {
                                    if cancel.load(Ordering::Relaxed)
                                        || failure.lock().is_ok_and(|f| f.is_some())
                                    {
                                        return;
                                    }
                                    let Some(block) = queue.lock().ok().and_then(|mut queue| {
                                        let (blocks, next_priority_at) = &mut *queue;
                                        let now = Instant::now();
                                        if blocks.len() > loaders
                                            && now >= *next_priority_at
                                            && let Some(text) = priority
                                                .lock()
                                                .ok()
                                                .and_then(|mut pending| pending.take())
                                        {
                                            decoder.prioritize(blocks, &text);
                                            *next_priority_at = now + Duration::from_millis(40);
                                        }
                                        blocks.pop_front()
                                    }) else {
                                        return;
                                    };
                                    match decoder.decode(block) {
                                        Ok((position, batch)) => {
                                            let sent = tx.send(Message::CacheBatch {
                                                job,
                                                position,
                                                batch,
                                            });
                                            if sent.is_err() {
                                                return;
                                            }
                                        }
                                        Err(error) => {
                                            if let Ok(mut failure) = failure.lock() {
                                                failure.get_or_insert(error.to_string());
                                            }
                                            return;
                                        }
                                    }
                                }
                            });
                        }
                    });
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    match failure.into_inner().ok().flatten() {
                        Some(error) => warning = Some(error),
                        None => valid = true,
                    }
                }
                if !cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(Message::CacheDone {
                        job,
                        event,
                        valid,
                        milliseconds: start.elapsed().as_secs_f64() * 1000.0,
                        warning,
                    });
                }
            });
    }
    fn start_watcher(&mut self, since: Option<u64>) {
        self.watch = None;
        self.watch_job = self.next();
        let job = self.watch_job;
        let tx = self.sender.clone();
        let ready_tx = tx.clone();
        self.watch_active = false;
        self.watch_start_until = Some(Instant::now() + Duration::from_secs(2));
        self.watch = Some(Watcher::start(
            self.root.clone(),
            since,
            move |ready| {
                let _ = ready_tx.send(Message::WatchStarted { job, ready });
            },
            move |events| {
                let _ = tx.send(Message::Events { job, events });
            },
        ));
    }
    fn watch_started(&mut self, ready: bool) {
        self.watch_start_until = None;
        self.watch_active = ready;
        if !ready {
            self.watch_job = self.next();
            self.watch = None;
            self.replay = None;
            self.pending_full = true;
            self.report["file_search_catch_up"] =
                json!({"method":"walk","reason":"history unavailable"});
        }
        if self.pending_full {
            self.refresh_at = Some(Instant::now());
        }
    }
    fn events(&mut self, events: Events) {
        self.latest_event = self.latest_event.max(events.last_id);
        let alias_changed = self.alias_probe.as_ref().is_some_and(|probe| {
            events.full_scan || events.paths.iter().any(|path| probe.path.starts_with(path))
        });
        if events.full_scan {
            self.pending_full = true;
        }
        for path in events.paths {
            if self.pending_full {
                break;
            }
            if path.starts_with(&self.support) {
                continue;
            }
            let Ok(relative) = path.strip_prefix(&self.physical_root) else {
                continue;
            };
            let Some(relative) = relative.to_str() else {
                continue;
            };
            let relative = relative.trim_end_matches('/');
            if relative.is_empty() {
                self.pending_full = true;
                continue;
            }
            if relative.split('/').any(|p| p.starts_with('.'))
                || discovery::parent_contains(relative, &self.excluded)
            {
                continue;
            }
            self.pending.insert(relative.to_owned());
        }
        if self.pending.len() > 50_000 {
            self.pending_full = true;
        }
        if self.pending_full {
            self.pending.clear();
        }
        if self.replay.is_some() {
            if self.pending_full {
                self.fallback(if events.full_scan {
                    "history needs a full scan"
                } else {
                    "too many changes"
                });
            } else if events.history_done {
                if let Some(replay) = self.replay.take() {
                    self.report["file_search_catch_up"]["replay_ms"] =
                        json!(replay.start.elapsed().as_secs_f64() * 1000.0);
                    self.report["file_search_catch_up"]["replayed_paths"] =
                        json!(self.pending.len());
                }
                if !self.loading {
                    let changes = std::mem::take(&mut self.pending);
                    self.begin_scan(Some(changes));
                }
            }
        } else if (!self.pending.is_empty() || self.pending_full) && self.active.is_some() {
            self.refresh_at = Some(Instant::now() + Duration::from_millis(220));
        }
        if alias_changed {
            self.alias_probe = None;
            if self.active.is_some() {
                self.ensure_supplement();
                if self.scoring {
                    self.dirty = true;
                } else {
                    self.score();
                }
                self.status();
            }
        }
    }
    fn fallback(&mut self, reason: &str) {
        if let Some(replay) = self.replay.take() {
            self.report["file_search_catch_up"]["replay_ms"] =
                json!(replay.start.elapsed().as_secs_f64() * 1000.0);
        }
        self.report["file_search_catch_up"]["method"] = json!("walk");
        self.report["file_search_catch_up"]["reason"] = json!(reason);
        self.start_watcher(None);
        self.pending.clear();
        self.pending_full = true;
        self.event = None;
        if !self.loading && self.scan.is_none() {
            self.begin_scan(None);
        }
    }
    fn begin_scan(&mut self, changes: Option<HashSet<String>>) {
        if !self.can_scan() {
            return;
        }
        if changes.as_ref().is_some_and(HashSet::is_empty) {
            self.complete = true;
            if self.watch_active
                && self.retry.is_empty()
                && self.latest_event > self.event.map_or(0, |e| e.id)
            {
                self.event = Some(Event::now(self.latest_event));
            }
            if self.cache_legacy
                || self
                    .event
                    .is_some_and(|e| e.date - self.saved.map_or(0.0, |s| s.date) > 3600.0)
            {
                self.queue_save();
            }
            self.restart();
            self.status();
            return;
        }
        let incremental = changes.is_some();
        if !incremental {
            self.pending_full = false;
            self.pending.clear();
            self.excluded.clear();
        } else if let Some(changes) = &changes {
            self.excluded
                .retain(|p| !discovery::ancestors_contain(p, changes));
        }
        let through = self.watch_active.then(|| {
            Event::now(if incremental {
                self.latest_event.max(self.event.map_or(0, |e| e.id))
            } else {
                watcher::current()
            })
        });
        let id = self.next();
        let cancel = Arc::new(AtomicBool::new(false));
        self.scan = Some(ScanJob {
            id,
            cancel: Arc::clone(&cancel),
            through,
            start: Instant::now(),
            incremental,
        });
        self.complete = false;
        self.failure = None;
        let pool = Arc::clone(&self.read_pool);
        let root = self.root.clone();
        let previous = self.batches.clone();
        let tx = self.sender.clone();
        let _ = std::thread::Builder::new()
            .name("crawler-discovery".into())
            .spawn(move || {
                discovery::run(
                    root,
                    String::new(),
                    previous,
                    changes,
                    cancel,
                    pool,
                    move |progress| {
                        let _ = tx.send(Message::Scan {
                            job: id,
                            supplement: false,
                            progress,
                        });
                    },
                )
            });
        self.status();
    }
    fn ensure_supplement(&mut self) {
        let Some(query) = self.active.as_ref().map(|active| &active.key.query) else {
            return;
        };
        let desired = if query.volume.is_none()
            && query.folder_mode
            && let Scope::Under(path) = &query.scope
            && path != &self.root
        {
            let path = path.clone();
            if !self.alias_probe.as_ref().is_some_and(|probe| {
                probe.path == path && !matches!(probe.state, AliasState::Unavailable(_))
            }) {
                self.probe_alias(path);
            }
            self.alias_probe.as_ref().and_then(|probe| {
                let use_supplement = match probe.state {
                    AliasState::Resolved { is_alias } => is_alias,
                    _ => self
                        .supplement
                        .as_ref()
                        .is_some_and(|s| s.path == probe.path),
                };
                use_supplement.then(|| probe.path.clone())
            })
        } else {
            self.alias_probe = None;
            query.volume.clone()
        };
        if self.supplement.as_ref().map(|s| &s.path) == desired.as_ref()
            && self.supplement.as_ref().is_none_or(|s| {
                s.failure.is_none() && (s.complete || !s.cancel.load(Ordering::Acquire))
            })
        {
            if self
                .supplement
                .as_ref()
                .is_some_and(|s| s.pending_full || !s.pending.is_empty())
            {
                self.supplement_refresh_at = Some(Instant::now());
            }
            return;
        }
        let previous = self.supplement.take().and_then(|old| {
            old.cancel.store(true, Ordering::Release);
            (Some(&old.path) == desired.as_ref()).then_some((old.batches, old.watch))
        });
        self.invalidate();
        let Some(path) = desired.filter(|p| p.starts_with(&self.root) && p != &self.root) else {
            return;
        };
        let (batches, watch) = previous.map_or_else(|| (Vec::new(), None), |(b, w)| (b, Some(w)));
        self.start_supplement(path, batches, None, watch);
    }
    fn probe_alias(&mut self, path: PathBuf) {
        let job = self.next();
        let tx = self.sender.clone();
        let location = path.clone();
        let state = match self.read_pool.submit(move || {
            let is_alias = discovery::alias_scope(&location);
            let _ = tx.send(Message::AliasResolved { job, is_alias });
        }) {
            Ok(true) => AliasState::Checking {
                job,
                until: Instant::now() + Duration::from_secs(2),
            },
            Ok(false) => AliasState::Unavailable("macOS is still reading other folders".into()),
            Err(error) => AliasState::Unavailable(error),
        };
        self.alias_probe = Some(AliasProbe { path, state });
    }
    fn start_supplement(
        &mut self,
        path: PathBuf,
        previous: Vec<Arc<Snapshot>>,
        changes: Option<HashSet<String>>,
        watch: Option<(u64, Watcher)>,
    ) {
        let job = self.next();
        let cancel = Arc::new(AtomicBool::new(false));
        let watch = watch.unwrap_or_else(|| {
            let event_tx = self.sender.clone();
            let stream = Watcher::start(
                path.clone(),
                Some(watcher::current()),
                |_| {},
                move |events| {
                    let _ = event_tx.send(Message::SupplementEvents { job, events });
                },
            );
            (job, stream)
        });
        self.supplement = Some(Supplement {
            path: path.clone(),
            batches: previous.clone(),
            complete: false,
            cancel: Arc::clone(&cancel),
            job,
            watch,
            pending: HashSet::new(),
            pending_full: false,
            failure: None,
        });
        let tx = self.sender.clone();
        let prefix = path
            .strip_prefix(&self.root)
            .ok()
            .and_then(Path::to_str)
            .unwrap_or("")
            .to_owned();
        let pool = Arc::clone(&self.read_pool);
        let _ = std::thread::Builder::new()
            .name("crawler-supplement".into())
            .spawn(move || {
                discovery::run(
                    path,
                    prefix,
                    previous,
                    changes,
                    cancel,
                    pool,
                    move |progress| {
                        let _ = tx.send(Message::Scan {
                            job,
                            supplement: true,
                            progress,
                        });
                    },
                )
            });
    }
    fn restart(&mut self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.scoring = false;
        self.score();
    }
    fn schedule_index_score(&mut self) {
        if !self.dirty {
            return;
        }
        // The first usable result and every typed query still start immediately.
        // Coalesce index arrivals after that result, rather than rescoring the
        // growing cache dozens of times before its restore finishes.
        if self
            .active
            .as_ref()
            .is_some_and(|active| !active.first_result)
        {
            if !self.scoring {
                self.score();
            }
        } else if self.active.is_some() {
            self.score_at
                .get_or_insert_with(|| Instant::now() + Duration::from_millis(40));
        }
    }
    fn score(&mut self) {
        if self.scoring {
            self.dirty = true;
            return;
        }
        self.score_at = None;
        let Some(active) = &self.active else {
            return;
        };
        let id = active.id;
        if self.generation.load(Ordering::Acquire) != id {
            return;
        }
        let complete = self.ready();
        if !complete
            && self.batches.is_empty()
            && self
                .supplement
                .as_ref()
                .is_none_or(|s| s.batches.is_empty())
        {
            return;
        }
        let key = active.key.clone();
        let boosts = matching::boosts(&key.boosts, &self.root);
        let mac = self.batches.clone();
        let (exclude, supplement) = match &self.supplement {
            Some(s) => (Some(s.path.clone()), s.batches.clone()),
            None => (None, Vec::new()),
        };
        let root = self.root.clone();
        let epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let token = Arc::clone(&self.epoch);
        let generation = Arc::clone(&self.generation);
        let tx = self.sender.clone();
        let publish_early = !active.first_result;
        let fallback = self.cache_loaded && key.query.text.split_whitespace().count() > 1;
        let workers = self.workers;
        let previous = self.partial.take();
        self.scoring = true;
        self.dirty = false;
        self.report["engine_passes"] =
            json!(self.report["engine_passes"].as_u64().unwrap_or(0) + 1);
        self.report["last_query_cache_hit"] = json!(false);
        let pool = match &mut self.pool {
            Some(pool) => pool,
            slot @ None => match rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .thread_name(|n| format!("crawler-match-{n}"))
                .build()
            {
                Ok(pool) => slot.insert(pool),
                Err(error) => {
                    self.scoring = false;
                    self.failure = Some(error.to_string());
                    self.status();
                    return;
                }
            },
        };
        pool.spawn(move || {
            let cancelled = || {
                token.load(Ordering::Acquire) != epoch || generation.load(Ordering::Acquire) != id
            };
            let query = &key.query;
            let score = |mac: &[Arc<Snapshot>], supplement: &[Arc<Snapshot>], literal: bool| {
                matching::pass(
                    mac,
                    supplement,
                    query,
                    &root,
                    exclude.as_deref(),
                    &boosts,
                    literal,
                    &cancelled,
                )
            };
            let previous = previous.filter(|p| {
                p.key == key
                    && p.exclude == exclude
                    && p.fallback == fallback
                    && extends(&p.mac, &mac)
                    && extends(&p.supplement, &supplement)
            });
            let mut sent_early = false;
            let (matches, literal) = if let Some(previous) = previous {
                // Only the batches that arrived since the last pass are scored.
                let new_mac = &mac[previous.mac.len()..];
                let new_supplement = &supplement[previous.supplement.len()..];
                let mut matches = previous.matches;
                let added = score(new_mac, new_supplement, true);
                if previous.literal {
                    matches.merge(added);
                    (matches, true)
                } else if added.total > 0 {
                    // The earlier batches had no literal match, so the fuzzy fallback stood
                    // in. A new literal match takes priority over that fallback.
                    (added, true)
                } else {
                    matches.merge(score(new_mac, new_supplement, false));
                    (matches, false)
                }
            } else {
                let mut literal = true;
                loop {
                    let mut matches = Matches::default();
                    let mut scored = 0;
                    // Until the first usable result is out, score a few batches at a time
                    // and publish as soon as one of them matches. Cache blocks arrive in the
                    // query's priority order, so a match is normally among the first groups.
                    let early = (workers * 8).min(mac.len());
                    while publish_early && literal && scored < early && matches.hits.is_empty() {
                        if cancelled() {
                            return;
                        }
                        let end = (scored + workers).min(mac.len());
                        matches.merge(score(&mac[scored..end], &[], literal));
                        scored = end;
                        if !matches.hits.is_empty() {
                            let response = matches.response(query, &root, id, false, literal);
                            let _ = tx.send(Message::Scored {
                                epoch,
                                response,
                                final_pass: false,
                                fuzzy: !literal,
                                partial: None,
                            });
                            sent_early = true;
                        }
                    }
                    // Everything else is one parallel pass with a single merge.
                    matches.merge(score(&mac[scored..], &supplement, literal));
                    if cancelled() {
                        return;
                    }
                    if matches.total == 0 && literal && fallback {
                        literal = false;
                        continue;
                    }
                    break (matches, literal);
                }
            };
            if cancelled() {
                return;
            }
            // A final response is dropped when batches arrived meanwhile. The first usable
            // result must not wait for arrivals to stop, so it goes out on its own first.
            if publish_early && !sent_early && !matches.hits.is_empty() {
                let _ = tx.send(Message::Scored {
                    epoch,
                    response: matches.response(query, &root, id, false, literal),
                    final_pass: false,
                    fuzzy: !literal,
                    partial: None,
                });
            }
            let response = matches.response(query, &root, id, complete, literal);
            let _ = tx.send(Message::Scored {
                epoch,
                response,
                final_pass: true,
                fuzzy: !literal,
                partial: Some(Box::new(Partial {
                    key,
                    exclude,
                    fallback,
                    literal,
                    mac,
                    supplement,
                    matches,
                })),
            });
        });
    }
    fn queue_save(&mut self) {
        self.save_at
            .get_or_insert_with(|| Instant::now() + Duration::from_secs(1));
    }
    fn persist(&mut self) {
        if !self.cache_loaded || self.loading || self.replay.is_some() {
            return;
        }
        let job = self.next();
        self.saving = Some(job);
        self.save_cancel = Arc::new(AtomicBool::new(false));
        let token = Arc::clone(&self.save_cancel);
        let gate = Arc::clone(&self.save_lock);
        let batches = self.batches.clone();
        let event = self.event;
        let root = self.root.clone();
        let file = self.cache_file.clone();
        let tx = self.sender.clone();
        let spawned = std::thread::Builder::new()
            .name("crawler-cache-save".into())
            .spawn(move || {
                let Ok(_guard) = gate.lock() else {
                    return;
                };
                if token.load(Ordering::Acquire) {
                    return;
                }
                let at = Instant::now();
                let error = cache::save(&file, &root, &batches, event, || {
                    token.load(Ordering::Relaxed)
                })
                .err()
                .map(|e| e.to_string());
                if !token.load(Ordering::Relaxed) {
                    let _ = tx.send(Message::Saved {
                        job,
                        event,
                        milliseconds: at.elapsed().as_secs_f64() * 1000.0,
                        error,
                    });
                }
            });
        if let Err(error) = spawned {
            self.saving = None;
            self.report["cache_save_error"] = json!(error.to_string());
        }
    }
    fn pause(&mut self) {
        self.active = None;
        if self
            .alias_probe
            .as_ref()
            .is_some_and(|probe| !matches!(probe.state, AliasState::Resolved { .. }))
        {
            self.alias_probe = None;
        }
        self.partial = None;
        self.score_at = None;
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.scoring = false;
        if self.loading {
            self.cache_cancel.store(true, Ordering::Release);
            self.loading = false;
            self.cache_loaded = false;
            self.cache_priority = None;
        }
        if let Some(scan) = self.scan.take() {
            scan.cancel.store(true, Ordering::Release);
            self.complete = false;
            self.pending_full = true;
        }
        if let Some(supplement) = &self.supplement {
            supplement.cancel.store(true, Ordering::Release);
        }
        if self.saving.take().is_some() {
            self.save_cancel.store(true, Ordering::Release);
            self.queue_save();
        }
        self.supplement_refresh_at = None;
        self.idle_at = Some(Instant::now());
        self.status();
    }
    fn release(&mut self) {
        self.pause();
        self.idle_at = None;
        self.pool = None;
        // Retain the permit budget if a cancelled scan still holds a macOS call.
        if Arc::get_mut(&mut self.read_pool).is_some_and(|pool| pool.is_idle()) {
            self.read_pool = Arc::default();
        }
        self.alias_probe = None;
        self.retry.clear();
        self.retry_at = None;
        self.save_at = None;
        self.watch = None;
        self.watch_active = false;
        self.watch_start_until = None;
        self.watch_job = self.next();
        self.replay = None;
        self.batches.clear();
        self.supplement = None;
        self.unique = None;
        self.complete = false;
        self.failure = None;
        self.cache_loaded = false;
        self.cache_identity = None;
        self.loaded_blocks.clear();
        self.pending.clear();
        self.pending_full = false;
        self.refresh_at = None;
        self.excluded.clear();
        self.queries.clear();
        self.event = None;
        self.saved = None;
        self.report["idle_released"] = json!(true);
        self.status();
    }
    fn tick(&mut self) {
        let now = Instant::now();
        if let Some(probe) = &mut self.alias_probe
            && probe.deadline().is_some_and(|until| until <= now)
        {
            probe.state = AliasState::Unavailable(
                "macOS did not resolve the search folder within two seconds".into(),
            );
            self.status();
        }
        if self.watch_start_until.is_some_and(|at| at <= now) {
            self.watch_started(false);
        }
        if self.can_scan() && self.retry_at.is_some_and(|at| at <= now) {
            self.retry_at = None;
            self.pending.extend(self.retry.drain());
            self.refresh_at = Some(now);
        }
        if self.can_save() && self.save_at.is_some_and(|at| at <= now) {
            self.save_at = None;
            self.persist();
        }
        if !self.scoring && self.score_at.is_some_and(|at| at <= Instant::now()) {
            self.score();
        }
        if self.can_refresh_supplement()
            && self
                .supplement_refresh_at
                .is_some_and(|at| at <= Instant::now())
        {
            self.supplement_refresh_at = None;
            if let Some(old) = self.supplement.take() {
                old.cancel.store(true, Ordering::Release);
                let changes = if old.pending_full || !old.complete {
                    None
                } else {
                    Some(old.pending)
                };
                self.start_supplement(old.path, old.batches, changes, Some(old.watch));
            }
        }
        if self
            .idle_at
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(60))
        {
            self.release();
        }
        if self
            .replay
            .as_ref()
            .is_some_and(|r| r.start.elapsed() >= Duration::from_secs(30))
        {
            self.fallback("history took too long");
        }
        if self.can_scan() && self.refresh_at.is_some_and(|at| at <= Instant::now()) {
            self.refresh_at = None;
            if self.pending_full {
                self.begin_scan(None);
            } else if !self.pending.is_empty() {
                let changes = std::mem::take(&mut self.pending);
                self.begin_scan(Some(changes));
            }
        }
    }
}
fn same_batches(a: &[Arc<Snapshot>], b: &[Arc<Snapshot>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| Arc::ptr_eq(a, b))
}

fn physical_footprint() -> Option<u64> {
    // Physical footprint includes compressed memory, unlike RSS.
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    let result = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as i32,
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast(),
        )
    };
    if result == 0 {
        Some(unsafe { info.assume_init() }.ri_phys_footprint)
    } else {
        None
    }
}
