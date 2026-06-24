use crate::arena::IndexArena;
use crate::index::CandidateSet;
use rayon::prelude::*;
use memchr::memmem::Finder;

pub fn middle_out_search(arena: &IndexArena, term: &str, max_results: usize) -> Vec<String> {
    if term.is_empty() || max_results == 0 || arena.files.is_empty() {
        return Vec::new();
    }

    let term_lower = term.to_ascii_lowercase();
    let index = arena.search_index();
    let midpoint = arena.global_midpoint;
    let candidates = index.candidates_for(&term_lower);

    let mut results = Vec::with_capacity(max_results.min(arena.files.len()));
    let mut seen = vec![false; arena.files.len()];

    match candidates {
        CandidateSet::Filtered(list) => {
            collect_matches(
                arena,
                &list,
                &term_lower,
                midpoint,
                max_results,
                &mut results,
                &mut seen,
            );
            if results.len() < max_results {
                collect_matches(
                    arena,
                    &index.visit_order,
                    &term_lower,
                    midpoint,
                    max_results,
                    &mut results,
                    &mut seen,
                );
            }
        }
        CandidateSet::MiddleOut(list) => {
            collect_graph_matches(
                arena,
                &term_lower,
                midpoint,
                max_results,
                &mut results,
                &mut seen,
            );
            if results.len() < max_results {
                collect_matches(
                    arena,
                    list,
                    &term_lower,
                    midpoint,
                    max_results,
                    &mut results,
                    &mut seen,
                );
            }
        }
    }

    results
}

fn collect_graph_matches(
    arena: &IndexArena,
    term_lower: &str,
    midpoint: u16,
    max_results: usize,
    results: &mut Vec<String>,
    seen: &mut [bool],
) {
    let term_len = term_lower.chars().count().min(u16::MAX as usize) as u16;
    let mut stack = vec![(0u32, distance_to_interval(midpoint, arena.dirs[0].subtree_min_len, arena.dirs[0].subtree_max_len))];

    while let Some((dir_id, _)) = stack.pop() {
        if results.len() >= max_results {
            break;
        }

        let dir = &arena.dirs[dir_id as usize];
        if !subtree_may_match(dir.subtree_min_len, dir.subtree_max_len, term_len) {
            continue;
        }

        let mut children: Vec<(u32, u16)> = dir
            .child_dirs
            .iter()
            .map(|&child_id| {
                let child = &arena.dirs[child_id as usize];
                (
                    child_id,
                    distance_to_interval(midpoint, child.subtree_min_len, child.subtree_max_len),
                )
            })
            .collect();
        children.sort_by_key(|(_, dist)| *dist);

        for (child_id, _) in children {
            stack.push((child_id, 0));
        }

        let mut files: Vec<(u32, u16)> = dir
            .file_ids
            .iter()
            .map(|&file_id| {
                let file = &arena.files[file_id as usize];
                (file_id, file.name_len.abs_diff(midpoint))
            })
            .collect();
        files.sort_by_key(|(_, dist)| *dist);

        for (file_id, _) in files {
            let id = file_id as usize;
            if seen[id] {
                continue;
            }
            let file = &arena.files[id];
            if file.name_lower.contains(term_lower) {
                seen[id] = true;
                results.push(file.path.clone());
                if results.len() >= max_results {
                    return;
                }
            }
        }
    }
}

fn collect_matches(
    arena: &IndexArena,
    order: &[u32],
    term_lower: &str,
    midpoint: u16,
    max_results: usize,
    results: &mut Vec<String>,
    seen: &mut [bool],
) {
    let mut prioritized: Vec<(u16, u32)> = order
        .iter()
        .filter_map(|&file_id| {
            let id = file_id as usize;
            if seen[id] {
                return None;
            }
            let file = &arena.files[id];
            Some((file.name_len.abs_diff(midpoint), file_id))
        })
        .collect();
    prioritized.sort_by_key(|(dist, _)| *dist);

    for (_, file_id) in prioritized {
        let id = file_id as usize;
        if seen[id] {
            continue;
        }
        seen[id] = true;
        let file = &arena.files[id];
        if file.name_lower.contains(term_lower) {
            results.push(file.path.clone());
            if results.len() >= max_results {
                break;
            }
        }
    }
}

fn distance_to_interval(target: u16, min_len: u16, max_len: u16) -> u16 {
    if target < min_len {
        min_len - target
    } else if target > max_len {
        target - max_len
    } else {
        0
    }
}

fn subtree_may_match(min_len: u16, max_len: u16, term_len: u16) -> bool {
    if term_len == 0 {
        return true;
    }
    max_len + 8 >= term_len || min_len <= term_len
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    pub path: String,
    pub line_number: usize,
    pub content: String,
}

pub fn search_line_contents(arena: &IndexArena, term: &str, max_results: usize) -> Vec<LineMatch> {
    if term.is_empty() || max_results == 0 || arena.files.is_empty() {
        return Vec::new();
    }
    let term_lower = term.to_ascii_lowercase();
    let index = arena.search_index();
    let candidates = index.candidates_for(&term_lower);

    let mut search_order = Vec::with_capacity(arena.files.len());
    match candidates {
        CandidateSet::Filtered(list) => {
            search_order.extend_from_slice(&list);
            let priority_set: std::collections::HashSet<u32> = list.iter().copied().collect();
            for &id in &index.visit_order {
                if !priority_set.contains(&id) {
                    search_order.push(id);
                }
            }
        }
        CandidateSet::MiddleOut(list) => {
            search_order.extend_from_slice(list);
        }
    }

    let count = std::sync::atomic::AtomicUsize::new(0);
    let mut results: Vec<LineMatch> = search_order
        .par_iter()
        .flat_map_iter(|&file_id| {
            if count.load(std::sync::atomic::Ordering::Relaxed) >= max_results {
                return Vec::new().into_iter();
            }
            let file = &arena.files[file_id as usize];
            search_file_for_lines_parallel(&file.path, &term_lower, &count, max_results).into_iter()
        })
        .collect();

    results.truncate(max_results);
    results
}

fn search_file_for_lines_parallel(
    path: &str,
    term_lower: &str,
    global_count: &std::sync::atomic::AtomicUsize,
    max_results: usize,
) -> Vec<LineMatch> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Vec::new();
    };

    // If the file is smaller than 100 KB, use sequential line search
    if metadata.len() < 102400 {
        return search_file_for_lines(path, term_lower, global_count, max_results);
    }

    let Ok(data) = std::fs::read(path) else {
        return search_file_for_lines(path, term_lower, global_count, max_results);
    };

    if data.contains(&0) {
        return Vec::new(); // skip binary files
    }

    // Find all newline offsets to map to lines
    let mut line_offsets = vec![0];
    for pos in memchr::memchr_iter(b'\n', &data) {
        line_offsets.push(pos + 1);
    }

    let num_lines = line_offsets.len();
    if num_lines < 200 {
        return search_file_for_lines(path, term_lower, global_count, max_results);
    }

    // Determine number of crawlers
    let num_crawlers = 4.min(num_lines);
    let chunk_size = num_lines / num_crawlers;

    let mut matches: Vec<LineMatch> = (0..num_crawlers)
        .into_par_iter()
        .flat_map(|c_id| {
            if global_count.load(std::sync::atomic::Ordering::Relaxed) >= max_results {
                return Vec::new();
            }

            let start_line = c_id * chunk_size;
            let end_line = if c_id == num_crawlers - 1 { num_lines } else { (c_id + 1) * chunk_size };
            let len = end_line - start_line;
            let mid = start_line + len / 2;

            let mut chunk_matches = Vec::new();
            let finder = Finder::new(term_lower.as_bytes());

            // Search middle-out within the line range [start_line, end_line)
            for step in 0..len {
                if global_count.load(std::sync::atomic::Ordering::Relaxed) >= max_results {
                    break;
                }

                let idx = if step % 2 == 0 {
                    let offset = (step / 2) as usize;
                    if mid + offset < end_line {
                        mid + offset
                    } else {
                        continue;
                    }
                } else {
                    let offset = ((step + 1) / 2) as usize;
                    if mid >= offset && mid - offset >= start_line {
                        mid - offset
                    } else {
                        continue;
                    }
                };

                let start_offset = line_offsets[idx];
                let end_offset = if idx + 1 < num_lines {
                    line_offsets[idx + 1]
                } else {
                    data.len()
                };

                let line_bytes = &data[start_offset..end_offset];
                let mut line_stripped = line_bytes;
                while line_stripped.ends_with(b"\n") || line_stripped.ends_with(b"\r") {
                    line_stripped = &line_stripped[..line_stripped.len() - 1];
                }

                if finder.find(line_stripped).is_some() {
                    if let Ok(content) = std::str::from_utf8(line_stripped) {
                        chunk_matches.push(LineMatch {
                            path: path.to_owned(),
                            line_number: idx + 1,
                            content: content.to_owned(),
                        });
                        global_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }

            chunk_matches
        })
        .collect();

    // Sort single-file matches by line number for consistent output
    matches.sort_by_key(|m| m.line_number);
    matches
}

fn search_file_for_lines(
    path: &str,
    term_lower: &str,
    global_count: &std::sync::atomic::AtomicUsize,
    max_results: usize,
) -> Vec<LineMatch> {
    let mut matches = Vec::new();
    let Ok(data) = std::fs::read(path) else {
        return matches;
    };
    if data.contains(&0) {
        return matches; // skip binary files
    }

    let finder = Finder::new(term_lower.as_bytes());
    let mut line_number = 1;
    let mut last_newline_pos = 0;
    let mut cursor = 0;

    while let Some(pos) = finder.find(&data[cursor..]) {
        if global_count.load(std::sync::atomic::Ordering::Relaxed) >= max_results {
            break;
        }
        let abs_match_pos = cursor + pos;

        // Advance line count up to the match position
        let scan_segment = &data[last_newline_pos..abs_match_pos];
        let newlines = memchr::memchr_iter(b'\n', scan_segment).count();
        line_number += newlines;

        // Locate boundaries of the matching line
        let line_start = memchr::memrchr(b'\n', &data[..abs_match_pos])
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let line_end = memchr::memchr(b'\n', &data[abs_match_pos..])
            .map(|idx| abs_match_pos + idx)
            .unwrap_or(data.len());

        let line_bytes = &data[line_start..line_end];
        let mut line_stripped = line_bytes;
        while line_stripped.ends_with(b"\n") || line_stripped.ends_with(b"\r") {
            line_stripped = &line_stripped[..line_stripped.len() - 1];
        }

        if let Ok(content) = std::str::from_utf8(line_stripped) {
            matches.push(LineMatch {
                path: path.to_owned(),
                line_number,
                content: content.to_owned(),
            });
            global_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        // Sync the line counter tracking
        last_newline_pos = line_end;
        cursor = line_end + 1;
        if cursor >= data.len() {
            break;
        }
    }
    matches
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
                lines.push("This line has the special keyword middleoutcrawler target!".to_string());
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