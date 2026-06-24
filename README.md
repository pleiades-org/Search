# codesearch

Ultra-fast file finder with middle-out graph search and line-content support.

## Overview

codesearch builds a persistent, in-memory index of your filesystem to deliver sub-millisecond filename searches and fast content-driven line searches. On Windows, it leverages the NTFS Master File Table (MFT) to index millions of files in seconds. The index is cached to disk via an embedded database (redb), enabling near-instant cold starts.

## Features

- **Filename search** -- middle-out graph traversal over an arena-based directory tree, with trigram posting-list pruning for high-cardinality queries.
- **Line-content search** -- parallel, middle-out line scanning across the file corpus using `memchr`-based pattern matching, with automatic binary-file detection.
- **Windows MFT indexing** -- reads the NTFS MFT directly to enumerate files, bypassing userspace directory traversal (optional, default on Windows).
- **Filesystem watching** -- monitors the search root with `notify` and incrementally updates the index on file changes.
- **Persistent cache** -- serializes the arena index to a redb database for fast startup on subsequent invocations.
- **Comprehensive benchmarks** -- includes a benchmark suite that compares codesearch against ripgrep, fd, PowerShell, and cmd.

## Usage

```
codesearch [OPTIONS] <TERM>
```

### Arguments

| Argument | Description |
| -------- | ----------- |
| `<TERM>` | Search term (or file path when used with `--lines`) |

### Options

| Option | Description |
| ------ | ----------- |
| `-d`, `--dir <DIR>` | Search root directory (default: home directory) |
| `-l`, `--limit <N>` | Maximum results (default: 50) |
| `--rebuild` | Force a full index rebuild |
| `--stats` | Print index statistics |
| `--watch` | Watch filesystem for changes and update index live |
| `-c`, `--content` | Search file contents line-by-line (parallel) |
| `--lines <RANGE>` | Extract a range of lines from a file, e.g. `100-250` |

### Examples

Search for files by name:
```
codesearch Evergreen
codesearch --dir ~/projects "Config"
codesearch --rebuild --stats "lib.rs"
```

Search inside file contents:
```
codesearch -c "fn main"
codesearch -c --limit 10 "TODO"
```

View lines from a file:
```
codesearch --lines 50-100 /path/to/file.rs
```

## Benchmarks

The included `bench` binary runs controlled benchmarks comparing codesearch (warm and cold) against ripgrep, fd, and other tools:

```
cargo run --release --bin bench <root_dir>
```

## Build

Requires Rust edition 2024.

```
cargo build --release
```

## Platform Support

- **Windows**: Full support with optional MFT-based indexing (default, enabled via `windows-mft` feature).
- **Linux / macOS**: Filesystem walking via jwalk; MFT feature is Windows-only and disabled on these platforms.

## License

Apache 2.0
