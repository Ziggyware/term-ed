//! Clipboard: an internal register, mirrored to the system clipboard through
//! native tools (wl-copy / xclip / xsel / pbcopy) and the OSC 52 escape
//! sequence, which works over SSH and inside tmux.

use std::borrow::Cow;
use std::io::Write;
use std::process::{Command, Stdio};

use base64::Engine;

#[derive(Default)]
pub struct Clipboard {
    text: String,
    /// True when the text was copied as whole lines (copy with no selection).
    pub whole_line: bool,
    /// Native clipboard programs are probed once and then reused. Without
    /// this, every Alt+V/Ctrl+V spawns several known-missing programs first.
    paste_tool: Option<usize>,
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

/// Preserve horizontal whitespace and final newlines, while dropping clipboard
/// control bytes (e.g. a Windows NUL terminator) and normalizing line endings.
pub(crate) fn paste_text(text: &str) -> Cow<'_, str> {
    let unwanted = |c: char| c.is_control() && !matches!(c, '\t' | '\r' | '\n');
    if !text.contains('\r') && !text.contains(unwanted) {
        return Cow::Borrowed(text);
    }
    let mut text = text.replace("\r\n", "\n").replace('\r', "\n");
    text.retain(|c| !unwanted(c));
    Cow::Owned(text)
}

// PowerShell's default stdout formatting adds a newline and may use a lossy
// code page. Exchange base64-encoded UTF-8 via raw stdin/stdout instead.
#[cfg(windows)]
fn powershell(script: &str) -> Command {
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-STA", "-Command", script]);
    cmd.stderr(Stdio::null());
    cmd
}

#[cfg(windows)]
fn windows_get() -> Option<String> {
    let out = powershell("$ErrorActionPreference='Stop'; $s=Get-Clipboard -Raw; [Console]::Write([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes([string]$s)))")
        .output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(base64::engine::general_purpose::STANDARD.decode(out.stdout).ok()?).ok()
}

impl Clipboard {
    pub fn set(&mut self, text: String, whole_line: bool) {
        self.whole_line = whole_line;
        // OSC 52: ask the terminal to set the system clipboard.
        let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let mut out = std::io::stdout();
        let _ = write!(out, "\x1b]52;c;{b64}\x07");
        let _ = out.flush();
        #[cfg(windows)]
        if let Ok(mut child) = powershell("$ErrorActionPreference='Stop'; $s=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String([Console]::In.ReadToEnd())); Set-Clipboard -Value $s")
            .stdin(Stdio::piped()).stdout(Stdio::null()).spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(b64.as_bytes());
            }
            let _ = child.wait();
        }
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
                    if child.wait().is_ok_and(|status| status.success()) {
                        break;
                    }
                }
            }
        }
        self.text = text;
    }

    /// Returns the clipboard text, preferring the system clipboard when it
    /// holds something different from what we last copied.
    pub fn get(&mut self) -> String {
        #[cfg(windows)]
        if let Some(sys) = windows_get() {
            self.update_from_system(&sys);
            return self.text.clone();
        }
        if graphical_session() {
            // Retry the last successful provider first. This avoids a series
            // of process launches on every paste on machines with only one
            // clipboard utility installed.
            if let Some(i) = self.paste_tool {
                let (cmd, args) = PASTE_TOOLS[i];
                match Command::new(cmd).args(args).stderr(Stdio::null()).output() {
                    Ok(out) if out.status.success() => {
                        self.update_from_system(&String::from_utf8_lossy(&out.stdout));
                        return self.text.clone();
                    }
                    _ => self.paste_tool = None,
                }
            }
            for (i, (cmd, args)) in PASTE_TOOLS.iter().enumerate() {
                if let Ok(out) = Command::new(cmd).args(*args).stderr(Stdio::null()).output()
                    && out.status.success()
                {
                    self.paste_tool = Some(i);
                    self.update_from_system(&String::from_utf8_lossy(&out.stdout));
                    break;
                }
            }
        }
        self.text.clone()
    }
    fn update_from_system(&mut self, text: &str) {
        let text = paste_text(text);
        if text != paste_text(&self.text) {
            self.text = text.into_owned();
            self.whole_line = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_preserves_whitespace_and_unicode_but_not_control_bytes() {
        assert_eq!(paste_text("  α  \r\n\tβ\r\n\0"), "  α  \n\tβ\n");
    }

    #[test]
    fn changing_only_final_newlines_invalidates_whole_line_metadata() {
        let mut c = Clipboard {
            text: "a\n".into(),
            whole_line: true,
            ..Clipboard::default()
        };
        c.update_from_system("a\r\n");
        assert!(c.whole_line);
        c.update_from_system("a");
        assert!(!c.whole_line);
        assert_eq!(c.text, "a");
    }
}
