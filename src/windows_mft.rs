use crate::arena::IndexArena;
use ntfs_core::fs::NtfsFs;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf, Prefix};

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub fn build_with_mft(root: &Path) -> Result<IndexArena, Box<dyn std::error::Error>> {
    let volume = volume_path_for(root)?;
    let file = open_volume(&volume)?;
    let mut fs = NtfsFs::open(VolumeReader { file, pos: 0 })?;

    let ntfs_root = volume_relative_path(root)?;
    let root_record = fs.resolve_path(&ntfs_root)?;

    let mut paths = Vec::new();
    walk_mft_dir(&mut fs, &mut paths, root, root_record, &mut HashSet::new())?;
    // Raw enumeration must obey the normal walker's ignore and root policy.
    let allowed: HashSet<_> = crate::walker::collect_paths_checked(root)?
        .into_iter()
        .collect();
    paths.retain(|path| allowed.contains(path));
    paths.sort();
    paths.dedup();

    Ok(IndexArena::from_paths(root.to_path_buf(), paths))
}

fn volume_path_for(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    match root.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                Ok(format!(r"\\.\{}:", char::from(letter)))
            }
            _ => Err("root is not on a local drive letter".into()),
        },
        _ => Err("root is not on a local drive letter".into()),
    }
}

fn volume_relative_path(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    volume_path_for(root)?;
    if !root.has_root() {
        return Err("MFT search requires an absolute root".into());
    }
    let mut relative = String::new();
    for component in root.components() {
        if let Component::Normal(name) = component {
            relative.push('\\');
            relative.push_str(name.to_str().ok_or("MFT root is not valid Unicode")?);
        }
    }
    // NtfsFs resolves from the volume's root record; drive letters are not directory names.
    Ok(relative)
}

fn open_volume(path: &str) -> Result<File, Box<dyn std::error::Error>> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| e.into())
}

fn walk_mft_dir(
    fs: &mut NtfsFs<VolumeReader>,
    paths: &mut Vec<PathBuf>,
    root: &Path,
    record: u64,
    visited: &mut HashSet<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !visited.insert(record) {
        return Ok(());
    }
    let raw = fs.read_record(record)?;
    let entries = fs.directory_entries(&raw)?;

    for entry in entries {
        let Some(file_name) = entry.file_name.as_ref() else {
            continue;
        };

        let name = file_name.name.clone();
        if file_name.flags & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            continue;
        }
        if name == "." || name == ".." {
            continue;
        }

        let child_path = root.join(&name);
        if file_name.flags & FILE_ATTRIBUTE_DIRECTORY != 0 {
            walk_mft_dir(
                fs,
                paths,
                &child_path,
                entry.file_reference.record_number,
                visited,
            )?;
        } else {
            paths.push(child_path);
        }
    }

    Ok(())
}

struct VolumeReader {
    file: File,
    pos: u64,
}

impl Read for VolumeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.file.read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for VolumeReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.pos = self.file.seek(pos)?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_canonical_roots_resolve_relative_to_the_volume() {
        for root in [r"C:\projects\search", r"\\?\C:\projects\search"] {
            assert_eq!(volume_path_for(Path::new(root)).unwrap(), r"\\.\C:");
            assert_eq!(
                volume_relative_path(Path::new(root)).unwrap(),
                r"\projects\search"
            );
        }
        assert_eq!(volume_relative_path(Path::new(r"C:\")).unwrap(), "");
    }

    #[test]
    fn network_and_drive_relative_roots_are_rejected() {
        assert!(volume_path_for(Path::new(r"\\server\share\search")).is_err());
        assert!(volume_relative_path(Path::new(r"C:search")).is_err());
    }
}
