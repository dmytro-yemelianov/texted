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
    cell_grid::{self, CellCommand, CellGrid},
    lace::sprite::Sprite,
};

static NEXT_FONT_ID: AtomicU64 = AtomicU64::new(1);
const MAX_CACHED_GLYPHS: usize = 4096;
const MAX_CACHED_GLYPH_BYTES: usize = 8 * 1024 * 1024;

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
/// ShaderWallpaper's text shadow: an offset in font pixels and the palette's shadow color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextShadow {
    pub x: u8,
    pub y: u8,
    pub color: u32,
}
/// A tile frame for panels: one font pixel thick, single or double, with corner cells of
/// eight font pixels and edge tiles starting `corner_inset` away, as in ShaderWallpaper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub double: bool,
    pub pixel: f32,
    pub corner_inset: u8,
}
pub(crate) fn frame_rects(x: f32, y: f32, w: f32, h: f32, frame: Frame) -> Vec<[f32; 4]> {
    let t = frame.pixel.max(1.0);
    let cell = 8.0 * t;
    // ShaderWallpaper starts edge tiles max((corner inset + 1) / 2, 8) pixels from a corner.
    let edge = f32::from(u16::from(frame.corner_inset).div_ceil(2).max(8)) * t;
    let mut rects = Vec::new();
    let mut ring = |x: f32, y: f32, w: f32, h: f32| {
        if w < 2.0 * cell || h < 2.0 * cell {
            // Too small for corner cells, as the wallpaper skips such frames: plain lines.
            rects.extend([
                [x, y, w, t],
                [x, y + h - t, w, t],
                [x, y, t, h],
                [x + w - t, y, t, h],
            ]);
            return;
        }
        for (cx, cy) in [
            (x, y),
            (x + w - cell, y),
            (x, y + h - cell),
            (x + w - cell, y + h - cell),
        ] {
            let line_y = if cy == y { y } else { y + h - t };
            let line_x = if cx == x { x } else { x + w - t };
            rects.push([cx, line_y, cell, t]);
            rects.push([line_x, cy, t, cell]);
        }
        let (left, right) = (x + edge, x + w - edge);
        if right > left {
            rects.push([left, y, right - left, t]);
            rects.push([left, y + h - t, right - left, t]);
        }
        let (top, bottom) = (y + edge, y + h - edge);
        if bottom > top {
            rects.push([x, top, t, bottom - top]);
            rects.push([x + w - t, top, t, bottom - top]);
        }
    };
    ring(x, y, w, h);
    if frame.double {
        let d = 2.0 * t;
        ring(x + d, y + d, w - 2.0 * d, h - 2.0 * d);
    }
    rects
}
#[derive(Default)]
pub(crate) struct DrawList {
    pub clear: u32,
    pub draws: Vec<Draw>,
    pub cells: Vec<CellCommand>,
    /// Set by the window per frame and kept across `reset`; terminal cells never get it.
    pub shadow: Option<TextShadow>,
    /// Tile frames for every box, set like `shadow`; `None` keeps Texted's thin lines.
    pub frame: Option<Frame>,
}
impl DrawList {
    pub fn reset(&mut self, clear: u32) {
        self.clear = clear;
        self.draws.clear();
        self.cells.clear();
    }
    pub fn translate(&mut self, x: f32, y: f32) {
        for draw in &mut self.draws {
            draw.rect[0] += x;
            draw.rect[1] += y;
        }
        for command in &mut self.cells {
            command.translate(x, y);
        }
    }
    pub fn cell_grid(
        &self,
        columns: usize,
        rows: usize,
        cell_size: [f32; 2],
    ) -> Result<CellGrid, &'static str> {
        cell_grid::compose(columns, rows, cell_size, self.clear, &self.cells)
    }
    pub(crate) fn push(
        &mut self,
        rect: [f32; 4],
        bounds: [f32; 2],
        color: u32,
        glyph: Option<Arc<CachedGlyph>>,
    ) {
        self.push_within(rect, [0.0, 0.0, bounds[0], bounds[1]], color, glyph);
    }
    /// `push`, clipped to the `[left, top, right, bottom]` area instead of the bounds.
    pub(crate) fn push_within(
        &mut self,
        rect: [f32; 4],
        area: [f32; 4],
        color: u32,
        glyph: Option<Arc<CachedGlyph>>,
    ) {
        let [x, y, w, h] = rect;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let left = x.max(area[0]);
        let top = y.max(area[1]);
        let right = (x + w).min(area[2]);
        let bottom = (y + h).min(area[3]);
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
    logical_only: bool,
    font_id: u64,
    scale: u32,
    pub char_width_px: f32,
    pub line_height_px: f32,
    natural_line_height_px: f32,
    render_scale: f32,
    cache: HashMap<u32, Arc<CachedGlyph>>,
    cache_bytes: usize,
    preview_id: u64,
    preview_cache: HashMap<(u32, u32, u32), Arc<CachedGlyph>>,
    /// Lace sprites as glyphs, by the sprite's address; holding the sprite keeps that
    /// address from being reused while its glyph key is cached.
    sprites: HashMap<usize, (Arc<Sprite>, Arc<CachedGlyph>)>,
    sprite_id: u64,
}
impl FontEngine {
    pub fn for_cells() -> Self {
        let mut font = Self::new(16.0);
        font.logical_only = true;
        font
    }
    pub fn new(scale_px: f32) -> Self {
        Self::with_typography(Typography {
            size_px: scale_px.round() as u16,
            ..Default::default()
        })
    }
    pub fn with_typography(mut settings: Typography) -> Self {
        settings.normalize();
        let face = bitmap_fonts::face(settings.family);
        let mut font = Self::with_face(face, settings.size_px, settings.line_spacing);
        font.set_zoom(settings.zoom_percent, settings.line_spacing);
        font
    }
    fn with_face(face: BitmapFace, size_px: u16, line_spacing: u16) -> Self {
        let scale = u32::from(size_px / face.height);
        let natural_line_height_px = f32::from(size_px);
        Self {
            face,
            logical_only: false,
            font_id: NEXT_FONT_ID.fetch_add(1, Ordering::Relaxed),
            scale,
            char_width_px: f32::from(face.width) * scale as f32,
            line_height_px: (natural_line_height_px * f32::from(line_spacing) / 100.0).ceil(),
            natural_line_height_px,
            render_scale: 1.0,
            cache: HashMap::new(),
            cache_bytes: 0,
            preview_id: NEXT_FONT_ID.fetch_add(1, Ordering::Relaxed),
            preview_cache: HashMap::new(),
            sprites: HashMap::new(),
            sprite_id: NEXT_FONT_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
    /// The bitmap strike the editor text is drawn with.
    pub fn face(&self) -> BitmapFace {
        self.face
    }
    /// Names the strike: its records' address and its cell size.
    pub fn face_id(&self) -> (usize, u16, u16) {
        self.face.id()
    }
    /// A lace sprite as a glyph bitmap in font pixels, with a key of its own.
    pub(crate) fn sprite_glyph(&mut self, sprite: &Arc<Sprite>) -> Arc<CachedGlyph> {
        let at = Arc::as_ptr(sprite) as usize;
        if let Some((_, glyph)) = self.sprites.get(&at) {
            return glyph.clone();
        }
        if self.sprites.len() >= MAX_CACHED_GLYPHS {
            self.sprites.clear();
            self.sprite_id = NEXT_FONT_ID.fetch_add(1, Ordering::Relaxed);
        }
        let glyph = Arc::new(CachedGlyph {
            key: GlyphKey(self.sprite_id, self.sprites.len() as u32),
            width: sprite.width,
            height: sprite.height,
            coverage: sprite.coverage.clone(),
        });
        self.sprites.insert(at, (sprite.clone(), glyph.clone()));
        glyph
    }
    /// Small picker specimen; downsampling is nearest-texel and never changes document masks.
    pub fn draw_preview(
        &mut self,
        list: &mut DrawList,
        bounds: [f32; 2],
        at: [f32; 2],
        area: [f32; 2],
        text: &str,
        color: u32,
    ) {
        if self.logical_only {
            self.draw_string(
                list,
                bounds[0] as usize,
                bounds[1] as usize,
                at[0] as usize,
                at[1] as usize,
                text,
                color,
            );
            return;
        }
        let cells = text.chars().count().max(1) as f32;
        let mut factor =
            (area[0] / (cells * self.char_width_px)).min(area[1] / self.natural_line_height_px);
        if factor >= 1.0 {
            factor = factor.floor().min(3.0);
        }
        if factor <= 0.0 {
            return;
        }
        let mut x = at[0];
        for ch in text.chars() {
            let native = self.glyph(ch);
            let glyph = if factor < 1.0 && native.width > 0 {
                let width = (native.width as f32 * factor).floor().max(1.0) as u32;
                let height = (native.height as f32 * factor).floor().max(1.0) as u32;
                let key = (ch as u32, width, height);
                if !self.preview_cache.contains_key(&key) {
                    if self.preview_cache.len() >= 32 {
                        self.preview_cache.clear();
                        self.preview_id = NEXT_FONT_ID.fetch_add(1, Ordering::Relaxed);
                    }
                    let mut coverage = vec![0; (width * height) as usize];
                    for y in 0..height {
                        for x in 0..width {
                            let sy =
                                ((y as f32 + 0.5) * native.height as f32 / height as f32) as u32;
                            let sx = ((x as f32 + 0.5) * native.width as f32 / width as f32) as u32;
                            coverage[(y * width + x) as usize] =
                                native.coverage[(sy.min(native.height - 1) * native.width
                                    + sx.min(native.width - 1))
                                    as usize];
                        }
                    }
                    let glyph = Arc::new(CachedGlyph {
                        key: GlyphKey(self.preview_id, 0x110000 + self.preview_cache.len() as u32),
                        width,
                        height,
                        coverage,
                    });
                    self.preview_cache.insert(key, glyph);
                }
                self.preview_cache[&key].clone()
            } else {
                native
            };
            if glyph.width > 0 {
                let scale = if factor < 1.0 { 1.0 } else { factor };
                list.push(
                    [
                        x,
                        at[1],
                        glyph.width as f32 * scale,
                        glyph.height as f32 * scale,
                    ],
                    bounds,
                    color,
                    Some(glyph),
                );
            }
            x += self.char_width_px * factor;
        }
    }
    /// One pixel of the bitmap face, in scene pixels at the current zoom.
    pub fn pixel_px(&self) -> f32 {
        self.scale as f32 * self.render_scale
    }
    pub fn set_zoom(&mut self, percent: u16, line_spacing: u16) {
        self.render_scale = f32::from(percent.clamp(10, 800)) / 100.0;
        self.char_width_px = f32::from(self.face.width) * self.scale as f32 * self.render_scale;
        self.natural_line_height_px =
            f32::from(self.face.height) * self.scale as f32 * self.render_scale;
        self.set_line_spacing(line_spacing);
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
            self.cache_bytes = 0;
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
                        let native_x = x / self.scale;
                        let at = (y / self.scale) as usize
                            * usize::from(self.face.width.div_ceil(8))
                            + (native_x / 8) as usize;
                        if rows[at] & (0x80 >> (native_x % 8)) != 0 {
                            glyph.coverage[(y * glyph.width + x) as usize] = 255;
                        }
                    }
                }
            }
        }
        if self.cache_bytes + glyph.coverage.len() > MAX_CACHED_GLYPH_BYTES {
            self.cache.clear();
            self.cache_bytes = 0;
        }
        self.cache_bytes += glyph.coverage.len();
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
        buffer.cells.push(CellCommand::Glyph {
            at: [x as f32, y as f32],
            bounds: [stride as f32, max_height as f32],
            symbol: ch,
            color,
        });
        if self.logical_only {
            return;
        }
        let glyph = self.glyph(ch);
        let size = [
            glyph.width as f32 * self.render_scale,
            glyph.height as f32 * self.render_scale,
        ];
        // Like ShaderWallpaper, a shadow farther than a quarter of the cell is not drawn.
        let shadow = buffer.shadow.filter(|s| {
            (s.x > 0 || s.y > 0)
                && u16::from(s.x) <= (self.face.width / 4).max(1)
                && u16::from(s.y) <= (self.face.height / 4).max(1)
        });
        if let Some(shadow) = shadow {
            let pixel = self.pixel_px();
            buffer.push(
                [
                    x as f32 + f32::from(shadow.x) * pixel,
                    y as f32 + f32::from(shadow.y) * pixel,
                    size[0],
                    size[1],
                ],
                [stride as f32, max_height as f32],
                shadow.color,
                Some(glyph.clone()),
            );
        }
        buffer.push(
            [x as f32, y as f32, size[0], size[1]],
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
    buffer.cells.push(CellCommand::Fill {
        rect: [x as f32, y as f32, width as f32, height as f32],
        bounds: [stride as f32, max_height as f32],
        color,
    });
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
    let bounds = [stride as f32, max_height as f32];
    let lines = [
        [x, y, width, 1],
        [x, y + height.saturating_sub(1), width, 1],
        [x, y, 1, height],
        [x + width.saturating_sub(1), y, 1, height],
    ];
    match buffer.frame {
        None => {
            for [lx, ly, lw, lh] in lines {
                draw_rect(buffer, stride, max_height, lx, ly, lw, lh, border_color);
            }
        }
        Some(frame) => {
            // Terminal cells keep the plain box; the window draws the tile frame.
            for [lx, ly, lw, lh] in lines {
                buffer.cells.push(CellCommand::Fill {
                    rect: [lx as f32, ly as f32, lw as f32, lh as f32],
                    bounds,
                    color: border_color,
                });
            }
            let rects = frame_rects(x as f32, y as f32, width as f32, height as f32, frame);
            if let Some(shadow) = buffer.shadow.filter(|s| s.x > 0 || s.y > 0) {
                let dx = f32::from(shadow.x) * frame.pixel;
                let dy = f32::from(shadow.y) * frame.pixel;
                for r in &rects {
                    buffer.push(
                        [r[0] + dx, r[1] + dy, r[2], r[3]],
                        bounds,
                        shadow.color,
                        None,
                    );
                }
            }
            for r in rects {
                buffer.push(r, bounds, border_color, None);
            }
        }
    }
    buffer.cells.push(CellCommand::Border {
        rect: [x as f32, y as f32, width as f32, height as f32],
        bounds: [stride as f32, max_height as f32],
        color: border_color,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use texted_app::appearance::FontFamily;
    #[test]
    fn tile_frames_follow_the_wallpaper_corners_edges_and_gaps() {
        let covered = |rects: &[[f32; 4]], px: f32, py: f32| {
            rects
                .iter()
                .any(|r| px >= r[0] && px < r[0] + r[2] && py >= r[1] && py < r[1] + r[3])
        };
        let frame = Frame {
            double: false,
            pixel: 2.0,
            corner_inset: 16,
        };
        let single = frame_rects(10.0, 20.0, 200.0, 100.0, frame);
        // Corners meet the edges, and the line is one font pixel thick.
        for x in (10..210).step_by(5) {
            assert!(covered(&single, x as f32 + 0.5, 20.5), "top at {x}");
            assert!(covered(&single, x as f32 + 0.5, 118.5), "bottom at {x}");
            if (14..206).contains(&x) {
                assert!(!covered(&single, x as f32 + 0.5, 22.5), "thick at {x}");
            }
        }
        for y in (20..120).step_by(5) {
            assert!(covered(&single, 10.5, y as f32 + 0.5), "left at {y}");
            assert!(covered(&single, 208.5, y as f32 + 0.5), "right at {y}");
        }
        let double = frame_rects(
            10.0,
            20.0,
            200.0,
            100.0,
            Frame {
                double: true,
                ..frame
            },
        );
        assert!(covered(&double, 100.5, 24.5) && !covered(&double, 100.5, 22.5));
        // Edge tiles that start farther in leave a gap after each corner cell.
        let gapped = frame_rects(
            10.0,
            20.0,
            200.0,
            100.0,
            Frame {
                corner_inset: 32,
                ..frame
            },
        );
        assert!(covered(&gapped, 20.5, 20.5));
        assert!(!covered(&gapped, 34.5, 20.5));
        assert!(covered(&gapped, 50.5, 20.5));
    }
    #[test]
    fn a_framed_box_keeps_terminal_cells_and_draws_tiles_behind_their_shadow() {
        let mut plain = DrawList::default();
        draw_box(&mut plain, 400, 400, 10, 20, 200, 100, 0x111111, 0xeeeeee);
        let frame = Frame {
            double: false,
            pixel: 2.0,
            corner_inset: 16,
        };
        let mut framed = DrawList {
            frame: Some(frame),
            shadow: Some(TextShadow {
                x: 1,
                y: 1,
                color: 0x000001,
            }),
            ..Default::default()
        };
        draw_box(&mut framed, 400, 400, 10, 20, 200, 100, 0x111111, 0xeeeeee);
        assert_eq!(format!("{:?}", framed.cells), format!("{:?}", plain.cells));
        let tiles = frame_rects(10.0, 20.0, 200.0, 100.0, frame);
        let colors: Vec<_> = framed.draws.iter().map(|d| d.color).collect();
        let mut expected = vec![0x111111];
        expected.extend(std::iter::repeat_n(0x000001, tiles.len()));
        expected.extend(std::iter::repeat_n(0xeeeeee, tiles.len()));
        assert_eq!(colors, expected);
        assert_eq!(framed.draws[1].rect[0], tiles[0][0] + 2.0);
    }
    #[test]
    fn a_text_shadow_draws_each_glyph_offset_first_in_the_shadow_color() {
        let mut font = FontEngine::with_typography(Typography {
            family: FontFamily::Bios,
            size_px: 8,
            line_spacing: 100,
            zoom_percent: 200,
        });
        let pixel = font.pixel_px();
        let mut list = DrawList {
            shadow: Some(TextShadow {
                x: 1,
                y: 2,
                color: 0x123456,
            }),
            ..Default::default()
        };
        font.draw_char(&mut list, 400, 400, 10, 20, 'A', 0xffffff);
        let drawn: Vec<_> = list
            .draws
            .iter()
            .map(|d| (d.rect[0], d.rect[1], d.color))
            .collect();
        assert_eq!(
            drawn,
            [
                (10.0 + pixel, 20.0 + 2.0 * pixel, 0x123456),
                (10.0, 20.0, 0xffffff)
            ]
        );
        assert_eq!(list.cells.len(), 1, "terminal cells get no shadow");
        // ShaderWallpaper skips a shadow farther than a quarter of the cell.
        list.draws.clear();
        list.shadow = Some(TextShadow {
            x: 3,
            y: 1,
            color: 0x123456,
        });
        font.draw_char(&mut list, 400, 400, 10, 20, 'A', 0xffffff);
        assert_eq!(list.draws.len(), 1);
    }
    #[test]
    fn tiny_and_tall_native_strikes_preserve_their_extreme_columns() {
        for (width, height, size) in [(1u16, 1u16, 64u16), (128, 96, 96)] {
            let rb = usize::from(width.div_ceil(8));
            let mut data = Vec::new();
            for ch in ['A', '\u{fffd}'] {
                data.extend_from_slice(&(ch as u32).to_le_bytes());
                for _ in 0..height {
                    let mut row = vec![0; rb];
                    row[0] |= 0x80;
                    let x = usize::from(width - 1);
                    row[x / 8] |= 0x80 >> (x % 8);
                    data.extend(row);
                }
            }
            let face = BitmapFace::from_records(width, height, Box::leak(data.into_boxed_slice()))
                .unwrap();
            let mut font = FontEngine::with_face(face, size, 100);
            let glyph = font.glyph('A');
            assert_eq!(glyph.height, u32::from(size));
            for y in 0..glyph.height {
                assert_eq!(glyph.coverage[(y * glyph.width) as usize], 255);
                assert_eq!(glyph.coverage[((y + 1) * glyph.width - 1) as usize], 255);
            }
        }
    }
    #[test]
    fn wide_fonts_obey_a_byte_budget_and_live_frame_arcs_survive_eviction() {
        let mut data = Vec::new();
        data.extend_from_slice(&0xfffdu32.to_le_bytes());
        data.extend_from_slice(&[0xff; 40]);
        let face = BitmapFace::from_records(64, 5, Box::leak(data.into_boxed_slice())).unwrap();
        let mut font = FontEngine::with_face(face, 60, 100);
        let first = font.glyph('A');
        for cp in 0x1000..0x1200 {
            font.glyph(char::from_u32(cp).unwrap());
        }
        assert!(font.cache_bytes <= MAX_CACHED_GLYPH_BYTES);
        assert!(font.cache.len() < 512);
        assert_eq!(first.coverage.len(), 768 * 60);
        assert!(first.coverage.iter().all(|v| *v == 255));
    }
    #[test]
    fn wide_native_glyphs_preserve_pixels_past_the_first_byte_at_integer_scales() {
        let mut data = Vec::new();
        for ch in ['A', '\u{fffd}'] {
            data.extend_from_slice(&(ch as u32).to_le_bytes());
            for _ in 0..5 {
                data.extend_from_slice(&[0x80, 0x08]);
            }
        }
        let face = BitmapFace::from_records(13, 5, Box::leak(data.into_boxed_slice())).unwrap();
        for scale in 1..=3 {
            let mut font = FontEngine::with_face(face, 5 * scale, 100);
            let glyph = font.glyph('A');
            assert_eq!(
                (glyph.width, glyph.height),
                (13 * u32::from(scale), 5 * u32::from(scale))
            );
            for y in 0..glyph.height {
                for x in 0..glyph.width {
                    let on = x < u32::from(scale) || x >= 12 * u32::from(scale);
                    assert_eq!(
                        glyph.coverage[(y * glyph.width + x) as usize],
                        if on { 255 } else { 0 }
                    );
                }
            }
            assert_eq!(font.glyph('🦔').coverage, glyph.coverage);
        }
    }
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
                    zoom_percent: 100,
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
    fn fractional_zoom_reuses_binary_source_masks_and_scales_draw_geometry() {
        let mut font = FontEngine::with_typography(Typography::default());
        let source = font.glyph('Ї');
        let mut list = DrawList::default();
        for percent in [100, 110, 120, 90] {
            font.set_zoom(percent, 100);
            assert!(Arc::ptr_eq(&source, &font.glyph('Ї')));
            assert!(source.coverage.iter().all(|&p| p == 0 || p == 255));
            list.reset(0);
            font.draw_char(&mut list, 100, 100, 0, 0, 'Ї', 0xffffff);
            assert!(
                (list.draws[0].rect[2] - source.width as f32 * percent as f32 / 100.0).abs()
                    < 0.001
            );
            assert!((font.char_width_px - 8.0 * percent as f32 / 100.0).abs() < 0.001);
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
                        zoom_percent: 100,
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
