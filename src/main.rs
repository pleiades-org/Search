use clap::Parser;
use codesearch::{
    CodeSearch, CodeSearchOptions,
    content::{CaseMode, ContentSearchOptions},
};
use std::{
    io::{self, BufRead, BufReader, Write},
    process::ExitCode,
    time::{Duration, Instant},
};

const WATCH_REFRESH: Duration = Duration::from_millis(100);

#[derive(Parser)]
#[command(
    name = "codesearch",
    about = "Indexed filename and bounded literal content search"
)]
struct Args {
    /// Literal term, or file path when using --lines.
    term: String,
    #[arg(short, long, value_name = "DIR")]
    dir: Option<String>,
    #[arg(short, long, default_value = "50")]
    limit: usize,
    /// Reconcile the filesystem instead of trusting a saved snapshot.
    #[arg(long)]
    rebuild: bool,
    /// Trust the last saved filename snapshot (may be stale until rebuilt).
    #[arg(long)]
    cached: bool,
    #[arg(long)]
    stats: bool,
    /// Remain running and print updated results until Ctrl+C.
    #[arg(long, conflicts_with = "lines")]
    watch: bool,
    #[arg(short, long)]
    content: bool,
    /// Unicode lowercase matching for content literals.
    #[arg(short = 'i', long, requires = "content")]
    ignore_case: bool,
    /// Extract a validated inclusive range, such as 100-250.
    #[arg(long, value_name = "RANGE", conflicts_with_all = ["content", "cached", "rebuild"])]
    lines: Option<String>,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("codesearch: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(args: Args) -> io::Result<()> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    if let Some(range) = &args.lines {
        return print_lines(&args.term, range, &mut output);
    }
    if args.content && (args.term.is_empty() || args.term.contains(['\r', '\n'])) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "content term must be a nonempty single-line literal",
        ));
    }
    let root = args.dir.as_deref().unwrap_or("~");
    let engine = CodeSearch::try_with_options(
        root,
        CodeSearchOptions {
            watch: args.watch,
            prefer_cache: args.cached && !args.rebuild,
        },
    )?;
    if args.stats {
        eprintln!(
            "root: {}\nfiles: {}\nmidpoint: {}\nmft: {}\ncached snapshot: {}",
            engine.root_path().display(),
            engine.file_count(),
            engine.global_midpoint(),
            engine.used_mft_path(),
            engine.loaded_cache()
        );
    }
    let mut generation = engine.generation();
    print_results(&engine, &args, &mut output)?;
    if !args.watch {
        return Ok(());
    }
    eprintln!(
        "watching {}; press Ctrl+C to stop",
        engine.root_path().display()
    );
    loop {
        std::thread::sleep(WATCH_REFRESH);
        if engine.generation() != generation {
            generation = engine.generation();
            writeln!(output, "--- index generation {generation} ---")?;
            print_results(&engine, &args, &mut output)?;
        }
        for diagnostic in engine.take_diagnostics() {
            eprintln!("{diagnostic}");
        }
    }
}

fn print_results(engine: &CodeSearch, args: &Args, output: &mut impl Write) -> io::Result<()> {
    let started = Instant::now();
    let count = if args.content {
        let options = ContentSearchOptions {
            case_mode: if args.ignore_case {
                CaseMode::Insensitive
            } else {
                CaseMode::Sensitive
            },
            ..ContentSearchOptions::default()
        };
        let report = engine.search_lines_with_options(&args.term, args.limit, &options);
        for matched in &report.matches {
            writeln!(
                output,
                "{}:{}: {}",
                matched.path, matched.line_number, matched.content
            )?;
        }
        output.flush()?;
        if !report.errors.is_empty() {
            for error in report.errors {
                eprintln!("content read {:?}: {}", error.path, error.message);
            }
            return Err(io::Error::other(
                "content search incomplete; see read diagnostics",
            ));
        }
        report.matches.len()
    } else {
        let results = engine.search(&args.term, args.limit);
        for result in &results {
            writeln!(output, "{result}")?;
        }
        results.len()
    };
    output.flush()?;
    eprintln!(
        "{count} results including output in {:?}",
        started.elapsed()
    );
    for diagnostic in engine.take_diagnostics() {
        eprintln!("{diagnostic}");
    }
    Ok(())
}

fn parse_range(range: &str) -> io::Result<(usize, usize)> {
    let parts: Vec<_> = range.split('-').collect();
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected positive line number or start-end with start <= end",
        )
    };
    let (start, end) = match parts.as_slice() {
        [number] => {
            let number = number.parse::<usize>().map_err(|_| invalid())?;
            (number, number)
        }
        [start, end] => (
            start.parse::<usize>().map_err(|_| invalid())?,
            end.parse::<usize>().map_err(|_| invalid())?,
        ),
        _ => return Err(invalid()),
    };
    if start == 0 || end < start {
        return Err(invalid());
    }
    Ok((start, end))
}

fn print_lines(file: &str, range: &str, output: &mut impl Write) -> io::Result<()> {
    let (start, end) = parse_range(range)?;
    let path = codesearch::util::expand_root(file);
    let reader = BufReader::new(std::fs::File::open(path)?);
    for (ordinal, line) in reader.lines().take(end).enumerate() {
        let line = line?;
        if ordinal + 1 >= start {
            writeln!(output, "{}: {line}", ordinal + 1)?;
        }
    }
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_reject_malformed_zero_and_reversed_input() {
        for range in ["", "abc", "0", "0-1", "10-2", "1-2-3", "-1"] {
            assert!(parse_range(range).is_err(), "{range}");
        }
        assert_eq!(parse_range("1").unwrap(), (1, 1));
        assert_eq!(parse_range("2-10").unwrap(), (2, 10));
    }
}
