//! FSEvents owns its callback and stream on one background thread.
use objc2_core_foundation::{CFArray, CFRunLoop, CFString, kCFRunLoopDefaultMode};
use objc2_core_services::*;
use std::{
    ffi::{CStr, c_char, c_void},
    path::PathBuf,
    ptr::{self, NonNull},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

#[derive(Debug)]
pub struct Events {
    pub paths: Vec<PathBuf>,
    pub full_scan: bool,
    pub history_done: bool,
    pub last_id: u64,
}
pub struct Watcher {
    cancel: Arc<AtomicBool>,
}
impl Watcher {
    pub fn start(
        root: PathBuf,
        since: Option<u64>,
        deliver: impl Fn(Events) + Send + 'static,
    ) -> (Self, mpsc::Receiver<bool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let token = Arc::clone(&cancel);
        let (ready, receiver) = mpsc::channel();
        let _ = thread::Builder::new()
            .name("crawler-fsevents".into())
            .spawn(move || {
                let mut callback: Box<Box<dyn Fn(Events) + Send>> = Box::new(Box::new(deliver));
                let mut context = FSEventStreamContext {
                    version: 0,
                    info: ptr::from_mut(callback.as_mut()).cast(),
                    retain: None,
                    release: None,
                    copyDescription: None,
                };
                let root = std::fs::canonicalize(&root).unwrap_or(root);
                let path = CFString::from_str(root.to_str().unwrap_or("/"));
                let paths = CFArray::from_objects(&[&*path]);
                // All CF objects and callback storage stay alive until Stop/Invalidate/Release below.
                let stream = unsafe {
                    FSEventStreamCreate(
                        None,
                        Some(receive),
                        &mut context,
                        paths.as_ref(),
                        since.unwrap_or(u64::MAX),
                        0.2,
                        kFSEventStreamCreateFlagFileEvents
                            | kFSEventStreamCreateFlagWatchRoot
                            | kFSEventStreamCreateFlagNoDefer,
                    )
                };
                if stream.is_null() {
                    let _ = ready.send(false);
                    return;
                }
                let Some(run_loop) = CFRunLoop::current() else {
                    unsafe {
                        FSEventStreamRelease(stream);
                    }
                    let _ = ready.send(false);
                    return;
                };
                // The deprecated run-loop API keeps teardown and callback lifetime on this thread.
                #[allow(deprecated)]
                unsafe {
                    FSEventStreamScheduleWithRunLoop(
                        stream,
                        &run_loop,
                        kCFRunLoopDefaultMode.expect("default CF run-loop mode"),
                    );
                }
                let started = unsafe { FSEventStreamStart(stream) };
                let _ = ready.send(started);
                if started {
                    while !token.load(Ordering::Relaxed) {
                        CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, 0.1, false);
                    }
                    unsafe {
                        FSEventStreamStop(stream);
                    }
                }
                unsafe {
                    FSEventStreamInvalidate(stream);
                    FSEventStreamRelease(stream);
                }
            });
        (Self { cancel }, receiver)
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
pub fn current() -> u64 {
    unsafe { FSEventsGetCurrentEventId() }
}
unsafe extern "C-unwind" fn receive(
    _: ConstFSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: NonNull<c_void>,
    flags: NonNull<u32>,
    ids: NonNull<u64>,
) {
    if info.is_null() {
        return;
    }
    // No UseCFTypes flag: CoreServices supplies count C string pointers, flags and ids.
    let callback = unsafe { &*info.cast::<Box<dyn Fn(Events) + Send>>() };
    let paths =
        unsafe { std::slice::from_raw_parts(paths.as_ptr().cast::<*const c_char>(), count) };
    let flags = unsafe { std::slice::from_raw_parts(flags.as_ptr(), count) };
    let ids = unsafe { std::slice::from_raw_parts(ids.as_ptr(), count) };
    let mut events = Events {
        paths: Vec::new(),
        full_scan: false,
        history_done: false,
        last_id: 0,
    };
    for ((&path, &flag), &id) in paths.iter().zip(flags).zip(ids) {
        if flag & kFSEventStreamEventFlagHistoryDone != 0 {
            events.history_done = true;
            continue;
        }
        events.last_id = events.last_id.max(id);
        if flag
            & (kFSEventStreamEventFlagMustScanSubDirs
                | kFSEventStreamEventFlagRootChanged
                | kFSEventStreamEventFlagUserDropped
                | kFSEventStreamEventFlagKernelDropped
                | kFSEventStreamEventFlagEventIdsWrapped)
            != 0
        {
            events.full_scan = true;
        }
        if flag
            & (kFSEventStreamEventFlagItemCreated
                | kFSEventStreamEventFlagItemRemoved
                | kFSEventStreamEventFlagItemRenamed
                | kFSEventStreamEventFlagItemFinderInfoMod
                | kFSEventStreamEventFlagItemInodeMetaMod)
            == 0
            || path.is_null()
        {
            continue;
        }
        if let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() {
            events.paths.push(PathBuf::from(path));
        }
    }
    // Never unwind through CoreServices if a receiver has already gone away.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(events)));
}
