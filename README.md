# Pleiades Search

A Rust library and command-line tool for indexed filename search and bounded literal content search.

The `codesearch` engine builds an in-memory filename catalogue, uses trigram postings to narrow substring candidates, and reads matching content directly from disk. It supports persistent filename snapshots, filesystem watching, explicit case handling, and line-range extraction.

## Quick start

Use a Rust toolchain supporting edition 2024. Dependencies are pinned in `Cargo.lock`.

```sh
git clone https://github.com/pleiades-org/Search.git
cd Search
cargo build --release --locked --bin codesearch
./target/release/codesearch --dir . Cargo.toml
```

On Windows, the executable is `target\release\codesearch.exe`. To install the CLI on your Cargo binary path:

```sh
cargo install --path . --locked --bin codesearch
```

## Command-line usage

```text
codesearch [OPTIONS] <TERM>
```

| Option | Description |
| --- | --- |
| `-d`, `--dir <DIR>` | Root directory, default `~` |
| `-l`, `--limit <N>` | Maximum results, default 50 |
| `--cached` | Explicitly trust the last saved filename catalog |
| `--rebuild` | Refresh the catalog; overrides `--cached` |
| `--stats` | Print index and backend statistics |
| `--watch` | Stay running and print results after index updates; Ctrl+C stops |
| `-c`, `--content` | Search literal content lines |
| `-i`, `--ignore-case` | Unicode lowercase content matching; requires `--content` |
| `--lines <RANGE>` | Extract a positive inclusive line range, e.g. `50-100` |

```sh
codesearch --dir ~/projects Config
codesearch --dir ~/projects -c 'fn main'
codesearch --dir ~/projects -c -i TODO
codesearch --dir ~/projects --watch lib.rs
codesearch --cached --stats Cargo.toml
codesearch --lines 50-100 /path/to/file.rs
```

Invalid roots, malformed ranges and incomplete content reads exit with status 2. Content errors are printed with the affected path; successfully collected results can precede that error. Empty filename terms and zero limits return no results.

## Search behavior

### Matching and ordering

Filename matching lowercases the query and filename using Rust's Unicode lowercase conversion. It does not perform Unicode normalization or full case folding. Native paths are preserved for filesystem access; displayed paths use lossy Unicode conversion when necessary.

Candidates come from the rarest required trigram posting, followed by a substring check. Short or common terms scan a precomputed middle-out filename-length order; sparse candidates use bounded top-k selection. A missing required trigram returns no candidates.

Content terms are **single-line literals**, case-sensitive by default. `--ignore-case` applies Unicode lowercase conversion while preserving the original output text. There is one result per matching source line, including a final unterminated line; CRLF terminators are removed from returned text. Results follow catalogue order, then source-line order. Content search does not support regular expressions.

### Filesystem and memory bounds

Traversal uses `ignore::WalkBuilder`, respects ignore-file policy, includes non-ignored hidden files, and does not follow symlinks. Home-directory searches also skip common dependency and build directories. Paths are sorted before the catalogue is built.

Content is scanned live in bounded batches, with buffers up to 256 KiB and a default maximum line size of 8 MiB. Binary detection samples the initial buffer for NUL bytes. An incomplete read is reported with its path; callers must check diagnostics when accepting partial results.

### Cache and watching

Startup scans the filesystem by default to reconcile changes made while the application was stopped. `--cached` explicitly trusts a previous filename snapshot and can return stale paths. Cache files use the platform user cache directory, overridable with `CODESEARCH_CACHE_DIR`. An unavailable or locked cache falls back to memory with a diagnostic.

Watch mode coalesces notifications and builds a complete policy-aware snapshot on a background worker. Readers retain immutable `Arc` snapshots. Queries do not process watcher updates or persist the cache. Full rebuilds prioritize consistent results but can be expensive on very large roots.

## Rust library

```rust
use codesearch::CodeSearch;
use codesearch::content::{CaseMode, ContentSearchOptions};

fn main() -> std::io::Result<()> {
    let search = CodeSearch::try_new(".")?;
    let paths = search.search("main", 50);
    println!("{} matching paths", paths.len());

    let options = ContentSearchOptions {
        case_mode: CaseMode::Insensitive,
        ..ContentSearchOptions::default()
    };
    let report = search.search_lines_with_options("TODO", 20, &options);
    for matched in report.matches {
        println!("{}:{}: {}", matched.path, matched.line_number, matched.content);
    }
    for error in report.errors {
        eprintln!("content read {:?}: {}", error.path, error.message);
    }
    Ok(())
}
```

Use `try_new`, `try_with_options`, `try_rebuild` and `try_enable_watch` when initialization/update errors must be handled. Compatibility constructors still panic on invalid roots. `search_lines_with_options` returns matches and read diagnostics; the simpler `search_lines` records diagnostics available through `take_diagnostics`. The standalone `search_line_contents_with_cancel` accepts a cancellation callback and marks the report as cancelled; already collected matches can remain in that partial report.

## Build and checks

```sh
cargo build --release --locked
cargo test --locked --all-targets
cargo test --locked --all-targets --no-default-features
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
```

Windows enables the optional `windows-mft` feature by default. Opening an NTFS volume may require elevated access; failures fall back to normal traversal. The MFT backend still applies walker policy and therefore also incurs a filesystem walk. Linux and macOS use the walker.

The [CI workflow](.github/workflows/ci.yml) runs checks on Windows, Linux, and macOS with default features enabled and disabled. This repository is unpublished, and its public data structures may change.

## Benchmarks

The resident-query benchmark verifies complete result membership against a substring oracle and prints each query's median and p95 separately. Startup includes fresh enumeration, index construction and persistence; query timings include path-result allocation.

```sh
cargo run --release --locked --bin bench -- /path/to/root
```

The controlled [before/after harness](benchmarks/compare/README.md) uses identical source, fixtures and query definitions against a pinned original revision and the updated implementation. See [measured results and tradeoffs](benchmarks/compare/RESULTS.md). It does not compare unrelated workloads or combine minima across queries.

## Compatibility notes

Changes from the original 0.1.0 implementation:

- `CodeSearchOptions::default().prefer_cache` is now false. Explicitly set it to true only when stale filename snapshots are acceptable.
- Content queries preserve case by default. Use the explicit insensitive mode for lowercase matching. Multiline literals are rejected by the report API/CLI.
- Capped filename results can differ: ranking now follows a global middle-out filename-length order instead of directory graph traversal. Capped content results now follow catalog/source order instead of worker scheduling or midpoint line scanning.
- Cache format `CSH4` stores validated native paths and reconstructs derived names, postings and directory IDs. Old working-directory caches are not reused or deleted automatically.
- `FileEntry` includes `native_path: PathBuf`; trigram postings use `Box<[u32]>`. Callers constructing public data structures directly must adapt. Walker/store fallible APIs return `std::io::Result`. This crate is unpublished and its public data structures have changed.

## License

[Apache License 2.0](LICENSE).
