//! Parallel SIMD-accelerated `grep` using `regex::bytes::Regex` and `ignore::WalkBuilder`.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use clap::Parser;
use ignore::{WalkBuilder, WalkState};
use memchr::{memchr, memchr_iter};
use regex::bytes::{Regex, RegexBuilder};

const READ_CHUNK: usize = 64 * 1024;
const MAX_LINE_CAP: usize = 16 * 1024 * 1024;

#[derive(Parser, Debug, Clone)]
#[command(name = "grep", version, about = "Search for PATTERN in each FILE or standard input")]
struct Args {
    /// Ignore case distinctions in patterns and input data
    #[arg(short = 'i', long = "ignore-case")]
    ignore_case: bool,

    /// Select non-matching lines
    #[arg(short = 'v', long = "invert-match")]
    invert_match: bool,

    /// Prefix each line of output with the 1-based line number
    #[arg(short = 'n', long = "line-number")]
    line_number: bool,

    /// Print only a count of selected lines per FILE
    #[arg(short = 'c', long = "count")]
    count: bool,

    /// Print only names of FILEs with selected lines
    #[arg(short = 'l', long = "files-with-matches")]
    files_with_matches: bool,

    /// Select only those lines containing matches that form whole words
    #[arg(short = 'w', long = "word-regexp")]
    word_regexp: bool,

    /// Interpret PATTERN as fixed literal strings, not regular expressions
    #[arg(short = 'F', long = "fixed-strings")]
    fixed_strings: bool,

    /// Read all files under each directory, recursively (respects .gitignore)
    #[arg(short = 'r', short_alias = 'R', long = "recursive")]
    recursive: bool,

    /// Print the file name for each match
    #[arg(short = 'H', long = "with-filename", overrides_with = "no_filename")]
    with_filename: bool,

    /// Suppress the prefixing of file names on output
    #[arg(short = 'h', long = "no-filename", overrides_with = "with_filename")]
    no_filename: bool,

    /// Process a binary file as if it were text
    #[arg(short = 'a', long = "text")]
    text: bool,

    /// Stop reading a file after NUM matching lines
    #[arg(short = 'm', long = "max-count")]
    max_count: Option<usize>,

    /// Suppress normal output; exit immediately with 0 if any match is found
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,

    /// Pattern to search for
    pattern: String,

    /// Files or directories to search (defaults to stdin if omitted, or "." with -r)
    paths: Vec<PathBuf>,
}

fn compile_matcher(args: &Args) -> Result<Regex, regex::Error> {
    let mut pat = if args.fixed_strings {
        regex::escape(&args.pattern)
    } else {
        args.pattern.clone()
    };
    if args.word_regexp {
        pat = format!(r"\b(?:{pat})\b");
    }
    RegexBuilder::new(&pat)
        .case_insensitive(args.ignore_case)
        .unicode(false)
        .size_limit(16 * 1024 * 1024)
        .build()
}

struct SearchConfig {
    re: Regex,
    args: Args,
    show_filename: bool,
    matched_any: AtomicBool,
    had_error: AtomicBool,
    active_matches: AtomicUsize,
}

/// Reusable per-thread buffers to eliminate heap allocations inside the scan loop.
struct WorkerScratch {
    io_buf: Vec<u8>,
    carry: Vec<u8>,
    out_buf: Vec<u8>,
}

impl WorkerScratch {
    fn new() -> Self {
        Self {
            io_buf: vec![0u8; READ_CHUNK],
            carry: Vec::with_capacity(4096),
            out_buf: Vec::with_capacity(READ_CHUNK),
        }
    }
}

#[inline]
fn check_line(re: &Regex, line: &[u8], invert: bool) -> bool {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    re.is_match(line) ^ invert
}

#[inline]
fn format_match_line(
    out: &mut Vec<u8>,
    label: Option<&str>,
    line_no: Option<usize>,
    line: &[u8],
) {
    if let Some(name) = label {
        out.extend_from_slice(name.as_bytes());
        out.push(b':');
    }
    if let Some(n) = line_no {
        let mut num_buf = [0u8; 24];
        let s = format_usize(n, &mut num_buf);
        out.extend_from_slice(s);
        out.push(b':');
    }
    out.extend_from_slice(line);
    out.push(b'\n');
}

#[inline]
fn format_usize(mut n: usize, buf: &mut [u8; 24]) -> &[u8] {
    if n == 0 {
        buf[23] = b'0';
        return &buf[23..];
    }
    let mut i = 24;
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    &buf[i..]
}

fn scan_reader<R: Read>(
    mut reader: R,
    label: Option<&str>,
    cfg: &SearchConfig,
    scratch: &mut WorkerScratch,
) -> io::Result<()> {
    scratch.carry.clear();
    scratch.out_buf.clear();

    let max_count = cfg.args.max_count.unwrap_or(usize::MAX);
    let mut match_count = 0usize;
    let mut line_no = 0usize;
    let mut binary_checked = cfg.args.text;
    let file_label = if cfg.show_filename { label } else { None };
    let want_line_no = cfg.args.line_number;

    'read_loop: loop {
        let n = reader.read(&mut scratch.io_buf)?;
        if n == 0 {
            break;
        }
        let chunk = &scratch.io_buf[..n];

        if !binary_checked {
            if memchr(0, chunk).is_some() {
                return Ok(());
            }
            binary_checked = true;
        }

        let mut start = 0usize;
        for nl in memchr_iter(b'\n', chunk) {
            line_no += 1;
            let matched = if scratch.carry.is_empty() {
                let slice = &chunk[start..nl];
                let hit = check_line(&cfg.re, slice, cfg.args.invert_match);
                if hit && !cfg.args.quiet && !cfg.args.count && !cfg.args.files_with_matches {
                    format_match_line(
                        &mut scratch.out_buf,
                        file_label,
                        want_line_no.then_some(line_no),
                        slice,
                    );
                }
                hit
            } else {
                scratch.carry.extend_from_slice(&chunk[start..nl]);
                let hit = check_line(&cfg.re, &scratch.carry, cfg.args.invert_match);
                if hit && !cfg.args.quiet && !cfg.args.count && !cfg.args.files_with_matches {
                    format_match_line(
                        &mut scratch.out_buf,
                        file_label,
                        want_line_no.then_some(line_no),
                        &scratch.carry,
                    );
                }
                scratch.carry.clear();
                hit
            };

            start = nl + 1;

            if matched {
                match_count += 1;
                cfg.matched_any.store(true, Ordering::Relaxed);
                if cfg.args.quiet || cfg.args.files_with_matches || match_count >= max_count {
                    break 'read_loop;
                }
            }

            if scratch.out_buf.len() >= READ_CHUNK {
                flush_stdout(&mut scratch.out_buf)?;
            }
        }

        if start < n {
            if scratch.carry.len() + (n - start) > MAX_LINE_CAP {
                scratch.carry.clear();
            } else {
                scratch.carry.extend_from_slice(&chunk[start..n]);
            }
        }
    }

    // Handle trailing line without newline at EOF.
    if !scratch.carry.is_empty()
        && match_count < max_count
        && !(cfg.args.quiet && cfg.matched_any.load(Ordering::Relaxed))
        && !(cfg.args.files_with_matches && match_count > 0)
    {
        line_no += 1;
        if check_line(&cfg.re, &scratch.carry, cfg.args.invert_match) {
            match_count += 1;
            cfg.matched_any.store(true, Ordering::Relaxed);
            if !cfg.args.quiet && !cfg.args.count && !cfg.args.files_with_matches {
                format_match_line(
                    &mut scratch.out_buf,
                    file_label,
                    want_line_no.then_some(line_no),
                    &scratch.carry,
                );
            }
        }
        scratch.carry.clear();
    }

    if cfg.args.quiet {
        return Ok(());
    }

    if cfg.args.files_with_matches {
        if match_count > 0 {
            let name = label.unwrap_or("(standard input)");
            scratch.out_buf.extend_from_slice(name.as_bytes());
            scratch.out_buf.push(b'\n');
        }
    } else if cfg.args.count {
        if let Some(name) = file_label {
            scratch.out_buf.extend_from_slice(name.as_bytes());
            scratch.out_buf.push(b':');
        }
        let mut num_buf = [0u8; 24];
        scratch
            .out_buf
            .extend_from_slice(format_usize(match_count, &mut num_buf));
        scratch.out_buf.push(b'\n');
    }

    if !scratch.out_buf.is_empty() {
        flush_stdout(&mut scratch.out_buf)?;
    }
    cfg.active_matches.fetch_add(match_count, Ordering::Relaxed);
    Ok(())
}

fn flush_stdout(buf: &mut Vec<u8>) -> io::Result<()> {
    let stdout = io::stdout();
    let mut lock = BufWriter::new(stdout.lock());
    lock.write_all(buf)?;
    lock.flush()?;
    buf.clear();
    Ok(())
}

fn scan_path(path: &Path, cfg: &SearchConfig, scratch: &mut WorkerScratch) -> io::Result<()> {
    let label = path.to_string_lossy();
    match File::open(path) {
        Ok(file) => scan_reader(file, Some(&label), cfg, scratch),
        Err(e) => {
            eprintln!("grep: {label}: {e}");
            cfg.had_error.store(true, Ordering::Relaxed);
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    let re = match compile_matcher(&args) {
        Ok(re) => re,
        Err(e) => {
            eprintln!("grep: invalid pattern: {e}");
            return ExitCode::from(2);
        }
    };

    let paths = if args.paths.is_empty() {
        if args.recursive {
            vec![PathBuf::from(".")]
        } else {
            vec![PathBuf::from("-")]
        }
    } else {
        args.paths.clone()
    };

    let any_dir = args.recursive || paths.iter().any(|p| p.is_dir());
    let show_filename = if args.with_filename {
        true
    } else if args.no_filename {
        false
    } else {
        any_dir || paths.len() > 1
    };

    let cfg = Arc::new(SearchConfig {
        re,
        args: args.clone(),
        show_filename,
        matched_any: AtomicBool::new(false),
        had_error: AtomicBool::new(false),
        active_matches: AtomicUsize::new(0),
    });

    if !any_dir {
        let mut scratch = WorkerScratch::new();
        for p in &paths {
            if args.quiet && cfg.matched_any.load(Ordering::Relaxed) {
                break;
            }
            let res = if p.as_os_str() == "-" {
                let stdin = io::stdin();
                scan_reader(stdin.lock(), Some("(standard input)"), &cfg, &mut scratch)
            } else {
                scan_path(p, &cfg, &mut scratch)
            };
            if let Err(e) = res {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    return ExitCode::SUCCESS;
                }
                eprintln!("grep: I/O error: {e}");
                cfg.had_error.store(true, Ordering::Relaxed);
            }
        }
    } else {
        let mut builder = WalkBuilder::new(&paths[0]);
        for p in paths.iter().skip(1) {
            builder.add(p);
        }
        builder
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false)
            .threads(
                std::thread::available_parallelism()
                    .map_or(4, |n| n.get())
                    .clamp(2, 8),
            )
            .filter_entry(|e| e.file_name() != ".git");

        let walker = builder.build_parallel();
        walker.run(|| {
            let cfg = Arc::clone(&cfg);
            let mut scratch = WorkerScratch::new();
            Box::new(move |entry| {
                if cfg.args.quiet && cfg.matched_any.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                match entry {
                    Ok(d) if d.file_type().is_some_and(|t| t.is_file()) => {
                        if let Err(e) = scan_path(d.path(), &cfg, &mut scratch)
                            && e.kind() == io::ErrorKind::BrokenPipe
                        {
                            return WalkState::Quit;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("grep: {e}");
                        cfg.had_error.store(true, Ordering::Relaxed);
                    }
                }
                WalkState::Continue
            })
        });
    }

    if cfg.had_error.load(Ordering::Relaxed) && !cfg.args.quiet {
        ExitCode::from(2)
    } else if cfg.matched_any.load(Ordering::Relaxed) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
