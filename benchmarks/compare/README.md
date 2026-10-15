# Controlled before/after benchmark

The same Rust harness is built against original Search commit `fb1d7969450357955e23c344912c7856478716db` and the current working tree. The baseline is extracted from Git, so source edits do not alter its implementation. Both builds use release defaults, the same pinned dependency versions and `default-features = false`, excluding raw NTFS access from the comparison.

From this directory, with PowerShell 7, Git, tar and Rust available:

```powershell
./run.ps1 -Runs 3
```

Builds run offline. On a machine without the required Cargo dependencies cached, first build the current harness and the extracted baseline harness without `--offline`, then repeat this command. The script writes build artifacts under `artifacts/`, generated fixtures under `fixtures/`, and evidence under `results/`. The first two are Git-ignored. Existing result files with the same labels are overwritten on another run; archive them first if needed. This script measures constructor/cache behavior and writes only to these fixtures and the application's normal user cache locations.

## Workloads

| Case | Definition | Samples per run |
| --- | --- | ---: |
| Filename | 2,000 and 150,000 synthetic paths across 64 ordinary directories and one rare-file directory; queries `a`, `report`, `needle_tag_7fd9`, `zzqxyw_no_such_file`; limit 50 | 101 |
| Arena build | Construct paths, directory arena and index, then drop the completed arena; no filesystem traversal or persistence | 11 |
| Content | 256 files × 512 lines and one file containing exactly the concatenated bytes; rare, frequent and absent literals; limits 12 and all results | 21 |
| Startup | Open/drop a search engine over the 256-file root; explicit fresh scan, explicit trusted cache, and each version's default policy | 21 |

Each case receives eight untimed warmups. Content contains one rare match, 8,191 `return` lines and zero absent-query matches. The corpus is plain ASCII text so both versions' original matching semantics can be compared. Separate regression tests cover the uppercase/Unicode fixes; their original wrong answers are not counted as performance wins.

## Correctness and statistics

Before timing, filename membership is checked against a direct substring scan, including full-result equality, limits and duplicates. Content full `(path, line_number, text)` sets are checked against an independent `BufReader::lines` oracle. Each timed operation also checks the expected count. Query timings include allocation and destruction of returned paths/line matches; oracle preparation and output logging are excluded.

Three run pairs alternate before/after, after/before, before/after. Each run sorts its samples to compute p50 and nearest-rank p95. `comparison.csv` reports the median of run p50s and the median of run p95s; the latter is not a pooled percentile. Raw nanosecond samples are retained. Sub-microsecond results are marked `timer_limited`; precise multi-thousand-fold speedup claims would overstate timing resolution.

The OS caches are warm. No CPU affinity, system isolation, confidence intervals, or true cold-disk measurements are provided. This is a local synthetic comparison, not a guarantee for arbitrary filesystems or machines. Background activity can affect tails.

## Ordering and memory limitations

Full-result membership is comparable across versions. Capped results have a changed contract: filename ranking is now global rather than directory graph based, and content results are deterministic catalog/source order rather than midpoint/worker order. The `first12` rows therefore measure the cost of each version's policy, not retrieval of an identical selected prefix.

`*-index.csv` measures backing storage for trigram ID postings, length buckets and visit order only. It excludes map overhead, path/name strings, directory nodes and the rest of the arena. `*-resources.csv` records the largest process `PeakWorkingSet64` observed during 10 ms polling over the whole benchmark sequence, including correctness oracles and builds. This is a lower-bound observation of process working set, not allocator live bytes or steady-state index memory. Preserving native paths adds storage, and callers retaining old `Arc` snapshots retain old arenas. Reduced posting storage does not imply reduced total process memory.

## Evidence

- `comparison.csv`: all before/after cases and aggregates.
- `before-N-samples.csv`, `after-N-samples.csv`: raw measurements.
- `*-summary.csv`, `*-index.csv`, `*-resources.csv`: per-run summaries/storage/resource observations.
- `*-stderr.txt`: constructor logs or diagnostics; benchmark failure aborts the script.
- `environment.json`: compiler/platform, revision, executable hashes and caveats.
- `source-manifest.json`: SHA-256 hashes of updated Rust implementation, tests and harness sources.
- [RESULTS.md](RESULTS.md): interpretation, regressions and validation.
