use std::path::{Components, Path, PathBuf};

#[cfg(windows)]
pub fn canonicalize(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

#[cfg(not(windows))]
pub fn canonicalize(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

/// The index stores relative paths with `/` on every platform. These helpers
/// convert between that canonical form and the OS-native separator, and are
/// no-ops on non-Windows where `/` is already native.

/// Fold a relative path to the canonical `/` form (no-op off Windows).
#[cfg(windows)]
pub fn to_canonical_slashes(rel: &str) -> std::borrow::Cow<'_, str> {
    if rel.contains('\\') {
        std::borrow::Cow::Owned(rel.replace('\\', "/"))
    } else {
        std::borrow::Cow::Borrowed(rel)
    }
}

#[cfg(not(windows))]
#[inline]
pub fn to_canonical_slashes(rel: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Borrowed(rel)
}

/// Rewrite canonical `/` bytes to the OS-native separator in place (no-op off
/// Windows). Used at OS/state boundaries (absolute-path reconstruction).
#[cfg(windows)]
#[inline]
pub fn nativize_slashes_in_place(bytes: &mut [u8]) {
    for b in bytes {
        if *b == b'/' {
            *b = b'\\';
        }
    }
}

#[cfg(not(windows))]
#[inline]
pub fn nativize_slashes_in_place(_bytes: &mut [u8]) {}

/// Git requires a normalized forward-slashed paths on windows
#[cfg(windows)]
pub fn normalize(path: PathBuf) -> PathBuf {
    let as_str = path.to_string_lossy();
    let with_backslashes: String = as_str.replace('/', "\\");
    let buf = PathBuf::from(with_backslashes);
    dunce::canonicalize(&buf).unwrap_or(buf)
}

#[cfg(not(windows))]
pub fn normalize(path: PathBuf) -> PathBuf {
    path
}

#[cfg(windows)]
pub fn expand_tilde(path: &str) -> PathBuf {
    return PathBuf::from(path);
}

#[cfg(not(windows))]
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/")
        && let Some(home_dir) = dirs::home_dir()
    {
        return home_dir.join(stripped);
    }

    PathBuf::from(path)
}

/// Calculate the directory proximity penalty without allocating.
/// `candidate_dir` may include a trailing separator.
pub fn calculate_distance_penalty(current_file: Option<&str>, candidate_dir: &str) -> i32 {
    let Some(current_path) = current_file else {
        return 0;
    };

    let current_dir = Path::new(current_path).parent().unwrap_or(Path::new(""));
    let candidate = Path::new(candidate_dir);

    if current_dir == candidate {
        return 0;
    }

    let mut current_parts = current_dir.components();
    let mut candidate_parts = candidate.components();

    let mut common_len = 0usize;
    let mut current_total = 0usize;

    loop {
        match (current_parts.next(), candidate_parts.next()) {
            (Some(a), Some(b)) => {
                current_total += 1;
                if a == b {
                    common_len += 1;
                } else {
                    current_total += current_parts.count();
                    break;
                }
            }
            (Some(_), None) => {
                current_total += 1 + current_parts.count();
                break;
            }
            (None, _) => {
                break;
            }
        }
    }

    let depth_from_common = current_total - common_len;
    if depth_from_common == 0 {
        return 0;
    }

    (-(depth_from_common as i32)).max(-20)
}

pub(crate) struct DirectoryDistance<'a> {
    components: Components<'a>,
    depth: usize,
}

impl<'a> DirectoryDistance<'a> {
    pub(crate) fn new(current_file: &'a str) -> Self {
        let directory = Path::new(current_file).parent().unwrap_or(Path::new(""));
        let components = directory.components();
        let depth = components.clone().count();
        Self { components, depth }
    }

    pub(crate) fn penalty(&self, candidate_dir: &str) -> i32 {
        let common = self
            .components
            .clone()
            .zip(Path::new(candidate_dir).components())
            .take_while(|(a, b)| a == b)
            .count();
        -((self.depth - common).min(20) as i32)
    }
}

