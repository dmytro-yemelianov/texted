//! Selection of ligatures over the visible grid.
//!
//! Only the visible cells are looked at, tables are never computed here (pairs
//! that are not filled yet are reported in [`Selection::missing`]), and every
//! random choice is keyed by document position, so scrolling does not change
//! which connections are drawn.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use super::ligatures::{Key, Side};
use super::table::{Entry, Relation, Tables};

/// One visible row, top to bottom; matches `VisibleLine`.
pub struct Row<'a> {
    pub line: usize,
    pub start_col: usize,
    pub text: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub density: f32,
    pub spacing: u8,
    pub seed: u32,
}

/// A chosen ligature. `row` indexes `rows`; `col` is the left or upper cell's
/// column on screen (its document column is `start_col + col`).
#[derive(Clone, Debug)]
pub struct Placed {
    pub row: usize,
    pub col: usize,
    pub rel: Relation,
    pub entry: Arc<Entry>,
}

#[derive(Debug, Default)]
pub struct Selection {
    pub placed: Vec<Placed>,
    /// Pairs the worker still has to fill, without duplicates.
    pub missing: Vec<(Relation, char, char)>,
}

type Cell = (usize, usize);

/// A resource a candidate uses up, with cells named by (row, screen column).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Res {
    Key(u8, Cell, u16, u8),
    Pair(Cell, Cell),
    Slot(Cell, u8),
    Band(usize, usize),
}

fn key_res(k: Key, g: Cell, h: Cell) -> Res {
    let at = |s: Side| if s == Side::G { g } else { h };
    match k {
        Key::Ray(s, i) => Res::Key(0, at(s), i, 0),
        Key::Latent(s, i) => Res::Key(1, at(s), i, 0),
        Key::Dot(s, i) => Res::Key(2, at(s), i, 0),
        Key::Terminal(s, i, e) => Res::Key(3, at(s), i, e),
        Key::Drop(s, i) => Res::Key(4, at(s), i, 0),
        Key::Rerouted(s, i) => Res::Key(5, at(s), i, 0),
    }
}

struct Cand {
    g: Cell,
    h: Cell,
    rel: Relation,
    entry: Arc<Entry>,
    hash: f64,
}

fn rel_code(rel: Relation) -> u64 {
    match rel {
        Relation::H => 0,
        Relation::S(n) => u64::from(n),
        Relation::V => 8,
        Relation::Dr => 9,
        Relation::Dl => 10,
    }
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// splitmix64 of (line, document column, relation, entry index, seed) in [0, 1).
fn unit_hash(line: usize, doc_col: usize, rel: Relation, entry: usize, seed: u32) -> f64 {
    let mut h = 0u64;
    for v in [
        line as u64,
        doc_col as u64,
        rel_code(rel),
        entry as u64,
        u64::from(seed),
    ] {
        h = splitmix64(h ^ v);
    }
    (h >> 11) as f64 / (1u64 << 53) as f64
}

struct Grid<'a> {
    rows: &'a [Row<'a>],
    chars: Vec<Vec<char>>,
    tables: &'a Tables,
    seed: u32,
    cands: Vec<Cand>,
    missing: BTreeSet<(Relation, char, char)>,
}

impl Grid<'_> {
    fn letter(&self, (r, c): Cell) -> Option<char> {
        self.chars
            .get(r)
            .and_then(|row| row.get(c))
            .copied()
            .filter(|c| c.is_alphabetic())
    }

    fn add(&mut self, rel: Relation, g: Cell, h: Cell) {
        let (Some(a), Some(b)) = (self.letter(g), self.letter(h)) else {
            return;
        };
        let Some(list) = self.tables.get(rel, a, b) else {
            self.missing.insert((rel, a, b));
            return;
        };
        let row = &self.rows[g.0];
        for (i, e) in list.iter().enumerate() {
            self.cands.push(Cand {
                g,
                h,
                rel,
                entry: e.clone(),
                hash: unit_hash(row.line, row.start_col + g.1, rel, i, self.seed),
            });
        }
    }

    /// Accept candidates in priority order against `used`.
    fn accept(&mut self, used: &mut HashSet<Res>, density: f64, out: &mut Vec<Placed>) {
        self.cands.sort_by(|a, b| {
            a.entry
                .mech
                .order()
                .cmp(&b.entry.mech.order())
                .then(a.entry.cost.total_cmp(&b.entry.cost))
                .then(a.hash.total_cmp(&b.hash))
        });
        for c in self.cands.drain(..) {
            if c.rel != Relation::H && c.hash >= density {
                continue;
            }
            let mut keys: Vec<Res> = c.entry.keys.iter().map(|&k| key_res(k, c.g, c.h)).collect();
            keys.push(Res::Pair(c.g, c.h));
            match c.rel {
                Relation::H | Relation::S(_) => {
                    keys.push(Res::Slot(c.g, b'F'));
                    keys.push(Res::Slot(c.h, b'B'));
                }
                _ => {
                    keys.push(Res::Slot(c.g, b'D'));
                    keys.push(Res::Slot(c.h, b'U'));
                    let doc = self.rows[c.g.0].start_col + c.g.1;
                    keys.push(Res::Band(c.g.0, doc / 2));
                }
            }
            if keys.iter().any(|k| used.contains(k)) {
                continue;
            }
            used.extend(keys);
            out.push(Placed {
                row: c.g.0,
                col: c.g.1,
                rel: c.rel,
                entry: c.entry,
            });
        }
    }
}

/// Choose the ligatures for the visible rows.
pub fn select(rows: &[Row], tables: &Tables, s: &Settings) -> Selection {
    run(rows, tables, s).0
}

/// `select`, also returning the number of cells that were examined.
fn run(rows: &[Row], tables: &Tables, s: &Settings) -> (Selection, usize) {
    let mut grid = Grid {
        rows,
        chars: rows.iter().map(|r| r.text.chars().collect()).collect(),
        tables,
        seed: s.seed,
        cands: Vec::new(),
        missing: BTreeSet::new(),
    };
    let density = f64::from(s.density);
    let mut used = HashSet::new();
    let mut placed = Vec::new();
    let mut cells = 0;

    for r in 0..rows.len() {
        let len = grid.chars[r].len();
        cells += len;
        for c in 0..len {
            if grid.letter((r, c)).is_none() {
                continue;
            }
            grid.add(Relation::H, (r, c), (r, c + 1));
            for n in 1..=s.spacing {
                let last = c + usize::from(n);
                if grid.chars[r].get(last) != Some(&' ') {
                    break;
                }
                grid.add(Relation::S(n), (r, c), (r, last + 1));
            }
        }
        grid.accept(&mut used, density, &mut placed);
    }

    for r in 0..rows.len().saturating_sub(1) {
        for c in 0..grid.chars[r].len() {
            if grid.letter((r, c)).is_none() {
                continue;
            }
            grid.add(Relation::V, (r, c), (r + 1, c));
            grid.add(Relation::Dr, (r, c), (r + 1, c + 1));
            if c > 0 {
                grid.add(Relation::Dl, (r, c), (r + 1, c - 1));
            }
        }
        grid.accept(&mut used, density, &mut placed);
    }

    let missing = grid.missing.into_iter().collect();
    (Selection { placed, missing }, cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::FontFamily;
    use crate::bitmap_fonts::face;
    use crate::lace::vectorize::FaceMetrics;
    use std::collections::BTreeMap;

    fn rows_of(c: char) -> Option<Vec<u8>> {
        face(FontFamily::Vga).glyph(c).map(<[u8]>::to_vec)
    }

    fn tables() -> Tables {
        let f = face(FontFamily::Vga);
        let m = FaceMetrics::measure(rows_of, f.width, f.height).unwrap();
        Tables::new(m, false, [8, 16])
    }

    fn mulberry32(seed: u32) -> impl FnMut() -> f64 {
        let mut seed = seed;
        move || {
            seed = seed.wrapping_add(0x6d2b_79f5);
            let mut t = (seed ^ (seed >> 15)).wrapping_mul(1 | seed);
            t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
            f64::from(t ^ (t >> 14)) / 4_294_967_296.0
        }
    }

    /// Port of `fillGrid` in the prototype's grid.js.
    fn fill_grid(words: &[&str], cols: usize, nrows: usize, seed: u32) -> Vec<String> {
        let mut rng = mulberry32(seed);
        let mut rows = Vec::new();
        let mut k = (rng() * words.len() as f64) as usize;
        let mut line = String::new();
        while rows.len() < nrows {
            let w = words[k % words.len()];
            k += 1;
            let have = line.chars().count();
            if (if have > 0 { have + 1 } else { 0 }) + w.chars().count() > cols {
                rows.push(std::mem::replace(&mut line, w.to_string()));
            } else if line.is_empty() {
                line = w.to_string();
            } else {
                line = format!("{line} {w}");
            }
        }
        rows
    }

    /// Select, fill every missing pair, and repeat until nothing is missing.
    fn settle(t: &mut Tables, rows: &[Row], s: &Settings) -> (Selection, usize) {
        for _ in 0..10 {
            let (sel, n) = run(rows, t, s);
            if sel.missing.is_empty() {
                return (sel, n);
            }
            for (rel, a, b) in sel.missing {
                t.fill(rel, a, b, &rows_of);
            }
        }
        panic!("selection never settled");
    }

    fn rows_from(text: &[String], first: usize, end: usize, start_col: usize) -> Vec<Row<'_>> {
        (first..end)
            .map(|i| Row {
                line: i,
                start_col,
                text: &text[i],
            })
            .collect()
    }

    const WORDS: [&str; 14] = [
        "листопад",
        "щастя",
        "ступінь",
        "пташка",
        "вітер",
        "плетиво",
        "мереживо",
        "office",
        "fifth",
        "staff",
        "lattice",
        "thrifty",
        "after",
        "first",
    ];

    fn settings(density: f32, spacing: u8) -> Settings {
        Settings {
            density,
            spacing,
            seed: 3,
        }
    }

    fn h_cell(p: &Placed) -> Cell {
        match p.rel {
            Relation::H => (p.row, p.col + 1),
            Relation::S(n) => (p.row, p.col + 1 + usize::from(n)),
            Relation::V => (p.row + 1, p.col),
            Relation::Dr => (p.row + 1, p.col + 1),
            Relation::Dl => (p.row + 1, p.col - 1),
        }
    }

    #[test]
    fn rules_hold_on_a_filled_grid() {
        let text = fill_grid(&WORDS, 48, 14, 3);
        let rows = rows_from(&text, 0, 14, 0);
        let mut t = tables();
        let (sel, _) = settle(&mut t, &rows, &settings(1.0, 1));
        assert!(sel.placed.len() > 20, "only {} placed", sel.placed.len());
        let mut pairs = HashSet::new();
        let mut slots = HashSet::new();
        let mut bands = HashSet::new();
        let mut per_letter: BTreeMap<Cell, usize> = BTreeMap::new();
        for p in &sel.placed {
            let (g, h) = ((p.row, p.col), h_cell(p));
            assert!(pairs.insert((g, h)), "pair used twice: {g:?} {h:?}");
            let (a, b) = match p.rel {
                Relation::H | Relation::S(_) => (b'F', b'B'),
                _ => (b'D', b'U'),
            };
            assert!(slots.insert((g, a)), "side used twice at {g:?}");
            assert!(slots.insert((h, b)), "side used twice at {h:?}");
            if !matches!(p.rel, Relation::H | Relation::S(_)) {
                assert!(bands.insert((p.row, p.col / 2)), "band used twice");
            }
            *per_letter.entry(g).or_default() += 1;
            *per_letter.entry(h).or_default() += 1;
        }
        assert!(per_letter.values().all(|&n| n <= 4), "{per_letter:?}");
    }

    fn key_of(p: &Placed, rows: &[Row]) -> (usize, usize, Relation, Vec<Key>) {
        (
            rows[p.row].line,
            rows[p.row].start_col + p.col,
            p.rel,
            p.entry.keys.clone(),
        )
    }

    #[test]
    fn same_text_same_lace_when_scrolled() {
        let text = fill_grid(&WORDS, 48, 17, 3);
        let mut t = tables();
        let s = settings(0.6, 1);
        let top = rows_from(&text, 0, 14, 0);
        let low = rows_from(&text, 3, 17, 0);
        let (a, _) = settle(&mut t, &top, &s);
        let (b, _) = settle(&mut t, &low, &s);
        let inside = |p: &Placed, rows: &[Row]| (3..13).contains(&rows[p.row].line);
        let mut x: Vec<_> = a
            .placed
            .iter()
            .filter(|p| inside(p, &top))
            .map(|p| key_of(p, &top))
            .collect();
        let mut y: Vec<_> = b
            .placed
            .iter()
            .filter(|p| inside(p, &low))
            .map(|p| key_of(p, &low))
            .collect();
        x.sort_by_key(|k| (k.0, k.1, format!("{:?}", k.2)));
        y.sort_by_key(|k| (k.0, k.1, format!("{:?}", k.2)));
        assert!(x.len() > 5);
        assert_eq!(x, y);
    }

    #[test]
    fn scrolling_sideways_keeps_columns() {
        let text = "lattice thrifty after first office fifth staff";
        let mut t = tables();
        let s = settings(0.5, 1);
        let wide = [Row {
            line: 0,
            start_col: 0,
            text,
        }];
        let cut: String = text.chars().skip(8).collect();
        let narrow = [Row {
            line: 0,
            start_col: 8,
            text: &cut,
        }];
        let (a, _) = settle(&mut t, &wide, &s);
        let (b, _) = settle(&mut t, &narrow, &s);
        let doc = |sel: &Selection, rows: &[Row]| -> Vec<_> {
            sel.placed
                .iter()
                .map(|p| (rows[p.row].start_col + p.col, p.rel))
                .filter(|(c, _)| *c >= 9)
                .collect()
        };
        let (mut x, mut y) = (doc(&a, &wide), doc(&b, &narrow));
        x.sort_by_key(|k| (k.0, format!("{:?}", k.1)));
        y.sort_by_key(|k| (k.0, format!("{:?}", k.1)));
        assert!(!x.is_empty());
        assert_eq!(x, y);
    }

    #[test]
    fn density_is_keyed_by_document_position() {
        let text = "staff fifth after first office staff lattice fifth ".repeat(4);
        let text = text.as_str();
        let mut t = tables();
        let s = settings(0.5, 1);
        let place = |t: &mut Tables, at: usize| {
            let mut rows: Vec<Row> = (0..=at)
                .map(|i| Row {
                    line: 100 + i,
                    start_col: 0,
                    text: "",
                })
                .collect();
            rows[at] = Row {
                line: 7,
                start_col: 0,
                text,
            };
            let (sel, _) = settle(t, &rows, &s);
            let mut v: Vec<_> = sel
                .placed
                .iter()
                .filter(|p| matches!(p.rel, Relation::S(_)))
                .map(|p| (p.col, p.rel, p.entry.keys.clone()))
                .collect();
            v.sort_by_key(|k| (k.0, format!("{:?}", k.1)));
            v
        };
        let a = place(&mut t, 0);
        let b = place(&mut t, 5);
        assert!(!a.is_empty());
        assert_eq!(a, b);
        // Not everything is accepted at density 0.5.
        let full = {
            let rows = [Row {
                line: 7,
                start_col: 0,
                text,
            }];
            let (sel, _) = settle(&mut t, &rows, &settings(1.0, 1));
            sel.placed
                .iter()
                .filter(|p| matches!(p.rel, Relation::S(_)))
                .count()
        };
        assert!(full > a.len(), "{full} vs {}", a.len());
    }

    #[test]
    fn spacing_reaches_across_up_to_n_empty_cells() {
        let mut t = tables();
        let rows = [Row {
            line: 0,
            start_col: 0,
            text: "act  fifth",
        }];
        let across = |t: &mut Tables, n: u8| -> Vec<Relation> {
            let (sel, _) = settle(t, &rows, &settings(1.0, n));
            sel.placed
                .iter()
                .map(|p| p.rel)
                .filter(|r| matches!(r, Relation::S(_)))
                .collect()
        };
        assert!(across(&mut t, 1).is_empty());
        assert!(across(&mut t, 2).contains(&Relation::S(2)));
    }

    #[test]
    fn unknown_characters_are_empty_cells() {
        let mut t = tables();
        let rows = [
            Row {
                line: 0,
                start_col: 0,
                text: "ab\t😀cd",
            },
            Row {
                line: 1,
                start_col: 0,
                text: "ab\t😀cd",
            },
        ];
        let (sel, _) = settle(&mut t, &rows, &settings(1.0, 2));
        assert!(!sel.placed.is_empty());
        for p in &sel.placed {
            for (_, c) in [(p.row, p.col), h_cell(p)] {
                assert!(c != 2 && c != 3, "{p:?} touches an empty cell");
            }
        }
    }

    #[test]
    fn work_is_bounded_by_visible_cells() {
        let text: String = "lattice thrifty ".repeat(10).chars().take(80).collect();
        let rows = [Row {
            line: 3,
            start_col: 100_000,
            text: &text,
        }];
        let mut t = tables();
        let (sel, cells) = settle(&mut t, &rows, &settings(1.0, 1));
        assert_eq!(cells, 80);
        assert!(!sel.placed.is_empty());
    }
}
