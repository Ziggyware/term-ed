//! Clipboard: an internal register, mirrored to the system clipboard through
//! native tools (wl-copy / xclip / xsel / pbcopy) and the OSC 52 escape
//! sequence, which works over SSH and inside tmux.

use std::io::Write;
use std::process::{Command, Stdio};

use base64::Engine;

#[derive(Default)]
pub struct Clipboard {
    text: String,
    /// True when the text was copied as whole lines (copy with no selection).
    pub whole_line: bool,
}

const COPY_TOOLS: &[(&str, &[&str])] = &[
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--input"]),
    ("pbcopy", &[]),
];
const PASTE_TOOLS: &[(&str, &[&str])] = &[
    ("wl-paste", &["--no-newline"]),
    ("xclip", &["-selection", "clipboard", "-o"]),
    ("xsel", &["--clipboard", "--output"]),
    ("pbpaste", &[]),
];

fn graphical_session() -> bool {
    cfg!(target_os = "macos")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("DISPLAY").is_some()
}

impl Clipboard {
    pub fn set(&mut self, text: String, whole_line: bool) {
        self.whole_line = whole_line;
        // OSC 52: ask the terminal to set the system clipboard.
        let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let mut out = std::io::stdout();
        let _ = write!(out, "\x1b]52;c;{b64}\x07");
        let _ = out.flush();
        if graphical_session() {
            for (cmd, args) in COPY_TOOLS {
                if let Ok(mut child) = Command::new(cmd)
                    .args(*args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(text.as_bytes());
                    }
                    let _ = child.wait();
                    break;
                }
            }
        }
        self.text = text;
    }

    /// Returns the clipboard text, preferring the system clipboard when it
    /// holds something different from what we last copied.
    pub fn get(&mut self) -> String {
        if graphical_session() {
            for (cmd, args) in PASTE_TOOLS {
                if let Ok(out) = Command::new(cmd).args(*args).stderr(Stdio::null()).output()
                    && out.status.success()
                {
                    let sys = String::from_utf8_lossy(&out.stdout).into_owned();
                    if !sys.is_empty()
                        && sys != self.text
                        && sys.trim_end_matches('\n') != self.text.trim_end_matches('\n')
                    {
                        self.text = sys;
                        self.whole_line = false;
                    }
                    break;
                }
            }
        }
        self.text.clone()
    }
}
