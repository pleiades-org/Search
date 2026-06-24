use crate::arena::IndexArena;
use crate::search::middle_out_search;
use crate::store::IndexStore;
use crate::util::{expand_root, index_db_path};
use crate::walker::{build_with_jwalk, rebuild_subtree};
use notify::RecommendedWatcher;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct CodeSearchOptions {
    pub watch: bool,
    pub prefer_cache: bool,
}

impl Default for CodeSearchOptions {
    fn default() -> Self {
        Self {
            watch: false,
            prefer_cache: true,
        }
    }
}

pub struct CodeSearch {
    root: PathBuf,
    store: IndexStore,
    arena: RefCell<IndexArena>,
    watcher: Option<RecommendedWatcher>,
    watch_rx: Option<Receiver<notify::Result<notify::Event>>>,
    watch_enabled: bool,
    used_mft: bool,
}

impl CodeSearch {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self::with_options(root, CodeSearchOptions::default())
    }

    pub fn with_options(root: impl AsRef<Path>, options: CodeSearchOptions) -> Self {
        let root = expand_root(root);
        let db_path = index_db_path(&root);
        let store = IndexStore::open(&db_path).expect("failed to open index database");

        let mut me = Self {
            arena: RefCell::new(IndexArena::empty(root.clone())),
            root: root.clone(),
            store,
            watcher: None,
            watch_rx: None,
            watch_enabled: options.watch,
            used_mft: false,
        };

        if options.prefer_cache {
            if let Ok(Some(cached)) = me.store.load() {
                if cached.root == me.root {
                    *me.arena.borrow_mut() = cached;
                    if options.watch {
                        me.enable_watch();
                    }
                    return me;
                }
            }
        }

        me.rebuild();
        if options.watch {
            me.enable_watch();
        }
        me
    }

    pub fn enable_watch(&mut self) {
        if self.watcher.is_some() {
            return;
        }
        self.watch_enabled = true;
        crate::watcher::spawn_watcher(self);
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }

    pub(crate) fn set_watcher(&mut self, watcher: RecommendedWatcher) {
        self.watcher = Some(watcher);
    }

    pub(crate) fn set_watch_receiver(
        &mut self,
        rx: Receiver<notify::Result<notify::Event>>,
    ) {
        self.watch_rx = Some(rx);
    }

    pub fn rebuild(&mut self) {
        let started = Instant::now();

        #[cfg(all(windows, feature = "windows-mft"))]
        if let Ok(arena) = crate::windows_mft::build_with_mft(&self.root) {
            *self.arena.borrow_mut() = arena;
            self.used_mft = true;
            let arena = self.arena.borrow();
            let _ = self.store.save(&arena);
            eprintln!(
                "indexed {} files via Windows MFT in {:?}",
                arena.files.len(),
                started.elapsed()
            );
            return;
        }

        *self.arena.borrow_mut() = build_with_jwalk(&self.root);
        self.used_mft = false;
        let arena = self.arena.borrow();
        let _ = self.store.save(&arena);
        eprintln!(
            "indexed {} files via jwalk in {:?}",
            arena.files.len(),
            started.elapsed()
        );
    }

    fn poll_updates(&self) {
        if !self.watch_enabled {
            return;
        }
        let Some(rx) = self.watch_rx.as_ref() else {
            return;
        };

        while let Ok(Ok(event)) = rx.try_recv() {
            let mut arena = self.arena.borrow_mut();
            for path in event.paths {
                rebuild_subtree(&self.root, &path, &mut arena);
            }
            let _ = self.store.save(&arena);
        }
    }

    pub fn search(&self, term: &str, max_results: usize) -> Vec<String> {
        self.poll_updates();
        middle_out_search(&self.arena.borrow(), term, max_results)
    }

    pub fn search_lines(&self, term: &str, max_results: usize) -> Vec<crate::search::LineMatch> {
        self.poll_updates();
        crate::search::search_line_contents(&self.arena.borrow(), term, max_results)
    }

    pub fn file_count(&self) -> usize {
        self.arena.borrow().files.len()
    }

    pub fn global_midpoint(&self) -> u16 {
        self.arena.borrow().global_midpoint
    }

    pub fn used_mft_path(&self) -> bool {
        self.used_mft
    }
}