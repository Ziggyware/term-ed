//! End-to-end: the agent panel against a scripted ACP agent
//! (tests/fixtures/fake_acp_agent.py) through the real UI and protocol.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use termed::agent::State;
use termed::app::{AgentTarget, App, Focus};
use termed::commands::Command;

fn draw(term: &mut Terminal<TestBackend>, app: &mut App) {
    term.draw(|f| termed::ui::draw(f, app, true)).unwrap();
}

fn screen(term: &Terminal<TestBackend>) -> String {
    let buf = term.backend().buffer();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            s.push_str(buf[(x, y)].symbol());
        }
        s.push('\n');
    }
    s
}

fn wait_for(
    term: &mut Terminal<TestBackend>,
    app: &mut App,
    what: &str,
    done: impl Fn(&App) -> bool,
) {
    let t = Instant::now();
    while !done(app) {
        assert!(
            t.elapsed() < Duration::from_secs(15),
            "timed out waiting for {what}\n{}",
            screen(term)
        );
        app.pump_agent();
        draw(term, app);
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn tap(app: &mut App, x: u16, y: u16) {
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_event(Event::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }));
    }
}

#[test]
fn agent_edits_land_in_the_editor_as_undoable_changes() {
    if common::isolate("agent_edits_land_in_the_editor_as_undoable_changes") {
        return;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("python3 not available; skipping");
        return;
    }
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_acp_agent.py");
    // SAFETY: this test binary has a single test, so no other thread reads the environment.
    unsafe { std::env::set_var("TERMED_AGENT", format!("python3 {}", fixture.display())) };
    let root = std::env::temp_dir().join(format!("termed-agent-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let calc = root.join("calc.py");
    std::fs::write(&calc, "def add(a, b):\n    return a - b\n").unwrap();

    let mut app = App::new(
        root.clone(),
        vec![calc.clone()],
        "Visual Studio Dark",
        false,
    );
    let mut term = Terminal::new(TestBackend::new(120, 36)).unwrap();
    draw(&mut term, &mut app);

    app.run(Command::AgentToggle);
    assert_eq!(app.focus, Focus::Agent);
    draw(&mut term, &mut app);
    assert!(
        screen(&term).contains("fake_acp_agent"),
        "named after the script before it starts"
    );
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

    // The agent asks for permission: answer by tapping the button, like on a phone.
    wait_for(&mut term, &mut app, "permission prompt", |a| {
        a.agent.pending_permission().is_some()
    });
    assert!(screen(&term).contains("Allow: edit calc.py?"));
    let (r, _) = app
        .layout
        .agent_targets
        .iter()
        .find(|(_, t)| matches!(t, AgentTarget::Permission(_, id) if id == "once"))
        .cloned()
        .expect("allow button");
    tap(&mut app, r.x + 1, r.y);

    wait_for(&mut term, &mut app, "turn end", |a| {
        a.agent.state == State::Ready && a.agent.usage.is_some()
    });
    let s = screen(&term);
    assert!(s.contains("Fixed it"), "{s}");
    assert!(
        s.contains("+ ") && s.contains("return a + b"),
        "inline diff shown"
    );
    assert!(s.contains("free"), "free badge/usage shown");
    assert_eq!(app.agent.model_label().as_deref(), Some("First free"));

    // The edit is on disk and in the buffer, saved, with change bars...
    assert_eq!(
        std::fs::read_to_string(&calc).unwrap(),
        "def add(a, b):\n    return a + b\n"
    );
    let e = app.editor().unwrap();
    assert_eq!(e.doc.rope.to_string(), "def add(a, b):\n    return a + b\n");
    assert!(!e.doc.is_dirty());
    // ...and one Ctrl+Z reverts the agent's change.
    app.focus = Focus::Editor;
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('z'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(
        app.editor().unwrap().doc.rope.to_string(),
        "def add(a, b):\n    return a - b\n"
    );
    app.agent.stop();
}
