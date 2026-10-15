# codesearch

Indexed filename substring search and bounded, literal line-content search.

## Behavior

- Filename matching lowercases both query and filename with Rust's Unicode lowercase conversion. It is not Unicode normalization or full case folding. Full paths are preserved in native platform encoding for filesystem access; printed paths use lossy Unicode display when necessary.
- Filename candidates come from the rarest required trigram posting. A missing gram returns immediately. Sparse candidates use bounded top-k selection; common and short terms scan a precomputed middle-out filename-length order. Matching is verified with a substring check before returning a path.
- Content matching is case-sensitive by default. `--ignore-case` uses Unicode lowercase conversion and preserves original output text. Terms must be single-line literals, with one result per matching source line, including the last unterminated line. CRLF terminators are stripped from returned text.
- Content files are scanned in bounded batches with buffers up to 256 KiB and a default maximum line size of 8 MiB. Results follow catalog order, then source line order. The filesystem walker sorts paths before building the catalog. Binary detection samples the initial buffer for NUL bytes. Content is read live rather than indexed.
- Default startup scans the filesystem so changes since shutdown are reflected. `--cached` explicitly trusts the previous filename snapshot, which may be stale. Cache paths live in the platform user cache directory, with `CODESEARCH_CACHE_DIR` available as an override. An unavailable or locked cache falls back to memory with a diagnostic.
- Watching coalesces notifications and rebuilds a complete, policy-aware snapshot on a background worker. Readers retain immutable `Arc` snapshots. Queries perform no watcher update or cache persistence. This favors correctness over incremental update speed on very large roots.
- Traversal uses `ignore::WalkBuilder`, respects ignore-file policy, includes hidden files that are not ignored, and does not follow symlinks. Searching the home directory also skips common dependency/build directories. Optional Windows MFT enumeration applies the same walker policy before publication; it therefore still incurs a filesystem walk.

## Usage

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

## Library

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

## Migration from the original 0.1.0 implementation

- `CodeSearchOptions::default().prefer_cache` is now false. Explicitly set it to true only when stale filename snapshots are acceptable.
- Content queries preserve case by default. Use the explicit insensitive mode for lowercase matching. Multiline literals are rejected by the report API/CLI.
- Capped filename results can differ: ranking now follows a global middle-out filename-length order instead of directory graph traversal. Capped content results now follow catalog/source order instead of worker scheduling or midpoint line scanning.
- Cache format `CSH4` stores validated native paths and reconstructs derived names, postings and directory IDs. Old working-directory caches are not reused or deleted automatically.
- `FileEntry` includes `native_path: PathBuf`; trigram postings use `Box<[u32]>`. Callers constructing public data structures directly must adapt. Walker/store fallible APIs return `std::io::Result`. This crate is unpublished and its public data structures have changed.

## Build and checks

Rust edition 2024, with the existing dependencies in `Cargo.lock`:

```sh
cargo build --release --locked
cargo test --locked --all-targets
cargo test --locked --all-targets --no-default-features
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
```

Windows optionally uses the `windows-mft` feature, enabled by default. Opening an NTFS volume may require elevated access; failures fall back to the normal walker. Linux and macOS use the walker. CI covers Windows, Linux and macOS with both feature configurations; the local validation reported below was performed on Windows.

## Benchmarks

The resident-query benchmark verifies complete result membership against a substring oracle and prints each query's median and p95 separately. Startup includes fresh enumeration, index construction and persistence; query timings include path-result allocation.

```sh
cargo run --release --locked --bin bench -- /path/to/root
```

The controlled [before/after harness](benchmarks/compare/README.md) uses identical source, fixtures and query definitions against a pinned original revision and the updated implementation. See [measured results and tradeoffs](benchmarks/compare/RESULTS.md). It does not compare unrelated workloads or combine minima across queries.

## License

[Apache 2.0](LICENSE).
