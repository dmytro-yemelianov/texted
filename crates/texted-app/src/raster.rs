//! CPU presentation of the same clipped, binary-mask draw scene used by Metal.
use crate::{framebuffer::ScreenMapping, graphics::DrawList};
pub(crate) const MAX_PIXELS: usize = 16 * 1024 * 1024;
#[derive(Default)]
pub(crate) struct Rasterizer {
    source: Vec<u8>,
    pixels: Vec<u8>,
}
fn bytes(width: u32, height: u32) -> Result<usize, String> {
    let n = (width as usize)
        .checked_mul(height as usize)
        .filter(|n| *n > 0 && *n <= MAX_PIXELS)
        .ok_or("Framebuffer exceeds 16 megapixels")?;
    Ok(n * 3)
}
fn color(value: u32) -> [u8; 3] {
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}
impl Rasterizer {
    pub fn render(
        &mut self,
        draws: &DrawList,
        width: u32,
        height: u32,
        mapping: ScreenMapping,
    ) -> Result<&[u8], String> {
        let (sw, sh) = mapping.source;
        self.source.resize(bytes(sw, sh)?, 0);
        let clear = color(draws.clear);
        for px in self.source.chunks_exact_mut(3) {
            px.copy_from_slice(&clear);
        }
        for draw in &draws.draws {
            let [x, y, w, h] = draw.rect;
            if ![x, y, w, h].iter().all(|n| n.is_finite()) || w <= 0. || h <= 0. {
                continue;
            }
            let left = (x - 0.5).ceil().max(0.) as u32;
            let top = (y - 0.5).ceil().max(0.) as u32;
            let right = ((x + w - 0.5).ceil().max(0.) as u32).min(sw);
            let bottom = ((y + h - 0.5).ceil().max(0.) as u32).min(sh);
            let rgb = color(draw.color);
            for py in top.min(sh)..bottom {
                for px in left.min(sw)..right {
                    if let Some(glyph) = &draw.glyph {
                        if glyph.width == 0 || glyph.height == 0 {
                            continue;
                        }
                        let u = draw.crop[0]
                            + (px as f32 + 0.5 - x) / w * (draw.crop[2] - draw.crop[0]);
                        let v = draw.crop[1]
                            + (py as f32 + 0.5 - y) / h * (draw.crop[3] - draw.crop[1]);
                        let gx =
                            ((u * glyph.width as f32).floor().max(0.) as u32).min(glyph.width - 1);
                        let gy = ((v * glyph.height as f32).floor().max(0.) as u32)
                            .min(glyph.height - 1);
                        if glyph
                            .coverage
                            .get((gy * glyph.width + gx) as usize)
                            .copied()
                            .unwrap_or(0)
                            < 128
                        {
                            continue;
                        }
                    }
                    let at = ((py * sw + px) * 3) as usize;
                    self.source[at..at + 3].copy_from_slice(&rgb);
                }
            }
        }
        if mapping.source == (width, height) && mapping.destination == (0, 0, width, height) {
            return Ok(&self.source);
        }
        self.pixels.resize(bytes(width, height)?, 0);
        self.pixels.fill(0);
        let (dx, dy, dw, dh) = mapping.destination;
        if dw == 0
            || dh == 0
            || dx.checked_add(dw).is_none_or(|v| v > width)
            || dy.checked_add(dh).is_none_or(|v| v > height)
        {
            return Err("Invalid framebuffer presentation rectangle".into());
        }
        for y in 0..dh {
            let sy = (((u64::from(y) * 2 + 1) * u64::from(sh)) / (u64::from(dh) * 2))
                .min(u64::from(sh - 1)) as u32;
            for x in 0..dw {
                let sx = (((u64::from(x) * 2 + 1) * u64::from(sw)) / (u64::from(dw) * 2))
                    .min(u64::from(sw - 1)) as u32;
                let from = ((sy * sw + sx) * 3) as usize;
                let to = (((y + dy) * width + x + dx) * 3) as usize;
                self.pixels[to..to + 3].copy_from_slice(&self.source[from..from + 3]);
            }
        }
        Ok(&self.pixels)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::{draw_rect, FontEngine};
    #[test]
    fn bitmap_masks_keep_exact_colors_at_fractional_scale_and_in_letterboxes() {
        let mut d = DrawList::default();
        d.reset(0x123456);
        let mut font = FontEngine::new(16.);
        font.set_zoom(110, 100);
        font.draw_string(&mut d, 40, 30, 0, 0, "Їґ", 0xffffff);
        let mapping = ScreenMapping::new((100, 90), Some((40, 30)));
        let mut r = Rasterizer::default();
        let p = r.render(&d, 100, 90, mapping).unwrap();
        assert!(p
            .chunks_exact(3)
            .all(|p| p == [0, 0, 0] || p == [0x12, 0x34, 0x56] || p == [255, 255, 255]));
        assert!(p.chunks_exact(3).any(|p| p == [255, 255, 255]));
        assert_eq!(&p[..3], &[0, 0, 0]);
    }
    #[test]
    fn paint_order_clipping_and_resize_do_not_retain_stale_pixels() {
        let mut d = DrawList::default();
        d.reset(0);
        draw_rect(&mut d, 8, 8, 1, 1, 4, 4, 0xff0000);
        draw_rect(&mut d, 8, 8, 2, 2, 8, 8, 0x00ff00);
        let mut r = Rasterizer::default();
        let p = r
            .render(&d, 8, 8, ScreenMapping::new((8, 8), None))
            .unwrap();
        assert_eq!(&p[9 * 3..][..3], &[255, 0, 0]);
        assert_eq!(&p[(7 * 8 + 7) * 3..][..3], &[0, 255, 0]);
        d.reset(0x0000ff);
        assert!(r
            .render(&d, 4, 4, ScreenMapping::new((4, 4), None))
            .unwrap()
            .chunks_exact(3)
            .all(|p| p == [0, 0, 255]));
        assert!(r
            .render(&d, u32::MAX, 8, ScreenMapping::new((u32::MAX, 8), None))
            .is_err());
    }
}
