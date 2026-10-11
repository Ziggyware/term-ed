//! Keyed providers end to end against local mocks: an OpenAI-compatible
//! server and Anthropic's native Messages API (thinking echo, eager tool
//! input validation, one user message of tool results), plus the key store.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termed::agent::{Entry, State};
use termed::app::App;
use termed::commands::Command as Cmd;
use termed::providers;

fn ask(root: &Path, file: &Path, prompt: &str) -> App {
    let mut app = App::new(
        root.to_path_buf(),
        vec![file.to_path_buf()],
        "Visual Studio Dark",
        false,
    );
    app.run(Cmd::AgentToggle);
    for c in prompt.chars() {
        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    app
}

fn wait(app: &mut App, done: impl Fn(&App) -> bool) {
    let t = Instant::now();
    while !done(app) {
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "agent stuck: {:?} {:?}",
            app.agent.state,
            app.agent.entries
        );
        app.pump_agent();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn said(app: &App, needle: &str) -> bool {
    app.agent.entries.iter().any(|e| match e {
        Entry::Agent(t) => t.contains(needle),
        Entry::Notice { text, .. } => text.contains(needle),
        _ => false,
    })
}

#[test]
fn keyed_providers_work_end_to_end() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_providers.py");
    let Ok(mut server) = Command::new("python3")
        .arg(&fixture)
        .stdout(Stdio::piped())
        .spawn()
    else {
        return; // no python3
    };
    let mut port = String::new();
    BufReader::new(server.stdout.take().unwrap())
        .read_line(&mut port)
        .unwrap();
    let base = format!("http://127.0.0.1:{}", port.trim());
    let root = std::env::temp_dir().join(format!("termed-providers-{}", std::process::id()));
    let config = root.join("config");
    std::fs::create_dir_all(&root).unwrap();
    let calc = root.join("calc.py");
    // SAFETY: the only test in this binary; no concurrent environment access.
    unsafe {
        std::env::set_var("TERMED_CONFIG_DIR", &config);
        std::env::set_var("TERMED_ANTHROPIC_BASE_URL", format!("{base}/anthropic"));
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::set_var("TERMED_AGENT", "builtin:anthropic");
    }

    // 1. No key yet: the agent says how to get one instead of failing silently.
    std::fs::write(&calc, "def add(a, b):\n    return a - b\n").unwrap();
    let mut app = ask(&root, &calc, "fix add");
    wait(&mut app, |a| said(a, "needs an API key"));
    app.agent.stop();

    // 2. Saved key (owner-only file) → native Messages API turn with tools.
    providers::save_key("anthropic", "sk-test-123").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(config.join("config.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let mut app = ask(&root, &calc, "fix add");
    wait(&mut app, |a| {
        a.agent.state == State::Ready && a.agent.usage.is_some()
    });
    assert!(said(&app, "Fixed via Claude."), "{:?}", app.agent.entries);
    assert_eq!(
        std::fs::read_to_string(&calc).unwrap(),
        "def add(a, b):\n    return a + b\n"
    );
    assert!(
        !root.join("x.py").exists(),
        "a tool call with broken JSON must not run"
    );
    assert_eq!(app.agent.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(
        app.agent.usage.unwrap().2,
        None,
        "paid usage is not shown as free"
    );
    app.agent.stop();

    // 3. Any OpenAI-compatible server with a key.
    std::fs::write(&calc, "def add(a, b):\n    return a - b\n").unwrap();
    unsafe {
        std::env::set_var("TERMED_AGENT", "builtin:custom");
        std::env::set_var("TERMED_OPENAI_BASE_URL", format!("{base}/v1"));
        std::env::set_var("TERMED_OPENAI_API_KEY", "sk-test-123");
    }
    let mut app = ask(&root, &calc, "fix add");
    wait(&mut app, |a| {
        a.agent.state == State::Ready && a.agent.usage.is_some()
    });
    assert!(
        said(&app, "Fixed via OpenAI-compatible."),
        "{:?}",
        app.agent.entries
    );
    assert_eq!(
        app.agent.model.as_deref(),
        Some("local-coder"),
        "embedding models are skipped"
    );
    assert_eq!(
        std::fs::read_to_string(&calc).unwrap(),
        "def add(a, b):\n    return a + b\n"
    );
    app.agent.stop();

    // 4. A wrong key is explained.
    std::fs::write(&calc, "x\n").unwrap();
    unsafe { std::env::set_var("TERMED_OPENAI_API_KEY", "wrong") };
    let mut app = ask(&root, &calc, "hi");
    wait(&mut app, |a| {
        said(a, "rejected the API key") || said(a, "refused")
    });
    app.agent.stop();
    let _ = server.kill();
}
