//! A rate-limited free model stays silent; termed switches to another free
//! model and resends the prompt by itself.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termed::agent::{Entry, State};
use termed::app::App;
use termed::commands::Command;

#[test]
fn stalled_free_model_fails_over_to_another_free_model() {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_acp_agent.py");
    // SAFETY: single test in this binary; no concurrent environment access.
    unsafe {
        std::env::set_var("TERMED_AGENT", format!("python3 {}", fixture.display()));
        std::env::set_var("FAKE_STALL", "opencode/big-pickle");
    }
    let root = std::env::temp_dir().join(format!("termed-failover-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("calc.py"), "def add(a, b):\n    return a - b\n").unwrap();
    let mut app = App::new(
        root.clone(),
        vec![root.join("calc.py")],
        "Visual Studio Dark",
        false,
    );
    app.agent.stall_after = Duration::from_millis(400);
    app.run(Command::AgentToggle);
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
    loop {
        app.pump_agent();
        if let Some(i) = app.agent.pending_permission() {
            app.agent.choose(i, Some("once"));
        }
        if app.agent.state == State::Ready && app.agent.usage.is_some() {
            break;
        }
        assert!(
            t.elapsed() < Duration::from_secs(15),
            "no failover: {:?}",
            app.agent.entries
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        app.agent.model.as_deref(),
        Some("opencode/step-5-free"),
        "skipped the paid model"
    );
    assert!(
        app.agent
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Notice { text, .. } if text.contains("Switched to")))
    );
    assert!(
        app.agent
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Agent(t) if t.contains("Fixed it")))
    );
    assert_eq!(
        std::fs::read_to_string(root.join("calc.py")).unwrap(),
        "def add(a, b):\n    return a + b\n"
    );
    app.agent.stop();
}
