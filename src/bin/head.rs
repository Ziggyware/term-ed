//! Zero-allocation streaming `head` utility using SIMD `memchr`.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use memchr::memchr_iter;

const BUF_SIZE: usize = 64 * 1024;

#[derive(Parser, Debug)]
#[command(name = "head", version, about = "Output the first part of files")]
struct Args {
    /// Print the first NUM lines instead of the first 10
    #[arg(short = 'n', long = "lines", default_value_t = 10)]
    lines: usize,

    /// Print the first NUM bytes of each file (overrides --lines)
    #[arg(short = 'c', long = "bytes")]
    bytes: Option<usize>,

    /// Never print headers giving file names
    #[arg(short = 'q', long = "quiet", overrides_with = "verbose")]
    quiet: bool,

    /// Always print headers giving file names
    #[arg(short = 'v', long = "verbose", overrides_with = "quiet")]
    verbose: bool,

    /// Input files (defaults to stdin if omitted or "-")
    files: Vec<PathBuf>,
}

fn head_bytes<R: Read, W: Write>(
    mut reader: R,
    out: &mut W,
    mut remaining: usize,
    buf: &mut [u8],
) -> io::Result<()> {
    while remaining > 0 {
        let cap = buf.len().min(remaining);
        let n = reader.read(&mut buf[..cap])?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        remaining -= n;
    }
    Ok(())
}

fn head_lines<R: Read, W: Write>(
    mut reader: R,
    out: &mut W,
    mut remaining_lines: usize,
    buf: &mut [u8],
) -> io::Result<()> {
    if remaining_lines == 0 {
        return Ok(());
    }
    loop {
        let n = reader.read(buf)?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        let mut cut = n;
        for pos in memchr_iter(b'\n', chunk) {
            remaining_lines -= 1;
            if remaining_lines == 0 {
                cut = pos + 1;
                break;
            }
        }
        out.write_all(&chunk[..cut])?;
        if remaining_lines == 0 {
            break;
        }
    }
    Ok(())
}

fn process_stream<R: Read, W: Write>(
    reader: R,
    out: &mut W,
    args: &Args,
    buf: &mut [u8],
) -> io::Result<()> {
    if let Some(bytes) = args.bytes {
        head_bytes(reader, out, bytes, buf)
    } else {
        head_lines(reader, out, args.lines, buf)
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
    let mut out = BufWriter::with_capacity(BUF_SIZE, stdout.lock());
    let mut scratch = vec![0u8; BUF_SIZE];
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
            process_stream(stdin.lock(), &mut out, &args, &mut scratch)?;
        } else {
            match File::open(path) {
                Ok(file) => {
                    if show_headers {
                        if idx > 0 {
                            out.write_all(b"\n")?;
                        }
                        write!(out, "==> {display_name} <==\n")?;
                    }
                    process_stream(file, &mut out, &args, &mut scratch)?;
                }
                Err(e) => {
                    out.flush()?;
                    eprintln!("head: cannot open '{display_name}' for reading: {e}");
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
            eprintln!("head: I/O error: {e}");
            ExitCode::from(1)
        }
    }
}
