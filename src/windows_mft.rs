use crate::arena::IndexArena;
use ntfs_core::fs::NtfsFs;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

pub fn build_with_mft(root: &Path) -> Result<IndexArena, Box<dyn std::error::Error>> {
    let volume = volume_path_for(root)?;
    let file = open_volume(&volume)?;
    let mut fs = NtfsFs::open(VolumeReader { file, pos: 0 })?;

    let ntfs_root = root
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\");
    let root_record = fs.resolve_path(&ntfs_root)?;

    let mut paths = Vec::new();
    walk_mft_dir(&mut fs, &mut paths, root, root_record)?;

    Ok(IndexArena::from_paths(root.to_path_buf(), paths))
}

fn volume_path_for(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut components = root.components();
    let prefix = components.next().ok_or("empty root path")?;
    let drive = prefix.as_os_str().to_string_lossy();
    if drive.len() >= 2 && drive.as_bytes()[1] == b':' {
        Ok(format!(r"\\.\{}", &drive[..2]))
    } else {
        Err("root is not on a local drive letter".into())
    }
}

fn open_volume(path: &str) -> Result<File, Box<dyn std::error::Error>> {
    OpenOptions::new().read(true).open(path).map_err(|e| e.into())
}

fn walk_mft_dir(
    fs: &mut NtfsFs<VolumeReader>,
    paths: &mut Vec<PathBuf>,
    root: &Path,
    record: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = fs.read_record(record)?;
    let entries = fs.directory_entries(&raw)?;

    for entry in entries {
        let Some(file_name) = entry.file_name.as_ref() else {
            continue;
        };

        let name = file_name.name.clone();
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