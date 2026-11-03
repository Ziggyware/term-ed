//! Neon lighting and living creatures.
//!
//! Paints a slow-moving neon aurora behind the UI and gives every component a
//! small "creature" presence — pulsing borders, breathing indicators, and
//! critters that react to focus and activity. All rendering happens through
//! the existing ratatui cell buffer using Unicode block graphics hacks and
//! color blending, so no extra dependencies are needed.

use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use crate::theme::{mix, rgb};

/// Slow-moving neon lighting state. The hue rotates and a glow drifts across
/// the screen, casting a soft aurora over everything.
#[derive(Clone, Debug, Default)]
pub struct Neon {
    started: Instant,
}

impl Neon {
    pub fn new() -> Self {
        Self { started: Instant::now() }
    }

    /// Seconds since creation.
    pub fn t(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// Rotating hue in degrees (0..360).
    pub fn hue(&self) -> f32 {
        (self.t() * 40.0) % 360.0
    }

    /// Pulsing intensity in 0..1.
    pub fn intensity(&self) -> f32 {
        (self.t() * 2.5).sin() * 0.5 + 0.5
    }

    /// Glow center in 0..1 space, drifting slowly.
    pub fn glow_pos(&self) -> (f32, f32) {
        let t = self.t();
        ((t * 0.13).sin() * 0.45 + 0.5, (t * 0.09 + 1.2).cos() * 0.45 + 0.5)
    }

    /// Primary neon color at the current hue (saturated, bright).
    pub fn neon_color(&self) -> Color {
        hsv_to_rgb(self.hue(), 0.85, 1.0)
    }

    /// A softer secondary neon color, offset in hue.
    pub fn neon_color_2(&self) -> Color {
        hsv_to_rgb((self.hue() + 70.0) % 360.0, 0.7, 0.95)
    }

    /// Strength (0..1) and color of the neon glow at (x, y) within `area`.
    /// Returns None when the point is outside the glow radius.
    pub fn glow(&self, x: u16, y: u16, area: Rect) -> Option<(f32, Color)> {
        let (gx, gy) = self.glow_pos();
        let cx = area.x as f32 + gx * area.width.max(1) as f32;
        let cy = area.y as f32 + gy * area.height.max(1) as f32;
        let dx = (x as f32 - cx) / area.width.max(1) as f32;
        let dy = (y as f32 - cy) / area.height.max(1) as f32;
        let dist = (dx * dx + dy * dy).sqrt();
        let radius = 0.42 + 0.30 * self.intensity();
        if dist > radius {
            return None;
        }
        let strength = (1.0 - dist / radius) * self.intensity() * 0.65;
        Some((strength, self.neon_color()))
    }
}

/// Convert HSV to an RGB color.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Color {
    let h = h % 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let to_byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(to_byte(r), to_byte(g), to_byte(b))
}

/// Blend a neon tint over an existing cell, preserving its character.
pub fn tint_cell(cell: &mut ratatui::buffer::Cell, strength: f32, neon: Color, bg: Color) {
    let (nr, ng, nb) = rgb(neon);
    let (br, bg_, bb) = rgb(bg);
    let blend = |n: u8, b: u8| (n as f32 * strength + b as f32 * (1.0 - strength)).round() as u8;
    cell.set_bg(Color::Rgb(blend(nr, br), blend(ng, bg_), blend(nb, bb)));
    cell.set_fg(neon);
    cell.add_modifier(Modifier::BOLD);
}

/// The mood of a creature, driven by focus and activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mood {
    #[default]
    Idle,
    Active,
    Focused,
    Sleeping,
}

/// A small animated creature attached to a UI component.
#[derive(Clone, Debug)]
pub struct Creature {
    pub kind: CreatureKind,
    pub mood: Mood,
    pub pulse: f32,
    pub position: (f32, f32), // 0..1 relative to component
    pub wander: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreatureKind {
    /// Watches over the whole app from the title bar.
    Guardian,
    /// Lives in the editor and follows the caret.
    CursorCritter,
    /// Lives in a terminal at the prompt.
    ShellSpirit,
    /// Rustles in the sidebar tree.
    FolderFae,
    /// Reports health in the status bar.
    StatusSprite,
}

impl Creature {
    pub fn new(kind: CreatureKind) -> Self {
        let position = match kind {
            CreatureKind::Guardian => (0.92, 0.5),
            CreatureKind::CursorCritter => (0.5, 0.5),
            CreatureKind::ShellSpirit => (0.95, 0.85),
            CreatureKind::FolderFae => (0.08, 0.3),
            CreatureKind::StatusSprite => (0.05, 0.5),
        };
        Self {
            kind,
            mood: Mood::Idle,
            pulse: 0.0,
            position,
            wander: 0.0,
        }
    }

    /// Advance the creature's animation. `now` is seconds since app start.
    pub fn tick(&mut self, now: f32, active: bool, focused: bool) {
        self.mood = if focused {
            Mood::Focused
        } else if active {
            Mood::Active
        } else {
            Mood::Idle
        };
        let speed = match self.mood {
            Mood::Focused => 14.0,
            Mood::Active => 7.0,
            Mood::Idle => 2.5,
            Mood::Sleeping => 0.6,
        };
        self.pulse = (now * speed).sin() * 0.5 + 0.5;
        // Slow wandering drift so creatures feel alive even when idle.
        self.wander = (now * 0.7 + self.kind as f32 * 2.0).sin() * 0.08;
    }

    /// Glyph representing the creature in its current mood.
    pub fn glyph(&self) -> char {
        match (self.kind, self.mood) {
            (CreatureKind::Guardian, Mood::Focused) => "👁",
            (CreatureKind::Guardian, _) => "◆",
            (CreatureKind::CursorCritter, Mood::Focused) => "⬟",
            (CreatureKind::CursorCritter, Mood::Active) => "⬢",
            (CreatureKind::CursorCritter, _) => "●",
            (CreatureKind::ShellSpirit, Mood::Focused) => "⚡",
            (CreatureKind::ShellSpirit, Mood::Active) => "⟳",
            (CreatureKind::ShellSpirit, _) => "✦",
            (CreatureKind::FolderFae, Mood::Active) => "🌿",
            (CreatureKind::FolderFae, _) => "☘",
            (CreatureKind::StatusSprite, Mood::Focused) => "✦",
            (CreatureKind::StatusSprite, Mood::Active) => "✧",
            (CreatureKind::StatusSprite, _) => "•",
        }
        .chars()
        .next()
        .unwrap_or('•')
    }

    /// Color for the creature based on mood and the current neon hue.
    pub fn color(&self, neon: &Neon) -> Color {
        match self.mood {
            Mood::Focused => neon.neon_color(),
            Mood::Active => neon.neon_color_2(),
            Mood::Idle => mix(neon.neon_color(), Color::Black, 0.55),
            Mood::Sleeping => mix(neon.neon_color(), Color::Black, 0.8),
        }
    }
}

/// Render the moving neon aurora across the whole screen area.
pub fn render_aurora(buf: &mut Buffer, area: Rect, neon: &Neon) {
    if area.height < 3 || area.width < 4 {
        return;
    }
    // A slow horizontal aurora band that drifts up and down.
    let t = neon.t();
    let band_y = (area.y as f32 + (t * 0.25).sin() * (area.height.max(1) as f32) * 0.25) as u16;
    let band_y = band_y.clamp(area.top(), area.bottom().saturating_sub(1));
    let hue = neon.hue();
    let intensity = neon.intensity();

    // Draw aurora as semi-transparent horizontal bands using block characters.
    for offset in 0..3 {
        let y = band_y.saturating_add(offset);
        if y >= area.bottom() {
            break;
        }
        let band_hue = (hue + offset as f32 * 40.0) % 360.0;
        let band_color = hsv_to_rgb(band_hue, 0.9, 1.0);
        let strength = intensity * 0.35 * (1.0 - offset as f32 * 0.25);
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                let bg = cell.bg;
                let (nr, ng, nb) = rgb(band_color);
                let (br, bg_, bb) = rgb(bg);
                let blend = |n: u8, b: u8| {
                    (n as f32 * strength + b as f32 * (1.0 - strength)).round() as u8
                };
                cell.set_bg(Color::Rgb(blend(nr, br), blend(ng, bg_), blend(nb, bb)));
                if strength > 0.5 {
                    cell.set_fg(band_color);
                    cell.add_modifier(Modifier::BOLD);
                }
                cell.set_symbol("▓");
            }
        }
    }

    // Corner glows that pulse.
    for &(cx, cy) in &[
        (0.0, 0.0),
        (1.0, 0.0),
        (0.0, 1.0),
        (1.0, 1.0),
        neon.glow_pos(),
    ] {
        let sx = (area.x as f32 + cx * area.width.max(1) as f32) as u16;
        let sy = (area.y as f32 + cy * area.height.max(1) as f32) as u16;
        for dy in 0..4u16 {
            for dx in 0..6u16 {
                let x = if cx < 0.5 {
                    area.left().saturating_add(dx)
                } else {
                    area.right().saturating_sub(dx + 1)
                };
                let y = if cy < 0.5 {
                    area.top().saturating_add(dy)
                } else {
                    area.bottom().saturating_sub(dy + 1)
                };
                if x >= area.right() || y >= area.bottom() || x < area.left() || y < area.top() {
                    continue;
                }
                let dist = ((dx as f32 / 6.0) + (dy as f32 / 4.0)) / 2.0;
                let strength = (1.0 - dist).clamp(0.0, 1.0) * intensity * 0.5;
                if strength <= 0.05 {
                    continue;
                }
                if let Some(cell) = buf.cell_mut((x, y)) {
                    let bg = cell.bg;
                    let (nr, ng, nb) = rgb(neon.neon_color());
                    let (br, bg_, bb) = rgb(bg);
                    let blend = |n: u8, b: u8| {
                        (n as f32 * strength + b as f32 * (1.0 - strength)).round() as u8
                    };
                    cell.set_bg(Color::Rgb(blend(nr, br), blend(ng, bg_), blend(nb, bb)));
                }
            }
        }
    }
}

/// Render a single creature at a position relative to `area`.
pub fn render_creature(
    buf: &mut Buffer,
    area: Rect,
    creature: &Creature,
    neon: &Neon,
) {
    if area.width < 2 || area.height < 1 {
        return;
    }
    let x = (area.x as f32 + creature.position.0 * area.width.max(1) as f32
        + creature.wander * area.width.max(1) as f32) as u16;
    let y = (area.y as f32 + creature.position.1 * area.height.max(1) as f32) as u16;
    let x = x.clamp(area.left(), area.right().saturating_sub(1));
    let y = y.clamp(area.top(), area.bottom().saturating_sub(1));
    if let Some(cell) = buf.cell_mut((x, y)) {
        let c = creature.glyph();
        let mut encoded = [0; 4];
        let s = c.encode_utf8(&mut encoded);
        cell.set_symbol(s);
        cell.set_fg(creature.color(neon));
        cell.add_modifier(Modifier::BOLD);
        // A faint halo around active creatures.
        if creature.mood != Mood::Idle {
            cell.set_bg(mix(Color::Black, creature.color(neon), creature.pulse * 0.25));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neon_hue_wraps() {
        let n = Neon::new();
        assert!(n.hue() >= 0.0 && n.hue() < 360.0);
    }

    #[test]
    fn creature_glyph_changes_with_mood() {
        let mut c = Creature::new(CreatureKind::CursorCritter);
        c.mood = Mood::Idle;
        let idle = c.glyph();
        c.mood = Mood::Focused;
        assert_ne!(idle, c.glyph());
    }
}
