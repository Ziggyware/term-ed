//! High-performance `tail` utility using reverse 64 KiB page scanning for
//! seekable files and a contiguous arena ring buffer for non-seekable pipes.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use memchr::{memchr_iter, memrchr_iter};

const PAGE_SIZE: usize = 64 * 1024;

#[derive(Parser, Debug)]
#[command(name = "tail", version, about = "Output the last part of files")]
struct Args {
    /// Output the last NUM lines instead of the last 10
    #[arg(short = 'n', long = "lines", default_value_t = 10)]
    lines: usize,

    /// Output the last NUM bytes of each file (overrides --lines)
    #[arg(short = 'c', long = "bytes")]
    bytes: Option<usize>,

    /// Never output headers giving file names
    #[arg(short = 'q', long = "quiet", overrides_with = "verbose")]
    quiet: bool,

    /// Always output headers giving file names
    #[arg(short = 'v', long = "verbose", overrides_with = "quiet")]
    verbose: bool,

    /// Input files (defaults to stdin if omitted or "-")
    files: Vec<PathBuf>,
}

fn tail_file_lines<W: Write>(
    file: &mut File,
    file_len: u64,
    out: &mut W,
    lines: usize,
    buf: &mut [u8],
) -> io::Result<()> {
    if lines == 0 || file_len == 0 {
        return Ok(());
    }

    let mut pos = file_len;
    let mut newlines_needed = lines;
    let mut check_trailing_newline = true;
    let mut start_offset = 0u64;

    'outer: while pos > 0 {
        let read_len = (pos as usize).min(buf.len());
        pos -= read_len as u64;
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut buf[..read_len])?;

        let chunk = &buf[..read_len];
        for idx in memrchr_iter(b'\n', chunk) {
            if check_trailing_newline && pos + (idx as u64) + 1 == file_len {
                check_trailing_newline = false;
                continue;
            }
            check_trailing_newline = false;
            if newlines_needed == 0 {
                start_offset = pos + (idx as u64) + 1;
                break 'outer;
            }
            newlines_needed -= 1;
            if newlines_needed == 0 {
                start_offset = pos + (idx as u64) + 1;
                break 'outer;
            }
        }
        check_trailing_newline = false;
    }

    file.seek(SeekFrom::Start(start_offset))?;
    loop {
        let n = file.read(buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
    }
    Ok(())
}

fn tail_stream_lines<R: Read, W: Write>(
    mut reader: R,
    out: &mut W,
    lines: usize,
    buf: &mut [u8],
) -> io::Result<()> {
    if lines == 0 {
        return Ok(());
    }
    let mut arena: Vec<u8> = Vec::with_capacity(PAGE_SIZE);
    let mut line_starts: VecDeque<usize> = VecDeque::with_capacity(lines + 2);
    line_starts.push_back(0);

    loop {
        let n = reader.read(buf)?;
        if n == 0 {
            break;
        }
        let base = arena.len();
        arena.extend_from_slice(&buf[..n]);
        for nl in memchr_iter(b'\n', &buf[..n]) {
            line_starts.push_back(base + nl + 1);
            while line_starts.len() > lines + 1 {
                line_starts.pop_front();
            }
        }

        if let Some(&first_keep) = line_starts.front()
            && first_keep >= 256 * 1024
            && first_keep * 2 >= arena.len()
        {
            arena.drain(..first_keep);
            for s in &mut line_starts {
                *s -= first_keep;
            }
        }
    }

    if arena.is_empty() {
        return Ok(());
    }
    if line_starts.back() == Some(&arena.len()) {
        line_starts.pop_back();
    }
    while line_starts.len() > lines {
        line_starts.pop_front();
    }
    let start = line_starts.front().copied().unwrap_or(0);
    out.write_all(&arena[start..])
}

fn tail_stream_bytes<R: Read, W: Write>(
    mut reader: R,
    out: &mut W,
    keep_bytes: usize,
    buf: &mut [u8],
) -> io::Result<()> {
    if keep_bytes == 0 {
        return Ok(());
    }
    let mut ring: VecDeque<u8> = VecDeque::with_capacity(keep_bytes.min(1024 * 1024));
    loop {
        let n = reader.read(buf)?;
        if n == 0 {
            break;
        }
        if n >= keep_bytes {
            ring.clear();
            ring.extend(&buf[n - keep_bytes..n]);
        } else {
            let overflow = (ring.len() + n).saturating_sub(keep_bytes);
            if overflow > 0 {
                ring.drain(..overflow);
            }
            ring.extend(&buf[..n]);
        }
    }
    let (a, b) = ring.as_slices();
    out.write_all(a)?;
    out.write_all(b)
}

fn process_file<W: Write>(
    mut file: File,
    out: &mut W,
    args: &Args,
    buf: &mut [u8],
) -> io::Result<()> {
    if let Ok(meta) = file.metadata() {
        let len = meta.len();
        if let Some(bytes) = args.bytes {
            let start = len.saturating_sub(bytes as u64);
            if file.seek(SeekFrom::Start(start)).is_ok() {
                loop {
                    let n = file.read(buf)?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buf[..n])?;
                }
                return Ok(());
            }
        } else if len > 0 && file.seek(SeekFrom::End(0)).is_ok() {
            return tail_file_lines(&mut file, len, out, args.lines, buf);
        }
    }

    if let Some(bytes) = args.bytes {
        tail_stream_bytes(file, out, bytes, buf)
    } else {
        tail_stream_lines(file, out, args.lines, buf)
    }
}

fn run() -> io::Result<bool> {
    let args = Args::parse();
    let files = if args.files.is_empty() {
        vec![PathBuf::from("-")]
    } else {
        args.files.clone()
    };

    let show_headers = args.verbose || (!args.quiet && files.len() > 1);
    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(PAGE_SIZE, stdout.lock());
    let mut scratch = vec![0u8; PAGE_SIZE];
    let mut had_error = false;

    for (idx, path) in files.iter().enumerate() {
        let is_stdin = path.as_os_str() == "-";
        let display_name = if is_stdin {
            "standard input".to_string()
        } else {
            path.display().to_string()
        };

        if is_stdin {
            if show_headers {
                if idx > 0 {
                    out.write_all(b"\n")?;
                }
                write!(out, "==> {display_name} <==\n")?;
            }
            let stdin = io::stdin();
            if let Some(bytes) = args.bytes {
                tail_stream_bytes(stdin.lock(), &mut out, bytes, &mut scratch)?;
            } else {
                tail_stream_lines(stdin.lock(), &mut out, args.lines, &mut scratch)?;
            }
        } else {
            match File::open(path) {
                Ok(file) => {
                    if show_headers {
                        if idx > 0 {
                            out.write_all(b"\n")?;
                        }
                        write!(out, "==> {display_name} <==\n")?;
                    }
                    process_file(file, &mut out, &args, &mut scratch)?;
                }
                Err(e) => {
                    out.flush()?;
                    eprintln!("tail: cannot open '{display_name}' for reading: {e}");
                    had_error = true;
                }
            }
        }
    }
    out.flush()?;
    Ok(!had_error)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tail: I/O error: {e}");
            ExitCode::from(1)
        }
    }
}
