use crate::index::{file_name_from_path, name_len_chars, SearchIndex};
use std::cmp::{max, min};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub name_lower: String,
    pub name_len: u16,
}

#[derive(Debug, Clone)]
pub struct DirNode {
    pub name: String,
    pub parent: Option<u32>,
    pub child_dirs: Vec<u32>,
    pub file_ids: Vec<u32>,
    pub subtree_min_len: u16,
    pub subtree_max_len: u16,
    pub file_count: u32,
}

#[derive(Debug, Clone)]
pub struct IndexArena {
    pub root: PathBuf,
    pub dirs: Vec<DirNode>,
    pub files: Vec<FileEntry>,
    pub global_midpoint: u16,
    pub search: SearchIndex,
}

impl IndexArena {
    pub fn empty(root: PathBuf) -> Self {
        Self {
            root,
            dirs: vec![DirNode {
                name: String::new(),
                parent: None,
                child_dirs: Vec::new(),
                file_ids: Vec::new(),
                subtree_min_len: u16::MAX,
                subtree_max_len: 0,
                file_count: 0,
            }],
            files: Vec::new(),
            global_midpoint: 0,
            search: SearchIndex::build_from_files(&[], 0),
        }
    }

    pub fn from_paths(root: PathBuf, paths: Vec<PathBuf>) -> Self {
        ArenaBuilder::new(root).with_paths(paths).finish()
    }

    pub fn search_index(&self) -> &SearchIndex {
        &self.search
    }

    pub fn recompute_stats(&mut self) {
        for idx in (0..self.dirs.len()).rev() {
            let (child_dirs, file_ids) = {
                let dir = &self.dirs[idx];
                (dir.child_dirs.clone(), dir.file_ids.clone())
            };

            let mut min_len = u16::MAX;
            let mut max_len = 0u16;
            let mut count = 0u32;

            for &file_id in &file_ids {
                let file = &self.files[file_id as usize];
                min_len = min(min_len, file.name_len);
                max_len = max(max_len, file.name_len);
                count += 1;
            }

            for &child_id in &child_dirs {
                let child = &self.dirs[child_id as usize];
                min_len = min(min_len, child.subtree_min_len);
                max_len = max(max_len, child.subtree_max_len);
                count += child.file_count;
            }

            if count == 0 {
                min_len = 0;
            }

            let dir = &mut self.dirs[idx];
            dir.subtree_min_len = min_len;
            dir.subtree_max_len = max_len;
            dir.file_count = count;
        }

        let total: u64 = self.files.iter().map(|f| f.name_len as u64).sum();
        self.global_midpoint = if self.files.is_empty() {
            0
        } else {
            (total / self.files.len() as u64) as u16
        };
        let midpoint = self.global_midpoint;
        self.search = SearchIndex::build_from_files(&self.files, midpoint);
    }
}

struct ArenaBuilder {
    arena: IndexArena,
    child_maps: Vec<HashMap<String, u32>>,
}

impl ArenaBuilder {
    fn new(root: PathBuf) -> Self {
        let arena = IndexArena::empty(root);
        Self {
            child_maps: vec![HashMap::new()],
            arena,
        }
    }

    fn with_paths(mut self, paths: Vec<PathBuf>) -> Self {
        let root_str = self.arena.root.to_string_lossy().replace('\\', "/");
        let root_prefix = if root_str.ends_with('/') {
            root_str.clone()
        } else {
            format!("{root_str}/")
        };

        for path in paths {
            let normalized = path.to_string_lossy().replace('\\', "/");
            let rel = normalized
                .strip_prefix(&root_prefix)
                .or_else(|| {
                    if normalized == root_str.trim_end_matches('/') {
                        Some("")
                    } else {
                        None
                    }
                })
                .unwrap_or(normalized.as_str());

            if rel.is_empty() {
                continue;
            }

            let name_lower = file_name_from_path(&path.to_string_lossy());
            let name_len = name_len_chars(&name_lower);

            let mut dir_id = 0u32;
            let mut current = PathBuf::new();
            for component in Path::new(rel).components() {
                let part = component.as_os_str().to_string_lossy().into_owned();
                current.push(&part);
                let is_file = current.as_os_str().len() == Path::new(rel).as_os_len();

                if is_file {
                    let file_id = self.arena.files.len() as u32;
                    self.arena.files.push(FileEntry {
                        path: path.to_string_lossy().into_owned(),
                        name_lower: name_lower.clone(),
                        name_len,
                    });
                    self.arena.dirs[dir_id as usize].file_ids.push(file_id);
                } else {
                    dir_id = self.ensure_child_dir(dir_id, &part);
                }
            }
        }

        self
    }

    fn ensure_child_dir(&mut self, parent_id: u32, name: &str) -> u32 {
        if let Some(&existing) = self.child_maps[parent_id as usize].get(name) {
            return existing;
        }

        let id = self.arena.dirs.len() as u32;
        self.arena.dirs.push(DirNode {
            name: name.to_owned(),
            parent: Some(parent_id),
            child_dirs: Vec::new(),
            file_ids: Vec::new(),
            subtree_min_len: u16::MAX,
            subtree_max_len: 0,
            file_count: 0,
        });
        self.child_maps.push(HashMap::new());
        self.child_maps[parent_id as usize].insert(name.to_owned(), id);
        self.arena.dirs[parent_id as usize].child_dirs.push(id);
        id
    }

    fn finish(mut self) -> IndexArena {
        self.arena.recompute_stats();
        self.arena
    }
}

trait OsLen {
    fn as_os_len(&self) -> usize;
}

impl OsLen for Path {
    fn as_os_len(&self) -> usize {
        self.as_os_str().len()
    }
}