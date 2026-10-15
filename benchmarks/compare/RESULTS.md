# Search implementation and before/after results

Measured 6 October 2026 against original Search commit `fb1d7969450357955e23c344912c7856478716db`. The updated working tree is on branch `improve/search-correctness-and-performance`. Changes are applied locally and are not committed or published. The Core repository was inspected as a reference; this implementation does not modify it.

Filename search is substantially faster on these fixtures, and single-large-file content scans improve by about 2.2–3.1× for complete results. Many-file complete content scans are broadly unchanged. Limited many-file searches, startup and whole-process memory have regressions, shown below.

## Applied changes

| Area | Result |
| --- | --- |
| Filename planning | Borrow the rarest posting instead of copying/intersecting lists; a missing required gram proves absence; verify complete substrings; remove the unnecessary whole-catalog fallback. Sparse candidates use bounded top-k selection; common/short terms stop early in a precomputed length order. |
| Index storage | Ordered posting construction deduplicates repeated grams without sorting every list, then compacts postings to boxed slices. Fix repeated visits to bucket zero at midpoint boundaries. |
| Matching | Unicode lowercase filename matching; exact content literals by default; explicit Unicode lowercase insensitive mode; reject multiline content terms. Preserve original line text and native filesystem path identity, including distinct non-Unicode directory names. |
| Content scanning | Compile one exact matcher per query; read bounded buffers; locate matching lines only when needed; handle CRLF, buffer boundaries, repeated hits and final unterminated lines. Merge bounded parallel batches in deterministic catalog/source order. Expose cancellation and structured partial-read diagnostics. |
| Watcher | Coalesce create, modify, rename, remove and error notifications on a background owner; reconstruct complete consistent arenas and publish immutable snapshots. Queries do not mutate IDs or save the cache. CLI `--watch` now remains alive and prints updated results. |
| Cache/startup | Canonicalize and validate roots; fresh scan by default; explicitly trusted snapshot option; native-path `CSH4` cache with checked lengths/root membership/duplicates and reconstructed derived IDs. Use the platform cache directory, exclude it from scans/watch events, and fall back to memory on cache contention/failure. Opening an unchanged trusted snapshot no longer rewrites it. |
| Windows MFT | Handle ordinary and canonical verbatim drive prefixes; pass volume-relative paths to NTFS resolution; reject unsupported roots; skip reparse points/cycles and filter enumeration through walker policy. |
| API/CLI | Add fallible constructors/update methods, bounded diagnostics, content options/reports and validated line ranges. Print contextual errors and return CLI exit status 2 for incomplete reads or invalid requests. |
| Hygiene | Remove unused `jwalk`, add the declared Apache license, update README/migration instructions, replace misleading competitor summaries, and add Windows/Linux/macOS CI checks and regression tests. No new runtime dependencies were introduced. |

Core's rarest-posting planning, bounded result selection, immutable publication and experiment methodology informed these changes. Its application aliases, typo prototypes and file/block/line content indexes were not copied into filesystem literal matching. An optional resident content index, relevance scoring and incremental update optimization remain separate design choices; this patch establishes a correct measured scanning baseline.

## Measurement conditions

- Windows 11 build 26200; AMD Ryzen 7 7800X3D, 8 cores/16 logical processors; `rustc 1.95.0 (59807616e 2026-04-14)`.
- Identical harness source, release defaults and common pinned dependency versions for both builds; default features disabled so raw MFT access is excluded.
- Three alternating before/after run pairs, eight warmups per case, warm OS caches. No affinity or system isolation. The final run followed the validation commands.
- Filename queries use 2,000 or 150,000 deterministic names with limit 50. Content scans use 14,499,769 bytes (13.83 MiB), either in 256 files or one concatenated file. Startup uses the 256-file root with an existing cache.
- The tables report medians of the three run p50s and medians of the three run p95s. They are not pooled percentiles. Sub-microsecond readings are timer limited.
- Before timing, independent oracles verify complete filename membership and content `(path, line, text)` sets. Every timed operation checks its count. Capped result order changed; see the limitations below.

Full methodology and rerun instructions: [README.md](README.md). Complete numeric comparison: [comparison.csv](results/comparison.csv). Compiler/revision/executable hashes: [environment.json](results/environment.json). Updated source hashes: [source-manifest.json](results/source-manifest.json). Raw samples and per-run summaries are retained in [results](results).

## Filename queries and arena builds

All values below are **microseconds**. Filename queries return at most 50 paths and include result allocation/destruction. Arena construction includes path generation, tree/index building and arena destruction, excluding filesystem traversal and persistence.

| Names / query | Matches returned | Before p50 | After p50 | Before p95 | After p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 2,000 / `a` | 50 | 2.9 | 2.0 | 3.0 | 2.1 |
| 2,000 / `report` | 50 | 7.2 | 2.2 | 7.3 | 2.3 |
| 2,000 / rare | 1 | 72.4 | <1 (0.3 observed) | 81.1 | <1 (0.4 observed) |
| 2,000 / absent | 0 | 108.1 | <1 (0.1 observed) | 125.3 | <1 (0.1 observed) |
| 150,000 / `a` | 50 | 9.4 | 2.1 | 11.8 | 2.2 |
| 150,000 / `report` | 50 | 308.8 | 2.2 | 325.2 | 2.4 |
| 150,000 / rare | 1 | 5,738.5 | <1 (0.3 observed) | 6,248.0 | <1 (0.4 observed) |
| 150,000 / absent | 0 | 8,740.6 | <1 (0.1 observed) | 9,283.6 | <1 (0.1 observed) |
| Build 2,000 | 2,000 | 2,265.2 | 2,141.9 | 2,625.5 | 2,229.4 |
| Build 150,000 | 150,000 | 180,971.4 | 153,942.0 | 202,465.9 | 166,877.3 |

The 150,000-name `report` query is about 140× faster on this fixture. Rare and absent queries avoid the previous full scan; their new times are below one microsecond, so the large raw ratios in the CSV should not be interpreted as precise speedup estimates. The large arena build is about 15% faster in this run set; synthetic construction is not a filesystem indexing benchmark.

## Content queries

All values below are **milliseconds**. Rare = `needle_tag_7fd9`, frequent = `return`, absent = `zzqxyw_no_such_content`. All-results cases return 1, 8,191 and 0 matching lines respectively.

| Corpus / query / limit | Before p50 | After p50 | Before p95 | After p95 |
| --- | ---: | ---: | ---: | ---: |
| 256 files / rare / 12 | 2.867 | 3.525 | 3.234 | 4.150 |
| 256 files / rare / all | 2.757 | 2.628 | 3.051 | 2.884 |
| 256 files / frequent / 12 | 0.296 | 0.336 | 0.364 | 0.404 |
| 256 files / frequent / all | 3.390 | 3.406 | 3.954 | 3.740 |
| 256 files / absent / 12 | 2.845 | 2.598 | 3.171 | 2.926 |
| 256 files / absent / all | 2.694 | 2.664 | 3.000 | 2.986 |
| One file / rare / 12 | 4.559 | 1.599 | 5.640 | 1.920 |
| One file / rare / all | 4.540 | 1.617 | 5.145 | 1.925 |
| One file / frequent / 12 | 4.078 | 0.052 | 4.741 | 0.074 |
| One file / frequent / all | 6.030 | 2.766 | 6.569 | 3.319 |
| One file / absent / 12 | 4.653 | 1.550 | 5.659 | 1.909 |
| One file / absent / all | 4.660 | 1.525 | 5.274 | 1.975 |

Single-file complete scans improve by 2.81× for rare, 2.18× for frequent and 3.06× for absent queries. The limited frequent case improves strongly because streaming can stop after enough source lines; the old implementation read/indexed the complete file first. That limited comparison also changes which source lines are selected, so it is not an identical-prefix benchmark.

Many-file complete scans are approximately unchanged: the frequent case's p50 is 0.5% slower, while rare and absent are about 4.7% and 1.1% faster. These small differences should not be treated as robust gains. Many-file limited rare/frequent cases are approximately 23%/13% slower. Bounded batches and deterministic merging incur work/coordination that the old first-available scheduling did not require. The patch does not claim a universal content throughput improvement.

## Startup and memory costs

Startup times are **milliseconds**, including construction and destruction, with warm filesystem/database caches.

| Policy | Before p50 | After p50 | Before p95 | After p95 |
| --- | ---: | ---: | ---: | ---: |
| Explicit fresh scan | 15.527 | 16.205 | 18.647 | 17.533 |
| Explicit trusted cache | 4.771 | 5.366 | 5.318 | 5.834 |
| Each version's default | 4.764 | 16.550 | 5.445 | 18.420 |

Fresh startup is about 4.4% slower; trusted-cache startup is about 12.5% slower because derived names/postings/IDs are reconstructed from validated paths. The default is about 3.47× slower because it now performs a fresh scan instead of silently returning an unchecked old catalog. This last row compares different freshness policies deliberately; it is not an equal-policy regression. Explicit trusted-cache mode remains available when that tradeoff is acceptable.

| Storage/resource measurement | Before | After | Change |
| --- | ---: | ---: | ---: |
| Posting/bucket/order backing storage, 2,000 names | 269,416 B | 231,712 B | 14.0% lower |
| Posting/bucket/order backing storage, 150,000 names | 28,385,384 B (27.07 MiB) | 17,844,900 B (17.02 MiB) | 37.1% lower |
| Median observed whole-benchmark peak working set | 102,690,816 B (97.9 MiB) | 128,180,224 B (122.2 MiB) | 24.8% higher |

The index-storage counter excludes maps, file/path/name strings, directory nodes and allocator overhead. The process peak includes builds, oracles, temporary results and runtime allocation behavior; 10 ms polling makes it a lower-bound observation. Preserving native paths alongside display strings adds catalog storage. These figures demonstrate lower posting storage and higher observed total working set, not lower overall application memory. Retaining older snapshots can add further memory in a live embedding.

## Validation and practical limits

The final source passed on Windows:

```text
cargo test --locked --offline --all-targets                         # 32 tests
cargo test --locked --offline --all-targets --no-default-features   # 30 tests
cargo clippy --locked --offline --all-targets -- -D warnings
cargo clippy --locked --offline --all-targets --no-default-features -- -D warnings
cargo fmt --all -- --check
cargo build --release --locked --offline --bins
```

The original ten focused probes had nine failures and one control success. The implemented versions pass, with additional contracts covering explicitly trusted-cache staleness/rebuild, immutable old snapshots, actual create/modify/rename/directory-delete notifications, persistent CLI watching, cache exclusion, Unicode modes, non-Unicode directory identity, bounded lines/buffers, cancellation, binary/read errors, CRLF, final lines and malformed ranges/caches. Benchmark oracles also passed in all six final measurement processes.

Capped filename ranking changed from graph traversal to a global middle-out length order; capped content ordering changed to catalog/source order. These are documented API changes, not relevance improvements. Native paths are used for I/O, but lossy printed paths can still look identical for different non-Unicode names. Cancellation reports can contain already collected partial matches.

The watcher performs complete root rescans, coalesced over 50–250 ms, rather than surgical incremental indexing. Queries retain the last successfully published snapshot during a failed scan, with diagnostics available. No watch-update scalability benchmark, true cold-storage test, raw NTFS-volume execution benchmark or cross-platform runtime test was performed locally. The new CI workflow is configured for Windows/Linux/macOS but has not been run remotely. MFT policy filtering currently walks the root as well, so the previous claim of bypassing ordinary traversal has been removed.

The cache's stable FNV root fingerprint is independently checked against the stored canonical root before use; it is not a cryptographic identifier. Format migration and public struct changes are described in the [root README](../../README.md). Future optimizations should preserve those correctness contracts and measure startup, total memory and limited-result ordering along with query time.
