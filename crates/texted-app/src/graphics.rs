//! Ordered GPU drawing commands. Font outlines are rasterized once per cached glyph.
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use font8x8::UnicodeFonts;
use std::{
    collections::HashMap,
    fs,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
};
use texted_app::appearance::{FontFamily, Typography};

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

// All selectable system faces are loaded once at startup; switching/zooming clones shared font data.
static FONTS: OnceLock<[Option<FontArc>; 3]> = OnceLock::new();
fn fonts() -> &'static [Option<FontArc>; 3] {
    FONTS.get_or_init(|| {
        [
            "/System/Library/Fonts/Supplemental/Courier New.ttf",
            "/System/Library/Fonts/Menlo.ttc",
            "/System/Library/Fonts/Monaco.ttf",
        ]
        .map(|path| {
            fs::read(path)
                .ok()
                .and_then(|bytes| FontArc::try_from_vec(bytes).ok())
        })
    })
}
fn bitmap(ch: char) -> Option<[u8; 8]> {
    font8x8::BASIC_FONTS
        .get(ch)
        .or_else(|| font8x8::LATIN_FONTS.get(ch))
        .or_else(|| font8x8::BLOCK_FONTS.get(ch))
        .or_else(|| font8x8::BOX_FONTS.get(ch))
}
pub struct FontEngine {
    font: Option<FontArc>,
    font_id: u64,
    scale_px: f32,
    pixel: bool,
    pixel_scale: u32,
    pub char_width_px: f32,
    pub line_height_px: f32,
    natural_line_height_px: f32,
    pub ascent_px: f32,
    cache: HashMap<u32, Arc<CachedGlyph>>,
}
impl FontEngine {
    pub fn new(scale_px: f32) -> Self {
        Self::with_typography(Typography {
            size_px: scale_px.round() as u16,
            ..Default::default()
        })
    }
    pub fn available(family: FontFamily) -> bool {
        match family {
            FontFamily::Courier => fonts()[0].is_some(),
            FontFamily::Menlo => fonts()[1].is_some(),
            FontFamily::Monaco => fonts()[2].is_some(),
            FontFamily::Pixel => true,
        }
    }
    pub fn with_typography(mut settings: Typography) -> Self {
        settings.normalize();
        let index = match settings.family {
            FontFamily::Courier | FontFamily::Pixel => 0,
            FontFamily::Menlo => 1,
            FontFamily::Monaco => 2,
        };
        let font = fonts()[index]
            .clone()
            .or_else(|| fonts().iter().find_map(Clone::clone));
        let pixel = settings.family == FontFamily::Pixel || font.is_none();
        let scale_px = f32::from(settings.size_px);
        let pixel_scale = (scale_px / 8.0).round().max(1.0) as u32;
        let (char_width_px, natural_height, ascent_px) = if pixel {
            (
                8.0 * pixel_scale as f32,
                10.0 * pixel_scale as f32,
                8.0 * pixel_scale as f32,
            )
        } else {
            let scaled = font.as_ref().unwrap().as_scaled(PxScale::from(scale_px));
            (
                scaled.h_advance(scaled.glyph_id('M')).round().max(1.0),
                (scaled.ascent() - scaled.descent() + scaled.line_gap())
                    .ceil()
                    .max(scale_px * 1.25),
                scaled.ascent(),
            )
        };
        Self {
            font,
            font_id: NEXT_FONT_ID.fetch_add(1, Ordering::Relaxed),
            scale_px,
            pixel,
            pixel_scale,
            char_width_px,
            natural_line_height_px: natural_height,
            line_height_px: (natural_height * f32::from(settings.line_spacing) / 100.0).ceil(),
            ascent_px,
            cache: HashMap::new(),
        }
    }
    pub fn set_line_spacing(&mut self, percent: u16) {
        self.line_height_px =
            (self.natural_line_height_px * f32::from(percent.clamp(100, 180)) / 100.0).ceil();
    }
    fn glyph(&mut self, ch: char) -> Arc<CachedGlyph> {
        let id = if self.pixel {
            ch as u32
        } else {
            self.font
                .as_ref()
                .map_or(0, |f| u32::from(f.glyph_id(ch).0))
        };
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
        if self.pixel {
            self.pixel_glyph(ch, &mut glyph);
        } else if let Some(ref font) = self.font {
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
                        (a * 255.0).round().clamp(0.0, 255.0) as u8
                });
            }
        }
        let glyph = Arc::new(glyph);
        self.cache.insert(id, glyph.clone());
        glyph
    }
    fn pixel_glyph(&self, ch: char, glyph: &mut CachedGlyph) {
        let bits = bitmap(ch).unwrap_or_else(|| {
            // Keep non-bitmap Unicode (including Ukrainian) in the same 8x8 cell, using the system face.
            let mut bits = [0u8; 8];
            if let Some(font) = self.font.as_ref().filter(|font| font.glyph_id(ch).0 != 0) {
                let scaled = font.as_scaled(PxScale::from(8.0));
                if let Some(outline) = font.outline_glyph(scaled.scaled_glyph(ch)) {
                    let bounds = outline.px_bounds();
                    let dx = (8.0 - scaled.h_advance(scaled.glyph_id(ch))) / 2.0;
                    outline.draw(|x, y, a| {
                        let x = x as i32 + (bounds.min.x + dx).round() as i32;
                        let y = y as i32 + (bounds.min.y + scaled.ascent()).round() as i32;
                        if a >= 0.35 && (0..8).contains(&x) && (0..8).contains(&y) {
                            bits[y as usize] |= 1 << x;
                        }
                    });
                }
            } else {
                bits = bitmap('?').unwrap();
            }
            bits
        });
        if bits == [0; 8] {
            return;
        }
        let scale = self.pixel_scale;
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
    fn line_spacing_changes_reuse_the_same_cached_glyphs() {
        let mut font = FontEngine::new(18.0);
        let before = font.glyph('ї');
        let old_height = font.line_height_px;
        font.set_line_spacing(150);
        assert!(font.line_height_px > old_height);
        assert!(Arc::ptr_eq(&before, &font.glyph('ї')));
    }
    #[test]
    fn pixel_glyphs_are_integer_scaled_and_keep_ukrainian_visible() {
        let mut font = FontEngine::with_typography(Typography {
            family: FontFamily::Pixel,
            size_px: 24,
            line_spacing: 130,
        });
        assert_eq!(font.char_width_px, 24.0);
        assert_eq!(font.line_height_px, 39.0);
        for ch in "AҐґІіЇїЄє".chars() {
            let g = font.glyph(ch);
            assert!(g.coverage.contains(&255), "missing {ch}");
            assert!(g.coverage.iter().all(|p| *p == 0 || *p == 255));
            assert_eq!((g.width, g.height), (24, 24));
            for y in 0..24 {
                for x in 0..24 {
                    assert_eq!(
                        g.coverage[y * 24 + x],
                        g.coverage[(y / 3 * 3) * 24 + x / 3 * 3]
                    );
                }
            }
        }
        assert_eq!(font.glyph(' ').width, 0);
    }
    #[test]
    fn every_typography_uses_the_same_geometry_for_text_caret_and_hit_testing() {
        use texted_buffer::TextBuffer;
        use texted_layout::{LayoutEngine, ViewportConfig};
        let mut buffer = TextBuffer::from_string("Привіт їжак — hello".into());
        buffer.set_cursor_offset("Привіт ".len());
        let snapshot = buffer.create_snapshot();
        for family in FontFamily::ALL {
            for size_px in [8, 18, 40] {
                for line_spacing in [100, 180] {
                    let font = FontEngine::with_typography(Typography {
                        family,
                        size_px,
                        line_spacing,
                    });
                    let config = ViewportConfig {
                        width_px: 980.0,
                        height_px: 500.0,
                        char_width_px: font.char_width_px,
                        line_height_px: font.line_height_px,
                        ..Default::default()
                    };
                    let layout = LayoutEngine::compute_layout(&snapshot, &config, None);
                    let rect = layout.caret_rect;
                    assert_eq!(
                        layout.screen_to_line_col(
                            &config,
                            rect.x + font.char_width_px * 0.1,
                            rect.y + 1.0
                        ),
                        snapshot.offset_to_line_col(snapshot.cursor_offset)
                    );
                }
            }
        }
    }
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
