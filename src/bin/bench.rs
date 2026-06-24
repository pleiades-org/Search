use codesearch::CodeSearch;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const WARMUP: usize = 3;
const FAST_ITERS: usize = 50;
const COLD_ITERS: usize = 10;
const SLOW_ITERS: usize = 3;

struct BenchResult {
    name: String,
    median_ms: f64,
    p95_ms: f64,
    results: usize,
    notes: String,
}

fn main() {
    let root = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        codesearch::util::default_search_root()
    });

    let queries = [
        "Evergreen",
        "mod.rs",
        "Cargo.toml",
        "main.rs",
        "config",
        "lib.rs",
        "codesearch",
        "Benchmark",
    ];
    let limit = 50usize;

    println!("=== CodeSearch Benchmark ===");
    println!("root: {}", root.display());
    println!("queries: {queries:?}");
    println!("iterations: {FAST_ITERS} indexed / {COLD_ITERS} rg+fd / {SLOW_ITERS} slow baselines\n");

    let index_start = Instant::now();
    let mut engine = CodeSearch::new(&root);
    let index_elapsed = index_start.elapsed();
    let file_count = engine.file_count();

    println!("--- Index (cold start via CodeSearch::new) ---");
    println!(
        "files: {file_count}, time: {:.1}ms, mft: {}\n",
        index_elapsed.as_secs_f64() * 1000.0,
        engine.used_mft_path()
    );

    let mut filename_warm_results = Vec::new();
    let mut filename_cold_results = Vec::new();

    println!("=== CodeSearch Filename Searches (Warm) & Competitors ===");
    for query in queries {
        println!("--- query: \"{query}\" ---");
        let cs_warm = bench_codesearch_warm(&mut engine, query, limit);
        print_result(&cs_warm);
        filename_warm_results.push(cs_warm);

        let mut rg_cmd = rg_command(query, &root);
        if let Some(rg) = bench_command("ripgrep --files", &mut rg_cmd, limit, COLD_ITERS) {
            print_result(&rg);
        }

        let mut fd_cmd = fd_command(query, &root, limit);
        if let Some(fd) = bench_command("fd", &mut fd_cmd, limit, COLD_ITERS) {
            print_result(&fd);
        }
        println!();
    }

    // Drop the engine to release the redb database file lock so cold searches can open it
    drop(engine);

    println!("\n=== Filename Searches (Cold & Competitors) ===");
    for query in queries {
        println!("--- query: \"{query}\" ---");

        let cs_cold = bench_codesearch_cold(&root, query, limit);
        print_result(&cs_cold);
        filename_cold_results.push(cs_cold);

        let mut rg_cmd = rg_command(query, &root);
        if let Some(rg) = bench_command("ripgrep --files", &mut rg_cmd, limit, COLD_ITERS) {
            print_result(&rg);
            filename_cold_results.push(rg);
        }

        let mut fd_cmd = fd_command(query, &root, limit);
        if let Some(fd) = bench_command("fd", &mut fd_cmd, limit, COLD_ITERS) {
            print_result(&fd);
            filename_cold_results.push(fd);
        }
        println!();
    }

    let mut filename_all_results = Vec::new();
    filename_all_results.extend(filename_warm_results);
    filename_all_results.extend(filename_cold_results);

    print_summary(&filename_all_results, file_count, index_elapsed);

    // =========================================================================
    // Line Content Searches (Benched Separately)
    // =========================================================================

    // Re-create engine for warm line searches
    let mut engine = CodeSearch::new(&root);
    let mut line_warm_results = Vec::new();
    let mut line_cold_results = Vec::new();

    println!("\n=== CodeSearch Line Content Searches (Warm) & Competitor ===");
    for query in queries {
        println!("--- query: \"{query}\" ---");
        let cs_line_warm = bench_codesearch_lines_warm(&mut engine, query, limit);
        print_result(&cs_line_warm);
        line_warm_results.push(cs_line_warm);

        let mut rg_content = rg_content_command(query, &root);
        if let Some(rg) = bench_command("ripgrep (content)", &mut rg_content, limit, COLD_ITERS) {
            print_result(&rg);
        }
        println!();
    }

    // Drop the engine to release the database lock
    drop(engine);

    println!("\n=== Line Content Searches (Cold & Competitors) ===");
    for query in queries {
        println!("--- query: \"{query}\" ---");
        let cs_line_cold = bench_codesearch_lines_cold(&root, query, limit);
        print_result(&cs_line_cold);
        line_cold_results.push(cs_line_cold);

        let mut rg_content = rg_content_command(query, &root);
        if let Some(rg) = bench_command("ripgrep (content)", &mut rg_content, limit, COLD_ITERS) {
            print_result(&rg);
            line_cold_results.push(rg);
        }
        println!();
    }
}

fn bench_codesearch_warm(engine: &mut CodeSearch, query: &str, limit: usize) -> BenchResult {
    let mut samples = Vec::with_capacity(WARMUP + FAST_ITERS);
    for i in 0..(WARMUP + FAST_ITERS) {
        let start = Instant::now();
        let results = engine.search(query, limit);
        let elapsed = start.elapsed();
        if i >= WARMUP {
            samples.push((elapsed, results.len()));
        }
    }
    summarize("codesearch (warm)", &samples, "warm in-memory index search")
}

fn bench_codesearch_cold(root: &Path, query: &str, limit: usize) -> BenchResult {
    let mut samples = Vec::with_capacity(WARMUP + COLD_ITERS);
    for i in 0..(WARMUP + COLD_ITERS) {
        let start = Instant::now();
        let engine = CodeSearch::with_options(
            root,
            codesearch::CodeSearchOptions {
                watch: false,
                prefer_cache: true,
            },
        );
        let results = engine.search(query, limit);
        let elapsed = start.elapsed();
        if i >= WARMUP {
            samples.push((elapsed, results.len()));
        }
    }
    summarize("codesearch (cold)", &samples, "cold start: reload cache from redb + search")
}

fn bench_command(
    name: &str,
    cmd: &mut Command,
    limit: usize,
    iters: usize,
) -> Option<BenchResult> {
    if cmd.get_program().is_empty() {
        return None;
    }
    if Command::new(cmd.get_program())
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return None;
    }

    let mut samples = Vec::with_capacity(WARMUP + iters);
    for i in 0..(WARMUP + iters) {
        let start = Instant::now();
        let count = run_limited(cmd, limit)?;
        let elapsed = start.elapsed();
        if i >= WARMUP {
            samples.push((elapsed, count));
        }
    }
    Some(summarize(
        name,
        &samples,
        &format!("cold scan, stop after {limit} hits"),
    ))
}

fn run_limited(cmd: &mut Command, limit: usize) -> Option<usize> {
    use std::io::{BufRead, BufReader};
    let mut child = cmd.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let reader = BufReader::new(stdout);
    let mut count = 0usize;
    for line in reader.lines() {
        let line = line.ok()?;
        if !line.is_empty() {
            count += 1;
            if count >= limit {
                let _ = child.kill();
                break;
            }
        }
    }
    let _ = child.wait();
    Some(count)
}

fn rg_command(query: &str, root: &Path) -> Command {
    let mut cmd = Command::new(tool_path("rg"));
    cmd.arg("--files")
        .arg("--glob")
        .arg(format!("*{query}*"))
        .arg("--no-ignore-vcs")
        .arg(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}

fn fd_command(query: &str, root: &Path, limit: usize) -> Command {
    let mut cmd = Command::new(tool_path("fd"));
    cmd.arg(query)
        .arg("--max-results")
        .arg(limit.to_string())
        .arg("--no-ignore-vcs")
        .arg(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}

#[allow(dead_code)]
fn bench_powershell(query: &str, root: &Path, limit: usize) -> Option<BenchResult> {
    let root = root.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$null = Get-ChildItem -LiteralPath '{root}' -Recurse -File -ErrorAction SilentlyContinue | Where-Object {{ $_.Name -like '*{query}*' }} | Select-Object -First {limit}"
    );

    let mut samples = Vec::with_capacity(WARMUP + SLOW_ITERS);
    for i in 0..(WARMUP + SLOW_ITERS) {
        let start = Instant::now();
        let status = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        let elapsed = start.elapsed();
        if !status.success() {
            return None;
        }
        if i >= WARMUP {
            samples.push((elapsed, limit));
        }
    }
    Some(summarize(
        "PowerShell Get-ChildItem",
        &samples,
        "cold .NET recursive scan",
    ))
}

#[allow(dead_code)]
fn bench_cmd_dir(query: &str, root: &Path, limit: usize) -> Option<BenchResult> {
    let root = root.to_string_lossy();
    let mut samples = Vec::with_capacity(WARMUP + SLOW_ITERS);
    for i in 0..(WARMUP + SLOW_ITERS) {
        let start = Instant::now();
        let output = Command::new("cmd")
            .args(["/C", &format!("dir /s /b \"{root}\\*{query}*\"")])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let elapsed = start.elapsed();
        let count = output
            .stdout
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .count()
            .min(limit);
        if i >= WARMUP {
            samples.push((elapsed, count));
        }
    }
    Some(summarize(
        "cmd dir /s /b",
        &samples,
        "cold Windows CMD recursive listing",
    ))
}

fn tool_path(name: &str) -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    if let Some(home) = home {
        let candidate = home.join(".cargo").join("bin").join(format!("{name}.exe"));
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(name)
}

fn summarize(name: &str, samples: &[(Duration, usize)], notes: &str) -> BenchResult {
    let mut ms: Vec<f64> = samples.iter().map(|(d, _)| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = ms[ms.len() / 2];
    let p95_idx = ((ms.len() as f64 * 0.95) as usize).min(ms.len().saturating_sub(1));
    let p95 = ms[p95_idx];
    let results = samples.last().map(|(_, n)| *n).unwrap_or(0);
    BenchResult {
        name: name.to_string(),
        median_ms: median,
        p95_ms: p95,
        results,
        notes: notes.to_string(),
    }
}

fn print_result(r: &BenchResult) {
    println!(
        "  {:28} {:>8.2}ms median  {:>8.2}ms p95  {:>5} hits  ({})",
        r.name, r.median_ms, r.p95_ms, r.results, r.notes
    );
}

fn print_summary(results: &[BenchResult], file_count: usize, index_time: Duration) {
    println!("=== Summary ===");
    println!("Corpus: {file_count} files");
    println!(
        "CodeSearch index amortization: {:.1}s one-time build",
        index_time.as_secs_f64()
    );
    println!();
    println!("Tool                    | Typical role");
    println!("------------------------|------------------------------------------");
    println!("codesearch (indexed)    | Sub-ms search after index build");
    println!("ripgrep --files         | Fast cold scan, no persistent index");
    println!("fd                      | Fast cold scan, user-friendly");
    println!("PowerShell              | Baseline scripting approach");
    println!("cmd dir /s              | Legacy Windows baseline");

    let cs: Vec<_> = results
        .iter()
        .filter(|r| r.name.starts_with("codesearch"))
        .collect();
    let others: Vec<_> = results
        .iter()
        .filter(|r| !r.name.starts_with("codesearch"))
        .collect();

    if let (Some(cs), Some(other)) = (
        cs.iter().map(|r| r.median_ms).reduce(f64::min),
        others.iter().map(|r| r.median_ms).reduce(f64::min),
    ) {
        println!();
        println!(
            "Fastest cold-scan competitor: {:.2}ms vs codesearch warm: {:.2}ms ({:.0}x faster search)",
            other,
            cs,
            other / cs
        );
    }
}

fn bench_codesearch_lines_warm(engine: &mut CodeSearch, query: &str, limit: usize) -> BenchResult {
    let mut samples = Vec::with_capacity(WARMUP + COLD_ITERS);
    for i in 0..(WARMUP + COLD_ITERS) {
        let start = Instant::now();
        let results = engine.search_lines(query, limit);
        let elapsed = start.elapsed();
        if i >= WARMUP {
            samples.push((elapsed, results.len()));
        }
    }
    summarize("codesearch lines (warm)", &samples, "warm in-memory parallel content scan")
}

fn bench_codesearch_lines_cold(root: &Path, query: &str, limit: usize) -> BenchResult {
    let mut samples = Vec::with_capacity(WARMUP + COLD_ITERS);
    for i in 0..(WARMUP + COLD_ITERS) {
        let start = Instant::now();
        let engine = CodeSearch::with_options(
            root,
            codesearch::CodeSearchOptions {
                watch: false,
                prefer_cache: true,
            },
        );
        let results = engine.search_lines(query, limit);
        let elapsed = start.elapsed();
        if i >= WARMUP {
            samples.push((elapsed, results.len()));
        }
    }
    summarize("codesearch lines (cold)", &samples, "cold start: reload cache + parallel content scan")
}

fn rg_content_command(query: &str, root: &Path) -> Command {
    let mut cmd = Command::new(tool_path("rg"));
    cmd.arg(query)
        .arg("--no-ignore-vcs")
        .arg(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}