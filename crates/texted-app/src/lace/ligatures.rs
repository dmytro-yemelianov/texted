//! Ligatures from the connector model, in three situations.
//!
//! In a word: a top bar meets a top bar (fuse), a bar runs onto a stem top
//! (bar and stem), strokes coincide (share), or a free arc end leaves its arc
//! tangentially and lands on a stem top (hook or arch); the stem top may sit
//! under a dot that the ligature absorbs. Across a space: top bars continue
//! (rail) or a two-arc bridge crosses the gap (swash). Between rows: a descender
//! continues straight into a stem below (stem) or a two-arc bridge goes down
//! (tail).
//!
//! The first glyph `g` sits at the origin and `h` at `(dx, dy)`, y up, so a row
//! below has a negative `dy`. All paths are in `g`'s coordinates.

use super::connectors::Glyph;
use super::geom::{arc_point, dist_prim, Prim, V2};

/// How a ligature joins two letters; lower [`Mech::order`] is preferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mech {
    Share,
    Fuse,
    BarStem,
    Hook,
    Arch,
    Rail,
    Swash,
    Stem,
    Tail,
}

impl Mech {
    pub fn order(self) -> u8 {
        match self {
            Mech::Share => 0,
            Mech::Fuse | Mech::BarStem | Mech::Hook | Mech::Arch => 1,
            Mech::Rail | Mech::Swash => 2,
            Mech::Stem | Mech::Tail => 3,
        }
    }
}

/// The letter a key or edit belongs to: the first (`G`) or the second (`H`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Side {
    G,
    H,
}

/// A resource a ligature uses up; a candidate conflicts with another sharing a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Ray(Side, u16),
    Latent(Side, u16),
    Dot(Side, u16),
    /// A free stroke end: stroke index and end (`a`/`a0` is 0, `b`/`a1` is 1).
    Terminal(Side, u16, u8),
    Drop(Side, u16),
    Rerouted(Side, u16),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub mech: Mech,
    pub cost: f32,
    pub keys: Vec<Key>,
    /// The connecting polyline in `g`'s coordinates (a straight join has two points).
    pub path: Vec<V2>,
    /// An arc cut short to leave from the bridge's start: side, stroke, kept part.
    /// Arcs of `h` are in `g`'s coordinates.
    pub trim: Option<(Side, u16, Prim)>,
    /// Whole strokes the bridge replaces.
    pub drop: Vec<(Side, u16)>,
    /// A dot of `h` the ligature absorbs.
    pub absorb: Option<(Side, u16)>,
}

fn sub(a: V2, b: V2) -> V2 {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: V2, b: V2) -> V2 {
    [a[0] + b[0], a[1] + b[1]]
}
fn mul(a: V2, k: f32) -> V2 {
    [a[0] * k, a[1] * k]
}
fn dot(a: V2, b: V2) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: V2, b: V2) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}
fn len(a: V2) -> f32 {
    a[0].hypot(a[1])
}
fn dist(a: V2, b: V2) -> f32 {
    len(sub(a, b))
}
fn neg(a: V2) -> V2 {
    [-a[0], -a[1]]
}
fn norm(a: V2) -> V2 {
    let l = len(a);
    let l = if l == 0.0 { 1.0 } else { l };
    mul(a, 1.0 / l)
}
fn ang_of(v: V2) -> f32 {
    v[1].atan2(v[0]).to_degrees()
}

fn shift(p: &Prim, o: V2) -> Prim {
    match *p {
        Prim::Line { a, b } => Prim::Line {
            a: add(a, o),
            b: add(b, o),
        },
        Prim::Arc { c, r, a0, a1, full } => Prim::Arc {
            c: add(c, o),
            r,
            a0,
            a1,
            full,
        },
    }
}

fn prim_len(p: &Prim) -> f32 {
    match *p {
        Prim::Line { a, b } => dist(a, b),
        Prim::Arc {
            r, a0, a1, full, ..
        } => r * if full { 360.0 } else { a1 - a0 }.to_radians(),
    }
}

/// An arc leaving `s` along unit tangent `ts` and ending at `e`.
struct ArcPath {
    pts: Vec<V2>,
    turn: f32,
    r: f32,
}

fn arc_points(s: V2, ts: V2, e: V2) -> ArcPath {
    let n = [-ts[1], ts[0]];
    let w = sub(e, s);
    let k = dot(n, w);
    if k.abs() < 1e-6 {
        return ArcPath {
            pts: vec![s, e],
            turn: 0.0,
            r: f32::INFINITY,
        };
    }
    let sr = dot(w, w) / (2.0 * k);
    let c = add(s, mul(n, sr));
    let r = sr.abs();
    let (a_s, a_e) = (ang_of(sub(s, c)), ang_of(sub(e, c)));
    let m = |x: f32| (x % 360.0 + 360.0) % 360.0;
    let sweep = if sr > 0.0 {
        m(a_e - a_s)
    } else {
        -m(a_s - a_e)
    };
    let steps = ((sweep.abs() / 6.0).ceil() as usize).max(2);
    let pts = (0..=steps)
        .map(|i| arc_point(c, r, a_s + sweep * i as f32 / steps as f32))
        .collect();
    ArcPath {
        pts,
        turn: sweep.abs(),
        r,
    }
}

/// Two arcs from `p1` (tangent `t1`) to `p2` (arriving with tangent `t2`).
#[derive(Clone, Debug)]
pub struct Biarc {
    pub pts: Vec<V2>,
    /// Total turning in degrees.
    pub turn: f32,
    pub rmin: f32,
    pub length: f32,
}

pub fn biarc(p1: V2, t1: V2, p2: V2, t2: V2) -> Option<Biarc> {
    let v = sub(p2, p1);
    let t = add(t1, t2);
    let denom = 2.0 * (1.0 - dot(t1, t2));
    let vt = dot(v, t);
    let alpha = if denom < 1e-6 {
        // Parallel tangents: an S-curve.
        if vt <= 1e-9 {
            return None;
        }
        dot(v, v) / (2.0 * vt)
    } else {
        (-vt + (vt * vt + denom * dot(v, v)).sqrt()) / denom
    };
    if alpha.is_nan() || alpha <= 0.0 {
        return None;
    }
    let j = mul(add(add(p1, mul(t1, alpha)), sub(p2, mul(t2, alpha))), 0.5);
    let a1 = arc_points(p1, t1, j);
    let a2 = arc_points(p2, neg(t2), j);
    let mut pts = a1.pts;
    pts.extend(a2.pts[..a2.pts.len() - 1].iter().rev());
    let length = pts.windows(2).map(|w| dist(w[0], w[1])).sum();
    Some(Biarc {
        pts,
        turn: a1.turn + a2.turn,
        rmin: a1.r.min(a2.r),
        length,
    })
}

/// A free stroke end a ligature may reroute, with the strokes on the way to it.
struct Terminal {
    qi: usize,
    end: u8,
    drops: Vec<usize>,
    max_cut: f32,
    keys: Vec<Key>,
}

fn ends(q: &Prim) -> Vec<(u8, V2)> {
    match *q {
        Prim::Line { a, b } => vec![(0, a), (1, b)],
        Prim::Arc { full: true, .. } => Vec::new(),
        Prim::Arc { c, r, a0, a1, .. } => vec![(0, arc_point(c, r, a0)), (1, arc_point(c, r, a1))],
    }
}

fn plen_open(q: &Prim) -> f32 {
    match *q {
        Prim::Line { a, b } => dist(a, b),
        Prim::Arc { r, a0, a1, .. } => r * (a1 - a0).to_radians(),
    }
}

/// Free ends of `g`. From each, walk back along the stroke while it continues
/// end to end (up to 110 degrees of turning and 9 units, stopping at junctions):
/// every arc on the way can be the departure point.
fn free_terminals(g: &Glyph) -> Vec<Terminal> {
    let touching = |p: V2, except: &[usize]| -> Vec<usize> {
        (0..g.prims.len())
            .filter(|i| !except.contains(i) && dist_prim(&g.prims[*i], p) < 0.05)
            .collect()
    };
    let mut out = Vec::new();
    for (i0, q0) in g.prims.iter().enumerate() {
        for (e0, f) in ends(q0) {
            if !touching(f, &[i0]).is_empty() {
                continue;
            }
            let rk = g
                .rays
                .iter()
                .position(|r| r.prim as usize == i0 && dist(r.p, f) < 0.05);
            let mut base = vec![Key::Terminal(Side::G, i0 as u16, e0)];
            base.extend(rk.map(|k| Key::Ray(Side::G, k as u16)));
            let mut drops: Vec<usize> = Vec::new();
            let (mut cur, mut entry, mut turn, mut length) = (i0, e0, 0.0f32, 0.0f32);
            for _ in 0..6 {
                let q = &g.prims[cur];
                if let Prim::Arc { a0, a1, .. } = *q {
                    let mut keys = base.clone();
                    keys.extend(drops.iter().map(|d| Key::Drop(Side::G, *d as u16)));
                    keys.push(Key::Rerouted(Side::G, cur as u16));
                    out.push(Terminal {
                        qi: cur,
                        end: entry,
                        drops: drops.clone(),
                        max_cut: (a1 - a0 - 5.0).min(110.0 - turn).max(0.0),
                        keys,
                    });
                    turn += a1 - a0;
                }
                length += plen_open(q);
                if turn >= 110.0 || length > 9.0 {
                    break;
                }
                let Some((_, other)) = ends(q).into_iter().find(|(e, _)| *e != entry) else {
                    break;
                };
                let next = touching(other, &[cur]);
                if next.len() != 1 {
                    break;
                }
                let Some((ne, _)) = ends(&g.prims[next[0]])
                    .into_iter()
                    .find(|(_, pn)| dist(*pn, other) < 0.05)
                else {
                    break;
                };
                drops.push(cur);
                cur = next[0];
                entry = ne;
            }
        }
    }
    out
}

struct Departure {
    s: V2,
    d: V2,
    kept: Prim,
}

/// Points of the terminal arc where a bridge may leave. Only the end of an arc
/// is rerouted: at most 100 degrees and 60 % of it, so the letter stays readable.
fn departures(g: &Glyph, term: &Terminal) -> Vec<Departure> {
    let q = &g.prims[term.qi];
    let Prim::Arc { c, r, a0, a1, full } = *q else {
        return Vec::new();
    };
    let max_cut = if term.drops.is_empty() {
        100.0f32.min(0.6 * (a1 - a0))
    } else {
        term.max_cut
    };
    let mut out = Vec::new();
    let mut k = 0;
    while k as f32 * 5.0 <= max_cut {
        let off = k as f32 * 5.0;
        let th = if term.end == 0 { a0 + off } else { a1 - off };
        if (term.end == 0 && th > a1 - 10.0) || (term.end == 1 && th < a0 + 10.0) {
            break;
        }
        let (sn, cs) = th.to_radians().sin_cos();
        let (d, kept) = if term.end == 0 {
            (
                [sn, -cs],
                Prim::Arc {
                    c,
                    r,
                    a0: th,
                    a1,
                    full,
                },
            )
        } else {
            (
                [-sn, cs],
                Prim::Arc {
                    c,
                    r,
                    a0,
                    a1: th,
                    full,
                },
            )
        };
        out.push(Departure {
            s: arc_point(c, r, th),
            d,
            kept,
        });
        k += 1;
    }
    out
}

struct Target {
    p: V2,
    d: V2,
    prim: u16,
    absorbs: Option<u16>,
    keys: Vec<Key>,
}

fn targets_of(h: &Glyph, off: V2, pred: fn(V2, V2) -> bool) -> Vec<Target> {
    let mut out = Vec::new();
    for (k, r) in h.rays.iter().enumerate() {
        if pred(r.p, r.d) {
            out.push(Target {
                p: add(r.p, off),
                d: r.d,
                prim: r.prim,
                absorbs: None,
                keys: vec![Key::Ray(Side::H, k as u16)],
            });
        }
    }
    for (k, l) in h.latent.iter().enumerate() {
        if pred(l.ray.p, l.ray.d) {
            out.push(Target {
                p: add(l.ray.p, off),
                d: l.ray.d,
                prim: l.ray.prim,
                absorbs: Some(l.absorbs),
                keys: vec![Key::Latent(Side::H, k as u16), Key::Dot(Side::H, l.absorbs)],
            });
        }
    }
    out
}

/// Everything a bridge generator needs: `g` at the origin, `h` shifted by `off`.
struct Pair<'a> {
    g: &'a Glyph,
    h: &'a Glyph,
    off: V2,
    hw: Vec<Prim>,
}

impl<'a> Pair<'a> {
    fn new(g: &'a Glyph, h: &'a Glyph, off: V2) -> Self {
        let hw = h.prims.iter().map(|p| shift(p, off)).collect();
        Pair { g, h, off, hw }
    }

    /// The bridge stays off both letters except the rerouted and target strokes.
    fn clear_path(
        &self,
        pts: &[V2],
        s: V2,
        t: V2,
        skip: &[(Side, u16)],
        over: Option<(u16, Prim)>,
    ) -> bool {
        for &p in pts {
            if dist(p, s) < 0.7 || dist(p, t) < 0.7 {
                continue;
            }
            for (side, prims) in [(Side::G, &self.g.prims), (Side::H, &self.hw)] {
                for (qi, q) in prims.iter().enumerate() {
                    let key = (side, qi as u16);
                    let q = match over {
                        Some((oi, ref o)) if side == Side::G && oi as usize == qi => o,
                        _ if skip.contains(&key) => continue,
                        _ => q,
                    };
                    if dist_prim(q, p) < 0.4 {
                        return false;
                    }
                }
            }
        }
        true
    }
}

struct Rules {
    target: fn(V2, V2) -> bool,
    depart: Option<fn(V2, V2) -> bool>,
    from_ray: Option<fn(V2, V2) -> bool>,
    max_turn: f32,
    max_len: f32,
    rmin: f32,
    no_dive: bool,
    band: Option<fn(f32) -> bool>,
}

struct Bridge {
    pts: Vec<V2>,
    cost: f32,
    straight: bool,
}

fn try_bridge(
    pr: &Pair,
    s: V2,
    d: V2,
    t: &Target,
    o: &Rules,
    skip: &[(Side, u16)],
    over: Option<(u16, Prim)>,
) -> Option<Bridge> {
    let v = sub(t.p, s);
    let l = len(v);
    let (pts, turn, length, rmin) = if l > 0.05
        && cross(d, v).abs() / l < 0.03
        && dot(d, v) > 0.0
        && dot(neg(t.d), mul(v, 1.0 / l)) > 0.999
    {
        (vec![s, t.p], 0.0, l, f32::INFINITY)
    } else {
        let b = biarc(s, d, t.p, neg(t.d))?;
        (b.pts, b.turn, b.length, b.rmin)
    };
    if rmin < o.rmin || turn > o.max_turn || length > o.max_len {
        return None;
    }
    if o.no_dive {
        let lo = pts.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        if lo < s[1].min(t.p[1]) - 0.6 {
            return None;
        }
    }
    if let Some(band) = o.band {
        if !pts.iter().all(|p| band(p[1])) {
            return None;
        }
    }
    if !pr.clear_path(&pts, s, t.p, skip, over) {
        return None;
    }
    Some(Bridge {
        pts,
        cost: length + turn * 0.05,
        straight: turn == 0.0,
    })
}

/// A bridge candidate with the height its path starts from.
struct Found {
    cand: Candidate,
    straight: bool,
    src_y: f32,
}

fn bridges(pr: &Pair, o: &Rules) -> Vec<Found> {
    let (g, h) = (pr.g, pr.h);
    let mut out = Vec::new();
    let tg = targets_of(h, pr.off, o.target);
    let skip_t = |t: &Target| {
        let mut s = vec![(Side::H, t.prim)];
        s.extend(t.absorbs.map(|a| (Side::H, a)));
        s
    };
    let ink: f32 = g.prims.iter().map(prim_len).sum();
    if let Some(depart) = o.depart {
        for term in free_terminals(g) {
            for dep in departures(g, &term) {
                if !depart(dep.s, dep.d) {
                    continue;
                }
                // A ligature reroutes a stroke end; it never takes away more than a quarter of the
                // letter (the tolerance keeps exact ties, which the prototype's doubles also keep).
                let removed: f32 = term
                    .drops
                    .iter()
                    .map(|d| prim_len(&g.prims[*d]))
                    .sum::<f32>()
                    + prim_len(&g.prims[term.qi])
                    - prim_len(&dep.kept);
                if removed > 0.25 * ink + 1e-4 {
                    continue;
                }
                for t in &tg {
                    let mut skip = skip_t(t);
                    skip.extend(term.drops.iter().map(|d| (Side::G, *d as u16)));
                    let over = Some((term.qi as u16, dep.kept));
                    let Some(b) = try_bridge(pr, dep.s, dep.d, t, o, &skip, over) else {
                        continue;
                    };
                    let mut keys = term.keys.clone();
                    keys.extend(t.keys.iter().copied());
                    out.push(Found {
                        cand: Candidate {
                            mech: Mech::Arch,
                            cost: b.cost,
                            keys,
                            path: b.pts,
                            trim: Some((Side::G, term.qi as u16, dep.kept)),
                            drop: term.drops.iter().map(|d| (Side::G, *d as u16)).collect(),
                            absorb: t.absorbs.map(|a| (Side::H, a)),
                        },
                        straight: b.straight,
                        src_y: dep.s[1],
                    });
                }
            }
        }
    }
    if let Some(from_ray) = o.from_ray {
        for (k, r) in g.rays.iter().enumerate() {
            if !from_ray(r.p, r.d) {
                continue;
            }
            for t in &tg {
                let mut skip = vec![(Side::G, r.prim)];
                skip.extend(skip_t(t));
                let Some(b) = try_bridge(pr, r.p, r.d, t, o, &skip, None) else {
                    continue;
                };
                let mut keys = vec![Key::Ray(Side::G, k as u16)];
                keys.extend(t.keys.iter().copied());
                out.push(Found {
                    cand: Candidate {
                        mech: Mech::Arch,
                        cost: b.cost,
                        keys,
                        path: b.pts,
                        trim: None,
                        drop: Vec::new(),
                        absorb: t.absorbs.map(|a| (Side::H, a)),
                    },
                    straight: b.straight,
                    src_y: r.p[1],
                });
            }
        }
    }
    out
}

/// Top bars of both letters that meet (fuse), or a bar running onto a stem top.
fn bar_joins(g: &Glyph, h: &Glyph, off: V2, max_gap: f32, fuse_only: bool) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (i, a) in g.rays.iter().enumerate() {
        for (j, b) in h.rays.iter().enumerate() {
            let bp = add(b.p, off);
            let dy = (a.p[1] - bp[1]).abs();
            let dx = bp[0] - a.p[0];
            if dy > 1.0 || dx < -0.3 || dx > max_gap || a.p[1] < 6.0 {
                continue;
            }
            let (ad, bd) = (a.d, b.d);
            let mech = if ad[0] > 0.85 && bd[0] < -0.85 {
                (Mech::Fuse, 0.0)
            } else if !fuse_only
                && ((ad[0] > 0.85 && bd[1] > 0.9) || (bd[0] < -0.85 && ad[1] > 0.9))
            {
                (Mech::BarStem, 0.5)
            } else {
                continue;
            };
            out.push(Candidate {
                mech: mech.0,
                cost: dx.max(0.0) + mech.1,
                keys: vec![Key::Ray(Side::G, i as u16), Key::Ray(Side::H, j as u16)],
                path: vec![a.p, bp],
                trim: None,
                drop: Vec::new(),
                absorb: None,
            });
        }
    }
    out
}

/// Collinear overlapping straight strokes of the two letters.
fn shared_strokes(pr: &Pair) -> Vec<Candidate> {
    let mut out = Vec::new();
    for p in &pr.g.prims {
        for q in &pr.hw {
            let (Prim::Line { a: pa, b: pb }, Prim::Line { a: qa, b: qb }) = (p, q) else {
                continue;
            };
            let (u, v) = (norm(sub(*pb, *pa)), norm(sub(*qb, *qa)));
            let (ps, qs) = (Prim::Line { a: *pa, b: *pb }, Prim::Line { a: *qa, b: *qb });
            if cross(u, v).abs() > 0.01
                || (dist_prim(&ps, *qa) > 0.3
                    && dist_prim(&ps, *qb) > 0.3
                    && dist_prim(&qs, *pa) > 0.3)
            {
                continue;
            }
            if cross(sub(*qa, *pa), u).abs() > 0.3 {
                continue;
            }
            out.push(Candidate {
                mech: Mech::Share,
                cost: 0.0,
                keys: Vec::new(),
                path: vec![*pa, *pb],
                trim: None,
                drop: Vec::new(),
                absorb: None,
            });
        }
    }
    out
}

/// In a word: a stem top, or a serif at the top that points back towards the source.
fn word_rules() -> Rules {
    Rules {
        target: |p, d| (d[0] < -0.85 || d[1] > 0.9) && p[1] >= 6.0,
        depart: Some(|_, _| true),
        from_ray: None,
        max_turn: 240.0,
        max_len: 18.0,
        rmin: 1.4,
        no_dive: true,
        band: None,
    }
}

/// Ligatures between `g` at the origin and the next letter `h` at `(dx, 0)`.
pub fn in_word(g: &Glyph, h: &Glyph, dx: f32, max_gap: f32) -> Vec<Candidate> {
    let off = [dx, 0.0];
    let pr = Pair::new(g, h, off);
    let mut out = bar_joins(g, h, off, max_gap, false);
    out.extend(shared_strokes(&pr));
    for mut f in bridges(&pr, &word_rules()) {
        f.cand.mech = if f.src_y >= 9.5 {
            Mech::Hook
        } else {
            Mech::Arch
        };
        out.push(f.cand);
    }
    out
}

/// Ligatures from `g` to the first letter `h` of the next word, `gap` units of space apart.
pub fn across_space(g: &Glyph, h: &Glyph, dx: f32, gap: f32) -> Vec<Candidate> {
    let off = [dx, 0.0];
    let pr = Pair::new(g, h, off);
    let mut out = bar_joins(g, h, off, gap + 3.0, true);
    for c in &mut out {
        c.mech = Mech::Rail;
    }
    let rules = Rules {
        target: |p, d| (d[1] > 0.9 && p[1] >= 6.0) || d[0] < -0.7,
        depart: Some(|_, d| d[0] > 0.2),
        from_ray: Some(|p, d| d[0] > 0.7 || (d[1] > 0.9 && p[1] >= 6.0)),
        max_turn: 220.0,
        max_len: gap * 1.6 + 14.0,
        rmin: 1.6,
        no_dive: false,
        band: Some(|y| y > -3.5 && y < 15.5),
    };
    out.extend(bridges(&pr, &rules).into_iter().map(|mut f| {
        f.cand.mech = Mech::Swash;
        f.cand
    }));
    out
}

/// Ligatures from `g` at the origin down to `h` at `(dx, dy)`, rows `row` units apart.
pub fn between_rows(g: &Glyph, h: &Glyph, dx: f32, dy: f32, row: f32) -> Vec<Candidate> {
    let pr = Pair::new(g, h, [dx, dy]);
    let rules = Rules {
        target: |_, d| d[1] > 0.7,
        depart: Some(|s, d| d[1] < -0.3 && s[1] < 2.0),
        from_ray: Some(|_, d| d[1] < -0.7),
        max_turn: 110.0,
        max_len: row + 6.0,
        rmin: 2.0,
        no_dive: false,
        band: None,
    };
    bridges(&pr, &rules)
        .into_iter()
        .map(|mut f| {
            f.cand.mech = if f.straight { Mech::Stem } else { Mech::Tail };
            f.cand
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::FontFamily;
    use crate::bitmap_fonts::face;
    use crate::lace::connectors::analyse;
    use crate::lace::model_font::model;
    use crate::lace::vectorize::{strip_serifs, vectorize, FaceMetrics};

    fn mg(c: char) -> Glyph {
        analyse(model(c).unwrap())
    }

    fn pair(a: char, b: char) -> Vec<Candidate> {
        in_word(&mg(a), &mg(b), 9.0, 4.5)
    }

    fn mechs(c: &[Candidate]) -> Vec<Mech> {
        c.iter().map(|c| c.mech).collect()
    }

    #[test]
    fn biarc_bends_twice_for_parallel_tangents() {
        let b = biarc([0.0, 0.0], [1.0, 0.0], [10.0, 8.0], [1.0, 0.0]).unwrap();
        let end = *b.pts.last().unwrap();
        assert!(dist(end, [10.0, 8.0]) < 1e-3);
        assert!(b.turn > 0.0);
    }

    #[test]
    fn classic_pairs_on_the_model() {
        let has = |a, b, m| mechs(&pair(a, b)).contains(&m);
        assert!(has('f', 'f', Mech::Fuse));
        assert!(pair('f', 'i')
            .iter()
            .any(|c| c.mech == Mech::Hook && c.absorb.is_some()));
        assert!(has('f', 'l', Mech::Hook));
        assert!(has('f', 't', Mech::Fuse) && has('f', 't', Mech::Hook));
        assert!(pair('f', 'j')
            .iter()
            .any(|c| c.mech == Mech::Hook && c.absorb.is_some()));
        assert!(has('T', 'h', Mech::BarStem));
        for (a, b) in [('c', 't'), ('s', 't'), ('s', 'p'), ('с', 'т')] {
            assert!(has(a, b, Mech::Arch), "{a}{b}");
        }
        assert!(has('т', 'т', Mech::Fuse));
        for (a, b) in [('е', 'е'), ('е', 'в'), ('н', 'н')] {
            assert!(pair(a, b).is_empty(), "{a}{b}: {:?}", mechs(&pair(a, b)));
        }
    }

    fn vga_glyph(c: char, serifs: bool, m: &FaceMetrics) -> Glyph {
        let f = face(FontFamily::Vga);
        let rows = f.glyph(c).unwrap().to_vec();
        let rows = if serifs {
            rows
        } else {
            strip_serifs(&rows, f.width, f.height)
        };
        analyse(vectorize(&rows, m))
    }

    #[test]
    fn classic_pairs_on_vga_without_serifs_and_with() {
        let f = face(FontFamily::Vga);
        let m =
            FaceMetrics::measure(|c| f.glyph(c).map(<[u8]>::to_vec), f.width, f.height).unwrap();
        let w = 8.0 * m.k;
        let max_gap = (w - 8.0 + 2.5).max(0.75 * w);
        let cases: [(&str, &[Mech]); 11] = [
            ("ff", &[Mech::Fuse]),
            ("fi", &[Mech::Hook]),
            ("fl", &[Mech::Hook]),
            ("ft", &[Mech::Fuse, Mech::Hook]),
            ("Th", &[Mech::BarStem, Mech::Fuse]),
            ("ct", &[Mech::Arch]),
            ("st", &[Mech::Arch]),
            ("sp", &[Mech::Arch]),
            ("ст", &[Mech::Arch]),
            ("тт", &[Mech::Fuse]),
            ("пт", &[Mech::Fuse, Mech::BarStem]),
        ];
        for serifs in [false, true] {
            for (p, want) in cases {
                let mut it = p.chars();
                let (a, b) = (it.next().unwrap(), it.next().unwrap());
                let got = mechs(&in_word(
                    &vga_glyph(a, serifs, &m),
                    &vga_glyph(b, serifs, &m),
                    w,
                    max_gap,
                ));
                assert!(
                    want.iter().any(|x| got.contains(x)),
                    "{p} serifs={serifs}: wanted one of {want:?}, got {got:?}"
                );
            }
        }
    }

    #[test]
    fn rerouting_stays_beyond_the_attachment_and_under_a_quarter() {
        let all: Vec<char> = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz\
            АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯабвгґдеєжзиіїйклмнопрстуфхцчшщьюя"
            .chars()
            .collect();
        let glyphs: Vec<Glyph> = all.iter().map(|c| mg(*c)).collect();
        let mut checked = 0;
        for g in &glyphs {
            let ink: f32 = g.prims.iter().map(prim_len).sum();
            for h in &glyphs {
                for c in in_word(g, h, 9.0, 4.5) {
                    let Some((side, qi, kept)) = c.trim else {
                        continue;
                    };
                    assert_eq!(side, Side::G);
                    let removed: f32 = c
                        .drop
                        .iter()
                        .map(|(_, d)| prim_len(&g.prims[*d as usize]))
                        .sum::<f32>()
                        + prim_len(&g.prims[qi as usize])
                        - prim_len(&kept);
                    assert!(removed <= 0.25 * ink + 1e-3, "removed {removed} of {ink}");
                    for (_, d) in &c.drop {
                        let dp = &g.prims[*d as usize];
                        for (_, e) in ends(dp) {
                            let n = g
                                .prims
                                .iter()
                                .enumerate()
                                .filter(|(i, q)| *i != *d as usize && dist_prim(q, e) < 0.05)
                                .count();
                            assert!(n <= 1, "dropped stroke {d} touches a junction");
                        }
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 100, "only {checked} reroutes checked");
    }
}
