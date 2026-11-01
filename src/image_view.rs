//! Full-resolution source retention, premultiplied-area viewport sampling,
//! bounded caches, and exact pixel panning. Native graphics use the same view transform.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ViewKey { width: u32, height: u32, zoom: i8, actual: bool, x: i32, y: i32 }

pub struct ImageView {
    pub pixels: RgbaImage,
    pub original: Vec<u8>,
    pub format: String,
    /// Half-octave zoom relative to fit. Zero always fits on resize.
    pub zoom: i8,
    /// Exact 1:1 source-pixel mode, independent of terminal size.
    pub actual_pixels: bool,
    pub pan_x: i32,
    pub pan_y: i32,
    cache: Option<(ViewKey, RgbaImage)>,
    mipmap: Option<RgbaImage>,
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
            actual_pixels: false,
            pan_x: 0,
            pan_y: 0,
            cache: None,
            mipmap: None,
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        self.pixels.dimensions()
    }
    pub fn fit(&mut self) {
        self.zoom = 0;
        self.actual_pixels = false;
        self.pan_x = 0;
        self.pan_y = 0;
    }
    pub fn zoom_by(&mut self, d: i8) {
        if self.actual_pixels {
            self.actual_pixels = false;
            // Caller sets the nearest relative zoom before this transition if desired.
        }
        self.zoom = self.zoom.saturating_add(d).clamp(-12, 20);
        self.pan_x = 0;
        self.pan_y = 0;
    }
    pub fn key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('+') | KeyCode::Char('=') => self.zoom_by(1),
            KeyCode::Char('-') => self.zoom_by(-1),
            KeyCode::Char('0') | KeyCode::Home => self.fit(),
            KeyCode::Char('1') => self.actual_size(),
            KeyCode::PageUp => self.pan_y = self.pan_y.saturating_sub(64),
            KeyCode::PageDown => self.pan_y = self.pan_y.saturating_add(64),
            KeyCode::Left => self.pan_x = self.pan_x.saturating_sub(4),
            KeyCode::Right => self.pan_x = self.pan_x.saturating_add(4),
            KeyCode::Up => self.pan_y = self.pan_y.saturating_sub(4),
            KeyCode::Down => self.pan_y = self.pan_y.saturating_add(4),
            _ => {}
        }
    }

    /// Enter native 1:1 mode. A source pixel is never interpolated in this mode.
    pub fn actual_size(&mut self) {
        self.actual_pixels = true;
        self.zoom = 0;
        self.pan_x = 0;
        self.pan_y = 0;
    }

    pub fn scale_for(&self, width: u32, height: u32) -> f64 {
        if self.actual_pixels { return 1.0; }
        let w = f64::from(self.pixels.width());
        let h = f64::from(self.pixels.height());
        (f64::from(width.max(1)) / w).min(f64::from(height.max(1)) / h)
            * 2_f64.powf(f64::from(self.zoom) / 2.0)
    }

    /// Convert relative-fit zoom to actual pixels and back without losing the
    /// current magnification when the user presses + or - from 1:1.
    pub fn zoom_at_size(&mut self, d: i8, width: u32, height: u32) {
        if self.actual_pixels {
            let fit = (f64::from(width.max(1)) / f64::from(self.pixels.width()))
                .min(f64::from(height.max(1)) / f64::from(self.pixels.height()));
            self.zoom = ((fit.max(0.0001).log2() * 2.0).round() as i32).clamp(-12, 20) as i8;
            self.actual_pixels = false;
        }
        self.zoom_by(d);
    }

    pub fn sample(&mut self, cols: u16, rows: u16) -> (&RgbaImage, f64) {
        self.sample_pixels(u32::from(cols.max(1)), u32::from(rows.max(1)) * 2)
    }

    /// One resize only. Fit uses a cached 2× reduction pyramid and a
    /// premultiplied-alpha Lanczos filter; magnification uses nearest pixels.
    /// Pan is applied BEFORE resizing, so fit, 1:1 and native graphics agree.
    pub fn sample_pixels(&mut self, width: u32, height: u32) -> (&RgbaImage, f64) {
        let width = width.max(1);
        let height = height.max(1);
        let scale = self.scale_for(width, height);
        let iw = self.pixels.width();
        let ih = self.pixels.height();
        let scaled_w = (f64::from(iw) * scale).round().max(1.0) as u32;
        let scaled_h = (f64::from(ih) * scale).round().max(1.0) as u32;
        let out_w = width.min(scaled_w);
        let out_h = height.min(scaled_h);
        self.pan_x = self.pan_x.clamp(0, scaled_w.saturating_sub(out_w) as i32);
        self.pan_y = self.pan_y.clamp(0, scaled_h.saturating_sub(out_h) as i32);
        let key = ViewKey { width, height, zoom: self.zoom, actual: self.actual_pixels,
            x: self.pan_x, y: self.pan_y };
        if self.cache.as_ref().is_none_or(|(k,_)| *k != key) {
            let sx = (f64::from(self.pan_x) / scale).floor() as u32;
            let sy = (f64::from(self.pan_y) / scale).floor() as u32;
            let sw = ((f64::from(out_w) / scale).ceil() as u32).saturating_add(1).min(iw - sx);
            let sh = ((f64::from(out_h) / scale).ceil() as u32).saturating_add(1).min(ih - sy);
            let crop = image::imageops::crop_imm(&self.pixels, sx, sy, sw, sh).to_image();
            let result = if scale >= 1.0 && (scale - scale.round()).abs() < 0.001 {
                image::imageops::resize(&crop, out_w, out_h, image::imageops::FilterType::Nearest)
            } else {
                let source = if scale < 0.5 {
                    if self.mipmap.is_none() {
                        self.mipmap = Some(resize_premultiplied(&self.pixels,
                            (iw / 2).max(1), (ih / 2).max(1), image::imageops::FilterType::Triangle));
                    }
                    let small = self.mipmap.as_ref().unwrap();
                    image::imageops::crop_imm(small, (sx / 2).min(small.width() - 1),
                        (sy / 2).min(small.height() - 1),
                        sw.div_ceil(2).max(1).min(small.width() - sx / 2),
                        sh.div_ceil(2).max(1).min(small.height() - sy / 2)).to_image()
                } else { crop };
                resize_premultiplied(&source, out_w, out_h, image::imageops::FilterType::Lanczos3)
            };
            self.cache = Some((key, result));
        }
        (&self.cache.as_ref().unwrap().1, scale)
    }

    pub fn invalidate(&mut self) {
        self.cache = None;
        self.mipmap = None;
    }
}

fn resize_premultiplied(image: &RgbaImage, width: u32, height: u32, filter: image::imageops::FilterType) -> RgbaImage {
    let mut premultiplied = image.clone();
    for p in premultiplied.pixels_mut() {
        let a = u16::from(p[3]);
        for c in &mut p.0[..3] { *c = ((u16::from(*c) * a + 127) / 255) as u8; }
    }
    let mut result = image::imageops::resize(&premultiplied, width, height, filter);
    for p in result.pixels_mut() {
        let a = u32::from(p[3]);
        if a > 0 && a < 255 {
            for c in &mut p.0[..3] { *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8; }
        }
    }
    result
}
