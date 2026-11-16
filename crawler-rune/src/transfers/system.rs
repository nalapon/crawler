//! Explicit system fallback. didShare means the picker closed, including cancellation.
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
};
use objc2_app_kit::{
    NSSharingService, NSSharingServiceDelegate, NSSharingServiceNameSendViaAirDrop,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol, NSString, NSURL};
use rune::Waker;
use std::{path::PathBuf, sync::mpsc};
struct Callback {
    tx: mpsc::Sender<Result<(), String>>,
    waker: Waker,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Callback]
    #[name = "CrawlerRuneAirDropDelegate"]
    struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl NSSharingServiceDelegate for Delegate {
        #[unsafe(method(sharingService:didShareItems:))]
        unsafe fn did_share(&self, _: &NSSharingService, _: &NSArray) {
            let _ = self.ivars().tx.send(Ok(()));
            (self.ivars().waker)();
        }
        #[unsafe(method(sharingService:didFailToShareItems:error:))]
        unsafe fn failed(&self, _: &NSSharingService, _: &NSArray, error: &NSError) {
            let _ = self
                .ivars()
                .tx
                .send(Err(error.localizedDescription().to_string()));
            (self.ivars().waker)();
        }
    }
);
pub struct SystemShare {
    _service: Retained<NSSharingService>,
    _delegate: Retained<Delegate>,
    pub rx: mpsc::Receiver<Result<(), String>>,
}
impl Drop for SystemShare {
    fn drop(&mut self) {
        self._service.setDelegate(None);
    }
}
pub fn share(files: &[PathBuf], waker: Waker) -> Result<SystemShare, String> {
    if rune::tooling::headless_run().is_some() {
        return Err("The macOS AirDrop window is unavailable in a headless run.".into());
    }
    let mtm = MainThreadMarker::new().ok_or("AirDrop must open on the UI thread.")?;
    let service =
        NSSharingService::sharingServiceNamed(unsafe { NSSharingServiceNameSendViaAirDrop })
            .ok_or("AirDrop is unavailable on this Mac.")?;
    let urls: Vec<Retained<AnyObject>> = files
        .iter()
        .map(|p| {
            NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&p.to_string_lossy()), false)
                .into_super()
                .into_super()
        })
        .collect();
    let items = NSArray::from_retained_slice(&urls);
    if !unsafe { service.canPerformWithItems(Some(&items)) } {
        return Err("AirDrop is unavailable for these files.".into());
    }
    let (tx, rx) = mpsc::channel();
    let delegate = Delegate::alloc(mtm).set_ivars(Callback { tx, waker });
    let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
    service.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    unsafe {
        service.performWithItems(&items);
    }
    Ok(SystemShare {
        _service: service,
        _delegate: delegate,
        rx,
    })
}
