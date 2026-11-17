//! Read-only Word Open XML conversion. Package entries are bounded before XML parsing.

use roxmltree::{Document, Node};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::Path,
};
use zip::ZipArchive;

const XML_LIMIT: u64 = 24 * 1024 * 1024;

pub fn markdown(path: &Path, cancelled: &impl Fn() -> bool) -> Result<String, String> {
    let mut zip =
        ZipArchive::new(File::open(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let body =
        part(&mut zip, "word/document.xml")?.ok_or("This Word document has no document body.")?;
    let styles = part(&mut zip, "word/styles.xml")?.unwrap_or_default();
    let numbering = part(&mut zip, "word/numbering.xml")?.unwrap_or_default();
    let relationships = part(&mut zip, "word/_rels/document.xml.rels")?.unwrap_or_default();
    if cancelled() {
        return Err("Cancelled".into());
    }
    let doc = parse_xml(&body).map_err(|e| format!("Unreadable Word document: {e}"))?;
    let mut converter = Converter {
        headings: HashMap::new(),
        bullets: HashSet::new(),
        links: HashMap::new(),
        output: String::new(),
        in_list: false,
    };
    if let Ok(styles) = parse_xml(&styles) {
        for style in styles.descendants().filter(|n| named(*n, "style")) {
            let Some(id) = attr(style, "styleId") else {
                continue;
            };
            let name = child(style, "name")
                .and_then(|n| attr(n, "val"))
                .unwrap_or(id)
                .to_lowercase();
            let level = if name == "title" {
                Some(1)
            } else if let Some(n) = name
                .strip_prefix("heading ")
                .and_then(|n| n.parse::<u8>().ok())
            {
                Some((n + 1).min(6))
            } else {
                child(style, "pPr")
                    .and_then(|n| child(n, "outlineLvl"))
                    .and_then(|n| attr(n, "val"))
                    .and_then(|n| n.parse::<u8>().ok())
                    .filter(|n| *n < 9)
                    .map(|n| (n + 2).min(6))
            };
            if let Some(level) = level {
                converter.headings.insert(id.to_string(), level);
            }
        }
    }
    if let Ok(numbering) = parse_xml(&numbering) {
        let mut abstracts: HashMap<&str, Vec<&str>> = HashMap::new();
        for n in numbering.descendants().filter(|n| named(*n, "abstractNum")) {
            if let Some(id) = attr(n, "abstractNumId") {
                abstracts.insert(
                    id,
                    n.children()
                        .filter(|n| named(*n, "lvl"))
                        .filter(|n| {
                            child(*n, "numFmt").and_then(|n| attr(n, "val")) == Some("bullet")
                        })
                        .filter_map(|n| attr(n, "ilvl"))
                        .collect(),
                );
            }
        }
        for n in numbering.descendants().filter(|n| named(*n, "num")) {
            if let (Some(id), Some(levels)) = (
                attr(n, "numId"),
                child(n, "abstractNumId")
                    .and_then(|n| attr(n, "val"))
                    .and_then(|id| abstracts.get(id)),
            ) {
                for level in levels {
                    converter.bullets.insert(format!("{id}:{level}"));
                }
            }
        }
    }
    if let Ok(rels) = parse_xml(&relationships) {
        for n in rels.descendants().filter(|n| named(*n, "Relationship")) {
            if attr(n, "TargetMode") == Some("External")
                && let (Some(id), Some(target)) = (attr(n, "Id"), attr(n, "Target"))
            {
                converter.links.insert(id.to_owned(), target.to_owned());
            }
        }
    }
    let body = doc
        .descendants()
        .find(|n| named(*n, "body"))
        .ok_or("This Word document has no document body.")?;
    converter.blocks(body, cancelled)?;
    Ok(converter.output)
}

/// `.doc`, `.rtf` and `.odt`: macOS's `textutil` rewrites them as Word Open XML in a temporary
/// folder, and the same conversion as `.docx` reads that. The original is only read.
pub fn legacy(path: &Path, cancelled: &impl Fn() -> bool) -> Result<String, String> {
    let folder = tempfile::tempdir().map_err(|e| e.to_string())?;
    let converted = folder.path().join("converted.docx");
    let output = std::process::Command::new("/usr/bin/textutil")
        .args(["-convert", "docx", "-output"])
        .arg(&converted)
        .arg(path)
        .output()
        .map_err(|e| format!("Could not convert this document: {e}"))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Could not convert this document: {}", reason.trim()));
    }
    markdown(&converted, cancelled)
}

fn part(zip: &mut ZipArchive<File>, name: &str) -> Result<Option<String>, String> {
    let mut entry = match zip.by_name(name) {
        Ok(v) => v,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if entry.size() > XML_LIMIT {
        return Err("A Word document part exceeds the 24 MB read limit.".into());
    }
    let mut data = String::new();
    entry
        .by_ref()
        .take(XML_LIMIT + 1)
        .read_to_string(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > XML_LIMIT {
        return Err("A Word document part exceeds the read limit.".into());
    }
    Ok(Some(data))
}

fn named(n: Node<'_, '_>, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name
}
fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|n| named(*n, name))
}
fn attr<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == name).map(|a| a.value())
}

#[derive(PartialEq, Eq)]
struct Run {
    value: String,
    bold: bool,
    italic: bool,
    strike: bool,
    link: Option<String>,
}
struct Converter {
    headings: HashMap<String, u8>,
    bullets: HashSet<String>,
    links: HashMap<String, String>,
    output: String,
    in_list: bool,
}
impl Converter {
    fn blocks(&mut self, node: Node<'_, '_>, cancelled: &impl Fn() -> bool) -> Result<(), String> {
        for n in node.children().filter(Node::is_element) {
            if cancelled() {
                return Err("Cancelled".into());
            }
            // Keep conversion bounded even for a compressed package containing millions of runs.
            if self.output.len() > 4 * 1024 * 1024 {
                break;
            }
            match n.tag_name().name() {
                "p" => self.paragraph(n),
                "tbl" => self.table(n),
                "sdt" => {
                    if let Some(c) = child(n, "sdtContent") {
                        self.blocks(c, cancelled)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn emit(&mut self, value: &str, list: bool) {
        if !self.output.is_empty() {
            self.output
                .push_str(if list && self.in_list { "\n" } else { "\n\n" });
        }
        self.output.push_str(value);
        self.in_list = list;
    }
    fn paragraph(&mut self, node: Node<'_, '_>) {
        let value = self.inline(node, "\\\n");
        if value.trim().is_empty() {
            return;
        }
        let props = child(node, "pPr");
        if let Some(level) = props
            .and_then(|p| child(p, "pStyle"))
            .and_then(|n| attr(n, "val"))
            .and_then(|id| self.headings.get(id))
        {
            self.emit(&format!("{} {value}", "#".repeat(*level as usize)), false);
        } else if let Some(number) = props.and_then(|p| child(p, "numPr")) {
            let id = child(number, "numId")
                .and_then(|n| attr(n, "val"))
                .unwrap_or("0");
            if id == "0" {
                self.emit(&escape_block(&value), false);
                return;
            }
            let level = child(number, "ilvl")
                .and_then(|n| attr(n, "val"))
                .unwrap_or("0");
            let marker = if self.bullets.contains(&format!("{id}:{level}")) {
                "-"
            } else {
                "1."
            };
            let indent = level.parse::<usize>().unwrap_or(0).min(16);
            self.emit(&format!("{}{marker} {value}", "    ".repeat(indent)), true);
        } else {
            self.emit(&escape_block(&value), false);
        }
    }
    fn table(&mut self, node: Node<'_, '_>) {
        let rows: Vec<Vec<String>> = node
            .children()
            .filter(|n| named(*n, "tr"))
            .map(|row| {
                let mut cells = Vec::new();
                for c in row.children().filter(|n| named(*n, "tc")) {
                    let value = c
                        .descendants()
                        .filter(|n| named(*n, "p"))
                        .map(|p| self.inline(p, " "))
                        .collect::<Vec<_>>()
                        .join(" ");
                    cells.push(value);
                    let span = child(c, "tcPr")
                        .and_then(|n| child(n, "gridSpan"))
                        .and_then(|n| attr(n, "val"))
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(1)
                        .clamp(1, 128);
                    cells.extend((1..span).map(|_| String::new()));
                }
                cells
            })
            .collect();
        let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let mut lines = Vec::new();
        for (i, mut cells) in rows.into_iter().enumerate() {
            cells.resize(cols, String::new());
            lines.push(format!("| {} |", cells.join(" | ")));
            if i == 0 {
                lines.push(format!("|{}", " --- |".repeat(cols)));
            }
        }
        self.emit(&lines.join("\n"), false);
    }
    fn runs(&self, node: Node<'_, '_>, link: Option<&str>, out: &mut Vec<Run>) {
        for n in node.children().filter(Node::is_element) {
            match n.tag_name().name() {
                "r" => {
                    let props = child(n, "rPr");
                    let on = |name| {
                        props
                            .and_then(|p| child(p, name))
                            .is_some_and(|n| !matches!(attr(n, "val"), Some("0" | "false" | "off")))
                    };
                    let mut value = String::new();
                    for c in n.children().filter(Node::is_element) {
                        match c.tag_name().name() {
                            "t" => value.push_str(c.text().unwrap_or("")),
                            "tab" => value.push(' '),
                            "br" | "cr" => value.push('\n'),
                            "noBreakHyphen" => value.push('-'),
                            _ => {}
                        }
                    }
                    if value.is_empty() {
                        continue;
                    }
                    let run = Run {
                        value,
                        bold: on("b"),
                        italic: on("i"),
                        strike: on("strike") || on("dstrike"),
                        link: link.map(str::to_owned),
                    };
                    if let Some(last) = out.last_mut()
                        && last.bold == run.bold
                        && last.italic == run.italic
                        && last.strike == run.strike
                        && last.link == run.link
                    {
                        last.value.push_str(&run.value);
                    } else {
                        out.push(run);
                    }
                }
                "hyperlink" => self.runs(
                    n,
                    attr(n, "id")
                        .and_then(|id| self.links.get(id))
                        .map(String::as_str)
                        .or(link),
                    out,
                ),
                "pPr" | "rPr" | "del" | "tbl" | "p" => {}
                _ => self.runs(n, link, out),
            }
        }
    }
    fn inline(&self, node: Node<'_, '_>, hard_break: &str) -> String {
        let mut runs = Vec::new();
        self.runs(node, None, &mut runs);
        let mut result = String::new();
        for run in runs {
            let core = run.value.trim();
            let prefix = run.value.len() - run.value.trim_start().len();
            result.push_str(&run.value[..prefix]);
            if !core.is_empty() {
                let mut s = escape(core).replace('\n', hard_break);
                if run.bold {
                    s = format!("**{s}**");
                }
                if run.italic {
                    s = format!("*{s}*");
                }
                if run.strike {
                    s = format!("~~{s}~~");
                }
                if let Some(link) = run.link {
                    s = format!("[{s}](<{}>)", link.replace('>', "%3E").replace('<', "%3C"));
                }
                result.push_str(&s);
                result.push_str(&run.value[run.value.trim_end().len()..]);
            }
        }
        result
    }
}
fn escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            if "\\`*_[]<>|".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}
fn escape_block(value: &str) -> String {
    if value.starts_with(['#', '>', '-', '+']) {
        format!("\\{value}")
    } else {
        value.to_owned()
    }
}

fn parse_xml(source: &str) -> Result<Document<'_>, roxmltree::Error> {
    Document::parse_with_options(
        source,
        roxmltree::ParsingOptions {
            nodes_limit: 400_000,
            ..Default::default()
        },
    )
}
