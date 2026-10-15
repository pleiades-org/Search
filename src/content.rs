use crate::arena::{FileEntry, IndexArena};
use memchr::{memchr, memchr_iter, memmem::Finder, memrchr};
use rayon::prelude::*;
use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    path::PathBuf,
};

const READ_BUFFER_BYTES: usize = 256 * 1024;
const MIN_READ_BUFFER_BYTES: usize = 4096;
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const PARALLEL_FILES: usize = 32;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CaseMode {
    #[default]
    Sensitive,
    Insensitive,
}

#[derive(Debug, Clone)]
pub struct ContentSearchOptions {
    pub case_mode: CaseMode,
    pub buffer_bytes: usize,
    pub max_line_bytes: usize,
    pub parallel_files: usize,
}

impl Default for ContentSearchOptions {
    fn default() -> Self {
        Self {
            case_mode: CaseMode::Sensitive,
            buffer_bytes: READ_BUFFER_BYTES,
            max_line_bytes: MAX_LINE_BYTES,
            parallel_files: PARALLEL_FILES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    pub path: String,
    pub line_number: usize,
    pub content: String,
}

#[derive(Debug)]
pub struct ContentError {
    pub path: Option<PathBuf>,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct ContentSearchReport {
    pub matches: Vec<LineMatch>,
    pub errors: Vec<ContentError>,
    pub cancelled: bool,
}

/// Compatibility convenience API. Use the report API to handle partial-read diagnostics.
pub fn search_line_contents(arena: &IndexArena, term: &str, limit: usize) -> Vec<LineMatch> {
    let report =
        search_line_contents_with_options(arena, term, limit, &ContentSearchOptions::default());
    for error in &report.errors {
        eprintln!("content search: {:?}: {}", error.path, error.message);
    }
    report.matches
}

pub fn search_line_contents_with_options(
    arena: &IndexArena,
    term: &str,
    limit: usize,
    options: &ContentSearchOptions,
) -> ContentSearchReport {
    search_line_contents_with_cancel(arena, term, limit, options, &|| false)
}

/// Search bounded batches; merge in catalog/file-line order, independent of worker scheduling.
pub fn search_line_contents_with_cancel(
    arena: &IndexArena,
    term: &str,
    limit: usize,
    options: &ContentSearchOptions,
    cancelled: &(impl Fn() -> bool + Sync),
) -> ContentSearchReport {
    let mut report = ContentSearchReport::default();
    if term.is_empty() || limit == 0 {
        return report;
    }
    if term.contains(['\r', '\n'])
        || options.buffer_bytes == 0
        || options.parallel_files == 0
        || options.max_line_bytes < options.buffer_bytes
    {
        report.errors.push(ContentError { path: None,
            message: "expected a single-line literal and positive buffer/batch sizes within the line limit".into() });
        return report;
    }
    let normalized = match options.case_mode {
        CaseMode::Sensitive => term.to_owned(),
        CaseMode::Insensitive => term.to_lowercase(),
    };
    let finder = Finder::new(normalized.as_bytes());
    for batch in arena.files.chunks(options.parallel_files) {
        if cancelled() {
            report.cancelled = true;
            break;
        }
        let remaining = limit - report.matches.len();
        let results: Vec<_> = batch
            .par_iter()
            .map(|file| search_file(file, &finder, &normalized, remaining, options, cancelled))
            .collect();
        merge_batch(batch, results, limit, &mut report);
        if report.matches.len() == limit {
            break;
        }
    }
    report.cancelled |= cancelled();
    report
}

fn merge_batch(
    files: &[FileEntry],
    results: Vec<io::Result<Vec<LineMatch>>>,
    limit: usize,
    report: &mut ContentSearchReport,
) {
    for (file, result) in files.iter().zip(results) {
        match result {
            Ok(matches) => report
                .matches
                .extend(matches.into_iter().take(limit - report.matches.len())),
            Err(error) => report.errors.push(ContentError {
                path: Some(file.native_path.clone()),
                message: error.to_string(),
            }),
        }
    }
}

fn search_file(
    file: &FileEntry,
    finder: &Finder<'_>,
    query: &str,
    limit: usize,
    options: &ContentSearchOptions,
    cancelled: &(impl Fn() -> bool + Sync),
) -> io::Result<Vec<LineMatch>> {
    let input = File::open(&file.native_path)?;
    let size = usize::try_from(input.metadata()?.len()).unwrap_or(options.buffer_bytes);
    let capacity = size.max(MIN_READ_BUFFER_BYTES).min(options.buffer_bytes);
    let mut reader = BufReader::with_capacity(capacity, input);
    // Like conventional text scanners, binary classification samples the initial buffer.
    if reader.fill_buf()?.contains(&0) {
        return Ok(Vec::new());
    }
    let mut matches = Vec::new();
    let mut line_number = 1;
    while matches.len() < limit && !cancelled() {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            break;
        }
        if let Some(last_newline) = memrchr(b'\n', bytes) {
            let block = &bytes[..=last_newline];
            scan_block(
                file,
                block,
                line_number,
                finder,
                query,
                limit,
                options.case_mode,
                &mut matches,
            )?;
            line_number += memchr_iter(b'\n', block).count();
            reader.consume(last_newline + 1);
        } else {
            let line = read_bounded_line(&mut reader, options.max_line_bytes)?;
            scan_block(
                file,
                &line,
                line_number,
                finder,
                query,
                limit,
                options.case_mode,
                &mut matches,
            )?;
            line_number += 1;
        }
    }
    Ok(matches)
}

fn read_bounded_line(reader: &mut impl BufRead, maximum: usize) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return Ok(line);
        }
        let length = memchr(b'\n', bytes).map_or(bytes.len(), |position| position + 1);
        if length > maximum.saturating_sub(line.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "line exceeds configured memory limit",
            ));
        }
        let finished = bytes[length - 1] == b'\n';
        line.extend_from_slice(&bytes[..length]);
        reader.consume(length);
        if finished {
            return Ok(line);
        }
    }
}

#[allow(clippy::too_many_arguments)] // The scan context is explicit and shared by both case modes.
fn scan_block(
    file: &FileEntry,
    bytes: &[u8],
    first_line: usize,
    finder: &Finder<'_>,
    query: &str,
    limit: usize,
    case_mode: CaseMode,
    matches: &mut Vec<LineMatch>,
) -> io::Result<()> {
    if case_mode == CaseMode::Insensitive {
        return scan_insensitive(file, bytes, first_line, query, limit, matches);
    }
    let mut cursor = 0;
    let mut line_number = first_line;
    while cursor < bytes.len() && matches.len() < limit {
        let Some(relative) = finder.find(&bytes[cursor..]) else {
            break;
        };
        let hit = cursor + relative;
        line_number += memchr_iter(b'\n', &bytes[cursor..hit]).count();
        let start =
            memrchr(b'\n', &bytes[cursor..hit]).map_or(cursor, |position| cursor + position + 1);
        let end = memchr(b'\n', &bytes[hit..]).map_or(bytes.len(), |position| hit + position);
        append_match(file, &bytes[start..end], line_number, matches)?;
        cursor = end + 1;
        line_number += 1;
    }
    Ok(())
}

fn scan_insensitive(
    file: &FileEntry,
    bytes: &[u8],
    first_line: usize,
    query: &str,
    limit: usize,
    matches: &mut Vec<LineMatch>,
) -> io::Result<()> {
    for (offset, line) in bytes.split(|&byte| byte == b'\n').enumerate() {
        if matches.len() == limit {
            break;
        }
        let text = std::str::from_utf8(line)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if text.to_lowercase().contains(query) {
            append_match(file, line, first_line + offset, matches)?;
        }
    }
    Ok(())
}

fn append_match(
    file: &FileEntry,
    bytes: &[u8],
    line_number: usize,
    matches: &mut Vec<LineMatch>,
) -> io::Result<()> {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    let content = std::str::from_utf8(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    matches.push(LineMatch {
        path: file.path.clone(),
        line_number,
        content: content.to_owned(),
    });
    Ok(())
}
