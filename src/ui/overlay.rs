//! Floating UI: command palette / quick open, find & replace widget,
//! confirmation dialog and toast notifications.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Widget};
use unicode_width::UnicodeWidthChar;

use super::util::{bold, ellipsize, fg, fgbg, fill, put, shadow, width, wrap_text};
use crate::app::{App, Focus, PaletteMode, ToastKind};
use crate::icons;
use crate::theme::mix;

fn panel(buf: &mut Buffer, r: Rect, border: Color, bg: Color, title: Option<&str>) {
    shadow(buf, Rect::new(r.x + 1, r.bottom(), r.width, 1));
    shadow(buf, Rect::new(r.right(), r.y + 1, 1, r.height));
    fill(buf, r, bg);
    let mut b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border).bg(bg))
        .style(Style::default().bg(bg));
    if let Some(t) = title {
        b = b
            .title(format!(" {t} "))
            .title_style(Style::default().fg(border).add_modifier(Modifier::BOLD));
    }
    b.render(r, buf);
}

/// Draw an editable single-line field; returns the caret position.
#[allow(clippy::too_many_arguments)]
fn input(
    buf: &mut Buffer,
    r: Rect,
    text: &str,
    cursor: usize,
    placeholder: &str,
    fgc: Color,
    dim: Color,
    bg: Color,
) -> Position {
    fill(buf, r, bg);
    let w = r.width.saturating_sub(1) as usize;
    let before: usize = text
        .chars()
        .take(cursor)
        .map(|c| c.width().unwrap_or(0))
        .sum();
    let skip_w = before.saturating_sub(w);
    let mut skipped = 0;
    let visible: String = text
        .chars()
        .skip_while(|c| {
            let keep = skipped >= skip_w;
            if !keep {
                skipped += c.width().unwrap_or(0);
            }
            !keep
        })
        .collect();
    if text.is_empty() {
        put(
            buf,
            r.x,
            r.y,
            placeholder,
            fgbg(dim, bg).add_modifier(Modifier::ITALIC),
            r.right(),
        );
    } else {
        put(buf, r.x, r.y, &visible, fgbg(fgc, bg), r.right());
    }
    Position::new(r.x + (before - skip_w) as u16, r.y)
}

pub fn render_palette(buf: &mut Buffer, screen: Rect, app: &mut App) -> Option<Position> {
    let pal = app.theme.palette.clone();
    let nerd = app.settings.nerd_font;
    let (cur_line, total_lines) = app
        .editor()
        .map(|e| (e.cursor().0 + 1, e.doc.len_lines()))
        .unwrap_or((0, 0));
    let compact = app.compact;
    // Rows taken at the bottom by the toolbar and status bar.
    let reserved = 1 + u16::from(app.toolbar_visible());
    let p = app.palette.as_mut()?;
    // Phones: edge-to-edge sheet just under the title bar.
    let (w, x, top) = if compact {
        let w = screen.width.saturating_sub(2).max(10);
        (w, screen.x + 1, screen.y + 1)
    } else {
        let w = (screen.width.saturating_sub(8))
            .clamp(30, 90)
            .min(screen.width);
        (w, screen.x + (screen.width - w) / 2, screen.y + 2)
    };
    let list_rows = match p.mode {
        PaletteMode::GotoLine | PaletteMode::SaveAs => 1,
        _ => (p.filtered.len() as u16)
            .clamp(1, 14)
            .min(screen.height.saturating_sub(top + 4 + reserved).max(1)),
    };
    let h = list_rows + 4;
    let r = Rect::new(x, top, w, h);
    let title = match p.mode {
        PaletteMode::Commands => "Command Palette",
        PaletteMode::Files => "Go to File",
        PaletteMode::GotoLine => "Go to Line",
        PaletteMode::Themes => "Color Theme",
        PaletteMode::Languages => "Language Mode",
        PaletteMode::SaveAs => "Save As",
    };
    panel(buf, r, pal.accent, pal.popup_bg, Some(title));

    // Input row.
    let (prompt, placeholder) = match p.mode {
        PaletteMode::Commands => ("›", "Type a command"),
        PaletteMode::Files => ("⌕", "Search files by name (fuzzy)"),
        PaletteMode::GotoLine => (":", "line[:column]"),
        PaletteMode::Themes => ("◐", "Select a color theme (↑↓ to preview)"),
        PaletteMode::Languages => ("{}", "Select language mode"),
        PaletteMode::SaveAs => ("⤓", "path/relative/to/project"),
    };
    let iy = r.y + 1;
    let field = Rect::new(r.x + 2, iy, r.width.saturating_sub(4), 1);
    fill(buf, field, pal.input_bg);
    put(
        buf,
        field.x + 1,
        iy,
        prompt,
        bold(fgbg(pal.accent, pal.input_bg)),
        field.right(),
    );
    let px = field.x + width(prompt) + 2;
    let caret = input(
        buf,
        Rect::new(px, iy, field.right().saturating_sub(px + 1), 1),
        &p.input,
        p.cursor,
        placeholder,
        pal.popup_fg,
        pal.popup_dim,
        pal.input_bg,
    );
    // Separator.
    let sep = "─".repeat(r.width.saturating_sub(2) as usize);
    put(
        buf,
        r.x + 1,
        iy + 1,
        &sep,
        fgbg(pal.border, pal.popup_bg),
        r.right() - 1,
    );

    let list = Rect::new(r.x + 1, iy + 2, r.width.saturating_sub(2), list_rows);
    app.layout.palette_list = list;
    match p.mode {
        PaletteMode::GotoLine => {
            let msg = format!(
                "Current line: {cur_line} of {total_lines}. Type a line number between 1 and {total_lines}."
            );
            put(
                buf,
                list.x + 2,
                list.y,
                &msg,
                fgbg(pal.popup_dim, pal.popup_bg),
                list.right(),
            );
            return Some(caret);
        }
        PaletteMode::SaveAs => {
            put(
                buf,
                list.x + 2,
                list.y,
                "Enter ↵ to save · Esc to cancel",
                fgbg(pal.popup_dim, pal.popup_bg),
                list.right(),
            );
            return Some(caret);
        }
        _ => {}
    }
    if p.filtered.is_empty() {
        put(
            buf,
            list.x + 2,
            list.y,
            "No matching results",
            fgbg(pal.popup_dim, pal.popup_bg),
            list.right(),
        );
        return Some(caret);
    }
    if p.selected < p.scroll {
        p.scroll = p.selected;
    } else if p.selected >= p.scroll + list_rows as usize {
        p.scroll = p.selected + 1 - list_rows as usize;
    }
    for row in 0..list_rows as usize {
        let i = p.scroll + row;
        let Some(item) = p.filtered.get(i) else { break };
        let y = list.y + row as u16;
        let selected = i == p.selected;
        let bg = if selected {
            pal.popup_selected
        } else {
            pal.popup_bg
        };
        fill(buf, Rect::new(list.x, y, list.width, 1), bg);
        if selected {
            put(buf, list.x, y, "▌", fgbg(pal.accent, bg), list.right());
        }
        let label = &p.items[item.index];
        let hint = p.hints.get(item.index).cloned().unwrap_or_default();
        let mut x = list.x + 2;
        let display = if p.mode == PaletteMode::Files {
            let name = label.rsplit('/').next().unwrap_or(label);
            let icon = icons::file_icon(name, nerd);
            x = put(buf, x, y, icon.glyph, fgbg(icon.color, bg), list.right()) + 1;
            label.clone()
        } else {
            label.clone()
        };
        // Keyboard hints are noise on a phone and steal width from the label.
        let hint = if compact { String::new() } else { hint };
        let hint_w = if hint.is_empty() { 0 } else { width(&hint) + 3 };
        let max_x = list.right().saturating_sub(hint_w + 1);
        for (ci, ch) in display.chars().enumerate() {
            let hit = item.positions.binary_search(&ci).is_ok();
            let st = if hit {
                bold(fgbg(pal.match_fg, bg))
            } else {
                fgbg(pal.popup_fg, bg)
            };
            x = put(buf, x, y, &ch.to_string(), st, max_x);
        }
        if p.mode == PaletteMode::Themes && *label == app.theme.name {
            put(buf, x + 1, y, "✓", fgbg(pal.success, bg), max_x);
        }
        if !hint.is_empty() {
            let hx = list.right().saturating_sub(width(&hint) + 2);
            let st = if p.mode == PaletteMode::Commands {
                fgbg(pal.popup_fg, mix(bg, pal.popup_fg, 0.12))
            } else {
                fgbg(pal.popup_dim, bg)
            };
            put(buf, hx, y, &format!(" {hint} "), st, list.right());
        }
    }
    // Count badge.
    let count = format!(" {} ", p.filtered.len());
    put(
        buf,
        r.right().saturating_sub(width(&count) + 2),
        r.bottom() - 1,
        &count,
        fgbg(pal.popup_dim, pal.popup_bg),
        r.right(),
    );
    Some(caret)
}

pub fn render_find(buf: &mut Buffer, editor: Rect, app: &mut App) -> Option<Position> {
    let pal = app.theme.palette.clone();
    let f = &app.find;
    let compact = app.compact;
    let w = if compact {
        editor.width
    } else {
        64u16.min(editor.width.saturating_sub(4))
    };
    if w < 24 {
        return None;
    }
    let h = if f.show_replace { 4 } else { 3 };
    let minimap_pad = if app.settings.minimap { 16 } else { 3 };
    let x = if compact {
        editor.x
    } else {
        editor.right().saturating_sub(w + minimap_pad).max(editor.x)
    };
    let r = Rect::new(x, editor.y, w, h);
    let focused = app.focus == Focus::Find;
    panel(
        buf,
        r,
        if focused { pal.accent } else { pal.border },
        pal.popup_bg,
        None,
    );
    app.layout.find_box = r;
    let bg = pal.popup_bg;

    // Toggle for replace row.
    let tx = r.x + 1;
    put(
        buf,
        tx,
        r.y + 1,
        if f.show_replace { "⌄" } else { "›" },
        fgbg(pal.popup_fg, bg),
        r.right(),
    );
    app.layout
        .find_buttons
        .push((Rect::new(tx, r.y + 1, 1, 1), 8));

    let field_w = w.saturating_sub(30);
    let fx = r.x + 3;
    let qbg = if f.error {
        mix(pal.input_bg, pal.error, 0.35)
    } else {
        pal.input_bg
    };
    let mut caret = input(
        buf,
        Rect::new(fx, r.y + 1, field_w, 1),
        &f.query,
        f.query_cursor,
        "Find",
        pal.popup_fg,
        pal.popup_dim,
        qbg,
    );
    if f.query_selected && !f.query.is_empty() {
        // Pre-filled text is selected: typing replaces it.
        let qw = width(&f.query).min(field_w.saturating_sub(1));
        for i in 0..qw {
            super::util::set_bg(buf, fx + i, r.y + 1, pal.selection);
        }
    }
    // Option toggles, VS style.
    let mut bx = fx + field_w + 1;
    for (i, (label, on)) in [
        ("Aa", f.case_sensitive),
        ("ab", f.whole_word),
        (".*", f.regex),
    ]
    .iter()
    .enumerate()
    {
        let st = if *on {
            bold(fgbg(pal.accent_fg, pal.accent))
        } else {
            fgbg(pal.popup_dim, pal.input_bg)
        };
        put(buf, bx, r.y + 1, label, st, r.right());
        app.layout
            .find_buttons
            .push((Rect::new(bx, r.y + 1, 2, 1), i as u8));
        bx += 3;
    }
    let status = if f.query.is_empty() {
        "".to_string()
    } else if f.matches.is_empty() {
        "No results".to_string()
    } else {
        format!(
            "{} of {}",
            f.current.map(|c| c + 1).unwrap_or(0),
            f.matches.len()
        )
    };
    let sc = if !f.query.is_empty() && f.matches.is_empty() {
        pal.error
    } else {
        pal.popup_fg
    };
    let nav_x = r.right().saturating_sub(8);
    put(
        buf,
        bx,
        r.y + 1,
        &ellipsize(&status, nav_x.saturating_sub(bx + 1) as usize),
        fgbg(sc, bg),
        nav_x,
    );
    for (i, sym) in ["↑", "↓", "×"].iter().enumerate() {
        let x = nav_x + i as u16 * 2;
        put(buf, x, r.y + 1, sym, fgbg(pal.popup_fg, bg), r.right());
        app.layout
            .find_buttons
            .push((Rect::new(x, r.y + 1, 1, 1), 3 + i as u8));
    }

    if f.show_replace {
        let c2 = input(
            buf,
            Rect::new(fx, r.y + 2, field_w, 1),
            &f.replace,
            f.replace_cursor,
            "Replace",
            pal.popup_fg,
            pal.popup_dim,
            pal.input_bg,
        );
        if f.on_replace {
            caret = c2;
        }
        let bx = fx + field_w + 1;
        put(
            buf,
            bx,
            r.y + 2,
            "⇄ One",
            fgbg(pal.popup_fg, mix(bg, pal.popup_fg, 0.1)),
            r.right(),
        );
        app.layout
            .find_buttons
            .push((Rect::new(bx, r.y + 2, 5, 1), 6));
        put(
            buf,
            bx + 6,
            r.y + 2,
            "⇄ All",
            fgbg(pal.popup_fg, mix(bg, pal.popup_fg, 0.1)),
            r.right(),
        );
        app.layout
            .find_buttons
            .push((Rect::new(bx + 6, r.y + 2, 5, 1), 7));
    }
    focused.then_some(caret)
}

pub fn render_dialog(buf: &mut Buffer, screen: Rect, app: &mut App) {
    let pal = app.theme.palette.clone();
    let Some(d) = &app.dialog else { return };
    // Dim everything behind the modal.
    shadow(buf, screen);
    let w = 64u16.min(screen.width.saturating_sub(2));
    let text_w = w.saturating_sub(9) as usize;
    let mut lines = wrap_text(&d.message, text_w);
    let first_detail = lines.len();
    lines.extend(wrap_text(
        "Your changes will be lost if you don't save them.",
        text_w,
    ));
    let narrow = w < 40;
    let labels: [&str; 3] = if narrow {
        [" Save ", " Don't Save ", " Cancel "]
    } else {
        ["  Save  ", "  Don't Save  ", "  Cancel  "]
    };
    let total: u16 = labels.iter().map(|l| width(l) + 2).sum();
    // Stack the buttons when they don't fit on one row.
    let stacked = total + 4 > w;
    let button_rows = if stacked { 3 * 2 - 1 } else { 1 };
    let h = 5 + lines.len() as u16 + button_rows;
    let r = Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + screen.height.saturating_sub(h) / 2,
        w,
        h,
    );
    panel(buf, r, pal.warning, pal.popup_bg, Some(&d.title));
    put(
        buf,
        r.x + 3,
        r.y + 2,
        "⚠",
        fgbg(pal.warning, pal.popup_bg),
        r.right(),
    );
    for (i, l) in lines.iter().enumerate() {
        let st = if i < first_detail {
            bold(fgbg(pal.popup_fg, pal.popup_bg))
        } else {
            fgbg(pal.popup_dim, pal.popup_bg)
        };
        put(buf, r.x + 6, r.y + 2 + i as u16, l, st, r.right() - 2);
    }
    let mut x = r.right().saturating_sub(total + 2);
    let mut y = r.y + 3 + lines.len() as u16;
    let mut buttons = Vec::new();
    for (i, l) in labels.iter().enumerate() {
        let selected = i == d.selected;
        let st = if selected {
            bold(fgbg(pal.accent_fg, pal.accent))
        } else {
            fgbg(pal.popup_fg, pal.input_bg)
        };
        if stacked {
            let br = Rect::new(r.x + 3, y, w.saturating_sub(6), 1);
            fill(buf, br, st.bg.unwrap_or(pal.input_bg));
            put(
                buf,
                br.x + (br.width.saturating_sub(width(l.trim()))) / 2,
                y,
                l.trim(),
                st,
                br.right(),
            );
            buttons.push((br, i));
            y += 2;
        } else {
            let nx = put(buf, x, y, l, st, r.right());
            buttons.push((Rect::new(x, y, nx - x, 1), i));
            x = nx + 2;
        }
    }
    app.layout.dialog_buttons = buttons;
}

pub fn render_toasts(buf: &mut Buffer, area: Rect, app: &App) {
    let pal = &app.theme.palette;
    let mut y = area.bottom();
    for t in app.toasts.iter().rev() {
        let (icon, color) = match t.kind {
            ToastKind::Info => ("ℹ", pal.accent),
            ToastKind::Success => ("✔", pal.success),
            ToastKind::Error => ("✖", pal.error),
        };
        let text = ellipsize(&t.text, (area.width as usize).saturating_sub(10).min(70));
        let w = width(&text) + 7;
        if y < area.y + 4 || w > area.width {
            break;
        }
        y -= 3;
        let x = area
            .right()
            .saturating_sub(w + 1 + app.layout.minimap.width + app.layout.scrollbar.width);
        let r = Rect::new(x, y, w, 3);
        panel(buf, r, mix(pal.popup_bg, color, 0.6), pal.popup_bg, None);
        put(
            buf,
            r.x + 2,
            r.y + 1,
            icon,
            bold(fg(color).bg(pal.popup_bg)),
            r.right(),
        );
        put(
            buf,
            r.x + 4,
            r.y + 1,
            &text,
            fgbg(pal.popup_fg, pal.popup_bg),
            r.right() - 1,
        );
    }
}
