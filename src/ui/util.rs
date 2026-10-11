//! Low-level drawing helpers on top of ratatui's cell buffer.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::theme::{mix, rgb};

pub fn fill(buf: &mut Buffer, area: Rect, bg: Color) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.reset();
                c.set_bg(bg);
            }
        }
    }
}

/// Write `s` at (x, y), clipped at `max_x`. Returns the x after the text.
pub fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, style: Style, max_x: u16) -> u16 {
    if x >= max_x || y >= buf.area.bottom() || x < buf.area.left() {
        return x;
    }
    let max_x = max_x.min(buf.area.right());
    let (nx, _) = buf.set_stringn(x, y, s, (max_x - x) as usize, style);
    nx
}

pub fn width(s: &str) -> u16 {
    UnicodeWidthStr::width(s) as u16
}

pub fn fg(c: Color) -> Style {
    Style::default().fg(c)
}

pub fn fgbg(f: Color, b: Color) -> Style {
    Style::default().fg(f).bg(b)
}

pub fn bold(s: Style) -> Style {
    s.add_modifier(Modifier::BOLD)
}

pub fn set_bg(buf: &mut Buffer, x: u16, y: u16, bg: Color) {
    if let Some(c) = buf.cell_mut((x, y)) {
        c.set_bg(bg);
    }
}

/// Darken the cells of `area` to fake a drop shadow under floating panels.
pub fn shadow(buf: &mut Buffer, area: Rect) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                if let Color::Rgb(..) = c.bg {
                    c.bg = mix(c.bg, Color::Rgb(0, 0, 0), 0.4);
                }
                if let Color::Rgb(..) = c.fg {
                    c.fg = mix(c.fg, Color::Rgb(0, 0, 0), 0.4);
                }
            }
        }
    }
}

/// Greedy word wrap to `max` columns (long words are hard-broken).
pub fn wrap_text(s: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let mut word = word.to_string();
        loop {
            let cw = UnicodeWidthStr::width(cur.as_str());
            let ww = UnicodeWidthStr::width(word.as_str());
            let need = if cur.is_empty() { ww } else { cw + 1 + ww };
            if need <= max {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(&word);
                break;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                continue;
            }
            // Word longer than a whole line.
            let cut: String = word.chars().take(max).collect();
            word = word.chars().skip(max).collect();
            lines.push(cut);
            if word.is_empty() {
                break;
            }
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Text truncated to `max` columns with an ellipsis.
pub fn ellipsize(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Downgrade truecolor to the xterm 256-color cube for terminals without
/// 24-bit support.
pub fn downgrade_colors(buf: &mut Buffer) {
    for c in buf.content.iter_mut() {
        c.fg = to_256(c.fg);
        c.bg = to_256(c.bg);
    }
}

fn to_256(c: Color) -> Color {
    let Color::Rgb(..) = c else { return c };
    let (r, g, b) = rgb(c);
    let q = |v: u8| -> u8 {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            (v - 35) / 40
        }
    };
    let (qr, qg, qb) = (q(r), q(g), q(b));
    let lv = |i: u8| if i == 0 { 0 } else { 55 + 40 * i as i32 };
    let cube = 16 + 36 * qr + 6 * qg + qb;
    let cube_err =
        (lv(qr) - r as i32).pow(2) + (lv(qg) - g as i32).pow(2) + (lv(qb) - b as i32).pow(2);
    let avg = (r as i32 + g as i32 + b as i32) / 3;
    let gi = ((avg - 8).max(0) / 10).min(23);
    let gv = 8 + 10 * gi;
    let gray_err = (gv - r as i32).pow(2) + (gv - g as i32).pow(2) + (gv - b as i32).pow(2);
    Color::Indexed(if gray_err < cube_err {
        232 + gi as u8
    } else {
        cube
    })
}
