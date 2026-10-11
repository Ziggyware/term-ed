//! Phone-screen behavior: compact layout, soft wrap, touch toolbar and taps,
//! driven through the real renderer on a headless backend.

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use termed::app::{App, Focus};

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
    // Toolbar sits above the status bar.
    assert!(row(&term, 22).contains("Esc"));
    assert!(row(&term, 22).contains("»"));

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

#[test]
fn touch_selection_mode_extends_selection_with_toolbar_arrows() {
    let root = project();
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/main.rs")],
        "Visual Studio Dark",
        false,
    );
    let mut term = Terminal::new(TestBackend::new(44, 24)).unwrap();
    draw(&mut term, &mut app);

    let (x, y) = find(&term, "Sel").unwrap();
    tap(&mut app, x, y);
    draw(&mut term, &mut app);
    assert!(app.select_mode);
    assert!(row(&term, 23).contains("SELECT"));
    let (x, y) = find(&term, "▶").unwrap();
    tap(&mut app, x, y);
    tap(&mut app, x, y);
    draw(&mut term, &mut app);
    let e = app.editor().unwrap();
    assert_eq!(e.sel.end() - e.sel.start(), 2);

    // Esc on the toolbar leaves selection mode.
    let (x, y) = find(&term, "Esc").unwrap();
    tap(&mut app, x, y);
    assert!(!app.select_mode);
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
    draw(&mut term, &mut app);
    let (x, y) = find(&term, "»").unwrap();
    tap(&mut app, x, y);
    draw(&mut term, &mut app);
    assert!(
        row(&term, 22).contains("Paste"),
        "second toolbar page: {}",
        row(&term, 22)
    );

    // Rotate to landscape: full layout, no toolbar, no wrap.
    term.backend_mut().resize(120, 30);
    term.autoresize().unwrap();
    draw(&mut term, &mut app);
    assert!(!app.compact);
    assert!(!app.word_wrap());
    assert!(row(&term, 0).contains("File"));
    assert!(!row(&term, 28).contains("Esc"));
}
