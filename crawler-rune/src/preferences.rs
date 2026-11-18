//! Keep disk sync off the UI thread and collapse bursts of changes to the same preference.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rune::Waker;
use rune::prefs::{Prefs, PrefsError};
use serde_json::Value;

type Changes = BTreeMap<&'static str, Value>;

pub struct Preferences {
    store: Option<Prefs>,
    changes: Arc<Mutex<Changes>>,
    writer: Option<(mpsc::SyncSender<()>, JoinHandle<()>)>,
    errors: mpsc::Receiver<String>,
    send_error: mpsc::SyncSender<String>,
}

impl Preferences {
    pub fn new(store: Prefs) -> Self {
        let (send_error, errors) = mpsc::sync_channel(1);
        Self {
            store: store.is_persistent().then_some(store),
            changes: Arc::default(),
            writer: None,
            errors,
            send_error,
        }
    }

    pub fn set<T: serde::Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
        wake: Option<Waker>,
    ) -> Result<(), PrefsError> {
        if self.writer.is_none() {
            let Some(store) = self.store.take() else {
                return Ok(());
            };
            let changes = self.changes.clone();
            let errors = self.send_error.clone();
            let (send, receive) = mpsc::sync_channel(1);
            let worker = thread::Builder::new()
                .name("crawler-preferences".into())
                .spawn(move || save(store, changes, receive, errors, wake))
                .map_err(PrefsError::Io)?;
            self.writer = Some((send, worker));
        }
        let value = serde_json::to_value(value)
            .map_err(|error| PrefsError::Serialize(error.to_string()))?;
        self.changes.lock().unwrap().insert(key, value);
        // An outstanding notification covers every change in the map.
        let (send, _) = self.writer.as_ref().unwrap();
        let _ = send.try_send(());
        Ok(())
    }

    pub fn take_error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }

    pub fn finish(&mut self) {
        if let Some((send, worker)) = self.writer.take() {
            drop(send);
            let _ = worker.join();
        }
    }
}

impl Drop for Preferences {
    fn drop(&mut self) {
        self.finish();
    }
}

fn save(
    mut store: Prefs,
    changes: Arc<Mutex<Changes>>,
    receive: mpsc::Receiver<()>,
    errors: mpsc::SyncSender<String>,
    wake: Option<Waker>,
) {
    let mut needs_save = false;
    while receive.recv().is_ok() {
        thread::sleep(Duration::from_millis(80));
        let batch = std::mem::take(&mut *changes.lock().unwrap());
        for (key, value) in batch {
            if !needs_save && store.get::<Value>(key).as_ref() == Some(&value) {
                continue;
            }
            let result = store.set(key, &value);
            needs_save = result.is_err();
            if let Err(error) = result {
                if errors.try_send(error.to_string()).is_ok()
                    && let Some(wake) = &wake
                {
                    wake();
                }
            }
        }
    }
}
