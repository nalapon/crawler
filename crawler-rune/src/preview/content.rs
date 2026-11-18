use super::{
    native, presentation,
    sheet::{self, Workbook},
    word,
};
use crate::funnel::model::DocumentKind;
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use rune::{Image, ImageCache};
use rune_kit::syntax::{self, Highlight};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
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

/// How a file is drawn. `DocumentKind` owns the extension table; this only groups its kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pdf,
    Markdown,
    Text,
    Word,
    Sheet,
    PowerPoint,
    Image,
    Video,
    Thumbnail,
}
impl Format {
    pub fn of(path: &Path) -> Self {
        Self::from_kind(DocumentKind::for_path(path, false))
    }
    fn from_kind(kind: DocumentKind) -> Self {
        match kind {
            DocumentKind::Pdf => Self::Pdf,
            DocumentKind::Markdown => Self::Markdown,
            DocumentKind::Word => Self::Word,
            DocumentKind::Sheet => Self::Sheet,
            DocumentKind::PowerPoint => Self::PowerPoint,
            DocumentKind::Text => Self::Text,
            DocumentKind::Image => Self::Image,
            DocumentKind::Video => Self::Video,
            DocumentKind::Folder
            | DocumentKind::Audio
            | DocumentKind::Package
            | DocumentKind::Other(_) => Self::Thumbnail,
        }
    }
    /// Whether the full-window document reader shows it. PDFs have their own reader.
    pub fn readable(self) -> bool {
        !matches!(self, Self::Pdf | Self::Thumbnail)
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
    fn bytes(self, format: Format) -> usize {
        match (self, format) {
            (Self::Preview, _) => 256 * 1024,
            // Plain text and delimited rows cost little per line, so the reader takes more.
            (Self::Reader, Format::Text | Format::Sheet) => 16 * 1024 * 1024,
            (Self::Reader, _) => 4 * 1024 * 1024,
        }
    }
    fn lines(self, format: Format) -> usize {
        match (self, format) {
            (Self::Preview, _) => 5_000,
            (Self::Reader, Format::Text | Format::Sheet) => usize::MAX,
            (Self::Reader, _) => 100_000,
        }
    }
    pub fn notice(self, format: Format) -> &'static str {
        match (self, format) {
            (Self::Preview, Format::Sheet) => "Partial preview · first 200 rows and 40 columns",
            (Self::Preview, _) => "Partial preview · limited to 256 KB and 5,000 lines",
            (Self::Reader, Format::Sheet) => {
                "Partial sheet · limited to 200,000 rows, 512 columns and 4 million cells"
            }
            (Self::Reader, Format::Text) => "Partial document · limited to 16 MB",
            (Self::Reader, _) => "Partial document · limited to 4 MB and 100,000 lines",
        }
    }
}

pub struct Heading {
    /// The GitHub-style anchor `#fragment` links scroll to.
    pub slug: String,
    pub title: String,
    pub level: u8,
}
/// One top-level Markdown block, rendered as one virtual row.
pub struct Block {
    source: String,
    definitions: Vec<Arc<str>>,
    pub heading: Option<Heading>,
}

impl Block {
    // Rune parses each visible block independently. Only add definitions this block uses.
    pub fn markdown(&self) -> Cow<'_, str> {
        if self.definitions.is_empty() {
            return Cow::Borrowed(&self.source);
        }
        let mut text = self.source.clone();
        for definition in &self.definitions {
            text.push_str("\n\n");
            text.push_str(definition);
        }
        Cow::Owned(text)
    }
}

/// Code or plain text: one buffer, the offset where each line starts and one sorted list of
/// syntax colours. A line costs four bytes, not an allocation.
pub struct Text {
    source: String,
    starts: Vec<u32>,
    highlights: Vec<Highlight>,
}
impl Text {
    /// Characters of one line that are drawn; a longer line (minified code) is cut.
    const LINE_BYTES: usize = 4096;

    fn new(source: String, language: Option<&str>, cancelled: &impl Fn() -> bool) -> Self {
        let mut starts = vec![0];
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(at, _)| at as u32 + 1),
        );
        // Rune colours the first 128 KiB of a source; the rest stays plain.
        let highlights = if cancelled() {
            Vec::new()
        } else {
            syntax::highlight(language, &source).to_vec()
        };
        Self {
            source,
            starts,
            highlights,
        }
    }
    pub fn lines(&self) -> usize {
        self.starts.len()
    }
    fn range(&self, line: usize) -> Range<usize> {
        let start = self.starts[line] as usize;
        let end = self
            .starts
            .get(line + 1)
            .map_or(self.source.len(), |next| *next as usize - 1);
        let end = end.min(start + Self::LINE_BYTES);
        let mut end = end;
        while !self.source.is_char_boundary(end) {
            end -= 1;
        }
        start..end
    }
    pub fn line(&self, line: usize) -> &str {
        self.source[self.range(line)].trim_end_matches('\r')
    }
    /// The line's syntax colours, with ranges relative to its start.
    pub fn highlights(&self, line: usize) -> Vec<Highlight> {
        let Range { start, end } = self.range(line);
        let first = self.highlights.partition_point(|(r, _)| r.end <= start);
        self.highlights[first..]
            .iter()
            .take_while(|(r, _)| r.start < end)
            .filter_map(|(r, paint)| {
                let a = r.start.max(start);
                let b = r.end.min(end);
                (a < b).then(|| (a - start..b - start, *paint))
            })
            .collect()
    }
}

pub enum Body {
    /// Pictures, video and thumbnails carry no rows.
    None,
    Blocks(Vec<Block>),
    Text(Text),
    Sheet(Workbook),
    Slide {
        index: usize,
        count: usize,
        image: Image,
    },
}
pub struct Document {
    pub path: PathBuf,
    pub format: Format,
    pub bytes: Option<u64>,
    pub modified: Option<SystemTime>,
    pub body: Body,
    pub truncated: bool,
    pub budget: Budget,
    pub images: Vec<(usize, String, PathBuf)>,
    pub retained_bytes: usize,
    pub thumbnail: Option<Image>,
    pub kind: DocumentKind,
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
    /// Markdown blocks, text lines, sheet rows or presentation slides.
    pub fn rows(&self) -> usize {
        match &self.body {
            Body::None => 0,
            Body::Blocks(blocks) => blocks.len(),
            Body::Text(text) => text.lines(),
            Body::Sheet(book) => book.sheet.rows(),
            Body::Slide { count, .. } => *count,
        }
    }
    pub fn blocks(&self) -> &[Block] {
        match &self.body {
            Body::Blocks(blocks) => blocks,
            _ => &[],
        }
    }
}
#[derive(Clone)]
pub struct Request {
    pub path: PathBuf,
    pub budget: Budget,
    /// The workbook sheet or presentation slide to load.
    pub part: usize,
    pub revision: u64,
    pub generation: Arc<AtomicU64>,
    pub candidate: Option<Arc<Document>>,
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
    if let Some(candidate) = &request.candidate
        && metadata.is_file()
        && candidate.path == request.path
        && candidate.bytes == Some(metadata.len())
        && candidate.modified.is_some()
        && candidate.modified == metadata.modified().ok()
    {
        return Ok(candidate.clone());
    }
    let kind = DocumentKind::for_path(&request.path, false);
    let format = Format::from_kind(kind);
    if !(metadata.is_file() || metadata.is_dir() && format == Format::Thumbnail) {
        return Err("This item is not a regular file.".into());
    }
    let mut document = Document {
        path: request.path.clone(),
        format,
        bytes: metadata.is_file().then_some(metadata.len()),
        modified: metadata.modified().ok(),
        body: Body::None,
        truncated: false,
        budget: request.budget,
        images: Vec::new(),
        retained_bytes: 0,
        thumbnail: None,
        kind: if metadata.is_dir() {
            DocumentKind::Package
        } else {
            kind
        },
        fenced: Arc::default(),
        revision: request.revision,
    };
    let kind = extension(&request.path);
    match format {
        Format::Markdown => {
            let (source, truncated) = read_text(file, request.budget, format, &cancelled)?;
            document.truncated = truncated;
            prepare_markdown(&mut document, &source, &request)?;
        }
        Format::Text => {
            let (source, truncated) = read_text(file, request.budget, format, &cancelled)?;
            document.truncated = truncated;
            let path = request.path.to_string_lossy();
            document.body = Body::Text(Text::new(source, syntax::lang_for_path(&path), &cancelled));
        }
        Format::Word if request.budget == Budget::Reader => {
            let source = if kind == "docx" || kind == "docm" || kind == "dotx" {
                word::markdown(&request.path, &cancelled)?
            } else {
                word::legacy(&request.path, &cancelled)?
            };
            let (source, truncated) = bound_text(source, request.budget, format);
            document.truncated = truncated;
            prepare_markdown(&mut document, &source, &request)?;
        }
        Format::Sheet if kind == "csv" || kind == "tsv" => {
            let (source, cut) = read_text(file, request.budget, format, &cancelled)?;
            let (book, truncated) =
                sheet::delimited(&source, kind == "tsv", sheet::Limits::of(request.budget));
            document.truncated = cut || truncated;
            document.body = Body::Sheet(book);
        }
        // Only the streaming formats are cheap enough to parse for a pane preview.
        Format::Sheet
            if request.budget == Budget::Reader
                || matches!(kind.as_str(), "xlsx" | "xlsm" | "xlsb") =>
        {
            let (book, truncated) = sheet::workbook(
                &request.path,
                request.part,
                sheet::Limits::of(request.budget),
                &cancelled,
            )?;
            document.truncated = truncated;
            document.body = Body::Sheet(book);
        }
        Format::Image => {
            // The pane never draws more than its own width; the reader keeps detail for zoom.
            let pixels = match request.budget {
                Budget::Preview => 1280,
                Budget::Reader => 3072,
            };
            // ImageIO has no SVG decoder; Quick Look draws those.
            if let Some(bitmap) = native::image(&request.path, pixels)
                .ok()
                .or_else(|| native::thumbnail(&request.path, &cancelled))
            {
                document.thumbnail = Some(decoded_image(&request.cache, bitmap));
            }
        }
        Format::PowerPoint if request.budget == Budget::Reader => {
            let (bitmap, count) = presentation::render(file, request.part, &cancelled)?;
            document.body = Body::Slide {
                index: request.part,
                count,
                image: decoded_image(&request.cache, bitmap),
            };
        }
        Format::Thumbnail | Format::Word | Format::Sheet | Format::PowerPoint => {
            if let Some(bitmap) = native::thumbnail(&request.path, &cancelled) {
                document.thumbnail = Some(decoded_image(&request.cache, bitmap));
            }
        }
        Format::Pdf | Format::Video => {}
    }
    if cancelled() {
        return Err("Cancelled".into());
    }
    if request.budget == Budget::Preview {
        // Charge decoded pixels, including mipmaps, and owned row buffers. Only pane documents
        // enter the navigation cache. A conservative charge leaves room for allocator overhead.
        let pixels = document
            .thumbnail
            .as_ref()
            .and_then(Image::size)
            .map_or(0, |[w, h]| w as usize * h as usize * 8);
        let body = match &document.body {
            Body::None | Body::Slide { .. } => 0,
            Body::Text(t) => {
                t.source.capacity()
                    + 4 * t.starts.capacity()
                    + size_of::<Highlight>() * t.highlights.capacity()
            }
            Body::Sheet(book) => book.retained_bytes(),
            Body::Blocks(blocks) => {
                blocks.capacity() * size_of::<Block>()
                    + blocks
                        .iter()
                        .map(|b| {
                            b.source.capacity()
                                + b.definitions.capacity() * size_of::<Arc<str>>()
                                + b.definitions.iter().map(|s| s.len()).sum::<usize>()
                                + b.heading
                                    .as_ref()
                                    .map_or(0, |h| h.slug.capacity() + h.title.capacity())
                        })
                        .sum::<usize>()
                    + document
                        .fenced
                        .iter()
                        .map(|(k, v)| k.capacity() + v.capacity() * size_of::<Highlight>() + 64)
                        .sum::<usize>()
            }
        };
        document.retained_bytes = 512
            + document.path.as_os_str().len()
            + pixels
            + body
            + document
                .images
                .iter()
                .map(|(_, s, p)| 96 + s.capacity() + p.as_os_str().len())
                .sum::<usize>();
    }
    Ok(Arc::new(document))
}

fn read_text(
    mut file: File,
    budget: Budget,
    format: Format,
    cancelled: &impl Fn() -> bool,
) -> Result<(String, bool), String> {
    let limit = budget.bytes(format);
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
    let (source, bounded) = bound_text(source, budget, format);
    Ok((source, truncated || bounded))
}
fn bound_text(mut source: String, budget: Budget, format: Format) -> (String, bool) {
    let mut truncated = false;
    if source.len() > budget.bytes(format) {
        let mut end = budget.bytes(format);
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        source.truncate(end);
        truncated = true;
    }
    if let Some((end, _)) = source
        .match_indices('\n')
        .nth(budget.lines(format).saturating_sub(1))
    {
        if end + 1 < source.len() {
            source.truncate(end + 1);
            truncated = true;
        }
    }
    (source, truncated)
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
    let mut parser = Parser::new_ext(&sanitized, options()).into_offset_iter();
    let mut shared_definitions = HashMap::<usize, Arc<str>>::new();
    let mut used_definitions = Vec::<Arc<str>>::new();
    let mut definition_starts = HashSet::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut heading: Option<u8> = None;
    let mut heading_text = String::new();
    let mut blocks = Vec::new();
    let mut slugs: HashMap<String, usize> = HashMap::new();
    let mut fenced = HashMap::new();
    let mut code_language = None;
    let mut code_text = String::new();

    while let Some((event, range)) = parser.next() {
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
                if let Tag::Heading { level, .. } = tag {
                    heading = Some(*level as u8);
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
                if let Tag::Link { link_type, id, .. } | Tag::Image { link_type, id, .. } = tag
                    && matches!(
                        link_type,
                        LinkType::Shortcut | LinkType::Collapsed | LinkType::Reference
                    )
                    && let Some(def) = parser.reference_definitions().get(id)
                    && definition_starts.insert(def.span.start)
                {
                    let shared = shared_definitions
                        .entry(def.span.start)
                        .or_insert_with(|| Arc::from(&sanitized[def.span.clone()]));
                    used_definitions.push(shared.clone());
                }
                if let Tag::Image { dest_url, .. } = tag
                    && doc.images.len() < 48
                    && let Some(path) = local_path(&doc.path, dest_url)
                {
                    doc.images.push((blocks.len(), dest_url.to_string(), path));
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
                    definition_starts.clear();
                    let heading = heading.take().map(|level| {
                        let slug = slug(&heading_text);
                        let count = slugs.entry(slug.clone()).or_default();
                        let slug = if *count == 0 {
                            slug
                        } else {
                            format!("{slug}-{count}")
                        };
                        *count += 1;
                        Heading {
                            slug,
                            title: heading_text.trim().to_owned(),
                            level,
                        }
                    });
                    blocks.push(Block {
                        source: sanitized[start..range.end].to_owned(),
                        definitions: std::mem::take(&mut used_definitions),
                        heading,
                    });
                }
            }
            Event::Rule if depth == 0 => blocks.push(Block {
                source: sanitized[range].to_owned(),
                definitions: Vec::new(),
                heading: None,
            }),
            _ => {}
        }
    }
    doc.body = Body::Blocks(blocks);
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
/// Case-insensitive matches, at most 10,000. `block` is the row: a Markdown block, a text
/// line or a sheet row (whose `ranges` stay empty: the whole row is the match).
pub fn find(doc: &Document, needle: &str) -> Vec<Found> {
    if needle.is_empty() {
        return Vec::new();
    }
    match &doc.body {
        Body::None | Body::Slide { .. } => Vec::new(),
        Body::Blocks(blocks) => blocks
            .iter()
            .enumerate()
            .flat_map(|(block, b)| {
                rune_kit::markdown::find(
                    &b.markdown(),
                    needle,
                    rune_kit::markdown::Case::Insensitive,
                )
                .into_iter()
                .map(move |ranges| Found { block, ranges })
            })
            .take(10_000)
            .collect(),
        Body::Text(text) => (0..text.lines())
            .flat_map(|line| {
                occurrences(text.line(line), needle).map(move |range| Found {
                    block: line,
                    ranges: vec![range],
                })
            })
            .take(10_000)
            .collect(),
        Body::Sheet(book) => (0..book.sheet.rows())
            .filter(|row| {
                (0..book.sheet.columns).any(|column| {
                    occurrences(book.sheet.cell(*row, column), needle)
                        .next()
                        .is_some()
                })
            })
            .map(|block| Found {
                block,
                ranges: Vec::new(),
            })
            .take(10_000)
            .collect(),
    }
}
/// Byte ranges of `needle` in `text`, ignoring ASCII case.
fn occurrences<'a>(text: &'a str, needle: &'a str) -> impl Iterator<Item = Range<usize>> + 'a {
    let (haystack, needle) = (text.as_bytes(), needle.as_bytes());
    let mut at = 0;
    std::iter::from_fn(move || {
        while at + needle.len() <= haystack.len() {
            let start = at;
            at += 1;
            if haystack[start..start + needle.len()].eq_ignore_ascii_case(needle) {
                at = start + needle.len();
                return Some(start..at);
            }
        }
        None
    })
}

pub fn decoded_image(cache: &ImageCache, bitmap: native::Bitmap) -> Image {
    let image = cache.rgba(bitmap.width, bitmap.height, bitmap.rgba);
    let _ = image.wait();
    image
}
