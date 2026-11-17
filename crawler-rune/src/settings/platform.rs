//! Background palette context discovery.
use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSString;
use std::path::PathBuf;

#[derive(Default)]
pub struct Context {
    pub selected_is_folder: bool,
}

pub fn context(selected: Option<PathBuf>) -> Context {
    objc2::rc::autoreleasepool(|_| Context {
        selected_is_folder: selected.is_some_and(|path| {
            path.is_dir()
                && !NSWorkspace::sharedWorkspace()
                    .isFilePackageAtPath(&NSString::from_str(&path.to_string_lossy()))
        }),
    })
}
