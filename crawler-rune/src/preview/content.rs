use super::{native, word};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use rune::{Image, ImageCache};
use rune_kit::syntax::{self, Highlight};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::Read,
    ops::Range,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pdf,
    Markdown,
    Text,
    Word,
    Image,
    Video,
    Thumbnail,
}
impl Format {
    pub fn of(path: &Path) -> Self {
        let extension = extension(path);
        match extension.as_str() {
            "pdf" => Self::Pdf,
            "md" | "markdown" | "mdown" | "mkd" => Self::Markdown,
            "docx" | "docm" | "dotx" => Self::Word,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "tiff" | "tif" | "bmp"
            | "ico" | "avif" => Self::Image,
            "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "mpg" | "mpeg" | "mts" | "m2ts"
            | "ogv" | "wmv" | "flv" => Self::Video,
            _ if syntax::lang_for_path(&path.to_string_lossy()).is_some()
                || matches!(
                    extension.as_str(),
                    "txt" | "log" | "csv" | "tsv" | "ini" | "conf" | "cfg" | "rtf"
                )
                || matches!(
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str(),
                    "license"
                        | "readme"
                        | ".env"
                        | ".gitignore"
                        | ".editorconfig"
                        | "go.mod"
                        | "go.sum"
                        | "go.work"
                        | "go.work.sum"
                ) =>
            {
                Self::Text
            }
            _ => Self::Thumbnail,
        }
    }
}
pub fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Budget {
    Preview,
    Reader,
}
impl Budget {
    fn bytes(self) -> usize {
        match self {
            Self::Preview => 256 * 1024,
            Self::Reader => 4 * 1024 * 1024,
        }
    }
    fn lines(self) -> usize {
        match self {
            Self::Preview => 5_000,
            Self::Reader => 100_000,
        }
    }
    pub fn notice(self) -> &'static str {
        match self {
            Self::Preview => {
                "Partial preview · limited to 256 KB, 5,000 lines and 4,096 characters per line"
            }
            Self::Reader => "Partial document · limited to 4 MB and 100,000 lines",
        }
    }
}
#[derive(Clone)]
pub struct Block {
    pub source: String,
    pub code: bool,
    pub highlights: Vec<Highlight>,
    pub heading: Option<String>,
}
pub struct Document {
    pub path: PathBuf,
    pub format: Format,
    pub bytes: Option<u64>,
    pub modified: Option<SystemTime>,
    pub blocks: Vec<Block>,
    pub truncated: bool,
    pub budget: Budget,
    pub images: HashMap<String, Image>,
    pub thumbnail: Option<Image>,
    pub icon: Option<Image>,
    pub fenced: Arc<HashMap<String, Vec<Highlight>>>,
    pub revision: u64,
}
impl Document {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }
}
#[derive(Clone)]
pub struct Request {
    pub path: PathBuf,
    pub budget: Budget,
    pub revision: u64,
    pub generation: Arc<AtomicU64>,
    pub cache: Arc<ImageCache>,
}
impl Request {
    fn cancelled(&self) -> bool {
        self.generation.load(Ordering::Relaxed) != self.revision
    }
}

pub fn load(request: Request) -> Result<Arc<Document>, String> {
    let cancelled = || request.cancelled();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&request.path)
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let format = Format::of(&request.path);
    if !(metadata.is_file() || metadata.is_dir() && format == Format::Thumbnail) {
        return Err("This item is not a regular file.".into());
    }
    let mut document = Document {
        path: request.path.clone(),
        format,
        bytes: metadata.is_file().then_some(metadata.len()),
        modified: metadata.modified().ok(),
        blocks: Vec::new(),
        truncated: false,
        budget: request.budget,
        images: HashMap::new(),
        thumbnail: None,
        icon: None,
        fenced: Arc::default(),
        revision: request.revision,
    };
    match format {
        Format::Markdown | Format::Text => {
            let (source, truncated) = read_text(file, request.budget, &cancelled)?;
            document.truncated = truncated;
            if format == Format::Markdown {
                prepare_markdown(&mut document, &source, &request)?;
            } else {
                prepare_text(&mut document, &source, &cancelled)?;
            }
        }
        Format::Word if request.budget == Budget::Reader => {
            let source = word::markdown(&request.path, &cancelled)?;
            let (source, truncated) = bound_text(source, request.budget);
            document.truncated = truncated;
            prepare_markdown(&mut document, &source, &request)?;
        }
        Format::Image => {
            if let Ok(bitmap) = native::image(&request.path, 1920) {
                document.thumbnail = Some(decoded_image(&request.cache, bitmap));
            }
        }
        Format::Thumbnail | Format::Word => {
            if let Some(bitmap) = native::thumbnail(&request.path, &cancelled) {
                document.thumbnail = Some(decoded_image(&request.cache, bitmap));
            }
        }
        Format::Pdf | Format::Video => {}
    }
    if request.budget == Budget::Preview
        && document.thumbnail.is_none()
        && matches!(format, Format::Word | Format::Thumbnail | Format::Image)
    {
        let icon = request.cache.app_icon(&request.path, 64.0);
        let _ = icon.wait();
        document.icon = Some(icon);
    }
    if cancelled() {
        return Err("Cancelled".into());
    }
    Ok(Arc::new(document))
}

fn read_text(
    mut file: File,
    budget: Budget,
    cancelled: &impl Fn() -> bool,
) -> Result<(String, bool), String> {
    let limit = budget.bytes();
    let mut data = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0u8; 64 * 1024];
    while data.len() <= limit {
        if cancelled() {
            return Err("Cancelled".into());
        }
        let count = (limit + 1 - data.len()).min(buffer.len());
        let n = file.read(&mut buffer[..count]).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..n]);
    }
    let truncated = data.len() > limit;
    data.truncate(limit);
    let source = if data.starts_with(&[0xff, 0xfe]) || data.starts_with(&[0xfe, 0xff]) {
        let little = data.starts_with(&[0xff, 0xfe]);
        let mut words: Vec<u16> = data[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if little {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        if truncated && words.last().is_some_and(|c| (0xd800..=0xdbff).contains(c)) {
            words.pop();
        }
        String::from_utf16(&words).map_err(|_| "Unsupported text encoding.")?
    } else {
        if data.starts_with(&[0xef, 0xbb, 0xbf]) {
            data.drain(..3);
        }
        if data.contains(&0) {
            return Err("This file is binary or uses an unsupported text encoding.".into());
        }
        if truncated
            && let Err(e) = std::str::from_utf8(&data)
            && e.error_len().is_none()
        {
            data.truncate(e.valid_up_to());
        }
        String::from_utf8(data)
            .map_err(|_| "This file is binary or uses an unsupported text encoding.")?
    };
    let (source, bounded) = bound_text(source, budget);
    Ok((source, truncated || bounded))
}
fn bound_text(mut source: String, budget: Budget) -> (String, bool) {
    let mut truncated = false;
    if source.len() > budget.bytes() {
        let mut end = budget.bytes();
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        source.truncate(end);
        truncated = true;
    }
    let mut output = String::with_capacity(source.len());
    for (i, line) in source.split_inclusive('\n').enumerate() {
        if i == budget.lines() {
            truncated = true;
            break;
        }
        if budget == Budget::Preview
            && let Some((end, _)) = line.char_indices().nth(4096)
        {
            output.push_str(&line[..end]);
            output.push('\n');
            truncated = true;
        } else {
            output.push_str(line);
        }
    }
    (output, truncated)
}
fn prepare_text(
    doc: &mut Document,
    source: &str,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    let highlights = syntax::highlight(syntax::lang_for_path(&doc.path.to_string_lossy()), source);
    let mut offset = 0;
    let mut token_start = 0;
    for line in source.split('\n') {
        if cancelled() {
            return Err("Cancelled".into());
        }
        let end = offset + line.len();
        while token_start < highlights.len() && highlights[token_start].0.end <= offset {
            token_start += 1;
        }
        let ranges = highlights[token_start..]
            .iter()
            .take_while(|(r, _)| r.start < end)
            .filter_map(|(r, p)| {
                let a = r.start.max(offset);
                let b = r.end.min(end);
                (a < b).then(|| (a - offset..b - offset, *p))
            })
            .collect();
        doc.blocks.push(Block {
            source: line.to_owned(),
            code: true,
            highlights: ranges,
            heading: None,
        });
        offset = end + 1;
    }
    Ok(())
}

fn prepare_markdown(doc: &mut Document, source: &str, request: &Request) -> Result<(), String> {
    // Rewrite only HTML events. Code containing HTML is untouched; other HTML blocks become
    // fenced source, comments disappear, and inline tags are dropped without executing them.
    let mut replacements = Vec::new();
    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        match event {
            Event::InlineHtml(_) => replacements.push((range, String::new())),
            Event::Html(html) => replacements.push((
                range,
                if html.trim_start().starts_with("<!--") {
                    String::new()
                } else {
                    format!("\n~~~~html\n{html}\n~~~~\n")
                },
            )),
            _ => {}
        }
    }
    let mut sanitized = String::with_capacity(source.len());
    let mut previous = 0;
    for (range, value) in replacements {
        if range.start >= previous {
            sanitized.push_str(&source[previous..range.start]);
            sanitized.push_str(&value);
            previous = range.end;
        }
    }
    sanitized.push_str(&source[previous..]);
    let parser = Parser::new_ext(&sanitized, options());
    let references = parser
        .reference_definitions()
        .iter()
        .map(|(name, def)| format!("\n[{name}]: <{}>\n", def.dest))
        .collect::<String>();
    let mut depth = 0usize;
    let mut start = 0;
    let mut heading = None;
    let mut heading_text = String::new();
    let mut slugs: HashMap<String, usize> = HashMap::new();
    let mut fenced = HashMap::new();
    let mut code_language = None;
    let mut code_text = String::new();
    let mut image_paths = Vec::new();
    for (event, range) in parser.into_offset_iter() {
        if request.cancelled() {
            return Err("Cancelled".into());
        }
        match &event {
            Event::Start(tag) => {
                if depth == 0 {
                    start = range.start;
                    heading = None;
                    heading_text.clear();
                }
                if matches!(tag, Tag::Heading { .. }) {
                    heading = Some(String::new());
                }
                if let Tag::CodeBlock(kind) = tag {
                    code_language = Some(match kind {
                        pulldown_cmark::CodeBlockKind::Fenced(lang) => {
                            lang.split_whitespace().next().unwrap_or("").to_owned()
                        }
                        _ => String::new(),
                    });
                    code_text.clear();
                }
                if let Tag::Image { dest_url, .. } = tag
                    && image_paths.len() < 48
                {
                    image_paths.push(dest_url.to_string());
                }
                depth += 1;
            }
            Event::Text(value) | Event::Code(value) => {
                if heading.is_some() {
                    heading_text.push_str(value);
                }
                if code_language.is_some() {
                    code_text.push_str(value);
                }
            }
            Event::End(tag) => {
                if *tag == TagEnd::CodeBlock
                    && let Some(language) = code_language.take()
                {
                    let code = code_text.trim_end_matches(['\n', '\r']);
                    // Rune gives a custom highlighter only its line-aligned 128 KB prefix.
                    let end = if code.len() > 128 * 1024 {
                        code.as_bytes()[..128 * 1024]
                            .iter()
                            .rposition(|b| *b == b'\n')
                            .unwrap_or(0)
                    } else {
                        code.len()
                    };
                    let code = code[..end].trim_end_matches(['\n', '\r']);
                    fenced.insert(
                        code.to_owned(),
                        syntax::highlight(Some(&language), code).to_vec(),
                    );
                }
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if heading.is_some() {
                        let slug = slug(&heading_text);
                        let count = slugs.entry(slug.clone()).or_default();
                        heading = Some(if *count == 0 {
                            slug
                        } else {
                            format!("{slug}-{count}")
                        });
                        *count += 1;
                    }
                    let mut block = sanitized[start..range.end].to_owned();
                    block.push_str(&references);
                    doc.blocks.push(Block {
                        source: block,
                        code: false,
                        highlights: Vec::new(),
                        heading: heading.take(),
                    });
                }
            }
            Event::Rule if depth == 0 => doc.blocks.push(Block {
                source: sanitized[range].to_owned(),
                code: false,
                highlights: Vec::new(),
                heading: None,
            }),
            _ => {}
        }
    }
    for source in image_paths {
        if request.cancelled() {
            return Err("Cancelled".into());
        }
        if doc.images.contains_key(&source) {
            continue;
        }
        if let Some(path) = local_path(&doc.path, &source)
            && let Ok(bitmap) = native::image(&path, 1600)
        {
            doc.images
                .insert(source, decoded_image(&request.cache, bitmap));
        }
    }
    doc.fenced = Arc::new(fenced);
    Ok(())
}
fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
}
pub fn slug(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                Some('-')
            } else if c.is_alphanumeric() || c == '-' || c == '_' {
                Some(c)
            } else {
                None
            }
        })
        .collect()
}
pub fn local_path(base: &Path, destination: &str) -> Option<PathBuf> {
    if destination.starts_with("//") {
        return None;
    }
    let destination = if let Some(file) = destination.strip_prefix("file://") {
        if !file.starts_with('/') {
            return None;
        }
        file
    } else {
        destination
    };
    if destination.contains(':') {
        return None;
    }
    let path = percent_decode(destination.split('#').next().unwrap_or(""))?;
    if path.is_empty() {
        return None;
    }
    Some(base.parent().unwrap_or(Path::new("/")).join(path))
}
pub fn percent_decode(value: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut bytes = value.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let a = (bytes.next()? as char).to_digit(16)?;
            let b = (bytes.next()? as char).to_digit(16)?;
            out.push((a * 16 + b) as u8);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}
pub struct Found {
    pub block: usize,
    pub ranges: Vec<Range<usize>>,
}
pub fn find(doc: &Document, needle: &str) -> Vec<Found> {
    if needle.is_empty() {
        return Vec::new();
    }
    doc.blocks
        .iter()
        .enumerate()
        .flat_map(|(block, b)| {
            rune_kit::markdown::find(&b.source, needle, rune_kit::markdown::Case::Insensitive)
                .into_iter()
                .map(move |ranges| Found { block, ranges })
        })
        .take(10_000)
        .collect()
}

fn decoded_image(cache: &ImageCache, bitmap: native::Bitmap) -> Image {
    let image = cache.rgba(bitmap.width, bitmap.height, bitmap.rgba);
    let _ = image.wait();
    image
}
