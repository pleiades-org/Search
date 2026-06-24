use crate::arena::IndexArena;
use crate::util::home_dir;
use ignore::WalkBuilder;
use ignore::WalkState;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const HOME_SKIP_DIRS: &[&str] = &[
    "AppData",
    ".cargo",
    ".rustup",
    ".nuget",
    "node_modules",
    "target",
    ".git",
];

pub fn build_with_jwalk(root: &Path) -> IndexArena {
    let paths = collect_paths(root);
    IndexArena::from_paths(root.to_path_buf(), paths)
}

pub fn collect_paths(root: &Path) -> Vec<PathBuf> {
    let paths = Mutex::new(Vec::<PathBuf>::new());
    let skip_home_junk = home_dir().as_ref().is_some_and(|home| root == home);
    let root_buf = root.to_path_buf();

    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true);

    if skip_home_junk {
        let root_owned = root_buf.clone();
        builder.filter_entry(move |entry| {
            let path = entry.path();
            if path == root_owned {
                return true;
            }
            let Ok(rel) = path.strip_prefix(&root_owned) else {
                return true;
            };
            for component in rel.components() {
                let name = component.as_os_str().to_string_lossy();
                if HOME_SKIP_DIRS.iter().any(|skip| name.eq_ignore_ascii_case(skip)) {
                    return false;
                }
            }
            true
        });
    }

    let walker = builder.build_parallel();

    walker.run(|| {
        let paths = &paths;
        Box::new(move |entry| {
            if let Ok(entry) = entry {
                if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                    paths.lock().unwrap().push(entry.into_path());
                }
            }
            WalkState::Continue
        })
    });

    paths.into_inner().unwrap()
}

pub fn rebuild_subtree(root: &Path, changed_dir: &Path, arena: &mut IndexArena) {
    let root = root.to_path_buf();
    let changed = changed_dir.to_path_buf();

    let stale: Vec<String> = arena
        .files
        .iter()
        .filter(|f| path_within(&f.path, &changed))
        .map(|f| f.path.clone())
        .collect();

    if !stale.is_empty() {
        let stale_set: std::collections::HashSet<String> = stale.into_iter().collect();
        arena.files.retain(|f| !stale_set.contains(&f.path));
        for dir in &mut arena.dirs {
            dir.file_ids.retain(|&id| {
                arena
                    .files
                    .get(id as usize)
                    .map(|f| !stale_set.contains(&f.path))
                    .unwrap_or(false)
            });
        }
    }

    let fresh = if changed.is_dir() {
        collect_paths(&changed)
    } else {
        Vec::new()
    };

    if !fresh.is_empty() {
        let existing: Vec<PathBuf> = arena.files.iter().map(|f| f.path.clone().into()).collect();
        let mut all = existing;
        all.extend(fresh);
        *arena = IndexArena::from_paths(root, all);
    } else {
        arena.recompute_stats();
    }
}

fn path_within(path: &str, dir: &Path) -> bool {
    let path = Path::new(path);
    path.starts_with(dir)
}