//! Portable image preview: RGBA decoding with strict budgets and cached,
//! aspect-correct half-block rendering. No terminal graphics protocol required.
use std::io::Cursor;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, ImageDecoder, ImageReader, Limits, RgbaImage};
use ratatui::crossterm::event::KeyCode;

pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 16 * 1024 * 1024;

pub fn image_extension(path: &std::path::Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "bmp"
                | "ico"
                | "tiff"
                | "tif"
                | "avif"
                | "qoi"
                | "pnm"
                | "ppm"
                | "pgm"
                | "tga"
        )
    })
}

pub struct ImageView {
    pub pixels: RgbaImage,
    pub original: Vec<u8>,
    pub format: String,
    /// Half-octave zoom relative to fit. Zero always fits on resize.
    pub zoom: i8,
    pub pan_x: i32,
    pub pan_y: i32,
    cache: Option<((u16, u16, i8, i32, i32), RgbaImage)>,
}

impl ImageView {
    pub fn decode(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() > MAX_IMAGE_BYTES {
            bail!("Image exceeds the 32 MB input limit");
        }
        let reader = ImageReader::new(Cursor::new(&bytes)).with_guessed_format()?;
        let format = reader.format().context(
            "Unsupported image format (PNG, JPEG, GIF, WebP, BMP and ICO are supported)",
        )?;
        let (w, h) = reader
            .into_dimensions()
            .context("Cannot read image dimensions")?;
        if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
            bail!("Image exceeds the 16 megapixel preview limit");
        }
        let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
        let mut limits = Limits::default();
        limits.max_image_width = Some(16 * 1024 * 1024);
        limits.max_image_height = Some(16 * 1024 * 1024);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let mut decoder = reader
            .into_decoder()
            .context("This image decoder is unavailable or the file is corrupt")?;
        let orientation = decoder
            .orientation()
            .unwrap_or(image::metadata::Orientation::NoTransforms);
        let mut decoded = DynamicImage::from_decoder(decoder).context("Cannot decode image")?;
        decoded.apply_orientation(orientation);
        Ok(Self {
            pixels: decoded.to_rgba8(),
            original: bytes,
            format: format!("{format:?}").to_ascii_uppercase(),
            zoom: 0,
            pan_x: 0,
            pan_y: 0,
            cache: None,
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        self.pixels.dimensions()
    }
    pub fn fit(&mut self) {
        self.zoom = 0;
        self.pan_x = 0;
        self.pan_y = 0;
    }
    pub fn zoom_by(&mut self, d: i8) {
        self.zoom = self.zoom.saturating_add(d).clamp(-6, 8);
        self.pan_x = 0;
        self.pan_y = 0;
    }
    pub fn key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('+') | KeyCode::Char('=') => self.zoom_by(1),
            KeyCode::Char('-') => self.zoom_by(-1),
            KeyCode::Char('0') | KeyCode::Home => self.fit(),
            KeyCode::Left => self.pan_x = self.pan_x.saturating_sub(4),
            KeyCode::Right => self.pan_x = self.pan_x.saturating_add(4),
            KeyCode::Up => self.pan_y = self.pan_y.saturating_sub(4),
            KeyCode::Down => self.pan_y = self.pan_y.saturating_add(4),
            _ => {}
        }
    }

    /// Cache only a viewport-sized sample, not a potentially enormous zoomed
    /// bitmap. Sampling offsets are clamped to the visible image extents.
    pub fn sample(&mut self, cols: u16, rows: u16) -> (&RgbaImage, f64) {
        let w = f64::from(self.pixels.width());
        let h = f64::from(self.pixels.height());
        let scale = (f64::from(cols.max(1)) / w).min(f64::from(rows.max(1)) * 2.0 / h)
            * 2_f64.powf(f64::from(self.zoom) / 2.0);
        let scaled_w = (w * scale).round().max(1.0) as u32;
        let scaled_h = (h * scale).round().max(1.0) as u32;
        let out_w = scaled_w.min(u32::from(cols.max(1)));
        let out_h = scaled_h.min(u32::from(rows.max(1)) * 2);
        self.pan_x = self.pan_x.clamp(0, scaled_w.saturating_sub(out_w) as i32);
        self.pan_y = self.pan_y.clamp(0, scaled_h.saturating_sub(out_h) as i32);
        let key = (cols, rows, self.zoom, self.pan_x, self.pan_y);
        if self.cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let mut pixels = RgbaImage::new(out_w, out_h);
            for y in 0..out_h {
                for x in 0..out_w {
                    let sx = ((f64::from(x) + f64::from(self.pan_x)) / scale)
                        .floor()
                        .clamp(0.0, w - 1.0) as u32;
                    let sy = ((f64::from(y) + f64::from(self.pan_y)) / scale)
                        .floor()
                        .clamp(0.0, h - 1.0) as u32;
                    pixels.put_pixel(x, y, *self.pixels.get_pixel(sx, sy));
                }
            }
            if self.zoom <= 0 {
                pixels = image::imageops::resize(
                    &self.pixels,
                    out_w,
                    out_h,
                    image::imageops::FilterType::Triangle,
                );
            }
            self.cache = Some((key, pixels));
        }
        (&self.cache.as_ref().unwrap().1, scale)
    }

    pub fn invalidate(&mut self) {
        self.cache = None;
    }
}
