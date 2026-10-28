//! Low-level drawing helpers on top of ratatui's cell buffer.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;
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
    let max_x = max_x.min(buf.area.right());
    if x >= max_x || x < buf.area.left() || y < buf.area.top() || y >= buf.area.bottom() {
        return x;
    }
    let (nx, _) = buf.set_stringn(x, y, s, (max_x - x) as usize, style);
    nx
}

/// Write one glyph through its stack-encoded UTF-8 representation.
pub fn put_char(buf: &mut Buffer, x: u16, y: u16, c: char, style: Style, max_x: u16) -> u16 {
    let mut encoded = [0; 4];
    put(buf, x, y, c.encode_utf8(&mut encoded), style, max_x)
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

/// Greedy word wrap to `max` terminal columns. Long words are hard-broken at
/// grapheme boundaries; an individually wider grapheme remains intact.
pub fn wrap_text(s: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_width = 0usize;
    for word in s.split_whitespace() {
        let mut rest = word;
        let mut rest_width = UnicodeWidthStr::width(rest);
        loop {
            let separator = usize::from(!cur.is_empty());
            let needed = cur_width
                .saturating_add(separator)
                .saturating_add(rest_width);
            if needed <= max {
                if separator == 1 {
                    cur.push(' ');
                    cur_width += 1;
                }
                cur.push_str(rest);
                cur_width += rest_width;
                break;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                cur_width = 0;
                continue;
            }

            let mut cut = 0;
            let mut cut_width = 0usize;
            for (index, grapheme) in rest.grapheme_indices(true) {
                let grapheme_width = UnicodeWidthStr::width(grapheme);
                if cut_width > 0 && cut_width.saturating_add(grapheme_width) > max {
                    break;
                }
                cut = index + grapheme.len();
                cut_width = cut_width.saturating_add(grapheme_width);
                if cut_width >= max {
                    break;
                }
            }
            lines.push(rest[..cut].to_string());
            rest = &rest[cut..];
            rest_width = rest_width.saturating_sub(cut_width);
            if rest.is_empty() {
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
    let mut w = 0usize;
    for grapheme in s.graphemes(true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if w.saturating_add(grapheme_width).saturating_add(1) > max {
            break;
        }
        out.push_str(grapheme);
        w += grapheme_width;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_hard_breaks_by_display_columns() {
        assert_eq!(wrap_text("界界界", 4).join("|"), "界界|界");
        assert_eq!(wrap_text("abcdef", 3).join("|"), "abc|def");
        assert!(wrap_text("ab 界界界 cd", 4)
            .iter()
            .all(|line| UnicodeWidthStr::width(line.as_str()) <= 4));
    }

    #[test]
    fn wrapping_and_ellipsizing_preserve_graphemes() {
        let grapheme = "e\u{301}";
        assert_eq!(
            wrap_text(&format!("{grapheme}{grapheme}"), 1).join("|"),
            format!("{grapheme}|{grapheme}")
        );
        assert_eq!(
            ellipsize(&format!("{grapheme}xy"), 2),
            format!("{grapheme}…")
        );
    }
}
