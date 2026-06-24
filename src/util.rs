use std::path::{Path, PathBuf};

pub fn default_search_root() -> PathBuf {
    home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn expand_root(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let raw = path.to_string_lossy();

    if raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
        if let Some(home) = home_dir() {
            if raw == "~" {
                return home;
            }
            let rest = &raw[2..];
            return home.join(rest);
        }
    }

    path.to_path_buf()
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

pub fn index_db_path(root: &Path) -> PathBuf {
    let mut name = root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    if name.is_empty() {
        name = "codesearch".into();
    }
    PathBuf::from(format!(".codesearch-{name}.redb"))
}