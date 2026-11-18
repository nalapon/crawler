use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// Lexical identities deliberately retain symlinks. Resolving them would break navigation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FolderID(pub PathBuf);
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemID(pub PathBuf);

pub fn standardized(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        crate::app::home().join(path)
    };
    let mut out = PathBuf::from("/");
    for part in absolute.components() {
        match part {
            Component::Normal(s) => out.push(s),
            Component::ParentDir => {
                out.pop();
            }
            _ => {}
        }
    }
    out
}
impl FolderID {
    pub fn new(path: &Path) -> Self {
        Self(standardized(path))
    }
}
impl ItemID {
    pub fn new(path: &Path) -> Self {
        Self(standardized(path))
    }
    pub fn key(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

/// What a path is, from its name alone. This is the only extension table: icons' labels, the
/// Enter policy and the preview formats all read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    Folder,
    Pdf,
    Markdown,
    /// Word and the legacy formats converted through it (`.doc`, `.rtf`, `.odt`).
    Word,
    /// Workbooks and delimited text shown as a grid.
    Sheet,
    PowerPoint,
    Text,
    Image,
    Video,
    Audio,
    Package,
    Other(FileType),
}
/// File types that appear in lists and Quick Look but have no full-window reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    Archive,
    DiskImage,
    Font,
    Presentation,
    Ebook,
    Binary,
    Pages,
    Numbers,
    Unknown,
}
impl DocumentKind {
    pub fn for_path(path: &Path, directory: bool) -> Self {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let lower;
        let ext = if ext.bytes().any(|b| b.is_ascii_uppercase()) {
            lower = ext.to_ascii_lowercase();
            lower.as_str()
        } else {
            ext
        };
        if directory {
            return if matches!(
                ext,
                "app" | "bundle" | "framework" | "plugin" | "rtfd" | "pages" | "numbers" | "key"
            ) {
                Self::Package
            } else {
                Self::Folder
            };
        }
        match ext {
            "pdf" => Self::Pdf,
            "md" | "markdown" | "mdown" | "mkd" => Self::Markdown,
            "docx" | "docm" | "dotx" | "doc" | "rtf" | "odt" => Self::Word,
            "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" | "csv" | "tsv" => Self::Sheet,
            "pptx" => Self::PowerPoint,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "tif" | "tiff" | "bmp"
            | "ico" | "svg" | "avif" => Self::Image,
            "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "mpg" | "mpeg" | "mts" | "m2ts"
            | "ogv" | "wmv" | "flv" => Self::Video,
            "mp3" | "m4a" | "wav" | "flac" | "ogg" | "aiff" => Self::Audio,
            "txt" | "text" | "log" | "ini" | "conf" | "cfg" | "env" | "mod" | "sum" => Self::Text,
            "zip" | "tar" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "tgz" | "pkg" => {
                Self::Other(FileType::Archive)
            }
            "dmg" | "iso" | "img" | "sparseimage" | "sparsebundle" => {
                Self::Other(FileType::DiskImage)
            }
            "otf" | "ttf" | "ttc" | "woff" | "woff2" => Self::Other(FileType::Font),
            "ppt" | "pps" | "ppsx" | "odp" | "key" => Self::Other(FileType::Presentation),
            "epub" | "mobi" | "azw" | "azw3" => Self::Other(FileType::Ebook),
            "o" | "a" | "so" | "dylib" | "dll" | "exe" | "rlib" | "rmeta" => {
                Self::Other(FileType::Binary)
            }
            "pages" => Self::Other(FileType::Pages),
            "numbers" => Self::Other(FileType::Numbers),
            _ if rune_kit::syntax::lang_for_path(&path.to_string_lossy()).is_some() => Self::Text,
            _ if matches!(
                path.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str(),
                "makefile"
                    | "dockerfile"
                    | "license"
                    | "readme"
                    | "gemfile"
                    | ".env"
                    | ".gitignore"
                    | ".editorconfig"
                    | "go.work"
                    | "go.work.sum"
            ) =>
            {
                Self::Text
            }
            _ => Self::Other(FileType::Unknown),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Folder => "Folder",
            Self::Pdf => "PDF document",
            Self::Markdown => "Markdown",
            Self::Word => "Word document",
            Self::Sheet => "Spreadsheet",
            Self::Text | Self::Other(FileType::Unknown) => "File",
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Other(FileType::Archive) => "Archive",
            Self::Other(FileType::DiskImage) => "Disk image",
            Self::Other(FileType::Font) => "Font",
            Self::PowerPoint | Self::Other(FileType::Presentation) => "Presentation",
            Self::Other(FileType::Ebook) => "E-book",
            Self::Other(FileType::Binary) => "Binary",
            Self::Other(FileType::Pages) => "Pages document",
            Self::Other(FileType::Numbers) => "Spreadsheet",
            Self::Package => "Application or package",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: ItemID,
    pub kind: DocumentKind,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub symlink: bool,
    pub hidden: bool,
    pub highlights: Vec<std::ops::Range<usize>>,
    pub path_highlights: Vec<std::ops::Range<usize>>,
}
impl Item {
    pub fn read(path: &Path) -> Result<Self, String> {
        Self::read_entry(standardized(path))
    }

    pub fn name(&self) -> std::borrow::Cow<'_, str> {
        self.id
            .0
            .file_name()
            .unwrap_or(self.id.0.as_os_str())
            .to_string_lossy()
    }

    pub(super) fn read_entry(path: PathBuf) -> Result<Self, String> {
        use std::os::macos::fs::MetadataExt;
        let link = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        let symlink = link.is_symlink();
        // The same rule as NSURLIsHiddenKey, read from the lstat already in hand.
        let hidden = link.st_flags() & libc::UF_HIDDEN != 0
            || path
                .file_name()
                .is_some_and(|n| n.as_encoded_bytes().starts_with(b"."));
        let meta = if symlink {
            std::fs::metadata(&path).ok()
        } else {
            Some(link)
        };
        let dir = meta.as_ref().is_some_and(|m| m.is_dir());
        let mut kind = DocumentKind::for_path(&path, dir);
        if kind == DocumentKind::Folder && crate::opening::platform::is_package(&path) {
            kind = DocumentKind::Package;
        }
        Ok(Self {
            id: ItemID(path),
            kind,
            size: meta.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
            modified: meta.as_ref().and_then(|m| m.modified().ok()),
            created: meta.as_ref().and_then(|m| m.created().ok()),
            symlink,
            hidden,
            highlights: Vec::new(),
            path_highlights: Vec::new(),
        })
    }
    pub fn from_hit(hit: &crate::index::Hit) -> Self {
        Self {
            id: ItemID::new(&hit.path),
            kind: DocumentKind::for_path(&hit.path, hit.is_dir),
            size: None,
            modified: None,
            created: None,
            symlink: false,
            hidden: false,
            highlights: hit.name_highlights.clone(),
            path_highlights: hit.dir_highlights.clone(),
        }
    }
}
pub fn name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
pub fn size_label(size: Option<u64>) -> String {
    use objc2_foundation::{NSByteCountFormatter, NSByteCountFormatterCountStyle};
    size.and_then(|n| i64::try_from(n).ok()).map_or_else(
        || "—".into(),
        |n| {
            NSByteCountFormatter::stringFromByteCount_countStyle(
                n,
                NSByteCountFormatterCountStyle::File,
            )
            .to_string()
            .replace("KB", "K")
            .replace("MB", "M")
            .replace("GB", "G")
            .replace("TB", "T")
        },
    )
}

pub fn display_path(path: &Path) -> String {
    path.strip_prefix(crate::app::home()).map_or_else(
        |_| path.display().to_string(),
        |relative| {
            if relative.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", relative.display())
            }
        },
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortField {
    #[default]
    Name,
    Modified,
    Created,
    Kind,
    Size,
}
impl SortField {
    pub const ALL: [Self; 5] = [
        Self::Name,
        Self::Modified,
        Self::Created,
        Self::Kind,
        Self::Size,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Modified => "Modified",
            Self::Created => "Created",
            Self::Kind => "Kind",
            Self::Size => "Size",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sort {
    pub field: SortField,
    pub descending: bool,
}
impl Sort {
    pub fn order(
        self,
        items: &[Item],
        sizes: Option<&std::collections::HashMap<ItemID, u64>>,
    ) -> Vec<usize> {
        use std::cmp::Ordering;
        fn optional<T: Ord>(a: Option<T>, b: Option<T>, desc: bool) -> Ordering {
            match (a, b) {
                (Some(a), Some(b)) => {
                    if desc {
                        b.cmp(&a)
                    } else {
                        a.cmp(&b)
                    }
                }
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (None, None) => Ordering::Equal,
            }
        }
        let size = |i: &Item| i.size.or_else(|| sizes?.get(&i.id).copied());
        let mut order: Vec<_> = (0..items.len()).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&items[a], &items[b]);
            let folders = (b.kind == DocumentKind::Folder).cmp(&(a.kind == DocumentKind::Folder));
            if folders != Ordering::Equal {
                return folders;
            }
            // Library snapshots already use natural name order. The stable sort preserves
            // that order when the requested field ties.
            match self.field {
                SortField::Name => {
                    let order = rune_kit::natural_cmp(&a.name(), &b.name());
                    if self.descending {
                        order.reverse()
                    } else {
                        order
                    }
                }
                SortField::Modified => optional(a.modified, b.modified, self.descending),
                SortField::Created => optional(a.created, b.created, self.descending),
                SortField::Size => optional(size(a), size(b), self.descending),
                SortField::Kind => {
                    let order = rune_kit::natural_cmp(a.kind.label(), b.kind.label());
                    if self.descending {
                        order.reverse()
                    } else {
                        order
                    }
                }
            }
        });
        order
    }
}
