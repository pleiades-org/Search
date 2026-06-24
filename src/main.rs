use clap::Parser;
use codesearch::CodeSearch;
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "codesearch",
    about = "Ultra-fast file finder with middle-out graph search and line-content support"
)]
struct Args {
    /// Search term (or file path if using --lines)
    term: String,
    /// Search root (default: home directory — broad search including Pictures, code, etc.)
    #[arg(short, long, value_name = "DIR")]
    dir: Option<String>,
    #[arg(short, long, default_value = "50")]
    limit: usize,
    #[arg(long)]
    rebuild: bool,
    #[arg(long)]
    stats: bool,
    #[arg(long)]
    watch: bool,
    /// Search inside file contents line-by-line in parallel
    #[arg(short, long)]
    content: bool,
    /// Extract a range of lines from a file, e.g. "100-250" or "100"
    #[arg(long, value_name = "RANGE")]
    lines: Option<String>,
}

fn main() {
    let args = Args::parse();

    if let Some(range_str) = &args.lines {
        print_lines(&args.term, range_str);
        return;
    }

    let root = args.dir.as_deref().unwrap_or("~");

    let mut engine = CodeSearch::with_options(
        root,
        codesearch::CodeSearchOptions {
            watch: args.watch,
            prefer_cache: true,
        },
    );

    if args.rebuild {
        engine.rebuild();
    }

    if args.stats {
        eprintln!("root: {}", engine.root_path().display());
        eprintln!("files: {}", engine.file_count());
        eprintln!("global midpoint: {}", engine.global_midpoint());
        eprintln!("mft path: {}", engine.used_mft_path());
    }

    let started = Instant::now();
    if args.content {
        let results = engine.search_lines(&args.term, args.limit);
        let elapsed = started.elapsed();

        for result in &results {
            println!("{}:{}: {}", result.path, result.line_number, result.content);
        }

        eprintln!(
            "{} line matches in {:?}",
            results.len(),
            elapsed
        );
    } else {
        let results = engine.search(&args.term, args.limit);
        let elapsed = started.elapsed();

        for result in &results {
            println!("{result}");
        }

        eprintln!(
            "{} results in {:?}",
            results.len(),
            elapsed
        );
    }
}

fn print_lines(file_path: &str, range_str: &str) {
    let parts: Vec<&str> = range_str.split('-').collect();
    let (start, end) = match parts.as_slice() {
        [s, e] => {
            let s = s.parse::<usize>().unwrap_or(1);
            let e = e.parse::<usize>().unwrap_or(s);
            (s, e)
        }
        [s] => {
            let s = s.parse::<usize>().unwrap_or(1);
            (s, s)
        }
        _ => {
            eprintln!("invalid range format: expected 'start-end' or 'line_number'");
            return;
        }
    };

    let file = match std::fs::File::open(file_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("failed to open file '{}': {}", file_path, e);
            return;
        }
    };

    use std::io::{BufRead, BufReader};
    let reader = BufReader::new(file);
    for (idx, line) in reader.lines().enumerate() {
        let line_num = idx + 1;
        if line_num >= start && line_num <= end {
            match line {
                Ok(l) => println!("{}: {}", line_num, l),
                Err(e) => {
                    eprintln!("error reading line {}: {}", line_num, e);
                    break;
                }
            }
        }
        if line_num > end {
            break;
        }
    }
}