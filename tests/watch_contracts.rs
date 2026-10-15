use codesearch::{CodeSearch, CodeSearchOptions};
use std::{
    fs,
    time::{Duration, Instant},
};

fn wait_for(engine: &CodeSearch, term: &str, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if engine.search(term, 100).len() == count {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "watch did not reconcile {term:?}: {:?}",
        engine.take_diagnostics()
    );
}

fn wait_for_generation(engine: &CodeSearch, previous: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if engine.generation() > previous {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "watch did not publish after modification: {:?}",
        engine.take_diagnostics()
    );
}

#[test]
fn background_watch_reconciles_create_modify_rename_and_directory_delete() {
    let root = std::env::temp_dir()
        .join("codesearch-tests")
        .join(format!("watch-{}", std::process::id()));
    fs::create_dir_all(root.join("nested")).unwrap();
    let original = root.join("nested/original.txt");
    fs::write(&original, "before").unwrap();
    let engine = CodeSearch::try_with_options(
        &root,
        CodeSearchOptions {
            watch: true,
            prefer_cache: false,
        },
    )
    .unwrap();
    let old_snapshot = engine.snapshot();
    let created = root.join("created.txt");
    fs::write(&created, "new").unwrap();
    wait_for(&engine, "created", 1);
    assert_eq!(
        old_snapshot.files.len(),
        1,
        "a published snapshot must stay immutable"
    );
    let previous_generation = engine.generation();
    fs::write(&original, "after").unwrap();
    wait_for_generation(&engine, previous_generation);
    wait_for(&engine, "original", 1);
    assert_eq!(engine.search_lines("after", 100).len(), 1);
    let renamed = root.join("renamed.txt");
    fs::rename(&created, &renamed).unwrap();
    wait_for(&engine, "renamed", 1);
    wait_for(&engine, "created", 0);
    // Remove only the explicitly created fixture file/directory, never an inferred root.
    fs::remove_file(original).unwrap();
    fs::remove_dir(root.join("nested")).unwrap();
    wait_for(&engine, "original", 0);
    assert_eq!(engine.search("renamed", 100).len(), 1);
}
