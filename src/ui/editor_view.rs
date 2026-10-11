//! The code view: gutter with change bars, syntax-colored text, indent
//! guides, bracket-pair colorization, selection, find and reference
//! highlights, a braille minimap and an overview-ruler scrollbar.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};

use super::util::{fill, put};
use crate::app::{App, Focus};
use crate::document::Indent;
use crate::editor::{Editor, LineChange, Seg, char_width};
use crate::highlight::LineHighlight;
use crate::theme::{Palette, mix};

const MINIMAP_WIDTH: u16 = 14;

#[allow(clippy::needless_range_loop)]
pub fn render(buf: &mut Buffer, area: Rect, app: &mut App) -> Option<Position> {
    let pal = app.theme.palette.clone();
    let settings_minimap = app.settings.minimap;
    let show_numbers = app.settings.line_numbers;
    let focus_editor = app.focus == Focus::Editor;
    app.update_find();
    let find_matches = if app.find.visible {
        app.find.matches.clone()
    } else {
        Vec::new()
    };
    let find_current = app.find.current;
    let indent_guides = app.settings.indent_guides;
    let render_ws = app.settings.whitespace;
    let compact = app.compact;
    let wrap = app.word_wrap();
    let bg = pal.bg;
    if app.editor()?.image.is_some() {
        render_image(buf, area, app);
        return None;
    }
    let e = app.editors.get_mut(app.active)?;

    // ---------------------------------------------------------------- layout
    let total = e.doc.len_lines();
    // Phones get a tight gutter: number, change bar, one space.
    let digits = total.to_string().len().max(if compact { 2 } else { 3 }) as u16;
    let pad = if compact { 0 } else { 1 };
    let gutter_w = if show_numbers {
        digits + 2 + 2 * pad
    } else {
        2 + pad
    };
    let minimap_w = if settings_minimap && !compact && area.width > 90 {
        MINIMAP_WIDTH
    } else {
        0
    };
    let gutter = Rect::new(area.x, area.y, gutter_w.min(area.width), area.height);
    let scrollbar = Rect::new(
        area.right().saturating_sub(1),
        area.y,
        1.min(area.width),
        area.height,
    );
    let minimap = Rect::new(
        scrollbar.x.saturating_sub(minimap_w),
        area.y,
        minimap_w,
        area.height,
    );
    let text = Rect::new(
        gutter.right(),
        area.y,
        minimap.x.saturating_sub(gutter.right()),
        area.height,
    );
    app.layout.gutter = gutter;
    app.layout.text = text;
    app.layout.minimap = minimap;
    app.layout.scrollbar = scrollbar;
    app.layout.editor = area;

    e.view_height = text.height as usize;
    e.view_width = text.width.saturating_sub(1) as usize;
    e.scroll_line = e.scroll_line.min(total.saturating_sub(1));
    let new_wrap = wrap.then(|| text.width.saturating_sub(1).max(8) as usize);
    if e.wrap_width != new_wrap {
        // Width changed (rotation, sidebar, toggle): keep the caret in view.
        e.wrap_width = new_wrap;
        e.scroll_seg = 0;
        e.scroll_col = 0;
        e.scroll_into_view();
    }

    let h = text.height as usize;
    let mm_lines = minimap.height as usize * 4;
    let mm_top = minimap_top(e, mm_lines);
    let hl_upto = (e.scroll_line + h).max(if minimap_w > 0 { mm_top + mm_lines } else { 0 });
    let hl_from = if minimap_w > 0 {
        e.scroll_line.min(mm_top)
    } else {
        e.scroll_line
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(30);
    e.hl_pending = !e.hl.ensure_view(
        &e.doc.rope,
        hl_from,
        hl_upto,
        &app.theme.syntax,
        bg,
        deadline,
    );

    fill(buf, area, bg);
    fill(buf, gutter, bg);

    let sel = e.sel;
    let block = e.block;
    let changes: Vec<LineChange> = e.line_changes().to_vec();
    let bracket_pair = e.bracket_pair();
    let word_refs = reference_ranges(e, h);
    let unit = match e.doc.indent {
        Indent::Spaces(n) => n,
        Indent::Tabs => e.doc.tab_width,
    }
    .max(1);
    let tab = e.tab();

    let rows = e.visual_rows(h);
    let mut levels = std::collections::HashMap::new();
    let active_guide = active_guide(e, unit);
    let (cur_line, cur_col) = e.cursor();
    let (cur_row, cur_x) = e.visual_x(cur_line, cur_col);
    let mut caret = None;

    for (row, &(line, k, seg, last)) in rows.iter().enumerate() {
        let y = text.y + row as u16;
        let is_cur = line == cur_line;
        let line_bg = if is_cur && sel.is_empty() {
            pal.line_highlight
        } else {
            bg
        };
        if line_bg != bg {
            fill(buf, Rect::new(text.x, y, text.width, 1), line_bg);
        }

        // Gutter: the number goes on the first row of a wrapped line.
        if show_numbers && k == 0 {
            let num = format!("{:>w$}", line + 1, w = digits as usize);
            let st = if is_cur {
                Style::default().fg(pal.gutter_fg_active)
            } else {
                Style::default().fg(pal.gutter_fg)
            };
            put(buf, gutter.x + pad, y, &num, st.bg(bg), gutter.right());
        } else if show_numbers && k > 0 {
            let x = gutter.right().saturating_sub(3 + pad);
            put(
                buf,
                x,
                y,
                "↪",
                Style::default().fg(pal.indent_guide).bg(bg),
                gutter.right(),
            );
        }
        let marker_x = gutter.right().saturating_sub(1 + pad);
        let marker = match changes.get(line).copied().unwrap_or(LineChange::None) {
            LineChange::Added => Some(("▎", pal.diff_added)),
            LineChange::Modified => Some(("▎", pal.diff_modified)),
            LineChange::DeletedAbove if k == 0 => Some(("▔", pal.diff_deleted)),
            _ => None,
        };
        if let Some((sym, c)) = marker {
            put(
                buf,
                marker_x,
                y,
                sym,
                Style::default().fg(c),
                gutter.right(),
            );
        }

        // Code.
        let line_text = e.doc.line_str(line);
        let line_start = e.doc.line_start(line);
        let lh = e.hl.line(line);
        let ctx = LineCtx {
            pal: &pal,
            lh,
            line,
            line_start,
            sel_range: (!sel.is_empty()).then(|| (sel.start(), sel.end())),
            finds: &find_matches,
            find_current,
            refs: &word_refs,
            bracket_pair,
            line_bg,
            render_ws,
            box_cols: block.and_then(|b| {
                let (t, btm) = b.lines();
                (line >= t && line <= btm).then(|| b.cols())
            }),
        };
        let origin = seg.dstart + e.scroll_col;
        draw_line(buf, text, y, &line_text, seg, origin, tab, &ctx);

        // Box selection past the end of a short line (virtual space), and the
        // column of carets for a zero-width box.
        if let Some((l, r)) = ctx.box_cols
            && last
        {
            let eol = crate::editor::display_col(&line_text, line_text.chars().count(), tab);
            let left_px = text.x as usize + seg.indent;
            let paint = |buf: &mut Buffer, dc: usize, color: Color| {
                if dc >= origin {
                    let x = left_px + dc - origin;
                    if x < text.right() as usize {
                        super::util::set_bg(buf, x as u16, y, color);
                    }
                }
            };
            for dc in eol.max(l)..r {
                paint(buf, dc, pal.selection);
            }
            if l == r && l >= eol {
                paint(buf, l, mix(pal.bg, pal.fg, 0.7));
            }
        }

        // Selection includes the newline: show one cell past EOL like VS.
        if let Some((s, en)) = ctx.sel_range {
            let eol = line_start + line_text.chars().count();
            if last && s <= eol && en > eol && line + 1 < total {
                let dc = crate::editor::display_col(&line_text, line_text.chars().count(), tab);
                if dc >= origin {
                    let x = text.x + (seg.indent + dc - origin) as u16;
                    if x < text.right() {
                        super::util::set_bg(buf, x, y, pal.selection);
                    }
                }
            }
        }

        // Indent guides (drawn through wrapped rows too).
        if indent_guides {
            let lvl = *levels
                .entry(line)
                .or_insert_with(|| level_of(e, line, unit));
            for g in 0..lvl {
                let dc = g * unit;
                if dc < e.scroll_col {
                    continue;
                }
                let x = text.x + (dc - e.scroll_col) as u16;
                if x >= text.right() {
                    break;
                }
                let active =
                    active_guide.is_some_and(|(ag, a, b)| ag == g && line >= a && line <= b);
                if let Some(cell) = buf.cell_mut((x, y))
                    && (cell.symbol() == " " || cell.symbol() == "·")
                {
                    cell.set_symbol("│");
                    cell.set_fg(if active {
                        pal.indent_guide_active
                    } else {
                        pal.indent_guide
                    });
                }
            }
        }

        if line == cur_line && k == cur_row && cur_x >= e.scroll_col {
            let x = text.x + (cur_x - e.scroll_col) as u16;
            if x < text.right() {
                caret = Some(Position::new(x, y));
            }
        }
    }

    if minimap_w > 0 {
        render_minimap(buf, minimap, e, &pal, mm_top);
    }
    render_scrollbar(buf, scrollbar, e, &pal, &find_matches, &changes);

    caret.filter(|_| focus_editor)
}

struct LineCtx<'a> {
    pal: &'a Palette,
    lh: Option<&'a LineHighlight>,
    line: usize,
    line_start: usize,
    sel_range: Option<(usize, usize)>,
    finds: &'a [(usize, usize)],
    find_current: Option<usize>,
    refs: &'a [(usize, usize)],
    bracket_pair: Option<((usize, usize), (usize, usize))>,
    line_bg: Color,
    render_ws: bool,
    /// Display-column range of a box selection on this line.
    box_cols: Option<(usize, usize)>,
}

/// Draw one visual row: the chars of `seg`, with display column `origin`
/// at the left edge (after the row's indent).
#[allow(clippy::too_many_arguments)]
fn draw_line(
    buf: &mut Buffer,
    area: Rect,
    y: u16,
    text: &str,
    seg: Seg,
    origin: usize,
    tab: usize,
    ctx: &LineCtx,
) {
    let left = area.x as usize + seg.indent;
    let pal = ctx.pal;
    let spans = ctx.lh.map(|l| l.spans.as_slice()).unwrap_or(&[]);
    let brackets = ctx.lh.map(|l| l.brackets.as_slice()).unwrap_or(&[]);
    let mut span_i = 0;
    let mut br_i = 0;
    let mut dcol = 0usize;
    // Find matches on this line start from the first one at or after line start.
    let first_find = ctx.finds.partition_point(|(_, e)| *e <= ctx.line_start);
    let mut find_i = first_find;

    for (col, ch) in text.chars().enumerate() {
        let w = char_width(ch, dcol, tab);
        if col < seg.start {
            dcol += w;
            continue;
        }
        if col >= seg.end || left + dcol >= origin + area.right() as usize {
            break;
        }
        let idx = ctx.line_start + col;

        while span_i < spans.len() && spans[span_i].end <= col {
            span_i += 1;
        }
        let mut style = Style::default().fg(pal.fg).bg(ctx.line_bg);
        if let Some(s) = spans.get(span_i).filter(|s| s.start <= col) {
            style = style.fg(s.fg);
            if s.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if s.italic {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if s.underline {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
        }
        // Bracket pair colorization.
        while br_i < brackets.len() && brackets[br_i].col < col {
            br_i += 1;
        }
        if let Some(b) = brackets.get(br_i).filter(|b| b.col == col) {
            style = style.fg(pal.brackets[b.depth % 3]);
        }

        // Backgrounds, lowest priority first.
        if ctx.refs.iter().any(|(s, e)| idx >= *s && idx < *e) {
            style = style.bg(pal.word_highlight);
        }
        while find_i < ctx.finds.len() && ctx.finds[find_i].1 <= idx {
            find_i += 1;
        }
        if let Some(&(s, e)) = ctx.finds.get(find_i)
            && idx >= s
            && idx < e
        {
            let current = ctx.find_current == Some(find_i);
            style = style.bg(if current {
                pal.find_match_current
            } else {
                pal.find_match
            });
        }
        if let Some(((l1, c1), (l2, c2))) = ctx.bracket_pair
            && ((l1 == ctx.line && c1 == col) || (l2 == ctx.line && c2 == col))
        {
            style = style.bg(pal.bracket_match).add_modifier(Modifier::BOLD);
        }
        let mut selected = ctx.sel_range.is_some_and(|(s, e)| idx >= s && idx < e);
        if let Some((l, r)) = ctx.box_cols {
            if dcol >= l && dcol < r {
                selected = true;
            } else if l == r && dcol == l {
                // One of the box's carets: draw it as a block cursor.
                style = style.bg(mix(pal.bg, pal.fg, 0.7)).fg(pal.bg);
            }
        }
        if selected {
            style = style.bg(pal.selection);
        }

        // Glyph.
        let show_ws = selected || ctx.render_ws;
        let glyph: String = match ch {
            '\t' => {
                if show_ws {
                    let mut s = "→".to_string();
                    s.push_str(&" ".repeat(w.saturating_sub(1)));
                    s
                } else {
                    " ".repeat(w)
                }
            }
            ' ' if show_ws => "·".into(),
            c if (c as u32) < 0x20 => char::from_u32(0x2400 + c as u32).unwrap_or('?').to_string(),
            c => c.to_string(),
        };
        if matches!(ch, ' ' | '\t') && show_ws {
            style = style.fg(pal.whitespace);
        }

        // Clip against horizontal scroll.
        if dcol + w > origin {
            if dcol >= origin {
                put(
                    buf,
                    (left + dcol - origin) as u16,
                    y,
                    &glyph,
                    style,
                    area.right(),
                );
            } else {
                // Partially scrolled-off wide char / tab: paint the visible part blank.
                let visible = dcol + w - origin;
                put(
                    buf,
                    left as u16,
                    y,
                    &" ".repeat(visible),
                    style,
                    area.right(),
                );
            }
        }
        dcol += w;
    }
}

/// Occurrences of the identifier under the caret in the visible region
/// (Visual Studio's "highlight references").
fn reference_ranges(e: &Editor, h: usize) -> Vec<(usize, usize)> {
    if !e.sel.is_empty() {
        return Vec::new();
    }
    let Some((s, en)) = e.word_at(e.sel.head) else {
        return Vec::new();
    };
    let word = e.doc.rope.slice(s..en).to_string();
    if word.chars().count() < 2 || word.chars().all(|c| c.is_ascii_digit()) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let last = (e.scroll_line + h).min(e.doc.len_lines());
    for line in e.scroll_line..last {
        let text = e.doc.line_str(line);
        let base = e.doc.line_start(line);
        let chars: Vec<char> = text.chars().collect();
        let wchars: Vec<char> = word.chars().collect();
        let n = wchars.len();
        let mut i = 0;
        while i + n <= chars.len() {
            if chars[i..i + n] == wchars[..] {
                let before = i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
                let after = i + n == chars.len()
                    || !(chars[i + n].is_alphanumeric() || chars[i + n] == '_');
                if before && after {
                    out.push((base + i, base + i + n));
                    i += n;
                    continue;
                }
            }
            i += 1;
        }
    }
    if out.len() < 2 { Vec::new() } else { out }
}

fn leading_width(text: &str, tab: usize) -> Option<usize> {
    if text.trim().is_empty() {
        return None;
    }
    let mut w = 0;
    for c in text.chars() {
        match c {
            ' ' => w += 1,
            '\t' => w += tab - w % tab,
            _ => break,
        }
    }
    Some(w)
}

fn level_of(e: &Editor, line: usize, unit: usize) -> usize {
    let tab = e.tab();
    if let Some(w) = leading_width(&e.doc.line_str(line), tab) {
        return w / unit;
    }
    // Blank line: inherit the deeper of the nearest non-blank neighbours'
    // levels, but only if both sides are indented (like VS).
    let total = e.doc.len_lines();
    let up = (0..line)
        .rev()
        .take(200)
        .find_map(|l| leading_width(&e.doc.line_str(l), tab));
    let down = (line + 1..total)
        .take(200)
        .find_map(|l| leading_width(&e.doc.line_str(l), tab));
    match (up, down) {
        (Some(a), Some(b)) => a.min(b) / unit,
        _ => 0,
    }
}

/// The guide for the block that contains the caret: (level, first line, last line).
fn active_guide(e: &Editor, unit: usize) -> Option<(usize, usize, usize)> {
    let total = e.doc.len_lines();
    let (cur, _) = e.cursor();
    let lvl = level_of(e, cur, unit);
    let next = if cur + 1 < total {
        level_of(e, cur + 1, unit)
    } else {
        0
    };
    let (guide, anchor) = if next > lvl {
        (lvl, cur + 1)
    } else if lvl > 0 {
        (lvl - 1, cur)
    } else {
        return None;
    };
    let mut a = anchor;
    while a > 0 && level_of(e, a - 1, unit) > guide && anchor - a < 2000 {
        a -= 1;
    }
    let mut b = anchor;
    while b + 1 < total && level_of(e, b + 1, unit) > guide && b - anchor < 2000 {
        b += 1;
    }
    Some((guide, a, b))
}

fn minimap_top(e: &Editor, mm_lines: usize) -> usize {
    let total = e.doc.len_lines();
    if total <= mm_lines {
        return 0;
    }
    let max_scroll = total.saturating_sub(e.view_height.max(1)).max(1);
    let ratio = (e.scroll_line as f32 / max_scroll as f32).clamp(0.0, 1.0);
    (ratio * (total - mm_lines) as f32) as usize
}

/// Braille-dot code overview: each cell is 2×4 dots, each dot covers 2 columns of one line.
#[allow(clippy::needless_range_loop)]
fn render_minimap(buf: &mut Buffer, area: Rect, e: &Editor, pal: &Palette, top: usize) {
    let bg = mix(pal.bg, pal.sidebar_bg, 0.35);
    fill(buf, area, bg);
    let total = e.doc.len_lines();
    let tab = e.tab();
    let cells = area.width.saturating_sub(1) as usize;
    let x0 = area.x + 1;
    let view = e.scroll_line..e.scroll_line + e.view_height;

    for row in 0..area.height as usize {
        let first = top + row * 4;
        if first >= total {
            break;
        }
        let in_view = (first..first + 4).any(|l| view.contains(&l));
        let row_bg = if in_view { pal.minimap_viewport } else { bg };
        let y = area.y + row as u16;
        if in_view {
            fill(buf, Rect::new(area.x, y, area.width, 1), row_bg);
        }
        // Expand up to 4 lines to cells of display columns.
        let mut grid: Vec<Vec<Option<Color>>> = Vec::with_capacity(4);
        for dy in 0..4 {
            let l = first + dy;
            let mut cols: Vec<Option<Color>> = vec![None; cells * 4];
            if l < total {
                let text = e.doc.line_str(l);
                let spans = e.hl.line(l).map(|h| h.spans.as_slice()).unwrap_or(&[]);
                let mut d = 0usize;
                let mut si = 0;
                for (ci, ch) in text.chars().enumerate() {
                    let w = char_width(ch, d, tab);
                    if d >= cols.len() {
                        break;
                    }
                    if !ch.is_whitespace() {
                        while si < spans.len() && spans[si].end <= ci {
                            si += 1;
                        }
                        let c = spans.get(si).map(|s| s.fg).unwrap_or(pal.fg);
                        cols[d] = Some(c);
                    }
                    d += w;
                }
            }
            grid.push(cols);
        }
        for cx in 0..cells {
            let mut bits = 0u32;
            let mut color = None;
            for dy in 0..4 {
                for dx in 0..2 {
                    let c0 = cx * 4 + dx * 2;
                    let hit = grid[dy][c0].or(grid[dy][c0 + 1]);
                    if let Some(c) = hit {
                        bits |= match (dx, dy) {
                            (0, 0) => 0x01,
                            (0, 1) => 0x02,
                            (0, 2) => 0x04,
                            (0, 3) => 0x40,
                            (1, 0) => 0x08,
                            (1, 1) => 0x10,
                            (1, 2) => 0x20,
                            _ => 0x80,
                        };
                        color.get_or_insert(c);
                    }
                }
            }
            if bits != 0 {
                let ch = char::from_u32(0x2800 + bits).unwrap_or(' ');
                let c = mix(color.unwrap_or(pal.fg), row_bg, 0.3);
                put(
                    buf,
                    x0 + cx as u16,
                    y,
                    &ch.to_string(),
                    Style::default().fg(c).bg(row_bg),
                    area.right(),
                );
            }
        }
    }
}

/// Scrollbar doubling as an overview ruler (find hits, changes, caret).
fn render_scrollbar(
    buf: &mut Buffer,
    area: Rect,
    e: &Editor,
    pal: &Palette,
    finds: &[(usize, usize)],
    changes: &[LineChange],
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let h = area.height as usize;
    let total = e.doc.len_lines().max(1);
    fill(buf, area, pal.scrollbar_track);
    let content = total + e.view_height.saturating_sub(1);
    // The thumb moves in eighths of a cell: partial cells use the eighth
    // blocks (▁▂▃▄▅▆▇) so it glides instead of jumping a row at a time.
    let h8 = h * 8;
    let len8 = ((h8 * e.view_height) / content.max(1)).clamp(8, h8);
    let pos8 = if total > 1 {
        e.scroll_line * (h8 - len8) / (total - 1).max(1)
    } else {
        0
    };
    const LOWER: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    for y in 0..h {
        let (c0, c1) = (y * 8, y * 8 + 8);
        let covered = c1.min(pos8 + len8).saturating_sub(c0.max(pos8));
        let Some(cell) = buf.cell_mut((area.x, area.y + y as u16)) else {
            continue;
        };
        if covered == 8 {
            cell.set_bg(pal.scrollbar_thumb);
        } else if covered > 0 {
            if pos8 > c0 {
                // Thumb starts inside this cell: fill its lower part.
                cell.set_symbol(LOWER[covered]).set_fg(pal.scrollbar_thumb);
            } else {
                // Thumb ends inside: track fills the lower part.
                cell.set_symbol(LOWER[8 - covered])
                    .set_fg(pal.scrollbar_track)
                    .set_bg(pal.scrollbar_thumb);
            }
        }
    }
    let row_of = |line: usize| (line * h / total).min(h - 1) as u16;
    let mark = |buf: &mut Buffer, row: u16, sym: &str, color: Color| {
        if let Some(c) = buf.cell_mut((area.x, area.y + row)) {
            c.set_symbol(sym);
            c.set_fg(color);
        }
    };
    for (i, ch) in changes.iter().enumerate() {
        let color = match ch {
            LineChange::Added => pal.diff_added,
            LineChange::Modified => pal.diff_modified,
            LineChange::DeletedAbove => pal.diff_deleted,
            LineChange::None => continue,
        };
        mark(buf, row_of(i), "▐", color);
    }
    let mut last_row = u16::MAX;
    for (s, _) in finds {
        let (line, _) = e.doc.pos(*s);
        let r = row_of(line);
        if r != last_row {
            mark(buf, r, "━", pal.find_match_current);
            last_row = r;
        }
    }
    let (cl, _) = e.cursor();
    mark(buf, row_of(cl), "━", pal.fg);
}

/// A picture tab: real pixels when the terminal supports a graphics
/// protocol, otherwise a high-resolution block mosaic; transparent parts
/// show a checkerboard.
fn render_image(buf: &mut Buffer, area: Rect, app: &mut App) {
    let pal = app.theme.palette.clone();
    fill(buf, area, pal.bg);
    let Some(e) = app.editor() else { return };
    let Some(tab) = &e.image else { return };
    let (img, (iw, ih), format, bytes) = (tab.img.clone(), tab.size, tab.format, tab.bytes);
    let key = format!(
        "{}:{}",
        e.doc
            .path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        app.theme.name
    );
    let size = if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1 << 20) as f64)
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    };
    let how = match app.graphics.protocol() {
        Some(p) => format!("{p} graphics"),
        None => app.graphics.glyphs.name().to_string(),
    };
    let info = format!(" {} · {iw}×{ih} · {size} · {how} ", format.to_uppercase());
    let ix = area.x + area.width.saturating_sub(super::util::width(&info)) / 2;
    put(
        buf,
        ix,
        area.bottom().saturating_sub(1),
        &info,
        Style::default().fg(pal.sidebar_dim).bg(pal.bg),
        area.right(),
    );
    let room = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(3),
    );
    // Natural size in cells (small icons are enlarged at most 8×).
    let (cw, chh) = app.graphics.cell_px.unwrap_or((10, 20));
    let natural = (
        ((iw * 8) / cw as u32).max(1).min(u16::MAX as u32) as u16,
        ((ih * 8) / chh as u32).max(1).min(u16::MAX as u32) as u16,
    );
    let rect = super::hires::fit(room, iw, ih, app.graphics.cell_aspect(), Some(natural));
    let light = super::hires::rgb(mix(pal.bg, pal.fg, 0.10));
    let dark = super::hires::rgb(mix(pal.bg, pal.fg, 0.04));
    let shown = app
        .graphics
        .draw_image(buf, rect, &key, |w, h| checkered(&img, w, h, (light, dark)));
    if !shown {
        let g = app.graphics.glyphs;
        // Photos keep more color with half blocks; mosaics suit line art.
        let g = if matches!(
            g,
            super::hires::Glyphs::Octant | super::hires::Glyphs::Sextant
        ) {
            g
        } else {
            super::hires::Glyphs::Half
        };
        app.graphics.draw_mosaic(buf, rect, &key, || {
            super::hires::image_canvas(&img, rect, g, (light, dark))
        });
    }
}

/// Scale a picture to `w×h` over a checkerboard (for terminal graphics).
fn checkered(
    img: &image::DynamicImage,
    w: u32,
    h: u32,
    check: (super::hires::Rgb, super::hires::Rgb),
) -> image::DynamicImage {
    let scaled = img
        .resize_exact(w.max(1), h.max(1), image::imageops::FilterType::Triangle)
        .to_rgba8();
    let sq = (w.max(h) / 48).max(8);
    let mut out = image::RgbaImage::new(w.max(1), h.max(1));
    for (x, y, p) in out.enumerate_pixels_mut() {
        let back = if ((x / sq) + (y / sq)).is_multiple_of(2) {
            check.0
        } else {
            check.1
        };
        let s = scaled.get_pixel(x, y).0;
        let c = super::hires::lerp(
            back,
            [s[0] as f32, s[1] as f32, s[2] as f32],
            s[3] as f32 / 255.0,
        );
        *p = image::Rgba([c[0] as u8, c[1] as u8, c[2] as u8, 255]);
    }
    image::DynamicImage::ImageRgba8(out)
}
