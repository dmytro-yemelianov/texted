//! Connectors of one letter, in its own coordinates.
//!
//! A ray is a straight line end, or an arc end continued along its tangent,
//! that leaves the letter without running into another of its strokes. A host is
//! an arc or circle another line may touch tangentially. A latent ray is blocked
//! only by a dot and opens when a ligature absorbs that dot.

use super::geom::{arc_point, bbox, dist_prim, leave_dirs, Prim, V2};

/// What a ray continues from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RayKind {
    Line,
    Arc,
}

/// A connector: point `p`, unit direction `d`, leaving from stroke `prim`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub p: V2,
    pub d: V2,
    pub prim: u16,
    pub kind: RayKind,
}

/// A ray blocked only by the dot stroke `absorbs`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Latent {
    pub ray: Ray,
    pub absorbs: u16,
}

/// The strokes of a letter with its connectors.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub prims: Vec<Prim>,
    pub rays: Vec<Ray>,
    /// Indices of arcs and circles that lines may touch.
    pub hosts: Vec<u16>,
    pub latent: Vec<Latent>,
    pub bbox: [f32; 4],
}

fn norm(a: V2) -> V2 {
    let l = a[0].hypot(a[1]);
    let l = if l == 0.0 { 1.0 } else { l };
    [a[0] / l, a[1] / l]
}

fn ang_deg(u: V2, v: V2) -> f32 {
    let (u, v) = (norm(u), norm(v));
    (u[0] * v[0] + u[1] * v[1])
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

enum Outcome {
    Ray,
    Latent(u16),
    Blocked,
}

fn try_ray(prims: &[Prim], bb: [f32; 4], p: V2, d: V2, i: usize, kind: RayKind) -> Outcome {
    let leaving = prims
        .iter()
        .enumerate()
        .filter(|(j, _)| *j != i)
        .flat_map(|(_, q)| leave_dirs(q, p));
    let min_ang = leaving.map(|v| ang_deg(d, v)).fold(180.0, f32::min);
    // A line may continue through a joint unless it runs into another stroke;
    // an arc end continues only when it leaves cleanly across the joint.
    let limit = if kind == RayKind::Arc { 60.0 } else { 25.0 };
    if min_ang < limit {
        return Outcome::Blocked;
    }
    let touching: Vec<bool> = prims.iter().map(|q| dist_prim(q, p) < 0.05).collect();
    let mut k = 0;
    loop {
        let t = 0.4 + 0.2 * k as f32;
        if t >= 40.0 {
            return Outcome::Blocked;
        }
        k += 1;
        let x = [p[0] + d[0] * t, p[1] + d[1] * t];
        if x[0] < bb[0] - 0.3 || x[0] > bb[2] + 0.3 || x[1] < bb[1] - 0.3 || x[1] > bb[3] + 0.3 {
            return Outcome::Ray;
        }
        for (j, q) in prims.iter().enumerate() {
            if j == i || (touching[j] && t < 0.6) {
                continue;
            }
            if dist_prim(q, x) < 0.3 {
                // Blocked only by a dot: the connector is latent.
                return match q {
                    Prim::Arc { full: true, r, .. } if *r < 1.0 => Outcome::Latent(j as u16),
                    _ => Outcome::Blocked,
                };
            }
        }
    }
}

/// Finds the connectors of a letter's strokes.
pub fn analyse(prims: Vec<Prim>) -> Glyph {
    let bb = bbox(&prims);
    let (mut rays, mut hosts, mut latent) = (Vec::new(), Vec::new(), Vec::new());
    let mut attempt = |p: V2, d: V2, i: usize, kind: RayKind| {
        let ray = Ray {
            p,
            d,
            prim: i as u16,
            kind,
        };
        match try_ray(&prims, bb, p, d, i, kind) {
            Outcome::Ray => rays.push(ray),
            Outcome::Latent(absorbs) => latent.push(Latent { ray, absorbs }),
            Outcome::Blocked => {}
        }
    };
    for (i, q) in prims.iter().enumerate() {
        match *q {
            Prim::Line { a, b } => {
                attempt(a, norm([a[0] - b[0], a[1] - b[1]]), i, RayKind::Line);
                attempt(b, norm([b[0] - a[0], b[1] - a[1]]), i, RayKind::Line);
            }
            Prim::Arc { c, r, a0, a1, full } => {
                hosts.push(i as u16);
                if !full {
                    let (s0, c0) = a0.to_radians().sin_cos();
                    let (s1, c1) = a1.to_radians().sin_cos();
                    attempt(arc_point(c, r, a0), [s0, -c0], i, RayKind::Arc);
                    attempt(arc_point(c, r, a1), [-s1, c1], i, RayKind::Arc);
                }
            }
        }
    }
    Glyph {
        prims,
        rays,
        hosts,
        latent,
        bbox: bb,
    }
}

#[cfg(test)]
mod tests {
    use super::super::model_font::model;
    use super::*;

    const TOL: f32 = 0.02;

    fn glyph(ch: char) -> Glyph {
        analyse(model(ch).expect("in the model alphabet"))
    }

    fn has(rays: &[Ray], p: V2, d: V2) -> bool {
        rays.iter().any(|r| {
            (r.p[0] - p[0]).abs() < TOL
                && (r.p[1] - p[1]).abs() < TOL
                && (r.d[0] - d[0]).abs() < TOL
                && (r.d[1] - d[1]).abs() < TOL
        })
    }

    #[test]
    fn capital_a_has_the_six_connectors_the_owner_described() {
        let g = glyph('А');
        assert_eq!(g.rays.len(), 6, "{:?}", g.rays);
        assert!(has(&g.rays, [4.0, 12.0], [-0.32, 0.95]));
        assert!(has(&g.rays, [4.0, 12.0], [0.32, 0.95]));
        assert!(has(&g.rays, [1.33, 4.0], [-1.0, 0.0]));
        assert!(has(&g.rays, [6.67, 4.0], [1.0, 0.0]));
        assert!(has(&g.rays, [0.0, 0.0], [-0.32, -0.95]));
        assert!(has(&g.rays, [8.0, 0.0], [0.32, -0.95]));
    }

    #[test]
    fn small_a_has_stem_ends_bowl_ends_and_two_arcs() {
        let g = glyph('a');
        assert!(has(&g.rays, [8.0, 0.0], [0.0, -1.0]));
        assert!(has(&g.rays, [8.0, 6.95], [0.0, 1.0]));
        assert!(has(&g.rays, [8.0, 4.8], [1.0, 0.0]));
        assert!(has(&g.rays, [8.0, 0.0], [1.0, 0.0]));
        assert_eq!(g.hosts.len(), 2);
    }

    #[test]
    fn a_circle_has_no_rays_and_one_host() {
        let g = glyph('о');
        assert!(g.rays.is_empty());
        assert_eq!(g.hosts.len(), 1);
    }

    #[test]
    fn zhe_centre_does_not_extend_through_itself() {
        let g = glyph('ж');
        assert_eq!(g.rays.len(), 6, "{:?}", g.rays);
        assert!(g
            .rays
            .iter()
            .all(|r| (r.p[0] - 4.0).abs() > TOL || (r.p[1] - 4.0).abs() > TOL));
    }

    #[test]
    fn a_dot_hides_a_latent_stem_top() {
        let g = glyph('і');
        assert_eq!(g.rays.len(), 1, "{:?}", g.rays);
        assert!(has(&g.rays, [4.0, 0.0], [0.0, -1.0]));
        assert_eq!(g.latent.len(), 1);
        assert!(has(&[g.latent[0].ray], [4.0, 8.0], [0.0, 1.0]));
        assert!(matches!(
            g.prims[g.latent[0].absorbs as usize],
            Prim::Arc { full: true, .. }
        ));
    }
}
