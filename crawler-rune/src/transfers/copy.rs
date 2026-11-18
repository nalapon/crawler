//! One worker owns a copy job. Publication contains values, never filesystem handles.
use libc::{
    copyfile, copyfile_state_alloc, copyfile_state_free, copyfile_state_get, copyfile_state_set,
    renameatx_np,
};
use rune::Waker;
use std::{
    ffi::{CString, c_char, c_int, c_void},
    fs, io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub total: u64,
    pub copied: u64,
    pub name: String,
}

pub struct Progress {
    pub cancel: AtomicBool,
    value: Mutex<Snapshot>,
    base: Mutex<u64>,
    last_wake: Mutex<Instant>,
    waker: Waker,
}
impl Progress {
    pub fn new(waker: Waker) -> Arc<Self> {
        Arc::new(Self {
            cancel: AtomicBool::new(false),
            value: Mutex::new(Snapshot::default()),
            base: Mutex::new(0),
            last_wake: Mutex::new(Instant::now()),
            waker,
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        self.value.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    fn wake(&self, force: bool) {
        let mut last = self.last_wake.lock().unwrap_or_else(|e| e.into_inner());
        if force || last.elapsed().as_millis() >= 80 {
            *last = Instant::now();
            (self.waker)();
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Copied,
    Cancelled,
    Failed,
}
#[derive(Debug)]
pub struct Finished {
    pub outcome: Outcome,
    pub detail: String,
    pub copied: u64,
}

// copyfile.h. The state and callback context live until copyfile returns on this worker.
type CopyState = libc::copyfile_state_t;
fn cpath(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "A path contains a zero byte"))
}

unsafe extern "C" fn callback(
    what: c_int,
    stage: c_int,
    state: CopyState,
    source: *const c_char,
    _: *const c_char,
    context: *mut c_void,
) -> c_int {
    // SAFETY: run keeps this borrowed Progress alive for the entire synchronous copyfile call.
    let p = unsafe { &*(context as *const Progress) };
    if p.cancel.load(Ordering::Relaxed) {
        return libc::COPYFILE_QUIT;
    }
    if what == libc::COPYFILE_COPY_DATA && stage == libc::COPYFILE_PROGRESS {
        let mut bytes: libc::off_t = 0;
        if unsafe {
            copyfile_state_get(
                state,
                libc::COPYFILE_STATE_COPIED as u32,
                (&mut bytes as *mut libc::off_t).cast(),
            )
        } == 0
        {
            let base = *p.base.lock().unwrap_or_else(|e| e.into_inner());
            p.value.lock().unwrap_or_else(|e| e.into_inner()).copied =
                base.saturating_add(bytes.max(0) as u64);
            p.wake(false);
        }
    } else if what == libc::COPYFILE_RECURSE_FILE
        && stage == libc::COPYFILE_FINISH
        && !source.is_null()
    {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::lstat(source, stat.as_mut_ptr()) } == 0 {
            let bytes = unsafe { stat.assume_init() }.st_size.max(0) as u64;
            let mut base = p.base.lock().unwrap_or_else(|e| e.into_inner());
            *base = base.saturating_add(bytes);
        }
    }
    libc::COPYFILE_CONTINUE
}

fn size(path: &Path, p: &Progress) -> io::Result<u64> {
    if p.cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "Cancelled"));
    }
    let m = fs::symlink_metadata(path)?;
    if m.is_file() {
        return Ok(m.len());
    }
    if !m.is_dir() {
        return Ok(0);
    }
    let mut bytes = 0u64;
    for child in fs::read_dir(path)? {
        bytes = bytes.saturating_add(size(&child?.path(), p)?);
    }
    Ok(bytes)
}
fn writable(dest: &Path) -> io::Result<()> {
    if !fs::metadata(dest)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "The destination is not a folder",
        ));
    }
    let path = cpath(dest)?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { stat.assume_init() }.f_flags & libc::MNT_RDONLY as u32 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::ReadOnlyFilesystem,
            "This volume is read-only. macOS can't write to it.",
        ));
    }
    if unsafe { libc::access(path.as_ptr(), libc::W_OK) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn run(sources: Vec<PathBuf>, dest: PathBuf, progress: &Progress) -> Finished {
    let mut failures = Vec::new();
    let result = (|| -> io::Result<()> {
        writable(&dest)?;
        let destination = fs::canonicalize(&dest)?;
        let mut planned = Vec::with_capacity(sources.len());
        for source in &sources {
            if progress.cancel.load(Ordering::Relaxed) {
                break;
            }
            let Some(name) = source.file_name() else {
                failures.push("Can't copy a filesystem root".into());
                continue;
            };
            let target = destination.join(name);
            let display = name.to_string_lossy();
            if fs::canonicalize(source).is_ok_and(|s| destination.starts_with(s)) {
                failures.push(format!("Can't copy {display} into itself"));
                continue;
            }
            if fs::symlink_metadata(&target).is_ok() {
                failures.push(format!("{display} already exists. Skipped."));
                continue;
            }
            match size(source, progress) {
                Ok(bytes) => planned.push((source, name, target, bytes)),
                Err(e) if progress.cancel.load(Ordering::Relaxed) => return Err(e),
                Err(_) => planned.push((source, name, target, 0)),
            }
        }
        progress
            .value
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .total = planned
            .iter()
            .fold(0u64, |sum, (_, _, _, bytes)| sum.saturating_add(*bytes));
        progress.wake(true);
        for (source, name, target, bytes) in planned {
            if progress.cancel.load(Ordering::Relaxed) {
                break;
            }
            let before = progress.snapshot().copied;
            *progress.base.lock().unwrap_or_else(|e| e.into_inner()) = before;
            progress
                .value
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .name = name.to_string_lossy().into_owned();
            progress.wake(true);
            // A private directory owns every partial byte. An exclusive rename publishes the
            // finished item, so cancellation and races can never delete somebody else's target.
            let item = (|| -> io::Result<()> {
                let staging = tempfile::Builder::new()
                    .prefix(".crawler-copy-")
                    .tempdir_in(&destination)?;
                let partial = staging.path().join(name);
                let src = cpath(source)?;
                let dst = cpath(&partial)?;
                let final_path = cpath(&target)?;
                let state = unsafe { copyfile_state_alloc() };
                if state.is_null() {
                    return Err(io::Error::other("Couldn't allocate copy state"));
                }
                let setup = unsafe {
                    copyfile_state_set(
                        state,
                        libc::COPYFILE_STATE_STATUS_CB as u32,
                        callback as *const c_void,
                    ) == 0
                        && copyfile_state_set(
                            state,
                            libc::COPYFILE_STATE_STATUS_CTX as u32,
                            (progress as *const Progress).cast(),
                        ) == 0
                };
                let result = if setup {
                    unsafe {
                        copyfile(
                            src.as_ptr(),
                            dst.as_ptr(),
                            state,
                            libc::COPYFILE_METADATA
                                | libc::COPYFILE_DATA
                                | libc::COPYFILE_RECURSIVE
                                | libc::COPYFILE_EXCL
                                | libc::COPYFILE_NOFOLLOW_SRC,
                        )
                    }
                } else {
                    -1
                };
                let error = io::Error::last_os_error();
                unsafe {
                    copyfile_state_free(state);
                }
                if result != 0 {
                    return Err(error);
                }
                if progress.cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "Cancelled"));
                }
                if unsafe {
                    renameatx_np(
                        libc::AT_FDCWD,
                        dst.as_ptr(),
                        libc::AT_FDCWD,
                        final_path.as_ptr(),
                        libc::RENAME_EXCL,
                    )
                } != 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            })();
            match item {
                Ok(()) => {
                    progress
                        .value
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .copied = before.saturating_add(bytes);
                }
                Err(_) if progress.cancel.load(Ordering::Relaxed) => break,
                Err(e) => {
                    progress
                        .value
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .copied = before;
                    failures.push(format!("Couldn't copy {}: {e}", name.to_string_lossy()));
                }
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        if !progress.cancel.load(Ordering::Relaxed) {
            failures.push(e.to_string());
        }
    }
    let outcome = if progress.cancel.load(Ordering::Relaxed) {
        Outcome::Cancelled
    } else if failures.is_empty() {
        Outcome::Copied
    } else {
        Outcome::Failed
    };
    Finished {
        outcome,
        copied: progress.snapshot().copied,
        detail: if failures.is_empty() {
            match outcome {
                Outcome::Cancelled => "The partial item was removed.".into(),
                _ => String::new(),
            }
        } else {
            failures.join(" · ")
        },
    }
}
