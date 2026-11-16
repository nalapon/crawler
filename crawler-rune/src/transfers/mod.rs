//! Copies and AirDrop share the Transfers section. Workers publish owned snapshots;
//! the UI commits only when a value it displays changes.
mod copy;
mod discovery;
mod network;
mod picker;
mod system;

use crate::{
    app::{App, Paint, Section},
    shell,
};
use copy::{Outcome, Progress, Snapshot};
use rune::tok::{motion, type_};
use rune::{
    Cx, El, Icon, Key, KeyEvent, ScrollSpec, Transition, Tx, Waker, col, el, icon, row, text,
};
use std::{
    borrow::Cow,
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

static AIRDROP: Icon = Icon {
    name: "crawler-airdrop",
    view: 32.0,
    body: r#"<g fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="M7 23a12 12 0 1 1 18 0M11 19a7 7 0 1 1 10 0"/><path d="m16 17-6 11h12Z"/></g>"#,
};
static PHONE: Icon = Icon {
    name: "crawler-phone",
    view: 32.0,
    body: r#"<g fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><rect x="8" y="2" width="16" height="28" rx="3"/><path d="M13 5h6M14 27h4"/></g>"#,
};

struct Job {
    sources: Vec<PathBuf>,
    destination: PathBuf,
    id: u64,
}
impl Job {
    fn items(&self) -> String {
        if self.sources.len() == 1 {
            name(&self.sources[0])
        } else {
            format!("{} items", self.sources.len())
        }
    }
    fn place(&self) -> String {
        name(&self.destination)
    }
}
struct Running {
    job: Job,
    progress: Arc<Progress>,
    snapshot: Snapshot,
    done: mpsc::Receiver<copy::Finished>,
}
struct Record {
    id: String,
    title: String,
    detail: String,
    outcome: Outcome,
    time: String,
}
struct Sending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<network::TransferUpdate>,
}
impl Drop for Sending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
#[derive(Default)]
pub struct State {
    waker: Option<Waker>,
    next_id: u64,
    running: Option<Running>,
    queue: VecDeque<Job>,
    records: Vec<Record>,
    discovery: Option<discovery::Discovery>,
    devices: Vec<discovery::Device>,
    discovery_error: Option<String>,
    files: Vec<PathBuf>,
    selected: Option<String>,
    message: Option<String>,
    error: bool,
    delivered: bool,
    sending: Option<Sending>,
    upload: Option<(u64, u64)>,
    system: Option<system::SystemShare>,
    picker: picker::Picker,
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(copy) = &self.running {
            copy.progress.cancel();
        }
    }
}

pub fn setup(app: &mut App, waker: &Waker) {
    app.transfers.waker = Some(waker.clone());
    app.transfers.picker.setup(waker);
}
pub fn on_close(app: &mut App) {
    let s = &mut app.transfers;
    if let Some(copy) = &s.running {
        copy.progress.cancel();
    }
    s.queue.clear();
    s.discovery = None;
    s.sending = None;
    s.picker.close();
}
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let workspace_visible = app.reader == crate::app::Reader::None;
    let visible = app.section == Section::Transfers && workspace_visible;
    let s = &mut app.transfers;
    let mut changed = false;
    if visible && s.discovery.is_none() {
        if let Some(waker) = &s.waker {
            s.discovery = Some(discovery::Discovery::start(waker.clone()));
        }
    } else if !visible && s.discovery.is_some() {
        s.discovery = None;
        s.devices.clear();
        if s.sending.is_none() {
            s.selected = None;
        }
    }
    if let Some(d) = &s.discovery {
        for update in d.rx.try_iter() {
            match update {
                discovery::Update::Device(device) => {
                    if let Some(old) = s.devices.iter_mut().find(|d| d.id == device.id) {
                        if *old != device {
                            *old = device;
                            changed |= visible;
                        }
                    } else {
                        s.devices.push(device);
                        changed |= visible;
                    }
                }
                discovery::Update::Identified(device) => {
                    if let Some(old) = s.devices.iter_mut().find(|d| d.id == device.id) {
                        if *old != device {
                            *old = device;
                            changed |= visible;
                        }
                    }
                }
                discovery::Update::Removed(id) => {
                    let count = s.devices.len();
                    s.devices.retain(|d| d.id != id);
                    if s.selected.as_deref() == Some(&id) && s.sending.is_none() {
                        s.selected = None;
                    }
                    changed |= visible && count != s.devices.len();
                }
                discovery::Update::Failure(error) => {
                    if s.discovery_error.as_ref() != Some(&error) {
                        s.discovery_error = Some(error);
                        changed |= visible;
                    }
                }
            }
        }
    }
    let mut finished = None;
    let mut copy_notice = None;
    if let Some(running) = &mut s.running {
        let snapshot = running.progress.snapshot();
        if snapshot != running.snapshot {
            running.snapshot = snapshot;
            changed |= workspace_visible;
        }
        finished = match running.done.try_recv() {
            Ok(done) => Some(done),
            Err(mpsc::TryRecvError::Disconnected) => Some(copy::Finished {
                outcome: Outcome::Failed,
                detail: "The copy worker stopped unexpectedly.".into(),
                copied: running.snapshot.copied,
            }),
            Err(mpsc::TryRecvError::Empty) => None,
        };
    }
    if let Some(done) = finished {
        if let Some(running) = s.running.take() {
            let title = match done.outcome {
                Outcome::Copied => {
                    format!("Copied {} to {}", running.job.items(), running.job.place())
                }
                Outcome::Cancelled => format!(
                    "Copy of {} to {} cancelled",
                    running.job.items(),
                    running.job.place()
                ),
                Outcome::Failed => format!(
                    "Could not copy all of {} to {}",
                    running.job.items(),
                    running.job.place()
                ),
            };
            copy_notice = Some(if done.outcome == Outcome::Failed {
                done.detail.clone()
            } else {
                title.clone()
            });
            s.records.insert(
                0,
                Record {
                    id: format!("copy-{}", running.job.id),
                    title,
                    detail: if done.detail.is_empty() {
                        bytes(done.copied)
                    } else {
                        done.detail
                    },
                    outcome: done.outcome,
                    time: time_label(),
                },
            );
            changed |= workspace_visible;
        }
    }
    if s.running.is_none() && !s.queue.is_empty() {
        begin_next(s);
        changed |= workspace_visible;
    }
    let mut send_finished = None;
    if let Some(sending) = &s.sending {
        for update in sending.rx.try_iter() {
            let (message, upload) = match update {
                network::TransferUpdate::Preparing => ("Preparing files…".to_string(), s.upload),
                network::TransferUpdate::Connecting => ("Connecting…".into(), s.upload),
                network::TransferUpdate::Asking => {
                    ("Waiting for the receiving device…".into(), s.upload)
                }
                network::TransferUpdate::Uploading { sent, total } => {
                    ("Sending…".into(), Some((sent, total)))
                }
                network::TransferUpdate::AwaitingAcknowledgment => {
                    ("Waiting for delivery confirmation…".into(), s.upload)
                }
                network::TransferUpdate::Finished(result) => {
                    send_finished = Some(result);
                    continue;
                }
            };
            if s.message.as_ref() != Some(&message) || s.upload != upload {
                s.message = Some(message);
                s.upload = upload;
                changed |= visible;
            }
        }
    }
    if let Some(result) = send_finished {
        s.sending = None;
        match result {
            Ok(()) => {
                s.delivered = true;
                s.message = Some("Sent. The receiving device acknowledged the transfer.".into());
                s.error = false;
            }
            Err(error) => {
                s.error = error != "Transfer cancelled";
                s.message = Some(error);
                s.upload = None;
            }
        }
        changed |= visible;
    }
    let system_done = s.system.as_ref().and_then(|share| share.rx.try_recv().ok());
    if let Some(result) = system_done {
        s.system = None;
        match result {
            Ok(()) => {
                s.message = Some(
                    "AirDrop window closed. Check the receiving device to confirm delivery.".into(),
                );
                s.error = false;
            }
            Err(error) => {
                s.message = Some(error);
                s.error = true;
            }
        }
        changed |= visible;
    }
    changed |= s.picker.poll();
    if let Some(notice) = copy_notice {
        app.toast(notice);
        changed = true;
    }
    if changed {
        tx.animated();
        tx.motion(motion::SNAPPY);
    }
    changed
}
fn begin_next(s: &mut State) {
    let Some(waker) = s.waker.clone() else {
        return;
    };
    let Some(job) = s.queue.pop_front() else {
        return;
    };
    let progress = Progress::new(waker.clone());
    let worker_progress = progress.clone();
    let sources = job.sources.clone();
    let destination = job.destination.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = copy::run(sources, destination, &worker_progress);
        let _ = tx.send(result);
        waker();
    });
    s.running = Some(Running {
        job,
        progress,
        snapshot: Snapshot::default(),
        done: rx,
    });
}
/// Copies into `dest` without replacing names. Jobs queue behind the running copy.
pub fn start_copy(app: &mut App, sources: Vec<PathBuf>, dest: PathBuf, _tx: &mut Tx) {
    if sources.is_empty() {
        return;
    }
    let s = &mut app.transfers;
    s.next_id += 1;
    s.queue.push_back(Job {
        sources,
        destination: dest,
        id: s.next_id,
    });
    if s.running.is_none() {
        begin_next(s);
    }
}
/// Stages files for AirDrop and shows Transfers. Disk reads start only when sending.
pub fn airdrop(app: &mut App, files: Vec<PathBuf>, _tx: &mut Tx) {
    app.show(Section::Transfers);
    let s = &mut app.transfers;
    if busy(s) {
        return;
    }
    for path in files {
        if !s.files.contains(&path) {
            s.files.push(path);
        }
    }
    s.message = None;
    s.error = false;
    s.delivered = false;
    s.upload = None;
}
fn busy(s: &State) -> bool {
    s.sending.is_some() || s.system.is_some()
}
fn cancel(app: &mut App) {
    if let Some(copy) = &app.transfers.running {
        copy.progress.cancel();
    }
}
fn refresh(app: &mut App) {
    let s = &mut app.transfers;
    if busy(s) {
        return;
    }
    s.discovery = None;
    s.devices.clear();
    s.selected = None;
    s.discovery_error = None;
    s.message = None;
    s.error = false;
}
fn send(app: &mut App) {
    let s = &mut app.transfers;
    if busy(s) || s.files.is_empty() {
        return;
    }
    let Some(device) = s
        .devices
        .iter()
        .find(|d| Some(&d.id) == s.selected.as_ref() && d.ready)
        .cloned()
    else {
        return;
    };
    let Some(waker) = s.waker.clone() else {
        return;
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    let files = s.files.clone();
    let (tx, rx) = mpsc::channel();
    s.message = Some("Preparing files…".into());
    s.error = false;
    s.delivered = false;
    s.upload = None;
    std::thread::spawn(move || {
        let mut last = Instant::now() - Duration::from_secs(1);
        network::send(device.endpoint, files, &stop, |update| {
            let progress = matches!(update, network::TransferUpdate::Uploading { sent, total } if sent < total);
            if !progress || last.elapsed() >= Duration::from_millis(80) {
                last = Instant::now();
                let _ = tx.send(update);
                waker();
            }
        });
    });
    s.sending = Some(Sending { cancel, rx });
}
fn system_share(app: &mut App) {
    let s = &mut app.transfers;
    if busy(s) || s.files.is_empty() {
        return;
    }
    let Some(waker) = s.waker.clone() else {
        return;
    };
    s.delivered = false;
    s.upload = None;
    match system::share(&s.files, waker) {
        Ok(share) => {
            s.system = Some(share);
            s.message = Some("Follow the transfer in the macOS AirDrop window.".into());
            s.error = false;
        }
        Err(e) => {
            s.message = Some(e);
            s.error = true;
        }
    }
}
pub fn on_key(app: &mut App, ev: &KeyEvent, _tx: &mut Tx) -> bool {
    if app.transfers.picker.open {
        return false;
    }
    if ev.key == Key::Command('r') {
        refresh(app);
        return true;
    }
    if ev.key == Key::Escape {
        if let Some(send) = &app.transfers.sending {
            send.cancel.store(true, Ordering::Relaxed);
            app.transfers.message = Some("Cancelling…".into());
            return true;
        }
    }
    false
}
fn name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
pub(super) fn bytes(n: u64) -> String {
    if n < 1000 {
        format!("{n} bytes")
    } else {
        let mut value = n as f64;
        let mut unit = "bytes";
        for next in ["KB", "MB", "GB", "TB"] {
            value /= 1000.0;
            unit = next;
            if value < 1000.0 {
                break;
            }
        }
        format!("{value:.1} {unit}")
    }
}
fn time_label() -> String {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t;
    let mut time = std::mem::MaybeUninit::<libc::tm>::uninit();
    if unsafe { libc::localtime_r(&seconds, time.as_mut_ptr()) }.is_null() {
        return String::new();
    }
    let time = unsafe { time.assume_init() };
    format!("{:02}:{:02}", time.tm_hour, time.tm_min)
}
fn progress(p: &Paint, key: &str, copied: u64, total: u64) -> El {
    let fraction = if total == 0 {
        0.0
    } else {
        (copied as f64 / total as f64).clamp(0.0, 1.0) as f32
    };
    el().key(key.to_string())
        .role("progressbar")
        .range_value(fraction as f64, 0.0, 1.0)
        .label(format!("{} of {}", bytes(copied), bytes(total)))
        .h(6.0)
        .w_full()
        .radius(3.0)
        .fill(p.look.line)
        .child(
            el().key("fill")
                .h_full()
                .w_frac(fraction)
                .radius(3.0)
                .fill(p.look.accent)
                .motion(motion::SNAPPY),
        )
}
pub fn status(app: &App, p: &Paint) -> Option<El> {
    let running = app.transfers.running.as_ref()?;
    let snap = &running.snapshot;
    let message = if snap.total == 0 {
        format!("Preparing copy to {}", running.job.place())
    } else {
        format!(
            "Copy to {} · {} of {}",
            running.job.place(),
            bytes(snap.copied.min(snap.total)),
            bytes(snap.total)
        )
    };
    Some(
        row()
            .key("copy-status")
            .items_center()
            .gap(8.0)
            .child(
                row()
                    .key("copy-status-open")
                    .role("button")
                    .label("Show transfers")
                    .items_center()
                    .gap(8.0)
                    .on_press(|a: &mut App| a.show(Section::Transfers))
                    .child(
                        text(message)
                            .font(type_::LABEL)
                            .color(p.look.text)
                            .truncate(),
                    )
                    .child(el().w(84.0).child(progress(
                        p,
                        "copy-status-progress",
                        snap.copied,
                        snap.total,
                    ))),
            )
            .child(
                shell::shell_button(p, "copy-status-cancel", "Cancel", false)
                    .h(20.0)
                    .on_press(cancel),
            ),
    )
}
fn copies(app: &App, p: &Paint) -> El {
    let s = &app.transfers;
    let title = if s.queue.is_empty() {
        "One copy runs at a time".into()
    } else {
        format!("{} queued", s.queue.len())
    };
    let mut pane = col()
        .key("copies")
        .w_frac(0.36)
        .min_w(280.0)
        .h_full()
        .border_sides([0.0, 0.0, 0.0, 1.0], p.look.line)
        .child(shell::pane_title(
            p,
            "copies-title",
            "Copies",
            Some(&title),
            false,
        ));
    if let Some(running) = &s.running {
        let snap = &running.snapshot;
        pane = pane.child(
            col()
                .key(format!("running-copy-{}", running.job.id))
                .matched(format!("copy-{}", running.job.id))
                .padding(16.0)
                .gap(8.0)
                .child(
                    text(if snap.name.is_empty() {
                        format!("Preparing copy to {}", running.job.place())
                    } else {
                        format!("Copying {} to {}", snap.name, running.job.place())
                    })
                    .font(type_::LABEL)
                    .color(p.look.text)
                    .truncate(),
                )
                .child(
                    text(format!(
                        "{} of {}",
                        bytes(snap.copied.min(snap.total)),
                        bytes(snap.total)
                    ))
                    .font(type_::LABEL)
                    .color(p.look.muted),
                )
                .child(progress(p, "copy-progress", snap.copied, snap.total))
                .child(
                    row()
                        .justify_between()
                        .items_center()
                        .child(
                            text("Existing names are skipped.")
                                .font(type_::LABEL)
                                .color(p.look.muted),
                        )
                        .child(
                            shell::shell_button(p, "copy-cancel", "Cancel", false).on_press(cancel),
                        ),
                ),
        );
    }
    if s.running.is_none() && s.records.is_empty() {
        pane = pane.child(col().padding(18.0).gap(8.0).child(text("No copies yet").font(type_::LABEL).color(p.look.text)).child(text("Drag files from Search or Folders onto a place in Folders to copy them there.").font(type_::BODY).color(p.look.muted).wrap()));
    } else {
        let look = p.look;
        pane = pane.child(
            col()
                .key("copy-record-scroll")
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .grow(1.0)
                .min_h(0.0)
                .child(
                    col()
                        .key("copy-records")
                        .growth(rune::GrowthSpec::default())
                        .virtual_rows(
                            s.records.len(),
                            82.0,
                            |a: &App, i| Cow::Borrowed(a.transfers.records[i].id.as_str()),
                            move |a: &App, i| {
                                let record = &a.transfers.records[i];
                                col()
                                    .key(record.id.clone())
                                    .matched(record.id.clone())
                                    .w_full()
                                    .min_h(82.0)
                                    .px(16.0)
                                    .py(12.0)
                                    .gap(5.0)
                                    .border_sides([0.0, 0.0, 1.0, 0.0], look.line)
                                    .transition(
                                        Transition::opacity()
                                            .combined(Transition::offset(0.0, -8.0)),
                                    )
                                    .motion(motion::SNAPPY)
                                    .child(
                                        text(record.title.as_str())
                                            .font(type_::LABEL)
                                            .color(if record.outcome == Outcome::Failed {
                                                crate::theme::rgb(0xc53c3c)
                                            } else {
                                                look.text
                                            })
                                            .wrap(),
                                    )
                                    .child(
                                        text(format!("{} · {}", record.time, record.detail))
                                            .font(type_::LABEL)
                                            .color(look.muted)
                                            .max_lines(2)
                                            .help(record.detail.clone()),
                                    )
                            },
                        ),
                ),
        );
    }
    pane
}
fn beacon(s: &State, p: &Paint) -> El {
    let active = s.sending.is_some() || s.devices.is_empty() && s.discovery_error.is_none();
    let mut body = el().key("airdrop-beacon-waves").size(100.0, 100.0).center();
    for (index, diameter) in [96.0, 74.0, 52.0].into_iter().enumerate() {
        let wave = el()
            .key(format!("wave-{index}"))
            .absolute((100.0 - diameter) / 2.0, (100.0 - diameter) / 2.0)
            .size(diameter, diameter)
            .radius(diameter / 2.0)
            .border(1.0, p.look.accent.alpha(0.35))
            .inert();
        body = body.child(if active {
            wave.looping(rune::LoopSpec::opacity(2.4, index as f64 * 0.25, 0.15, 1.0))
        } else {
            wave
        });
    }
    col()
        .key("airdrop-beacon")
        .h(112.0)
        .w_full()
        .center()
        .child(
            body.child(
                icon(
                    if s.delivered {
                        p.kit.glyph.check
                    } else {
                        &AIRDROP
                    },
                    32.0,
                )
                .color(p.look.accent),
            ),
        )
}
fn air_view(app: &App, p: &Paint) -> El {
    let s = &app.transfers;
    let busy = busy(s);
    let status = s
        .message
        .as_deref()
        .or(s.discovery_error.as_deref())
        .unwrap_or(if s.devices.is_empty() {
            "Looking for nearby devices…"
        } else {
            "Choose a device"
        });
    let mut devices = row()
        .key("airdrop-devices")
        .wrap()
        .gap(8.0)
        .justify_center();
    for device in &s.devices {
        let id = device.id.clone();
        let drop_id = id.clone();
        let ready = device.ready;
        let tile = col()
            .key(format!("airdrop-device-{}", device.id))
            .w(158.0)
            .min_h(108.0)
            .padding(12.0)
            .gap(8.0)
            .items_center()
            .radius(p.kit.radius.sm)
            .fill(if s.selected.as_ref() == Some(&device.id) {
                p.look.selection
            } else {
                p.look.raised
            })
            .border(1.0, p.look.line)
            .role("button")
            .label(format!(
                "{}, {}. Choose files to send.",
                device.name, device.detail
            ))
            .transition(Transition::opacity().combined(Transition::offset(0.0, 8.0)))
            .motion(motion::SNAPPY)
            .child(
                icon(
                    if device.name.to_lowercase().contains("iphone")
                        || device.name.to_lowercase().contains("ipad")
                    {
                        &PHONE
                    } else {
                        &AIRDROP
                    },
                    34.0,
                )
                .color(p.look.accent),
            )
            .child(
                text(device.name.clone())
                    .font(type_::LABEL)
                    .color(p.look.text)
                    .truncate(),
            )
            .child(
                text(device.detail.clone())
                    .font(type_::LABEL)
                    .color(p.look.muted)
                    .max_lines(2),
            );
        let tile = if ready && !busy {
            p.kit
                .interactive(tile)
                .press_scale(0.98)
                .on_press(move |a: &mut App| {
                    a.transfers.selected = Some(id.clone());
                    a.transfers.message = None;
                    if a.transfers.files.is_empty() {
                        a.transfers.picker.show(true);
                    }
                })
                .on_drop_files(move |a: &mut App, paths, tx| {
                    a.transfers.selected = Some(drop_id.clone());
                    a.transfers.files.clear();
                    airdrop(a, paths.to_vec(), tx);
                    send(a);
                })
        } else {
            tile
        };
        devices = devices.child(tile);
    }
    let files_label = match s.files.as_slice() {
        [] => "No files selected".into(),
        [file] => name(file),
        _ => format!("{} items ready to send", s.files.len()),
    };
    let mut actions = row()
        .key("airdrop-actions")
        .gap(8.0)
        .justify_center()
        .wrap();
    if busy {
        if s.sending.is_some() {
            actions = actions.child(
                shell::shell_button(p, "airdrop-cancel", "Cancel transfer", false).on_press(
                    |a: &mut App| {
                        if let Some(sending) = &a.transfers.sending {
                            sending.cancel.store(true, Ordering::Relaxed);
                            a.transfers.message = Some("Cancelling…".into());
                        }
                    },
                ),
            );
        }
    } else {
        actions = actions.child(
            shell::shell_button(p, "airdrop-choose", "Choose files…", false)
                .on_press(|a: &mut App| a.transfers.picker.show(false)),
        );
        if !s.files.is_empty() {
            if s.devices
                .iter()
                .any(|d| d.ready && Some(&d.id) == s.selected.as_ref())
            {
                actions = actions
                    .child(shell::shell_button(p, "airdrop-send", "Send", true).on_press(send));
            }
            actions = actions.child(
                shell::shell_button(p, "airdrop-clear", "Clear files", false).on_press(
                    |a: &mut App| {
                        a.transfers.files.clear();
                        a.transfers.error = false;
                        a.transfers.message = None;
                        a.transfers.upload = None;
                        a.transfers.delivered = false;
                    },
                ),
            );
        }
    }
    let mut content = col()
        .key("airdrop-content")
        .padding(30.0)
        .gap(18.0)
        .w_full()
        .child(
            row()
                .justify_between()
                .items_center()
                .child(text("AirDrop").font(type_::TITLE).color(p.look.text))
                .child(
                    shell::shell_button(p, "airdrop-refresh", "Refresh devices", false)
                        .on_press(refresh),
                ),
        )
        .child(
            text("Drop files onto a device, or click it to choose what to send.")
                .font(type_::BODY)
                .color(p.look.muted)
                .wrap(),
        )
        .child(beacon(s, p))
        .child(
            text(status)
                .key("airdrop-status")
                .font(type_::TITLE)
                .color(if s.error {
                    crate::theme::rgb(0xc53c3c)
                } else {
                    p.look.text
                })
                .wrap(),
        )
        .child(
            text(if s.discovery_error.is_some() {
                "Network discovery is unavailable. You can still use the macOS AirDrop window."
            } else if s.devices.is_empty() {
                "Your iPad, iPhone or Mac will appear here when discovered."
            } else {
                "Click a device to select files, or drop files directly onto it."
            })
            .font(type_::BODY)
            .color(p.look.muted)
            .wrap(),
        )
        .child(devices)
        .child(
            text(files_label)
                .key("airdrop-files")
                .font(type_::LABEL)
                .color(p.look.text)
                .help(
                    s.files
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
        );
    if let Some((sent, total)) = s.upload {
        content = content.child(
            row()
                .items_center()
                .gap(8.0)
                .child(progress(p, "airdrop-progress", sent, total).grow(1.0))
                .child(
                    text(format!("{}%", sent.saturating_mul(100) / total.max(1)))
                        .font(type_::LABEL)
                        .color(p.look.muted),
                ),
        );
    }
    content = content.child(actions)
        .child(text("Keep the receiving device unlocked, with Wi-Fi and Bluetooth on. Use AirDrop's Everyone for 10 Minutes setting. Custom sending requires macOS 15 or later.").font(type_::LABEL).color(p.look.muted).wrap());
    if !busy && !s.files.is_empty() {
        content = content.child(
            shell::shell_button(p, "airdrop-system", "Use macOS AirDrop…", false)
                .on_press(system_share),
        );
    }
    let pane = col()
        .key("airdrop")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .child(shell::pane_title(
            p,
            "airdrop-pane-title",
            "AirDrop",
            None,
            true,
        ))
        .child(
            col()
                .key("airdrop-scroll")
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .grow(1.0)
                .min_h(0.0)
                .child(content),
        );
    if busy {
        pane
    } else {
        pane.on_drop_files(|a: &mut App, files, tx| airdrop(a, files.to_vec(), tx))
    }
}
pub fn view(app: &App, cx: &mut Cx<App>, p: &Paint, layer: &mut Vec<El>) -> El {
    picker::overlay(app, cx, p, layer);
    row()
        .key("transfers")
        .w_full()
        .h_full()
        .fill(p.look.background)
        .child(air_view(app, p))
        .child(copies(app, p))
}
