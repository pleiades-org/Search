use crate::index::{SearchIndex, name_len_chars};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
};

#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Display path; filesystem operations use native_path to preserve non-Unicode names.
    pub path: String,
    pub native_path: PathBuf,
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

impl DirNode {
    fn empty(name: String, parent: Option<u32>) -> Self {
        Self {
            name,
            parent,
            child_dirs: Vec::new(),
            file_ids: Vec::new(),
            subtree_min_len: 0,
            subtree_max_len: 0,
            file_count: 0,
        }
    }
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
            dirs: vec![DirNode::empty(String::new(), None)],
            files: Vec::new(),
            global_midpoint: 0,
            search: SearchIndex::default(),
        }
    }

    pub fn from_paths(root: PathBuf, paths: Vec<PathBuf>) -> Self {
        let mut builder = ArenaBuilder::new(root);
        for path in paths {
            builder.add_path(path);
        }
        builder.arena.recompute_stats();
        builder.arena
    }

    pub fn search_index(&self) -> &SearchIndex {
        &self.search
    }

    pub fn recompute_stats(&mut self) {
        for ordinal in (0..self.dirs.len()).rev() {
            let (minimum, maximum, count) = self.directory_stats(ordinal);
            let directory = &mut self.dirs[ordinal];
            directory.subtree_min_len = minimum;
            directory.subtree_max_len = maximum;
            directory.file_count = count;
        }
        let total: u64 = self.files.iter().map(|file| u64::from(file.name_len)).sum();
        self.global_midpoint = if self.files.is_empty() {
            0
        } else {
            (total / self.files.len() as u64) as u16
        };
        self.search = SearchIndex::build_from_files(&self.files, self.global_midpoint);
    }

    fn directory_stats(&self, ordinal: usize) -> (u16, u16, u32) {
        let directory = &self.dirs[ordinal];
        let mut minimum = u16::MAX;
        let mut maximum = 0;
        let mut count = 0;
        for &identifier in &directory.file_ids {
            let file = &self.files[identifier as usize];
            minimum = minimum.min(file.name_len);
            maximum = maximum.max(file.name_len);
            count += 1;
        }
        for &identifier in &directory.child_dirs {
            let child = &self.dirs[identifier as usize];
            if child.file_count == 0 {
                continue;
            }
            minimum = minimum.min(child.subtree_min_len);
            maximum = maximum.max(child.subtree_max_len);
            count += child.file_count;
        }
        (if count == 0 { 0 } else { minimum }, maximum, count)
    }
}

struct ArenaBuilder {
    arena: IndexArena,
    children: Vec<HashMap<OsString, u32>>,
}

impl ArenaBuilder {
    fn new(root: PathBuf) -> Self {
        Self {
            arena: IndexArena::empty(root),
            children: vec![HashMap::new()],
        }
    }

    fn add_path(&mut self, path: PathBuf) {
        let Ok(relative) = path.strip_prefix(&self.arena.root) else {
            return;
        };
        if relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return;
        }
        let parts: Vec<_> = relative
            .components()
            .filter_map(|part| match part {
                std::path::Component::Normal(name) => Some(name.to_owned()),
                _ => None,
            })
            .collect();
        let Some((filename, directories)) = parts.split_last() else {
            return;
        };
        let mut directory_id = 0;
        for name in directories {
            directory_id = self.ensure_directory(directory_id, name);
        }
        let name_lower = filename.to_string_lossy().to_lowercase();
        let name_len = name_len_chars(&name_lower);
        let file_id =
            u32::try_from(self.arena.files.len()).expect("file catalog exceeds 32-bit IDs");
        self.arena.files.push(FileEntry {
            path: path.to_string_lossy().into_owned(),
            native_path: path,
            name_lower,
            name_len,
        });
        self.arena.dirs[directory_id as usize]
            .file_ids
            .push(file_id);
    }

    fn ensure_directory(&mut self, parent: u32, name: &OsStr) -> u32 {
        if let Some(&identifier) = self.children[parent as usize].get(name) {
            return identifier;
        }
        let identifier =
            u32::try_from(self.arena.dirs.len()).expect("directory catalog exceeds 32-bit IDs");
        self.arena.dirs.push(DirNode::empty(
            name.to_string_lossy().into_owned(),
            Some(parent),
        ));
        self.children.push(HashMap::new());
        self.children[parent as usize].insert(name.to_owned(), identifier);
        self.arena.dirs[parent as usize].child_dirs.push(identifier);
        identifier
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_non_unicode_directory_names_keep_separate_file_ids() {
        #[cfg(unix)]
        let names = {
            use std::os::unix::ffi::OsStringExt;
            [
                OsString::from_vec(vec![0xfe]),
                OsString::from_vec(vec![0xff]),
            ]
        };
        #[cfg(windows)]
        let names = {
            use std::os::windows::ffi::OsStringExt;
            [
                OsString::from_wide(&[0xd800]),
                OsString::from_wide(&[0xd801]),
            ]
        };
        let root = PathBuf::from("/native-path-fixture");
        let paths = names
            .iter()
            .map(|name| root.join(name).join("file.txt"))
            .collect();
        let arena = IndexArena::from_paths(root, paths);
        assert_eq!(arena.dirs[0].child_dirs.len(), 2);
        assert_eq!(arena.dirs[1].file_ids, vec![0]);
        assert_eq!(arena.dirs[2].file_ids, vec![1]);
        assert_ne!(arena.files[0].native_path, arena.files[1].native_path);
    }
}
