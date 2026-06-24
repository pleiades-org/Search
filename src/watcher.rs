use crate::engine::CodeSearch;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;
use std::time::Duration;

pub fn spawn_watcher(search: &mut CodeSearch) {
    let root = search.root_path().to_path_buf();
    let (tx, rx) = mpsc::channel();

    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        Config::default().with_poll_interval(Duration::from_millis(500)),
    )
    .expect("failed to create filesystem watcher");

    watcher
        .watch(&root, RecursiveMode::Recursive)
        .expect("failed to watch root directory");

    search.set_watcher(watcher);
    search.set_watch_receiver(rx);
}