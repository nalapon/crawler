//! Native URL actions and background palette context discovery.
use objc2::rc::Retained;
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSArray, NSError, NSString, NSURL};
use rune::Waker;
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::mpsc::Sender,
};

#[derive(Clone, Debug)]
pub struct Application {
    pub name: String,
    pub path: PathBuf,
}
#[derive(Default)]
pub struct Context {
    pub selected_is_folder: bool,
    pub applications: Vec<Application>,
    pub drives: Vec<PathBuf>,
    pub error: Option<String>,
}
fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

pub fn context(selected: Option<PathBuf>) -> Context {
    objc2::rc::autoreleasepool(|_| {
        let mut out = Context::default();
        if let Some(path) = selected {
            out.selected_is_folder = path.is_dir()
                && !NSWorkspace::sharedWorkspace()
                    .isFilePackageAtPath(&NSString::from_str(&path.to_string_lossy()));
            if !out.selected_is_folder {
                let apps =
                    NSWorkspace::sharedWorkspace().URLsForApplicationsToOpenURL(&file_url(&path));
                out.applications = apps
                    .iter()
                    .filter_map(|url| {
                        url.path().map(|p| {
                            let path = PathBuf::from(p.to_string());
                            Application {
                                name: path
                                    .file_stem()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                                path,
                            }
                        })
                    })
                    .collect();
                out.applications.sort_by(|a, b| a.name.cmp(&b.name));
            }
        }
        let startup = std::fs::metadata("/").ok().map(|m| m.dev());
        match std::fs::read_dir("/Volumes") {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if entry.file_name().to_string_lossy().starts_with('.') {
                        continue;
                    }
                    let path = entry.path();
                    if std::fs::metadata(&path)
                        .ok()
                        .is_some_and(|m| m.is_dir() && Some(m.dev()) != startup)
                    {
                        out.drives.push(path);
                    }
                }
            }
            Err(e) => out.error = Some(format!("Could not read connected drives: {e}")),
        }
        out.drives.sort();
        out
    })
}

pub fn reveal(paths: &[PathBuf]) {
    let urls: Vec<_> = paths.iter().map(|p| file_url(p)).collect();
    NSWorkspace::sharedWorkspace()
        .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&urls));
}

pub fn open(
    path: &Path,
    application: Option<&Path>,
    terminal: bool,
    send: Sender<Result<(), String>>,
    waker: Option<Waker>,
) {
    let workspace = NSWorkspace::sharedWorkspace();
    let application = if terminal {
        workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str("com.apple.Terminal"))
    } else {
        application.map(file_url)
    };
    let Some(application) = application else {
        let _ = send.send(Err("The application is unavailable.".into()));
        if let Some(w) = waker {
            w();
        }
        return;
    };
    let complete =
        block2::RcBlock::new(move |_: *mut NSRunningApplication, error: *mut NSError| {
            // SAFETY: NSWorkspace supplies a borrowed NSError valid for the completion call.
            let result = unsafe { error.as_ref() }
                .map_or(Ok(()), |e| Err(e.localizedDescription().to_string()));
            let _ = send.send(result);
            if let Some(w) = &waker {
                w();
            }
        });
    workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
        &NSArray::from_retained_slice(&[file_url(path)]),
        &application,
        &NSWorkspaceOpenConfiguration::new(),
        Some(&complete),
    );
}
