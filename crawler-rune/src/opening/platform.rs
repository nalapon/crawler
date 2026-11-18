//! Cocoa calls stay here. Callers send only owned Rust paths and results across threads.
use super::OpenResult;
use objc2::{msg_send, rc::autoreleasepool};
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{
    NSArray, NSDirectoryEnumerationOptions, NSError, NSFileManager, NSString, NSURL,
    NSURLIsPackageKey, NSURLVolumeIsBrowsableKey, NSURLVolumeIsEjectableKey,
    NSURLVolumeIsRemovableKey, NSURLVolumeNameKey, NSVolumeEnumerationOptions,
};
use rune::Waker;
use std::collections::HashMap;
use std::ffi::{CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

/// Hands `path` to an application (Terminal for a folder). The result arrives on `result`.
pub fn open_application(
    path: &Path,
    application: &Path,
    result: Sender<OpenResult>,
    waker: Option<Waker>,
) {
    unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let completion = block2::RcBlock::new(
            move |_app: *mut NSRunningApplication, error: *mut NSError| {
                let outcome = match error.as_ref() {
                    Some(error) => Err(error.localizedDescription().to_string()),
                    None => Ok(()),
                };
                let _ = result.send(outcome);
                if let Some(w) = &waker {
                    w();
                }
            },
        );
        let application =
            NSURL::fileURLWithPath(&NSString::from_str(&application.to_string_lossy()));
        NSWorkspace::sharedWorkspace()
            .openURLs_withApplicationAtURL_configuration_completionHandler(
                &NSArray::from_slice(&[&*url]),
                &application,
                &NSWorkspaceOpenConfiguration::new(),
                Some(&completion),
            );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    pub path: PathBuf,
    pub name: String,
    pub ejectable: bool,
}
pub fn volumes() -> Vec<Volume> {
    autoreleasepool(|_| unsafe {
        let manager = NSFileManager::defaultManager();
        let Some(urls) = manager.mountedVolumeURLsIncludingResourceValuesForKeys_options(
            None,
            NSVolumeEnumerationOptions::SkipHiddenVolumes,
        ) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for url in urls.iter() {
            let Some(path) = url.path() else { continue };
            let path = PathBuf::from(path.to_string());
            if path == Path::new("/") || path.starts_with("/System/Volumes") {
                continue;
            }
            let bool_value = |key: &NSString| -> Option<bool> {
                let mut value = None;
                url.getResourceValue_forKey_error(&mut value, key).ok()?;
                value.map(|v| {
                    let b: bool = msg_send![&*v, boolValue];
                    b
                })
            };
            if bool_value(NSURLVolumeIsBrowsableKey) == Some(false) {
                continue;
            }
            let mut title = None;
            let _ = url.getResourceValue_forKey_error(&mut title, NSURLVolumeNameKey);
            let name = title
                .map(|v| {
                    let s: &NSString = msg_send![&*v, description];
                    s.to_string()
                })
                .unwrap_or_else(|| crate::funnel::model::name(&path));
            out.push(Volume {
                path,
                name,
                ejectable: bool_value(NSURLVolumeIsEjectableKey) == Some(true)
                    || bool_value(NSURLVolumeIsRemovableKey) == Some(true),
            });
        }
        out.sort_by(|a, b| rune_kit::natural_cmp(&a.name, &b.name));
        out
    })
}
pub fn eject(path: &Path) -> Result<(), String> {
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        NSWorkspace::sharedWorkspace()
            .unmountAndEjectDeviceAtURL_error(&url)
            .map_err(|e| e.localizedDescription().to_string())
    })
}

pub struct WorkspaceObserver {
    center: objc2::rc::Retained<objc2_foundation::NSNotificationCenter>,
    tokens: Vec<
        objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>,
    >,
}
impl Drop for WorkspaceObserver {
    fn drop(&mut self) {
        unsafe {
            for token in &self.tokens {
                self.center.removeObserver(token.as_ref());
            }
        }
    }
}
pub fn observe_volumes(tx: Sender<()>, waker: Waker) -> WorkspaceObserver {
    use objc2_app_kit::{
        NSWorkspaceDidMountNotification, NSWorkspaceDidRenameVolumeNotification,
        NSWorkspaceDidUnmountNotification,
    };
    use objc2_foundation::NSNotification;
    use std::ptr::NonNull;
    unsafe {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let mut tokens = Vec::new();
        for name in [
            NSWorkspaceDidMountNotification,
            NSWorkspaceDidUnmountNotification,
            NSWorkspaceDidRenameVolumeNotification,
        ] {
            let tx = tx.clone();
            let waker = waker.clone();
            let block = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
                let _ = tx.send(());
                waker();
            });
            tokens.push(center.addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                None,
                &block,
            ));
        }
        WorkspaceObserver { center, tokens }
    }
}

pub fn is_package(path: &Path) -> bool {
    package_status(path).unwrap_or(false)
}

/// Unknown resource values are safe to display as folders but should be checked again.
pub(crate) fn package_status(path: &Path) -> Option<bool> {
    autoreleasepool(|_| {
        let url =
            NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&path.to_string_lossy()), true);
        url_package_status(&url)
    })
}

fn url_package_status(url: &NSURL) -> Option<bool> {
    unsafe {
        let mut value = None;
        url.getResourceValue_forKey_error(&mut value, NSURLIsPackageKey)
            .ok()?;
        value.map(|v| {
            let b: bool = msg_send![&*v, boolValue];
            b
        })
    }
}

/// Reads the same package key from URLs with Foundation's directory prefetch enabled.
/// A missing URL or unknown resource value is left for the caller's ordinary lookup.
pub(crate) fn prefetch_packages(
    folder: &Path,
    mut candidates: HashMap<&OsStr, &mut Option<bool>>,
    cancel: &AtomicBool,
) {
    if candidates.is_empty() || cancel.load(Ordering::Relaxed) {
        return;
    }
    let Some(folder) = folder.to_str() else {
        return;
    };
    autoreleasepool(|_| unsafe {
        let path = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(folder), true);
        let keys = NSArray::from_slice(&[NSURLIsPackageKey]);
        let Some(entries) = NSFileManager::defaultManager()
            .enumeratorAtURL_includingPropertiesForKeys_options_errorHandler(
                &path,
                Some(&keys),
                NSDirectoryEnumerationOptions::SkipsSubdirectoryDescendants
                    | NSDirectoryEnumerationOptions::SkipsHiddenFiles,
                None,
            )
        else {
            return;
        };
        while !candidates.is_empty() && !cancel.load(Ordering::Relaxed) {
            let next = autoreleasepool(|_| {
                let Some(entry) = entries.nextObject() else {
                    return false;
                };
                if let Ok(url) = entry.downcast::<NSURL>() {
                    let bytes = CStr::from_ptr(url.fileSystemRepresentation().as_ptr()).to_bytes();
                    let name = Path::new(OsStr::from_bytes(bytes)).file_name();
                    if let Some(value) = name.and_then(|name| candidates.remove(name)) {
                        *value = url_package_status(&url);
                    }
                }
                true
            });
            if !next {
                break;
            }
        }
    })
}

pub fn terminal_application() -> Option<PathBuf> {
    NSWorkspace::sharedWorkspace()
        .URLForApplicationWithBundleIdentifier(&NSString::from_str("com.apple.Terminal"))
        .and_then(|url| url.path())
        .map(|p| PathBuf::from(p.to_string()))
}
pub fn reveal(paths: &[PathBuf]) {
    let urls: Vec<_> = paths
        .iter()
        .map(|p| NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy())))
        .collect();
    NSWorkspace::sharedWorkspace()
        .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&urls));
}
