//! Shell selection, plus a deterministic single-pass command lexer,
//! workspace-escape verifier, and native-tool redirector.
//!
//! The built-in agent hands every `run_command` to a real shell. POSIX systems
//! always ship `/bin/sh`, but Windows has none unless Git for Windows, MSYS2,
//! Cygwin or WSL is installed — so a hardcoded `sh -c` fails there with
//! "cannot run: program not found" before the user's command is ever started.
//! This module picks a shell the machine actually has, and lets the user
//! override the choice with `TERMED_SHELL`.
//!
//! Windows itself ships `bash.exe`, `wsl.exe` and `lxrun.exe` launcher stubs in
//! `C:\Windows\System32`. They are not shells: they relay the exec to WSL, and
//! when no distro answers they print `<3>WSL (...) ERROR: CreateProcessCommon:...`
//! and fail — so auto-detection never picks them. An explicit `TERMED_SHELL`
//! (for example `wsl bash -c`) is still honored as-is.
//!
//! [`analyze_command`] eliminates wasteful and failure-prone subprocess spawns
//! when an LLM issues stateful navigation (`cd ..`) or POSIX file-inspection
//! commands (`ls`, `cat`, `grep`, `find`, `head`, `tail`, `pwd`) that are
//! natively served by termed's in-process tools.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A shell program plus the leading arguments that precede the command text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shell {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Pass the command text verbatim: cmd.exe re-parses its own command line,
    /// so Rust's MSVCRT-style argument quoting would corrupt it.
    pub raw: bool,
}

impl Shell {
    fn new(program: &str, args: &[&str], raw: bool) -> Self {
        Self {
            program: PathBuf::from(program),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            raw,
        }
    }

    /// Whether the program can be found on this machine. The WSL launcher
    /// stubs in the Windows system directory do not count as available.
    pub fn available(&self) -> bool {
        if self.program.is_absolute() {
            self.program.is_file() && !is_wsl_stub(&self.program)
        } else {
            resolve_shell(&self.program.to_string_lossy()).is_some()
        }
    }

    /// Display name used in diagnostics.
    pub fn label(&self) -> String {
        self.program.display().to_string()
    }

    /// A process that runs `text` through this shell.
    pub fn command(&self, text: &str) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        if self.raw {
            push_raw_arg(&mut cmd, text);
        } else {
            cmd.arg(text);
        }
        cmd
    }
}

#[cfg(windows)]
fn push_raw_arg(cmd: &mut Command, text: &str) {
    use std::os::windows::process::CommandExt;
    cmd.raw_arg(text);
}

#[cfg(not(windows))]
fn push_raw_arg(cmd: &mut Command, text: &str) {
    cmd.arg(text);
}

/// Classification of a shell command string prior to subprocess execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellAnalysis {
    /// Command is a no-op stateful navigation or duplicates a native editor tool.
    /// Returns a deterministic directive instructing the model which native tool to call.
    Redirect(String),
    /// Command attempts to escape the workspace root or execute a forbidden primitive.
    Block(String),
    /// Command is a legitimate build, test, VCS, or pipeline operation.
    Allow {
        /// Risk score in `0..=100` derived from static command structure.
        structural_risk: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sep {
    And,
    Or,
    Semi,
    Pipe,
    Background,
}

struct Segment<'a> {
    tokens: Vec<&'a str>,
    sep_after: Option<Sep>,
    has_redirect: bool,
}

/// Single-pass lexical splitter respecting single quotes, double quotes, and escapes.
fn lex_pipeline(input: &str) -> Result<Vec<Segment<'_>>, String> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut segments = Vec::new();
    let mut tokens = Vec::new();
    let mut has_redirect = false;
    let mut i = 0;

    while i < len {
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= len {
            break;
        }

        // Command separators and redirection operators outside quotes.
        let sep = match bytes[i] {
            b';' => {
                i += 1;
                Some(Sep::Semi)
            }
            b'|' if bytes.get(i + 1) == Some(&b'|') => {
                i += 2;
                Some(Sep::Or)
            }
            b'|' => {
                i += 1;
                Some(Sep::Pipe)
            }
            b'&' if bytes.get(i + 1) == Some(&b'&') => {
                i += 2;
                Some(Sep::And)
            }
            b'&' => {
                i += 1;
                Some(Sep::Background)
            }
            b'>' | b'<' => {
                has_redirect = true;
                i += 1;
                if i < len && (bytes[i] == b'>' || bytes[i] == b'&') {
                    i += 1;
                }
                continue;
            }
            _ => None,
        };
        if let Some(sep) = sep {
            segments.push(Segment {
                tokens: std::mem::take(&mut tokens),
                sep_after: Some(sep),
                has_redirect,
            });
            has_redirect = false;
            continue;
        }

        // Scan one token (handling quotes and backslash escapes).
        let start = i;
        let mut quote: Option<u8> = None;
        while i < len {
            let b = bytes[i];
            if let Some(q) = quote {
                if b == b'\\' && q == b'"' && i + 1 < len {
                    i += 2;
                    continue;
                }
                if b == q {
                    quote = None;
                }
                i += 1;
            } else {
                match b {
                    b'\'' | b'"' => {
                        quote = Some(b);
                        i += 1;
                    }
                    b'\\' if i + 1 < len => i += 2,
                    b if b.is_ascii_whitespace()
                        || matches!(b, b';' | b'|' | b'&' | b'>' | b'<') =>
                    {
                        break;
                    }
                    _ => i += 1,
                }
            }
        }
        if quote.is_some() {
            return Err(
                "Unterminated quote in shell command; fix quoting or use native tools.".into(),
            );
        }
        let raw = &input[start..i];
        let unquoted = raw
            .strip_prefix(['\'', '"'])
            .and_then(|s| s.strip_suffix(['\'', '"']))
            .unwrap_or(raw);
        if !unquoted.is_empty() {
            tokens.push(unquoted);
        }
    }

    if !tokens.is_empty() || has_redirect {
        segments.push(Segment {
            tokens,
            sep_after: None,
            has_redirect,
        });
    }

    Ok(segments)
}

/// Computes the net directory depth delta inside the workspace for a relative `cd` target.
/// Returns `None` if the path escapes the workspace root (`depth < 0`, absolute path, or `~`).
fn step_virtual_depth(mut depth: isize, target: &str) -> Option<isize> {
    if target.is_empty() || target == "~" || target.starts_with("~/") || target.starts_with("~\\") {
        return None;
    }
    if target.starts_with('/') || target.starts_with('\\') || target.contains(':') {
        return None;
    }
    for part in target.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            _ => depth += 1,
        }
    }
    Some(depth)
}

/// First match for `program` on the `PATH` (or `program` itself if it is a path).
fn resolve(program: &str) -> Option<PathBuf> {
    crate::plugin::path_candidates(program).into_iter().next()
}

/// Like [`resolve`], but for picking a shell: the WSL launcher stubs Windows
/// ships in its system directory are skipped, and the rest of the `PATH` is
/// searched, so `C:\Windows\System32\bash.exe` never shadows a real shell
/// installed further down the `PATH`.
fn resolve_shell(program: &str) -> Option<PathBuf> {
    crate::plugin::path_candidates(program)
        .into_iter()
        .find(|candidate| !is_wsl_stub(candidate))
}

/// Whether `path` is one of the WSL launcher stubs Windows ships directly in
/// its system directory. Those stubs (`bash.exe`, `wsl.exe`, `lxrun.exe`) are
/// not shells: they hand the exec to WSL, and when no distro answers they print
/// `<3>WSL (...) ERROR: CreateProcessCommon:...` and fail. From inside WSL the
/// same stubs are reachable as `/mnt/<drive>/Windows/System32/bash.exe`, so
/// that view is recognized too.
fn is_wsl_stub(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    #[cfg(windows)]
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    // From inside WSL the Windows system directory is mounted under /mnt/<drive>.
    #[cfg(not(windows))]
    let system_root = if raw.starts_with("/mnt/") {
        r"C:\Windows".to_string()
    } else {
        return false;
    };
    is_wsl_stub_path(&system_root, &raw)
}

/// Pure form of [`is_wsl_stub`], split out so it can be tested on any platform.
/// `system_root` is the Windows directory (e.g. `C:\Windows`); both paths are
/// compared case-insensitively with `/` and `\` treated as separators.
fn is_wsl_stub_path(system_root: &str, path: &str) -> bool {
    const STUBS: [&str; 3] = ["bash.exe", "wsl.exe", "lxrun.exe"];
    let normalize = |s: &str| s.to_ascii_lowercase().replace('/', "\\");
    let path = normalize(path);
    let root = normalize(system_root);
    let root = root.trim_end_matches('\\');
    let root_name = root.rsplit('\\').next().unwrap_or("");
    for dir in ["system32", "syswow64"] {
        for stub in STUBS {
            // Native view, e.g. C:\Windows\System32\bash.exe.
            if !root.is_empty() && path == format!("{root}\\{dir}\\{stub}") {
                return true;
            }
            // WSL mount view, e.g. /mnt/c/Windows/System32/bash.exe.
            let tail = format!("\\{root_name}\\{dir}\\{stub}");
            if !root_name.is_empty() && path.ends_with(tail.as_str()) {
                return true;
            }
        }
    }
    false
}

/// Evaluates a raw command string before any permission prompt or process spawn.
pub fn analyze_command(command: &str, cwd: &Path) -> ShellAnalysis {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return ShellAnalysis::Redirect(
            "Empty command ignored. Use `list_files`, `read_file`, or `search` to inspect the project."
                .into(),
        );
    }

    let segments = match lex_pipeline(trimmed) {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => {
            return ShellAnalysis::Redirect(
                "Empty command ignored. Use native tools (`list_files`, `read_file`, `search`)."
                    .into(),
            );
        }
        Err(msg) => return ShellAnalysis::Redirect(msg),
    };

    // Track virtual directory depth across chained `cd` commands.
    let mut depth: isize = 0;
    for (idx, seg) in segments.iter().enumerate() {
        let Some(&head) = seg.tokens.first() else {
            continue;
        };
        let prog = head.rsplit(['/', '\\']).next().unwrap_or(head);

        if matches!(prog, "cd" | "chdir" | "pushd" | "popd") {
            // A trailing `cd` has zero effect on future tool calls.
            if idx + 1 == segments.len() {
                let arg = seg.tokens.get(1).copied().unwrap_or("");
                if arg == ".." || arg.starts_with("../") || arg.starts_with("..\\") {
                    return ShellAnalysis::Redirect(format!(
                        "`{trimmed}` cannot change the agent's working directory or access parent folders outside the workspace root (`{}`). Every tool and command is rooted at `{}`. Call `list_files` with `{{\"path\": \".\"}}` to see all files in the workspace.",
                        cwd.display(),
                        cwd.display()
                    ));
                }
                return ShellAnalysis::Redirect(format!(
                    "`{trimmed}` has no persistent effect because each `run_command` executes in an isolated subshell rooted at `{}`. Pass subdirectory paths directly to `list_files`, `read_file`, `search`, or `edit_file` (or chain `cd <subdir> && <build_command>` in a single call).",
                    cwd.display()
                ));
            }

            let target = seg.tokens.get(1).copied().unwrap_or("~");
            if prog == "popd" || target == "-" {
                return ShellAnalysis::Redirect(format!(
                    "`{prog}` directory stack navigation is unsupported in stateless subshells. Use explicit relative paths from `{}`.",
                    cwd.display()
                ));
            }
            match step_virtual_depth(depth, target) {
                // The target is fully accounted for; re-checking it below
                // against the already-updated depth would count `..` twice.
                Some(next_depth) => {
                    depth = next_depth;
                    continue;
                }
                None => {
                    return ShellAnalysis::Block(format!(
                        "Directory navigation `cd {target}` escapes the workspace root (`{}`). All commands must remain inside the project root.",
                        cwd.display()
                    ));
                }
            }
        }

        // Reject `../` traversal in any argument that would leave the workspace.
        for &arg in seg.tokens.iter().skip(1) {
            let clean = arg.trim_start_matches(|c: char| c == '-' || c.is_ascii_alphanumeric() || c == '=');
            for candidate in [arg, clean] {
                if (candidate == ".."
                    || candidate.starts_with("../")
                    || candidate.starts_with("..\\"))
                    && step_virtual_depth(depth, candidate).is_none()
                {
                    return ShellAnalysis::Block(format!(
                        "Argument `{arg}` references a parent directory outside the workspace root (`{}`). Stay within the workspace.",
                        cwd.display()
                    ));
                }
            }
        }
    }

    // Intercept single-segment unredirected commands that duplicate fast native tools.
    if segments.len() == 1 && !segments[0].has_redirect && segments[0].sep_after.is_none() {
        let tokens = &segments[0].tokens;
        if let Some(&head) = tokens.first() {
            let prog = head.rsplit(['/', '\\']).next().unwrap_or(head);
            let first_operand = |fallback| {
                tokens
                    .iter()
                    .skip(1)
                    .find(|t| !t.starts_with('-'))
                    .copied()
                    .unwrap_or(fallback)
            };
            match prog {
                "pwd" => {
                    return ShellAnalysis::Redirect(format!(
                        "Current working directory is always `{}`. Use relative paths with `list_files`, `read_file`, and `search`.",
                        cwd.display()
                    ));
                }
                "ls" | "dir" | "tree" => {
                    let target = first_operand(".");
                    return ShellAnalysis::Redirect(format!(
                        "Use the native `list_files` tool (`{{\"path\": \"{target}\"}}`) instead of running `{prog}` in a shell. `list_files` is instant and does not require command approval."
                    ));
                }
                "find" | "fd" => {
                    return ShellAnalysis::Redirect(
                        "Use the native `list_files` tool to discover file paths or `search` to match content instead of spawning `find`."
                            .into(),
                    );
                }
                "cat" | "head" | "tail" | "type" | "less" | "more" => {
                    let file = first_operand("<path>");
                    return ShellAnalysis::Redirect(format!(
                        "Use the native `read_file` tool (`{{\"path\": \"{file}\"}}` with optional `start_line`/`end_line`) or `read_editor` instead of `{prog}`."
                    ));
                }
                "grep" | "rg" | "egrep" | "fgrep" | "ag" | "ack" | "findstr" => {
                    return ShellAnalysis::Redirect(
                        "Use the native `search` tool (`{\"pattern\": \"...\", \"path\": \".\"}`) instead of spawning a shell grep."
                            .into(),
                    );
                }
                "sed" | "awk" | "perl" if tokens.iter().any(|t| t.starts_with("-i")) => {
                    return ShellAnalysis::Redirect(
                        "Do not mutate files with in-place shell stream editors (`sed -i` / `perl -i`). Use `read_file` followed by `edit_file` so edits are tracked by AEP and undoable in the editor."
                            .into(),
                    );
                }
                _ => {}
            }
        }
    }

    ShellAnalysis::Allow {
        structural_risk: 10,
    }
}

/// Git for Windows installs that are commonly absent from `PATH`.
fn git_bash_shells() -> Vec<Shell> {
    let roots = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(|var| std::env::var_os(var).map(|v| PathBuf::from(v).join("Git")))
        .chain(
            std::env::var_os("LOCALAPPDATA")
                .map(|v| PathBuf::from(v).join("Programs").join("Git")),
        );
    let mut out: Vec<Shell> = Vec::new();
    for root in roots {
        for path in [root.join("bin").join("bash.exe"), root.join("usr").join("bin").join("bash.exe")] {
            let shell = Shell::new(&path.to_string_lossy(), &["-c"], false);
            if !out.contains(&shell) {
                out.push(shell);
            }
        }
    }
    out
}

/// Shells tried in order on this platform.
fn candidates() -> Vec<Shell> {
    let mut out = Vec::new();
    if cfg!(windows) {
        // Git for Windows, MSYS2, Cygwin and WSL all provide `sh`, which runs
        // the POSIX command lines the agent is trained to write. Only fall
        // back to the native shells when none of them is installed. The WSL
        // launcher stubs in the system directory (`bash.exe`, `wsl.exe`,
        // `lxrun.exe`) are filtered out by `available`, so `bash` here can
        // never resolve to `C:\Windows\System32\bash.exe`.
        out.push(Shell::new("sh", &["-c"], false));
        out.push(Shell::new("bash", &["-c"], false));
        out.extend(git_bash_shells());
        out.push(Shell::new("cmd", &["/C"], true));
        out.push(Shell::new(
            "pwsh",
            &["-NoProfile", "-NonInteractive", "-Command"],
            false,
        ));
        out.push(Shell::new(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command"],
            false,
        ));
    } else {
        out.push(Shell::new("/bin/sh", &["-c"], false));
        out.push(Shell::new("sh", &["-c"], false));
        out.push(Shell::new("bash", &["-c"], false));
        out.push(Shell::new("dash", &["-c"], false));
        out.push(Shell::new("zsh", &["-c"], false));
    }
    out
}

/// The shell to use, or `None` when this machine has none of the candidates.
///
/// `TERMED_SHELL` overrides detection; it holds the program followed by any
/// leading arguments, and the command text is appended as one final argument:
/// `TERMED_SHELL="bash -c"` or `TERMED_SHELL="wsl bash -c"`. A WSL launcher
/// stub is only used when explicitly asked for and nothing real matches.
pub fn detect() -> Option<Shell> {
    if let Some(spec) = std::env::var_os("TERMED_SHELL") {
        let spec = spec.to_string_lossy();
        let parts = crate::plugin::split_command(&spec);
        if let Some((program, args)) = parts.split_first() {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            // Prefer a real shell over the WSL launcher stubs in the system
            // directory, but still honor an explicit stub such as `wsl`.
            if let Some(path) = resolve_shell(program).or_else(|| resolve(program)) {
                // cmd.exe re-parses its own command line, so the command text
                // must not be quoted for it.
                let raw = path
                    .file_stem()
                    .is_some_and(|stem| stem.eq_ignore_ascii_case("cmd"));
                let mut shell = Shell::new(program, &args, raw);
                shell.program = path;
                return Some(shell);
            }
        }
    }
    candidates().into_iter().find(Shell::available)
}

/// Explain a failed detection with something the user can act on.
pub fn missing_message() -> String {
    let tried = candidates()
        .iter()
        .map(Shell::label)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "No shell found on this system (tried {tried}), so shell commands cannot run. \
         Install one (on Windows: Git for Windows, which provides sh.exe) or set \
         TERMED_SHELL to the program and arguments to use, for example \
         TERMED_SHELL=\"bash -c\"."
    )
}

/// A guarded process that runs `text` through the detected shell in `cwd`:
/// own process group (so cancellation reaps descendants) and router key stripped.
pub fn build_shell_command(text: &str, cwd: &Path) -> Result<Command, String> {
    let shell = detect().ok_or_else(missing_message)?;
    let mut cmd = shell.command(text);
    cmd.current_dir(cwd);
    crate::process_guard::configure(&mut cmd);
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyzes_stateful_cd_and_workspace_escapes() {
        let root = Path::new("/workspace/app");
        assert!(matches!(
            analyze_command("cd ..", root),
            ShellAnalysis::Redirect(msg) if msg.contains("list_files")
        ));
        assert!(matches!(
            analyze_command("cd src", root),
            ShellAnalysis::Redirect(msg) if msg.contains("isolated subshell")
        ));
        assert!(matches!(
            analyze_command("cd .. && cargo test", root),
            ShellAnalysis::Block(msg) if msg.contains("escapes the workspace root")
        ));
        assert!(matches!(
            analyze_command("cd src && cd ../.. && ls", root),
            ShellAnalysis::Block(msg) if msg.contains("escapes the workspace root")
        ));
        assert!(matches!(
            analyze_command("cat ../secret.txt", root),
            ShellAnalysis::Block(msg) if msg.contains("outside the workspace root")
        ));
        // Subdirectory cd followed by build command inside workspace is allowed.
        assert!(matches!(
            analyze_command("cd crates/sub && cargo check", root),
            ShellAnalysis::Allow { .. }
        ));
        // Going into a subdir of depth 2 and stepping back 1 stays inside workspace.
        assert!(matches!(
            analyze_command("cd crates/sub && cd .. && cargo check", root),
            ShellAnalysis::Allow { .. }
        ));
        // Returning to the root via `cd ..` is not double-counted as an escape.
        assert!(matches!(
            analyze_command("cd src && cd .. && cargo check", root),
            ShellAnalysis::Allow { .. }
        ));
    }

    #[test]
    fn redirects_unpiped_file_inspection_commands_to_native_tools() {
        let root = Path::new("/workspace/app");
        assert!(matches!(
            analyze_command("ls -la src", root),
            ShellAnalysis::Redirect(msg) if msg.contains("\"path\": \"src\"")
        ));
        assert!(matches!(
            analyze_command("cat Cargo.toml", root),
            ShellAnalysis::Redirect(msg) if msg.contains("Cargo.toml")
        ));
        assert!(matches!(
            analyze_command("rg -n 'fn main' src/", root),
            ShellAnalysis::Redirect(msg) if msg.contains("native `search` tool")
        ));
        assert!(matches!(
            analyze_command("sed -i 's/foo/bar/g' src/lib.rs", root),
            ShellAnalysis::Redirect(msg) if msg.contains("edit_file")
        ));
        assert!(matches!(
            analyze_command("pwd", root),
            ShellAnalysis::Redirect(msg) if msg.contains("/workspace/app")
        ));
    }

    #[test]
    fn wsl_launcher_stubs_in_the_system_directory_are_not_shells() {
        let root = r"C:\Windows";
        // Native view of the stubs Windows ships in its system directory.
        assert!(is_wsl_stub_path(root, r"C:\WINDOWS\system32\bash.exe"));
        assert!(is_wsl_stub_path(root, r"C:\Windows\System32\wsl.exe"));
        assert!(is_wsl_stub_path(root, r"C:\Windows\SysWOW64\lxrun.exe"));
        // The same stubs as seen from inside WSL.
        assert!(is_wsl_stub_path(root, "/mnt/c/Windows/System32/bash.exe"));
        assert!(is_wsl_stub_path(root, "/mnt/C/WINDOWS/SYSTEM32/BASH.EXE"));
        // Real shells and unrelated programs are fine.
        assert!(!is_wsl_stub_path(root, r"C:\Windows\System32\cmd.exe"));
        assert!(!is_wsl_stub_path(
            root,
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
        ));
        assert!(!is_wsl_stub_path(
            root,
            r"C:\Program Files\Git\usr\bin\bash.exe"
        ));
        assert!(!is_wsl_stub_path(root, "/bin/bash"));
        assert!(!is_wsl_stub_path(root, "/usr/bin/sh"));
        // A custom Windows directory is honored via SystemRoot.
        assert!(is_wsl_stub_path(r"D:\Win", r"D:\Win\System32\bash.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn detection_never_picks_a_wsl_stub() {
        let stub = Shell::new(r"C:\Windows\System32\bash.exe", &["-c"], false);
        if stub.program.is_file() {
            assert!(
                !stub.available(),
                "the System32 bash.exe launcher stub must not count as a shell"
            );
        }
        if let Some(shell) = detect() {
            assert!(
                !is_wsl_stub(&shell.program),
                "detect() picked the WSL launcher stub {}",
                shell.program.display()
            );
        }
    }
}
