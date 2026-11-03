//! Dockable window system.
//!
//! Windows are floating (or docked) boxes that can hold any component —
//! terminals first and foremost. Each window has a title bar, a focus ring,
//! a z-order, and can be docked to an edge of the screen or floated freely.
//! Windows are rendered after the main content so they float on top, and
//! focus cycles between them with the keyboard.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::neon::{render_creature, Neon};
use crate::terminal::Terminal;
use crate::theme::{mix, rgb};
use crate::ui::util::{ellipsize, fill, put, width};

/// A dockable window.
#[derive(Clone, Debug)]
pub struct Window {
    pub id: u64,
    pub kind: WindowKind,
    /// Title shown in the window's title bar.
    pub title: String,
    /// Screen region.
    pub rect: Rect,
    /// Whether the window is docked to an edge (vs floating).
    pub docked: bool,
    /// Which edge it is docked to (meaningful when docked).
    pub dock_edge: DockEdge,
    /// Focus ring color source.
    pub focused: bool,
    /// Render order (higher = on top).
    pub z: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Terminal(u64), // references Terminal.id
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockEdge {
    None,
    Top,
    Bottom,
    Left,
    Right,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            id: 0,
            kind: WindowKind::Terminal(0),
            title: String::new(),
            rect: Rect::default(),
            docked: false,
            dock_edge: DockEdge::None,
            focused: false,
            z: 0,
        }
    }
}

impl Window {
    pub fn terminal(terminal: &Terminal) -> Self {
        Self {
            id: terminal.id,
            kind: WindowKind::Terminal(terminal.id),
            title: terminal.title.clone(),
            rect: terminal.rect,
            docked: terminal.docked,
            dock_edge: if terminal.docked {
                DockEdge::Bottom
            } else {
                DockEdge::None
            },
            focused: terminal.focused,
            z: terminal.z,
        }
    }
}

/// Manages the set of dockable windows.
#[derive(Clone, Debug, Default)]
pub struct Dock {
    pub windows: Vec<Window>,
    pub focused: Option<u64>,
}

impl Dock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_terminal(&mut self, terminal: &Terminal) {
        self.windows.push(Window::terminal(terminal));
        self.focused = Some(terminal.id);
    }

    pub fn focused_window(&self) -> Option<&Window> {
        self.focused.and_then(|id| self.windows.iter().find(|w| w.id == id))
    }

    pub fn focused_window_mut(&mut self) -> Option<&mut Window> {
        self.focused.and_then(|id| self.windows.iter_mut().find(move |w| w.id == id))
    }

    /// Focus the next window (cycling).
    pub fn focus_next(&mut self) {
        if self.windows.is_empty() {
            self.focused = None;
            return;
        }
        let cur = self.focused.and_then(|id| self.windows.iter().position(|w| w.id == id));
        let next = match cur {
            Some(i) => (i + 1) % self.windows.len(),
            None => 0,
        };
        self.focused = Some(self.windows[next].id);
    }

    /// Focus the previous window.
    pub fn focus_prev(&mut self) {
        if self.windows.is_empty() {
            self.focused = None;
            return;
        }
        let cur = self.focused.and_then(|id| self.windows.iter().position(|w| w.id == id));
        let next = match cur {
            Some(0) => self.windows.len() - 1,
            Some(i) => i - 1,
            None => self.windows.len() - 1,
        };
        self.focused = Some(self.windows[next].id);
    }

    /// Bring the focused window to the top (highest z).
    pub fn raise_focused(&mut self) {
        if let Some(id) = self.focused {
            let max_z = self.windows.iter().map(|w| w.z).max().unwrap_or(0);
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                w.z = max_z + 1;
            }
            self.windows.sort_by_key(|w| w.z);
        }
    }

    /// Dock/undock the focused window to the given edge.
    pub fn dock_focused(&mut self, edge: DockEdge) {
        let Some(id) = self.focused else { return };
        let currently = self.focused_window().map(|w| w.dock_edge).unwrap_or(DockEdge::None);
        let toggling_off = edge == DockEdge::None || edge == currently;
        for w in &mut self.windows {
            if w.id == id {
                w.docked = !toggling_off && edge != DockEdge::None;
                w.dock_edge = if w.docked { edge } else { DockEdge::None };
                break;
            }
        }
    }

    /// Apply docking geometry: snap docked windows to screen edges.
    pub fn apply_docking(&mut self, area: Rect) {
        for w in self.windows.iter_mut() {
            if !w.docked {
                continue;
            }
            let ww = w.rect.width.min(area.width);
            let wh = w.rect.height.min(area.height);
            let r = match w.dock_edge {
                DockEdge::Bottom => Rect::new(
                    area.x,
                    area.bottom().saturating_sub(wh),
                    ww,
                    wh,
                ),
                DockEdge::Right => Rect::new(
                    area.right().saturating_sub(ww),
                    area.y,
                    ww,
                    wh,
                ),
                DockEdge::Left => Rect::new(area.x, area.y, ww, wh),
                DockEdge::Top => Rect::new(area.x, area.y, ww, wh),
                DockEdge::None => w.rect,
            };
            w.rect = r;
        }
    }

    /// Sync window focus flags from the terminal list (terminals own focus).
    pub fn sync_focus_from_terminals(&mut self, terminals: &[Terminal]) {
        for w in &mut self.windows {
            let WindowKind::Terminal(tid) = w.kind;
            w.focused = terminals.iter().any(|t| t.id == tid && t.focused);
            if w.focused {
                self.focused = Some(w.id);
            }
        }
    }
}

/// Render a dockable window's title bar + border into the buffer.
pub fn render_window_frame(buf: &mut Buffer, window: &Window, neon: &Neon, theme_bg: Color) {
    let r = window.rect;
    if r.width < 4 || r.height < 3 {
        return;
    }
    let accent = if window.focused {
        neon.neon_color()
    } else {
        mix(theme_bg, Color::White, 0.15)
    };
    let title_bg = if window.focused { accent } else { mix(theme_bg, Color::Black, 0.35) };

    // Title bar background.
    for x in r.left()..r.right() {
        if let Some(cell) = buf.cell_mut((x, r.y)) {
            cell.set_bg(title_bg);
            cell.set_fg(if window.focused {
                mix(accent, Color::Black, 0.75)
            } else {
                mix(Color::White, theme_bg, 0.6)
            });
            cell.set_symbol(" ");
        }
    }

    // Title text, centered-ish.
    let title = format!(" {} ", window.title);
    put(
        buf,
        r.x + 1,
        r.y,
        &title,
        Style::default()
            .fg(if window.focused { Color::Black } else { Color::White })
            .bg(title_bg)
            .add_modifier(Modifier::BOLD),
        r.right(),
    );

    // Controls on the right side of the title bar.
    let controls = if window.focused { " ✕⚓" } else { "  " };
    let cx = r.right().saturating_sub(width(controls) as u16).max(r.x + 1);
    put(buf, cx, r.y, controls, fgbg(title_bg, title_bg), r.right());

    // Border around the window (below title bar, above bottom, sides).
    for x in r.left()..r.right() {
        let under_title = r.y.saturating_add(1);
        if under_title < r.bottom() {
            if let Some(c) = buf.cell_mut((x, under_title)) {
                c.set_symbol("─");
                c.set_fg(accent);
            }
        }
        let by = r.bottom().saturating_sub(1);
        if by > r.y && by != under_title {
            if let Some(c) = buf.cell_mut((x, by)) {
                c.set_symbol("─");
                c.set_fg(accent);
            }
        }
    }
    for y in (r.y.saturating_add(2))..r.bottom().saturating_sub(1) {
        if let Some(c) = buf.cell_mut((r.left(), y)) {
            c.set_symbol("│");
            c.set_fg(accent);
        }
        let rx = r.right().saturating_sub(1);
        if rx > r.left() && let Some(c) = buf.cell_mut((rx, y)) {
            c.set_symbol("│");
            c.set_fg(accent);
        }
    }
    // Corners.
    let corners = [
        (r.left(), r.y.saturating_add(1), "├"),
        (r.right().saturating_sub(1), r.y.saturating_add(1), "┤"),
        (r.left(), r.bottom().saturating_sub(1), "└"),
        (r.right().saturating_sub(1), r.bottom().saturating_sub(1), "┘"),
    ];
    for (x, y, sym) in corners {
        if x < r.right() && y < r.bottom() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(sym);
                c.set_fg(accent);
            }
        }
    }

    // Focus indicator glow around focused windows.
    if window.focused {
        let glow = neon.neon_color();
        for dy in 0..2u16 {
            for dx in 0..2u16 {
                let corners = [
                    (r.left().saturating_add(dx), r.y.saturating_add(1).saturating_add(dy)),
                    (
                        r.right().saturating_sub(dx + 1),
                        r.y.saturating_add(1).saturating_add(dy),
                    ),
                    (
                        r.left().saturating_add(dx),
                        r.bottom().saturating_sub(dy + 1),
                    ),
                    (
                        r.right().saturating_sub(dx + 1),
                        r.bottom().saturating_sub(dy + 1),
                    ),
                ];
                for (x, y) in corners {
                    if x < r.right() && y < r.bottom() && x >= r.left() && y >= r.top() {
                        if let Some(c) = buf.cell_mut((x, y)) {
                            c.set_fg(glow);
                            c.modifier.insert(Modifier::BOLD);
                        }
                    }
                }
            }
        }
    }
}

/// Render a terminal's content into its window's content area.
pub fn render_terminal_content(
    buf: &mut Buffer,
    terminal: &Terminal,
    neon: &Neon,
) {
    let inner = terminal.inner_rect();
    let content = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(1), // reserve input line
    );
    if content.height == 0 || content.width == 0 {
        return;
    }

    // Background of the terminal content.
    let tbg = Color::Rgb(0x0a, 0x0e, 0x0d);
    fill(buf, content, tbg);

    // Subtle neon glow tint over terminal background.
    for y in content.top()..content.bottom() {
        for x in content.left()..content.right() {
            if let Some((strength, color)) = neon.glow(x, y, content) {
                if strength > 0.35 {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        let bg = cell.bg;
                        let (nr, ng, nb) = rgb(color);
                        let (br, bg_, bb) = rgb(bg);
                        let blend = |n: u8, b: u8| {
                            (n as f32 * strength * 0.5 + b as f32 * (1.0 - strength * 0.5))
                                .round() as u8
                        };
                        cell.set_bg(Color::Rgb(blend(nr, br), blend(ng, bg_), blend(nb, bb)));
                    }
                }
            }
        }
    }

    // Visible lines (oldest first); visible_lines already fits content.height.
    for (row, line) in terminal.visible_lines().into_iter().enumerate() {
        let y = content.top() + row as u16;
        let line = ellipsize(line, content.width as usize);
        put(
            buf,
            content.left(),
            y,
            &line,
            fgbg(Color::Rgb(0xc8, 0xf7, 0xdc), tbg),
            content.right(),
        );
    }

    // Input line at the bottom of the window.
    let input_y = inner.bottom().saturating_sub(1);
    if input_y > inner.top() {
        let line = format!("> {}", terminal.input);
        put(
            buf,
            content.left(),
            input_y,
            &line,
            fgbg(Color::White, tbg),
            content.right(),
        );
        if terminal.focused {
            let cursor_x = content.left() + width("> ") + width(&terminal.input[..terminal.input_cursor]);
            if cursor_x < content.right() {
                if let Some(c) = buf.cell_mut((cursor_x, input_y)) {
                    c.set_symbol(" ");
                    c.set_bg(Color::White);
                    c.set_fg(tbg);
                    c.modifier.insert(Modifier::BOLD);
                }
            }
        }
        // Living creature in the bottom-right corner of the terminal.
        render_creature(buf, inner, &terminal.creature, neon);
    }
}

fn fgbg(f: Color, b: Color) -> Style {
    Style::default().fg(f).bg(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_next_cycles() {
        let mut dock = Dock::new();
        dock.windows.push(Window::default());
        dock.windows.push(Window { id: 1, ..Default::default() });
        dock.focused = Some(0);
        dock.focus_next();
        assert_eq!(dock.focused, Some(1));
        dock.focus_next();
        assert_eq!(dock.focused, Some(0));
    }

    #[test]
    fn dock_bottom_snaps() {
        let mut dock = Dock::new();
        dock.windows.push(Window {
            id: 1,
            rect: Rect::new(10, 10, 30, 8),
            docked: true,
            dock_edge: DockEdge::Bottom,
            ..Default::default()
        });
        dock.apply_docking(Rect::new(0, 0, 100, 40));
        let w = &dock.windows[0];
        assert_eq!(w.rect.y, 40 - 8);
        assert_eq!(w.rect.x, 0);
    }
}
