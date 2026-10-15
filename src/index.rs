use crate::arena::{FileEntry, IndexArena};
use std::{borrow::Cow, collections::HashMap};

const MAX_NAME_LEN: usize = 512;

#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    pub len_buckets: Vec<Vec<u32>>,
    pub trigram_postings: HashMap<u32, Box<[u32]>>,
    pub visit_order: Vec<u32>,
}

impl SearchIndex {
    pub fn build(arena: &IndexArena) -> Self {
        Self::build_from_files(&arena.files, arena.global_midpoint)
    }

    pub fn build_from_files(files: &[FileEntry], midpoint: u16) -> Self {
        let mut len_buckets = vec![Vec::new(); MAX_NAME_LEN + 1];
        let mut postings: HashMap<u32, Vec<u32>> = HashMap::new();
        for (ordinal, file) in files.iter().enumerate() {
            let identifier = u32::try_from(ordinal).expect("file catalog exceeds 32-bit IDs");
            len_buckets[usize::from(file.name_len).min(MAX_NAME_LEN)].push(identifier);
            for window in file.name_lower.as_bytes().windows(3) {
                let posting = postings.entry(pack_trigram(window)).or_default();
                // Ordered IDs let us deduplicate repeated grams without sorting each posting.
                if posting.last() != Some(&identifier) {
                    posting.push(identifier);
                }
            }
        }
        let trigram_postings = postings
            .into_iter()
            .map(|(gram, posting)| (gram, posting.into_boxed_slice()))
            .collect();
        let visit_order = middle_out_bucket_order(&len_buckets, midpoint);
        Self {
            len_buckets,
            trigram_postings,
            visit_order,
        }
    }

    /// A missing required gram proves absence; a short query needs a complete scan.
    pub fn candidates_for(&self, term_lower: &str) -> CandidateSet<'_> {
        if term_lower.len() < 3 {
            return CandidateSet::MiddleOut(&self.visit_order);
        }
        let mut shortest: Option<&[u32]> = None;
        for window in term_lower.as_bytes().windows(3) {
            let Some(posting) = self.trigram_postings.get(&pack_trigram(window)) else {
                return CandidateSet::Filtered(Cow::Borrowed(&[]));
            };
            if shortest.is_none_or(|current| posting.len() < current.len()) {
                shortest = Some(posting);
            }
        }
        CandidateSet::Filtered(Cow::Borrowed(shortest.unwrap_or(&[])))
    }
}

pub enum CandidateSet<'index> {
    Filtered(Cow<'index, [u32]>),
    MiddleOut(&'index [u32]),
}

fn middle_out_bucket_order(buckets: &[Vec<u32>], midpoint: u16) -> Vec<u32> {
    let mut order = Vec::with_capacity(buckets.iter().map(Vec::len).sum());
    let middle = usize::from(midpoint).min(MAX_NAME_LEN);
    order.extend_from_slice(&buckets[middle]);
    for offset in 1..=MAX_NAME_LEN {
        if let Some(bucket) = buckets.get(middle + offset) {
            order.extend_from_slice(bucket);
        }
        if let Some(lower) = middle.checked_sub(offset) {
            order.extend_from_slice(&buckets[lower]);
        }
    }
    order
}

pub fn pack_trigram(bytes: &[u8]) -> u32 {
    (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2])
}

pub fn file_name_from_path(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_lowercase()
}

pub fn name_len_chars(name: &str) -> u16 {
    name.chars().count().min(usize::from(u16::MAX)) as u16
}

pub fn push_trigram_postings(name: &str, identifier: u32, postings: &mut HashMap<u32, Vec<u32>>) {
    for window in name.as_bytes().windows(3) {
        postings
            .entry(pack_trigram(window))
            .or_default()
            .push(identifier);
    }
}

pub fn finalize_trigram_postings(postings: &mut HashMap<u32, Vec<u32>>) {
    for posting in postings.values_mut() {
        posting.sort_unstable();
        posting.dedup();
    }
}

pub fn serialize(index: &SearchIndex, output: &mut Vec<u8>) {
    write_ids(output, &index.visit_order);
    let mut entries: Vec<_> = index.trigram_postings.iter().collect();
    entries.sort_unstable_by_key(|&(gram, _)| *gram);
    write_u32(output, entries.len() as u32);
    for (&gram, posting) in entries {
        write_u32(output, gram);
        write_ids(output, posting);
    }
}

pub fn deserialize(data: &[u8], cursor: &mut usize) -> Result<SearchIndex, String> {
    let visit_order = read_ids(data, cursor)?;
    let count = read_u32(data, cursor)? as usize;
    if count > data.len().saturating_sub(*cursor) / 8 {
        return Err("invalid posting count".into());
    }
    let mut trigram_postings = HashMap::with_capacity(count);
    for _ in 0..count {
        let gram = read_u32(data, cursor)?;
        let posting = read_ids(data, cursor)?.into_boxed_slice();
        if trigram_postings.insert(gram, posting).is_some() {
            return Err("duplicate posting gram".into());
        }
    }
    Ok(SearchIndex {
        len_buckets: Vec::new(),
        trigram_postings,
        visit_order,
    })
}

fn write_u32(output: &mut Vec<u8>, number: u32) {
    output.extend_from_slice(&number.to_le_bytes());
}

fn write_ids(output: &mut Vec<u8>, identifiers: &[u32]) {
    write_u32(output, identifiers.len() as u32);
    for &identifier in identifiers {
        write_u32(output, identifier);
    }
}

fn read_u32(data: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let end = cursor.checked_add(4).ok_or("index offset overflow")?;
    let bytes = data.get(*cursor..end).ok_or("unexpected end of index")?;
    *cursor = end;
    Ok(u32::from_le_bytes(
        bytes.try_into().expect("four-byte range"),
    ))
}

fn read_ids(data: &[u8], cursor: &mut usize) -> Result<Vec<u32>, String> {
    let count = read_u32(data, cursor)? as usize;
    if count > data.len().saturating_sub(*cursor) / 4 {
        return Err("invalid identifier count".into());
    }
    (0..count).map(|_| read_u32(data, cursor)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_trigrams() {
        assert_eq!(pack_trigram(b"abc"), 0x00616263);
    }

    #[test]
    fn bucket_order_contains_every_id_exactly_once_even_at_the_edges() {
        for midpoint in [0, 1, 256, 512, u16::MAX] {
            let buckets: Vec<_> = (0..=MAX_NAME_LEN)
                .map(|ordinal| vec![ordinal as u32])
                .collect();
            let mut order = middle_out_bucket_order(&buckets, midpoint);
            order.sort_unstable();
            assert_eq!(order, (0..=MAX_NAME_LEN as u32).collect::<Vec<_>>());
        }
    }
}
