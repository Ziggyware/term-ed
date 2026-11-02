//! Host shell selection for agent-run commands.
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

use std::path::{Path, PathBuf};
use std::process::Command;

/// A shell program plus the arguments that make it evaluate one command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shell {
    pub program: PathBuf,
    /// Arguments placed before the command text, e.g. `-c` or `/C`.
    pub prefix: Vec<String>,
    /// Pass the command text verbatim instead of as a quoted argument.
    /// `cmd.exe` re-parses its own command line, so standard quoting breaks
    /// `&&`, `|` and redirections.
    pub raw: bool,
}

impl Shell {
    fn new(program: &str, prefix: &[&str], raw: bool) -> Self {
        Self {
            program: PathBuf::from(program),
            prefix: prefix.iter().map(|arg| (*arg).to_string()).collect(),
            raw,
        }
    }

    /// A `Command` that runs `command` through this shell.
    pub fn command(&self, command: &str) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix);
        #[cfg(windows)]
        {
            if self.raw {
                use std::os::windows::process::CommandExt;
                cmd.raw_arg(command);
                return cmd;
            }
        }
        cmd.arg(command);
        cmd
    }

    /// Whether the program can be found on this machine. The WSL launcher
    /// stubs in the Windows system directory do not count as available.
    pub fn available(&self) -> bool {
        resolve_shell(&self.program.to_string_lossy()).is_some()
    }

    /// A short description for messages, e.g. `/bin/sh -c`.
    pub fn label(&self) -> String {
        let mut parts = vec![self.program.display().to_string()];
        parts.extend(self.prefix.iter().cloned());
        parts.join(" ")
    }
}

/// Resolve a program the way the operating system would: an explicit path must
/// exist, a bare name is looked up on `PATH` (with Windows extensions).
fn resolve(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() || path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    crate::plugin::which_program(program)
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

/// Git for Windows installs `sh.exe` in its own directory and only adds
/// `cmd` to `PATH`, so a standard Git installation is invisible to a `PATH`
/// lookup even though it can run the agent's commands.
fn git_bash_shells() -> Vec<Shell> {
    let program = if cfg!(windows) { "sh.exe" } else { "sh" };
    let mut out: Vec<Shell> = Vec::new();
    for var in [
        "ProgramFiles",
        "ProgramW6432",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
    ] {
        let Some(base) = std::env::var_os(var) else {
            continue;
        };
        let base = PathBuf::from(base);
        let base = if var == "LOCALAPPDATA" {
            base.join("Programs")
        } else {
            base
        };
        for path in [
            base.join("Git").join("bin").join(program),
            base.join("Git").join("usr").join("bin").join(program),
        ] {
            if path.is_file() && !out.iter().any(|shell| shell.program == path) {
                out.push(Shell {
                    program: path,
                    prefix: vec!["-c".to_string()],
                    raw: false,
                });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detected_shell_runs_a_command() {
        let Some(shell) = detect() else {
            panic!("{}", missing_message());
        };
        let output = shell.command("echo termed-shell-ok").output().unwrap();
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("termed-shell-ok"),
            "{} did not run the command: {:?}",
            shell.label(),
            output
        );
    }

    #[test]
    fn the_shell_reports_exit_codes() {
        let shell = detect().unwrap();
        let status = shell.command("exit 3").status().unwrap();
        assert_eq!(status.code(), Some(3));
    }

    #[test]
    fn candidate_shells_carry_their_prefix_arguments() {
        let shell = Shell::new("sh", &["-c"], false);
        assert_eq!(shell.prefix, vec!["-c".to_string()]);
        assert_eq!(shell.label(), "sh -c");
        assert!(!shell.raw);
    }

    #[test]
    fn missing_message_names_the_override() {
        assert!(missing_message().contains("TERMED_SHELL"));
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
