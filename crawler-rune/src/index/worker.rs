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
    Events {
        job: u64,
        events: Events,
    },
    SupplementEvents {
        job: u64,
        events: Events,
    },
    Scored {
        epoch: u64,
        response: Response,
        final_pass: bool,
        fuzzy: bool,
    },
    Saved {
        event: Option<Event>,
        milliseconds: f64,
        error: Option<String>,
    },
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
    _watch: Watcher,
    pending: HashSet<String>,
    pending_full: bool,
    failure: Option<String>,
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
    pool: rayon::ThreadPool,
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
    scan: Option<ScanJob>,
    supplement: Option<Supplement>,
    watch: Option<Watcher>,
    watch_job: u64,
    watch_active: bool,
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
    queries: VecDeque<(Key, Response, bool)>,
    report: Value,
    samples: VecDeque<Value>,
    save_cancel: Arc<AtomicBool>,
    save_lock: Arc<Mutex<()>>,
    supplement_refresh_at: Option<Instant>,
}
pub(super) fn run(
    workers: usize,
    root: PathBuf,
    support: PathBuf,
    sender: Sender<Message>,
    receive: Receiver<Message>,
    shared: Arc<Mutex<Shared>>,
    generation: Arc<AtomicU64>,
) {
    let pool = match rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|n| format!("crawler-match-{n}"))
        .build()
    {
        Ok(pool) => pool,
        Err(error) => {
            if let Ok(mut state) = shared.lock() {
                state.status = Status::Failed(error.to_string());
            }
            return;
        }
    };
    let mut worker = Worker {
        physical_root: fs::canonicalize(&root).unwrap_or_else(|_| root.clone()),
        root,
        cache_file: support.join("mac-paths.fff4.bin"),
        history: History::new(),
        support,
        sender,
        shared,
        generation,
        epoch: Arc::new(AtomicU64::new(0)),
        pool,
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
        scan: None,
        supplement: None,
        watch: None,
        watch_job: 0,
        watch_active: false,
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
        queries: VecDeque::new(),
        samples: VecDeque::new(),
        report: json!({"workers": workers, "engine_revision": snapshot::REVISION, "cache_loaded": false, "query_cache_hits": 0, "engine_passes": 0}),
        save_cancel: Arc::new(AtomicBool::new(false)),
        save_lock: Arc::new(Mutex::new(())),
        supplement_refresh_at: None,
    };
    let mut history_loaded = false;
    loop {
        let timeout =
            worker
                .score_at
                .filter(|_| !worker.scoring)
                .map_or(Duration::from_millis(50), |at| {
                    at.saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(50))
                });
        match receive.recv_timeout(timeout) {
            Ok(Message::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                worker.release();
                break;
            }
            Ok(message) => {
                if !history_loaded
                    && matches!(&message, Message::Search { query, .. } if !query.text.trim().is_empty())
                {
                    worker.history = cache::history(&worker.support.join("access-history.json"));
                    history_loaded = true;
                }
                worker.message(message);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        worker.tick();
    }
}
impl Worker {
    fn next(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }
    fn notify(&mut self, status: Option<Status>, response: Option<Response>) {
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
        self.report["query_cache_entries"] = json!(self.queries.len());
        self.report["physical_footprint_bytes"] = json!(physical_footprint());
        self.report["cache_loaded"] = json!(self.cache_loaded);
        self.report["indexed_event_id"] = json!(self.event.map(|event| event.id));
        self.report["latest_event_id"] = json!(self.latest_event);
        self.report["stages"] = json!(self.samples);
        let waker = if let Ok(mut state) = self.shared.lock() {
            let changed = status.as_ref().is_some_and(|s| s != &state.status)
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
            state.report = self.report.clone();
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
            && self.supplement.as_ref().is_none_or(|s| s.complete)
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
            } => {
                if self.generation.load(Ordering::Acquire) != id {
                    return;
                }
                query.text = snapshot::folded(query.text.trim());
                if query.text.is_empty() {
                    self.pause();
                    return;
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
                self.samples.push_back(json!({"sequence": id, "input_dispatch_ms": started.elapsed().as_secs_f64() * 1000.0}));
                while self.samples.len() > 32 {
                    self.samples.pop_front();
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
                } else if was_idle && !self.pending.is_empty() {
                    self.refresh_at = Some(Instant::now());
                } else if !self.complete && self.scan.is_none() && !self.loading {
                    self.begin_scan(None);
                }
                if let Some(position) = self
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
                        | "input_dispatch_ms"
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
                    self.notify(None, None);
                }
            }
            Message::WarmBatch { job, batch }
                if self.active.is_some()
                    && job == self.cache_job
                    && !self.complete
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
                self.cache_loaded = true;
                self.unique = None;
                self.report["cache_load_ms"] = json!(milliseconds);
                if let Some(warning) = warning {
                    self.report["cache_error"] = json!(warning);
                }
                self.dirty = true;
                if event != self.cache_expected || (!valid && self.cache_expected.is_some()) {
                    self.fallback("cache changed");
                } else if self.replay.is_none() {
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
                Progress::Batch(batch) => self.append(batch),
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
                            self.event = scan.through;
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
                                self.persist();
                            }
                            if changed
                                || !self.active.as_ref().is_some_and(|a| {
                                    self.queries.iter().any(|(key, _, _)| key == &a.key)
                                })
                            {
                                self.restart();
                            }
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
                let prefix = supplement
                    .path
                    .strip_prefix(&self.root)
                    .ok()
                    .and_then(Path::to_str)
                    .unwrap_or("")
                    .to_owned();
                let rebase = |batch: Arc<Snapshot>| {
                    Arc::new(Snapshot::build(
                        &(0..batch.len())
                            .map(|r| {
                                (
                                    format!("{prefix}/{}", batch.path(r))
                                        .trim_start_matches('/')
                                        .to_owned(),
                                    batch.directory(r),
                                )
                            })
                            .collect::<Vec<_>>(),
                    ))
                };
                match progress {
                    Progress::Batch(batch) => {
                        supplement.batches.push(rebase(batch));
                        self.invalidate();
                        if !self.scoring {
                            self.score();
                        }
                    }
                    Progress::Finished(result) => {
                        supplement.complete = true;
                        match result {
                            Ok(result) => {
                                let batches: Vec<_> =
                                    result.batches.into_iter().map(rebase).collect();
                                if same_entries(&supplement.batches, &batches) {
                                    self.status();
                                    if !self.active.as_ref().is_some_and(|a| {
                                        self.queries.iter().any(|(key, _, _)| key == &a.key)
                                    }) {
                                        if self.scoring {
                                            self.dirty = true;
                                        } else {
                                            self.score();
                                        }
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
            Message::Events { job, events } if job == self.watch_job => self.events(events),
            Message::SupplementEvents { job, events }
                if self.supplement.as_ref().is_some_and(|s| s.job == job) =>
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
            Message::Scored {
                epoch,
                response,
                final_pass,
                fuzzy,
            } if epoch == self.epoch.load(Ordering::Acquire)
                && self.active.as_ref().is_some_and(|a| a.id == response.id) =>
            {
                if final_pass {
                    self.scoring = false;
                }
                if final_pass && self.dirty {
                    self.schedule_index_score();
                    return;
                }
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
                event,
                milliseconds,
                error,
            } => {
                self.report["cache_save_ms"] = json!(milliseconds);
                self.report["cache_save_error"] = json!(error);
                if error.is_none() {
                    self.saved = event;
                    self.cache_legacy = false;
                }
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
        let _ = std::thread::Builder::new()
            .name("crawler-warm-paths".into())
            .spawn(move || {
                discovery::warm(&root, &folders, &token, |batch| {
                    let _ = tx.send(Message::WarmBatch { job, batch });
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
            if let Some(active) = &self.active {
                reader.prioritize(&active.key.query.text);
            }
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
        if let Some(saved) = saved.filter(|_| self.watch_active) {
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
                if let Ok(mut reader) = reader {
                    event = reader.event;
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        match reader.next() {
                            Ok(Some((position, batch))) => {
                                if tx
                                    .send(Message::CacheBatch {
                                        job,
                                        position,
                                        batch,
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            Ok(None) => {
                                valid = true;
                                break;
                            }
                            Err(error) => {
                                warning = Some(error.to_string());
                                break;
                            }
                        }
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
        let (watch, ready) = Watcher::start(self.root.clone(), since, move |events| {
            let _ = tx.send(Message::Events { job, events });
        });
        self.watch_active = ready.recv_timeout(Duration::from_secs(2)).unwrap_or(false);
        self.watch = self.watch_active.then_some(watch);
    }
    fn events(&mut self, events: Events) {
        self.latest_event = self.latest_event.max(events.last_id);
        if events.full_scan {
            self.pending_full = true;
        }
        for path in events.paths {
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
        if self.active.is_none() || self.scan.is_some() || self.loading || self.replay.is_some() {
            return;
        }
        if changes.as_ref().is_some_and(HashSet::is_empty) {
            self.complete = true;
            if self.watch_active && self.latest_event > self.event.map_or(0, |e| e.id) {
                self.event = Some(Event::now(self.latest_event));
            }
            if self.cache_legacy
                || self
                    .event
                    .is_some_and(|e| e.date - self.saved.map_or(0.0, |s| s.date) > 3600.0)
            {
                self.persist();
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
        let root = self.root.clone();
        let previous = self.batches.clone();
        let tx = self.sender.clone();
        let _ = std::thread::Builder::new()
            .name("crawler-discovery".into())
            .spawn(move || {
                discovery::run(root, previous, changes, cancel, move |progress| {
                    let _ = tx.send(Message::Scan {
                        job: id,
                        supplement: false,
                        progress,
                    });
                })
            });
        self.status();
    }
    fn ensure_supplement(&mut self) {
        let desired = self.active.as_ref().and_then(|a| {
            a.key
                .query
                .volume
                .clone()
                .or_else(|| match &a.key.query.scope {
                    Scope::Under(path)
                        if a.key.query.folder_mode && discovery::alias_scope(path) =>
                    {
                        Some(path.clone())
                    }
                    _ => None,
                })
        });
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
        let previous = if let Some(old) = self.supplement.take() {
            old.cancel.store(true, Ordering::Release);
            if Some(&old.path) == desired.as_ref() {
                old.batches
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        self.invalidate();
        let Some(path) = desired.filter(|p| p.starts_with(&self.root) && p != &self.root) else {
            return;
        };
        self.start_supplement(path, previous, None);
    }
    fn start_supplement(
        &mut self,
        path: PathBuf,
        previous: Vec<Arc<Snapshot>>,
        changes: Option<HashSet<String>>,
    ) {
        let job = self.next();
        let cancel = Arc::new(AtomicBool::new(false));
        let event_tx = self.sender.clone();
        let (watch, _) = Watcher::start(path.clone(), None, move |events| {
            let _ = event_tx.send(Message::SupplementEvents { job, events });
        });
        self.supplement = Some(Supplement {
            path: path.clone(),
            batches: previous.clone(),
            complete: false,
            cancel: Arc::clone(&cancel),
            job,
            _watch: watch,
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
        let _ = std::thread::Builder::new()
            .name("crawler-supplement".into())
            .spawn(move || {
                let previous = previous
                    .iter()
                    .map(|batch| {
                        Arc::new(Snapshot::build(
                            &(0..batch.len())
                                .filter_map(|r| {
                                    batch.path(r).strip_prefix(&prefix).map(|path| {
                                        (
                                            path.trim_start_matches('/').to_owned(),
                                            batch.directory(r),
                                        )
                                    })
                                })
                                .collect::<Vec<_>>(),
                        ))
                    })
                    .collect();
                discovery::run(path, previous, changes, cancel, move |progress| {
                    let _ = tx.send(Message::Scan {
                        job,
                        supplement: true,
                        progress,
                    });
                })
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
        let query = active.key.query.clone();
        let boosts = matching::boosts(&active.key.boosts, &self.root);
        let batches = self.batches.clone();
        let supplemental = self
            .supplement
            .as_ref()
            .map(|s| (s.path.clone(), s.batches.clone()));
        let root = self.root.clone();
        let epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let token = Arc::clone(&self.epoch);
        let generation = Arc::clone(&self.generation);
        let tx = self.sender.clone();
        let complete = self.ready();
        let publish_early = !active.first_result;
        let fallback = self.cache_loaded && query.text.split_whitespace().count() > 1;
        let workers = self.workers;
        self.scoring = true;
        self.dirty = false;
        self.report["engine_passes"] =
            json!(self.report["engine_passes"].as_u64().unwrap_or(0) + 1);
        self.report["last_query_cache_hit"] = json!(false);
        self.pool.spawn(move || {
            let cancelled = || {
                token.load(Ordering::Acquire) != epoch || generation.load(Ordering::Acquire) != id
            };
            let mut all = batches;
            let split = all.len();
            if let Some((_, supplemental)) = &supplemental {
                all.extend(supplemental.iter().cloned());
            }
            let mut literal = true;
            loop {
                let mut matches = Matches::default();
                let mut first_nonempty = false;
                for (group_id, group) in all.chunks(workers).enumerate() {
                    if cancelled() {
                        return;
                    }
                    // Keep the Mac/supplement boundary: duplicate Mac coverage is excluded first.
                    let start = group_id * workers;
                    let mut part = Matches::default();
                    let mac_count = split.saturating_sub(start).min(group.len());
                    if mac_count > 0 {
                        part.merge(matching::group(
                            &group[..mac_count],
                            &query,
                            &root,
                            supplemental.as_ref().map(|s| s.0.as_path()),
                            &boosts,
                            literal,
                            &generation,
                            id,
                        ));
                    }
                    if mac_count < group.len() {
                        part.merge(matching::group(
                            &group[mac_count..],
                            &query,
                            &root,
                            None,
                            &boosts,
                            literal,
                            &generation,
                            id,
                        ));
                    }
                    matches.merge(part);
                    if cancelled() {
                        return;
                    }
                    if publish_early && !matches.hits.is_empty() && !first_nonempty {
                        first_nonempty = true;
                        let response = matches.response(&query, &root, id, false, literal);
                        let _ = tx.send(Message::Scored {
                            epoch,
                            response,
                            final_pass: false,
                            fuzzy: !literal,
                        });
                    }
                }
                if cancelled() {
                    return;
                }
                if matches.total == 0 && literal && fallback {
                    literal = false;
                    continue;
                }
                let response = matches.response(&query, &root, id, complete, literal);
                let _ = tx.send(Message::Scored {
                    epoch,
                    response,
                    final_pass: true,
                    fuzzy: !literal,
                });
                break;
            }
        });
    }
    fn persist(&mut self) {
        if !self.cache_loaded || self.loading || self.replay.is_some() {
            return;
        }
        self.save_cancel.store(true, Ordering::Release);
        self.save_cancel = Arc::new(AtomicBool::new(false));
        let token = Arc::clone(&self.save_cancel);
        let gate = Arc::clone(&self.save_lock);
        let batches = self.batches.clone();
        let event = self.event;
        let root = self.root.clone();
        let file = self.cache_file.clone();
        let tx = self.sender.clone();
        let _ = std::thread::Builder::new()
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
                        event,
                        milliseconds: at.elapsed().as_secs_f64() * 1000.0,
                        error,
                    });
                }
            });
    }
    fn pause(&mut self) {
        self.active = None;
        self.score_at = None;
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.scoring = false;
        if self.loading {
            self.cache_cancel.store(true, Ordering::Release);
            self.loading = false;
            self.cache_loaded = false;
        }
        if let Some(scan) = self.scan.take() {
            scan.cancel.store(true, Ordering::Release);
            self.complete = false;
            self.pending_full = true;
        }
        if let Some(supplement) = &self.supplement {
            supplement.cancel.store(true, Ordering::Release);
        }
        self.save_cancel.store(true, Ordering::Release);
        self.supplement_refresh_at = None;
        self.idle_at = Some(Instant::now());
        self.status();
    }
    fn release(&mut self) {
        self.pause();
        self.idle_at = None;
        self.watch = None;
        self.watch_active = false;
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
        self.excluded.clear();
        self.queries.clear();
        self.event = None;
        self.saved = None;
        self.report["idle_released"] = json!(true);
        self.status();
    }
    fn tick(&mut self) {
        if !self.scoring && self.score_at.is_some_and(|at| at <= Instant::now()) {
            self.score();
        }
        if self.active.is_some()
            && self
                .supplement_refresh_at
                .is_some_and(|at| at <= Instant::now())
        {
            self.supplement_refresh_at = None;
            if let Some(old) = self.supplement.take() {
                old.cancel.store(true, Ordering::Release);
                let changes = if old.pending_full {
                    None
                } else {
                    Some(old.pending)
                };
                self.start_supplement(old.path, old.batches, changes);
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
        if self.active.is_some()
            && !self.loading
            && self.scan.is_none()
            && self.replay.is_none()
            && self.refresh_at.is_some_and(|at| at <= Instant::now())
        {
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

fn same_entries(a: &[Arc<Snapshot>], b: &[Arc<Snapshot>]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.len() == b.len()
                && (0..a.len()).all(|r| {
                    a.directory(r) == b.directory(r) && snapshot::equal(a.path(r), b.path(r))
                })
        })
}
