//! Render one PowerPoint slide at a time. Quick Look can mix shapes across slides
//! when it receives the whole deck, so each request gets a private one-slide package.
use super::native::{Bitmap, thumbnail_at};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const MANIFEST: &str = "ppt/presentation.xml";
const MANIFEST_LIMIT: u64 = 4 * 1024 * 1024;
const PRESENTATION_NS: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const STRICT_PRESENTATION_NS: &str = "http://purl.oclc.org/ooxml/presentationml/main";

pub(super) fn render(
    file: File,
    index: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<(Bitmap, usize), String> {
    let package_error = |error: zip::result::ZipError| {
        format!("Could not read the PowerPoint presentation: {error}")
    };
    let mut archive =
        ZipArchive::new(CancellableFile { file, cancelled }).map_err(package_error)?;
    let mut manifest = Vec::new();
    archive
        .by_name(MANIFEST)
        .map_err(package_error)?
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut manifest)
        .map_err(|error| format!("Could not read the PowerPoint slide list: {error}"))?;
    if manifest.len() as u64 > MANIFEST_LIMIT {
        return Err("The PowerPoint slide list is too large.".into());
    }
    let (manifest, encoding) = decode_manifest(manifest)?;
    let (manifest, count) = one_slide(&manifest, index)?;
    let manifest = encode_manifest(manifest, encoding);
    let scratch = tempfile::Builder::new()
        .prefix("crawler-slide-")
        .tempdir()
        .map_err(|error| format!("Could not create a temporary PowerPoint preview: {error}"))?;
    let path = scratch.path().join("Slide.pptx");
    let mut create = || -> zip::result::ZipResult<()> {
        let mut slide = ZipWriter::new(File::create(&path)?);
        for at in 0..archive.len() {
            let entry = archive.by_index_raw(at)?;
            if entry.name() == MANIFEST {
                slide.start_file(MANIFEST, SimpleFileOptions::default())?;
                slide.write_all(&manifest)?;
            } else {
                slide.raw_copy_file(entry)?;
            }
        }
        slide.finish()?;
        Ok(())
    };
    create().map_err(|error| format!("Could not prepare slide {}: {error}", index + 1))?;
    let bitmap = thumbnail_at(&path, [3072.0, 3072.0], cancelled)
        .ok_or_else(|| format!("Quick Look could not display slide {}.", index + 1))?;
    Ok((bitmap, count))
}

fn one_slide(manifest: &str, index: usize) -> Result<(String, usize), String> {
    let xml = roxmltree::Document::parse(manifest)
        .map_err(|error| format!("Invalid PowerPoint slide list: {error}"))?;
    let root = xml.root_element();
    let namespace = root
        .tag_name()
        .namespace()
        .filter(|ns| matches!(*ns, PRESENTATION_NS | STRICT_PRESENTATION_NS))
        .filter(|_| root.has_tag_name("presentation"))
        .ok_or("Invalid PowerPoint presentation manifest.")?;
    let slides = root
        .children()
        .find(|child| child.has_tag_name((namespace, "sldIdLst")))
        .ok_or("This PowerPoint presentation has no slides.")?;
    let mut trimmed = String::with_capacity(manifest.len());
    let mut from = 0;
    let mut count = 0;
    for entry in slides
        .children()
        .filter(|entry| entry.has_tag_name((namespace, "sldId")))
    {
        if count != index {
            let removed = entry.range();
            trimmed.push_str(&manifest[from..removed.start]);
            from = removed.end;
        }
        count += 1;
    }
    if count == 0 {
        return Err("This PowerPoint presentation has no slides.".into());
    }
    if index >= count {
        return Err(format!(
            "This PowerPoint presentation has only {count} slides."
        ));
    }
    trimmed.push_str(&manifest[from..]);
    Ok((trimmed, count))
}

enum Encoding {
    Utf8,
    Utf16 { little_endian: bool, bom: bool },
}

fn decode_manifest(bytes: Vec<u8>) -> Result<(String, Encoding), String> {
    let (little_endian, bom) = match bytes.as_slice() {
        [0xff, 0xfe, ..] => (true, true),
        [0xfe, 0xff, ..] => (false, true),
        [b'<', 0, b'?', 0, ..] => (true, false),
        [0, b'<', 0, b'?', ..] => (false, false),
        _ => {
            return String::from_utf8(bytes)
                .map(|manifest| (manifest, Encoding::Utf8))
                .map_err(|_| "Unsupported PowerPoint slide list encoding.".into());
        }
    };
    let payload = &bytes[if bom { 2 } else { 0 }..];
    let (units, remainder) = payload.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err("Invalid UTF-16 PowerPoint slide list.".into());
    }
    let manifest = char::decode_utf16(units.iter().map(|unit| {
        if little_endian {
            u16::from_le_bytes(*unit)
        } else {
            u16::from_be_bytes(*unit)
        }
    }))
    .collect::<Result<String, _>>()
    .map_err(|_| "Invalid UTF-16 PowerPoint slide list.")?;
    Ok((manifest, Encoding::Utf16 { little_endian, bom }))
}

fn encode_manifest(manifest: String, encoding: Encoding) -> Vec<u8> {
    let Encoding::Utf16 { little_endian, bom } = encoding else {
        return manifest.into_bytes();
    };
    let mut result = Vec::with_capacity(manifest.len() * 2 + 2);
    if bom {
        result.extend(if little_endian {
            [0xff, 0xfe]
        } else {
            [0xfe, 0xff]
        });
    }
    for unit in manifest.encode_utf16() {
        result.extend(if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    result
}

struct CancellableFile<'a, F> {
    file: File,
    cancelled: &'a F,
}

impl<F: Fn() -> bool> Read for CancellableFile<'_, F> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if (self.cancelled)() {
            // io::copy retries Interrupted, so return a terminal error.
            return Err(io::Error::other("Cancelled"));
        }
        self.file.read(buffer)
    }
}

impl<F: Fn() -> bool> Seek for CancellableFile<'_, F> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}
