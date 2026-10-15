mod fixtures;
mod measurement;

use codesearch::{
    CodeSearch, CodeSearchOptions,
    arena::IndexArena,
    search::{middle_out_search, search_line_contents},
};
use measurement::Measurements;
use std::{
    collections::HashSet,
    fs::{self, File},
    hint::black_box,
    io::{self, BufRead, BufReader, Write},
    path::Path,
};

trait PostingBytes {
    fn storage_bytes(&self) -> usize;
}
impl PostingBytes for Vec<u32> {
    fn storage_bytes(&self) -> usize {
        self.capacity() * size_of::<u32>()
    }
}
impl PostingBytes for Box<[u32]> {
    fn storage_bytes(&self) -> usize {
        self.len() * size_of::<u32>()
    }
}

fn index_bytes(arena: &IndexArena) -> usize {
    let index = arena.search_index();
    index
        .trigram_postings
        .values()
        .map(PostingBytes::storage_bytes)
        .sum::<usize>()
        + index
            .len_buckets
            .iter()
            .map(|bucket| bucket.capacity() * size_of::<u32>())
            .sum::<usize>()
        + index.visit_order.capacity() * size_of::<u32>()
}

fn validate_filename(arena: &IndexArena, query: &str, limit: usize) -> usize {
    let expected: HashSet<_> = arena
        .files
        .iter()
        .filter(|file| file.name_lower.contains(query))
        .map(|file| file.path.as_str())
        .collect();
    let results = middle_out_search(arena, query, limit);
    let unique: HashSet<_> = results.iter().map(String::as_str).collect();
    assert_eq!(unique.len(), results.len(), "duplicate filename hits");
    assert!(unique.is_subset(&expected), "invalid filename hit");
    assert_eq!(results.len(), limit.min(expected.len()));
    let all = middle_out_search(arena, query, arena.files.len());
    assert_eq!(
        all.iter().map(String::as_str).collect::<HashSet<_>>(),
        expected
    );
    results.len()
}

fn filenames(measurements: &mut Measurements<'_>, memory: &mut File) -> io::Result<()> {
    for count in [2_000, 150_000] {
        let arena = fixtures::filename_arena(count);
        writeln!(
            memory,
            "{count},{},{}",
            index_bytes(&arena),
            arena.files.len()
        )?;
        for query in ["a", "report", "needle_tag_7fd9", "zzqxyw_no_such_file"] {
            let expected_count = validate_filename(&arena, query, 50);
            measurements.measure(
                &format!("filename/{count}/{query}"),
                101,
                expected_count,
                || black_box(middle_out_search(&arena, black_box(query), 50)).len(),
            )?;
        }
        measurements.measure(&format!("build/{count}"), 11, count, || {
            black_box(fixtures::filename_arena(count)).files.len()
        })?;
    }
    Ok(())
}

fn content_oracle(arena: &IndexArena, query: &str) -> io::Result<HashSet<(String, usize, String)>> {
    let mut expected = HashSet::new();
    for file in &arena.files {
        for (ordinal, line) in BufReader::new(File::open(&file.path)?).lines().enumerate() {
            let line = line?;
            if line.contains(query) {
                expected.insert((file.path.clone(), ordinal + 1, line));
            }
        }
    }
    Ok(expected)
}

fn contents(directory: &Path, measurements: &mut Measurements<'_>) -> io::Result<()> {
    for corpus in ["many", "single"] {
        let arena = fixtures::content_arena(&directory.join(corpus))?;
        for query in ["needle_tag_7fd9", "return", "zzqxyw_no_such_content"] {
            let expected = content_oracle(&arena, query)?;
            let all = search_line_contents(&arena, query, usize::MAX);
            let actual: HashSet<_> = all
                .into_iter()
                .map(|hit| (hit.path, hit.line_number, hit.content))
                .collect();
            assert_eq!(
                actual, expected,
                "wrong complete content results for {corpus}/{query}"
            );
            for limit in [12, usize::MAX] {
                let label = if limit == 12 { "first12" } else { "all" };
                let count = limit.min(expected.len());
                measurements.measure(
                    &format!("content/{corpus}/{query}/{label}"),
                    21,
                    count,
                    || black_box(search_line_contents(&arena, black_box(query), limit)).len(),
                )?;
            }
        }
    }
    Ok(())
}

fn run(directory: &Path, label: &str) -> io::Result<()> {
    if !label
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(io::Error::other(
            "label must contain only letters, digits or hyphens",
        ));
    }
    let output = directory.join("results");
    fs::create_dir_all(&output)?;
    let mut summary = File::create(output.join(format!("{label}-summary.csv")))?;
    let mut samples = File::create(output.join(format!("{label}-samples.csv")))?;
    let mut memory = File::create(output.join(format!("{label}-index.csv")))?;
    writeln!(summary, "case,samples,matches,p50_us,p95_us")?;
    writeln!(samples, "case,sample,elapsed_ns,matches")?;
    writeln!(memory, "names,index_vector_bytes,files")?;
    let mut measurements = Measurements {
        summary: &mut summary,
        samples: &mut samples,
    };
    filenames(&mut measurements, &mut memory)?;
    contents(directory, &mut measurements)?;
    startup(directory, &mut measurements)
}

fn startup(directory: &Path, measurements: &mut Measurements<'_>) -> io::Result<()> {
    let root = directory.join("many");
    drop(CodeSearch::with_options(
        &root,
        CodeSearchOptions {
            watch: false,
            prefer_cache: false,
        },
    ));
    for (case, prefer_cache) in [("startup/fresh", false), ("startup/trusted-cache", true)] {
        measurements.measure(case, 21, 256, || {
            CodeSearch::with_options(
                &root,
                CodeSearchOptions {
                    watch: false,
                    prefer_cache,
                },
            )
            .file_count()
        })?;
    }
    measurements.measure("startup/default-policy", 21, 256, || {
        CodeSearch::new(&root).file_count()
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    match arguments.as_slice() {
        [_, mode, directory] if mode == "prepare" => fixtures::prepare(Path::new(directory))?,
        [_, mode, directory, label] if mode == "run" => run(Path::new(directory), label)?,
        _ => return Err("usage: search-comparison prepare DIRECTORY | run DIRECTORY LABEL".into()),
    }
    Ok(())
}
