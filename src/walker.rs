use crate::{arena::IndexArena, util::home_dir};
use ignore::{WalkBuilder, WalkState};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

const HOME_SKIP_DIRS: &[&str] = &[
    "AppData",
    ".cargo",
    ".rustup",
    ".nuget",
    "node_modules",
    "target",
    ".git",
];

/// Compatibility name; traversal uses ignore::WalkBuilder, including ignore-file policy.
pub fn build_with_jwalk(root: &Path) -> IndexArena {
    build_with_walk(root).expect("failed to enumerate search root")
}

pub fn build_with_walk(root: &Path) -> io::Result<IndexArena> {
    Ok(IndexArena::from_paths(
        root.to_path_buf(),
        collect_paths_checked(root)?,
    ))
}

pub fn collect_paths(root: &Path) -> Vec<PathBuf> {
    match collect_paths_checked(root) {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("failed to enumerate {}: {error}", root.display());
            Vec::new()
        }
    }
}

pub fn collect_paths_checked(root: &Path) -> io::Result<Vec<PathBuf>> {
    collect_paths_with_cancel(root, &|| false)?.ok_or_else(|| io::Error::other("scan cancelled"))
}

pub(crate) fn collect_paths_with_cancel(
    root: &Path,
    cancelled: &(impl Fn() -> bool + Sync),
) -> io::Result<Option<Vec<PathBuf>>> {
    if !root.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "search root must be a directory",
        ));
    }
    let paths = Mutex::new(Vec::new());
    let errors = Mutex::new(Vec::new());
    let builder = walk_builder(root);
    builder.build_parallel().run(|| {
        Box::new(|entry| {
            if cancelled() {
                return WalkState::Quit;
            }
            match entry {
                Ok(entry) if entry.file_type().is_some_and(|kind| kind.is_file()) => {
                    paths
                        .lock()
                        .expect("scan paths lock")
                        .push(entry.into_path());
                }
                Ok(_) => {}
                Err(error) => errors
                    .lock()
                    .expect("scan errors lock")
                    .push(error.to_string()),
            }
            WalkState::Continue
        })
    });
    if cancelled() {
        return Ok(None);
    }
    if let Some(error) = errors
        .into_inner()
        .expect("scan errors lock")
        .into_iter()
        .next()
    {
        return Err(io::Error::other(format!(
            "incomplete scan of {}: {error}",
            root.display()
        )));
    }
    let mut paths = paths.into_inner().expect("scan paths lock");
    paths.sort();
    paths.dedup();
    Ok(Some(paths))
}

fn walk_builder(root: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .follow_links(false);
    let skip_home = home_dir().as_ref().is_some_and(|home| {
        root == home || std::fs::canonicalize(home).is_ok_and(|home| home == root)
    });
    let root = root.to_path_buf();
    let cache = crate::util::cache_directory();
    let cache = std::fs::canonicalize(&cache).unwrap_or(cache);
    builder.filter_entry(move |entry| {
        if (cache != root && entry.path().starts_with(&cache))
            || crate::util::is_index_cache_file(entry.path(), &cache)
        {
            return false;
        }
        if !skip_home {
            return true;
        }
        let Ok(relative) = entry.path().strip_prefix(&root) else {
            return false;
        };
        !relative.components().any(|component| {
            let name = component.as_os_str().to_string_lossy();
            HOME_SKIP_DIRS
                .iter()
                .any(|skip| name.eq_ignore_ascii_case(skip))
        })
    });
    builder
}

/// Reconstruct every derived ID reference, including deletions with no fresh paths.
pub fn rebuild_subtree(root: &Path, changed: &Path, arena: &mut IndexArena) {
    if let Err(error) = try_rebuild_subtree(root, changed, arena) {
        eprintln!("failed to update {}: {error}", changed.display());
    }
}

pub fn try_rebuild_subtree(root: &Path, changed: &Path, arena: &mut IndexArena) -> io::Result<()> {
    if !changed.starts_with(root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "changed path is outside search root",
        ));
    }
    let mut paths: Vec<PathBuf> = arena
        .files
        .iter()
        .filter(|file| !file.native_path.starts_with(changed))
        .map(|file| file.native_path.clone())
        .collect();
    if changed.exists() {
        // Enumerating under the original root preserves ancestor ignore and home-skip policy.
        paths.extend(
            collect_paths_checked(root)?
                .into_iter()
                .filter(|path| path.starts_with(changed)),
        );
    }
    paths.sort();
    paths.dedup();
    *arena = IndexArena::from_paths(root.to_path_buf(), paths);
    Ok(())
}
