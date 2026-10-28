//! Floating UI: command palette / quick open, find & replace widget,
//! confirmation dialog and toast notifications.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Widget};
use unicode_width::UnicodeWidthChar;

use super::util::{bold, ellipsize, fg, fgbg, fill, put, put_char, shadow, width, wrap_text};
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
    let reserved = 1 + if app.toolbar_visible() { 2 } else { 0 };
    let edit = app.management.edit.clone();
    let p = app.palette.as_mut()?;
    let managed = matches!(
        p.mode,
        PaletteMode::Settings
            | PaletteMode::AgentSettings
            | PaletteMode::SubAgents
            | PaletteMode::SubAgentSettings
            | PaletteMode::Skills
            | PaletteMode::SkillActions
            | PaletteMode::PluginActions
            | PaletteMode::PluginCommands
            | PaletteMode::ConfirmRemoveSkill
            | PaletteMode::ConfirmRemoveSubAgent
    );
    let footer_h = if managed { 3 } else { 0 };
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
        PaletteMode::Input => 4.min(
            screen
                .height
                .saturating_sub(top + 4 + reserved + footer_h)
                .max(1),
        ),
        _ => (p.filtered.len() as u16).clamp(1, 14).min(
            screen
                .height
                .saturating_sub(top + 4 + reserved + footer_h)
                .max(1),
        ),
    };
    let h = list_rows + 4 + footer_h;
    let r = Rect::new(x, top, w, h);
    let title = match p.mode {
        PaletteMode::Commands => "Command Palette",
        PaletteMode::Files => "Go to File",
        PaletteMode::GotoLine => "Go to Line",
        PaletteMode::Themes => "Color Theme",
        PaletteMode::Languages => "Language Mode",
        PaletteMode::AgentModels => "Agent Model",
        PaletteMode::AgentPicker => "AI Agent",
        PaletteMode::SaveAs => "Save As",
        PaletteMode::Plugins | PaletteMode::PluginActions | PaletteMode::PluginCommands => {
            "Plugins"
        }
        PaletteMode::Settings => "Settings · User",
        PaletteMode::AgentSettings => "AI Agent Configuration",
        PaletteMode::SubAgents | PaletteMode::SubAgentSettings => {
            "Sub-agents · Built-in Specialists"
        }
        PaletteMode::Skills | PaletteMode::SkillActions => "Skills",
        PaletteMode::Input => edit
            .as_ref()
            .map(|e| e.title.as_str())
            .unwrap_or("Edit Setting"),
        PaletteMode::ConfirmRemoveSkill => "Delete Skill?",
        PaletteMode::ConfirmRemoveSubAgent => "Delete Sub-agent?",
    };
    panel(buf, r, pal.accent, pal.popup_bg, Some(title));

    // Input row.
    let (prompt, placeholder) = match p.mode {
        PaletteMode::Commands => ("›", "Type a command"),
        PaletteMode::Files => ("⌕", "Search files by name (fuzzy)"),
        PaletteMode::GotoLine => (":", "line[:column]"),
        PaletteMode::Themes => ("◐", "Select a color theme (↑↓ to preview)"),
        PaletteMode::Languages => ("{}", "Select language mode"),
        PaletteMode::AgentModels => ("✦", "Choose a model (free ones first)"),
        PaletteMode::AgentPicker => ("✦", "Choose an AI agent"),
        PaletteMode::SaveAs => ("⤓", "path/relative/to/project"),
        PaletteMode::Input => ("›", "Enter a value; empty restores the default"),
        _ => ("⚙", "Filter options · ↑↓ · Enter to select · Esc to close"),
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
    let secret = p.mode == PaletteMode::Input && edit.as_ref().is_some_and(|e| e.secret);
    let masked = if secret {
        "•".repeat(p.input.chars().count())
    } else {
        String::new()
    };
    let display = if secret {
        masked.as_str()
    } else {
        p.input.as_str()
    };
    let cursor = if secret {
        p.input[..p.cursor].chars().count() * "•".len()
    } else {
        p.cursor
    };
    let caret = input(
        buf,
        Rect::new(px, iy, field.right().saturating_sub(px + 1), 1),
        display,
        cursor,
        placeholder,
        pal.popup_fg,
        pal.popup_dim,
        pal.input_bg,
    );
    if p.input_selected {
        for x in px..field.right().saturating_sub(1) {
            if let Some(cell) = buf.cell_mut((x, iy)) {
                cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
            }
        }
    }
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
        PaletteMode::Input => {
            let hint = edit.as_ref().map(|e| e.hint.as_str()).unwrap_or("");
            let text = format!("Enter: save · Esc: cancel · Ctrl+A: select all\n{hint}");
            for (row, line) in wrap_text(&text, list.width.saturating_sub(4) as usize)
                .into_iter()
                .take(list_rows as usize)
                .enumerate()
            {
                put(
                    buf,
                    list.x + 2,
                    list.y + row as u16,
                    &line,
                    fgbg(pal.popup_dim, pal.popup_bg),
                    list.right(),
                );
            }
            return Some(caret);
        }
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
        let hint = if managed
            || (compact && matches!(p.mode, PaletteMode::Commands | PaletteMode::Files))
        {
            String::new()
        } else {
            hint
        };
        let hint = ellipsize(&hint, (list.width / 3) as usize);
        let hint_w = if hint.is_empty() { 0 } else { width(&hint) + 3 };
        let max_x = list.right().saturating_sub(hint_w + 1);
        for (ci, ch) in display.chars().enumerate() {
            let hit = item.positions.binary_search(&ci).is_ok();
            let st = if hit {
                bold(fgbg(pal.match_fg, bg))
            } else {
                fgbg(pal.popup_fg, bg)
            };
            x = put_char(buf, x, y, ch, st, max_x);
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
    if managed {
        let y = list.bottom();
        put(
            buf,
            list.x + 1,
            y,
            &"─".repeat(list.width.saturating_sub(2) as usize),
            fgbg(pal.border, pal.popup_bg),
            list.right(),
        );
        let hint = p
            .filtered
            .get(p.selected)
            .and_then(|item| p.hints.get(item.index))
            .map(String::as_str)
            .unwrap_or("");
        for (row, line) in wrap_text(hint, list.width.saturating_sub(4) as usize)
            .into_iter()
            .take(2)
            .enumerate()
        {
            put(
                buf,
                list.x + 2,
                y + 1 + row as u16,
                &line,
                fgbg(pal.popup_dim, pal.popup_bg),
                list.right(),
            );
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

/// Show text with chars `[start, end)` highlighted, clipped at `max_x`.
/// Leading whitespace is trimmed, and long lines scroll horizontally so the
/// match stays in view.
#[allow(clippy::too_many_arguments)]
fn put_hit_text(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    text: &str,
    start: usize,
    end: usize,
    base: ratatui::style::Style,
    hl: ratatui::style::Style,
    max_x: u16,
) {
    let avail = max_x.saturating_sub(x) as usize;
    if avail == 0 {
        return;
    }
    let chars: Vec<char> = text
        .chars()
        .map(|c| if c == '\t' || c.is_control() { ' ' } else { c })
        .collect();
    let lead = chars
        .iter()
        .position(|c| !c.is_whitespace())
        .unwrap_or(chars.len());
    let mut from = lead;
    let mut s = start.saturating_sub(lead);
    let mut e = end.saturating_sub(lead);
    if s >= avail {
        let shift = s.saturating_sub(avail / 3);
        from = (lead + shift).min(chars.len());
        s -= shift;
        e = e.saturating_sub(shift);
    }
    let mut cx = x;
    for (i, ch) in chars.iter().enumerate().skip(from) {
        if cx >= max_x {
            break;
        }
        let off = i - from;
        let st = if off >= s && off < e { hl } else { base };
        let nx = put_char(buf, cx, y, *ch, st, max_x);
        cx = if nx > cx { nx } else { cx + 1 };
    }
}

/// Find/Replace in Files: inputs, options and a grouped results list.
pub fn render_search(buf: &mut Buffer, editor: Rect, app: &mut App) -> Option<Position> {
    if !app.search.visible {
        return None;
    }
    let pal = app.theme.palette.clone();
    let nerd = app.settings.nerd_font;
    let compact = app.compact;
    let replace = app.search.replace;
    let rows = app.search_rows();
    let w = if compact {
        editor.width
    } else {
        (editor.width.saturating_mul(4) / 5)
            .clamp(50, 96)
            .min(editor.width.saturating_sub(4))
    };
    if w < 30 || editor.height < 7 {
        return None;
    }
    // rows: query, [replace], root, dirs, files, status, results; border.
    let fixed = 7 + u16::from(replace);
    let max_h = editor.height.saturating_sub(u16::from(!compact));
    if max_h <= fixed {
        return None;
    }
    let list_rows = (rows.len() as u16).clamp(1, 12).min(max_h - fixed);
    let h = fixed + list_rows;
    let x = if compact {
        editor.x
    } else {
        editor.x + (editor.width - w) / 2
    };
    let y = editor.y + u16::from(!compact);
    let r = Rect::new(x, y, w, h);
    panel(
        buf,
        r,
        pal.accent,
        pal.popup_bg,
        Some(if replace {
            "Replace in Files"
        } else {
            "Find in Files"
        }),
    );
    app.layout.search_box = r;
    let bg = pal.popup_bg;
    let focused = app.focus == Focus::Search;
    let mut caret = None;

    // ------------------------------------------------------------ query row
    let qy = r.y + 1;
    put(buf, r.x + 2, qy, "⌕", bold(fgbg(pal.accent, bg)), r.right());
    let opts_w = 9;
    let qx = r.x + 4;
    let qw = r.right().saturating_sub(2 + opts_w).saturating_sub(qx);
    let c = input(
        buf,
        Rect::new(qx, qy, qw, 1),
        &app.search.query,
        app.search.query_cursor,
        "Search",
        pal.popup_fg,
        pal.popup_dim,
        pal.input_bg,
    );
    if focused && app.search.field == crate::app::SearchField::Query {
        caret = Some(c);
    }
    if app.search.query_selected && !app.search.query.is_empty() {
        let qn = width(&app.search.query).min(qw.saturating_sub(1));
        for i in 0..qn {
            super::util::set_bg(buf, qx + i, qy, pal.selection);
        }
    }
    app.layout
        .search_fields
        .push((Rect::new(r.x + 1, qy, r.width - 2, 1), 0));
    let mut bx = r.x + 4 + qw + 1;
    for (i, (label, on)) in [
        ("Aa", app.search.case_sensitive),
        ("ab", app.search.whole_word),
        (".*", app.search.regex),
    ]
    .iter()
    .enumerate()
    {
        let st = if *on {
            bold(fgbg(pal.accent_fg, pal.accent))
        } else {
            fgbg(pal.popup_dim, pal.input_bg)
        };
        put(buf, bx, qy, label, st, r.right());
        app.layout
            .search_buttons
            .push((Rect::new(bx, qy, 3, 1), i as u8));
        bx += 3;
    }

    // ---------------------------------------------------------- replace row
    let ry = r.y + 2;
    if replace {
        put(buf, r.x + 2, ry, "⇄", bold(fgbg(pal.accent, bg)), r.right());
        let btn_w = 12;
        let rw = r.right().saturating_sub(2 + btn_w).saturating_sub(r.x + 4);
        let c = input(
            buf,
            Rect::new(r.x + 4, ry, rw, 1),
            &app.search.replacement,
            app.search.replace_cursor,
            "Replace with ($1, $2 for groups)",
            pal.popup_fg,
            pal.popup_dim,
            pal.input_bg,
        );
        if focused && app.search.field == crate::app::SearchField::Replace {
            caret = Some(c);
        }
        app.layout
            .search_fields
            .push((Rect::new(r.x + 1, ry, r.width - 2, 1), 1));
        let btn_x = r.right().saturating_sub(1 + btn_w);
        let st = fgbg(pal.popup_fg, mix(bg, pal.popup_fg, 0.12));
        put(buf, btn_x, ry, "⇄ All", st, r.right());
        app.layout
            .search_buttons
            .push((Rect::new(btn_x, ry, 5, 1), 3));
        put(buf, btn_x + 6, ry, "⇄ File", st, r.right());
        app.layout
            .search_buttons
            .push((Rect::new(btn_x + 6, ry, 6, 1), 4));
    }

    // ------------------------------------------------------------ root row
    let root_y = r.y + 2 + u16::from(replace);
    put(
        buf,
        r.x + 2,
        root_y,
        "⌂",
        bold(fgbg(pal.accent, bg)),
        r.right(),
    );
    let project = crate::search::root_label(&app.explorer.root);
    let root_hint = if compact {
        project.clone()
    } else {
        format!("search in: {project}")
    };
    let c = input(
        buf,
        Rect::new(r.x + 4, root_y, r.width.saturating_sub(6), 1),
        &app.search.root,
        app.search.root_cursor,
        &root_hint,
        if app.search.missing_root {
            pal.error
        } else {
            pal.popup_fg
        },
        pal.popup_dim,
        pal.input_bg,
    );
    if focused && app.search.field == crate::app::SearchField::Root {
        caret = Some(c);
    }
    app.layout
        .search_fields
        .push((Rect::new(r.x + 1, root_y, r.width - 2, 1), 2));

    // ------------------------------------------------------------ dirs row
    let dyy = root_y + 1;
    put(
        buf,
        r.x + 2,
        dyy,
        "▤",
        bold(fgbg(pal.accent, bg)),
        r.right(),
    );
    let dirs_hint = if compact {
        "src;!bin;!obj"
    } else {
        "dirs: src;!bin;!obj   (plain = only these, ! = skip)"
    };
    let c = input(
        buf,
        Rect::new(r.x + 4, dyy, r.width.saturating_sub(6), 1),
        &app.search.dirs,
        app.search.dirs_cursor,
        &dirs_hint,
        pal.popup_fg,
        pal.popup_dim,
        pal.input_bg,
    );
    if focused && app.search.field == crate::app::SearchField::Dirs {
        caret = Some(c);
    }
    app.layout
        .search_fields
        .push((Rect::new(r.x + 1, dyy, r.width - 2, 1), 3));

    // ----------------------------------------------------------- files row
    let fy = dyy + 1;
    put(buf, r.x + 2, fy, "⚑", bold(fgbg(pal.accent, bg)), r.right());
    let fw = r.width.saturating_sub(6);
    let filter_hint = if compact {
        "*.cs;!*.csproj"
    } else {
        "files: *.cs;!*.csproj   (globs, ! excludes)"
    };
    let c = input(
        buf,
        Rect::new(r.x + 4, fy, fw, 1),
        &app.search.filter,
        app.search.filter_cursor,
        &filter_hint,
        pal.popup_fg,
        pal.popup_dim,
        pal.input_bg,
    );
    if focused && app.search.field == crate::app::SearchField::Filter {
        caret = Some(c);
    }
    app.layout
        .search_fields
        .push((Rect::new(r.x + 1, fy, r.width - 2, 1), 4));

    // ----------------------------------------------------------- status row
    let sy = fy + 1;
    let status = {
        let results = &app.search.results;
        if app.search.error {
            "Invalid pattern".to_string()
        } else if app.search.missing_root {
            format!(
                "No such folder: {}",
                crate::search::root_label(&app.search.root_path)
            )
        } else if app.search.query.is_empty() {
            "Type a query to search every file in the project".to_string()
        } else if let Some(error) = results.warnings.first() {
            format!("Search incomplete: {error}")
        } else if results.matches == 0 {
            format!(
                "No results · {} files scanned (check root and filters)",
                results.scanned
            )
        } else {
            let mut s = format!(
                "{} match{} in {} file{}",
                results.matches,
                if results.matches == 1 { "" } else { "es" },
                results.files.len(),
                if results.files.len() == 1 { "" } else { "s" }
            );
            if results.truncated {
                s.push_str(" · showing the first results");
            } else {
                s.push_str(&format!(" · {} scanned", results.scanned));
            }
            // When the search is rooted somewhere else, say where.
            if !app.search.root.trim().is_empty() {
                s.push_str(&format!(
                    " · in {}",
                    crate::search::root_label(&app.search.root_path)
                ));
            }
            s
        }
    };
    let sc = if app.search.error || app.search.missing_root {
        pal.error
    } else if !app.search.results.warnings.is_empty() {
        pal.warning
    } else {
        pal.popup_dim
    };
    let close_x = r.right().saturating_sub(2);
    put(
        buf,
        r.x + 3,
        sy,
        &ellipsize(&status, close_x.saturating_sub(r.x + 4) as usize),
        fgbg(sc, bg),
        close_x,
    );
    put(buf, close_x, sy, "×", fgbg(pal.popup_fg, bg), r.right());
    app.layout
        .search_buttons
        .push((Rect::new(close_x, sy, 1, 1), 5));

    // --------------------------------------------------------- results list
    let list = Rect::new(
        r.x + 1,
        sy + 1,
        r.width.saturating_sub(2),
        r.bottom().saturating_sub(1).saturating_sub(sy + 1),
    );
    app.layout.search_list = list;
    if rows.is_empty() {
        let msg = if app.search.error {
            "Fix the pattern to see results"
        } else if app.search.missing_root {
            "That folder does not exist - clear the root box to search the project"
        } else if app.search.query.is_empty() {
            "Filter with globs: dirs src;!bin   files *.cs;!*.csproj"
        } else {
            "No matches"
        };
        put(
            buf,
            list.x + 2,
            list.y,
            &ellipsize(msg, list.width.saturating_sub(4) as usize),
            fgbg(pal.popup_dim, bg),
            list.right(),
        );
        return focused.then_some(caret).flatten();
    }
    if list.height == 0 {
        return focused.then_some(caret).flatten();
    }
    // Keep the selected row on screen.
    let selected_row = app
        .search
        .selected
        .and_then(|(fi, mi)| rows.iter().position(|(f, m)| *f == fi && *m == Some(mi)));
    if let Some(sr) = selected_row {
        if sr < app.search.scroll {
            app.search.scroll = sr;
        } else if sr >= app.search.scroll + list.height as usize {
            app.search.scroll = sr + 1 - list.height as usize;
        }
    }
    let max_scroll = rows.len().saturating_sub(list.height as usize);
    app.search.scroll = app.search.scroll.min(max_scroll);

    for row in 0..list.height as usize {
        let idx = app.search.scroll + row;
        let Some(&(fi, mi)) = rows.get(idx) else {
            break;
        };
        let Some(f) = app.search.results.files.get(fi) else {
            continue;
        };
        let y = list.y + row as u16;
        match mi {
            None => {
                let hbg = mix(bg, pal.bg, 0.4);
                fill(buf, Rect::new(list.x, y, list.width, 1), hbg);
                let collapsed = app.search.collapsed.contains(&f.path);
                let mut x = put(
                    buf,
                    list.x + 1,
                    y,
                    if collapsed { "▸" } else { "▾" },
                    fgbg(pal.popup_dim, hbg),
                    list.right(),
                );
                let icon = icons::file_icon(&f.rel, nerd);
                x = put(
                    buf,
                    x + 1,
                    y,
                    icon.glyph,
                    fgbg(icon.color, hbg),
                    list.right(),
                );
                let name_w = (list.width as usize).saturating_sub(14);
                let label = ellipsize(&f.rel, name_w);
                put(
                    buf,
                    x + 1,
                    y,
                    &label,
                    bold(fgbg(pal.popup_fg, hbg)),
                    list.right(),
                );
                let count = format!("{} ", f.matches.len());
                put(
                    buf,
                    list.right().saturating_sub(width(&count) + 1),
                    y,
                    &count,
                    fgbg(pal.popup_dim, hbg),
                    list.right(),
                );
                app.layout
                    .search_files
                    .push((Rect::new(list.x, y, list.width, 1), fi));
            }
            Some(mi) => {
                let Some(hit) = f.matches.get(mi) else {
                    continue;
                };
                let selected = app.search.selected == Some((fi, mi));
                let hbg = if selected { pal.popup_selected } else { bg };
                fill(buf, Rect::new(list.x, y, list.width, 1), hbg);
                if selected {
                    put(buf, list.x, y, "▌", fgbg(pal.accent, hbg), list.right());
                }
                let lnum = format!("{:>5}", hit.line + 1);
                let mut x = put(
                    buf,
                    list.x + 2,
                    y,
                    &lnum,
                    fgbg(pal.gutter_fg, hbg),
                    list.right(),
                );
                x += 1;
                let mut right = list.right();
                if replace {
                    let btn = Rect::new(list.right().saturating_sub(2), y, 2, 1);
                    put(buf, btn.x, y, "⇄", fgbg(pal.popup_dim, hbg), list.right());
                    app.layout.search_replace_hits.push((btn, (fi, mi)));
                    right = btn.x.saturating_sub(1);
                }
                let hl = if selected {
                    bold(fgbg(pal.accent_fg, pal.accent))
                } else {
                    bold(fgbg(pal.match_fg, hbg))
                };
                put_hit_text(
                    buf,
                    x,
                    y,
                    &hit.text,
                    hit.start,
                    hit.end,
                    fgbg(pal.popup_fg, hbg),
                    hl,
                    right,
                );
                app.layout
                    .search_hits
                    .push((Rect::new(list.x, y, list.width, 1), (fi, mi)));
            }
        }
    }
    focused.then_some(caret).flatten()
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

pub fn render_toasts(buf: &mut Buffer, area: Rect, app: &App, top: bool) {
    let pal = &app.theme.palette;
    let mut y = if top { area.y + 1 } else { area.bottom() };
    for t in app.toasts.iter().rev() {
        let (icon, color) = match t.kind {
            ToastKind::Info => ("ℹ", pal.accent),
            ToastKind::Success => ("✔", pal.success),
            ToastKind::Error => ("✖", pal.error),
        };
        let text = ellipsize(&t.text, (area.width as usize).saturating_sub(10).min(70));
        let w = width(&text) + 7;
        if w > area.width || (!top && y < area.y + 4) || (top && y + 3 > area.bottom()) {
            break;
        }
        let ty = if top { y } else { y - 3 };
        y = if top { y + 3 } else { y - 3 };
        let x = area
            .right()
            .saturating_sub(w + 1 + app.layout.minimap.width + app.layout.scrollbar.width);
        let r = Rect::new(x, ty, w, 3);
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
