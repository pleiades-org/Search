use crate::{
    arena::IndexArena,
    content::{ContentSearchOptions, ContentSearchReport},
    search::middle_out_search,
    store::IndexStore,
    util::{expand_root, index_db_path},
    walker::build_with_walk,
    watcher::WatchWorker,
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

const MAX_DIAGNOSTICS: usize = 16;

#[derive(Debug, Clone, Default)]
pub struct CodeSearchOptions {
    pub watch: bool,
    /// Explicitly trust an existing snapshot. Default false reconciles changes since shutdown.
    pub prefer_cache: bool,
}

pub(crate) struct SharedIndex {
    pub snapshot: RwLock<Arc<IndexArena>>,
    pub generation: AtomicU64,
    pub used_mft: AtomicBool,
    store: Mutex<Option<IndexStore>>,
    diagnostics: Mutex<Vec<String>>,
}

impl SharedIndex {
    pub(crate) fn publish(&self, arena: IndexArena, used_mft: bool) {
        // Serialize publishers so the persisted snapshot cannot lag a later publication.
        let store = self.store.lock().expect("cache lock");
        let snapshot = Arc::new(arena);
        {
            let mut current = self.snapshot.write().expect("snapshot lock");
            *current = snapshot.clone();
            self.used_mft.store(used_mft, Ordering::Relaxed);
            self.generation.fetch_add(1, Ordering::Release);
        }
        if let Some(diagnostic) = save_cache(store.as_ref(), &snapshot) {
            self.diagnose(diagnostic);
        }
    }

    pub(crate) fn diagnose(&self, message: String) {
        let mut diagnostics = self.diagnostics.lock().expect("diagnostic lock");
        // Keep a bounded latest diagnostic even if a watcher repeatedly fails.
        if diagnostics.len() == MAX_DIAGNOSTICS {
            diagnostics.remove(0);
        }
        diagnostics.push(message);
    }
}

pub struct CodeSearch {
    root: PathBuf,
    shared: Arc<SharedIndex>,
    watcher: Option<WatchWorker>,
    loaded_cache: bool,
}

impl CodeSearch {
    /// Compatibility constructor. Prefer try_new to handle inaccessible roots without panicking.
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self::with_options(root, CodeSearchOptions::default())
    }

    pub fn try_new(root: impl AsRef<Path>) -> io::Result<Self> {
        Self::try_with_options(root, CodeSearchOptions::default())
    }

    pub fn with_options(root: impl AsRef<Path>, options: CodeSearchOptions) -> Self {
        Self::try_with_options(root, options).expect("failed to initialize search root")
    }

    pub fn try_with_options(
        root: impl AsRef<Path>,
        options: CodeSearchOptions,
    ) -> io::Result<Self> {
        let root = std::fs::canonicalize(expand_root(root))?;
        if !root.metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "search root must be a directory",
            ));
        }
        let (store, mut diagnostics) = open_cache(&root);
        let cached = load_cache(
            store.as_ref(),
            &root,
            options.prefer_cache,
            &mut diagnostics,
        );
        let loaded_cache = cached.is_some();
        let (arena, used_mft) = match cached {
            Some(arena) => (arena, false),
            None => build_index(&root)?,
        };
        if !loaded_cache && let Some(diagnostic) = save_cache(store.as_ref(), &arena) {
            diagnostics.push(diagnostic);
        }
        let shared = Arc::new(SharedIndex {
            snapshot: RwLock::new(Arc::new(arena)),
            generation: AtomicU64::new(1),
            used_mft: AtomicBool::new(used_mft),
            store: Mutex::new(store),
            diagnostics: Mutex::new(diagnostics),
        });
        let mut search = Self {
            root,
            shared,
            watcher: None,
            loaded_cache,
        };
        if options.watch {
            search.try_enable_watch()?;
        }
        Ok(search)
    }

    pub fn try_enable_watch(&mut self) -> io::Result<()> {
        if self.watcher.is_none() {
            self.watcher = Some(crate::watcher::spawn_watcher(
                &self.root,
                self.shared.clone(),
            )?);
            // Close the initial scan-to-watch registration gap before returning.
            self.try_rebuild()?;
        }
        Ok(())
    }

    pub fn enable_watch(&mut self) {
        if let Err(error) = self.try_enable_watch() {
            self.shared
                .diagnose(format!("failed to start watcher: {error}"));
        }
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }

    pub fn try_rebuild(&mut self) -> io::Result<()> {
        let (arena, used_mft) = build_index(&self.root)?;
        self.shared.publish(arena, used_mft);
        self.loaded_cache = false;
        Ok(())
    }

    pub fn rebuild(&mut self) {
        if let Err(error) = self.try_rebuild() {
            self.shared.diagnose(format!(
                "failed to rebuild {}: {error}",
                self.root.display()
            ));
        }
    }

    pub fn snapshot(&self) -> Arc<IndexArena> {
        self.shared.snapshot.read().expect("snapshot lock").clone()
    }
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }
    pub fn loaded_cache(&self) -> bool {
        self.loaded_cache
    }
    pub fn take_diagnostics(&self) -> Vec<String> {
        std::mem::take(&mut *self.shared.diagnostics.lock().expect("diagnostic lock"))
    }

    pub fn search(&self, term: &str, max_results: usize) -> Vec<String> {
        middle_out_search(&self.snapshot(), term, max_results)
    }

    pub fn search_lines(&self, term: &str, max_results: usize) -> Vec<crate::search::LineMatch> {
        let report =
            self.search_lines_with_options(term, max_results, &ContentSearchOptions::default());
        for error in report.errors {
            self.shared.diagnose(format!(
                "content search {:?}: {}",
                error.path, error.message
            ));
        }
        report.matches
    }

    pub fn search_lines_with_options(
        &self,
        term: &str,
        max_results: usize,
        options: &ContentSearchOptions,
    ) -> ContentSearchReport {
        crate::content::search_line_contents_with_options(
            &self.snapshot(),
            term,
            max_results,
            options,
        )
    }

    pub fn file_count(&self) -> usize {
        self.snapshot().files.len()
    }
    pub fn global_midpoint(&self) -> u16 {
        self.snapshot().global_midpoint
    }
    pub fn used_mft_path(&self) -> bool {
        self.shared.used_mft.load(Ordering::Relaxed)
    }
}

fn open_cache(root: &Path) -> (Option<IndexStore>, Vec<String>) {
    let path = index_db_path(root);
    let result = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid cache directory"))
        .and_then(std::fs::create_dir_all)
        .and_then(|()| IndexStore::open(&path));
    match result {
        Ok(store) => (Some(store), Vec::new()),
        Err(error) => (
            None,
            vec![format!(
                "cache unavailable at {}; using memory: {error}",
                path.display()
            )],
        ),
    }
}

fn save_cache(store: Option<&IndexStore>, arena: &IndexArena) -> Option<String> {
    store?
        .save(arena)
        .err()
        .map(|error| format!("failed to save index cache: {error}"))
}

fn load_cache(
    store: Option<&IndexStore>,
    root: &Path,
    prefer_cache: bool,
    diagnostics: &mut Vec<String>,
) -> Option<IndexArena> {
    if !prefer_cache {
        return None;
    }
    match store?.load() {
        Ok(Some(arena)) if arena.root == root => Some(arena),
        Ok(_) => None,
        Err(error) => {
            diagnostics.push(format!("cache load failed; rebuilding: {error}"));
            None
        }
    }
}

fn build_index(root: &Path) -> io::Result<(IndexArena, bool)> {
    #[cfg(all(windows, feature = "windows-mft"))]
    if let Ok(arena) = crate::windows_mft::build_with_mft(root) {
        return Ok((arena, true));
    }
    build_with_walk(root).map(|arena| (arena, false))
}
