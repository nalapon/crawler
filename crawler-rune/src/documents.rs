//! Folders and files sent by `open`, the `crawler` CLI, Finder or the Dock icon: the
//! open-documents Apple event. winit doesn't forward it, so Crawler installs its own handler and
//! queues the paths for `app::poll`, which opens them like Swift's `application(_:open:)`:
//! folders in Folders, files through the normal Enter policy.

use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::{Mutex, OnceLock};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSApplicationWillFinishLaunchingNotification;
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter,
};
use rune::Waker;

/// `kCoreEventClass` ('aevt'), `kAEOpenDocuments` ('odoc') and `keyDirectObject` ('----').
const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

static QUEUE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static WAKER: OnceLock<Waker> = OnceLock::new();

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CrawlerOpenDocuments"]
    struct Handler;

    impl Handler {
        #[unsafe(method(handleOpen:withReply:))]
        fn handle_open(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            let Some(list) = event.paramDescriptorForKeyword(DIRECT_OBJECT) else {
                return;
            };
            let paths = (1..=list.numberOfItems())
                .filter_map(|i| list.descriptorAtIndex(i)?.fileURLValue()?.path())
                .map(|path| PathBuf::from(path.to_string()));
            if let Ok(mut queue) = QUEUE.lock() {
                queue.extend(paths);
            }
            if let Some(waker) = WAKER.get() {
                waker();
            }
        }
    }
);

/// Called before the event loop. AppKit installs its own open-documents handler while it finishes
/// launching, so Crawler installs its handler again on `NSApplicationWillFinishLaunching`, the point
/// Apple documents for catching the event that launched the app.
pub fn install() {
    install_handler();
    let block = RcBlock::new(|_: NonNull<NSNotification>| install_handler());
    // SAFETY: the block takes the notification argument the API passes. The observer token is
    // leaked because the observer lives as long as the process.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationWillFinishLaunchingNotification),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(token);
}

/// Makes Crawler's handler the one for open-documents events.
fn install_handler() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let handler: Retained<Handler> = unsafe { msg_send![Handler::alloc(mtm), init] };
    let manager = NSAppleEventManager::sharedAppleEventManager();
    // SAFETY: the selector matches `handle_open`'s signature. The manager doesn't retain its
    // handler, so the handler is leaked and lives as long as the process.
    unsafe {
        manager.setEventHandler_andSelector_forEventClass_andEventID(
            &handler,
            sel!(handleOpen:withReply:),
            CORE_EVENT_CLASS,
            OPEN_DOCUMENTS,
        );
    }
    std::mem::forget(handler);
}

/// Wakes the app when paths arrive after launch.
pub fn set_waker(waker: Waker) {
    let _ = WAKER.set(waker);
}

/// Paths received since the last call, in arrival order.
pub fn take() -> Vec<PathBuf> {
    QUEUE
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}
