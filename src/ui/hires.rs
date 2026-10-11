//! High-resolution drawing inside the character grid.
//!
//! A terminal cell can show one glyph in two colors. Block-mosaic glyphs
//! split the cell into sub-pixels: half blocks (1×2), quadrants (2×2),
//! sextants (2×3, Unicode 13) and octants (2×4, Unicode 16). A [`Canvas`]
//! holds a picture at sub-pixel resolution; drawing it picks, for every
//! cell, the two colors that best represent its sub-pixels (a tiny 2-means
//! clustering) and the mosaic glyph whose shape matches. Edges keep partial
//! coverage in their colors, so shapes come out anti-aliased.
//!
//! Terminals that draw these glyphs themselves (kitty, ghostty, foot,
//! WezTerm, Windows Terminal) get the finest grid; everything else gets
//! quadrants, which every common monospace font has. `TERMED_GLYPHS`
//! (`half`, `quadrant`, `sextant`, `octant`) overrides the choice.

use std::sync::OnceLock;

use image::{DynamicImage, GenericImageView, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyphs {
    Half,
    Quadrant,
    Sextant,
    Octant,
}

impl Glyphs {
    /// Sub-pixels per cell (columns, rows).
    pub fn sub(self) -> (usize, usize) {
        match self {
            Glyphs::Half => (1, 2),
            Glyphs::Quadrant => (2, 2),
            Glyphs::Sextant => (2, 3),
            Glyphs::Octant => (2, 4),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Glyphs::Half => "half blocks (1×2)",
            Glyphs::Quadrant => "quadrants (2×2)",
            Glyphs::Sextant => "sextants (2×3)",
            Glyphs::Octant => "octants (2×4)",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "half" | "halfblocks" => Some(Glyphs::Half),
            "quadrant" | "quadrants" => Some(Glyphs::Quadrant),
            "sextant" | "sextants" => Some(Glyphs::Sextant),
            "octant" | "octants" => Some(Glyphs::Octant),
            _ => None,
        }
    }

    /// The finest mosaic this terminal is known to draw well.
    pub fn detect() -> Self {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        if let Some(g) = Self::parse(&var("TERMED_GLYPHS")) {
            return g;
        }
        let (term, program) = (var("TERM"), var("TERM_PROGRAM"));
        // These render block mosaics themselves, independent of the font.
        if !var("KITTY_WINDOW_ID").is_empty()
            || term.contains("kitty")
            || term.contains("ghostty")
            || program == "ghostty"
            || term.starts_with("foot")
        {
            Glyphs::Octant
        } else if program == "WezTerm" || !var("WT_SESSION").is_empty() {
            Glyphs::Sextant
        } else {
            Glyphs::Quadrant
        }
    }

    /// The glyph for a sub-pixel mask (bit `row * cols + col`).
    pub fn glyph(self, mask: u8) -> char {
        match self {
            Glyphs::Half => [' ', '▀', '▄', '█'][mask as usize & 3],
            Glyphs::Quadrant => QUADRANTS[mask as usize & 15],
            Glyphs::Sextant => {
                let p = (mask & 63) as u32;
                match p {
                    0 => ' ',
                    21 => '▌',
                    42 => '▐',
                    63 => '█',
                    _ => {
                        let skip = (p > 21) as u32 + (p > 42) as u32;
                        char::from_u32(0x1FB00 + p - 1 - skip).unwrap_or('█')
                    }
                }
            }
            Glyphs::Octant => OCTANTS[mask as usize],
        }
    }
}

const QUADRANTS: [char; 16] = [
    ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
];

/// Octant glyphs (Unicode 16) indexed by an 8-bit mask, bit `row*2+col`.
/// Patterns that already had a character (halves, quadrants, quarter
/// blocks) use that one, as the standard prescribes.
const OCTANTS: [char; 256] = [
    '\u{20}',
    '\u{1CEA8}',
    '\u{1CEAB}',
    '\u{1FB82}',
    '\u{1CD00}',
    '\u{2598}',
    '\u{1CD01}',
    '\u{1CD02}',
    '\u{1CD03}',
    '\u{1CD04}',
    '\u{259D}',
    '\u{1CD05}',
    '\u{1CD06}',
    '\u{1CD07}',
    '\u{1CD08}',
    '\u{2580}',
    '\u{1CD09}',
    '\u{1CD0A}',
    '\u{1CD0B}',
    '\u{1CD0C}',
    '\u{1FBE6}',
    '\u{1CD0D}',
    '\u{1CD0E}',
    '\u{1CD0F}',
    '\u{1CD10}',
    '\u{1CD11}',
    '\u{1CD12}',
    '\u{1CD13}',
    '\u{1CD14}',
    '\u{1CD15}',
    '\u{1CD16}',
    '\u{1CD17}',
    '\u{1CD18}',
    '\u{1CD19}',
    '\u{1CD1A}',
    '\u{1CD1B}',
    '\u{1CD1C}',
    '\u{1CD1D}',
    '\u{1CD1E}',
    '\u{1CD1F}',
    '\u{1FBE7}',
    '\u{1CD20}',
    '\u{1CD21}',
    '\u{1CD22}',
    '\u{1CD23}',
    '\u{1CD24}',
    '\u{1CD25}',
    '\u{1CD26}',
    '\u{1CD27}',
    '\u{1CD28}',
    '\u{1CD29}',
    '\u{1CD2A}',
    '\u{1CD2B}',
    '\u{1CD2C}',
    '\u{1CD2D}',
    '\u{1CD2E}',
    '\u{1CD2F}',
    '\u{1CD30}',
    '\u{1CD31}',
    '\u{1CD32}',
    '\u{1CD33}',
    '\u{1CD34}',
    '\u{1CD35}',
    '\u{1FB85}',
    '\u{1CEA3}',
    '\u{1CD36}',
    '\u{1CD37}',
    '\u{1CD38}',
    '\u{1CD39}',
    '\u{1CD3A}',
    '\u{1CD3B}',
    '\u{1CD3C}',
    '\u{1CD3D}',
    '\u{1CD3E}',
    '\u{1CD3F}',
    '\u{1CD40}',
    '\u{1CD41}',
    '\u{1CD42}',
    '\u{1CD43}',
    '\u{1CD44}',
    '\u{2596}',
    '\u{1CD45}',
    '\u{1CD46}',
    '\u{1CD47}',
    '\u{1CD48}',
    '\u{258C}',
    '\u{1CD49}',
    '\u{1CD4A}',
    '\u{1CD4B}',
    '\u{1CD4C}',
    '\u{259E}',
    '\u{1CD4D}',
    '\u{1CD4E}',
    '\u{1CD4F}',
    '\u{1CD50}',
    '\u{259B}',
    '\u{1CD51}',
    '\u{1CD52}',
    '\u{1CD53}',
    '\u{1CD54}',
    '\u{1CD55}',
    '\u{1CD56}',
    '\u{1CD57}',
    '\u{1CD58}',
    '\u{1CD59}',
    '\u{1CD5A}',
    '\u{1CD5B}',
    '\u{1CD5C}',
    '\u{1CD5D}',
    '\u{1CD5E}',
    '\u{1CD5F}',
    '\u{1CD60}',
    '\u{1CD61}',
    '\u{1CD62}',
    '\u{1CD63}',
    '\u{1CD64}',
    '\u{1CD65}',
    '\u{1CD66}',
    '\u{1CD67}',
    '\u{1CD68}',
    '\u{1CD69}',
    '\u{1CD6A}',
    '\u{1CD6B}',
    '\u{1CD6C}',
    '\u{1CD6D}',
    '\u{1CD6E}',
    '\u{1CD6F}',
    '\u{1CD70}',
    '\u{1CEA0}',
    '\u{1CD71}',
    '\u{1CD72}',
    '\u{1CD73}',
    '\u{1CD74}',
    '\u{1CD75}',
    '\u{1CD76}',
    '\u{1CD77}',
    '\u{1CD78}',
    '\u{1CD79}',
    '\u{1CD7A}',
    '\u{1CD7B}',
    '\u{1CD7C}',
    '\u{1CD7D}',
    '\u{1CD7E}',
    '\u{1CD7F}',
    '\u{1CD80}',
    '\u{1CD81}',
    '\u{1CD82}',
    '\u{1CD83}',
    '\u{1CD84}',
    '\u{1CD85}',
    '\u{1CD86}',
    '\u{1CD87}',
    '\u{1CD88}',
    '\u{1CD89}',
    '\u{1CD8A}',
    '\u{1CD8B}',
    '\u{1CD8C}',
    '\u{1CD8D}',
    '\u{1CD8E}',
    '\u{1CD8F}',
    '\u{2597}',
    '\u{1CD90}',
    '\u{1CD91}',
    '\u{1CD92}',
    '\u{1CD93}',
    '\u{259A}',
    '\u{1CD94}',
    '\u{1CD95}',
    '\u{1CD96}',
    '\u{1CD97}',
    '\u{2590}',
    '\u{1CD98}',
    '\u{1CD99}',
    '\u{1CD9A}',
    '\u{1CD9B}',
    '\u{259C}',
    '\u{1CD9C}',
    '\u{1CD9D}',
    '\u{1CD9E}',
    '\u{1CD9F}',
    '\u{1CDA0}',
    '\u{1CDA1}',
    '\u{1CDA2}',
    '\u{1CDA3}',
    '\u{1CDA4}',
    '\u{1CDA5}',
    '\u{1CDA6}',
    '\u{1CDA7}',
    '\u{1CDA8}',
    '\u{1CDA9}',
    '\u{1CDAA}',
    '\u{1CDAB}',
    '\u{2582}',
    '\u{1CDAC}',
    '\u{1CDAD}',
    '\u{1CDAE}',
    '\u{1CDAF}',
    '\u{1CDB0}',
    '\u{1CDB1}',
    '\u{1CDB2}',
    '\u{1CDB3}',
    '\u{1CDB4}',
    '\u{1CDB5}',
    '\u{1CDB6}',
    '\u{1CDB7}',
    '\u{1CDB8}',
    '\u{1CDB9}',
    '\u{1CDBA}',
    '\u{1CDBB}',
    '\u{1CDBC}',
    '\u{1CDBD}',
    '\u{1CDBE}',
    '\u{1CDBF}',
    '\u{1CDC0}',
    '\u{1CDC1}',
    '\u{1CDC2}',
    '\u{1CDC3}',
    '\u{1CDC4}',
    '\u{1CDC5}',
    '\u{1CDC6}',
    '\u{1CDC7}',
    '\u{1CDC8}',
    '\u{1CDC9}',
    '\u{1CDCA}',
    '\u{1CDCB}',
    '\u{1CDCC}',
    '\u{1CDCD}',
    '\u{1CDCE}',
    '\u{1CDCF}',
    '\u{1CDD0}',
    '\u{1CDD1}',
    '\u{1CDD2}',
    '\u{1CDD3}',
    '\u{1CDD4}',
    '\u{1CDD5}',
    '\u{1CDD6}',
    '\u{1CDD7}',
    '\u{1CDD8}',
    '\u{1CDD9}',
    '\u{1CDDA}',
    '\u{2584}',
    '\u{1CDDB}',
    '\u{1CDDC}',
    '\u{1CDDD}',
    '\u{1CDDE}',
    '\u{2599}',
    '\u{1CDDF}',
    '\u{1CDE0}',
    '\u{1CDE1}',
    '\u{1CDE2}',
    '\u{259F}',
    '\u{1CDE3}',
    '\u{2586}',
    '\u{1CDE4}',
    '\u{1CDE5}',
    '\u{2588}',
];

pub type Rgb = [f32; 3];

pub fn rgb(c: Color) -> Rgb {
    match c {
        Color::Rgb(r, g, b) => [r as f32, g as f32, b as f32],
        _ => [0.0, 0.0, 0.0],
    }
}

fn color(c: Rgb) -> Color {
    let q = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    Color::Rgb(q(c[0]), q(c[1]), q(c[2]))
}

pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn dist2(a: Rgb, b: Rgb) -> f32 {
    // Weighted for how the eye sees differences.
    let (dr, dg, db) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    2.0 * dr * dr + 4.0 * dg * dg + 3.0 * db * db
}

/// A picture at sub-pixel resolution.
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    px: Vec<Rgb>,
}

impl Canvas {
    pub fn new(w: usize, h: usize, fill: Rgb) -> Self {
        Self {
            w,
            h,
            px: vec![fill; w * h],
        }
    }

    /// A canvas covering `area` at the resolution of `g`.
    pub fn for_area(area: Rect, g: Glyphs, fill: Rgb) -> Self {
        let (sw, sh) = g.sub();
        Self::new(area.width as usize * sw, area.height as usize * sh, fill)
    }

    pub fn get(&self, x: usize, y: usize) -> Rgb {
        self.px[y * self.w + x]
    }

    pub fn set(&mut self, x: usize, y: usize, c: Rgb) {
        if x < self.w && y < self.h {
            self.px[y * self.w + x] = c;
        }
    }

    /// Paint `c` over a sub-pixel with opacity `a`.
    pub fn blend(&mut self, x: usize, y: usize, c: Rgb, a: f32) {
        if x < self.w && y < self.h {
            let p = &mut self.px[y * self.w + x];
            *p = lerp(*p, c, a);
        }
    }

    /// Draw into `area` with mosaic glyphs `g` (the canvas should have been
    /// made with [`Canvas::for_area`] for the same area and glyphs).
    pub fn draw(&self, buf: &mut Buffer, area: Rect, g: Glyphs) {
        let (sw, sh) = g.sub();
        let mut samples = [[0f32; 3]; 8];
        for cy in 0..area.height as usize {
            for cx in 0..area.width as usize {
                let n = sw * sh;
                for (i, sample) in samples.iter_mut().take(n).enumerate() {
                    let (x, y) = (cx * sw + i % sw, cy * sh + i / sw);
                    *sample = if x < self.w && y < self.h {
                        self.get(x, y)
                    } else {
                        [0.0; 3]
                    };
                }
                let (sym, fg, bg) = cell(&samples[..n], g);
                let pos = (area.x + cx as u16, area.y + cy as u16);
                if let Some(c) = buf.cell_mut(pos) {
                    c.set_char(sym);
                    c.set_fg(color(fg));
                    c.set_bg(color(bg));
                }
            }
        }
    }
}

/// Choose the glyph and two colors that best show one cell's sub-pixels.
fn cell(s: &[Rgb], g: Glyphs) -> (char, Rgb, Rgb) {
    // Start from the two most different sub-pixels…
    let (mut a, mut b, mut far) = (0, 0, 0.0);
    for i in 0..s.len() {
        for j in i + 1..s.len() {
            let d = dist2(s[i], s[j]);
            if d > far {
                (a, b, far) = (i, j, d);
            }
        }
    }
    let mean = |pick: &dyn Fn(usize) -> bool| {
        let (mut sum, mut k) = ([0f32; 3], 0f32);
        for (i, c) in s.iter().enumerate() {
            if pick(i) {
                sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
                k += 1.0;
            }
        }
        (k > 0.0).then(|| [sum[0] / k, sum[1] / k, sum[2] / k])
    };
    if far < 60.0 {
        // Flat: one color, no glyph.
        return (' ', [0.0; 3], mean(&|_| true).unwrap_or([0.0; 3]));
    }
    // …then refine the two centers (2-means, two rounds).
    let (mut ca, mut cb) = (s[a], s[b]);
    let mut mask = 0u8;
    for _ in 0..2 {
        mask = 0;
        for (i, c) in s.iter().enumerate() {
            if dist2(*c, cb) < dist2(*c, ca) {
                mask |= 1 << i;
            }
        }
        ca = mean(&|i| mask & (1 << i) == 0).unwrap_or(ca);
        cb = mean(&|i| mask & (1 << i) != 0).unwrap_or(cb);
    }
    // A plain mean smears edge pixels into both colors (a tinted, blotchy
    // background around shapes). Lean each color toward its purest member,
    // the one farthest from the other color, which keeps edges crisp.
    let purest = |inside: bool, other: Rgb, fallback: Rgb| {
        s.iter()
            .enumerate()
            .filter(|(i, _)| (mask & (1 << i) != 0) == inside)
            .map(|(_, c)| *c)
            .max_by(|a, b| dist2(*a, other).total_cmp(&dist2(*b, other)))
            .unwrap_or(fallback)
    };
    let (pa, pb) = (purest(false, cb, ca), purest(true, ca, cb));
    (g.glyph(mask), lerp(cb, pb, 0.6), lerp(ca, pa, 0.6))
}

// ------------------------------------------------------------- the logo

/// Coverage (0..1) of the logo artwork: "termed" plus a block cursor.
fn logo_alpha() -> &'static image::GrayImage {
    static LOGO: OnceLock<image::GrayImage> = OnceLock::new();
    LOGO.get_or_init(|| {
        image::load_from_memory(include_bytes!("../../assets/logo.png"))
            .map(|i| i.to_luma8())
            .unwrap_or_else(|_| image::GrayImage::new(1, 1))
    })
}

/// Whether the mosaic logo is legible at this height (letters need ~18
/// sub-pixels; below that the hand-drawn text logo looks better).
pub fn logo_fits(rows: u16, g: Glyphs) -> bool {
    rows as usize * g.sub().1 >= 18
}

/// Width in cells of the logo when drawn `rows` cells tall (cells are
/// about twice as tall as wide).
pub fn logo_cols(rows: u16) -> u16 {
    let a = logo_alpha();
    (a.width() as f32 / a.height() as f32 * rows as f32 * 2.0).ceil() as u16
}

/// Average coverage of the source over a box (in source pixels).
fn coverage(src: &image::GrayImage, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let (w, h) = (src.width() as i64, src.height() as i64);
    // 4×4 supersampling of the footprint.
    let mut sum = 0.0;
    for j in 0..4 {
        for i in 0..4 {
            let x = (x0 + (x1 - x0) * (i as f32 + 0.5) / 4.0) as i64;
            let y = (y0 + (y1 - y0) * (j as f32 + 0.5) / 4.0) as i64;
            if (0..w).contains(&x) && (0..h).contains(&y) {
                sum += src.get_pixel(x as u32, y as u32)[0] as f32 / 255.0;
            }
        }
    }
    sum / 16.0
}

/// Box blur, used for the soft glow behind the logo.
fn blur(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0; v.len()];
    let mut out = vec![0.0; v.len()];
    for y in 0..h {
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = (lo..=hi).map(|i| v[y * w + i]).sum::<f32>() / (2 * r + 1) as f32;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
            out[y * w + x] = (lo..=hi).map(|j| tmp[j * w + x]).sum::<f32>() / (2 * r + 1) as f32;
        }
    }
    out
}

/// Render the logo into a `w×h` grid of pixels whose aspect is `px_aspect`
/// (pixel width / height): a horizontal gradient with a soft glow.
fn logo_pixels(
    w: usize,
    h: usize,
    px_aspect: f32,
    glow_on: bool,
    from: Rgb,
    to: Rgb,
    bg: Rgb,
) -> Vec<Rgb> {
    let src = logo_alpha();
    let (iw, ih) = (src.width() as f32, src.height() as f32);
    // Fit the artwork into the grid (in square units), keeping its shape,
    // with a little room around it for the glow.
    let (gw, gh) = (w as f32 * px_aspect, h as f32);
    let scale = (gw * 0.96 / iw).min(gh * 0.84 / ih);
    let (dw, dh) = (iw * scale, ih * scale);
    let (ox, oy) = ((gw - dw) / 2.0, (gh - dh) / 2.0);
    let mut cov = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (ux0, ux1) = (x as f32 * px_aspect, (x + 1) as f32 * px_aspect);
            let (sx0, sx1) = ((ux0 - ox) / scale, (ux1 - ox) / scale);
            let (sy0, sy1) = ((y as f32 - oy) / scale, (y as f32 + 1.0 - oy) / scale);
            cov[y * w + x] = coverage(src, sx0, sy0, sx1, sy1);
        }
    }
    // The glow needs real pixels; in a mosaic it only adds blotches.
    let glow = if glow_on {
        blur(&blur(&cov, w, h, (w / 60).max(1)), w, h, (w / 60).max(1))
    } else {
        vec![0.0; w * h]
    };
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let t = x as f32 / w.max(1) as f32;
            let ink = lerp(from, to, t);
            // A touch brighter at the top, like light from above.
            let ink = lerp(ink, [255.0; 3], 0.18 * (1.0 - y as f32 / h.max(1) as f32));
            let base = lerp(bg, lerp(from, to, t), glow[y * w + x] * 0.28);
            out.push(lerp(base, ink, cov[y * w + x]));
        }
    }
    out
}

/// The logo as a mosaic canvas for `area`.
pub fn logo_canvas(area: Rect, g: Glyphs, from: Rgb, to: Rgb, bg: Rgb) -> Canvas {
    let (sw, sh) = g.sub();
    let mut c = Canvas::for_area(area, g, bg);
    // A cell is ~1:2, so a sub-pixel is (1/sw):(2/sh).
    let aspect = (1.0 / sw as f32) / (2.0 / sh as f32);
    c.px = logo_pixels(c.w, c.h, aspect, false, from, to, bg);
    c
}

/// The logo as a real image of `w×h` pixels (for terminal graphics).
pub fn logo_image(w: u32, h: u32, from: Rgb, to: Rgb, bg: Rgb) -> DynamicImage {
    let px = logo_pixels(w as usize, h as usize, 1.0, true, from, to, bg);
    let mut img = RgbaImage::new(w, h);
    for (i, p) in img.pixels_mut().enumerate() {
        let c = px[i];
        *p = image::Rgba([c[0] as u8, c[1] as u8, c[2] as u8, 255]);
    }
    DynamicImage::ImageRgba8(img)
}

// ------------------------------------------------------------ pictures

/// Draw a picture (e.g. an opened PNG) as a mosaic over a checkerboard
/// that shows through transparent parts. `area` should already have the
/// picture's aspect ratio.
pub fn image_canvas(img: &DynamicImage, area: Rect, g: Glyphs, check: (Rgb, Rgb)) -> Canvas {
    let mut c = Canvas::for_area(area, g, check.0);
    let (iw, ih) = img.dimensions();
    let (sx, sy) = (iw as f32 / c.w.max(1) as f32, ih as f32 / c.h.max(1) as f32);
    let (sw, sh) = g.sub();
    for y in 0..c.h {
        for x in 0..c.w {
            // Checkerboard squares of about one cell.
            let checker = ((x / sw.max(2)) + (y / sh.max(2))) % 2 == 0;
            let back = if checker { check.0 } else { check.1 };
            // Average the source pixels under this sub-pixel.
            let (x0, x1) = (
                (x as f32 * sx) as u32,
                (((x + 1) as f32 * sx) as u32).max((x as f32 * sx) as u32 + 1),
            );
            let (y0, y1) = (
                (y as f32 * sy) as u32,
                (((y + 1) as f32 * sy) as u32).max((y as f32 * sy) as u32 + 1),
            );
            let (stepx, stepy) = (((x1 - x0) / 4).max(1), ((y1 - y0) / 4).max(1));
            let (mut sum, mut a, mut k) = ([0f32; 3], 0f32, 0f32);
            let mut yy = y0;
            while yy < y1.min(ih) {
                let mut xx = x0;
                while xx < x1.min(iw) {
                    let p = img.get_pixel(xx, yy).0;
                    let pa = p[3] as f32 / 255.0;
                    sum = [
                        sum[0] + p[0] as f32 * pa,
                        sum[1] + p[1] as f32 * pa,
                        sum[2] + p[2] as f32 * pa,
                    ];
                    a += pa;
                    k += 1.0;
                    xx += stepx;
                }
                yy += stepy;
            }
            let px = if a > 0.0 {
                [sum[0] / a, sum[1] / a, sum[2] / a]
            } else {
                back
            };
            c.set(x, y, lerp(back, px, if k > 0.0 { a / k } else { 0.0 }));
        }
    }
    c
}

/// The largest rect inside `area` with the picture's aspect ratio, centered.
/// `cell_aspect` is a cell's width / height in pixels (about 0.5).
pub fn fit(
    area: Rect,
    iw: u32,
    ih: u32,
    cell_aspect: f32,
    upscale_limit: Option<(u16, u16)>,
) -> Rect {
    if iw == 0 || ih == 0 || area.width == 0 || area.height == 0 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    // Picture size in cells.
    let ratio = iw as f32 / ih as f32 / cell_aspect;
    let (mut w, mut h) = (area.width as f32, area.width as f32 / ratio);
    if h > area.height as f32 {
        h = area.height as f32;
        w = h * ratio;
    }
    if let Some((mw, mh)) = upscale_limit {
        // Don't blow tiny icons up past their natural size.
        if w > mw as f32 {
            w = mw as f32;
            h = w / ratio;
        }
        if h > mh as f32 {
            h = mh as f32;
            w = h * ratio;
        }
    }
    let (w, h) = (w.round().max(1.0) as u16, h.round().max(1.0) as u16);
    Rect::new(
        area.x + (area.width - w.min(area.width)) / 2,
        area.y + (area.height - h.min(area.height)) / 2,
        w.min(area.width),
        h.min(area.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mosaic_glyph_tables_are_consistent() {
        assert_eq!(Glyphs::Sextant.glyph(0b000001), '\u{1FB00}');
        assert_eq!(Glyphs::Sextant.glyph(0b010101), '▌');
        assert_eq!(Glyphs::Sextant.glyph(0b111110), '\u{1FB3B}');
        assert_eq!(Glyphs::Quadrant.glyph(0b1001), '▚');
        // Octants: halves and quadrants reuse the classic glyphs.
        assert_eq!(Glyphs::Octant.glyph(0x0F), '▀');
        assert_eq!(Glyphs::Octant.glyph(0x55), '▌');
        assert_eq!(Glyphs::Octant.glyph(0b0000_0101), '▘');
        let unique: std::collections::HashSet<char> = OCTANTS.iter().copied().collect();
        assert_eq!(unique.len(), 256);
    }

    #[test]
    fn a_cell_picks_its_two_colors_and_shape() {
        let (r, b) = ([255.0, 0.0, 0.0], [0.0, 0.0, 255.0]);
        // Left column red, right column blue → right half block.
        let (ch, fg, bg) = cell(&[r, b, r, b, r, b], Glyphs::Sextant);
        assert_eq!((ch, fg, bg), ('▐', b, r));
        let (ch, _, bg) = cell(&[r; 4], Glyphs::Quadrant);
        assert_eq!((ch, bg), (' ', r));
    }

    #[test]
    fn logo_is_drawn_with_mosaic_glyphs() {
        let area = Rect::new(0, 0, logo_cols(4), 4);
        let mut buf = Buffer::empty(area);
        let c = logo_canvas(
            area,
            Glyphs::Sextant,
            [255.0, 0.0, 128.0],
            [0.0, 200.0, 255.0],
            [20.0; 3],
        );
        c.draw(&mut buf, area, Glyphs::Sextant);
        let mosaic = buf
            .content()
            .iter()
            .filter(|c| {
                c.symbol()
                    .chars()
                    .any(|ch| ('\u{1FB00}'..='\u{1FB3B}').contains(&ch))
            })
            .count();
        assert!(mosaic > 10, "edges use sextants: {mosaic}");
    }

    #[test]
    fn fit_keeps_the_aspect_ratio() {
        let r = fit(Rect::new(0, 0, 80, 24), 1000, 500, 0.5, None);
        assert_eq!((r.width, r.height), (80, 20));
        let r = fit(Rect::new(0, 0, 80, 24), 16, 16, 0.5, Some((4, 2)));
        assert_eq!((r.width, r.height), (4, 2));
    }
}
