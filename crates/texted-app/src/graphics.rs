//! Software text rasterizer and pixel drawing primitives.
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use font8x8::UnicodeFonts;
use std::{collections::HashMap, fs};
#[derive(Clone)]
struct CachedGlyph {
    width: usize,
    height: usize,
    x_offset: i32,
    y_offset: i32,
    coverage: Vec<u8>,
}

pub struct FontEngine {
    font: Option<FontArc>,
    scale_px: f32,
    pub char_width_px: f32,
    pub line_height_px: f32,
    pub ascent_px: f32,
    cache: HashMap<char, CachedGlyph>,
}

impl FontEngine {
    pub fn new(scale_px: f32) -> Self {
        let candidate_paths = [
            "/System/Library/Fonts/Supplemental/Courier New.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Menlo.ttc",
        ];

        let mut loaded_font = None;
        for path in candidate_paths {
            if let Ok(bytes) = fs::read(path) {
                if let Ok(font) = FontArc::try_from_vec(bytes) {
                    loaded_font = Some(font);
                    break;
                }
            }
        }

        let (char_width_px, line_height_px, ascent_px) = if let Some(ref font) = loaded_font {
            let scaled = font.as_scaled(PxScale::from(scale_px));
            let h_advance = scaled.h_advance(scaled.glyph_id('M'));
            let ascent = scaled.ascent();
            let descent = scaled.descent();
            let line_gap = scaled.line_gap();
            let line_h = (ascent - descent + line_gap).ceil().max(scale_px * 1.25);
            (h_advance.round().max(8.0), line_h, ascent)
        } else {
            (scale_px * 0.6, scale_px * 1.3, scale_px)
        };

        Self {
            font: loaded_font,
            scale_px,
            char_width_px,
            line_height_px,
            ascent_px,
            cache: HashMap::new(),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Pixel drawing primitives take explicit target, geometry and color"
    )]
    pub fn draw_char(
        &mut self,
        buffer: &mut [u32],
        stride: usize,
        max_height: usize,
        x: usize,
        y: usize,
        ch: char,
        color: u32,
    ) {
        if let Some(ref font) = self.font {
            let scale_px = self.scale_px;
            let ascent_px = self.ascent_px;

            let glyph_data = self.cache.entry(ch).or_insert_with(|| {
                let scaled = font.as_scaled(PxScale::from(scale_px));
                let glyph = scaled.scaled_glyph(ch);
                if let Some(outlined) = font.outline_glyph(glyph) {
                    let bounds = outlined.px_bounds();
                    let width = bounds.width().ceil() as usize;
                    let height = bounds.height().ceil() as usize;
                    let mut cov = vec![0u8; width * height];
                    outlined.draw(|gx, gy, coverage| {
                        if (gx as usize) < width && (gy as usize) < height {
                            cov[gy as usize * width + gx as usize] =
                                (coverage * 255.0).round().min(255.0) as u8;
                        }
                    });
                    CachedGlyph {
                        width,
                        height,
                        x_offset: bounds.min.x.round() as i32,
                        y_offset: (bounds.min.y + ascent_px).round() as i32,
                        coverage: cov,
                    }
                } else {
                    CachedGlyph {
                        width: 0,
                        height: 0,
                        x_offset: 0,
                        y_offset: 0,
                        coverage: Vec::new(),
                    }
                }
            });

            if glyph_data.width > 0 && glyph_data.height > 0 {
                let base_x = x as i32 + glyph_data.x_offset;
                let base_y = y as i32 + glyph_data.y_offset;

                for row in 0..glyph_data.height {
                    let py = base_y + row as i32;
                    if py < 0 || py as usize >= max_height {
                        continue;
                    }
                    let row_offset = (py as usize) * stride;

                    for col in 0..glyph_data.width {
                        let px = base_x + col as i32;
                        if px < 0 || px as usize >= stride {
                            continue;
                        }
                        let alpha = glyph_data.coverage[row * glyph_data.width + col];
                        blend_pixel(&mut buffer[row_offset + px as usize], color, alpha);
                    }
                }
                return;
            }
        }

        // Fallback to font8x8 if TrueType glyph is unavailable
        let glyph = font8x8::BASIC_FONTS
            .get(ch)
            .or_else(|| font8x8::LATIN_FONTS.get(ch))
            .or_else(|| font8x8::BLOCK_FONTS.get(ch))
            .or_else(|| font8x8::BOX_FONTS.get(ch));

        if let Some(glyph_data) = glyph {
            let scale = (self.scale_px / 9.0).round().max(1.0) as usize;
            for (row_idx, &row_byte) in glyph_data.iter().enumerate() {
                for col_idx in 0..8 {
                    if (row_byte & (1 << col_idx)) != 0 {
                        let px = x + col_idx * scale;
                        let py = y + row_idx * scale;
                        draw_rect(buffer, stride, max_height, px, py, scale, scale, color);
                    }
                }
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Pixel drawing primitives take explicit target, geometry and color"
    )]
    pub fn draw_string(
        &mut self,
        buffer: &mut [u32],
        stride: usize,
        max_height: usize,
        x: usize,
        y: usize,
        text: &str,
        color: u32,
    ) {
        let mut cur_x = x as f32;
        let char_advance = self.char_width_px;

        for ch in text.chars() {
            if cur_x + char_advance > stride as f32 {
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
            cur_x += char_advance;
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Pixel drawing primitives take explicit target, geometry and color"
)]
pub(crate) fn draw_rect(
    buffer: &mut [u32],
    stride: usize,
    max_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: u32,
) {
    let x_end = (x + width).min(stride);
    let y_end = (y + height).min(max_height);
    for row in y..y_end {
        let row_offset = row * stride;
        for col in x..x_end {
            buffer[row_offset + col] = color;
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Pixel drawing primitives take explicit target, geometry and color"
)]
pub(crate) fn draw_box(
    buffer: &mut [u32],
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

// Alpha blend pixel over background
#[inline(always)]
fn blend_pixel(dst: &mut u32, fg_rgb: u32, alpha: u8) {
    if alpha == 0 {
        return;
    }
    if alpha == 255 {
        *dst = fg_rgb;
        return;
    }
    let a = alpha as u32;
    let inv_a = 255 - a;

    let fr = (fg_rgb >> 16) & 0xFF;
    let fg = (fg_rgb >> 8) & 0xFF;
    let fb = fg_rgb & 0xFF;

    let dr = (*dst >> 16) & 0xFF;
    let dg = (*dst >> 8) & 0xFF;
    let db = *dst & 0xFF;

    let r = (fr * a + dr * inv_a) / 255;
    let g = (fg * a + dg * inv_a) / 255;
    let b = (fb * a + db * inv_a) / 255;

    *dst = (r << 16) | (g << 8) | b;
}
