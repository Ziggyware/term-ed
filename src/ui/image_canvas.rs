//! Theme-aware image canvas with keyboard/touch controls and transparent pixels.
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::util::{bold, ellipsize, fgbg, fill, put, width};
use crate::app::App;
use crate::commands::Command;
use crate::document::Preview;
use crate::theme::{mix, rgb};

fn composite(pixel: [u8; 4], bg: Color) -> Color {
    let (r, g, b) = rgb(bg);
    let a = u32::from(pixel[3]);
    let blend = |v: u8, b: u8| ((u32::from(v) * a + u32::from(b) * (255 - a) + 127) / 255) as u8;
    Color::Rgb(blend(pixel[0], r), blend(pixel[1], g), blend(pixel[2], b))
}

pub fn render(buf: &mut Buffer, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let pal = app.theme.palette.clone();
    fill(buf, area, pal.bg);
    let Some(Preview::Image(image)) = app
        .editors
        .get_mut(app.active)
        .and_then(|e| e.doc.preview.as_mut())
    else {
        return;
    };
    let (w, h) = image.dimensions();
    let format = image.format.clone();
    let label = format!(
        "{format}  ·  {w} × {h}  ·  {}",
        crate::archive::byte_label(image.original.len() as u64)
    );
    let header_bg = mix(pal.bg, pal.sidebar_bg, 0.65);
    fill(buf, Rect::new(area.x, area.y, area.width, 1), header_bg);
    put(
        buf,
        area.x + 1,
        area.y,
        "IMAGE PREVIEW",
        bold(fgbg(pal.fg, header_bg)),
        area.right(),
    );
    if area.width > 38 {
        put(
            buf,
            area.right() - 13,
            area.y,
            " READ ONLY ",
            fgbg(pal.sidebar_dim, header_bg),
            area.right(),
        );
    }
    if area.height < 6 {
        return;
    }
    put(
        buf,
        area.x + 1,
        area.y + 1,
        &ellipsize(&label, area.width.saturating_sub(2) as usize),
        fgbg(pal.sidebar_dim, pal.bg),
        area.right(),
    );
    let canvas = Rect::new(
        area.x + 1,
        area.y + 3,
        area.width.saturating_sub(2),
        area.height.saturating_sub(5),
    );
    app.layout.text = canvas;
    // A subtle neutral stage keeps alpha visible in both dark and light themes.
    let stage = mix(pal.bg, pal.sidebar_bg, 0.5);
    fill(buf, canvas, stage);
    if canvas.width == 0 || canvas.height == 0 {
        return;
    }
    let (pixels, scale) = image.sample(canvas.width, canvas.height);
    let iw = pixels.width() as u16;
    let ih = pixels.height().div_ceil(2) as u16;
    let x0 = canvas.x + (canvas.width - iw) / 2;
    let y0 = canvas.y + (canvas.height - ih) / 2;
    for y in 0..ih {
        for x in 0..iw {
            let checker = if (x / 2 + y) % 2 == 0 {
                mix(stage, pal.fg, 0.08)
            } else {
                mix(stage, pal.fg, 0.16)
            };
            let top = pixels.get_pixel(u32::from(x), u32::from(y) * 2).0;
            let bottom = if u32::from(y) * 2 + 1 < pixels.height() {
                pixels.get_pixel(u32::from(x), u32::from(y) * 2 + 1).0
            } else {
                [0, 0, 0, 0]
            };
            if let Some(cell) = buf.cell_mut((x0 + x, y0 + y)) {
                cell.set_symbol("▀")
                    .set_fg(composite(top, checker))
                    .set_bg(composite(bottom, checker));
            }
        }
    }
    let foot = area.bottom() - 1;
    put(
        buf,
        area.x + 1,
        foot,
        &format!("{:.0}% · +/- zoom · arrows pan", scale * 100.0),
        fgbg(pal.sidebar_dim, pal.bg),
        area.right(),
    );
    let mut x = area.x + 1;
    for (label, command) in [
        (" Fit ", Command::ImageFit),
        (" − ", Command::ImageZoomOut),
        (" + ", Command::ImageZoomIn),
        (" Export… ", Command::SaveAs),
    ] {
        let w = width(label);
        if x + w > area.right() {
            break;
        }
        put(
            buf,
            x,
            area.y + 2,
            label,
            fgbg(pal.fg, pal.input_bg),
            area.right(),
        );
        app.layout
            .click_items
            .push((Rect::new(x, area.y + 2, w, 1), command));
        x += w + 1;
    }
}
