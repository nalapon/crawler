//! Shared file-type icons. Rune decodes/rasterizes on its image worker; handles stay cached by type.
use rune::{El, Image, ImageCache, Waker, el};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

type Types = Arc<Mutex<HashMap<String, Option<Image>>>>;
struct Cache {
    images: Arc<ImageCache>,
    types: Types,
    native: mpsc::Sender<String>,
    changed: Arc<AtomicBool>,
}
static CACHE: OnceLock<Cache> = OnceLock::new();

pub fn setup(waker: &Waker) {
    CACHE.get_or_init(|| {
        let images = Arc::new(ImageCache::new(waker.clone()));
        let types: Types = Arc::new(Mutex::new(HashMap::new()));
        let (native, receive) = mpsc::channel::<String>();
        let worker_images = images.clone();
        let worker_types = types.clone();
        let wake = waker.clone();
        let changed = Arc::new(AtomicBool::new(false));
        let worker_changed = changed.clone();
        std::thread::spawn(move || {
            for key in receive {
                let image = native_png(&key).map(|bytes| worker_images.load(bytes));
                if let Some(image) = &image {
                    let _ = image.wait();
                }
                if let Ok(mut types) = worker_types.lock() {
                    types.insert(key, image);
                }
                worker_changed.store(true, Ordering::Release);
                wake();
            }
        });
        Cache {
            images,
            types,
            native,
            changed,
        }
    });
}

/// Publishes completed native type icons without doing any lookup in the view.
pub fn poll() -> bool {
    CACHE
        .get()
        .is_some_and(|cache| cache.changed.swap(false, Ordering::AcqRel))
}

/// Material assets for known types and native type icons for the rest, cached at Retina resolution.
pub fn file(path: &Path, folder: bool, classic: bool, size: f32) -> El {
    let Some(cache) = CACHE.get() else {
        return el().size(size, size);
    };
    let mut key = key(path, folder, classic);
    let Ok(mut types) = cache.types.lock() else {
        return el().size(size, size);
    };
    if types.len() >= 256 && !types.contains_key(&key) {
        key = "native/".into();
    }
    let image = types.entry(key.clone()).or_insert_with(|| {
        if key == "folder/system7" {
            Some(cache.images.svg(br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M1.5 20.5V5.5h8l2 2h11v13z" fill="white" stroke="black"/><path d="M3 10h18v9H3z" fill="#ccc"/><path d="M2 9.5h20" stroke="black"/></svg>"##.as_slice(), 24.0, 2.0))
        } else if let Some(bytes) = key.strip_prefix("material/").and_then(material) {
            Some(cache.images.svg(bytes, 24.0, 2.0))
        } else {
            let _ = cache.native.send(key);
            None
        }
    }).clone();
    let icon = el().key("file-icon").size(size, size).inert();
    if let Some(image) = image {
        icon.pixels_fit(image.pixels(), rune::SurfaceFit::Fit)
    } else {
        icon
    }
}

// AppKit owns every object on this serial worker. Only encoded bytes cross to Rune.
fn native_png(key: &str) -> Option<Vec<u8>> {
    use objc2::{AllocAnyThread, rc::autoreleasepool};
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace,
        NSGraphicsContext, NSWorkspace,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};
    use objc2_uniform_type_identifiers::UTType;
    autoreleasepool(|_| {
        let kind = if key == "folder" {
            UTType::typeWithIdentifier(&NSString::from_str("public.folder"))
        } else {
            UTType::typeWithFilenameExtension(&NSString::from_str(
                key.trim_start_matches("native/"),
            ))
        }
        .or_else(|| UTType::typeWithIdentifier(&NSString::from_str("public.data")))?;
        // SAFETY: this is NSWorkspace's public macOS 11+ selector and its declared NSImage
        // result. The generated binding is behind an optional cross-crate feature.
        let image: objc2::rc::Retained<objc2_app_kit::NSImage> = unsafe {
            objc2::msg_send![&*NSWorkspace::sharedWorkspace(), iconForContentType: &*kind]
        };
        // SAFETY: null planes ask AppKit to allocate a 48×48 RGBA buffer. The empty
        // properties dictionary has no values with an incompatible Objective-C type.
        unsafe {
            let bitmap = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(), std::ptr::null_mut(), 48, 48, 8, 4, true, false, NSDeviceRGBColorSpace, 0, 0)?;
            let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)?;
            NSGraphicsContext::saveGraphicsState_class();
            NSGraphicsContext::setCurrentContext(Some(&context));
            image.drawInRect_fromRect_operation_fraction(
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(48.0, 48.0)),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
                NSCompositingOperation::SourceOver,
                1.0,
            );
            NSGraphicsContext::restoreGraphicsState_class();
            bitmap
                .representationUsingType_properties(
                    NSBitmapImageFileType::PNG,
                    &NSDictionary::new(),
                )
                .map(|data| data.to_vec())
        }
    })
}

fn key(path: &Path, folder: bool, classic: bool) -> String {
    if folder {
        return if classic { "folder/system7" } else { "folder" }.into();
    }
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let material = if matches!(
        name.as_str(),
        "go.mod" | "go.sum" | "go.work" | "go.work.sum"
    ) {
        Some("go-mod")
    } else if name == "dockerfile" || name.starts_with("dockerfile.") {
        Some("docker")
    } else if matches!(
        name.as_str(),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitconfig"
    ) {
        Some("git")
    } else {
        match ext.as_str() {
            "ts" | "mts" | "cts" => Some("typescript"),
            "tsx" => Some("react_ts"),
            "js" | "mjs" | "cjs" => Some("javascript"),
            "jsx" => Some("react"),
            "py" | "pyi" | "pyw" => Some("python"),
            "rs" => Some("rust"),
            "md" | "markdown" | "mdx" => Some("markdown"),
            "json" | "jsonc" | "json5" | "jsonl" => Some("json"),
            "yml" | "yaml" => Some("yaml"),
            "xml" | "plist" => Some("xml"),
            "html" | "htm" => Some("html"),
            "scss" | "sass" => Some("sass"),
            "sh" | "bash" | "zsh" | "fish" => Some("console"),
            "c" | "h" => Some("c"),
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Some("cpp"),
            "cs" => Some("csharp"),
            "kt" | "kts" => Some("kotlin"),
            "rb" => Some("ruby"),
            "sql" => Some("database"),
            "go" | "swift" | "css" | "less" | "java" | "php" | "lua" | "toml" | "vue"
            | "svelte" | "astro" => Some(ext.as_str()),
            _ => None,
        }
    };
    material.map_or_else(|| format!("native/{ext}"), |m| format!("material/{m}"))
}
fn material(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "astro" => include_bytes!("../../Resources/FileIcons/astro.svg"),
        "c" => include_bytes!("../../Resources/FileIcons/c.svg"),
        "console" => include_bytes!("../../Resources/FileIcons/console.svg"),
        "cpp" => include_bytes!("../../Resources/FileIcons/cpp.svg"),
        "csharp" => include_bytes!("../../Resources/FileIcons/csharp.svg"),
        "css" => include_bytes!("../../Resources/FileIcons/css.svg"),
        "database" => include_bytes!("../../Resources/FileIcons/database.svg"),
        "docker" => include_bytes!("../../Resources/FileIcons/docker.svg"),
        "git" => include_bytes!("../../Resources/FileIcons/git.svg"),
        "go" => include_bytes!("../../Resources/FileIcons/go.svg"),
        "go-mod" => include_bytes!("../../Resources/FileIcons/go-mod.svg"),
        "html" => include_bytes!("../../Resources/FileIcons/html.svg"),
        "java" => include_bytes!("../../Resources/FileIcons/java.svg"),
        "javascript" => include_bytes!("../../Resources/FileIcons/javascript.svg"),
        "json" => include_bytes!("../../Resources/FileIcons/json.svg"),
        "kotlin" => include_bytes!("../../Resources/FileIcons/kotlin.svg"),
        "less" => include_bytes!("../../Resources/FileIcons/less.svg"),
        "lua" => include_bytes!("../../Resources/FileIcons/lua.svg"),
        "markdown" => include_bytes!("../../Resources/FileIcons/markdown.svg"),
        "php" => include_bytes!("../../Resources/FileIcons/php.svg"),
        "python" => include_bytes!("../../Resources/FileIcons/python.svg"),
        "react" => include_bytes!("../../Resources/FileIcons/react.svg"),
        "react_ts" => include_bytes!("../../Resources/FileIcons/react_ts.svg"),
        "ruby" => include_bytes!("../../Resources/FileIcons/ruby.svg"),
        "rust" => include_bytes!("../../Resources/FileIcons/rust.svg"),
        "sass" => include_bytes!("../../Resources/FileIcons/sass.svg"),
        "svelte" => include_bytes!("../../Resources/FileIcons/svelte.svg"),
        "swift" => include_bytes!("../../Resources/FileIcons/swift.svg"),
        "toml" => include_bytes!("../../Resources/FileIcons/toml.svg"),
        "typescript" => include_bytes!("../../Resources/FileIcons/typescript.svg"),
        "vue" => include_bytes!("../../Resources/FileIcons/vue.svg"),
        "xml" => include_bytes!("../../Resources/FileIcons/xml.svg"),
        "yaml" => include_bytes!("../../Resources/FileIcons/yaml.svg"),
        _ => return None,
    })
}
