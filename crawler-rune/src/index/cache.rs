use super::snapshot::{REVISION, Snapshot};
use memmap2::{Mmap, MmapOptions};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: u64,
    pub date: f64,
}
impl Event {
    pub fn now(id: u64) -> Self {
        Self { id, date: now() }
    }
}
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
#[derive(Serialize, Deserialize)]
struct Header {
    version: u32,
    root: PathBuf,
    engine: String,
    event: Option<Event>,
    batches: usize,
    #[serde(default)]
    signatures: bool,
}
pub struct Reader {
    map: Arc<Mmap>,
    blocks: std::collections::VecDeque<(std::ops::Range<usize>, Option<std::ops::Range<usize>>)>,
    pub event: Option<Event>,
}
impl Reader {
    pub fn open(file: &Path, root: &Path) -> io::Result<Self> {
        let file = File::open(file)?;
        // Crawler writes by rename, never by truncating a mapped cache inode.
        let map = Arc::new(unsafe { MmapOptions::new().map(&file)? });
        if map.get(..8) != Some(b"RUNFFF04") {
            return Err(corrupt());
        }
        let size = u32::from_le_bytes(
            map.get(8..12)
                .ok_or_else(corrupt)?
                .try_into()
                .map_err(|_| corrupt())?,
        ) as usize;
        if size > 65536 {
            return Err(corrupt());
        }
        let header: Header = serde_json::from_slice(map.get(12..12 + size).ok_or_else(corrupt)?)
            .map_err(|_| corrupt())?;
        if header.version != 4
            || header.engine != REVISION
            || header.root != root
            || header.batches > 1_000_000
        {
            return Err(corrupt());
        }
        let mut at = (12 + size).next_multiple_of(8);
        if header.batches > map.len().saturating_sub(at) / 56 {
            return Err(corrupt());
        }
        let mut blocks = std::collections::VecDeque::with_capacity(header.batches);
        for _ in 0..header.batches {
            let size = u64::from_le_bytes(
                map.get(at..at + 8)
                    .ok_or_else(corrupt)?
                    .try_into()
                    .map_err(|_| corrupt())?,
            );
            let size = usize::try_from(size).map_err(|_| corrupt())?;
            at += 8;
            let signature = if header.signatures {
                let end = at.checked_add(8192).ok_or_else(corrupt)?;
                map.get(at..end).ok_or_else(corrupt)?;
                let range = at..end;
                at = end;
                Some(range)
            } else {
                None
            };
            let end = at.checked_add(size).ok_or_else(corrupt)?;
            map.get(at..end).ok_or_else(corrupt)?;
            blocks.push_back((at..end, signature));
            at = end.next_multiple_of(8);
        }
        if at != map.len() {
            return Err(corrupt());
        }
        Ok(Self {
            map,
            blocks,
            event: header.event.filter(|e| e.date.is_finite() && e.id > 0),
        })
    }
    pub fn prioritize(&mut self, text: &str) {
        let mut bits: Vec<_> = text
            .as_bytes()
            .windows(3)
            .map(super::snapshot::trigram_bit)
            .collect();
        bits.sort_unstable();
        bits.dedup();
        // Hints only change arrival order. Every whole snapshot still reaches FFF.
        let map = &self.map;
        self.blocks
            .make_contiguous()
            .sort_by_cached_key(|(_, signature)| {
                std::cmp::Reverse(signature.as_ref().map_or(0, |range| {
                    bits.iter()
                        .filter(|&&bit| map[range.start + bit / 8] & (1 << (bit % 8)) != 0)
                        .count()
                }))
            });
    }
    pub fn next(&mut self) -> io::Result<Option<Arc<Snapshot>>> {
        let Some((range, _)) = self.blocks.pop_front() else {
            return Ok(None);
        };
        Snapshot::decode(Arc::clone(&self.map), range)
            .map(Arc::new)
            .map(Some)
            .ok_or_else(corrupt)
    }
}
fn corrupt() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid prepared search cache")
}

pub fn support_dir() -> PathBuf {
    if let Some(root) = std::env::var_os("RUNE_PREFS_DIR") {
        return PathBuf::from(root).join("studio.crawler.rune/index");
    }
    if let Some(root) = std::env::var_os("RUNE_DIRS_ROOT") {
        return PathBuf::from(root).join("cache/studio.crawler.rune/index");
    }
    if rune::tooling::headless_run().is_some() {
        return std::env::temp_dir().join(format!("crawler-index-{}", std::process::id()));
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Library/Application Support/studio.crawler.rune")
}
pub fn save(
    file: &Path,
    root: &Path,
    batches: &[Arc<Snapshot>],
    event: Option<Event>,
    cancelled: impl Fn() -> bool,
) -> io::Result<()> {
    let header = serde_json::to_vec(&Header {
        version: 4,
        root: root.to_owned(),
        engine: REVISION.into(),
        event,
        batches: batches.len(),
        signatures: true,
    })?;
    atomic_write(file, |out| {
        out.write_all(b"RUNFFF04")?;
        out.write_all(&(header.len() as u32).to_le_bytes())?;
        out.write_all(&header)?;
        pad(out, 12 + header.len())?;
        for batch in batches {
            if cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Search cache save cancelled",
                ));
            }
            let bytes = batch.encode();
            out.write_all(&(bytes.len() as u64).to_le_bytes())?;
            out.write_all(&batch.signature())?;
            out.write_all(&bytes)?;
            pad(out, bytes.len())?;
        }
        if cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Search cache save cancelled",
            ));
        }
        Ok(())
    })
}
fn pad(out: &mut File, size: usize) -> io::Result<()> {
    out.write_all(&[0; 8][..size.next_multiple_of(8) - size])
}
pub fn atomic_write(
    file: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let parent = file.parent().ok_or_else(corrupt)?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".index-{}-{}.tmp",
        std::process::id(),
        now().to_bits()
    ));
    let result = (|| {
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        write(&mut out)?;
        out.sync_all()?;
        fs::rename(&temp, file)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Visit {
    pub count: u32,
    pub opened: f64,
}
pub type History = std::collections::BTreeMap<String, Visit>;
pub fn history(file: &Path) -> History {
    fs::read(file)
        .ok()
        .filter(|v| v.len() < 2_000_000)
        .and_then(|bytes| serde_json::from_slice::<History>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|(path, v)| {
            path.starts_with('/') && v.count > 0 && v.count <= 100 && v.opened.is_finite()
        })
        .take(1000)
        .collect()
}
pub fn boosts(history: &History) -> Vec<(String, i16)> {
    let now = now();
    history
        .iter()
        .map(|(path, v)| {
            (
                path.clone(),
                ((f64::from(v.count) * 6.0).min(60.0)
                    / (1.0 + ((now - v.opened).max(0.0) / 86400.0) / 7.0)) as i16,
            )
        })
        .collect()
}
