//! High-resolution rendering: picture tabs (mosaic and pixel graphics), the
//! sub-cell scrollbar, and that pictures can never be overwritten.

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termed::app::App;
use termed::commands::Command;
use termed::ui::hires::Glyphs;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("termed-hires-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn screen(term: &Terminal<TestBackend>) -> String {
    term.backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

fn picture(dir: &std::path::Path) -> PathBuf {
    let mut img = image::RgbaImage::new(64, 40);
    for (x, y, p) in img.enumerate_pixels_mut() {
        *p = image::Rgba([
            (x * 4) as u8,
            (y * 6) as u8,
            200,
            if x < 8 { 0 } else { 255 },
        ]);
    }
    let path = dir.join("pic.png");
    img.save(&path).unwrap();
    path
}

#[test]
fn pictures_open_as_view_only_tabs() {
    let dir = temp("pic");
    let path = picture(&dir);
    let before = std::fs::read(&path).unwrap();
    let mut app = App::new(dir.clone(), vec![path.clone()], "Visual Studio Dark", false);
    app.graphics.glyphs = Glyphs::Half;
    assert!(app.editor().unwrap().image.is_some());
    // Typing, pasting and saving do nothing to the file.
    for c in "hello".chars() {
        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }
    app.handle_event(Event::Paste("junk".into()));
    app.run(Command::Save);
    assert_eq!(app.editor().unwrap().doc.rope.len_chars(), 0);
    assert!(!app.editor().unwrap().doc.is_dirty());
    assert_eq!(std::fs::read(&path).unwrap(), before, "picture untouched");

    // Mosaic fallback draws the picture with block glyphs and its info.
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| termed::ui::draw(f, &mut app, true)).unwrap();
    let s = screen(&term);
    assert!(s.contains('▀') || s.contains('▄'), "half-block mosaic");
    assert!(s.contains("64×40"));

    // With pixel graphics the picture goes out as an image instead.
    app.toasts.clear();
    app.graphics.record = true;
    term.draw(|f| termed::ui::draw(f, &mut app, true)).unwrap();
    let (r, img) = app.graphics.recorded.last().expect("image placed");
    assert!(r.width > 10 && r.height > 5);
    assert_eq!(
        (img.width(), img.height()),
        (r.width as u32 * 10, r.height as u32 * 20)
    );
    let s = screen(&term);
    assert!(
        s.contains("kitty graphics"),
        "{}",
        s.chars()
            .collect::<Vec<_>>()
            .chunks(100)
            .map(|c| c.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn scrollbar_thumb_moves_in_eighths_of_a_cell() {
    let dir = temp("scroll");
    let path = dir.join("long.txt");
    let text: String = (0..2000).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&path, text).unwrap();
    let mut app = App::new(dir, vec![path], "Visual Studio Dark", false);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| termed::ui::draw(f, &mut app, true)).unwrap();
    let eighths = ['▁', '▂', '▃', '▄', '▅', '▆', '▇'];
    let mut partial = 0;
    for line in [137, 512, 901, 1333] {
        app.editors[0].scroll_line = line;
        term.draw(|f| termed::ui::draw(f, &mut app, true)).unwrap();
        let buf = term.backend().buffer();
        let x = app.layout.scrollbar.x;
        if (0..30).any(|y| buf[(x, y)].symbol().chars().any(|c| eighths.contains(&c))) {
            partial += 1;
        }
    }
    assert!(partial >= 2, "thumb edges use eighth blocks ({partial}/4)");
}

#[test]
fn welcome_logo_uses_the_finest_mosaic() {
    let dir = temp("welcome");
    let mut app = App::new(dir, vec![], "Visual Studio Dark", false);
    app.show_sidebar = false;
    app.graphics.glyphs = Glyphs::Octant;
    let mut term = Terminal::new(TestBackend::new(110, 32)).unwrap();
    term.draw(|f| termed::ui::draw(f, &mut app, true)).unwrap();
    let octants = screen(&term)
        .chars()
        .filter(|c| ('\u{1CD00}'..='\u{1CDE5}').contains(c))
        .count();
    assert!(octants > 20, "octant edges in the logo: {octants}");
}
