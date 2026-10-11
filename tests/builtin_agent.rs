//! The built-in agent end to end, against a local mock of the free gateway:
//! streaming, a tool call that edits a file, and failover after HTTP 429.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termed::agent::{Entry, State};
use termed::app::App;
use termed::commands::Command as Cmd;

#[test]
fn builtin_agent_edits_files_and_fails_over_on_rate_limit() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_free_gateway.py");
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
    // SAFETY: single test in this binary; no concurrent environment access.
    unsafe {
        std::env::set_var("TERMED_AGENT", "builtin");
        std::env::set_var(
            "TERMED_FREE_GATEWAY",
            format!("http://127.0.0.1:{}", port.trim()),
        );
        std::env::remove_var("TERMED_FREE_MODEL");
    }
    let root = std::env::temp_dir().join(format!("termed-builtin-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let calc = root.join("calc.py");
    std::fs::write(&calc, "def add(a, b):\n    return a - b\n").unwrap();

    let mut app = App::new(
        root.clone(),
        vec![calc.clone()],
        "Visual Studio Dark",
        false,
    );
    assert_eq!(app.agent.name(), "termed AI");
    app.run(Cmd::AgentToggle);
    for c in "fix add".chars() {
        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    let t = Instant::now();
    while !(app.agent.state == State::Ready && app.agent.usage.is_some()) {
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "agent did not finish: {:?}",
            app.agent.entries
        );
        app.pump_agent();
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = server.kill();

    assert_eq!(
        std::fs::read_to_string(&calc).unwrap(),
        "def add(a, b):\n    return a + b\n"
    );
    assert_eq!(
        app.editor().unwrap().doc.rope.to_string(),
        "def add(a, b):\n    return a + b\n"
    );
    let e = &app.agent.entries;
    assert!(
        e.iter()
            .any(|x| matches!(x, Entry::Notice { text, .. } if text.contains("busy"))),
        "429 failover noted: {e:?}"
    );
    assert!(
        e.iter()
            .any(|x| matches!(x, Entry::Agent(t) if t.contains("now adds.")))
    );
    assert!(e.iter().any(
        |x| matches!(x, Entry::Tool(tc) if tc.title == "Edit calc.py" && !tc.diffs.is_empty())
    ));
    assert_eq!(
        app.agent.model.as_deref(),
        Some("test/good:free"),
        "paid model never used"
    );
    app.agent.stop();
}
