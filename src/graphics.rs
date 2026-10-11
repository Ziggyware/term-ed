//! Terminal pixel graphics: real images through the kitty graphics
//! protocol, Sixel or iTerm2 inline images, chosen by asking the terminal
//! at startup (via `ratatui-image`). Without one of those, pictures are
//! drawn as high-resolution block mosaics ([`crate::ui::hires`]).
//!
//! `TERMED_GRAPHICS` = `auto` (default), `kitty`, `sixel`, `iterm2` or
//! `off` overrides detection.

use std::collections::HashMap;

use image::DynamicImage;
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};

use crate::ui::hires::{Canvas, Glyphs};

pub struct Graphics {
    /// Block mosaic used for drawing at sub-cell resolution.
    pub glyphs: Glyphs,
    /// Set when the terminal can show real pixels.
    picker: Option<Picker>,
    /// Cell size in pixels (when the terminal reported it).
    pub cell_px: Option<(u16, u16)>,
    /// Headless rendering (tests, screenshots): record pictures instead of
    /// emitting escape codes, as if the terminal supported pixels.
    pub record: bool,
    /// Protocol name reported while recording (for screenshots).
    pub record_as: &'static str,
    pub recorded: Vec<(Rect, DynamicImage)>,
    cache: HashMap<String, (Rect, Protocol)>,
    /// Finished mosaics, so a still picture costs nothing per frame.
    mosaics: HashMap<String, (Rect, Glyphs, Vec<Cell>)>,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            glyphs: Glyphs::detect(),
            picker: None,
            cell_px: None,
            record: false,
            record_as: "kitty",
            recorded: Vec::new(),
            cache: HashMap::new(),
            mosaics: HashMap::new(),
        }
    }
}

impl Graphics {
    /// Ask the terminal what it supports. Call after entering the alternate
    /// screen and before reading events (it reads the replies from stdin).
    pub fn query(&mut self) {
        let mode = std::env::var("TERMED_GRAPHICS")
            .unwrap_or_default()
            .to_lowercase();
        if mode == "off" || mode == "none" {
            return;
        }
        // Windows consoles may never answer the query; only ask when told to.
        if cfg!(windows) && mode.is_empty() {
            return;
        }
        let Ok(mut picker) = Picker::from_query_stdio() else {
            return;
        };
        let fs = picker.font_size();
        self.cell_px = Some((fs.width, fs.height));
        let forced = match mode.as_str() {
            "kitty" => Some(ProtocolType::Kitty),
            "sixel" => Some(ProtocolType::Sixel),
            "iterm2" | "iterm" => Some(ProtocolType::Iterm2),
            _ => None,
        };
        if let Some(p) = forced {
            picker.set_protocol_type(p);
        }
        if picker.protocol_type() != ProtocolType::Halfblocks {
            self.picker = Some(picker);
        }
    }

    /// The pixel protocol in use, if any.
    pub fn protocol(&self) -> Option<&'static str> {
        if self.record {
            return Some(self.record_as);
        }
        self.picker.as_ref().map(|p| match p.protocol_type() {
            ProtocolType::Kitty => "kitty",
            ProtocolType::Sixel => "Sixel",
            ProtocolType::Iterm2 => "iTerm2",
            ProtocolType::Halfblocks => "half blocks",
        })
    }

    /// Kitty's protocol encodes image ids in cell colors, which must not be
    /// rounded to the 256-color palette.
    pub fn needs_truecolor(&self) -> bool {
        self.picker
            .as_ref()
            .is_some_and(|p| p.protocol_type() == ProtocolType::Kitty)
    }

    /// Cell width / height in pixels (about 0.5 for most fonts).
    pub fn cell_aspect(&self) -> f32 {
        match self.cell_px {
            Some((w, h)) if w > 0 && h > 0 => w as f32 / h as f32,
            _ => 0.5,
        }
    }

    /// Pixel size to render a picture for `area`.
    pub fn pixels_for(&self, area: Rect) -> (u32, u32) {
        let (cw, ch) = self.cell_px.unwrap_or((10, 20));
        (
            area.width as u32 * cw as u32,
            area.height as u32 * ch as u32,
        )
    }

    /// Show a real image in `area` if the terminal can. `key` identifies the
    /// picture (it is re-encoded only when the key or area changes);
    /// `make` renders it at the given pixel size. Returns false when the
    /// caller should draw a mosaic instead.
    pub fn draw_image(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        key: &str,
        make: impl FnOnce(u32, u32) -> DynamicImage,
    ) -> bool {
        if area.width == 0 || area.height == 0 {
            return true;
        }
        if self.record {
            let (w, h) = self.pixels_for(area);
            self.recorded.push((area, make(w, h)));
            return true;
        }
        let Some(picker) = &self.picker else {
            return false;
        };
        let fresh = self.cache.get(key).is_none_or(|(r, _)| *r != area);
        if fresh {
            let (w, h) = self.pixels_for(area);
            match picker.new_protocol(make(w, h), area.as_size(), Resize::Fit(None)) {
                Ok(p) => {
                    self.cache.insert(key.to_string(), (area, p));
                }
                Err(_) => return false,
            }
        }
        if let Some((_, p)) = self.cache.get(key) {
            Image::new(p).render(area, buf);
        }
        true
    }

    /// Draw a mosaic for `area`, computed once per key, area and glyph set.
    pub fn draw_mosaic(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        key: &str,
        make: impl FnOnce() -> Canvas,
    ) {
        let g = self.glyphs;
        let fresh = self
            .mosaics
            .get(key)
            .is_none_or(|(r, cg, _)| *r != area || *cg != g);
        if fresh {
            let mut tmp = Buffer::empty(area);
            make().draw(&mut tmp, area, g);
            if self.mosaics.len() > 32 {
                self.mosaics.clear();
            }
            self.mosaics.insert(key.to_string(), (area, g, tmp.content));
        }
        if let Some((_, _, cells)) = self.mosaics.get(key) {
            for (i, c) in cells.iter().enumerate() {
                let pos = (
                    area.x + (i as u16 % area.width),
                    area.y + (i as u16 / area.width),
                );
                if let Some(dst) = buf.cell_mut(pos) {
                    *dst = c.clone();
                }
            }
        }
    }
}
