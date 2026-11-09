//! Stroke sprites: a ligature path rasterised one font pixel wide.

use std::collections::BTreeSet;

use super::geom::V2;

/// A bitmap of a connecting stroke. `origin` is the top-left pixel in font
/// pixels relative to the left/upper cell's top-left; coverage is 0 or 255.
#[derive(Clone, Debug, PartialEq)]
pub struct Sprite {
    pub origin: [i32; 2],
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<u8>,
}

impl Sprite {
    pub fn lit(&self, x: i32, y: i32) -> bool {
        let (dx, dy) = (x - self.origin[0], y - self.origin[1]);
        dx >= 0
            && dy >= 0
            && (dx as u32) < self.width
            && (dy as u32) < self.height
            && self.coverage[dy as usize * self.width as usize + dx as usize] != 0
    }
}

/// Rasterise a polyline given in model units, x right and y up from the cell's
/// top-left corner (so y is `<= 0` inside the cell), at `k` units per font pixel.
pub fn rasterize(path: &[V2], k: f32) -> Sprite {
    let px: Vec<[f32; 2]> = path.iter().map(|p| [p[0] / k, -p[1] / k]).collect();
    let mut lit: BTreeSet<(i32, i32)> = BTreeSet::new();
    if let [only] = px.as_slice() {
        lit.insert((only[1].floor() as i32, only[0].floor() as i32));
    }
    for w in px.windows(2) {
        let (p, q) = (w[0], w[1]);
        let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
        let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as u32;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            lit.insert((
                (p[1] + dy * t).floor() as i32,
                (p[0] + dx * t).floor() as i32,
            ));
        }
    }
    let (Some(y0), Some(y1)) = (lit.iter().map(|p| p.0).min(), lit.iter().map(|p| p.0).max())
    else {
        return Sprite {
            origin: [0, 0],
            width: 0,
            height: 0,
            coverage: Vec::new(),
        };
    };
    let x0 = lit.iter().map(|p| p.1).min().unwrap_or(0);
    let x1 = lit.iter().map(|p| p.1).max().unwrap_or(0);
    let (width, height) = ((x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32);
    let mut coverage = vec![0u8; width as usize * height as usize];
    for (y, x) in lit {
        coverage[(y - y0) as usize * width as usize + (x - x0) as usize] = 255;
    }
    Sprite {
        origin: [x0, y0],
        width,
        height,
        coverage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_line_is_one_pixel_thick() {
        let s = rasterize(&[[1.0, -3.5], [5.0, -3.5]], 1.0);
        assert_eq!(s.height, 1);
        assert_eq!(s.origin, [1, 3]);
        assert!((1..=5).all(|x| s.lit(x, 3)));
        assert!(s.coverage.iter().all(|&c| c == 255));
    }

    #[test]
    fn diagonal_is_connected_and_scaled_by_k() {
        let s = rasterize(&[[0.0, 0.0], [-0.0, 0.0], [20.0, -20.0]], 2.0);
        assert_eq!(s.origin, [0, 0]);
        assert!(s.lit(0, 0) && s.lit(10, 10));
        for i in 0..=10 {
            assert!(s.lit(i, i), "{i}");
        }
    }

    #[test]
    fn empty_path_gives_empty_sprite() {
        let s = rasterize(&[], 1.0);
        assert_eq!((s.width, s.height), (0, 0));
    }
}
