//! Screen composition: title bar, explorer, tabs, breadcrumbs, editor,
//! status bar, plus floating overlays.

mod agent_panel;
mod editor_view;
mod image_canvas;
mod overlay;
pub mod util;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

use crate::app::{App, Focus, Latch, Layout, TOOLBAR_PAGES, Tool};
use crate::commands::Command;
use crate::dock::{render_terminal_content, render_window_frame};
use crate::icons;
use crate::neon::{render_aurora, render_creature};
use crate::theme::mix;
use util::{bold, ellipsize, fg, fgbg, fill, put, put_char, width};

pub fn draw(frame: &mut Frame, app: &mut App, truecolor: bool) {
    app.graphics.pending = None;
    let area = frame.area();
    app.layout = Layout::default();
    let buf = frame.buffer_mut();
    let pal = app.theme.palette.clone();
    app.set_compact(area.width < 80);
    fill(buf, area, pal.bg);
    // Slow-moving neon aurora behind everything.
    render_aurora(buf, area, &app.neon);
    // Living guardian creature in the title bar.
    render_creature(
        buf,
        Rect::new(area.x, area.y, area.width, 1),
        &app.neon_guardian,
        &app.neon,
    );
    if area.height < 4 || area.width < 20 {
        put(
            buf,
            0,
            0,
            "termed: window too small",
            fg(pal.fg),
            area.right(),
        );
        return;
    }

    let compact = app.compact;
    let toolbar_h = if app.toolbar_visible() && area.height >= 10 {
        2
    } else {
        0
    };
    let title = Rect::new(area.x, area.y, area.width, 1);
    let status = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    let toolbar = Rect::new(area.x, status.y - toolbar_h, area.width, toolbar_h);
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 2 - toolbar_h);

    // Wide screens dock the explorer; phones slide it over the editor.
    let sidebar_w = if app.show_sidebar && !compact {
        app.sidebar_width.min(body.width / 2)
    } else {
        0
    };
    let sidebar = Rect::new(body.x, body.y, sidebar_w, body.height);
    let main = Rect::new(
        body.x + sidebar_w,
        body.y,
        body.width - sidebar_w,
        body.height,
    );

    if compact {
        render_title_compact(buf, title, app);
    } else {
        render_title(buf, title, app);
    }
    if sidebar_w > 0 {
        render_sidebar(buf, sidebar, app);
    }

    // Agent panel: docked on the right when there is room, otherwise a
    // full-screen sheet drawn over everything (phones).
    let dock_w = (main.width * 2 / 5).clamp(36, 64);
    let docked = app.show_agent && !compact && main.width >= dock_w + 40;
    let agent_area = if docked {
        Rect::new(main.right() - dock_w, main.y, dock_w, main.height)
    } else {
        body
    };
    let main = if docked {
        Rect::new(main.x, main.y, main.width - dock_w, main.height)
    } else {
        main
    };
    let mut cursor = None;
    if app.editors.is_empty() {
        render_welcome(buf, main, app);
        // Project search works without an open file too.
        if app.search.visible
            && let Some(c) = overlay::render_search(buf, main, app)
        {
            cursor = Some(c);
        }
    } else {
        // On a phone the title bar already names the file: skip the tab row
        // for a single file, and always skip breadcrumbs.
        let tabs_h = u16::from(!compact || app.editors.len() > 1);
        let crumbs_h = u16::from(!compact);
        let tabs = Rect::new(main.x, main.y, main.width, tabs_h);
        let crumbs = Rect::new(main.x, main.y + tabs_h, main.width, crumbs_h);
        let editor = Rect::new(
            main.x,
            main.y + tabs_h + crumbs_h,
            main.width,
            main.height.saturating_sub(tabs_h + crumbs_h),
        );
        if tabs_h > 0 {
            render_tabs(buf, tabs, app);
        }
        if crumbs_h > 0 {
            render_breadcrumbs(buf, crumbs, app);
        }
        if app
            .editor()
            .is_some_and(|e| matches!(e.doc.preview, Some(crate::document::Preview::Image(_))))
        {
            image_canvas::render(buf, editor, app);
        } else {
            cursor = editor_view::render(buf, editor, app);
        }
        if app.find.visible
            && let Some(c) = overlay::render_find(buf, editor, app)
        {
            cursor = Some(c);
        }
        if app.search.visible
            && let Some(c) = overlay::render_search(buf, editor, app)
        {
            cursor = Some(c);
        }
    }
    if compact && app.show_sidebar {
        let w = body.width.saturating_sub(6).clamp(20.min(body.width), 40);
        let drawer = Rect::new(body.x, body.y, w, body.height);
        util::shadow(buf, Rect::new(drawer.right(), body.y, 1, body.height));
        render_sidebar(buf, drawer, app);
        app.layout.drawer = drawer;
        if app.focus == Focus::Explorer {
            cursor = None;
        }
    }
    if app.show_agent {
        let c = agent_panel::render(buf, agent_area, app);
        if app.focus == Focus::Agent {
            cursor = c;
        } else if !docked {
            cursor = None;
        }
    }
    if toolbar_h > 0 {
        render_toolbar(buf, toolbar, app);
    }
    render_status(buf, status, app);

    // Dockable windows (terminals float on top of everything).
    app.dock.apply_docking(area);
    // Windows are z-ascending: draw each frame with its content so upper windows occlude lower ones.
    for w in &app.dock.windows {
        let crate::dock::WindowKind::Terminal(tid) = w.kind;
        let Some(t) = app.terminals.iter_mut().find(|t| t.id == tid) else { continue };
        t.rect = w.rect;
        t.focused = w.focused;
        t.z = w.z;
        render_window_frame(buf, w, &app.neon, pal.bg);
        render_terminal_content(buf, t, &app.neon);
    }

    // On a phone with the agent sheet open, toasts go to the top so they
    // never cover the prompt box.
    let toasts_top = app.show_agent && !docked;
    overlay::render_toasts(
        buf,
        Rect::new(main.x, main.y, main.width, main.height),
        app,
        toasts_top,
    );
    if app.palette.is_some() || app.dialog.is_some() || app.search.visible || app.find.visible || !app.toasts.is_empty() {
        app.graphics.pending = None;
    }
    if app.palette.is_some() {
        cursor = overlay::render_palette(buf, area, app);
    }
    if app.dialog.is_some() {
        overlay::render_dialog(buf, area, app);
        cursor = None;
    }
    if !truecolor {
        util::downgrade_colors(frame.buffer_mut());
    }
    if let Some(c) = cursor {
        frame.set_cursor_position(c);
    }
}

fn render_title(buf: &mut Buffer, area: Rect, app: &mut App) {
    let pal = &app.theme.palette;
    fill(buf, area, pal.titlebar_bg);
    // Gradient logo.
    let logo = " ◆ termed ";
    let n = logo.chars().count().max(2) as f32;
    let mut x = area.x;
    for (i, ch) in logo.chars().enumerate() {
        let c = mix(pal.logo[0], pal.logo[1], i as f32 / (n - 1.0));
        x = put_char(
            buf,
            x,
            area.y,
            ch,
            bold(fgbg(c, pal.titlebar_bg)),
            area.right(),
        );
    }
    // Real entry points for search, plugins and persistent preferences.
    x += 1;
    for (m, command) in [
        ("File", Command::CommandPalette),
        ("Edit", Command::CommandPalette),
        ("View", Command::CommandPalette),
        ("Find", Command::Find),
        ("Plugins", Command::Plugins),
        ("Settings", Command::Settings),
    ] {
        let start = x;
        x = put(
            buf,
            x,
            area.y,
            &format!(" {m} "),
            fgbg(pal.sidebar_dim, pal.titlebar_bg),
            area.right(),
        );
        app.layout
            .click_items
            .push((Rect::new(start, area.y, x - start, 1), command));
    }
    // Centered search box.
    let label = match app.editor() {
        Some(e) => format!("{}  —  {}", e.doc.name(), app.explorer.root_name()),
        None => app.explorer.root_name(),
    };
    let right_w = if area.width >= 120 { 20 } else { 10 };
    let avail = area.right().saturating_sub(x + 2 + right_w);
    let w = (area.width / 3).clamp(24, 60).min(avail);
    let centered = area.x + (area.width.saturating_sub(w)) / 2;
    let bx = centered.max(x + 2);
    if w >= 16 {
        let r = Rect::new(bx, area.y, w, 1);
        fill(buf, r, pal.input_bg);
        let text = ellipsize(&format!("⌕  {label}"), w as usize - 2);
        put(
            buf,
            bx + 1,
            area.y,
            &text,
            fgbg(pal.sidebar_fg, pal.input_bg),
            r.right(),
        );
        app.layout.title_search = r;
    }
    // Right: agent button + window-ish controls.
    let right = if area.width >= 120 {
        " ─  □  ✕ "
    } else {
        ""
    };
    let rx = area.right().saturating_sub(width(right));
    let ai = " ✦ Agent ";
    let ax = rx.saturating_sub(width(ai) + 1);
    if ax >= x + 1 {
        let (abg, afg) = if app.show_agent {
            (pal.accent, pal.accent_fg)
        } else {
            (mix(pal.titlebar_bg, pal.accent, 0.3), pal.fg)
        };
        put(buf, ax, area.y, ai, bold(fgbg(afg, abg)), rx);
        app.layout
            .click_items
            .push((Rect::new(ax, area.y, width(ai), 1), Command::AgentToggle));
    }
    if rx > bx + w {
        put(
            buf,
            rx,
            area.y,
            right,
            fgbg(pal.sidebar_dim, pal.titlebar_bg),
            area.right(),
        );
    }
}

/// Phone title bar: big tap targets for the drawer, quick open and commands.
fn render_title_compact(buf: &mut Buffer, area: Rect, app: &mut App) {
    let pal = app.theme.palette.clone();
    fill(buf, area, pal.titlebar_bg);
    let btn = |buf: &mut Buffer, x: u16, label: &str, on: bool| {
        let bg = if on { pal.accent } else { pal.input_bg };
        let fgc = if on { pal.accent_fg } else { pal.fg };
        put(
            buf,
            x,
            area.y,
            &format!(" {label} "),
            bold(fgbg(fgc, bg)),
            area.right(),
        )
    };
    let x0 = area.x;
    let x = btn(buf, x0, "☰", app.show_sidebar);
    app.layout
        .click_items
        .push((Rect::new(x0, area.y, x - x0, 1), Command::ToggleSidebar));
    let right_w = 11;
    let rx = area.right().saturating_sub(right_w);
    // File name (tap = quick open), with a dirty dot.
    let (name, dirty) = match app.editor() {
        Some(e) => (e.doc.name(), e.doc.is_dirty()),
        None => (app.explorer.root_name(), false),
    };
    let nx = x + 1;
    let logo_w = if rx > nx + 20 { 9 } else { 0 };
    if logo_w > 0 {
        put(
            buf,
            nx,
            area.y,
            "◆ termed",
            bold(fgbg(pal.logo[0], pal.titlebar_bg)),
            rx,
        );
    }
    let fx = nx + logo_w;
    let room = rx.saturating_sub(fx + 2) as usize;
    let label = ellipsize(&name, room.saturating_sub(2));
    let end = put(
        buf,
        fx,
        area.y,
        &label,
        bold(fgbg(pal.tab_active_fg, pal.titlebar_bg)),
        rx,
    );
    if dirty {
        put(
            buf,
            end + 1,
            area.y,
            "●",
            fgbg(pal.warning, pal.titlebar_bg),
            rx,
        );
    }
    app.layout.title_search = Rect::new(fx, area.y, rx.saturating_sub(fx), 1);
    let ax = btn(buf, rx, "✦", app.show_agent);
    app.layout
        .click_items
        .push((Rect::new(rx, area.y, ax - rx, 1), Command::AgentToggle));
    let qx = ax + 1;
    let sx = btn(buf, qx, "⌕", false);
    app.layout
        .click_items
        .push((Rect::new(qx, area.y, sx - qx, 1), Command::QuickOpen));
    let ex = btn(buf, sx + 1, "⋯", false);
    app.layout.click_items.push((
        Rect::new(sx + 1, area.y, ex - sx - 1, 1),
        Command::CommandPalette,
    ));
}

/// Touch keyboard: a 2×9 grid of keys a soft keyboard lacks. Every cell has
/// a fixed position (muscle memory), cells share the width evenly, and the
/// modifier keys show their latch state.
fn render_toolbar(buf: &mut Buffer, area: Rect, app: &mut App) {
    let pal = app.theme.palette.clone();
    let bg = pal.titlebar_bg;
    fill(buf, area, bg);
    let page = &TOOLBAR_PAGES[app.toolbar_page % TOOLBAR_PAGES.len()];
    let gap: u16 = if area.width >= 45 { 1 } else { 0 };
    for (r, keys) in page.iter().enumerate() {
        let y = area.y + r as u16;
        if y >= area.bottom() {
            break;
        }
        let n = keys.len() as u16;
        let avail = area.width.saturating_sub(gap * (n - 1));
        let (base, extra) = (avail / n, avail % n);
        let mut x = area.x;
        for (i, k) in keys.iter().enumerate() {
            let w = base + u16::from((i as u16) < extra);
            let (state_bg, state_fg, underline) = match k.tool {
                Tool::Mod(m) => match app.mods.get(m) {
                    Latch::Off => (None, None, false),
                    // One-shot: tinted + underlined, "applies to the next key".
                    Latch::Once => (Some(mix(bg, pal.accent, 0.45)), Some(pal.fg), true),
                    // Locked: solid.
                    Latch::Locked => (Some(pal.accent), Some(pal.accent_fg), false),
                },
                Tool::BoxMode if app.mods.box_mode() => {
                    (Some(pal.warning), Some(pal.accent_fg), false)
                }
                Tool::Cmd(crate::commands::Command::ToggleSidebar) if app.show_sidebar => {
                    (Some(pal.accent), Some(pal.accent_fg), false)
                }
                Tool::Cmd(crate::commands::Command::ToggleWordWrap) if app.word_wrap() => {
                    (Some(mix(bg, pal.accent, 0.45)), Some(pal.fg), false)
                }
                Tool::Page => (Some(mix(bg, pal.accent, 0.25)), Some(pal.fg), false),
                _ => (None, None, false),
            };
            // Shade modifiers and arrows differently so the grid reads at a glance.
            let base_bg = match k.tool {
                Tool::Mod(_) | Tool::BoxMode => mix(bg, pal.fg, 0.16),
                Tool::Key(KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right) => {
                    mix(bg, pal.fg, 0.2)
                }
                _ => mix(bg, pal.fg, 0.1),
            };
            // Without gaps, alternate shades keep neighbouring keys distinct.
            let base_bg = if gap == 0 && (i + r) % 2 == 1 {
                mix(base_bg, pal.fg, 0.06)
            } else {
                base_bg
            };
            let bbg = state_bg.unwrap_or(base_bg);
            let bfg = state_fg.unwrap_or(pal.fg);
            let cell = Rect::new(x, y, w.min(area.right().saturating_sub(x)), 1);
            fill(buf, cell, bbg);
            let label = if width(k.label) <= w {
                k.label
            } else {
                k.short
            };
            let lx = x + w.saturating_sub(width(label)) / 2;
            let mut st = bold(fgbg(bfg, bbg));
            if underline {
                st = st.add_modifier(Modifier::UNDERLINED);
            }
            put(buf, lx, y, label, st, cell.right());
            app.layout.toolbar.push((cell, k.tool));
            x += w + gap;
        }
    }
}

fn render_sidebar(buf: &mut Buffer, area: Rect, app: &mut App) {
    let pal = app.theme.palette.clone();
    let nerd = app.settings.nerd_font;
    fill(buf, area, pal.sidebar_bg);
    let inner_right = area.right().saturating_sub(1);
    // Divider (also the resize handle).
    for y in area.top()..area.bottom() {
        put(
            buf,
            inner_right,
            y,
            "│",
            fgbg(pal.border, pal.sidebar_bg),
            area.right(),
        );
    }
    let focused = app.focus == Focus::Explorer;
    put(
        buf,
        area.x + 2,
        area.y,
        "EXPLORER",
        fgbg(pal.sidebar_dim, pal.sidebar_bg),
        inner_right,
    );
    put(
        buf,
        inner_right.saturating_sub(4),
        area.y,
        "···",
        fgbg(pal.sidebar_dim, pal.sidebar_bg),
        inner_right,
    );
    let root = format!("⌄ {}", app.explorer.root_name().to_uppercase());
    put(
        buf,
        area.x + 1,
        area.y + 1,
        &root,
        bold(fgbg(pal.sidebar_fg, pal.sidebar_bg)),
        inner_right,
    );

    let list = Rect::new(
        area.x,
        area.y + 2,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let scrollbar = Rect::new(
        list.right(),
        list.y,
        if area.width > 1 { 1 } else { 0 },
        list.height,
    );
    app.layout.sidebar = area;
    app.layout.sidebar_list = list;
    app.layout.sidebar_scrollbar = scrollbar;

    let active_path = app.editor().and_then(|e| e.doc.path.clone());
    let dirty: Vec<std::path::PathBuf> = app
        .editors
        .iter()
        .filter(|e| e.doc.is_dirty())
        .filter_map(|e| e.doc.path.clone())
        .collect();
    let ex = &app.explorer;
    for (row, entry) in ex
        .entries
        .iter()
        .skip(ex.scroll)
        .take(list.height as usize)
        .enumerate()
    {
        let i = ex.scroll + row;
        let y = list.y + row as u16;
        let selected = i == ex.selected;
        let is_active = active_path.as_ref() == Some(&entry.path);
        let row_bg = if selected && focused {
            pal.sidebar_selected
        } else if selected || is_active {
            mix(pal.sidebar_bg, pal.sidebar_selected, 0.45)
        } else {
            pal.sidebar_bg
        };
        if row_bg != pal.sidebar_bg {
            fill(buf, Rect::new(list.x, y, list.width, 1), row_bg);
        }
        if selected && focused {
            put(buf, list.x, y, "▎", fgbg(pal.accent, row_bg), list.right());
        }
        let mut x = list.x + 1;
        // Tree guides.
        for _ in 0..entry.depth {
            x = put(
                buf,
                x,
                y,
                " │",
                fgbg(mix(pal.sidebar_bg, pal.sidebar_fg, 0.18), row_bg),
                list.right(),
            );
        }
        x += 1;
        let (icon, name_style) = if entry.is_dir {
            let open = ex.expanded.contains(&entry.path);
            let chevron = if open { "⌄" } else { "›" };
            x = put(
                buf,
                x,
                y,
                chevron,
                fgbg(pal.sidebar_dim, row_bg),
                list.right(),
            );
            x += 1;
            if entry.is_archive {
                (
                    icons::file_icon(&entry.name, nerd),
                    fgbg(pal.sidebar_fg, row_bg),
                )
            } else {
                (icons::folder_icon(open, nerd), fgbg(pal.sidebar_fg, row_bg))
            }
        } else {
            x += 2;
            let fg_c = if is_active {
                pal.tab_active_fg
            } else {
                pal.sidebar_fg
            };
            (icons::file_icon(&entry.name, nerd), fgbg(fg_c, row_bg))
        };
        if !entry.is_dir || entry.is_archive || nerd {
            x = put(
                buf,
                x,
                y,
                icon.glyph,
                fgbg(icon.color, row_bg),
                list.right(),
            );
            x += 1;
        }
        let is_dirty = dirty.contains(&entry.path);
        let style = if is_dirty {
            name_style.fg(pal.warning)
        } else {
            name_style
        };
        let room = list.right().saturating_sub(
            x + if entry.is_archive {
                5
            } else if is_dirty {
                3
            } else {
                1
            },
        ) as usize;
        put(
            buf,
            x,
            y,
            &ellipsize(&entry.name, room),
            style,
            list.right(),
        );
        if entry.is_archive && list.width >= 8 {
            put(
                buf,
                list.right().saturating_sub(4),
                y,
                "ZIP",
                fgbg(pal.sidebar_dim, row_bg),
                list.right(),
            );
        }
        if is_dirty {
            put(
                buf,
                list.right().saturating_sub(2),
                y,
                "●",
                fgbg(pal.warning, row_bg),
                list.right(),
            );
        }
    }
    // A dedicated, draggable thumb makes the full tree reachable even when
    // mouse-wheel events are unavailable or the project has many entries.
    let total = ex.entries.len();
    let page = scrollbar.height as usize;
    if scrollbar.width > 0 && page > 0 && total > 0 {
        let thumb = if total <= page {
            page
        } else {
            (page * page / total).max(1)
        };
        let max_scroll = total.saturating_sub(page);
        let thumb_top = if max_scroll == 0 {
            0
        } else {
            ex.scroll.min(max_scroll) * page.saturating_sub(thumb) / max_scroll
        };
        for row in 0..page {
            let glyph = if (thumb_top..thumb_top + thumb).contains(&row) {
                "█"
            } else {
                "│"
            };
            put(
                buf,
                scrollbar.x,
                scrollbar.y + row as u16,
                glyph,
                fgbg(
                    if glyph == "█" { pal.accent } else { pal.sidebar_dim },
                    pal.sidebar_bg,
                ),
                scrollbar.right(),
            );
        }
    }
}

fn render_tabs(buf: &mut Buffer, area: Rect, app: &mut App) {
    let pal = app.theme.palette.clone();
    let nerd = app.settings.nerd_font;
    fill(buf, area, pal.tabbar_bg);
    // Compute widths and scroll so the active tab is visible.
    let labels: Vec<(String, u16)> = app
        .editors
        .iter()
        .map(|e| {
            let name = ellipsize(&e.doc.name(), 28);
            let w = width(&name) + 7;
            (name, w)
        })
        .collect();
    let mut start = 0;
    loop {
        let used: u16 = labels[start..=app.active.max(start)]
            .iter()
            .map(|l| l.1)
            .sum();
        if used <= area.width || start >= app.active {
            break;
        }
        start += 1;
    }
    let mut x = area.x;
    let focused = app.focus == Focus::Editor;
    for (i, e) in app.editors.iter().enumerate().skip(start) {
        let (name, w) = &labels[i];
        if x >= area.right() {
            break;
        }
        let active = i == app.active;
        let bg = if active {
            pal.tab_active_bg
        } else {
            pal.tab_inactive_bg
        };
        let fgc = if active {
            pal.tab_active_fg
        } else {
            pal.tab_inactive_fg
        };
        let r = Rect::new(x, area.y, (*w).min(area.right() - x), 1);
        fill(buf, r, bg);
        let accent = if active && focused {
            pal.accent
        } else if active {
            pal.border
        } else {
            bg
        };
        let mut cx = put(buf, x, area.y, "▎", fgbg(accent, bg), r.right());
        let icon = icons::file_icon(&e.doc.name(), nerd);
        cx = put(
            buf,
            cx + 1,
            area.y,
            icon.glyph,
            fgbg(icon.color, bg),
            r.right(),
        );
        let style = if active {
            bold(fgbg(fgc, bg))
        } else {
            fgbg(fgc, bg)
        };
        let style = if e.doc.path.is_none() {
            style.add_modifier(Modifier::ITALIC)
        } else {
            style
        };
        cx = put(buf, cx + 1, area.y, name, style, r.right());
        let close_x = cx + 1;
        let (sym, c) = if e.doc.is_dirty() {
            (
                "●",
                if active {
                    pal.tab_active_fg
                } else {
                    pal.tab_inactive_fg
                },
            )
        } else if active {
            ("×", pal.tab_inactive_fg)
        } else {
            (" ", bg)
        };
        put(buf, close_x, area.y, sym, fgbg(c, bg), r.right());
        // Separator between inactive tabs.
        if !active && r.right() < area.right() {
            put(
                buf,
                r.right() - 1,
                area.y,
                "│",
                fgbg(mix(pal.tabbar_bg, pal.border, 0.6), bg),
                area.right(),
            );
        }
        let close = if app.compact {
            Rect::new(close_x.saturating_sub(1), area.y, 3, 1)
        } else {
            Rect::new(close_x, area.y, 1, 1)
        };
        app.layout.tabs.push((r, close, i));
        x += w;
    }
}

fn render_breadcrumbs(buf: &mut Buffer, area: Rect, app: &mut App) {
    let pal = &app.theme.palette;
    fill(buf, area, pal.bg);
    let Some(e) = app.editor() else { return };
    let sep = fgbg(pal.sidebar_dim, pal.bg);
    let mut x = area.x + 2;
    let parts: Vec<String> = match &e.doc.path {
        Some(p) => p
            .strip_prefix(&app.explorer.root)
            .unwrap_or(p)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect(),
        None => vec![e.doc.name()],
    };
    let n = parts.len();
    for (i, part) in parts.iter().enumerate() {
        if i + 1 == n {
            let icon = icons::file_icon(part, app.settings.nerd_font);
            x = put(
                buf,
                x,
                area.y,
                icon.glyph,
                fgbg(icon.color, pal.bg),
                area.right(),
            );
            x = put(
                buf,
                x + 1,
                area.y,
                part,
                fgbg(pal.sidebar_fg, pal.bg),
                area.right(),
            );
        } else {
            x = put(buf, x, area.y, part, sep, area.right());
            x = put(buf, x, area.y, " › ", sep, area.right());
        }
    }
    // Enclosing scope hint: the nearest line above the caret with less indent
    // that looks like a declaration.
    if let Some(scope) = enclosing_scope(e) {
        x = put(buf, x, area.y, " › ", sep, area.right());
        put(
            buf,
            x,
            area.y,
            &ellipsize(&scope, 48),
            fgbg(pal.sidebar_dim, pal.bg),
            area.right(),
        );
    }
}

fn enclosing_scope(e: &crate::editor::Editor) -> Option<String> {
    let (line, _) = e.cursor();
    let cur_indent = |t: &str| t.chars().take_while(|c| c.is_whitespace()).count();
    let text = e.doc.line_str(line);
    let mut limit = if text.trim().is_empty() {
        usize::MAX
    } else {
        cur_indent(&text)
    };
    let keywords = [
        "class ",
        "struct ",
        "interface ",
        "enum ",
        "namespace ",
        "fn ",
        "def ",
        "func ",
        "function ",
        "impl ",
        "trait ",
        "mod ",
        "public ",
        "private ",
        "protected ",
        "internal ",
        "static ",
        "async ",
        "pub ",
        "void ",
        "export ",
    ];
    for l in (0..line).rev().take(3000) {
        let t = e.doc.line_str(l);
        if t.trim().is_empty() {
            continue;
        }
        let ind = cur_indent(&t);
        if ind < limit {
            limit = ind;
            let trimmed = t.trim();
            if keywords.iter().any(|k| trimmed.starts_with(k)) && !trimmed.ends_with(';') {
                let sig = trimmed.trim_end_matches('{').trim_end_matches(':').trim();
                return Some(sig.to_string());
            }
            if ind == 0 {
                break;
            }
        }
    }
    None
}

fn render_status(buf: &mut Buffer, area: Rect, app: &mut App) {
    let pal = app.theme.palette.clone();
    let (bg, fgc) = (pal.status_bg, pal.status_fg);
    fill(buf, area, bg);
    let dark = mix(bg, ratatui::style::Color::Black, 0.25);

    let compact = app.compact;
    // Left.
    let mut x = put(
        buf,
        area.x,
        area.y,
        " ⟩_ ",
        bold(fgbg(fgc, dark)),
        area.right(),
    );
    // Latched modifiers and modes, so it is always clear what the next key does.
    let m = app.mods;
    if let Some(b) = app.editor().and_then(|e| e.block) {
        let (t, bt) = b.lines();
        let (l, r) = b.cols();
        let label = if b.is_carets() {
            format!(" BOX {}× ", bt - t + 1)
        } else {
            format!(" BOX {}×{} ", bt - t + 1, r - l)
        };
        x = put(
            buf,
            x,
            area.y,
            &label,
            bold(fgbg(pal.accent_fg, pal.warning)),
            area.right(),
        );
    } else if m.box_mode() {
        x = put(
            buf,
            x,
            area.y,
            " BOX ",
            bold(fgbg(pal.accent_fg, pal.warning)),
            area.right(),
        );
    }
    if m.select_mode() {
        x = put(
            buf,
            x,
            area.y,
            " SELECT ",
            bold(fgbg(pal.accent_fg, pal.warning)),
            area.right(),
        );
    }
    if !m.box_mode() {
        for (name, latch) in [("CTRL", m.ctrl), ("ALT", m.alt), ("SHIFT", m.shift)] {
            let label = match latch {
                Latch::Once => format!(" {name}+ "),
                Latch::Locked if name != "SHIFT" => format!(" {name}■ "),
                _ => continue,
            };
            x = put(
                buf,
                x,
                area.y,
                &label,
                bold(fgbg(
                    pal.accent_fg,
                    mix(bg, ratatui::style::Color::Black, 0.45),
                )),
                area.right(),
            );
        }
    }
    x += 1;
    if let Some(b) = app.git_branch.as_ref().filter(|_| !compact) {
        let branch = format!("⎇ {b} ");
        x = put(
            buf,
            x,
            area.y,
            &branch,
            fgbg(pal.accent_fg, mix(bg, pal.accent, 0.55)),
            area.right(),
        ) + 2;
    }
    let dirty = app.editors.iter().filter(|e| e.doc.is_dirty()).count();
    let (state, state_fg) = if app.chord {
        (
            "(Ctrl+K) was pressed. Waiting for second key of chord...".to_string(),
            Some(pal.warning),
        )
    } else if dirty > 0 {
        (
            if compact {
                format!("● {dirty}")
            } else {
                format!("● {dirty} unsaved")
            },
            Some(pal.warning),
        )
    } else if compact {
        ("✓".to_string(), Some(pal.success))
    } else {
        ("✓ Ready".to_string(), Some(pal.success))
    };
    // Unsaved work and pending chords are warnings; a clean buffer is green.
    let state_style = bold(fgbg(state_fg.unwrap_or(fgc), bg));
    let left_end = x + width(&state);

    // Right: clickable segments grouped by colour. Low-priority ones drop out
    // on narrow screens, so the bar stays useful on a phone.
    let mut segs: Vec<Seg> = Vec::new();
    if let Some(e) = app.editor() {
        if let Some(preview) = &e.doc.preview {
            match preview {
                crate::document::Preview::Image(image) => {
                    let (w, h) = image.dimensions();
                    segs.push(Seg::new(
                        format!("{} · {w}×{h}", image.format),
                        SegTone::Locked,
                        9,
                    ));
                    segs.push(Seg::new("Read-only", SegTone::Locked, 8).click(Command::SaveAs));
                }
                crate::document::Preview::Archive(index) => {
                    segs.push(Seg::new(
                        format!("ZIP · {} entries", index.entries.len()),
                        SegTone::Locked,
                        9,
                    ));
                    segs.push(
                        Seg::new("Overview · Read-only", SegTone::Locked, 8)
                            .click(Command::SaveAs),
                    );
                }
                crate::document::Preview::Text(text) => {
                    segs.push(Seg::new(
                        format!("Preview · {} lines", text.lines().count()),
                        SegTone::Locked,
                        9,
                    ));
                    segs.push(Seg::new("Read-only", SegTone::Locked, 8).click(Command::SaveAs));
                }
            }
        } else {
            if e.doc
                .path
                .as_ref()
                .is_some_and(|p| crate::archive::member_path(p).is_some())
            {
                segs.push(Seg::new("ZIP member", SegTone::Locked, 6));
            }
            let (line, _) = e.cursor();
            let col = e.cursor_display_col() + 1;
            let sel_len = e.sel.end() - e.sel.start();
            let pos = match (compact, sel_len) {
                (true, 0) => format!("{}:{col}", line + 1),
                (true, n) => format!("{}:{col} ({n})", line + 1),
                (false, 0) => format!("Ln {}, Col {col}", line + 1),
                (false, n) => format!("Ln {}, Col {col} ({n} selected)", line + 1),
            };
            segs.push(Seg::new(pos, SegTone::Position, 10).click(Command::GotoLine));

            // File shape: characters, words and lines. Cheap enough to keep
            // live (it is the same rope the renderer already walks), and it is
            // what people reach for when writing rather than coding.
            let lines = e.doc.len_lines();
            let sel_or_doc = if sel_len > 0 {
                e.selected_text()
            } else {
                e.doc.rope.to_string()
            };
            let chars = sel_or_doc.chars().count();
            let words = sel_or_doc.split_whitespace().count();
            if compact {
                segs.push(Seg::new(format!("{lines}L"), SegTone::File, 5));
            } else if sel_len > 0 {
                // With a selection the counts describe the selection, so say so.
                segs.push(Seg::new(
                    format!("{lines} lines · {words} words · {chars} chars selected"),
                    SegTone::File,
                    7,
                ));
            } else {
                segs.push(Seg::new(
                    format!("{lines} lines · {words} words · {chars} chars"),
                    SegTone::File,
                    7,
                ));
            }

            // Bytes on disk versus bytes in the buffer: a quick sanity check
            // for line-ending and encoding surprises.
            let disk = e
                .doc
                .path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len());
            if let Some(bytes) = disk.filter(|_| !compact) {
                segs.push(Seg::new(human_bytes(bytes), SegTone::File, 2));
            }

            segs.push(
                Seg::new(e.doc.indent.label(), SegTone::Setting, 3)
                    .click(Command::ConvertIndentToSpaces),
            );
            segs.push(Seg::new("UTF-8", SegTone::File, 1));
            segs.push(
                Seg::new(e.doc.line_ending.label(), SegTone::Setting, 4)
                    .click(Command::ToggleLineEnding),
            );
            let lang = if compact {
                e.language().to_string()
            } else {
                format!("{{ }} {}", e.language())
            };
            segs.push(Seg::new(lang, SegTone::Setting, 8).click(Command::SelectLanguage));
        }
    }
    segs.push(
        Seg::new(format!("◐ {}", app.theme.name), SegTone::Setting, 2)
            .click(Command::SelectTheme),
    );
    let budget = area.right().saturating_sub(left_end + 1);
    while segs.iter().map(|s| width(&s.text) + 2).sum::<u16>() > budget && !segs.is_empty() {
        let min = segs
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| s.priority)
            .map(|(i, _)| i)
            .unwrap();
        segs.remove(min);
    }
    put(buf, x, area.y, &state, state_style, area.right());
    let total: u16 = segs.iter().map(|s| width(&s.text) + 2).sum();
    let mut rx = area.right().saturating_sub(total);
    for seg in segs {
        let start = rx;
        rx = put(buf, rx + 1, area.y, &seg.text, seg_style(&pal, seg.tone), area.right()) + 1;
        if let Some(c) = seg.cmd {
            app.layout
                .click_items
                .push((Rect::new(start, area.y, rx - start, 1), c));
        }
    }
}

/// Human-readable byte count for the status bar.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A status-bar segment. Colours group related information so the bar can be
/// read at a glance: position is neutral, language/file facts are warm,
/// settings that change on click are cool, and anything dirty is red.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SegTone {
    /// Cursor position and selection size.
    Position,
    /// File facts: language, encoding, line ending, indent.
    File,
    /// Something the user should notice: unsaved work, warnings.
    Alert,
    /// The file is read-only or a preview.
    Locked,
    /// Editor/app settings reached by clicking (theme and the like).
    Setting,
}

struct Seg {
    text: String,
    cmd: Option<Command>,
    /// Lower numbers are dropped first when the bar is narrow.
    priority: u8,
    tone: SegTone,
}

impl Seg {
    fn new(text: impl Into<String>, tone: SegTone, priority: u8) -> Self {
        Self {
            text: text.into(),
            cmd: None,
            priority,
            tone,
        }
    }

    fn click(mut self, cmd: Command) -> Self {
        self.cmd = Some(cmd);
        self
    }
}

/// Background for a segment: a tint of its tone over the status-bar colour, so
/// adjacent segments stay distinguishable without drawing borders.
fn seg_style(pal: &crate::theme::Palette, tone: SegTone) -> ratatui::style::Style {
    let (fg, tint) = match tone {
        SegTone::Position => (pal.status_fg, 0.0),
        SegTone::File => (pal.status_fg, 0.22),
        SegTone::Alert => (pal.status_fg, 0.0),
        SegTone::Locked => (pal.status_fg, 0.30),
        SegTone::Setting => (pal.status_fg, 0.12),
    };
    let accent = match tone {
        SegTone::File => pal.success,
        SegTone::Alert => pal.warning,
        SegTone::Locked => pal.accent,
        SegTone::Setting => pal.accent,
        SegTone::Position => pal.status_fg,
    };
    let bg = if tint > 0.0 {
        mix(pal.status_bg, accent, tint)
    } else {
        pal.status_bg
    };
    fgbg(fg, bg)
}

const LOGO: [&str; 6] = [
    "████████╗███████╗██████╗ ███╗   ███╗███████╗██████╗ ",
    "╚══██╔══╝██╔════╝██╔══██╗████╗ ████║██╔════╝██╔══██╗",
    "   ██║   █████╗  ██████╔╝██╔████╔██║█████╗  ██║  ██║",
    "   ██║   ██╔══╝  ██╔══██╗██║╚██╔╝██║██╔══╝  ██║  ██║",
    "   ██║   ███████╗██║  ██║██║ ╚═╝ ██║███████╗██████╔╝",
    "   ╚═╝   ╚══════╝╚═╝  ╚═╝╚═╝     ╚═╝╚══════╝╚═════╝ ",
];

const SMALL_LOGO: [&str; 2] = ["▀█▀ █▀▀ █▀█ █▀▄▀█ █▀▀ █▀▄", " █  ██▄ █▀▄ █ ▀ █ ██▄ █▄▀"];

fn render_welcome(buf: &mut Buffer, area: Rect, app: &mut App) {
    let pal = app.theme.palette.clone();
    fill(buf, area, pal.bg);
    let compact = app.compact;
    // Every row is also a tap target.
    let shortcuts: &[(&str, &str, Command)] = if compact {
        &[
            ("Ask the AI agent", "✦", Command::AgentToggle),
            ("Browse files", "☰", Command::ToggleSidebar),
            ("Open file…", "⌕", Command::QuickOpen),
            ("All commands…", "⋯", Command::CommandPalette),
            ("New file", "+", Command::NewFile),
            ("Color theme", "◐", Command::SelectTheme),
            ("Quit", "✕", Command::Quit),
        ]
    } else {
        &[
            ("Open File", "Ctrl+P", Command::QuickOpen),
            ("AI Agent (free)", "Ctrl+K A", Command::AgentToggle),
            (
                "Command Palette",
                "Ctrl+Shift+P · F1",
                Command::CommandPalette,
            ),
            ("New File", "Ctrl+N", Command::NewFile),
            ("Toggle Explorer", "Ctrl+B", Command::ToggleSidebar),
            ("Go to Line", "Ctrl+G", Command::GotoLine),
            ("Color Theme", "Ctrl+K T · F8", Command::SelectTheme),
            ("Quit", "Ctrl+Q", Command::Quit),
        ]
    };
    let big = area.width >= width(LOGO[0]) + 4 && area.height >= 20;
    let logo: &[&str] = if big { &LOGO } else { &SMALL_LOGO };
    let logo_w = width(logo[0]);
    let gap = if compact { 1 } else { 2 };
    let block_h = logo.len() as u16 + 2 + gap + shortcuts.len() as u16 * gap;
    let top = area.y + area.height.saturating_sub(block_h) / 2;
    if area.width >= logo_w + 2 {
        let lx = area.x + (area.width - logo_w) / 2;
        for (row, line) in logo.iter().enumerate() {
            for (i, ch) in line.chars().enumerate() {
                let t = (i as f32 / logo_w as f32 + row as f32 * 0.04).min(1.0);
                let c = mix(pal.logo[0], pal.logo[1], t);
                let c = if ch == '█' || !big {
                    c
                } else {
                    mix(c, pal.bg, 0.45)
                };
                put_char(
                    buf,
                    lx + i as u16,
                    top + row as u16,
                    ch,
                    fgbg(c, pal.bg),
                    area.right(),
                );
            }
        }
    }
    let tag = if area.width < 62 {
        "elite terminal code editor"
    } else {
        "the elite terminal code editor  ·  every language, one look"
    };
    let ty = top + logo.len() as u16 + 1;
    put(
        buf,
        area.x + area.width.saturating_sub(width(tag)) / 2,
        ty,
        tag,
        fgbg(pal.sidebar_dim, pal.bg),
        area.right(),
    );
    let col_w = 44u16.min(area.width.saturating_sub(4));
    let sx = area.x + area.width.saturating_sub(col_w) / 2;
    for (i, (label, keys, cmd)) in shortcuts.iter().enumerate() {
        let y = ty + 1 + gap + i as u16 * gap;
        if y >= area.bottom() {
            break;
        }
        let row = Rect::new(sx, y, col_w, 1);
        if compact {
            // Button-like rows for fingers.
            let bbg = mix(pal.bg, pal.fg, 0.07);
            fill(buf, row, bbg);
            put(
                buf,
                sx + 1,
                y,
                keys,
                bold(fgbg(pal.match_fg, bbg)),
                row.right(),
            );
            put(buf, sx + 4, y, label, fgbg(pal.fg, bbg), row.right());
            put(
                buf,
                row.right().saturating_sub(2),
                y,
                "›",
                fgbg(pal.sidebar_dim, bbg),
                row.right(),
            );
        } else {
            put(buf, sx, y, label, fgbg(pal.fg, pal.bg), sx + col_w);
            let kx = sx + col_w - width(keys);
            put(buf, kx, y, keys, fgbg(pal.match_fg, pal.bg), sx + col_w);
            let dots_from = sx + width(label) + 1;
            if kx > dots_from + 1 {
                let dots = "·".repeat((kx - dots_from - 1) as usize);
                put(
                    buf,
                    dots_from,
                    y,
                    &dots,
                    fgbg(mix(pal.bg, pal.fg, 0.2), pal.bg),
                    kx,
                );
            }
        }
        app.layout.click_items.push((row, *cmd));
    }
}
