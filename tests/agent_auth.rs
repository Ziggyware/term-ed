//! An agent that requires sign-in: the panel shows its auth methods as
//! buttons, and choosing one authenticates and opens the session.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use termed::agent::State;
use termed::app::{AgentTarget, App};
use termed::commands::Command;

#[test]
fn sign_in_buttons_authenticate_the_agent() {
    if std::process::Command::new("python3")
        .arg("-V")
        .output()
        .is_err()
    {
        return;
    }
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_acp_agent.py");
    let root = std::env::temp_dir().join(format!("termed-auth-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    // SAFETY: the only test in this binary.
    unsafe {
        std::env::set_var("TERMED_AGENT", format!("python3 {}", fixture.display()));
        std::env::set_var("FAKE_AUTH", "1");
        std::env::set_var("TERMED_CONFIG_DIR", root.join("config"));
    }
    let mut app = App::new(root.clone(), vec![], "Visual Studio Dark", false);
    app.run(Command::AgentToggle);
    app.agent.start();
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let t = Instant::now();
    while !app.agent.needs_auth {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "{:?}",
            app.agent.state
        );
        app.pump_agent();
        std::thread::sleep(Duration::from_millis(20));
    }
    term.draw(|f| termed::ui::draw(f, &mut app, true)).unwrap();
    let buf = term.backend().buffer();
    let text: String = buf.content().iter().map(|c| c.symbol()).collect();
    assert!(text.contains("Log in with Test"), "sign-in button drawn");
    let (rect, _) = app
        .layout
        .agent_targets
        .iter()
        .find(|(_, t)| *t == AgentTarget::Auth(0))
        .cloned()
        .expect("auth button target");
    assert!(rect.width > 0);
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_event(Event::Mouse(MouseEvent {
            kind,
            column: rect.x + 1,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        }));
    }
    while app.agent.state != State::Ready {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "{:?}",
            app.agent.state
        );
        app.pump_agent();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!app.agent.needs_auth);
    app.agent.stop();
}
