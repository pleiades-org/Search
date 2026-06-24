pub mod arena;
pub mod engine;
pub mod index;
pub mod search;
pub mod store;
pub mod util;
pub mod walker;
pub mod watcher;

#[cfg(all(windows, feature = "windows-mft"))]
pub mod windows_mft;

use std::path::Path;

pub use engine::{CodeSearch, CodeSearchOptions};

pub fn create(root: impl AsRef<Path>) -> CodeSearch {
    CodeSearch::new(root)
}

/// Open a searcher over the user's home directory (broad search).
pub fn create_default() -> CodeSearch {
    CodeSearch::new(util::default_search_root())
}