//! Read an immediate directory in fixed-size batches of macOS attributes.
use super::model::{DocumentKind, Item, ItemID};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::{
    ffi::OsStrExt,
    fs::{MetadataExt, OpenOptionsExt},
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Darwin's sys/attr.h. ATTR_CMN_ERROR immediately follows the returned masks.
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const COMMON: u32 = libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_CRTIME
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_FLAGS;
const PACKAGE_IDENTITY: u32 = libc::ATTR_CMN_DEVID | libc::ATTR_CMN_CHGTIME | libc::ATTR_CMN_FILEID;
const VREG: u32 = 1; // sys/vnode.h
const VDIR: u32 = 2;

// getattrlistbulk packs fields on four-byte boundaries, in attribute-bit order.
#[repr(C, packed(4))]
#[derive(Copy, Clone)]
struct Entry {
    length: u32,
    returned: libc::attribute_set_t,
    error: u32,
    name: libc::attrreference_t,
    device: libc::dev_t,
    kind: u32,
    created: libc::timespec,
    modified: libc::timespec,
    changed: libc::timespec,
    flags: u32,
    file_id: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PackageIdentity {
    device: libc::dev_t,
    file_id: u64,
    changed: SystemTime,
}

impl PackageIdentity {
    fn from_entry(entry: Entry) -> Option<Self> {
        if entry.returned.commonattr & PACKAGE_IDENTITY != PACKAGE_IDENTITY
            || entry.device == 0
            || entry.file_id == 0
        {
            return None;
        }
        Some(Self {
            device: entry.device,
            file_id: entry.file_id,
            changed: timestamp(entry.changed)?,
        })
    }
}

pub(super) struct Packages {
    checked: Instant,
    kinds: HashMap<Box<OsStr>, (PackageIdentity, bool)>,
}

impl Packages {
    fn reuse(
        &mut self,
        previous: Option<&Self>,
        name: &OsStr,
        identity: PackageIdentity,
    ) -> Option<bool> {
        let known = previous
            .and_then(|old| old.kinds.get(name))
            .filter(|(old, _)| *old == identity)
            .map(|(_, is_package)| *is_package)?;
        self.kinds.insert(name.into(), (identity, known));
        Some(known)
    }

    fn finish(
        &mut self,
        folder: &Path,
        fd: &File,
        original: Option<Metadata>,
        rows: &mut [Item],
        pending: Vec<(usize, Option<PackageIdentity>)>,
        cancel: &AtomicBool,
    ) -> io::Result<()> {
        let mut prefetched = Vec::new();
        // A second shallow enumeration pays off when native lookups dominate the input.
        if pending.len() >= 128
            && pending.len() >= rows.len() / 2
            && let Some(original) = original
            && is_local(fd)
        {
            prefetched.resize(pending.len(), None);
            let candidates = pending
                .iter()
                .zip(&mut prefetched)
                .filter_map(|((i, _), status)| {
                    let name = rows[*i].id.0.file_name()?;
                    name.to_str()?;
                    Some((name, status))
                })
                .collect();
            crate::opening::platform::prefetch_packages(folder, candidates, cancel);
            if cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            // The URL enumerator reopens by path. If the parent was replaced or changed, its
            // prefetched results may describe another entry with the same name.
            if !std::fs::metadata(folder).is_ok_and(|current| same_directory(&original, &current)) {
                prefetched.clear();
            }
        }
        for (at, (index, identity)) in pending.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let row = &mut rows[index];
            let status = prefetched
                .get(at)
                .copied()
                .flatten()
                .or_else(|| crate::opening::platform::package_status(&row.id.0));
            if status == Some(true) {
                row.kind = DocumentKind::Package;
            }
            if let (Some(name), Some(identity), Some(is_package)) =
                (row.id.0.file_name(), identity, status)
            {
                self.kinds.insert(name.into(), (identity, is_package));
            }
        }
        Ok(())
    }
}

fn is_local(fd: &File) -> bool {
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fd is open, and a successful fstatfs initializes info.
    unsafe {
        libc::fstatfs(fd.as_raw_fd(), info.as_mut_ptr()) == 0
            && info.assume_init().f_flags & libc::MNT_LOCAL as u32 != 0
    }
}

fn same_directory(before: &Metadata, after: &Metadata) -> bool {
    (
        before.dev(),
        before.ino(),
        before.ctime(),
        before.ctime_nsec(),
        before.mtime(),
        before.mtime_nsec(),
    ) == (
        after.dev(),
        after.ino(),
        after.ctime(),
        after.ctime_nsec(),
        after.mtime(),
        after.mtime_nsec(),
    )
}

fn timestamp(t: libc::timespec) -> Option<SystemTime> {
    if !(0..1_000_000_000).contains(&t.tv_nsec) {
        return None;
    }
    let seconds = Duration::from_secs(t.tv_sec.unsigned_abs());
    let base = if t.tv_sec < 0 {
        UNIX_EPOCH.checked_sub(seconds)?
    } else {
        UNIX_EPOCH.checked_add(seconds)?
    };
    base.checked_add(Duration::from_nanos(t.tv_nsec as u64))
}

pub(super) fn read(
    folder: &Path,
    previous: Option<&Packages>,
    cancel: &AtomicBool,
) -> io::Result<(Vec<Item>, Packages)> {
    // Finder package registrations can change without touching the directory entry. Never
    // extend the lifetime of reused classifications by repeatedly refreshing the listing.
    let previous = previous.filter(|old| old.checked.elapsed() < Duration::from_secs(30));
    let mut packages = Packages {
        checked: previous.map_or_else(Instant::now, |old| old.checked),
        kinds: HashMap::new(),
    };
    let fd = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .open(folder)?;
    let original = (!crate::foreground_io::external_path(folder))
        .then(|| fd.metadata().ok())
        .flatten();
    let mut attrs = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: COMMON | PACKAGE_IDENTITY | ATTR_CMN_ERROR | libc::ATTR_CMN_RETURNED_ATTRS,
        volattr: 0,
        dirattr: 0,
        fileattr: libc::ATTR_FILE_DATALENGTH,
        forkattr: 0,
    };
    let mut rows = Vec::new();
    let mut pending = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        // SAFETY: the descriptor is open and the kernel writes only within this buffer.
        let count = unsafe {
            libc::getattrlistbulk(
                fd.as_raw_fd(),
                (&raw mut attrs).cast(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                libc::FSOPT_PACK_INVAL_ATTRS.into(),
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            packages.finish(folder, &fd, original, &mut rows, pending, cancel)?;
            return Ok((rows, packages));
        }
        let mut remaining = buffer.as_slice();
        for _ in 0..count {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let invalid = || io::Error::from(io::ErrorKind::InvalidData);
            if remaining.len() < size_of::<Entry>() {
                return Err(invalid());
            }
            // SAFETY: fixed-size prefix checked above, contains only integer fields.
            let entry = unsafe { remaining.as_ptr().cast::<Entry>().read_unaligned() };
            let (record, rest) = remaining
                .split_at_checked(entry.length as usize)
                .ok_or_else(invalid)?;
            remaining = rest;
            if record.len() < size_of::<Entry>()
                || entry.returned.commonattr & libc::ATTR_CMN_NAME == 0
            {
                return Err(invalid());
            }
            let start = std::mem::offset_of!(Entry, name)
                .checked_add_signed(entry.name.attr_dataoffset as isize)
                .ok_or_else(invalid)?;
            let name = record
                .get(start..)
                .and_then(|data| data.get(..entry.name.attr_length as usize))
                .and_then(|data| data.strip_suffix(&[0]))
                .filter(|name| {
                    !name.is_empty() && !name.contains(&b'/') && start >= size_of::<Entry>()
                })
                .ok_or_else(invalid)?;
            if name.starts_with(b".") {
                continue;
            }
            let name = OsStr::from_bytes(name);
            if entry.error != 0
                || entry.returned.commonattr & COMMON != COMMON
                || !matches!(entry.kind, VREG | VDIR)
            {
                // In particular, a symlink's displayed kind and size come from its target.
                super::library::push_entry(&mut rows, folder, name);
                continue;
            }
            if entry.flags & libc::UF_HIDDEN != 0 {
                continue;
            }
            // Darwin omits file-only fields from directory records even with PACK_INVAL_ATTRS.
            let size = if entry.kind == VREG {
                let bytes = record
                    .get(size_of::<Entry>()..start)
                    .and_then(|tail| tail.first_chunk::<8>())
                    .map(|raw| i64::from_ne_bytes(*raw))
                    .and_then(|n| u64::try_from(n).ok());
                if entry.returned.fileattr & libc::ATTR_FILE_DATALENGTH == 0 || bytes.is_none() {
                    super::library::push_entry(&mut rows, folder, name);
                    continue;
                }
                bytes
            } else {
                None
            };
            let path = folder.join(name);
            let mut kind = DocumentKind::for_path(&path, entry.kind == VDIR);
            if kind == DocumentKind::Folder {
                let identity = PackageIdentity::from_entry(entry);
                match identity.and_then(|identity| packages.reuse(previous, name, identity)) {
                    Some(true) => kind = DocumentKind::Package,
                    Some(false) => {}
                    None => pending.push((rows.len(), identity)),
                }
            }
            rows.push(Item {
                kind,
                id: ItemID(path),
                size,
                created: timestamp(entry.created),
                modified: timestamp(entry.modified),
                symlink: false,
                hidden: false,
                highlights: Vec::new(),
                path_highlights: Vec::new(),
            });
        }
    }
}
