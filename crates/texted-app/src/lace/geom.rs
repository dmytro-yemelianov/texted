//! Stroke geometry in model units: lines and circular arcs, y up, degrees
//! counter-clockwise.

/// A point or direction in model units.
pub type V2 = [f32; 2];

/// One stroke of a glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prim {
    /// A straight segment from `a` to `b`.
    Line { a: V2, b: V2 },
    /// An arc of the circle `(c, r)` running counter-clockwise from `a0` to
    /// `a1` degrees; `full` makes it the whole circle.
    Arc {
        c: V2,
        r: f32,
        a0: f32,
        a1: f32,
        full: bool,
    },
}

fn sub(a: V2, b: V2) -> V2 {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: V2, b: V2) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn len(a: V2) -> f32 {
    a[0].hypot(a[1])
}

fn dist(a: V2, b: V2) -> f32 {
    len(sub(a, b))
}

fn norm(a: V2) -> V2 {
    let l = len(a);
    let l = if l == 0.0 { 1.0 } else { l };
    [a[0] / l, a[1] / l]
}

fn neg(a: V2) -> V2 {
    [-a[0], -a[1]]
}

fn ang_of(v: V2) -> f32 {
    v[1].atan2(v[0]).to_degrees()
}

fn wrap180(d: f32) -> f32 {
    (d % 360.0 + 540.0) % 360.0 - 180.0
}

/// The point of the circle `(c, r)` at `deg` degrees.
pub fn arc_point(c: V2, r: f32, deg: f32) -> V2 {
    let a = deg.to_radians();
    [c[0] + r * a.cos(), c[1] + r * a.sin()]
}

/// Whether `deg` lies on the arc, keeping `margin` degrees clear of both ends.
pub fn in_arc(p: &Prim, deg: f32, margin: f32) -> bool {
    match *p {
        Prim::Line { .. } => false,
        Prim::Arc { full: true, .. } => true,
        Prim::Arc { a0, a1, .. } => {
            let d = ((deg - a0) % 360.0 + 360.0) % 360.0;
            d >= margin - 1e-6 && d <= a1 - a0 - margin + 1e-6
        }
    }
}

fn dist_seg(p: V2, a: V2, b: V2) -> f32 {
    let ab = sub(b, a);
    let dd = dot(ab, ab);
    let t = (dot(sub(p, a), ab) / if dd == 0.0 { 1.0 } else { dd }).clamp(0.0, 1.0);
    dist(p, [a[0] + ab[0] * t, a[1] + ab[1] * t])
}

/// Distance from `x` to the stroke.
pub fn dist_prim(p: &Prim, x: V2) -> f32 {
    match *p {
        Prim::Line { a, b } => dist_seg(x, a, b),
        Prim::Arc { c, r, a0, a1, .. } => {
            if in_arc(p, ang_of(sub(x, c)), 0.0) {
                (dist(x, c) - r).abs()
            } else {
                dist(x, arc_point(c, r, a0)).min(dist(x, arc_point(c, r, a1)))
            }
        }
    }
}

/// Directions in which the stroke leaves `at` (empty if it does not touch it).
pub fn leave_dirs(p: &Prim, at: V2) -> Vec<V2> {
    match *p {
        Prim::Line { a, b } => {
            if dist(a, at) < 0.05 {
                vec![norm(sub(b, a))]
            } else if dist(b, at) < 0.05 {
                vec![norm(sub(a, b))]
            } else if dist_seg(at, a, b) < 0.05 {
                let v = norm(sub(b, a));
                vec![v, neg(v)]
            } else {
                Vec::new()
            }
        }
        Prim::Arc { c, r, a0, a1, full } => {
            if (dist(at, c) - r).abs() > 0.05 {
                return Vec::new();
            }
            let a = ang_of(sub(at, c));
            if !full {
                if wrap180(a - a0).abs() < 0.5 {
                    let t = a0.to_radians();
                    return vec![[-t.sin(), t.cos()]];
                }
                if wrap180(a - a1).abs() < 0.5 {
                    let t = a1.to_radians();
                    return vec![[t.sin(), -t.cos()]];
                }
                if !in_arc(p, a, 0.0) {
                    return Vec::new();
                }
            }
            let t = [-a.to_radians().sin(), a.to_radians().cos()];
            vec![t, neg(t)]
        }
    }
}

fn sample_points(p: &Prim) -> Vec<V2> {
    match *p {
        Prim::Line { a, b } => vec![a, b],
        Prim::Arc { c, r, a0, a1, full } => {
            let span = if full { 360.0 } else { a1 - a0 };
            (0..=48)
                .map(|k| arc_point(c, r, a0 + span * k as f32 / 48.0))
                .collect()
        }
    }
}

/// Bounding box `[x0, y0, x1, y1]` of the strokes (arcs sampled at 48 steps).
pub fn bbox(prims: &[Prim]) -> [f32; 4] {
    let mut b: [f32; 4] = [1e9, 1e9, -1e9, -1e9];
    for p in prims.iter().flat_map(sample_points) {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}
