//! Terminal widgets for the dockable window system.
//!
//! Each terminal runs a shell session in a background thread (via
//! `std::process::Command`), captures its output, and renders it in a
//! bordered, focusable box. Terminals are the primary dockable windows.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use ratatui::layout::Rect;

use crate::neon::{Creature, CreatureKind};

/// A single terminal window: a shell session plus its display state.
#[derive(Debug)]
pub struct Terminal {
    pub id: u64,
    pub title: String,
    /// Screen region this terminal occupies.
    pub rect: Rect,
    /// Scrollback buffer (most recent lines last).
    pub lines: Vec<String>,
    /// Scroll offset into `lines` (0 = bottom/most recent).
    pub scroll: usize,
    /// Current input line being typed.
    pub input: String,
    pub input_cursor: usize,
    pub focused: bool,
    pub docked: bool,
    pub z: u32,
    pub shell: Option<ShellSession>,
    /// Last time the terminal received output or input (for creature mood).
    pub last_activity: Instant,
    /// The terminal's creature.
    pub creature: Creature,
    /// Maximum scrollback lines kept.
    max_lines: usize,
}

impl Terminal {
    pub fn new(id: u64, rect: Rect) -> Self {
        let shell = ShellSession::new().ok();
        let mut creature = Creature::new(CreatureKind::ShellSpirit);
        creature.position = (0.96, 0.9);
        Self {
            id,
            title: format!("terminal {}", id),
            rect,
            lines: Vec::new(),
            scroll: 0,
            input: String::new(),
            input_cursor: 0,
            focused: false,
            docked: false,
            z: 0,
            shell,
            last_activity: Instant::now(),
            creature,
            max_lines: 2000,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Append output text, splitting into lines.
    pub fn write(&mut self, text: &str) {
        self.last_activity = Instant::now();
        for line in text.split('\n') {
            // Hard-wrap long lines to the terminal's inner width.
            let inner_w = self.rect.width.saturating_sub(2) as usize;
            let wrapped = if inner_w > 0 && line.chars().count() > inner_w {
                wrap(line, inner_w)
            } else {
                vec![line.to_string()]
            };
            for w in wrapped {
                self.lines.push(w);
                if self.lines.len() > self.max_lines {
                    self.lines.remove(0);
                }
            }
        }
        // Auto-scroll to bottom on new output when not scrolled up.
        if self.scroll == 0 {
            self.scroll = 0;
        }
    }

    /// Run a one-off command (non-interactive) and show its output.
    pub fn run_command(&mut self, command: &str) {
        self.last_activity = Instant::now();
        self.write(&format!("$ {command}\n"));
        let output = if cfg!(windows) {
            Command::new("cmd")
                .args(["/C", command])
                .output()
        } else {
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .output()
        };
        match output {
            Ok(o) => {
                if !o.stdout.is_empty() {
                    self.write(&String::from_utf8_lossy(&o.stdout));
                }
                if !o.stderr.is_empty() {
                    self.write(&format!("[stderr] {}", String::from_utf8_lossy(&o.stderr)));
                }
                if !o.status.success() {
                    self.write(&format!(
                        "[exit code: {}\n",
                        o.status.code().unwrap_or(-1)
                    ));
                }
            }
            Err(e) => self.write(&format!("error running command: {e}\n")),
        }
        self.write("\n");
        self.prompt();
    }

    /// Print a shell prompt.
    pub fn prompt(&mut self) {
        self.write("> ");
    }

    /// Handle a typed character.
    pub fn type_char(&mut self, c: char) {
        self.last_activity = Instant::now();
        self.input.insert(self.input_cursor, c);
        self.input_cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor -= 1;
            self.input.remove(self.input_cursor);
            self.last_activity = Instant::now();
        }
    }

    pub fn delete_forward(&mut self) {
        if self.input_cursor < self.input.len() {
            self.input.remove(self.input_cursor);
            self.last_activity = Instant::now();
        }
    }

    pub fn move_left(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.input_cursor < self.input.len() {
            self.input_cursor += 1;
        }
    }

    /// Submit the current input line to the shell.
    pub fn submit_input(&mut self) {
        let line = std::mem::take(&mut self.input);
        self.input_cursor = 0;
        if line.trim().is_empty() {
            self.prompt();
            return;
        }
        self.last_activity = Instant::now();
        self.write(&format!("{line}\n"));
        if let Some(shell) = &mut self.shell {
            if shell.send_input(&line).is_err() {
                self.write("[shell session closed]\n> ");
                self.shell = ShellSession::new().ok();
            }
        } else {
            // No persistent shell: run the command once-off.
            self.run_command(&line);
        }
    }

    /// Poll the shell session for newly captured output lines.
    pub fn poll_output(&mut self) {
        if let Some(shell) = &mut self.shell {
            while let Ok(line) = shell.output_rx.try_recv() {
                self.write(&line);
            }
        }
    }

    /// Inner content area (inside the border).
    pub fn inner_rect(&self) -> Rect {
        let r = self.rect;
        Rect::new(
            r.x.saturating_add(1),
            r.y.saturating_add(1),
            r.width.saturating_sub(2),
            r.height.saturating_sub(2),
        )
    }

    /// Number of content rows available for lines + input.
    pub fn content_rows(&self) -> u16 {
        self.inner_rect().height.saturating_sub(1) // reserve input row
    }

    /// The slice of lines currently visible, oldest-first.
    pub fn visible_lines(&self) -> Vec<&str> {
        let rows = self.content_rows() as usize;
        let total = self.lines.len();
        if total == 0 {
            return vec![];
        }
        // scroll = how far above the bottom we are.
        let start = if scroll_clamp(self.scroll, total) >= total {
            0
        } else {
            total.saturating_sub(rows).saturating_add(scroll_clamp(self.scroll, total))
        };
        let end = total.min(start + rows);
        self.lines[start..end].iter().map(String::as_str).collect()
    }
}

fn scroll_clamp(scroll: usize, total: usize) -> usize {
    scroll.min(total.saturating_sub(1))
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new(0, Rect::default())
    }
}

/// Wrap `line` to at most `width` characters per visual row.
fn wrap(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in line.chars() {
        if cur.len() >= width {
            out.push(cur);
            cur = String::new();
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A persistent shell session running in a background thread.
///
/// The session thread owns the `Child` process. Output lines flow from the
/// session to the terminal via `output_rx`; input lines flow from the
/// terminal to the session via `input_tx`.
pub struct ShellSession {
    /// Join handle for the session thread (kept alive for the session's life).
    _thread: Option<thread::JoinHandle<()>>,
    /// Send input lines to the shell.
    pub input_tx: mpsc::Sender<String>,
    /// Receive captured output lines from the shell.
    pub output_rx: mpsc::Receiver<String>,
}

impl ShellSession {
    pub fn new() -> std::io::Result<Self> {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    "powershell".to_string()
                } else {
                    "sh".to_string()
                }
            });

        let (input_tx, input_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::channel();

        let thread = thread::spawn(move || {
            let mut child = match Command::new(&shell)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = output_tx.send(format!("[failed to start {shell}: {e}]\n"));
                    return;
                }
            };

            let stdout = match child.stdout.take() {
                Some(s) => s,
                None => return,
            };
            let mut stdin = match child.stdin.take() {
                Some(s) => s,
                None => return,
            };

            // Reader thread: forward shell output to the terminal.
            let out_tx = output_tx.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().flatten() {
                    let _ = out_tx.send(format!("{line}\n"));
                }
                let _ = out_tx.send("[session ended]\n".to_string());
            });

            // Writer loop: send typed input to the shell's stdin.
            while let Ok(cmd) = input_rx.recv() {
                if writeln!(stdin, "{cmd}").is_err() {
                    break;
                }
                let _ = stdin.flush();
            }
            // Reap the child when the writer loop exits.
            let _ = child.wait();
        });

        Ok(Self {
            _thread: Some(thread),
            input_tx,
            output_rx,
        })
    }

    pub fn send_input(&self, line: &str) -> std::io::Result<()> {
        self.input_tx.send(line.to_string()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "session closed")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_splits_and_wraps_lines() {
        let mut t = Terminal::new(1, Rect::new(0, 0, 10, 5));
        t.write("hello world this is a long line\n");
        assert!(!t.lines.is_empty());
        assert!(t.lines.iter().all(|l| l.len() <= 8));
    }

    #[test]
    fn submit_input_clears_line() {
        let mut t = Terminal::new(2, Rect::new(0, 0, 20, 5));
        t.type_char('e');
        t.type_char('c');
        t.type_char('h');
        t.type_char('o');
        assert_eq!(t.input, "echo");
        t.submit_input();
        assert_eq!(t.input, "");
        assert_eq!(t.input_cursor, 0);
    }
}
