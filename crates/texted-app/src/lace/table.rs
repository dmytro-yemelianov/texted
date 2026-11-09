//! Pair tables: for a pair of letters and a relation between their cells, the
//! ligatures that can join them, each with its stroke sprite.

use std::collections::BTreeMap;
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
    pub sprite: Arc<Sprite>,
    pub trim: Option<(Side, u16, Prim)>,
    pub drop: Vec<(Side, u16)>,
    pub absorb: Option<(Side, u16)>,
}

#[derive(Clone)]
pub struct Tables {
    metrics: FaceMetrics,
    serifs: bool,
    cell: [u16; 2],
    glyphs: BTreeMap<char, Option<Arc<Glyph>>>,
    entries: BTreeMap<(Relation, char, char), Vec<Arc<Entry>>>,
}

impl Tables {
    pub fn new(metrics: FaceMetrics, serifs: bool, cell: [u16; 2]) -> Self {
        Self {
            metrics,
            serifs,
            cell,
            glyphs: BTreeMap::new(),
            entries: BTreeMap::new(),
        }
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
        let out = best
            .into_values()
            .map(|c| {
                let path: Vec<V2> = c.path.iter().map(|p| [p[0], p[1] - drop_y]).collect();
                Arc::new(Entry {
                    mech: c.mech,
                    cost: c.cost,
                    keys: c.keys,
                    sprite: Arc::new(rasterize(&path, k)),
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

    #[test]
    fn vga_tables_reproduce_the_classic_pairs() {
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
        let mut t = tables();
        let mut ok = 0;
        for (p, want) in cases {
            let mut it = p.chars();
            let (a, b) = (it.next().unwrap(), it.next().unwrap());
            t.fill(Relation::H, a, b, &rows);
            let got: Vec<Mech> = t
                .get(Relation::H, a, b)
                .unwrap()
                .iter()
                .map(|e| e.mech)
                .collect();
            if want.iter().any(|m| got.contains(m)) {
                ok += 1;
            } else {
                eprintln!("{p}: wanted {want:?}, got {got:?}");
            }
        }
        assert_eq!(ok, 11);
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
        // Recompute the entry's path from the generator (entries do not keep it).
        let m = t.metrics;
        let glyph = |c| {
            let r = strip_serifs(&rows(c).unwrap(), m.width, m.height);
            analyse(vectorize(&r, &m))
        };
        let g = glyph('f');
        let cw = 8.0 * m.k;
        let cands = in_word(&g, &g, cw, (cw - cw + 2.5).max(0.75 * cw));
        let c = cands
            .iter()
            .find(|c| c.mech == Mech::Fuse && c.cost == e.cost)
            .expect("matching candidate");
        let base = f32::from(m.baseline);
        let px: Vec<[f32; 2]> = c
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
