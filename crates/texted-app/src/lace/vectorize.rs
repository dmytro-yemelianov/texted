//! Bitmap faces read as strokes: a pixel glyph becomes the lines and arcs the
//! connector model works on. Ported from the prototype's `vectorize.js`.
//!
//! Tiny components become dots; the rest is Scale2x-ed twice, thinned
//! (Zhang-Suen), traced as a graph of paths between ends and junctions, pruned
//! of spurs, simplified (Ramer-Douglas-Peucker) and given tangent fillets at
//! gentle turns. One-pixel bars the thinning erased are put back.

use std::collections::{HashMap, HashSet};

use super::geom::{dist_prim, Prim, V2};

const PASSES: u32 = 2;
const PRUNE: usize = 3;

/// What a face's cell and `x` say about its geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceMetrics {
    pub width: u16,
    pub height: u16,
    /// Row just below the baseline (one past the last ink row of `x`).
    pub baseline: u16,
    /// First ink row of `x`.
    pub x_top: u16,
    /// Model units per pixel: the x-height becomes 8.
    pub k: f32,
}

type Px = (i32, i32);

fn ink_in(rows: &[u8], w: u16, h: u16, c: i32, r: i32) -> bool {
    if r < 0 || r >= i32::from(h) || c < 0 || c >= i32::from(w) {
        return false;
    }
    let stride = usize::from(w.div_ceil(8));
    rows.get(r as usize * stride + c as usize / 8)
        .is_some_and(|b| b & (0x80 >> (c % 8)) != 0)
}

impl FaceMetrics {
    /// Measure a face from its `x` (else Cyrillic `х`).
    pub fn measure(
        glyph: impl Fn(char) -> Option<Vec<u8>>,
        width: u16,
        height: u16,
    ) -> Result<Self, String> {
        if !(6..=32).contains(&width) || !(8..=64).contains(&height) {
            return Err(format!("cell {width}×{height} is outside 6–32 × 8–64"));
        }
        for ch in ['x', 'х'] {
            let Some(rows) = glyph(ch) else { continue };
            let inked: Vec<i32> = (0..i32::from(height))
                .filter(|&r| (0..i32::from(width)).any(|c| ink_in(&rows, width, height, c, r)))
                .collect();
            let (Some(&top), Some(&last)) = (inked.first(), inked.last()) else {
                continue;
            };
            let (x_top, baseline) = (top as u16, last as u16 + 1);
            let k = 8.0 / (f32::from(baseline) - f32::from(x_top) - 0.5);
            return Ok(Self {
                width,
                height,
                baseline,
                x_top,
                k,
            });
        }
        Err("face has no x to measure".into())
    }
}

/// Remove serifs: feet and heads of stems, and nubs hanging off bars.
pub fn strip_serifs(rows: &[u8], width: u16, height: u16) -> Vec<u8> {
    let (w, h) = (i32::from(width), i32::from(height));
    let ink = |c: i32, r: i32| ink_in(rows, width, height, c, r);
    let mut out = rows.to_vec();
    let stride = usize::from(width.div_ceil(8));
    let mut clear = |c: i32, r: i32| {
        if let Some(b) = out.get_mut(r as usize * stride + c as usize / 8) {
            *b &= !(0x80u8 >> (c % 8));
        }
    };
    let runs_in = |r: i32, c0: i32, c1: i32| {
        let mut res = Vec::new();
        let mut c = c0;
        while c <= c1 {
            if !ink(c, r) {
                c += 1;
                continue;
            }
            let mut e = c;
            while e < c1 && ink(e + 1, r) {
                e += 1;
            }
            res.push((c, e));
            c = e + 1;
        }
        res
    };
    for r in 0..h {
        for (c0, c1) in runs_in(r, 0, w - 1) {
            for dir in [-1, 1] {
                let s = runs_in(r + dir, c0, c1);
                if s.len() != 1 {
                    continue;
                }
                let (s0, s1) = s[0];
                if s1 - s0 + 1 > 2
                    || !runs_in(r - dir, c0, c1).is_empty()
                    || runs_in(r + 2 * dir, s0, s1).is_empty()
                {
                    continue;
                }
                let (el, er) = (s0 - c0, c1 - s1);
                if (el == 0 && er == 0) || el > 2 || er > 2 {
                    continue;
                }
                let side: Vec<i32> = (c0..s0).chain(s1 + 1..=c1).collect();
                // A curve meeting the stem continues diagonally into the next
                // row; a serif touches nothing there.
                if side.iter().any(|&c| {
                    [-1, 0, 1]
                        .iter()
                        .any(|&dc| (c + dc < s0 || c + dc > s1) && ink(c + dc, r + dir))
                }) {
                    continue;
                }
                for c in side {
                    clear(c, r);
                }
            }
            if c1 - c0 + 1 < 4 {
                continue;
            }
            for c in [c0, c1] {
                for dir in [-1, 1] {
                    let mut n = 0;
                    while n < 3 && ink(c, r + dir * (n + 1)) {
                        n += 1;
                    }
                    if n == 0 || n > 2 {
                        continue;
                    }
                    let mut lone = true;
                    for k in 1..=n {
                        if ink(c - 1, r + dir * k) || ink(c + 1, r + dir * k) {
                            lone = false;
                        }
                    }
                    let tip = r + dir * (n + 1);
                    if ink(c - 1, tip) || ink(c, tip) || ink(c + 1, tip) {
                        lone = false;
                    }
                    if lone {
                        for k in 1..=n {
                            clear(c, r + dir * k);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Pixels in insertion order (the prototype iterates a JS `Set`).
#[derive(Default)]
struct PixSet {
    order: Vec<Px>,
    set: HashSet<Px>,
}

impl PixSet {
    fn add(&mut self, p: Px) {
        if self.set.insert(p) {
            self.order.push(p);
        }
    }
    fn has(&self, x: i32, y: i32) -> bool {
        self.set.contains(&(x, y))
    }
    fn nb(&self, p: Px) -> Vec<Px> {
        let (x, y) = p;
        let mut out = Vec::new();
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            if self.has(x + dx, y + dy) {
                out.push((x + dx, y + dy));
            }
        }
        for (dx, dy) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
            if self.has(x + dx, y + dy) && !self.has(x + dx, y) && !self.has(x, y + dy) {
                out.push((x + dx, y + dy));
            }
        }
        out
    }
}

fn scale2x(src: &PixSet) -> PixSet {
    let mut out = PixSet::default();
    let Some(&(x0, y0)) = src.order.first() else {
        return out;
    };
    let (mut xl, mut xh, mut yl, mut yh) = (x0, x0, y0, y0);
    for &(x, y) in &src.order {
        xl = xl.min(x);
        xh = xh.max(x);
        yl = yl.min(y);
        yh = yh.max(y);
    }
    for y in yl - 1..=yh + 1 {
        for x in xl - 1..=xh + 1 {
            let (p, a, b, c, d) = (
                src.has(x, y),
                src.has(x, y - 1),
                src.has(x + 1, y),
                src.has(x - 1, y),
                src.has(x, y + 1),
            );
            let mut q = [p; 4];
            if c == a && c != d && a != b {
                q[0] = a;
            }
            if a == b && a != c && b != d {
                q[1] = b;
            }
            if d == c && d != b && c != a {
                q[2] = c;
            }
            if b == d && b != a && d != c {
                q[3] = d;
            }
            for (i, (dx, dy)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                if q[i] {
                    out.add((2 * x + dx, 2 * y + dy));
                }
            }
        }
    }
    out
}

fn thin(s: &mut PixSet) {
    let mut changed = true;
    while changed {
        changed = false;
        for step in [0, 1] {
            let mut del = Vec::new();
            for &(x, y) in &s.order {
                let n = [
                    s.has(x, y - 1),
                    s.has(x + 1, y - 1),
                    s.has(x + 1, y),
                    s.has(x + 1, y + 1),
                    s.has(x, y + 1),
                    s.has(x - 1, y + 1),
                    s.has(x - 1, y),
                    s.has(x - 1, y - 1),
                ];
                let b = n.iter().filter(|&&v| v).count();
                let a = (0..8).filter(|&i| !n[i] && n[(i + 1) % 8]).count();
                let (p2, p4, p6, p8) = (n[0], n[2], n[4], n[6]);
                let ok = if step == 0 {
                    !(p4 && p6 && (p2 || p8))
                } else {
                    !(p2 && p8 && (p4 || p6))
                };
                if (2..=6).contains(&b) && a == 1 && ok {
                    del.push((x, y));
                }
            }
            if !del.is_empty() {
                changed = true;
                for d in &del {
                    s.set.remove(d);
                }
                s.order.retain(|p| s.set.contains(p));
            }
        }
    }
}

type Edge = (Px, Px);

fn edge(a: Px, b: Px) -> Edge {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

struct Graph {
    paths: Vec<Vec<Px>>,
    deg: HashMap<Px, usize>,
    centre: HashMap<Px, [f64; 2]>,
}

fn trace(s: &PixSet) -> Graph {
    let deg: HashMap<Px, usize> = s.order.iter().map(|&p| (p, s.nb(p).len())).collect();
    let key = |p: Px| deg.get(&p) != Some(&2);
    let mut centre: HashMap<Px, [f64; 2]> = HashMap::new();
    let mut seen: HashSet<Px> = HashSet::new();
    for &p in &s.order {
        if deg[&p] < 3 || seen.contains(&p) {
            continue;
        }
        let (mut cl, mut st) = (vec![p], vec![p]);
        seen.insert(p);
        while let Some(t) = st.pop() {
            for q in s.nb(t) {
                if deg[&q] >= 3 && seen.insert(q) {
                    cl.push(q);
                    st.push(q);
                }
            }
        }
        let n = cl.len() as f64;
        let c = [
            cl.iter().map(|q| f64::from(q.0)).sum::<f64>() / n,
            cl.iter().map(|q| f64::from(q.1)).sum::<f64>() / n,
        ];
        for q in cl {
            centre.insert(q, c);
        }
    }
    let mut used: HashSet<Edge> = HashSet::new();
    let mut paths = Vec::new();
    let walk = |start: Px, next: Px, used: &mut HashSet<Edge>| {
        let mut path = vec![start, next];
        used.insert(edge(start, next));
        let (mut prev, mut cur) = (start, next);
        while !key(cur) {
            let Some(nx) = s
                .nb(cur)
                .into_iter()
                .find(|&q| q != prev && !used.contains(&edge(cur, q)))
            else {
                break;
            };
            used.insert(edge(cur, nx));
            path.push(nx);
            prev = cur;
            cur = nx;
            if cur == start {
                break;
            }
        }
        path
    };
    for &p in &s.order {
        if key(p) {
            for q in s.nb(p) {
                if !used.contains(&edge(p, q)) {
                    paths.push(walk(p, q, &mut used));
                }
            }
        }
    }
    for &p in &s.order {
        for q in s.nb(p) {
            if !used.contains(&edge(p, q)) {
                paths.push(walk(p, q, &mut used));
            }
        }
    }
    Graph { paths, deg, centre }
}

type P64 = [f64; 2];

fn sub(a: P64, b: P64) -> P64 {
    [a[0] - b[0], a[1] - b[1]]
}
fn dist(a: P64, b: P64) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn cross(a: P64, b: P64) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn norm(a: P64) -> P64 {
    let l = a[0].hypot(a[1]);
    let l = if l == 0.0 { 1.0 } else { l };
    [a[0] / l, a[1] / l]
}

fn rdp(pts: &[P64], eps: f64) -> Vec<P64> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let (mut best, mut bi) = (-1.0, 0);
    for (i, &p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
        let d = if dist(a, b) < 1e-9 {
            dist(p, a)
        } else {
            cross(sub(b, a), sub(p, a)).abs() / dist(a, b)
        };
        if d > best {
            best = d;
            bi = i;
        }
    }
    if best <= eps {
        return vec![a, b];
    }
    let mut left = rdp(&pts[..=bi], eps);
    left.pop();
    left.extend(rdp(&pts[bi..], eps));
    left
}

/// Polyline to lines with tangent fillets at gentle vertices.
fn poly_to_prims(poly: &[P64], closed: bool) -> Vec<Prim> {
    let verts = if closed {
        &poly[..poly.len().saturating_sub(1)]
    } else {
        poly
    };
    let m = verts.len();
    if m < 2 {
        return Vec::new();
    }
    let (mut trim_start, mut trim_end) = (vec![0.0f64; m], vec![0.0f64; m]);
    let mut fillets = Vec::new();
    let idx: Vec<usize> = if closed {
        (0..m).collect()
    } else {
        (1..m - 1).collect()
    };
    for i in idx {
        let (prev, v, next) = (verts[(i + m - 1) % m], verts[i], verts[(i + 1) % m]);
        let (u, w) = (norm(sub(v, prev)), norm(sub(next, v)));
        let turn = (u[0] * w[0] + u[1] * w[1]).clamp(-1.0, 1.0).acos();
        if turn < 1e-3 || turn > 55f64.to_radians() {
            continue;
        }
        let t = (0.48 * dist(prev, v)).min(0.48 * dist(v, next));
        let rho = t / (turn / 2.0).tan();
        let p1 = [v[0] - u[0] * t, v[1] - u[1] * t];
        let p2 = [v[0] + w[0] * t, v[1] + w[1] * t];
        let left = cross(u, w) > 0.0;
        let nrm = if left { [-u[1], u[0]] } else { [u[1], -u[0]] };
        let c = [p1[0] + nrm[0] * rho, p1[1] + nrm[1] * rho];
        let ang = |p: P64| (p[1] - c[1]).atan2(p[0] - c[0]).to_degrees();
        let (mut a0, mut a1) = (ang(p1), ang(p2));
        if !left {
            std::mem::swap(&mut a0, &mut a1);
        }
        while a1 < a0 {
            a1 += 360.0;
        }
        fillets.push(Prim::Arc {
            c: [c[0] as f32, c[1] as f32],
            r: rho as f32,
            a0: a0 as f32,
            a1: a1 as f32,
            full: false,
        });
        trim_end[(i + m - 1) % m] = t;
        trim_start[i] = t;
    }
    let segs = if closed { m } else { m - 1 };
    let mut out = Vec::new();
    for i in 0..segs {
        let (a0, b0) = (verts[i], verts[(i + 1) % m]);
        let d = norm(sub(b0, a0));
        let a = [a0[0] + d[0] * trim_start[i], a0[1] + d[1] * trim_start[i]];
        let b = [b0[0] - d[0] * trim_end[i], b0[1] - d[1] * trim_end[i]];
        if dist(a, b) > 0.05 {
            out.push(Prim::Line {
                a: [a[0] as f32, a[1] as f32],
                b: [b[0] as f32, b[1] as f32],
            });
        }
    }
    out.extend(fillets);
    out
}

/// Read a bitmap glyph as strokes in model units (x-height 8, y up).
pub fn vectorize(rows: &[u8], m: &FaceMetrics) -> Vec<Prim> {
    let (w, h) = (i32::from(m.width), i32::from(m.height));
    let (base, k) = (f64::from(m.baseline), f64::from(m.k));
    let ink = |c: i32, r: i32| ink_in(rows, m.width, m.height, c, r);
    let p_at = |c: f64, r: f64| -> P64 { [(c + 0.5) * k, (base - r - 0.5) * k] };
    let mut prims: Vec<Prim> = Vec::new();
    // 8-connected components, in (row, column) discovery order.
    let mut seen: HashSet<Px> = HashSet::new();
    let mut s = PixSet::default();
    for r in 0..h {
        for c in 0..w {
            if !ink(c, r) || seen.contains(&(c, r)) {
                continue;
            }
            let (mut comp, mut stack) = (Vec::new(), vec![(c, r)]);
            seen.insert((c, r));
            while let Some((x, y)) = stack.pop() {
                comp.push((x, y));
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        if ink(nx, ny) && seen.insert((nx, ny)) {
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            let span = |f: fn(&Px) -> i32| {
                let v = comp.iter().map(f);
                v.clone().max().unwrap_or(0) - v.min().unwrap_or(0)
            };
            if comp.len() <= 4 && span(|p| p.0) <= 1 && span(|p| p.1) <= 1 {
                let n = comp.len() as f64;
                let cx = comp.iter().map(|p| f64::from(p.0)).sum::<f64>() / n;
                let cy = comp.iter().map(|p| f64::from(p.1)).sum::<f64>() / n;
                let c = p_at(cx, cy);
                prims.push(Prim::Arc {
                    c: [c[0] as f32, c[1] as f32],
                    r: 0.7,
                    a0: 0.0,
                    a1: 360.0,
                    full: true,
                });
            } else {
                for (x, y) in comp {
                    s.add((x, y));
                }
            }
        }
    }
    for _ in 0..PASSES {
        s = scale2x(&s);
    }
    let up = f64::from(1u32 << PASSES);
    thin(&mut s);
    let mut g = trace(&s);
    for _ in 0..3 {
        let mut pruned = false;
        for path in &g.paths {
            let (a, b) = (path[0], path[path.len() - 1]);
            let (da, db) = (g.deg[&a], g.deg[&b]);
            let (end_a, end_b) = (da == 1, db == 1);
            if path.len() <= PRUNE && ((end_a && db >= 3) || (end_b && da >= 3)) {
                let del = if end_a {
                    &path[..path.len() - 1]
                } else {
                    &path[1..]
                };
                for d in del {
                    s.set.remove(d);
                }
                pruned = true;
            }
        }
        if !pruned {
            break;
        }
        s.order.retain(|p| s.set.contains(p));
        g = trace(&s);
    }
    for path in &g.paths {
        let mut pts: Vec<P64> = Vec::new();
        for p in path {
            let v = g
                .centre
                .get(p)
                .copied()
                .unwrap_or([f64::from(p.0), f64::from(p.1)]);
            if pts.last() != Some(&v) {
                pts.push(v);
            }
        }
        if pts.len() < 2 {
            continue;
        }
        let closed = path.len() > 2 && path[0] == path[path.len() - 1];
        let qpt = |p: &P64| -> P64 { [(p[0] + 0.5) / up * k, (base - (p[1] + 0.5) / up) * k] };
        let qs: Vec<P64> = pts.iter().map(qpt).collect();
        let mut poly = rdp(&qs, 0.3 * k);
        if closed && poly.len() < 4 {
            poly = pts.iter().step_by(4).map(qpt).collect();
        }
        prims.extend(poly_to_prims(&poly, closed));
    }
    // Put back one-pixel bars the thinning took for bumps.
    for r in 0..h {
        let mut c = 0;
        while c < w {
            if !ink(c, r) {
                c += 1;
                continue;
            }
            let mut e = c;
            while e + 1 < w && ink(e + 1, r) {
                e += 1;
            }
            let above = (c..=e).filter(|&x| ink(x, r - 1)).count();
            let below = (c..=e).filter(|&x| ink(x, r + 1)).count();
            if e - c + 1 >= 3 && above <= 2 && below <= 2 {
                let [x0, y] = p_at(f64::from(c), f64::from(r));
                let x1 = p_at(f64::from(e), f64::from(r))[0];
                let (mut near, mut total) = (0, 0);
                let mut x = x0;
                while x <= x1 + 1e-6 {
                    total += 1;
                    let pt: V2 = [x as f32, y as f32];
                    if prims.iter().any(|p| dist_prim(p, pt) < 0.8) {
                        near += 1;
                    }
                    x += 0.4;
                }
                if f64::from(near) < 0.7 * f64::from(total) {
                    prims.push(Prim::Line {
                        a: [x0 as f32, y as f32],
                        b: [x1 as f32, y as f32],
                    });
                }
            }
            c = e + 1;
        }
    }
    prims
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::FontFamily;
    use crate::bitmap_fonts::face;
    use crate::lace::connectors::analyse;

    const ALPHABETS: [&str; 4] = [
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "abcdefghijklmnopqrstuvwxyz",
        "АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯ",
        "абвгґдеєжзиіїйклмнопрстуфхцчшщьюя",
    ];

    fn letters() -> Vec<char> {
        ALPHABETS.iter().flat_map(|a| a.chars()).collect()
    }

    fn vga() -> (impl Fn(char) -> Option<Vec<u8>>, FaceMetrics) {
        let f = face(FontFamily::Vga);
        let g = move |c| f.glyph(c).map(<[u8]>::to_vec);
        let m = FaceMetrics::measure(g, f.width, f.height).unwrap();
        (g, m)
    }

    fn row(rows: &[u8], r: usize) -> String {
        (0..8)
            .map(|c| if rows[r] & (0x80 >> c) != 0 { '#' } else { '.' })
            .collect()
    }

    #[test]
    fn vga_metrics_match_the_prototype() {
        let (_, m) = vga();
        assert_eq!((m.baseline, m.x_top), (12, 5));
        assert!((m.k - 1.2308).abs() < 1e-3);
    }

    #[test]
    fn serif_detector_cases() {
        let (g, _) = vga();
        let strip = |c: char| strip_serifs(&g(c).unwrap(), 8, 16);
        let f = strip('f');
        assert_eq!(row(&g('f').unwrap(), 11), "####....");
        assert_eq!(row(&f, 11), ".##.....");
        assert_eq!(row(&f, 6), "####....");
        let t = strip('т');
        assert_eq!(
            (row(&t, 6), row(&t, 11)),
            ("...##...".into(), "...##...".into())
        );
        assert_eq!(row(&g('a').unwrap(), 11), ".###.##.");
        assert_eq!(row(&strip('a'), 11), ".###.#..");
        assert_eq!(row(&g('n').unwrap(), 5), "##.###..");
        assert_eq!(row(&strip('n'), 5), ".#.###..");
    }

    #[test]
    fn serifs_found_in_51_of_118_letters() {
        let (g, _) = vga();
        let all = letters();
        assert_eq!(all.len(), 118);
        let n = all
            .iter()
            .filter(|&&c| {
                let r = g(c).unwrap();
                strip_serifs(&r, 8, 16) != r
            })
            .count();
        assert_eq!(n, 51);
    }

    #[test]
    fn vga_f_keeps_its_crossbar() {
        let (g, m) = vga();
        let prims = vectorize(&strip_serifs(&g('f').unwrap(), 8, 16), &m);
        assert!(prims.iter().any(|p| matches!(p,
            Prim::Line { a, b } if (a[1] - 6.8).abs() < 0.3 && (b[1] - 6.8).abs() < 0.3
                && (a[0] - b[0]).abs() > 0.6)));
    }

    #[test]
    fn vga_round_letters_are_smooth() {
        let (g, m) = vga();
        let glyph = |c| analyse(vectorize(&strip_serifs(&g(c).unwrap(), 8, 16), &m));
        assert!(glyph('o').rays.is_empty());
        assert_eq!(glyph('c').rays.len(), 2);
    }

    #[test]
    fn connectors_per_letter() {
        let (g, m) = vga();
        let avg = |strip: bool| {
            let all = letters();
            let total: usize = all
                .iter()
                .map(|&c| {
                    let r = g(c).unwrap();
                    let r = if strip { strip_serifs(&r, 8, 16) } else { r };
                    analyse(vectorize(&r, &m)).rays.len()
                })
                .sum();
            total as f32 / all.len() as f32
        };
        let (s, u) = (avg(true), avg(false));
        assert!((s - 5.4).abs() < 0.2, "stripped {s}");
        assert!((u - 6.7).abs() < 0.2, "unstripped {u}");
    }

    #[test]
    fn wide_faces_read_multi_byte_rows() {
        // 13 px wide, 5 rows high; the middle row is fully inked.
        let mut rows = vec![0u8; 2 * 5];
        rows[4] = 0xff;
        rows[5] = 0xf8;
        let m = FaceMetrics {
            width: 13,
            height: 5,
            baseline: 3,
            x_top: 0,
            k: 8.0 / 2.5,
        };
        let prims = vectorize(&rows, &m);
        let len: f32 = prims
            .iter()
            .map(|p| match p {
                Prim::Line { a, b } => (b[0] - a[0]).hypot(b[1] - a[1]),
                Prim::Arc { .. } => 0.0,
            })
            .sum();
        assert_eq!(prims.len(), 1, "{prims:?}");
        // Thinning shortens each end by 0.875 px, as in the prototype (8 px -> 6.25 px).
        assert!((len - 11.25 * m.k).abs() < 0.5, "{len}");
        let Prim::Line { a, b } = prims[0] else {
            panic!()
        };
        assert!((a[1] - b[1]).abs() < 1e-4);
    }

    #[test]
    fn unavailable_face_reports_why() {
        let none = |_| None;
        assert_eq!(
            FaceMetrics::measure(none, 4, 6),
            Err("cell 4×6 is outside 6–32 × 8–64".to_string())
        );
        assert_eq!(
            FaceMetrics::measure(none, 8, 16),
            Err("face has no x to measure".to_string())
        );
    }
}
