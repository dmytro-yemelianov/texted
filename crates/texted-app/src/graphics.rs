//! Ordered GPU drawing commands. Font outlines are rasterized once per cached glyph.
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use font8x8::UnicodeFonts;
use std::{
    collections::HashMap,
    fs,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

static NEXT_FONT_ID: AtomicU64 = AtomicU64::new(1);
const MAX_CACHED_GLYPHS: usize = 4096;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct GlyphKey(pub u64, pub u32);
pub(crate) struct CachedGlyph {
    pub key: GlyphKey,
    pub width: u32,
    pub height: u32,
    x_offset: i32,
    y_offset: i32,
    pub coverage: Vec<u8>,
}
pub(crate) struct Draw {
    pub rect: [f32; 4],
    pub color: u32,
    pub glyph: Option<Arc<CachedGlyph>>,
    pub crop: [f32; 4],
}
#[derive(Default)]
pub(crate) struct DrawList {
    pub clear: u32,
    pub draws: Vec<Draw>,
}
impl DrawList {
    pub fn reset(&mut self, clear: u32) {
        self.clear = clear;
        self.draws.clear();
    }
    fn push(
        &mut self,
        rect: [f32; 4],
        bounds: [f32; 2],
        color: u32,
        glyph: Option<Arc<CachedGlyph>>,
    ) {
        let [x, y, w, h] = rect;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let left = x.max(0.0);
        let top = y.max(0.0);
        let right = (x + w).min(bounds[0]);
        let bottom = (y + h).min(bounds[1]);
        if right <= left || bottom <= top {
            return;
        }
        self.draws.push(Draw {
            rect: [left, top, right - left, bottom - top],
            color,
            glyph,
            crop: [
                (left - x) / w,
                (top - y) / h,
                (right - x) / w,
                (bottom - y) / h,
            ],
        });
    }
}

pub struct FontEngine {
    font: Option<FontArc>,
    font_id: u64,
    scale_px: f32,
    pub char_width_px: f32,
    pub line_height_px: f32,
    pub ascent_px: f32,
    cache: HashMap<u32, Arc<CachedGlyph>>,
}
impl FontEngine {
    pub fn new(scale_px: f32) -> Self {
        let font = [
            "/System/Library/Fonts/Supplemental/Courier New.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Menlo.ttc",
        ]
        .into_iter()
        .find_map(|path| {
            fs::read(path)
                .ok()
                .and_then(|bytes| FontArc::try_from_vec(bytes).ok())
        });
        let (char_width_px, line_height_px, ascent_px) = if let Some(ref font) = font {
            let scaled = font.as_scaled(PxScale::from(scale_px));
            let line_h = (scaled.ascent() - scaled.descent() + scaled.line_gap())
                .ceil()
                .max(scale_px * 1.25);
            (
                scaled.h_advance(scaled.glyph_id('M')).round().max(8.0),
                line_h,
                scaled.ascent(),
            )
        } else {
            (scale_px * 0.6, scale_px * 1.3, scale_px)
        };
        Self {
            font,
            font_id: NEXT_FONT_ID.fetch_add(1, Ordering::Relaxed),
            scale_px,
            char_width_px,
            line_height_px,
            ascent_px,
            cache: HashMap::new(),
        }
    }

    fn glyph(&mut self, ch: char) -> Arc<CachedGlyph> {
        // Missing characters share the font's .notdef glyph rather than duplicate atlas entries.
        let id = self
            .font
            .as_ref()
            .map_or(ch as u32, |f| f.glyph_id(ch).0.into());
        if let Some(glyph) = self.cache.get(&id) {
            return glyph.clone();
        }
        if self.cache.len() >= MAX_CACHED_GLYPHS {
            self.cache.clear();
        }
        let mut glyph = CachedGlyph {
            key: GlyphKey(self.font_id, id),
            width: 0,
            height: 0,
            x_offset: 0,
            y_offset: 0,
            coverage: Vec::new(),
        };
        if let Some(ref font) = self.font {
            let scaled = font.as_scaled(PxScale::from(self.scale_px));
            if let Some(outlined) = font.outline_glyph(scaled.scaled_glyph(ch)) {
                let bounds = outlined.px_bounds();
                glyph.width = bounds.width().ceil() as u32;
                glyph.height = bounds.height().ceil() as u32;
                glyph.x_offset = bounds.min.x.round() as i32;
                glyph.y_offset = (bounds.min.y + self.ascent_px).round() as i32;
                glyph
                    .coverage
                    .resize((glyph.width * glyph.height) as usize, 0);
                outlined.draw(|x, y, a| {
                    glyph.coverage[(y * glyph.width + x) as usize] =
                        (a * 255.0).round().clamp(0.0, 255.0) as u8;
                });
            }
        } else if let Some(bits) = font8x8::BASIC_FONTS
            .get(ch)
            .or_else(|| font8x8::LATIN_FONTS.get(ch))
            .or_else(|| font8x8::BLOCK_FONTS.get(ch))
            .or_else(|| font8x8::BOX_FONTS.get(ch))
        {
            let scale = (self.scale_px / 9.0).round().max(1.0) as u32;
            glyph.width = 8 * scale;
            glyph.height = 8 * scale;
            glyph
                .coverage
                .resize((glyph.width * glyph.height) as usize, 0);
            for y in 0..glyph.height {
                for x in 0..glyph.width {
                    if bits[(y / scale) as usize] & (1 << (x / scale)) != 0 {
                        glyph.coverage[(y * glyph.width + x) as usize] = 255;
                    }
                }
            }
        }
        let glyph = Arc::new(glyph);
        self.cache.insert(id, glyph.clone());
        glyph
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Drawing calls carry target, clipping bounds, position and color"
    )]
    pub fn draw_char(
        &mut self,
        buffer: &mut DrawList,
        stride: usize,
        max_height: usize,
        x: usize,
        y: usize,
        ch: char,
        color: u32,
    ) {
        let glyph = self.glyph(ch);
        buffer.push(
            [
                x as f32 + glyph.x_offset as f32,
                y as f32 + glyph.y_offset as f32,
                glyph.width as f32,
                glyph.height as f32,
            ],
            [stride as f32, max_height as f32],
            color,
            Some(glyph),
        );
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Drawing calls carry target, clipping bounds, position and color"
    )]
    pub fn draw_string(
        &mut self,
        buffer: &mut DrawList,
        stride: usize,
        max_height: usize,
        x: usize,
        y: usize,
        text: &str,
        color: u32,
    ) {
        let mut cur_x = x as f32;
        for ch in text.chars() {
            if cur_x + self.char_width_px > stride as f32 {
                break;
            }
            self.draw_char(
                buffer,
                stride,
                max_height,
                cur_x.round() as usize,
                y,
                ch,
                color,
            );
            cur_x += self.char_width_px;
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Drawing calls carry target, clipping bounds, position, size and color"
)]
pub(crate) fn draw_rect(
    buffer: &mut DrawList,
    stride: usize,
    max_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: u32,
) {
    buffer.push(
        [x as f32, y as f32, width as f32, height as f32],
        [stride as f32, max_height as f32],
        color,
        None,
    );
}
#[expect(
    clippy::too_many_arguments,
    reason = "Drawing calls carry target, clipping bounds, position, size and colors"
)]
pub(crate) fn draw_box(
    buffer: &mut DrawList,
    stride: usize,
    max_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    bg_color: u32,
    border_color: u32,
) {
    draw_rect(buffer, stride, max_height, x, y, width, height, bg_color);
    draw_rect(buffer, stride, max_height, x, y, width, 1, border_color);
    draw_rect(
        buffer,
        stride,
        max_height,
        x,
        y + height.saturating_sub(1),
        width,
        1,
        border_color,
    );
    draw_rect(buffer, stride, max_height, x, y, 1, height, border_color);
    draw_rect(
        buffer,
        stride,
        max_height,
        x + width.saturating_sub(1),
        y,
        1,
        height,
        border_color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negative_glyph_bearings_clip_geometry_and_texture_coordinates_together() {
        let mut list = DrawList::default();
        list.push([-2.0, -3.0, 8.0, 10.0], [5.0, 6.0], 0, None);
        assert_eq!(list.draws[0].rect, [0.0, 0.0, 5.0, 6.0]);
        assert_eq!(list.draws[0].crop, [0.25, 0.3, 0.875, 0.9]);
        list.push([6.0, 0.0, 1.0, 1.0], [5.0, 6.0], 0, None);
        assert_eq!(list.draws.len(), 1);
    }
    #[test]
    fn cached_glyphs_reuse_pixels_and_keep_font_sizes_separate() {
        let mut font = FontEngine::new(18.0);
        let a = font.glyph('ї');
        assert!(Arc::ptr_eq(&a, &font.glyph('ї')));
        assert_ne!(a.key, FontEngine::new(24.0).glyph('ї').key);
    }
}
