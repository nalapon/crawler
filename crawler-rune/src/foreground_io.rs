//! A reserved worker for one foreground operation, with one replaceable queued request.
use rune::Waker;
use std::io;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

pub(crate) fn external_path(path: &Path) -> bool {
    ["/Volumes", "/Network", "/net", "/automount"]
        .iter()
        .any(|root| path.starts_with(root))
}

type Work<T> = Box<dyn FnOnce(&AtomicBool) -> T + Send>;

enum Request<T> {
    Idle,
    Waiting(Work<T>, Option<Waker>),
    Running(Arc<AtomicBool>),
    Ready(T),
}

pub(crate) struct Foreground<T> {
    name: &'static str,
    request: Arc<Mutex<Request<T>>>,
    signal: Option<mpsc::SyncSender<()>>,
}

impl<T> Foreground<T> {
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            name,
            request: Arc::new(Mutex::new(Request::Idle)),
            signal: None,
        }
    }

    pub(crate) fn cancel(&self) {
        let previous = std::mem::replace(&mut *self.request.lock().unwrap(), Request::Idle);
        if let Request::Running(cancelled) = previous {
            cancelled.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn is_pending(&self) -> bool {
        !matches!(*self.request.lock().unwrap(), Request::Idle)
    }

    pub(crate) fn take(&self) -> Option<T> {
        let mut request = self.request.lock().unwrap();
        if !matches!(*request, Request::Ready(_)) {
            return None;
        }
        match std::mem::replace(&mut *request, Request::Idle) {
            Request::Ready(value) => Some(value),
            _ => unreachable!(),
        }
    }
}

impl<T: Send + 'static> Foreground<T> {
    pub(crate) fn request(
        &mut self,
        wake: Option<Waker>,
        work: impl FnOnce(&AtomicBool) -> T + Send + 'static,
    ) -> io::Result<()> {
        if self.signal.is_none() {
            let (send, receive) = mpsc::sync_channel(1);
            let request = self.request.clone();
            rune::task::serve(self.name, receive, move |()| {
                run(&request);
                ControlFlow::Continue(())
            })?;
            self.signal = Some(send);
        }
        let previous = std::mem::replace(
            &mut *self.request.lock().unwrap(),
            Request::Waiting(Box::new(work), wake),
        );
        if let Request::Running(cancelled) = previous {
            cancelled.store(true, Ordering::Relaxed);
        }
        if let Err(mpsc::TrySendError::Disconnected(())) =
            self.signal.as_ref().unwrap().try_send(())
        {
            self.signal = None;
            self.cancel();
            return Err(io::Error::other("foreground worker stopped"));
        }
        Ok(())
    }
}

impl<T> Drop for Foreground<T> {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run<T>(request: &Mutex<Request<T>>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    let (work, wake) = {
        let mut request = request.lock().unwrap();
        if !matches!(*request, Request::Waiting(..)) {
            return;
        }
        match std::mem::replace(&mut *request, Request::Running(cancelled.clone())) {
            Request::Waiting(work, wake) => (work, wake),
            _ => unreachable!(),
        }
    };
    let result = work(&cancelled);
    let mut request = request.lock().unwrap();
    if matches!(&*request, Request::Running(active) if Arc::ptr_eq(active, &cancelled)) {
        *request = Request::Ready(result);
        drop(request);
        if let Some(wake) = wake {
            wake();
        }
    }
}
