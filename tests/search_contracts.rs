//! Regression contracts for filename, content, updates and cache lifecycle.
use codesearch::{
    CodeSearch, CodeSearchOptions,
    arena::IndexArena,
    index::CandidateSet,
    search::{middle_out_search, search_line_contents},
    walker::rebuild_subtree,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

fn fixture_directory(label: &str) -> PathBuf {
    let ordinal = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir()
        .join("codesearch-tests")
        .join(format!("{label}-{}-{ordinal}", std::process::id()));
    fs::create_dir_all(&directory).expect("create isolated analysis fixture");
    directory
}

#[test]
fn content_search_preserves_an_exact_uppercase_literal() {
    let root = fixture_directory("uppercase");
    let file = root.join("notes.txt");
    fs::write(&file, "NEEDLE\nneedle\n").unwrap();
    let arena = IndexArena::from_paths(root, vec![file]);
    let matches = search_line_contents(&arena, "NEEDLE", 10);
    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0].line_number, 1,
        "uppercase query selected lowercase line"
    );
}

#[test]
fn filename_search_lowercases_unicode_consistently() {
    let root = fixture_directory("unicode");
    let arena = IndexArena::from_paths(root.clone(), vec![root.join("CAFÉ.txt")]);
    assert_eq!(middle_out_search(&arena, "café", 10).len(), 1);
}

#[test]
fn a_created_file_event_adds_the_new_file() {
    let root = fixture_directory("created");
    let mut arena = IndexArena::empty(root.clone());
    let created_file = root.join("created.txt");
    fs::write(&created_file, "new file").unwrap();
    rebuild_subtree(&root, &created_file, &mut arena);
    assert_eq!(middle_out_search(&arena, "created", 10).len(), 1);
}

#[test]
fn a_modified_file_event_keeps_the_existing_file() {
    let root = fixture_directory("modified");
    let file = root.join("retained.txt");
    fs::write(&file, "before").unwrap();
    let mut arena = IndexArena::from_paths(root.clone(), vec![file.clone()]);
    fs::write(&file, "after").unwrap();
    rebuild_subtree(&root, &file, &mut arena);
    assert_eq!(middle_out_search(&arena, "retained", 10).len(), 1);
}

#[test]
fn deleting_a_directory_remaps_surviving_directory_file_ids() {
    let root = fixture_directory("deleted");
    let mut arena = IndexArena::from_paths(
        root.clone(),
        vec![root.join("removed/a.txt"), root.join("retained/b.txt")],
    );
    rebuild_subtree(&root, &root.join("removed"), &mut arena);
    assert_eq!(arena.files.len(), 1);
    let retained_directory = arena
        .dirs
        .iter()
        .find(|directory| directory.name == "retained")
        .unwrap();
    assert_eq!(retained_directory.file_ids, vec![0]);
}

#[test]
fn reopening_a_cache_reconciles_files_created_while_not_watching() {
    let root = fixture_directory("cache");
    fs::write(root.join("old.txt"), "old").unwrap();
    let initial_engine = CodeSearch::with_options(
        &root,
        CodeSearchOptions {
            watch: false,
            prefer_cache: false,
        },
    );
    assert_eq!(initial_engine.file_count(), 1);
    drop(initial_engine);
    fs::write(root.join("new.txt"), "new").unwrap();
    let cached_engine = CodeSearch::new(&root);
    assert_eq!(cached_engine.search("new", 10).len(), 1);
}

#[test]
fn explicitly_trusted_cache_stays_stale_until_rebuilt() {
    let root = fixture_directory("trusted-cache");
    fs::write(root.join("old.txt"), "old").unwrap();
    drop(CodeSearch::try_new(&root).unwrap());
    fs::write(root.join("new.txt"), "new").unwrap();
    let mut engine = CodeSearch::try_with_options(
        &root,
        CodeSearchOptions {
            watch: false,
            prefer_cache: true,
        },
    )
    .unwrap();
    assert!(engine.loaded_cache());
    assert!(engine.search("new", 10).is_empty());
    engine.try_rebuild().unwrap();
    assert!(!engine.loaded_cache());
    assert_eq!(engine.search("new", 10).len(), 1);
}

#[test]
fn a_missing_required_filename_gram_proves_no_candidates() {
    let root = fixture_directory("missing-gram");
    let arena = IndexArena::from_paths(root.clone(), vec![root.join("sample.txt")]);
    match arena.search_index().candidates_for("zzq") {
        CandidateSet::Filtered(candidates) => assert!(candidates.is_empty()),
        CandidateSet::MiddleOut(_) => {
            panic!("missing gram unnecessarily requests a full traversal")
        }
    }
}

#[test]
fn a_single_line_search_does_not_return_a_match_spanning_two_lines() {
    let root = fixture_directory("line-boundary");
    let file = root.join("notes.txt");
    fs::write(&file, "left\nright\n").unwrap();
    let arena = IndexArena::from_paths(root, vec![file]);
    assert!(search_line_contents(&arena, "left\nright", 10).is_empty());
}

#[test]
fn a_second_search_engine_can_open_an_existing_root_without_panicking() {
    let root = fixture_directory("concurrent-open");
    fs::write(root.join("sample.txt"), "sample").unwrap();
    let first_engine = CodeSearch::new(&root);
    let second_engine = std::panic::catch_unwind(|| CodeSearch::new(&root));
    assert!(
        second_engine.is_ok(),
        "a second opener panicked on the cache lock"
    );
    drop(first_engine);
}

#[test]
fn lowercase_ascii_search_and_zero_limits_work_as_control_cases() {
    let root = fixture_directory("controls");
    let file = root.join("sample.txt");
    fs::write(&file, "first\nneedle\nlast").unwrap();
    let arena = IndexArena::from_paths(root, vec![file]);
    assert_eq!(middle_out_search(&arena, "sample", 10).len(), 1);
    assert_eq!(search_line_contents(&arena, "needle", 10)[0].line_number, 2);
    assert!(middle_out_search(&arena, "sample", 0).is_empty());
    assert!(search_line_contents(&arena, "needle", 0).is_empty());
}
