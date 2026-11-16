//! Native URL actions and background palette context discovery.
use objc2::rc::Retained;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Application {
    pub name: String,
    pub path: PathBuf,
}
#[derive(Default)]
pub struct Context {
    pub selected_is_folder: bool,
    pub applications: Vec<Application>,
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
        out
    })
}
