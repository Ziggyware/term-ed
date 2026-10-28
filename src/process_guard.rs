//! Best-effort lifecycle cleanup for trusted local executables, not a sandbox.
//! Unix children get their own process group so cancellation also stops ordinary
//! shell descendants. A deliberately daemonizing program can escape that group.
use std::process::{Child, Command};

pub fn configure(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.env_remove(crate::credentials::ROUTER_ENV);
}

pub fn kill_group(child: &Child) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // SAFETY: kill accepts any integer PID; a negative PID addresses the
        // private group created above. SIGKILL=9 is specified by POSIX.
        if let Ok(pid) = i32::try_from(child.id()) {
            unsafe {
                kill(-pid, 9);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = child;
}

pub fn terminate(child: &mut Child) {
    kill_group(child);
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    #[test]
    fn stopping_a_managed_shell_also_stops_ordinary_background_descendants() {
        let marker =
            std::env::temp_dir().join(format!("termed-process-group-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "(sleep 1; echo escaped > \"$TERMED_GROUP_MARKER\") & printf '%s\\n' $!; wait",
            ])
            .env("TERMED_GROUP_MARKER", &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure(&mut command);
        let mut child = command.spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.trim().parse::<u32>().is_ok());
        terminate(&mut child);
        std::thread::sleep(std::time::Duration::from_millis(1200));
        assert!(
            !marker.exists(),
            "A normal shell descendant escaped cancellation"
        );
    }
}
