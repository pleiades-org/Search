use crate::arena::{FileEntry, IndexArena};
use std::collections::HashMap;

const MAX_NAME_LEN: usize = 512;

#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    pub len_buckets: Vec<Vec<u32>>,
    pub trigram_postings: HashMap<u32, Vec<u32>>,
    pub visit_order: Vec<u32>,
}

impl SearchIndex {
    pub fn build(arena: &IndexArena) -> Self {
        Self::build_from_files(&arena.files, arena.global_midpoint)
    }

    pub fn build_from_files(files: &[FileEntry], midpoint: u16) -> Self {
        let mut len_buckets = vec![Vec::new(); MAX_NAME_LEN + 1];
        let mut trigram_postings: HashMap<u32, Vec<u32>> = HashMap::new();

        for (file_id, file) in files.iter().enumerate() {
            let id = file_id as u32;
            let bucket = file.name_len.min(MAX_NAME_LEN as u16) as usize;
            len_buckets[bucket].push(id);

            let name = file.name_lower.as_bytes();
            if name.len() >= 3 {
                for window in name.windows(3) {
                    let key = pack_trigram(window);
                    trigram_postings.entry(key).or_default().push(id);
                }
            }
        }

        for list in trigram_postings.values_mut() {
            list.sort_unstable();
            list.dedup();
        }

        let visit_order = middle_out_bucket_order(&len_buckets, midpoint);

        Self {
            len_buckets,
            trigram_postings,
            visit_order,
        }
    }

    pub fn candidates_for(&self, term_lower: &str) -> CandidateSet<'_> {
        let bytes = term_lower.as_bytes();
        if bytes.len() >= 3 {
            let mut lists: Vec<&[u32]> = bytes
                .windows(3)
                .filter_map(|window| {
                    self.trigram_postings
                        .get(&pack_trigram(window))
                        .map(Vec::as_slice)
                })
                .collect();

            lists.sort_by_key(|list| list.len());

            if !lists.is_empty() {
                let mut intersected = std::borrow::Cow::Borrowed(lists[0]);
                for &next_list in lists.iter().skip(1).take(2) {
                    if next_list.is_empty() {
                        intersected = std::borrow::Cow::Owned(Vec::new());
                        break;
                    }
                    intersected = std::borrow::Cow::Owned(intersect(&intersected, next_list));
                }
                if intersected.len() < self.visit_order.len() / 4 {
                    return CandidateSet::Filtered(intersected);
                }
            }
        }

        CandidateSet::MiddleOut(&self.visit_order)
    }
}

pub enum CandidateSet<'a> {
    Filtered(std::borrow::Cow<'a, [u32]>),
    MiddleOut(&'a [u32]),
}

fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(a.len().min(b.len()));
    let mut i = 0;
    let mut j = 0;
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            result.push(a[i]);
            i += 1;
            j += 1;
        } else if a[i] < b[j] {
            i += 1;
        } else {
            j += 1;
        }
    }
    result
}

fn middle_out_bucket_order(len_buckets: &[Vec<u32>], midpoint: u16) -> Vec<u32> {
    let mut order = Vec::new();
    let mid = midpoint.min(MAX_NAME_LEN as u16) as usize;

    order.extend_from_slice(&len_buckets[mid]);
    for offset in 1..=MAX_NAME_LEN {
        let hi = mid + offset;
        let lo = mid.saturating_sub(offset);
        if hi <= MAX_NAME_LEN {
            order.extend_from_slice(&len_buckets[hi]);
        }
        if lo != hi {
            order.extend_from_slice(&len_buckets[lo]);
        }
        if hi > MAX_NAME_LEN && lo == 0 {
            break;
        }
    }

    order
}

pub fn pack_trigram(bytes: &[u8]) -> u32 {
    (bytes[0] as u32) << 16 | (bytes[1] as u32) << 8 | bytes[2] as u32
}

pub fn file_name_from_path(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase()
}

pub fn name_len_chars(name: &str) -> u16 {
    name.chars().count().min(u16::MAX as usize) as u16
}

pub fn push_trigram_postings(name_lower: &str, file_id: u32, postings: &mut HashMap<u32, Vec<u32>>) {
    let bytes = name_lower.as_bytes();
    if bytes.len() < 3 {
        return;
    }
    for window in bytes.windows(3) {
        postings
            .entry(pack_trigram(window))
            .or_default()
            .push(file_id);
    }
}

pub fn serialize(search: &SearchIndex, out: &mut Vec<u8>) {
    write_u32_slice(out, &search.visit_order);
    let mut entries: Vec<_> = search.trigram_postings.iter().collect();
    entries.sort_by_key(|(k, _)| **k);
    write_u32(out, entries.len() as u32);
    for (key, postings) in entries {
        out.extend_from_slice(&key.to_le_bytes());
        write_u32_slice(out, postings);
    }
}

pub fn deserialize(data: &[u8], cursor: &mut usize) -> Result<SearchIndex, String> {
    let visit_order = read_u32_slice(data, cursor)?;
    let entry_count = read_u32(data, cursor)? as usize;
    let mut trigram_postings = HashMap::with_capacity(entry_count);
    for _ in 0..entry_count {
        let key = read_u32(data, cursor)?;
        let postings = read_u32_slice(data, cursor)?;
        trigram_postings.insert(key, postings);
    }
    Ok(SearchIndex {
        len_buckets: Vec::new(),
        trigram_postings,
        visit_order,
    })
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32_slice(out: &mut Vec<u8>, values: &[u32]) {
    write_u32(out, values.len() as u32);
    for &v in values {
        write_u32(out, v);
    }
}

fn read_u32(data: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let end = cursor
        .checked_add(4)
        .ok_or_else(|| "search index overflow".to_string())?;
    if end > data.len() {
        return Err("unexpected end of search index".into());
    }
    let val = u32::from_le_bytes(data[*cursor..end].try_into().unwrap());
    *cursor = end;
    Ok(val)
}

fn read_u32_slice(data: &[u8], cursor: &mut usize) -> Result<Vec<u32>, String> {
    let len = read_u32(data, cursor)? as usize;
    let byte_len = len * 4;
    let end = cursor
        .checked_add(byte_len)
        .ok_or_else(|| "search index overflow".to_string())?;
    if end > data.len() {
        return Err("unexpected end of search index".into());
    }
    let slice = &data[*cursor..end];
    *cursor = end;

    let mut out = Vec::with_capacity(len);
    for chunk in slice.chunks_exact(4) {
        out.push(u32::from_le_bytes(chunk.try_into().unwrap()));
    }
    Ok(out)
}

pub fn finalize_trigram_postings(postings: &mut HashMap<u32, Vec<u32>>) {
    for list in postings.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_trigrams() {
        assert_eq!(pack_trigram(b"abc"), 0x00616263);
    }
}