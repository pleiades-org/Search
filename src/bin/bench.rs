//! Resident-query benchmark; controlled before/after workloads live in benchmarks/compare.
use codesearch::CodeSearch;
use std::{collections::HashSet, hint::black_box, io, path::PathBuf, time::Instant};

const SAMPLES: usize = 101;
const WARMUPS: usize = 8;
const QUERIES: &[&str] = &[
    "a",
    "report",
    "Cargo.toml",
    "main.rs",
    "config",
    "needle_tag_7fd9",
    "zzqxyw_no_such_file",
];

fn main() -> io::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(codesearch::util::default_search_root);
    let started = Instant::now();
    let engine = CodeSearch::try_new(root)?;
    eprintln!(
        "fresh filesystem scan + index build + persistence: {:?}; {} files",
        started.elapsed(),
        engine.file_count()
    );
    for diagnostic in engine.take_diagnostics() {
        eprintln!("{diagnostic}");
    }
    println!("query,samples,matches,p50_us,p95_us");
    for query in QUERIES {
        measure(&engine, query)?;
    }
    eprintln!("Query timings include returned path allocations; OS file caches may be warm.");
    eprintln!("For reproducible before/after results run benchmarks/compare/run.ps1.");
    Ok(())
}

fn measure(engine: &CodeSearch, query: &str) -> io::Result<()> {
    let snapshot = engine.snapshot();
    let normalized = query.to_lowercase();
    let expected: HashSet<_> = snapshot
        .files
        .iter()
        .filter(|file| file.name_lower.contains(&normalized))
        .map(|file| file.path.as_str())
        .collect();
    let complete = engine.search(query, snapshot.files.len());
    let actual: HashSet<_> = complete.iter().map(String::as_str).collect();
    if actual != expected || complete.len() != actual.len() {
        return Err(io::Error::other(format!(
            "incorrect complete results for {query:?}"
        )));
    }
    for _ in 0..WARMUPS {
        black_box(engine.search(query, 50));
    }
    let mut timings = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let matches = black_box(engine.search(black_box(query), 50));
        timings.push(started.elapsed().as_nanos());
        if matches.len() != expected.len().min(50) {
            return Err(io::Error::other(format!(
                "incorrect limited results for {query:?}"
            )));
        }
    }
    timings.sort_unstable();
    println!(
        "{query},{SAMPLES},{},{:.3},{:.3}",
        expected.len().min(50),
        timings[SAMPLES / 2] as f64 / 1000.,
        timings[(SAMPLES * 95).div_ceil(100) - 1] as f64 / 1000.
    );
    Ok(())
}
