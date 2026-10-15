use crate::{arena::IndexArena, index::CandidateSet};
use std::{cmp::Reverse, collections::BinaryHeap};

pub use crate::content::{
    CaseMode, ContentSearchOptions, ContentSearchReport, LineMatch, search_line_contents,
    search_line_contents_with_cancel, search_line_contents_with_options,
};

/// Unicode-lowercase filename substrings, ordered by distance from mean name length then ID.
pub fn middle_out_search(arena: &IndexArena, term: &str, max_results: usize) -> Vec<String> {
    if term.is_empty() || max_results == 0 || arena.files.is_empty() {
        return Vec::new();
    }
    let normalized = term.to_lowercase();
    match arena.search_index().candidates_for(&normalized) {
        CandidateSet::Filtered(candidates) if candidates.is_empty() => Vec::new(),
        CandidateSet::Filtered(candidates) if candidates.len() < arena.files.len() / 4 => {
            select_filtered(arena, &candidates, &normalized, max_results)
        }
        // A common posting is less useful than the precomputed order and an early stop.
        _ => scan_order(arena, &normalized, max_results),
    }
}

fn scan_order(arena: &IndexArena, term: &str, limit: usize) -> Vec<String> {
    arena
        .search_index()
        .visit_order
        .iter()
        .map(|&identifier| &arena.files[identifier as usize])
        .filter(|file| file.name_lower.contains(term))
        .take(limit)
        .map(|file| file.path.clone())
        .collect()
}

fn select_filtered(
    arena: &IndexArena,
    identifiers: &[u32],
    term: &str,
    limit: usize,
) -> Vec<String> {
    let mut selected = BinaryHeap::with_capacity(limit.min(identifiers.len()));
    for &identifier in identifiers {
        let file = &arena.files[identifier as usize];
        if !file.name_lower.contains(term) {
            continue;
        }
        let rank = (
            file.name_len.abs_diff(arena.global_midpoint),
            Reverse(file.name_len),
            identifier,
        );
        if selected.len() < limit {
            selected.push(rank);
        } else if selected.peek().is_some_and(|worst| rank < *worst) {
            *selected.peek_mut().expect("nonempty selection") = rank;
        }
    }
    selected
        .into_sorted_vec()
        .into_iter()
        .map(|(_, _, identifier)| arena.files[identifier as usize].path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::IndexArena;
    use std::path::PathBuf;

    #[test]
    fn test_search_line_contents() {
        let dir = std::env::temp_dir();
        let file_path = dir.join("test_line_search.txt");
        std::fs::write(&file_path, "Hello World\nLine two with match\nLine three").unwrap();

        let root = dir.clone();
        let paths = vec![file_path.clone()];
        let arena = IndexArena::from_paths(root, paths);
        let hits = search_line_contents(&arena, "match", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line_number, 2);
        assert_eq!(hits[0].content, "Line two with match");

        let _ = std::fs::remove_file(file_path);
    }

    #[test]
    fn test_parallel_middle_out_line_search() {
        let dir = std::env::temp_dir();
        let file_path = dir.join("test_large_line_search.txt");

        // Generate a file > 100 KB with 300 lines.
        // Line 150 will contain our search target.
        let mut lines = Vec::new();
        for idx in 0..300 {
            if idx == 150 {
                lines
                    .push("This line has the special keyword middleoutcrawler target!".to_string());
            } else {
                lines.push(format!("This is line {} with dummy data to make the file size larger than one hundred kilobytes and occupy some disk space, padded padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding", idx));
            }
        }
        std::fs::write(&file_path, lines.join("\n")).unwrap();

        let root = dir.clone();
        let paths = vec![file_path.clone()];
        let arena = IndexArena::from_paths(root, paths);
        let hits = search_line_contents(&arena, "middleoutcrawler", 5);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line_number, 151); // 1-indexed
        assert!(hits[0].content.contains("middleoutcrawler target"));

        let _ = std::fs::remove_file(file_path);
    }

    #[test]
    fn finds_matching_files() {
        let root = PathBuf::from("/code");
        let paths = vec![
            PathBuf::from("/code/src/main.rs"),
            PathBuf::from("/code/src/lib.rs"),
            PathBuf::from("/code/Cargo.toml"),
        ];
        let arena = IndexArena::from_paths(root, paths);
        let hits = middle_out_search(&arena, "main", 10);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].ends_with("main.rs"));
    }

    #[test]
    fn graph_prunes_empty_subtrees() {
        let root = PathBuf::from("/code");
        let paths = vec![
            PathBuf::from("/code/a/short.txt"),
            PathBuf::from("/code/deep/nested/verylongfilename.rs"),
        ];
        let arena = IndexArena::from_paths(root, paths);
        let hits = middle_out_search(&arena, "verylongfilename", 5);
        assert_eq!(hits.len(), 1);
    }
}
