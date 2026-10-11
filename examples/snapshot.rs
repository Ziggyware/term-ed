//! Render termed frames to HTML without a terminal — used to produce the
//! screenshots in the README and to eyeball UI changes in CI.
//!
//! ```text
//! cargo run --example snapshot -- out.html 140x40 "Visual Studio Dark" <root> [file] -- [keys...]
//! ```
//! Keys: `ctrl+p`, `alt+down`, `shift+f3`, `enter`, `esc`, `down`, or
//! `text:literal text` (typed char by char; `\n` presses Enter), `tap:x,y`,
//! `swipeup:x,y` / `swipedown:x,y` (scroll wheel), `resize:WxH`, `wait:ms`.

use std::fmt::Write as _;
use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::{Backend, TestBackend};
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::style::{Color, Modifier};
use termed::app::App;

fn mouse(kind: MouseEventKind, at: &str) -> Vec<Event> {
    let (x, y) = at.split_once(',').unwrap();
    vec![Event::Mouse(MouseEvent {
        kind,
        column: x.parse().unwrap(),
        row: y.parse().unwrap(),
        modifiers: KeyModifiers::NONE,
    })]
}

fn key(spec: &str) -> Vec<Event> {
    if let Some(at) = spec.strip_prefix("tap:") {
        // A tap is a press + release, as mobile terminals report it.
        let mut v = mouse(MouseEventKind::Down(MouseButton::Left), at);
        v.extend(mouse(MouseEventKind::Up(MouseButton::Left), at));
        return v;
    }
    if let Some(at) = spec.strip_prefix("swipeup:") {
        return mouse(MouseEventKind::ScrollDown, at);
    }
    if let Some(at) = spec.strip_prefix("swipedown:") {
        return mouse(MouseEventKind::ScrollUp, at);
    }
    if let Some(t) = spec.strip_prefix("text:") {
        return t
            .replace("\\n", "\n")
            .chars()
            .map(|c| {
                let code = if c == '\n' {
                    KeyCode::Enter
                } else {
                    KeyCode::Char(c)
                };
                Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
            })
            .collect();
    }
    let mut mods = KeyModifiers::NONE;
    let mut parts: Vec<&str> = spec.split('+').collect();
    let last = parts.pop().unwrap();
    for p in parts {
        mods |= match p {
            "ctrl" => KeyModifiers::CONTROL,
            "alt" => KeyModifiers::ALT,
            "shift" => KeyModifiers::SHIFT,
            _ => KeyModifiers::NONE,
        };
    }
    let code = match last {
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgdn" => KeyCode::PageDown,
        "pgup" => KeyCode::PageUp,
        f if f.starts_with('f') && f.len() > 1 => KeyCode::F(f[1..].parse().unwrap()),
        c => KeyCode::Char(c.chars().next().unwrap()),
    };
    vec![Event::Key(KeyEvent::new(code, mods))]
}

fn css(c: Color, default: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => default.to_string(),
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let split = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (pos, keys) = args.split_at(split);
    let out = PathBuf::from(&pos[0]);
    let (w, h): (u16, u16) = pos[1]
        .split_once('x')
        .map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap()))
        .unwrap();
    let theme = &pos[2];
    let root = std::path::absolute(&pos[3])?;
    let files: Vec<PathBuf> = pos[4..].iter().map(PathBuf::from).collect();
    let mut app = App::new(root, files, theme, false);
    let mut term = Terminal::new(TestBackend::new(w, h))?;
    term.draw(|f| termed::ui::draw(f, &mut app, true))?;
    for k in keys.iter().skip(1) {
        if let Some(size) = k.strip_prefix("resize:") {
            let (w, h) = size.split_once('x').unwrap();
            term.backend_mut().resize(w.parse()?, h.parse()?);
            term.autoresize()?;
            term.draw(|f| termed::ui::draw(f, &mut app, true))?;
            continue;
        }
        if let Some(ms) = k.strip_prefix("wait:") {
            // Let background work (the agent) progress.
            let until = std::time::Instant::now() + std::time::Duration::from_millis(ms.parse()?);
            while std::time::Instant::now() < until {
                app.pump_agent();
                term.draw(|f| termed::ui::draw(f, &mut app, true))?;
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            continue;
        }
        if let Some(ms) = k.strip_prefix("idle:") {
            // Wait (up to ms) until the agent finishes its turn.
            let until = std::time::Instant::now() + std::time::Duration::from_millis(ms.parse()?);
            std::thread::sleep(std::time::Duration::from_millis(300));
            while std::time::Instant::now() < until
                && (app.agent.is_working() || app.agent.pending_permission().is_some())
            {
                app.pump_agent();
                term.draw(|f| termed::ui::draw(f, &mut app, true))?;
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            continue;
        }
        for ev in key(k) {
            app.handle_event(ev);
            app.pump_agent();
            term.draw(|f| termed::ui::draw(f, &mut app, true))?;
        }
    }
    term.draw(|f| termed::ui::draw(f, &mut app, true))?;
    let cursor = term.backend_mut().get_cursor_position().ok();
    let cursor_visible = term.backend().cursor_visible();
    let buf = term.backend().buffer().clone();
    let (w, h) = (buf.area.width, buf.area.height);

    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><style>body{margin:0;background:#111}\
         .t{font:15px/19px 'DejaVu Sans Mono',monospace;white-space:pre;position:relative;display:inline-block}\
         .r{height:19px}.c{display:inline-block;width:9.03px;height:19px;line-height:19px;vertical-align:top;text-align:center;overflow:hidden}\
         .cur{position:absolute;width:2px;height:19px;background:#fff}</style><div class=t>",
    );
    for y in 0..h {
        html.push_str("<div class=r>");
        let mut x = 0;
        while x < w {
            let cell = &buf[(x, y)];
            let sym = cell.symbol();
            let cw = unicode_width::UnicodeWidthStr::width(sym).max(1) as u16;
            let mut style = format!(
                "color:{};background:{}",
                css(cell.fg, "#ddd"),
                css(cell.bg, "#1e1e1e")
            );
            if cell.modifier.contains(Modifier::BOLD) {
                style.push_str(";font-weight:bold");
            }
            if cell.modifier.contains(Modifier::ITALIC) {
                style.push_str(";font-style:italic");
            }
            if cw > 1 {
                let _ = write!(style, ";width:{:.2}px", 9.03 * cw as f32);
            }
            let s = sym
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            let _ = write!(html, "<span class=c style=\"{style}\">{s}</span>");
            x += cw;
        }
        html.push_str("</div>");
    }
    if let (Some(c), true) = (cursor, cursor_visible) {
        let _ = write!(
            html,
            "<div class=cur style=\"left:{:.2}px;top:{}px\"></div>",
            c.x as f32 * 9.03,
            c.y as u32 * 19
        );
    }
    html.push_str("</div>");
    std::fs::write(&out, html)?;
    Ok(())
}
