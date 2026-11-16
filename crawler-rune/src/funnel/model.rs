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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    Folder,
    Pdf,
    Markdown,
    Word,
    Text,
    Image,
    Video,
    Audio,
    Package,
    Other,
}
impl DocumentKind {
    pub fn for_path(path: &Path, directory: bool) -> Self {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if directory {
            return if matches!(
                ext.as_str(),
                "app" | "bundle" | "framework" | "plugin" | "rtfd" | "pages" | "numbers" | "key"
            ) {
                Self::Package
            } else {
                Self::Folder
            };
        }
        match ext.as_str() {
            "pdf" => Self::Pdf,
            "md" | "markdown" | "mdown" => Self::Markdown,
            "docx" | "docm" | "dotx" => Self::Word,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "tif" | "tiff" | "bmp" | "svg"
            | "avif" => Self::Image,
            "mp4" | "mov" | "m4v" | "mkv" | "avi" | "webm" | "mpeg" | "mpg" => Self::Video,
            "mp3" | "m4a" | "wav" | "flac" | "ogg" | "aiff" => Self::Audio,
            "txt" | "text" | "log" | "rs" | "swift" | "go" | "mod" | "sum" | "js" | "jsx"
            | "ts" | "tsx" | "json" | "yaml" | "yml" | "toml" | "xml" | "html" | "css" | "scss"
            | "py" | "rb" | "c" | "h" | "cpp" | "hpp" | "java" | "kt" | "sh" | "zsh" | "bash"
            | "sql" | "conf" | "ini" | "env" | "dockerfile" | "makefile" => Self::Text,
            _ if matches!(
                path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                "Makefile" | "Dockerfile" | "LICENSE" | "Gemfile" | "go.mod" | "go.sum"
            ) =>
            {
                Self::Text
            }
            _ => Self::Other,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Folder => "Folder",
            Self::Pdf => "PDF document",
            Self::Markdown => "Markdown",
            Self::Word => "Word document",
            Self::Text | Self::Other => "File",
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Package => "Application or package",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: ItemID,
    pub name: String,
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
        let path = standardized(path);
        let link = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        let symlink = link.is_symlink();
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
            name: name(&path),
            id: ItemID::new(&path),
            kind,
            size: meta.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
            modified: meta.as_ref().and_then(|m| m.modified().ok()),
            created: meta.as_ref().and_then(|m| m.created().ok()),
            symlink,
            hidden: crate::opening::platform::is_hidden(&path),
            highlights: Vec::new(),
            path_highlights: Vec::new(),
        })
    }
    pub fn from_hit(hit: &crate::index::Hit) -> Self {
        Self {
            id: ItemID::new(&hit.path),
            name: name(&hit.path),
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
    match size {
        None => "—".into(),
        Some(n) if n < 1000 => format!("{n} B"),
        Some(n) if n < 1_000_000 => format!("{:.1} KB", n as f64 / 1000.0),
        Some(n) if n < 1_000_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        Some(n) => format!("{:.1} GB", n as f64 / 1e9),
    }
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
    pub fn apply(self, items: &mut [Item]) {
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
        items.sort_by(|a, b| {
            let folders = (b.kind == DocumentKind::Folder).cmp(&(a.kind == DocumentKind::Folder));
            if folders != Ordering::Equal {
                return folders;
            }
            let by = match self.field {
                SortField::Name => {
                    let order = rune_kit::natural_cmp(&a.name, &b.name);
                    if self.descending {
                        order.reverse()
                    } else {
                        order
                    }
                }
                SortField::Modified => optional(a.modified, b.modified, self.descending),
                SortField::Created => optional(a.created, b.created, self.descending),
                SortField::Size => optional(a.size, b.size, self.descending),
                SortField::Kind => {
                    let order = rune_kit::natural_cmp(a.kind.label(), b.kind.label());
                    if self.descending {
                        order.reverse()
                    } else {
                        order
                    }
                }
            };
            by.then_with(|| rune_kit::natural_cmp(&a.name, &b.name))
        });
    }
}
