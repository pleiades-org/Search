use codesearch::arena::IndexArena;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

const FILE_COUNT: usize = 256;
const LINES_PER_FILE: usize = 512;

pub fn filename_arena(count: usize) -> IndexArena {
    let root = PathBuf::from("/benchmark");
    let mut paths: Vec<_> = (0..count)
        .map(|ordinal| {
            root.join(format!(
                "group_{:03}/application_report_{ordinal:06}.txt",
                ordinal % 64
            ))
        })
        .collect();
    paths[count / 2] = root.join("special/needle_tag_7fd9.txt");
    IndexArena::from_paths(root, paths)
}

pub fn prepare(directory: &Path) -> io::Result<()> {
    let many = directory.join("many");
    let single = directory.join("single");
    fs::create_dir_all(&many)?;
    fs::create_dir_all(&single)?;
    let mut combined = String::new();
    for file_id in 0..FILE_COUNT {
        let mut text = String::new();
        for line_id in 0..LINES_PER_FILE {
            if file_id == FILE_COUNT / 2 && line_id == LINES_PER_FILE / 2 {
                text.push_str("needle_tag_7fd9 appears only here\n");
            } else if line_id % 16 == 0 {
                text.push_str("return application_result with some repeated source text and enough padding to make realistic code lines\n");
            } else {
                text.push_str("ordinary source line with application metadata and repeated padding for consistent deterministic fixture bytes\n");
            }
        }
        fs::write(many.join(format!("source_{file_id:04}.txt")), &text)?;
        combined.push_str(&text);
    }
    fs::write(single.join("combined.txt"), combined)?;
    Ok(())
}

pub fn content_arena(directory: &Path) -> io::Result<IndexArena> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(IndexArena::from_paths(directory.to_path_buf(), paths))
}
