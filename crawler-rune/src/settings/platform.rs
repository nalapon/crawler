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

/// AppKit resolves the installed application's icon off the UI thread, then only pixels cross back.
pub fn application_icon(path: &Path) -> Option<rune::Pixels> {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo, CGImageByteOrderInfo,
    };
    objc2::rc::autoreleasepool(|_| {
        let icon = NSWorkspace::sharedWorkspace()
            .iconForFile(&NSString::from_str(&path.to_string_lossy()));
        let mut rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(32.0, 32.0));
        // SAFETY: the proposed rect lives through the call, with no context or hint pointers.
        let image = unsafe { icon.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let space = CGColorSpace::new_device_rgb()?;
        let mut rgba = vec![0_u8; 32 * 32 * 4];
        // SAFETY: the buffer outlives this context and holds exactly its 32 rows and stride.
        let context = unsafe {
            CGBitmapContextCreate(
                rgba.as_mut_ptr().cast(),
                32,
                32,
                8,
                128,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
            )
        }?;
        CGContext::draw_image(
            Some(&context),
            CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(32.0, 32.0)),
            Some(&image),
        );
        drop(context);
        for pixel in rgba.chunks_exact_mut(4) {
            let alpha = u32::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = (u32::from(*channel) * 255 + alpha / 2)
                    .checked_div(alpha)
                    .unwrap_or(0)
                    .min(255) as u8;
            }
        }
        let pixels = rune::Pixels::new();
        pixels.set(32, 32, rgba);
        Some(pixels)
    })
}
