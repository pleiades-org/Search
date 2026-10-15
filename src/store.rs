use crate::arena::IndexArena;
use redb::{Database, TableDefinition};
use std::{
    collections::HashSet,
    io,
    path::{Component, Path},
};

const INDEX_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("index");
const MAGIC: &[u8; 4] = b"CSH4";
const MAX_CACHE_BYTES: usize = 512 * 1024 * 1024;

pub struct IndexStore {
    database: Database,
}

impl IndexStore {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            database: Database::create(path).map_err(io::Error::other)?,
        })
    }

    pub fn save(&self, arena: &IndexArena) -> io::Result<()> {
        let blob = serialize_arena(arena)?;
        let transaction = self.database.begin_write().map_err(io::Error::other)?;
        {
            let mut table = transaction
                .open_table(INDEX_TABLE)
                .map_err(io::Error::other)?;
            table
                .insert("arena", blob.as_slice())
                .map_err(io::Error::other)?;
        }
        transaction.commit().map_err(io::Error::other)
    }

    pub fn load(&self) -> io::Result<Option<IndexArena>> {
        let transaction = self.database.begin_read().map_err(io::Error::other)?;
        let table = match transaction.open_table(INDEX_TABLE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(io::Error::other(error)),
        };
        let Some(value) = table.get("arena").map_err(io::Error::other)? else {
            return Ok(None);
        };
        deserialize_arena(value.value()).map(Some)
    }
}

/// Persist canonical source paths only: reconstruct graph IDs and Unicode grams on load.
fn serialize_arena(arena: &IndexArena) -> io::Result<Vec<u8>> {
    let mut output = MAGIC.to_vec();
    write_path(&mut output, &arena.root)?;
    let count = u32::try_from(arena.files.len()).map_err(|_| corrupt("too many cached paths"))?;
    output.extend_from_slice(&count.to_le_bytes());
    for file in &arena.files {
        write_path(&mut output, &file.native_path)?;
        if output.len() > MAX_CACHE_BYTES {
            return Err(corrupt("cache exceeds size limit"));
        }
    }
    Ok(output)
}

fn deserialize_arena(data: &[u8]) -> io::Result<IndexArena> {
    if data.len() > MAX_CACHE_BYTES || data.get(..4) != Some(MAGIC) {
        return Err(corrupt("unsupported or oversized cache; rebuild required"));
    }
    let mut cursor = 4;
    let root = read_path(data, &mut cursor)?;
    let count = read_u32(data, &mut cursor)? as usize;
    if count > data.len().saturating_sub(cursor) / 4 {
        return Err(corrupt("invalid path count"));
    }
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for _ in 0..count {
        let path = read_path(data, &mut cursor)?;
        let relative = path
            .strip_prefix(&root)
            .map_err(|_| corrupt("cached path outside root"))?;
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            || !seen.insert(path.clone())
        {
            return Err(corrupt("invalid or duplicate cached path"));
        }
        paths.push(path);
    }
    if cursor != data.len() {
        return Err(corrupt("trailing cache bytes"));
    }
    Ok(IndexArena::from_paths(root, paths))
}

fn write_path(output: &mut Vec<u8>, path: &Path) -> io::Result<()> {
    // The cache is native-platform specific; UTF-16 on Windows and raw bytes on Unix.
    #[cfg(windows)]
    let encoded: Vec<u8> = {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    };
    #[cfg(not(windows))]
    let encoded: Vec<u8> = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    let length = u32::try_from(encoded.len()).map_err(|_| corrupt("cached path too long"))?;
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(&encoded);
    Ok(())
}

fn read_path(data: &[u8], cursor: &mut usize) -> io::Result<std::path::PathBuf> {
    let length = read_u32(data, cursor)? as usize;
    let end = cursor
        .checked_add(length)
        .ok_or_else(|| corrupt("path offset overflow"))?;
    let bytes = data
        .get(*cursor..end)
        .ok_or_else(|| corrupt("truncated path"))?;
    *cursor = end;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        if bytes.len() % 2 != 0 {
            return Err(corrupt("invalid UTF-16 path"));
        }
        let units: Vec<_> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Ok(std::ffi::OsString::from_wide(&units).into())
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes.to_vec()).into())
    }
}

fn read_u32(data: &[u8], cursor: &mut usize) -> io::Result<u32> {
    let end = cursor
        .checked_add(4)
        .ok_or_else(|| corrupt("cache offset overflow"))?;
    let bytes = data
        .get(*cursor..end)
        .ok_or_else(|| corrupt("truncated cache"))?;
    *cursor = end;
    Ok(u32::from_le_bytes(
        bytes.try_into().expect("four-byte range"),
    ))
}

fn corrupt(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_caches_fail_without_unbounded_allocations_or_panics() {
        for bytes in [vec![], b"CSH3".to_vec(), b"CSH4\xff\xff\xff\xff".to_vec()] {
            assert!(deserialize_arena(&bytes).is_err());
        }
    }

    #[test]
    fn path_cache_roundtrip_reconstructs_consistent_ids_and_unicode_names() {
        let root = std::path::PathBuf::from("/source");
        let arena = IndexArena::from_paths(
            root.clone(),
            vec![root.join("a/CAFÉ.txt"), root.join("b/other.rs")],
        );
        let restored = deserialize_arena(&serialize_arena(&arena).unwrap()).unwrap();
        assert_eq!(restored.files[0].name_lower, "café.txt");
        assert_eq!(restored.dirs[1].file_ids, vec![0]);
    }
}
