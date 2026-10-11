//! Phone-screen behavior: compact layout, soft wrap, touch toolbar and taps,
//! driven through the real renderer on a headless backend.

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use termed::app::{App, Focus, Mod, Tool};

fn project() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("termed-mobile-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let long = "    let total = order.lines.iter().map(|l| l.price * l.quantity).sum::<f64>();\n";
    std::fs::write(dir.join("src/main.rs"), format!("fn main() {{\n{long}}}\n")).unwrap();
    dir
}

fn draw(term: &mut Terminal<TestBackend>, app: &mut App) {
    term.draw(|f| termed::ui::draw(f, app, true)).unwrap();
}

fn row(term: &Terminal<TestBackend>, y: u16) -> String {
    let buf = term.backend().buffer();
    (0..buf.area.width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect()
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

fn find(term: &Terminal<TestBackend>, needle: &str) -> Option<(u16, u16)> {
    let h = term.backend().buffer().area.height;
    (0..h).find_map(|y| {
        let r = row(term, y);
        r.find(needle).map(|b| (r[..b].chars().count() as u16, y))
    })
}

#[test]
fn phone_portrait_uses_compact_layout_with_wrap_and_toolbar() {
    let root = project();
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/main.rs")],
        "Visual Studio Dark",
        false,
    );
    let mut term = Terminal::new(TestBackend::new(44, 24)).unwrap();
    draw(&mut term, &mut app);

    assert!(app.compact);
    assert!(app.word_wrap());
    assert!(
        !app.show_sidebar,
        "drawer starts closed when a file is open"
    );
    // The long line wraps instead of scrolling sideways: its tail is visible.
    assert!(
        find(&term, "sum::<f64>();").is_some(),
        "wrapped tail should be on screen"
    );
    // The two-row touch keyboard sits above the status bar.
    assert!(row(&term, 21).contains("Esc"));
    assert!(row(&term, 22).contains("↓"));

    // ☰ opens the explorer drawer; tapping a file opens it and closes the drawer.
    tap(&mut app, 1, 0);
    draw(&mut term, &mut app);
    assert!(app.show_sidebar);
    let (x, y) = find(&term, "src").unwrap();
    tap(&mut app, x, y);
    draw(&mut term, &mut app);
    let (x, y) = find(&term, "main.rs").unwrap();
    tap(&mut app, x, y);
    draw(&mut term, &mut app);
    assert!(!app.show_sidebar);
    assert_eq!(app.focus, Focus::Editor);
}

/// Tap a touch-keyboard key by what it does (labels shorten on narrow screens).
fn tap_tool(app: &mut App, term: &mut Terminal<TestBackend>, tool: Tool) {
    draw(term, app);
    let (r, _) = *app
        .layout
        .toolbar
        .iter()
        .find(|(_, t)| *t == tool)
        .unwrap_or_else(|| panic!("no toolbar key {tool:?}"));
    tap(app, r.x + r.width / 2, r.y);
    draw(term, app);
}

fn sel_len(app: &App) -> usize {
    let e = app.editor().unwrap();
    e.sel.end() - e.sel.start()
}

#[test]
fn sticky_modifiers_latch_for_one_key_or_lock() {
    use termed::app::Latch;
    let root = project();
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/main.rs")],
        "Visual Studio Dark",
        false,
    );
    let mut term = Terminal::new(TestBackend::new(44, 24)).unwrap();

    // One tap on Shift: applies to the next key only.
    tap_tool(&mut app, &mut term, Tool::Mod(Mod::Shift));
    assert_eq!(app.mods.shift, Latch::Once);
    assert!(row(&term, 23).contains("SHIFT+"));
    tap_tool(&mut app, &mut term, Tool::Key(KeyCode::Right));
    assert_eq!(sel_len(&app), 1);
    assert_eq!(app.mods.shift, Latch::Off);

    // Two taps lock it: arrows keep extending (selection mode)...
    tap_tool(&mut app, &mut term, Tool::Mod(Mod::Shift));
    tap_tool(&mut app, &mut term, Tool::Mod(Mod::Shift));
    assert_eq!(app.mods.shift, Latch::Locked);
    assert!(row(&term, 23).contains("SELECT"));
    tap_tool(&mut app, &mut term, Tool::Key(KeyCode::Right));
    tap_tool(&mut app, &mut term, Tool::Key(KeyCode::Right));
    tap_tool(&mut app, &mut term, Tool::Key(KeyCode::Right));
    // Continues the existing 1-char selection.
    assert_eq!(sel_len(&app), 4);
    // ...and typing replaces the selection (lower-case) and ends the mode.
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('x'),
        KeyModifiers::NONE,
    )));
    assert_eq!(app.mods.shift, Latch::Off);
    assert!(app.editor().unwrap().doc.rope.to_string().starts_with("x"));

    // Ctrl latched + a letter from the soft keyboard = Ctrl+letter.
    tap_tool(&mut app, &mut term, Tool::Mod(Mod::Ctrl));
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('a'),
        KeyModifiers::NONE,
    )));
    assert_eq!(sel_len(&app), app.editor().unwrap().doc.rope.len_chars());
    assert_eq!(app.mods.ctrl, Latch::Off);

    // Esc releases everything.
    tap_tool(&mut app, &mut term, Tool::Mod(Mod::Ctrl));
    tap_tool(&mut app, &mut term, Tool::Mod(Mod::Ctrl));
    tap_tool(&mut app, &mut term, Tool::Key(KeyCode::Esc));
    assert!(!app.mods.any());
}

#[test]
fn box_mode_edits_a_column_on_every_line() {
    let root = project();
    std::fs::write(root.join("cols.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let mut app = App::new(
        root.clone(),
        vec![root.join("cols.txt")],
        "Visual Studio Dark",
        false,
    );
    let mut term = Terminal::new(TestBackend::new(44, 24)).unwrap();

    tap_tool(&mut app, &mut term, Tool::BoxMode);
    assert!(app.mods.box_mode());
    tap_tool(&mut app, &mut term, Tool::Key(KeyCode::Down));
    tap_tool(&mut app, &mut term, Tool::Key(KeyCode::Down));
    assert!(row(&term, 23).contains("BOX 3×"), "{}", row(&term, 23));
    // Typing goes to every line of the box.
    for c in "- ".chars() {
        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }
    assert_eq!(
        app.editor().unwrap().doc.rope.to_string(),
        "- alpha\n- beta\n- gamma\n"
    );
    // Widen the box with → and delete the column.
    tap_tool(&mut app, &mut term, Tool::Key(KeyCode::Right));
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Delete,
        KeyModifiers::NONE,
    )));
    assert_eq!(
        app.editor().unwrap().doc.rope.to_string(),
        "- lpha\n- eta\n- amma\n"
    );
    tap_tool(&mut app, &mut term, Tool::BoxMode);
    assert!(!app.mods.box_mode());

    // Physical keyboard: Alt+Shift+Down also makes a box.
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Down,
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    )));
    assert!(app.editor().unwrap().block.is_some());
}

#[test]
fn toolbar_pages_and_rotation_restore_desktop_layout() {
    let root = project();
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/main.rs")],
        "Visual Studio Dark",
        false,
    );
    let mut term = Terminal::new(TestBackend::new(44, 24)).unwrap();
    tap_tool(&mut app, &mut term, Tool::Page);
    assert!(
        row(&term, 21).contains("Copy"),
        "actions page: {}",
        row(&term, 21)
    );
    tap_tool(&mut app, &mut term, Tool::Page);
    assert!(row(&term, 21).contains("Esc"));

    // Rotate to landscape: full layout, no toolbar, no wrap.
    term.backend_mut().resize(120, 30);
    term.autoresize().unwrap();
    draw(&mut term, &mut app);
    assert!(!app.compact);
    assert!(!app.word_wrap());
    assert!(row(&term, 0).contains("File"));
    assert!(!row(&term, 27).contains("Esc"));
}
