//! File glyphs are drawn by Rune's icon atlas on the first visible frame.
use crate::{
    funnel::model::{DocumentKind, FileType},
    theme::{Era, Look},
};
use rune::{El, Icon, icon};
use std::path::Path;

macro_rules! outline {
    ($name:literal, $path:literal) => {
        &Icon { name: concat!("crawler-file-", $name), view: 24.0,
            body: concat!(r#"<path fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round" d=""#, $path, r#""/>"#) }
    };
}

macro_rules! material {
    ($name:literal, $view:literal) => {
        &Icon {
            name: concat!("crawler-lang-", $name),
            view: $view,
            body: include_str!(concat!("../../Resources/FileIcons/", $name, ".svg")),
        }
    };
}

pub fn file(path: &Path, kind: DocumentKind, look: Look, selected: bool, size: f32) -> El {
    use DocumentKind::*;
    let glyph = match kind {
        Folder => match look.era() {
            Era::Classic => outline!("classic-folder", "M2 20V5h7l2 2h11v13z M2 10h20 M5 17h14"),
            Era::WindowsBevel | Era::UnixBevel => {
                outline!("open-folder", "M2 19V4h7l3 3h8v3 M2 19l3-9h18l-3 9z")
            }
            Era::Terminal => outline!(
                "terminal-folder",
                "M2 20V5h7l2 2h11v13z M6 11l3 3-3 3 M12 17h5"
            ),
            Era::Modern => outline!(
                "folder",
                "M2 8V6q0-2 2-2h5l3 3h8q2 0 2 2v9q0 2-2 2H4q-2 0-2-2z M3 10h18"
            ),
        },
        Pdf => outline!(
            "pdf",
            "M5 2h9l5 5v15H5z M14 2v6h5 M8 17q4-5 4-8 0 5 4 7-5-2-8 1z"
        ),
        Word | Other(FileType::Pages) => {
            outline!("word", "M4 2h10l6 6v14H4z M14 2v6h6 M7 11l2 7 3-5 2 5 3-7")
        }
        Sheet | Other(FileType::Numbers) => {
            outline!("sheet", "M3 3h18v18H3z M3 9h18 M3 15h18 M9 9v12 M15 9v12")
        }
        Image => outline!("image", "M2 4h20v16H2z M3 18l6-7 5 5 3-4 4 5 M17 8h.5"),
        Video => outline!(
            "video",
            "M2 5h20v14H2z M2 8h3 M2 12h3 M2 16h3 M19 8h3 M19 12h3 M19 16h3 M10 8l6 4-6 4z"
        ),
        Audio => outline!(
            "audio",
            "M9 17V6l11-3v12 M9 9l11-3 M9 17c0 4-7 5-7 2s7-5 7-2z M20 15c0 4-7 5-7 2s7-5 7-2z"
        ),
        Package => outline!(
            "package",
            "M2 7l10-5 10 5v11l-10 5-10-5z M2 7l10 5 10-5 M12 12v11 M7 4.5l10 5V13"
        ),
        Other(FileType::Archive) => outline!(
            "archive",
            "M4 2h10l6 6v14H4z M14 2v6h6 M8 3h2 M10 6h2 M8 9h2 M10 12h2 M8 15h4v4H8z"
        ),
        Other(FileType::DiskImage) => outline!("disk", "M5 4h14l3 11v5H2v-5z M2 15h20 M16 18h2"),
        Other(FileType::Font) => outline!(
            "font",
            "M2 19L8 4l6 15 M5 13h6 M16 12q5-3 5 2v5 M21 15q-7-1-5 3 1 2 5-1"
        ),
        PowerPoint | Other(FileType::Presentation) => outline!(
            "presentation",
            "M2 3h20v13H2z M12 16v5 M7 22l5-3 5 3 M6 12l4-4 3 2 4-5"
        ),
        Other(FileType::Ebook) => outline!(
            "book",
            "M2 4q5-2 10 1 5-3 10-1v15q-6-2-10 1-5-3-10-1z M12 5v15"
        ),
        Other(FileType::Binary) => outline!(
            "binary",
            "M6 6h12v12H6z M9 2v4 M15 2v4 M9 18v4 M15 18v4 M2 9h4 M2 15h4 M18 9h4 M18 15h4 M10 10h4v4h-4z"
        ),
        Markdown | Text | Other(FileType::Unknown) => {
            language_icon(path).unwrap_or_else(|| match kind {
                Markdown => material!("markdown", 32.0),
                Text if rune_kit::syntax::lang_for_path(&path.to_string_lossy()).is_some() => {
                    outline!("code", "M8 6l-6 6 6 6 M16 6l6 6-6 6 M14 3l-4 18")
                }
                Text => outline!("text", "M5 2h9l5 5v15H5z M14 2v6h5 M8 11h8 M8 14h8 M8 17h5"),
                _ => outline!("document", "M5 2h9l5 5v15H5z M14 2v6h5"),
            })
        }
    };
    let ink = if selected {
        look.selection_text
    } else if look.era() == Era::Classic {
        look.text
    } else {
        look.accent
    };
    icon(glyph, size)
        .key("file-icon")
        .icon_one_color()
        .color(ink)
        .inert()
}

fn language_icon(path: &Path) -> Option<&'static Icon> {
    let name = path.file_name()?.to_str()?;
    let lower;
    let name = if name.bytes().any(|b| b.is_ascii_uppercase()) {
        lower = name.to_ascii_lowercase();
        lower.as_str()
    } else {
        name
    };
    let ext = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    Some(match name {
        "go.mod" | "go.sum" | "go.work" | "go.work.sum" => material!("go-mod", 32.0),
        "dockerfile" => material!("docker", 24.0),
        _ if name.starts_with("dockerfile.") => material!("docker", 24.0),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitconfig" => material!("git", 32.0),
        _ => match ext {
            "ts" | "mts" | "cts" => material!("typescript", 16.0),
            "tsx" => material!("react_ts", 32.0),
            "js" | "mjs" | "cjs" => material!("javascript", 16.0),
            "jsx" => material!("react", 32.0),
            "py" | "pyi" | "pyw" => material!("python", 24.0),
            "rs" => material!("rust", 32.0),
            "md" | "markdown" | "mdx" => material!("markdown", 32.0),
            "json" | "jsonc" | "json5" | "jsonl" => outline!(
                "json",
                "M9 3H7q-2 0-2 2v4q0 3-3 3 3 0 3 3v4q0 2 2 2h2 M15 3h2q2 0 2 2v4q0 3 3 3-3 0-3 3v4q0 2-2 2h-2"
            ),
            "yml" | "yaml" => material!("yaml", 24.0),
            "xml" | "plist" => material!("xml", 24.0),
            "html" | "htm" => material!("html", 32.0),
            "scss" | "sass" => material!("sass", 32.0),
            "sh" | "bash" | "zsh" | "fish" => material!("console", 16.0),
            "c" | "h" => material!("c", 32.0),
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" => material!("cpp", 32.0),
            "cs" => material!("csharp", 32.0),
            "kt" | "kts" => material!("kotlin", 24.0),
            "rb" => material!("ruby", 24.0),
            "sql" => material!("database", 32.0),
            "go" => material!("go", 32.0),
            "swift" => material!("swift", 24.0),
            "css" => material!("css", 32.0),
            "less" => material!("less", 24.0),
            "java" => material!("java", 32.0),
            "php" => material!("php", 24.0),
            "lua" => material!("lua", 32.0),
            "toml" => material!("toml", 16.0),
            "vue" => material!("vue", 24.0),
            "svelte" => material!("svelte", 300.0),
            "astro" => material!("astro", 32.0),
            _ => return None,
        },
    })
}
