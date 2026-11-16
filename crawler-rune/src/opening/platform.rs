//! Cocoa calls stay here. Callers send only owned Rust paths and results across threads.
use super::{Editor, OpenResult};
use objc2::{msg_send, rc::autoreleasepool};
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{
    NSArray, NSError, NSFileManager, NSString, NSURL, NSURLVolumeIsBrowsableKey,
    NSURLVolumeIsEjectableKey, NSURLVolumeIsRemovableKey, NSURLVolumeNameKey,
    NSVolumeEnumerationOptions,
};
use rune::Waker;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

pub fn registered_apps() -> Vec<PathBuf> {
    autoreleasepool(|_| unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        let mut out = Vec::new();
        for ext in ["txt", "swift", "md"] {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&format!("/Crawler.{ext}")));
            for url in workspace.URLsForApplicationsToOpenURL(&url).iter() {
                if let Some(path) = url.path() {
                    out.push(PathBuf::from(path.to_string()));
                }
            }
        }
        out
    })
}

pub fn open(
    path: &Path,
    editor: Option<&Editor>,
    result: Sender<OpenResult>,
    waker: Option<Waker>,
) {
    unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let path = path.to_owned();
        let completion = block2::RcBlock::new(
            move |_app: *mut NSRunningApplication, error: *mut NSError| {
                let outcome = if let Some(error) = error.as_ref() {
                    Err(error.localizedDescription().to_string())
                } else {
                    Ok(())
                };
                let _ = result.send(OpenResult {
                    path: path.clone(),
                    outcome,
                });
                if let Some(w) = &waker {
                    w();
                }
            },
        );
        let workspace = NSWorkspace::sharedWorkspace();
        let config = NSWorkspaceOpenConfiguration::new();
        if let Some(editor) = editor {
            let application =
                NSURL::fileURLWithPath(&NSString::from_str(&editor.path.to_string_lossy()));
            workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
                &NSArray::from_slice(&[&*url]),
                &application,
                &config,
                Some(&completion),
            );
        } else {
            workspace.openURL_configuration_completionHandler(&url, &config, Some(&completion));
        }
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
            NSVolumeEnumerationOptions::NSVolumeEnumerationSkipHiddenVolumes,
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
    autoreleasepool(|_| unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        NSWorkspace::sharedWorkspace()
            .unmountAndEjectDeviceAtURL_error(&url)
            .map_err(|e| e.localizedDescription().to_string())
    })
}

pub struct VolumeObserver {
    center: objc2::rc::Retained<objc2_foundation::NSNotificationCenter>,
    tokens: Vec<objc2::rc::Retained<objc2_foundation::NSObject>>,
}
impl Drop for VolumeObserver {
    fn drop(&mut self) {
        unsafe {
            for token in &self.tokens {
                self.center.removeObserver(token);
            }
        }
    }
}
pub fn observe_volumes(tx: Sender<()>, waker: Waker) -> VolumeObserver {
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
        VolumeObserver { center, tokens }
    }
}

pub fn source_type(identifier: &str) -> bool {
    use objc2_uniform_type_identifiers::{UTType, UTTypeSourceCode};
    autoreleasepool(|_| unsafe {
        UTType::typeWithIdentifier(&NSString::from_str(identifier))
            .is_some_and(|t| t.conformsToType(UTTypeSourceCode))
    })
}

pub fn is_package(path: &Path) -> bool {
    autoreleasepool(|_| unsafe {
        let url =
            NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&path.to_string_lossy()), true);
        let mut value = None;
        if url
            .getResourceValue_forKey_error(&mut value, objc2_foundation::NSURLIsPackageKey)
            .is_err()
        {
            return false;
        }
        value.is_some_and(|v| {
            let b: bool = msg_send![&*v, boolValue];
            b
        })
    })
}
