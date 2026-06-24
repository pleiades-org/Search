use crate::arena::{DirNode, FileEntry, IndexArena};
use crate::index::{file_name_from_path, SearchIndex};
use redb::{Database, TableDefinition};
use std::path::Path;

const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const INDEX_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("index");

const MAGIC_V1: &[u8; 4] = b"CSH1";
const MAGIC_V2: &[u8; 4] = b"CSH2";
const MAGIC_V3: &[u8; 4] = b"CSH3";

pub struct IndexStore {
    db: Database,
}

impl IndexStore {
    pub fn open(path: &Path) -> Result<Self, redb::Error> {
        let db = Database::create(path).map_err(redb::Error::from)?;
        Ok(Self { db })
    }

    pub fn save(&self, arena: &IndexArena) -> Result<(), redb::Error> {
        let blob = serialize_arena(arena);
        let write_txn = self.db.begin_write().map_err(redb::Error::from)?;
        {
            let mut meta = write_txn
                .open_table(META_TABLE)
                .map_err(redb::Error::from)?;
            meta.insert("root", arena.root.to_string_lossy().as_bytes())
                .map_err(redb::Error::from)?;
            let mut index = write_txn
                .open_table(INDEX_TABLE)
                .map_err(redb::Error::from)?;
            index.insert("arena", blob.as_slice())
                .map_err(redb::Error::from)?;
        }
        write_txn.commit().map_err(redb::Error::from)?;
        Ok(())
    }

    pub fn load(&self) -> Result<Option<IndexArena>, redb::Error> {
        let read_txn = self.db.begin_read().map_err(redb::Error::from)?;
        let index = match read_txn.open_table(INDEX_TABLE) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(redb::Error::from(e)),
        };

        let Some(blob) = index
            .get("arena")
            .map_err(redb::Error::from)?
            .map(|v| v.value().to_vec())
        else {
            return Ok(None);
        };

        deserialize_arena(&blob).map(Some)
    }
}

fn serialize_arena(arena: &IndexArena) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC_V3);

    write_str(&mut out, &arena.root.to_string_lossy());
    out.extend_from_slice(&arena.global_midpoint.to_le_bytes());

    write_u32(&mut out, arena.dirs.len() as u32);
    for dir in &arena.dirs {
        write_str(&mut out, &dir.name);
        write_opt_u32(&mut out, dir.parent);
        write_u32_slice(&mut out, &dir.child_dirs);
        write_u32_slice(&mut out, &dir.file_ids);
        out.extend_from_slice(&dir.subtree_min_len.to_le_bytes());
        out.extend_from_slice(&dir.subtree_max_len.to_le_bytes());
        out.extend_from_slice(&dir.file_count.to_le_bytes());
    }

    write_u32(&mut out, arena.files.len() as u32);
    for file in &arena.files {
        write_str(&mut out, &file.path);
        write_str(&mut out, &file.name_lower);
        out.extend_from_slice(&file.name_len.to_le_bytes());
    }

    crate::index::serialize(&arena.search, &mut out);
    out
}

fn deserialize_arena(data: &[u8]) -> Result<IndexArena, redb::Error> {
    let mut cursor = 0usize;
    let magic = read_bytes(data, &mut cursor, 4)?;
    let v2 = magic == MAGIC_V2 || magic == MAGIC_V3;
    let v3 = magic == MAGIC_V3;
    if magic != MAGIC_V1 && !v2 {
        return Err(redb::Error::from(redb::StorageError::Corrupted(
            "invalid index magic".into(),
        )));
    }

    let root = read_str(data, &mut cursor)?;
    let global_midpoint = read_u16(data, &mut cursor)?;

    let dir_count = read_u32(data, &mut cursor)? as usize;
    let mut dirs = Vec::with_capacity(dir_count);
    for _ in 0..dir_count {
        let name = read_str(data, &mut cursor)?;
        let parent = read_opt_u32(data, &mut cursor)?;
        let child_dirs = read_u32_slice(data, &mut cursor)?;
        let file_ids = read_u32_slice(data, &mut cursor)?;
        let subtree_min_len = read_u16(data, &mut cursor)?;
        let subtree_max_len = read_u16(data, &mut cursor)?;
        let file_count = read_u32(data, &mut cursor)?;
        dirs.push(DirNode {
            name,
            parent,
            child_dirs,
            file_ids,
            subtree_min_len,
            subtree_max_len,
            file_count,
        });
    }

    let file_count = read_u32(data, &mut cursor)? as usize;
    let mut files = Vec::with_capacity(file_count);
    for _ in 0..file_count {
        let path = read_str(data, &mut cursor)?;
        let (name_lower, name_len) = if v2 {
            let name_lower = read_str(data, &mut cursor)?;
            let name_len = read_u16(data, &mut cursor)?;
            (name_lower, name_len)
        } else {
            let name_len = read_u16(data, &mut cursor)?;
            let name_lower = file_name_from_path(&path);
            (name_lower, name_len)
        };
        files.push(FileEntry {
            path,
            name_lower,
            name_len,
        });
    }

    let search = if v3 && cursor < data.len() {
        crate::index::deserialize(data, &mut cursor).map_err(|e| {
            redb::Error::from(redb::StorageError::Corrupted(e))
        })?
    } else {
        SearchIndex::build_from_files(&files, global_midpoint)
    };

    if cursor != data.len() {
        return Err(redb::Error::from(redb::StorageError::Corrupted(
            "trailing index bytes".into(),
        )));
    }
    Ok(IndexArena {
        root: root.into(),
        dirs,
        files,
        global_midpoint,
        search,
    })
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    write_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

fn write_opt_u32(out: &mut Vec<u8>, value: Option<u32>) {
    match value {
        Some(v) => {
            out.push(1);
            write_u32(out, v);
        }
        None => out.push(0),
    }
}

fn write_u32_slice(out: &mut Vec<u8>, values: &[u32]) {
    write_u32(out, values.len() as u32);
    for &v in values {
        write_u32(out, v);
    }
}

fn read_bytes<'a>(data: &'a [u8], cursor: &mut usize, len: usize) -> Result<&'a [u8], redb::Error> {
    let end = cursor
        .checked_add(len)
        .ok_or_else(|| redb::Error::from(redb::StorageError::Corrupted("index overflow".into())))?;
    if end > data.len() {
        return Err(redb::Error::from(redb::StorageError::Corrupted(
            "unexpected end of index".into(),
        )));
    }
    let slice = &data[*cursor..end];
    *cursor = end;
    Ok(slice)
}

fn read_u32(data: &[u8], cursor: &mut usize) -> Result<u32, redb::Error> {
    let bytes = read_bytes(data, cursor, 4)?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u16(data: &[u8], cursor: &mut usize) -> Result<u16, redb::Error> {
    let bytes = read_bytes(data, cursor, 2)?;
    Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_str(data: &[u8], cursor: &mut usize) -> Result<String, redb::Error> {
    let len = read_u32(data, cursor)? as usize;
    let bytes = read_bytes(data, cursor, len)?;
    String::from_utf8(bytes.to_vec()).map_err(|e| {
        redb::Error::from(redb::StorageError::Corrupted(format!(
            "invalid utf8 in index: {e}"
        )))
    })
}

fn read_opt_u32(data: &[u8], cursor: &mut usize) -> Result<Option<u32>, redb::Error> {
    let tag = *read_bytes(data, cursor, 1)?.first().unwrap();
    match tag {
        0 => Ok(None),
        1 => Ok(Some(read_u32(data, cursor)?)),
        _ => Err(redb::Error::from(redb::StorageError::Corrupted(
            "invalid optional u32 tag".into(),
        ))),
    }
}

fn read_u32_slice(data: &[u8], cursor: &mut usize) -> Result<Vec<u32>, redb::Error> {
    let len = read_u32(data, cursor)? as usize;
    let byte_len = len * 4;
    let end = cursor
        .checked_add(byte_len)
        .ok_or_else(|| redb::Error::from(redb::StorageError::Corrupted("index overflow".into())))?;
    if end > data.len() {
        return Err(redb::Error::from(redb::StorageError::Corrupted(
            "unexpected end of index".into(),
        )));
    }
    let slice = &data[*cursor..end];
    *cursor = end;

    let mut out = Vec::with_capacity(len);
    for chunk in slice.chunks_exact(4) {
        out.push(u32::from_le_bytes(chunk.try_into().unwrap()));
    }
    Ok(out)
}