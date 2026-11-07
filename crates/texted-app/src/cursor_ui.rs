//! Contextual system pointers and optional one-bit pointer skins, with shared GPU masks.
use crate::graphics::{CachedGlyph, DrawList, GlyphKey};
use std::sync::{Arc, OnceLock};
use texted_app::appearance::MouseCursor;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Arrow,
    Text,
    Pointer,
    Grab,
    Grabbing,
}
impl Shape {
    pub fn system(self) -> winit::window::CursorIcon {
        use winit::window::CursorIcon;
        match self {
            Self::Arrow => CursorIcon::Default,
            Self::Text => CursorIcon::Text,
            Self::Pointer => CursorIcon::Pointer,
            Self::Grab => CursorIcon::Grab,
            Self::Grabbing => CursorIcon::Grabbing,
        }
    }
}
const ARROW: &[&str] = &[
    "#",
    "##",
    "#.#",
    "#..#",
    "#...#",
    "#....#",
    "#.....#",
    "#......#",
    "#.......#",
    "#....#####",
    "#..#.#",
    "#.#.#.#",
    "##..#.#",
    "#....#.#",
    ".....#.#",
    "......#",
];
const TEXT: &[&str] = &[
    "..#####", "....#", "....#", "....#", "....#", "....#", "....#", "....#", "....#", "....#",
    "....#", "..#####",
];
const HAND: &[&str] = &[
    "....##",
    "...#..#",
    "...#..#",
    "...#..#",
    "...#..###",
    "...#..#..##",
    "...#..#..#.#",
    ".###.......#",
    "#..#.......#",
    "#..........#",
    ".#.........#",
    "..#........#",
    "...#......#",
    "...#......#",
    "...########",
];
const GRAB: &[&str] = &[
    "..##.##.##",
    ".#..#..#..##",
    ".#.........#",
    ".#.........#",
    ".#.........#",
    ".#.........#",
    "..#........#",
    "...#......#",
    "...#......#",
    "...########",
];
type Layers = (Arc<CachedGlyph>, Arc<CachedGlyph>);
static MASKS: OnceLock<Vec<Layers>> = OnceLock::new();
fn masks() -> &'static [Layers] {
    MASKS.get_or_init(|| {
        [ARROW, TEXT, HAND, HAND, GRAB]
            .iter()
            .enumerate()
            .map(|(index, pattern)| {
                let (w, h) = (16usize, 20usize);
                let mut fill = vec![0u8; w * h];
                let mut outline = vec![0u8; w * h];
                for (y, row) in pattern.iter().enumerate() {
                    for (x, c) in row.chars().enumerate() {
                        if c == '#' {
                            fill[(y + 1) * w + x + 1] = 255;
                            for dy in 0..3 {
                                for dx in 0..3 {
                                    outline[(y + dy) * w + x + dx] = 255;
                                }
                            }
                        }
                    }
                }
                let glyph = |layer, coverage| {
                    Arc::new(CachedGlyph {
                        key: GlyphKey(u64::MAX - 17, (index * 2 + layer) as u32),
                        width: w as u32,
                        height: h as u32,
                        coverage,
                    })
                };
                (glyph(0, outline), glyph(1, fill))
            })
            .collect()
    })
}
pub(crate) fn paint(
    list: &mut DrawList,
    bounds: [f32; 2],
    point: [f32; 2],
    style: MouseCursor,
    shape: Shape,
) {
    if style == MouseCursor::System {
        return;
    }
    let scale = if style == MouseCursor::LargePixel {
        3.0
    } else {
        2.0
    };
    let index = shape as usize;
    let (outer, inner) = &masks()[index];
    let hotspot = if shape == Shape::Text {
        [5.0, 9.0]
    } else if matches!(shape, Shape::Grab | Shape::Grabbing) {
        [6.0, 7.0]
    } else {
        [1.0, 1.0]
    };
    let at = [point[0] - hotspot[0] * scale, point[1] - hotspot[1] * scale];
    for (glyph, color) in [(outer, 0x000000), (inner, 0xffffff)] {
        list.push(
            [
                at[0],
                at[1],
                glyph.width as f32 * scale,
                glyph.height as f32 * scale,
            ],
            bounds,
            color,
            Some(glyph.clone()),
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pixel_skins_have_stable_binary_masks_and_do_not_allocate_new_glyphs_each_frame() {
        let a = masks();
        let b = masks();
        assert!(Arc::ptr_eq(&a[0].0, &b[0].0));
        for (outer, inner) in a {
            assert!(inner.coverage.contains(&255));
            assert!(inner.coverage.iter().all(|v| *v == 0 || *v == 255));
            assert!(outer
                .coverage
                .iter()
                .zip(&inner.coverage)
                .all(|(a, b)| a >= b));
        }
    }
}
