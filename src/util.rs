use std::path::{Path, PathBuf};

pub fn default_search_root() -> PathBuf {
    home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn expand_root(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let text = path.to_string_lossy();
    if (text == "~" || text.starts_with("~/") || text.starts_with("~\\"))
        && let Some(home) = home_dir()
    {
        return if text == "~" {
            home
        } else {
            home.join(&text[2..])
        };
    }
    path.to_path_buf()
}

pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let variable = "USERPROFILE";
    #[cfg(not(windows))]
    let variable = "HOME";
    std::env::var_os(variable).map(PathBuf::from)
}

pub fn cache_directory() -> PathBuf {
    if let Some(directory) = std::env::var_os("CODESEARCH_CACHE_DIR") {
        return PathBuf::from(directory);
    }
    #[cfg(windows)]
    if let Some(directory) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(directory).join("CodeSearch");
    }
    #[cfg(not(windows))]
    if let Some(directory) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(directory).join("codesearch");
    }
    home_dir().map_or_else(
        || std::env::temp_dir().join("codesearch"),
        |home| home.join(".cache").join("codesearch"),
    )
}

/// Stable root fingerprint; the stored canonical root is independently checked on every load.
pub fn index_db_path(root: &Path) -> PathBuf {
    let mut fingerprint = 0xcbf29ce484222325_u64;
    for byte in root.as_os_str().as_encoded_bytes() {
        fingerprint ^= u64::from(*byte);
        fingerprint = fingerprint.wrapping_mul(0x100000001b3);
    }
    cache_directory().join(format!("v4-{fingerprint:016x}.redb"))
}

pub(crate) fn is_index_cache_file(path: &Path, directory: &Path) -> bool {
    path.parent() == Some(directory)
        && path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name.starts_with("v4-") && name.contains(".redb")
        })
}
