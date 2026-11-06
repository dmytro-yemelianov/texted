//! Ordered GPU drawing commands and exact native bitmap glyphs; all font masks are one-bit.
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use texted_app::{
    appearance::Typography,
    bitmap_fonts::{self, BitmapFace},
};

static NEXT_FONT_ID: AtomicU64 = AtomicU64::new(1);
const MAX_CACHED_GLYPHS: usize = 4096;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct GlyphKey(pub u64, pub u32);
pub(crate) struct CachedGlyph {
    pub key: GlyphKey,
    pub width: u32,
    pub height: u32,
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
    face: BitmapFace,
    font_id: u64,
    scale: u32,
    pub char_width_px: f32,
    pub line_height_px: f32,
    natural_line_height_px: f32,
    cache: HashMap<u32, Arc<CachedGlyph>>,
}
impl FontEngine {
    pub fn new(scale_px: f32) -> Self {
        Self::with_typography(Typography {
            size_px: scale_px.round() as u16,
            ..Default::default()
        })
    }
    pub fn with_typography(mut settings: Typography) -> Self {
        settings.normalize();
        let face = bitmap_fonts::face(settings.family);
        let scale = u32::from(settings.size_px / face.height);
        let natural_line_height_px = f32::from(settings.size_px);
        Self {
            face,
            font_id: NEXT_FONT_ID.fetch_add(1, Ordering::Relaxed),
            scale,
            char_width_px: f32::from(face.width) * scale as f32,
            line_height_px: (natural_line_height_px * f32::from(settings.line_spacing) / 100.0)
                .ceil(),
            natural_line_height_px,
            cache: HashMap::new(),
        }
    }
    pub fn set_line_spacing(&mut self, percent: u16) {
        self.line_height_px =
            (self.natural_line_height_px * f32::from(percent.clamp(100, 180)) / 100.0).ceil();
    }
    fn glyph(&mut self, ch: char) -> Arc<CachedGlyph> {
        let id = ch as u32;
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
            coverage: Vec::new(),
        };
        if !ch.is_control() {
            let rows = self
                .face
                .glyph(ch)
                .unwrap_or_else(|| self.face.glyph('\u{fffd}').expect("bundled missing glyph"));
            if rows.iter().any(|row| *row != 0) {
                glyph.width = u32::from(self.face.width) * self.scale;
                glyph.height = u32::from(self.face.height) * self.scale;
                glyph
                    .coverage
                    .resize((glyph.width * glyph.height) as usize, 0);
                for y in 0..glyph.height {
                    for x in 0..glyph.width {
                        if rows[(y / self.scale) as usize] & (0x80 >> (x / self.scale)) != 0 {
                            glyph.coverage[(y * glyph.width + x) as usize] = 255;
                        }
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
            [x as f32, y as f32, glyph.width as f32, glyph.height as f32],
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
    use texted_app::appearance::FontFamily;
    #[test]
    fn native_bitmap_punctuation_stays_visible() {
        let mut font = FontEngine::with_typography(Typography {
            family: FontFamily::Bios,
            size_px: 8,
            ..Default::default()
        });
        for ch in "—’".chars() {
            assert!(font.glyph(ch).coverage.contains(&255), "missing {ch}");
        }
    }
    #[test]
    fn pixel_ukrainian_capitals_match_bitmap_letter_height() {
        let mut font = FontEngine::with_typography(Typography {
            family: FontFamily::Bios,
            size_px: 8,
            ..Default::default()
        });
        for ch in "ПҐЇЄ".chars() {
            let glyph = font.glyph(ch);
            let rows: Vec<_> = glyph
                .coverage
                .chunks(8)
                .enumerate()
                .filter(|(_, row)| row.contains(&255))
                .map(|(i, _)| i)
                .collect();
            assert!(
                rows.last().unwrap() - rows.first().unwrap() + 1 >= 6,
                "{ch}: {rows:?}"
            );
        }
    }
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
    fn every_ukrainian_glyph_retains_exact_native_pixels_at_every_supported_scale() {
        let alphabet = "АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯабвгґдеєжзиіїйклмнопрстуфхцчшщьюяʼ’'₴—–…";
        for family in FontFamily::ALL {
            let face = bitmap_fonts::face(family);
            for scale in 1..=64 / face.height {
                let mut font = FontEngine::with_typography(Typography {
                    family,
                    size_px: face.height * scale,
                    line_spacing: 130,
                });
                assert_eq!(font.char_width_px, f32::from(face.width * scale));
                assert_eq!(
                    font.line_height_px,
                    (f32::from(face.height * scale) * 1.3).ceil()
                );
                for ch in alphabet.chars() {
                    let rows = face.glyph(ch).unwrap();
                    let g = font.glyph(ch);
                    assert_eq!(
                        (g.width, g.height),
                        (
                            u32::from(face.width * scale),
                            u32::from(face.height * scale)
                        )
                    );
                    for y in 0..g.height {
                        for x in 0..g.width {
                            let expected = if rows[(y / u32::from(scale)) as usize]
                                & (0x80 >> (x / u32::from(scale)))
                                != 0
                            {
                                255
                            } else {
                                0
                            };
                            assert_eq!(
                                g.coverage[(y * g.width + x) as usize],
                                expected,
                                "{} {ch} scale {scale}",
                                family.name()
                            );
                        }
                    }
                }
                assert_eq!(font.glyph(' ').width, 0);
                assert_eq!(font.glyph('🦔').coverage, font.glyph('\u{fffd}').coverage);
            }
        }
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
