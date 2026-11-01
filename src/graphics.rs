//! Native terminal graphics with an explicit, conservative capability detector.
//! Kitty transmits lossless RGBA PNG; iTerm2 uses its inline-image protocol.
//! Both reuse one bounded payload cache. Unsupported terminals use half-blocks.
use std::io::{self, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::RgbaImage;
use ratatui::layout::Rect;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Protocol {
    #[default]
    HalfBlock,
    Kitty,
    ITerm2,
}

impl Protocol {
    /// No blocking terminal query: only advertised capabilities auto-enable
    /// graphics. TERMED_IMAGE_PROTOCOL=kitty|iterm2|half-block is an override.
    pub fn detect() -> Self {
        match std::env::var("TERMED_IMAGE_PROTOCOL").unwrap_or_default().to_ascii_lowercase().as_str() {
            "kitty" => return Self::Kitty,
            "iterm2" | "iterm" => return Self::ITerm2,
            "half-block" | "none" | "off" => return Self::HalfBlock,
            _ => {}
        }
        if std::env::var_os("KITTY_WINDOW_ID").is_some()
            || std::env::var("TERM").is_ok_and(|t| t.contains("kitty"))
            || std::env::var("TERM_PROGRAM").is_ok_and(|t| matches!(t.as_str(), "ghostty" | "WezTerm"))
        { return Self::Kitty; }
        if std::env::var("TERM_PROGRAM").is_ok_and(|t| t == "iTerm.app")
            || std::env::var_os("ITERM_SESSION_ID").is_some()
        { return Self::ITerm2; }
        Self::HalfBlock
    }
    pub fn label(self) -> &'static str {
        match self { Self::Kitty => "Kitty · full color", Self::ITerm2 => "iTerm2 · full color", Self::HalfBlock => "Half-block · compatible" }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub key: String,
    pub area: Rect,
    pub image: RgbaImage,
    pub protocol: Protocol,
}

pub struct GraphicsState {
    pub protocol: Protocol,
    pub pending: Option<Frame>,
    pub active: Option<Rect>,
    active_protocol: Option<Protocol>,
    last_key: Option<String>,
    encoded: Option<(String, Vec<u8>)>,
    next_id: u32,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self { protocol: Protocol::detect(), pending: None, active: None, active_protocol: None,
            last_key: None, encoded: None, next_id: 31 }
    }
}

impl GraphicsState {
    pub fn repaint_needed(&self) -> bool {
        match &self.pending { Some(f) => self.last_key.as_ref() != Some(&f.key), None => self.active.is_some() }
    }
    pub fn force_repaint(&mut self) { self.last_key = None; }
    pub fn last_resize(&mut self) { self.last_key = None; }
    pub fn stage(&mut self, frame: Frame) { self.pending = Some(frame); }

    /// Called after Ratatui has painted its cells. Unchanged images are not
    /// transmitted again. Overlays invalidate the key and clear native pixels.
    pub fn present<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        let Some(frame) = self.pending.take() else {
            if self.active.is_some() { self.clear(out)?; }
            self.last_key = None;
            return Ok(());
        };
        if self.last_key.as_ref() == Some(&frame.key) { return Ok(()); }
        self.clear(out)?;
        if frame.area.width == 0 || frame.area.height == 0 || frame.protocol == Protocol::HalfBlock { return Ok(()); }
        let key = format!("{:?}:{}:{}:{}", frame.protocol, frame.key, frame.image.width(), frame.image.height());
        if self.encoded.as_ref().is_none_or(|(k,_)| *k != key) {
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(frame.image.clone()).write_to(&mut png, image::ImageFormat::Png)
                .map_err(io::Error::other)?;
            self.encoded = Some((key, png.into_inner()));
            self.next_id = self.next_id.wrapping_add(1).max(31);
        }
        let bytes = &self.encoded.as_ref().unwrap().1;
        let payload = STANDARD.encode(bytes);
        // Save/restore the text caret; native graphics must not steal focus.
        write!(out, "\x1b7\x1b[{};{}H", frame.area.y + 1, frame.area.x + 1)?;
        match frame.protocol {
            Protocol::Kitty => {
                let chunks: Vec<_> = payload.as_bytes().chunks(4096).collect();
                for (i, chunk) in chunks.iter().enumerate() {
                    let more = u8::from(i + 1 < chunks.len());
                    if i == 0 {
                        write!(out, "\x1b_Ga=T,f=100,i={},q=2,c={},r={},C=1,m={more};", self.next_id, frame.area.width, frame.area.height)?;
                    } else { write!(out, "\x1b_Gm={more};")?; }
                    out.write_all(chunk)?;
                    write!(out, "\x1b\\")?;
                }
            }
            Protocol::ITerm2 => {
                write!(out, "\x1b]1337;File=inline=1;size={};width={};height={};preserveAspectRatio=0:{payload}\x07",
                    bytes.len(), frame.image.width(), frame.image.height())?;
            }
            Protocol::HalfBlock => {}
        }
        write!(out, "\x1b8")?;
        out.flush()?;
        self.last_key = Some(frame.key);
        self.active = Some(frame.area);
        self.active_protocol = Some(frame.protocol);
        Ok(())
    }

    pub fn clear<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        if self.active.is_some() && self.active_protocol == Some(Protocol::Kitty) {
            // Delete Kitty placements, then repaint the cells underneath on
            // the next Ratatui frame. iTerm images are overwritten by cells.
            write!(out, "\x1b_Ga=d,d=A,q=2;\x1b\\")?;
            out.flush()?;
        }
        self.active = None;
        self.active_protocol = None;
        self.last_key = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kitty_payload_is_chunked_and_cached() {
        let mut state = GraphicsState { protocol: Protocol::Kitty, ..Default::default() };
        let frame = Frame { key: "test".into(), area: Rect::new(1,2,10,5),
            image: RgbaImage::from_pixel(64,64,image::Rgba([20,40,60,255])), protocol: Protocol::Kitty };
        state.stage(frame.clone());
        let mut out = Vec::new(); state.present(&mut out).unwrap();
        assert!(String::from_utf8_lossy(&out).contains("\x1b_Ga=T,f=100"));
        state.stage(frame);
        let mut again = Vec::new(); state.present(&mut again).unwrap();
        assert!(again.is_empty());
        let mut clear = Vec::new(); state.clear(&mut clear).unwrap();
        assert!(String::from_utf8_lossy(&clear).contains("a=d"));
    }
}
