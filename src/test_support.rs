//! Isolate per-user preferences and credentials for each integration test.
//!
//! The application now persists settings outside the workspace, so changing
//! process-wide environment variables in parallel tests would race and could
//! modify the developer's real configuration. Re-exec each test with its own
//! configuration directory instead.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

pub fn isolate(test: &str) -> bool {
    if std::env::var("TERMED_ISOLATED_TEST").as_deref() == Ok(test) {
        return false;
    }
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let config = std::env::temp_dir().join(format!(
        "termed-test-user-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(config.join("home")).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("TERMED_ISOLATED_TEST", test)
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_CACHE_HOME", config.join("cache"))
        .env("HOME", config.join("home"))
        .env_remove("TERMED_AGENT")
        .env_remove("TERMED_FREE_GATEWAY")
        .env_remove("TERMED_FREE_MODEL")
        .env_remove("ROUTER_API_KEY")
        .env_remove("TERMED_JEV_API_KEY")
        .output()
        .expect("isolated test process starts");
    let _ = std::fs::remove_dir_all(config);
    assert!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("running 1 test")
            && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "isolated {test}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

/// Unit tests have a module-qualified name in libtest; integration tests pass
/// their top-level names directly to `isolate`.
#[allow(dead_code)]
pub fn isolate_unit(module: &str, test: &str) -> bool {
    let module = module
        .strip_prefix(concat!(env!("CARGO_CRATE_NAME"), "::"))
        .unwrap_or(module);
    isolate(&format!("{module}::{test}"))
}
