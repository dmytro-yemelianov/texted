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
const CLASSIC: &[&str] = &[
    "#",
    "##",
    "###",
    "####",
    "#####",
    "######",
    "#######",
    "########",
    "#########",
    "##########",
    "######",
    "###.###",
    "##..###",
    ".....###",
    ".....###",
    "......##",
];
const CROSSHAIR: &[&str] = &[
    "......#",
    "......#",
    "......#",
    "......#",
    "",
    "",
    "####.###.####",
    "",
    "",
    "......#",
    "......#",
    "......#",
    "......#",
];
const RING: &[&str] = &[
    "....#####",
    "..##.....##",
    ".#.........#",
    ".#.........#",
    "#...........#",
    "#...........#",
    "#.....#.....#",
    "#...........#",
    "#...........#",
    ".#.........#",
    ".#.........#",
    "..##.....##",
    "....#####",
];
struct Skin {
    scale: f32,
    outer: u32,
    inner: u32,
    index: usize,
    hotspot: [f32; 2],
}
fn skin(style: MouseCursor, shape: Shape, pixelated: bool) -> Skin {
    use MouseCursor as C;
    let (outer, inner) = match style {
        C::DarkPixel | C::Classic => (0xffffff, 0x101010),
        C::AmberPixel => (0x1b1100, 0xffc04d),
        C::GreenPixel => (0x001800, 0x70e070),
        C::CyanPixel => (0x00141b, 0x69dfff),
        _ => (0x000000, 0xffffff),
    };
    let native_scale = match style {
        C::SmallPixel => 1.0,
        C::LargePixel => 3.0,
        _ => 2.0,
    };
    let scale = if pixelated {
        if style == C::LargePixel {
            2.0
        } else {
            1.0
        }
    } else {
        native_scale
    };
    let index = match (style, shape) {
        (C::Classic, Shape::Arrow) => 5,
        (C::Crosshair, Shape::Arrow | Shape::Pointer) => 6,
        (C::Ring, Shape::Arrow | Shape::Pointer) => 7,
        _ => shape as usize,
    };
    let hotspot = if index >= 6 {
        [7.0, 7.0]
    } else if shape == Shape::Text {
        [5.0, 9.0]
    } else if matches!(shape, Shape::Grab | Shape::Grabbing) {
        [6.0, 7.0]
    } else {
        [1.0, 1.0]
    };
    Skin {
        scale,
        outer,
        inner,
        index,
        hotspot,
    }
}
type Layers = (Arc<CachedGlyph>, Arc<CachedGlyph>);
static MASKS: OnceLock<Vec<Layers>> = OnceLock::new();
fn masks() -> &'static [Layers] {
    MASKS.get_or_init(|| {
        [ARROW, TEXT, HAND, HAND, GRAB, CLASSIC, CROSSHAIR, RING]
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
    pixelated: bool,
) {
    if style == MouseCursor::System {
        return;
    }
    let Skin {
        scale,
        outer: outer_color,
        inner: inner_color,
        index,
        hotspot,
    } = skin(style, shape, pixelated);
    let (outer, inner) = &masks()[index];
    let at = [point[0] - hotspot[0] * scale, point[1] - hotspot[1] * scale];
    for (glyph, color) in [(outer, outer_color), (inner, inner_color)] {
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
    fn skins_change_shape_size_and_color_while_preserving_text_and_drag_feedback() {
        assert_eq!(skin(MouseCursor::Classic, Shape::Arrow, false).index, 5);
        assert_eq!(skin(MouseCursor::Crosshair, Shape::Pointer, false).index, 6);
        assert_eq!(skin(MouseCursor::Ring, Shape::Arrow, false).index, 7);
        assert_eq!(
            skin(MouseCursor::Crosshair, Shape::Text, false).index,
            Shape::Text as usize
        );
        assert_eq!(
            skin(MouseCursor::Ring, Shape::Grabbing, false).index,
            Shape::Grabbing as usize
        );
        assert!(
            skin(MouseCursor::SmallPixel, Shape::Arrow, false).scale
                < skin(MouseCursor::LargePixel, Shape::Arrow, false).scale
        );
        assert_eq!(skin(MouseCursor::Pixel, Shape::Arrow, true).scale, 1.0);
        assert_ne!(
            skin(MouseCursor::AmberPixel, Shape::Arrow, false).inner,
            skin(MouseCursor::GreenPixel, Shape::Arrow, false).inner
        );
        for style in MouseCursor::ALL {
            let text = serde_json::to_string(&style).unwrap();
            assert_eq!(serde_json::from_str::<MouseCursor>(&text).unwrap(), style);
        }
    }
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
