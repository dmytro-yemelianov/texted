//! Pair tables: for a pair of letters and a relation between their cells, the
//! ligatures that can join them, each with its stroke sprite.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::connectors::{analyse, Glyph};
use super::geom::{Prim, V2};
use super::ligatures::{across_space, between_rows, in_word, Candidate, Key, Mech, Side};
use super::sprite::{rasterize, Sprite};
use super::vectorize::{strip_serifs, vectorize, FaceMetrics};

/// Where the second letter sits relative to the first. `S(n)` crosses `n`
/// empty cells (1 to 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Relation {
    H,
    S(u8),
    V,
    Dr,
    Dl,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub mech: Mech,
    pub cost: f32,
    pub keys: Vec<Key>,
    /// The connecting polyline in model units, in the first letter's frame: x from its
    /// cell's left edge, y up from its baseline.
    pub path: Vec<V2>,
    /// What the grid's clearance check reads of `path`.
    pub probe: Probe,
    pub sprite: Arc<Sprite>,
    pub trim: Option<(Side, u16, Prim)>,
    pub drop: Vec<(Side, u16)>,
    pub absorb: Option<(Side, u16)>,
}

/// A path must stay this far (model units) from the ink of the other cells around
/// its pair...
pub const CLEARANCE: f32 = 0.4;
/// ...except within this distance of its two ends, where it joins its letters.
pub const END_ZONE: f32 = 0.7;
/// How far (model units) a glyph's strokes may lie outside its pixel rows at `k`
/// units per pixel. Traced strokes, their fillets and restored bars run through
/// pixel centres or inside the ink; only a dot (radius 0.7 about the middle of its
/// pixels, at least half a pixel inside) can reach past it. 0.05 covers rounding.
fn stray(k: f32) -> f32 {
    (0.7 - 0.5 * k).max(0.0) + 0.05
}

/// The part of a path the clearance check tests, worked out when the entry is built.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    /// Path points farther than [`END_ZONE`] from both ends (none for a straight join).
    pub points: Vec<V2>,
    /// Their bounding box `[x0, y0, x1, y1]`.
    pub bbox: [f32; 4],
    /// Whether a point comes near enough to the row above the upper cell (`[0]`) or
    /// the row below the lower cell (`[1]`) for any glyph there to block it; when not,
    /// the choice of this entry never depends on that row.
    pub reach: [bool; 2],
}

/// Work a selection needs from the worker before it can be final.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Need {
    /// The entries for `a` then `b` in a relation.
    Pair(Relation, char, char),
    /// The strokes of a character beside a ligature, for the clearance check.
    Glyph(char),
}

/// Numbers each new [`Tables`].
static LINEAGE: AtomicU64 = AtomicU64::new(0);

pub struct Tables {
    metrics: FaceMetrics,
    serifs: bool,
    cell: [u16; 2],
    glyphs: BTreeMap<char, Option<Arc<Glyph>>>,
    entries: BTreeMap<(Relation, char, char), Vec<Arc<Entry>>>,
    /// Shared by these tables and their snapshots; new tables get a new one.
    lineage: u64,
    /// How many glyphs and entry lists went in so far.
    filled: u64,
}

impl Tables {
    pub fn new(metrics: FaceMetrics, serifs: bool, cell: [u16; 2]) -> Self {
        Self {
            metrics,
            serifs,
            cell,
            glyphs: BTreeMap::new(),
            entries: BTreeMap::new(),
            lineage: LINEAGE.fetch_add(1, Ordering::Relaxed) + 1,
            filled: 0,
        }
    }

    /// A copy to publish: the same entries and glyphs, never filled further.
    pub fn snapshot(&self) -> Arc<Self> {
        Arc::new(Self {
            metrics: self.metrics,
            serifs: self.serifs,
            cell: self.cell,
            glyphs: self.glyphs.clone(),
            entries: self.entries.clone(),
            lineage: self.lineage,
            filled: self.filled,
        })
    }

    /// `(lineage, filled)`. Tables only grow and only the original is filled, so of
    /// two stamps with the same lineage the larger holds everything the smaller did,
    /// entry for entry.
    pub fn stamp(&self) -> (u64, u64) {
        (self.lineage, self.filled)
    }

    /// Fill what `need` asks for (worker side).
    pub fn supply(&mut self, need: Need, rows: &impl Fn(char) -> Option<Vec<u8>>) {
        match need {
            Need::Pair(rel, a, b) => self.fill(rel, a, b, rows),
            Need::Glyph(c) => {
                self.glyph(c, rows);
            }
        }
    }

    /// The strokes of `c` if they were worked out: `Some(None)` when the face lacks it.
    pub fn strokes(&self, c: char) -> Option<Option<&Glyph>> {
        self.glyphs.get(&c).map(Option::as_deref)
    }

    /// Where the cell `rows` below and `cols` right of a letter puts its frame, in
    /// that letter's frame (y up).
    pub fn offset(&self, rows: isize, cols: isize) -> V2 {
        let k = self.metrics.k;
        [
            cols as f32 * f32::from(self.cell[0]) * k,
            -(rows as f32) * f32::from(self.cell[1]) * k,
        ]
    }

    /// The glyph of `c`, vectorised on first use; `None` when the face lacks it.
    fn glyph(&mut self, c: char, rows: &impl Fn(char) -> Option<Vec<u8>>) -> Option<Arc<Glyph>> {
        if let Some(g) = self.glyphs.get(&c) {
            return g.clone();
        }
        let g = rows(c).map(|r| {
            let (w, h) = (self.metrics.width, self.metrics.height);
            let r = if self.serifs {
                r
            } else {
                strip_serifs(&r, w, h)
            };
            Arc::new(analyse(vectorize(&r, &self.metrics)))
        });
        self.glyphs.insert(c, g.clone());
        self.filled += 1;
        g
    }

    /// Compute the entries for `a` then `b` in relation `rel` (worker side).
    /// Keeps the cheapest candidate per mechanism and sorted key set.
    pub fn fill(
        &mut self,
        rel: Relation,
        a: char,
        b: char,
        rows: &impl Fn(char) -> Option<Vec<u8>>,
    ) {
        if self.entries.contains_key(&(rel, a, b)) {
            return;
        }
        self.filled += 1;
        let (Some(g), Some(h)) = (self.glyph(a, rows), self.glyph(b, rows)) else {
            self.entries.insert((rel, a, b), Vec::new());
            return;
        };
        let k = self.metrics.k;
        let cell_w = f32::from(self.cell[0]) * k;
        let row = f32::from(self.cell[1]) * k;
        let glyph_w = f32::from(self.metrics.width) * k;
        let list = match rel {
            Relation::H => in_word(&g, &h, cell_w, (cell_w - glyph_w + 2.5).max(0.75 * cell_w)),
            Relation::S(n) => {
                let span = (f32::from(n) + 1.0) * cell_w;
                across_space(&g, &h, span, span - glyph_w)
            }
            Relation::V => between_rows(&g, &h, 0.0, -row, row),
            Relation::Dr => between_rows(&g, &h, cell_w, -row, row),
            Relation::Dl => between_rows(&g, &h, -cell_w, -row, row),
        };
        let mut best: BTreeMap<(u8, Vec<Key>), Candidate> = BTreeMap::new();
        for c in list {
            let mut keys = c.keys.clone();
            keys.sort();
            let slot = (c.mech as u8, keys);
            if best.get(&slot).is_none_or(|o| c.cost < o.cost) {
                best.insert(slot, c);
            }
        }
        let drop_y = f32::from(self.metrics.baseline) * k;
        // The lower cell's row: the first letter's for in-row relations.
        let lower = match rel {
            Relation::H | Relation::S(_) => 0.0,
            Relation::V | Relation::Dr | Relation::Dl => -row,
        };
        let out = best
            .into_values()
            .map(|c| {
                let from_top: Vec<V2> = c.path.iter().map(|p| [p[0], p[1] - drop_y]).collect();
                let probe = self.probe(&c.path, lower);
                Arc::new(Entry {
                    mech: c.mech,
                    cost: c.cost,
                    keys: c.keys,
                    path: c.path,
                    probe,
                    sprite: Arc::new(rasterize(&from_top, k)),
                    trim: c.trim,
                    drop: c.drop,
                    absorb: c.absorb,
                })
            })
            .collect();
        self.entries.insert((rel, a, b), out);
    }

    pub fn get(&self, rel: Relation, a: char, b: char) -> Option<&[Arc<Entry>]> {
        self.entries.get(&(rel, a, b)).map(Vec::as_slice)
    }

    /// The clearance probe of `path`, whose lower cell's frame sits `lower` below the
    /// first letter's. A glyph's strokes lie within its pixel rows, give or take
    /// [`stray`], so a neighbour row's ink is at least that far into its own row.
    fn probe(&self, path: &[V2], lower: f32) -> Probe {
        let (Some(&s), Some(&e)) = (path.first(), path.last()) else {
            return Probe::default();
        };
        let far = |p: &V2, q: V2| (p[0] - q[0]).hypot(p[1] - q[1]) >= END_ZONE;
        let points: Vec<V2> = path
            .iter()
            .filter(|p| far(p, s) && far(p, e))
            .copied()
            .collect();
        if points.is_empty() {
            return Probe::default();
        }
        let mut bbox = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for p in &points {
            bbox = [
                bbox[0].min(p[0]),
                bbox[1].min(p[1]),
                bbox[2].max(p[0]),
                bbox[3].max(p[1]),
            ];
        }
        let k = self.metrics.k;
        let row = f32::from(self.cell[1]) * k;
        let (top, bottom) = (
            f32::from(self.metrics.baseline) * k + stray(k),
            (f32::from(self.metrics.baseline) - f32::from(self.metrics.height)) * k - stray(k),
        );
        Probe {
            reach: [
                bbox[3] + CLEARANCE > bottom + row,
                bbox[1] - CLEARANCE < top + lower - row,
            ],
            points,
            bbox,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::FontFamily;
    use crate::bitmap_fonts::face;

    fn rows(c: char) -> Option<Vec<u8>> {
        face(FontFamily::Vga).glyph(c).map(<[u8]>::to_vec)
    }

    fn tables() -> Tables {
        let f = face(FontFamily::Vga);
        let m = FaceMetrics::measure(rows, f.width, f.height).unwrap();
        Tables::new(m, false, [8, 16])
    }

    /// The clearance probe's `reach` relies on every stroke lying within its glyph's
    /// pixel rows, give or take `stray`; check it on every bundled face, letters and
    /// the other characters a document puts beside them.
    #[test]
    fn strokes_stay_within_their_rows() {
        let chars: Vec<char> = ('!'..='~')
            .chain('А'..='я')
            .chain("ҐґЄєІіЇї«»—–…№₴".chars())
            .collect();
        for family in FontFamily::ALL {
            let f = face(family);
            let rows = move |c: char| f.glyph(c).map(<[u8]>::to_vec);
            let m = FaceMetrics::measure(rows, f.width, f.height).unwrap();
            let (top, bottom) = (
                f32::from(m.baseline) * m.k + stray(m.k),
                (f32::from(m.baseline) - f32::from(m.height)) * m.k - stray(m.k),
            );
            // Serifs kept: every ink pixel is vectorised.
            let mut t = Tables::new(m, true, [f.width, f.height]);
            for &c in &chars {
                t.supply(Need::Glyph(c), &rows);
                let Some(Some(g)) = t.strokes(c) else {
                    continue;
                };
                for p in &g.prims {
                    let b = crate::lace::geom::bbox(std::slice::from_ref(p));
                    assert!(
                        b[1] >= bottom && b[3] <= top,
                        "{} {c:?}: {b:?} outside {bottom}..{top}",
                        family.name()
                    );
                }
            }
        }
    }

    #[test]
    fn probes_skip_the_ends_and_know_which_rows_they_reach() {
        let t = tables();
        let k = t.metrics.k;
        // A straight join has nothing to probe.
        let straight = t.probe(&[[0.0, 0.0], [10.0, 0.0]], 0.0);
        assert_eq!(straight, Probe::default());
        // A path along the x-height: only its middle is probed, and it reaches no row.
        let path: Vec<V2> = (0..=10).map(|i| [i as f32, 8.0]).collect();
        let mid = t.probe(&path, 0.0);
        assert_eq!(mid.points.first(), Some(&[1.0, 8.0]));
        assert_eq!(mid.points.last(), Some(&[9.0, 8.0]));
        assert_eq!(mid.reach, [false, false]);
        // At the cell's top edge it reaches the row above; at its bottom edge, the row
        // below; a lower cell one row down moves that row down too.
        let at = |y: f32| t.probe(&[[0.0, y], [2.0, y], [4.0, y]], 0.0).reach;
        assert_eq!(at(12.0 * k), [true, false]);
        assert_eq!(at(-4.0 * k), [false, true]);
        let down = |y: f32| t.probe(&[[0.0, y], [2.0, y], [4.0, y]], -16.0 * k).reach;
        assert_eq!(down(-4.0 * k), [false, false]);
        assert_eq!(down(-20.0 * k), [false, true]);
    }

    #[test]
    fn one_entry_per_connector_set() {
        let mut t = tables();
        assert!(t.get(Relation::H, 'f', 'i').is_none());
        t.fill(Relation::H, 'f', 'i', &rows);
        let e = t.get(Relation::H, 'f', 'i').unwrap();
        // Hook-with-absorb exists; the cheapest reroute depth wins the cheapest slot.
        let hooks: Vec<&Arc<Entry>> = e
            .iter()
            .filter(|e| e.mech == Mech::Hook && e.absorb.is_some())
            .collect();
        assert!(!hooks.is_empty());
        // No two entries share a mechanism and sorted key set.
        let mut seen = std::collections::BTreeSet::new();
        for e in e {
            let mut k = e.keys.clone();
            k.sort();
            assert!(seen.insert((e.mech.order(), format!("{:?}{k:?}", e.mech))));
        }
        // Filling twice changes nothing.
        let before = e.to_vec();
        t.fill(Relation::H, 'f', 'i', &rows);
        assert_eq!(t.get(Relation::H, 'f', 'i').unwrap(), before.as_slice());
    }

    /// The spec's classic pairs and the mechanisms that count as reproducing each.
    const CLASSIC: [(&str, &[Mech]); 11] = [
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

    /// Classic pairs whose `H` table, serifs ignored, holds a reproducing mechanism.
    fn classic_pairs(family: FontFamily) -> Vec<&'static str> {
        let f = face(family);
        let rows = move |c: char| f.glyph(c).map(<[u8]>::to_vec);
        let m = FaceMetrics::measure(rows, f.width, f.height).unwrap();
        let mut t = Tables::new(m, false, [f.width, f.height]);
        CLASSIC
            .iter()
            .filter(|(p, want)| {
                let mut it = p.chars();
                let (a, b) = (it.next().unwrap(), it.next().unwrap());
                t.fill(Relation::H, a, b, &rows);
                let got = t.get(Relation::H, a, b).unwrap();
                got.iter().any(|e| want.contains(&e.mech))
            })
            .map(|(p, _)| *p)
            .collect()
    }

    /// Baseline, not a target: VGA meets the spec's 11/11 and the other bundled faces
    /// are pinned where they stand, so a regression fails and an improvement passes.
    #[test]
    fn bundled_faces_keep_their_classic_pair_baseline() {
        for (family, floor) in [
            (FontFamily::Vga, 11),
            (FontFamily::Bios, 10),
            (FontFamily::Ega, 10),
            (FontFamily::Spleen, 8),
            (FontFamily::SpleenSmall, 5),
        ] {
            let got = classic_pairs(family);
            assert!(
                got.len() >= floor,
                "{}: {} of 11 ({got:?}), baseline {floor}",
                family.name(),
                got.len()
            );
        }
    }

    #[test]
    fn vga_tables_reproduce_the_classic_pairs() {
        assert_eq!(
            classic_pairs(FontFamily::Vga),
            CLASSIC.map(|(p, _)| p),
            "VGA must reproduce all eleven"
        );
    }

    #[test]
    fn sprite_lies_on_the_path() {
        let mut t = tables();
        t.fill(Relation::H, 'f', 'f', &rows);
        let e = t
            .get(Relation::H, 'f', 'f')
            .unwrap()
            .iter()
            .find(|e| e.mech == Mech::Fuse)
            .expect("ff fuse");
        let s = &e.sprite;
        // The entry's path in font pixels, y down from the cell's top.
        let m = t.metrics;
        let base = f32::from(m.baseline);
        let px: Vec<[f32; 2]> = e
            .path
            .iter()
            .map(|p| [p[0] / m.k, base - p[1] / m.k])
            .collect();
        let (first, last) = (px[0], px[px.len() - 1]);
        let (x_lo, x_hi) = (
            first[0].min(last[0]).floor() as i32,
            first[0].max(last[0]).floor() as i32,
        );
        // Pixels on the crossbar row, and all of them between the bar ends.
        let on_bar: Vec<i32> = (s.origin[0]..s.origin[0] + s.width as i32)
            .filter(|&x| s.lit(x, 6))
            .collect();
        assert!(!on_bar.is_empty(), "no pixel on row 6: {s:?}");
        assert!(
            on_bar.iter().all(|&x| x >= x_lo && x <= x_hi),
            "row 6 pixels {on_bar:?} outside bar ends {x_lo}..={x_hi}"
        );
        assert!(
            x_lo < 8 && x_hi >= 8,
            "bar ends {x_lo}..{x_hi} do not span the cells"
        );
        // No lit pixel outside the path's bbox expanded by one pixel.
        let min = |i: usize| {
            px.iter()
                .map(|p| p[i])
                .fold(f32::INFINITY, f32::min)
                .floor() as i32
                - 1
        };
        let max = |i: usize| {
            px.iter()
                .map(|p| p[i])
                .fold(f32::NEG_INFINITY, f32::max)
                .floor() as i32
                + 1
        };
        let mut lit = 0;
        for r in 0..s.height as i32 {
            for col in 0..s.width as i32 {
                if s.coverage[(r as u32 * s.width + col as u32) as usize] == 0 {
                    continue;
                }
                lit += 1;
                let (x, y) = (s.origin[0] + col, s.origin[1] + r);
                assert!(
                    x >= min(0) && x <= max(0) && y >= min(1) && y <= max(1),
                    "lit ({x},{y}) outside bbox"
                );
            }
        }
        assert!(lit > 0);
    }

    #[test]
    fn spacing_relation_reaches_across_n_cells() {
        let mut t = tables();
        for n in [1u8, 3] {
            t.fill(Relation::S(n), 'f', 'f', &rows);
            assert!(t.get(Relation::S(n), 'f', 'f').is_some());
        }
        // Different gaps are different tables.
        assert!(t.get(Relation::S(2), 'f', 'f').is_none());
        let far = t.get(Relation::S(3), 'f', 'f').unwrap();
        assert!(!far.is_empty());
        for e in far {
            // 4 cells − glyph width: the strokes span more than a cell.
            assert!(e.sprite.width > 8, "{:?}", e.sprite);
        }
    }

    #[test]
    fn tables_are_deterministic() {
        let (mut a, mut b) = (tables(), tables());
        for (rel, x, y) in [
            (Relation::H, 'f', 't'),
            (Relation::V, 'l', 'i'),
            (Relation::Dr, 'T', 'h'),
            (Relation::Dl, 'f', 'j'),
        ] {
            a.fill(rel, x, y, &rows);
            b.fill(rel, x, y, &rows);
            assert!(!a.get(rel, x, y).unwrap().is_empty() || rel != Relation::H);
            assert_eq!(a.get(rel, x, y), b.get(rel, x, y));
        }
    }
}
