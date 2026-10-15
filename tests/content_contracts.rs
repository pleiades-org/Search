use codesearch::{
    arena::IndexArena,
    content::{
        CaseMode, ContentSearchOptions, search_line_contents_with_cancel,
        search_line_contents_with_options,
    },
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

fn arena_with_files(contents: &[&[u8]]) -> IndexArena {
    let root = std::env::temp_dir().join("codesearch-tests").join(format!(
        "content-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let paths: Vec<PathBuf> = contents
        .iter()
        .enumerate()
        .map(|(ordinal, bytes)| {
            let path = root.join(format!("file-{ordinal}.txt"));
            fs::write(&path, bytes).unwrap();
            path
        })
        .collect();
    IndexArena::from_paths(root, paths)
}

fn small_buffers() -> ContentSearchOptions {
    ContentSearchOptions {
        buffer_bytes: 16,
        max_line_bytes: 256,
        parallel_files: 2,
        ..ContentSearchOptions::default()
    }
}

#[test]
fn buffer_boundaries_crlf_repeated_hits_and_final_lines_preserve_full_tuples() {
    let arena = arena_with_files(&[b"first\r\nxxxxxxxxxxxxx needle needle\r\nneedle"]);
    let report = search_line_contents_with_options(&arena, "needle", 10, &small_buffers());
    assert!(report.errors.is_empty());
    assert_eq!(
        report
            .matches
            .iter()
            .map(|hit| (hit.line_number, hit.content.as_str()))
            .collect::<Vec<_>>(),
        vec![(2, "xxxxxxxxxxxxx needle needle"), (3, "needle")]
    );
}

#[test]
fn unicode_insensitive_mode_preserves_original_line_text() {
    let arena = arena_with_files(&["CAFÉ\ncafé\nCafé\n".as_bytes()]);
    let options = ContentSearchOptions {
        case_mode: CaseMode::Insensitive,
        ..small_buffers()
    };
    let report = search_line_contents_with_options(&arena, "café", 10, &options);
    assert_eq!(report.matches.len(), 3);
    assert_eq!(report.matches[0].content, "CAFÉ");
}

#[test]
fn limits_keep_catalog_and_source_line_order_across_parallel_files() {
    let arena = arena_with_files(&[b"needle first\nneedle second\n", b"needle third\n"]);
    for _ in 0..20 {
        let report = search_line_contents_with_options(&arena, "needle", 2, &small_buffers());
        assert_eq!(
            report
                .matches
                .iter()
                .map(|hit| hit.content.as_str())
                .collect::<Vec<_>>(),
            vec!["needle first", "needle second"]
        );
    }
}

#[test]
fn binary_prefixes_are_skipped_and_missing_files_are_reported() {
    let mut arena = arena_with_files(&[b"needle\0binary"]);
    let missing = arena.root.join("missing.txt");
    arena = IndexArena::from_paths(
        arena.root.clone(),
        vec![arena.files[0].native_path.clone(), missing],
    );
    let report = search_line_contents_with_options(&arena, "needle", 10, &small_buffers());
    assert!(report.matches.is_empty());
    assert_eq!(report.errors.len(), 1);
}

#[test]
fn excessive_lines_and_invalid_options_fail_with_diagnostics() {
    let text = "x".repeat(257);
    let arena = arena_with_files(&[text.as_bytes()]);
    let report = search_line_contents_with_options(&arena, "x", 1, &small_buffers());
    assert_eq!(report.errors.len(), 1);
    assert!(report.matches.is_empty());
    let invalid = ContentSearchOptions {
        buffer_bytes: 0,
        ..small_buffers()
    };
    assert_eq!(
        search_line_contents_with_options(&arena, "x", 1, &invalid)
            .errors
            .len(),
        1
    );
}

#[test]
fn cancellation_returns_no_unstarted_file_results() {
    let arena = arena_with_files(&[b"needle"]);
    let report = search_line_contents_with_cancel(&arena, "needle", 10, &small_buffers(), &|| true);
    assert!(report.cancelled);
    assert!(report.matches.is_empty());
}
