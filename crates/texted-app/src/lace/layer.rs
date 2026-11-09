//! Selection of ligatures over the visible grid.
//!
//! Only the visible cells are looked at, tables are never computed here (pairs
//! and glyphs that are not filled yet are reported in [`Selection::missing`]), and
//! every random choice is keyed by document position, so scrolling does not change
//! which connections are drawn. A table entry only knows its two cells, so a
//! candidate is also checked against the ink of the other cells around its pair.
//!
//! Each row chooses its in-row ligatures on its own, and each band (a row and the
//! next) chooses its ligatures against the claims of those two rows only. A
//! [`Selector`] keeps both and, on the next frame, works out again only the rows
//! whose text changed and the bands next to them: scrolling and typing cost a few
//! rows, not the window.

use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use super::connectors::Glyph;
use super::geom::{dist_prim, V2};
use super::ligatures::{Key, Side};
use super::table::{Entry, Need, Probe, Relation, Tables, CLEARANCE};

/// One visible row, top to bottom; matches `VisibleLine`.
pub struct Row<'a> {
    pub line: usize,
    pub start_col: usize,
    pub text: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Pairs and glyphs the worker still has to fill, sorted, without duplicates.
    pub missing: Vec<Need>,
}

/// What the last [`Selector::select`] worked out afresh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    /// Rows whose in-row ligatures were chosen again.
    pub rows: usize,
    /// Bands (a row and the next) whose ligatures were chosen again.
    pub bands: usize,
    /// Cells of the rows chosen again.
    pub cells: usize,
}

/// A cell of a candidate's pair: its row within the pair (0 the upper or only row,
/// 1 the lower) and its screen column.
type Cell = (u8, usize);

/// FxHash's multiply-and-rotate: claimed resources are a few small integers, and
/// SipHash would be half of the selection's time. Claim sets are never iterated in
/// an order that matters.
type Quick = std::hash::BuildHasherDefault<QuickHasher>;

#[derive(Clone, Copy, Default)]
struct QuickHasher(u64);

impl QuickHasher {
    fn add(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl std::hash::Hasher for QuickHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.add(u64::from(b));
        }
    }
    fn write_u8(&mut self, n: u8) {
        self.add(u64::from(n));
    }
    fn write_u16(&mut self, n: u16) {
        self.add(u64::from(n));
    }
    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }
    fn write_isize(&mut self, n: isize) {
        self.add(n as u64);
    }
}

/// A set of resources, hashed with [`Quick`].
type Claims = HashSet<Res, Quick>;

/// A resource a candidate uses up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Res {
    Key(u8, Cell, u16, u8),
    Pair(Cell, Cell),
    Slot(Cell, u8),
    /// Between two rows, the column pair `(2n, 2n + 1)` of the upper row's document
    /// columns.
    Band(usize),
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

/// A table entry placed on a pair of cells: `g` and `h` are screen columns, in one
/// row or (with a band slot) in a row and the next.
struct Cand<'t> {
    g: usize,
    h: usize,
    rel: Relation,
    entry: &'t Arc<Entry>,
    hash: f64,
}

impl Cand<'_> {
    /// Everything accepting the candidate uses up: its connector keys, the pair, one
    /// side slot of each letter and, between rows, the band slot `band`.
    fn uses(&self, band: Option<usize>) -> impl Iterator<Item = Res> + Clone + '_ {
        let (g, h) = ((0, self.g), (u8::from(band.is_some()), self.h));
        let (a, b) = if band.is_some() {
            (b'D', b'U')
        } else {
            (b'F', b'B')
        };
        let more = [
            Some(Res::Pair(g, h)),
            Some(Res::Slot(g, a)),
            Some(Res::Slot(h, b)),
            band.map(Res::Band),
        ];
        self.entry
            .keys
            .iter()
            .map(move |&k| key_res(k, g, h))
            .chain(more.into_iter().flatten())
    }
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

/// splitmix64 folded over (line, document column, relation): the part of
/// [`unit_hash`] a cell's entries share.
fn cell_hash(line: usize, doc_col: usize, rel: Relation) -> u64 {
    [line as u64, doc_col as u64, rel_code(rel)]
        .into_iter()
        .fold(0, |h, v| splitmix64(h ^ v))
}

/// splitmix64 folded over (line, document column, relation, entry index, seed),
/// continued from the cell's part, in [0, 1).
fn unit_hash(cell: u64, entry: usize, seed: u32) -> f64 {
    let h = splitmix64(splitmix64(cell ^ entry as u64) ^ u64::from(seed));
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// What one row or band chose.
#[derive(Default)]
struct Chosen {
    placed: Vec<(usize, Relation, Arc<Entry>)>,
    /// Connector keys the ligatures claimed, as a row's own (row 0 of the pair).
    claims: Claims,
    /// Whether a checked path could reach the row above (`[0]`) or below (`[1]`).
    reach: [bool; 2],
    missing: Vec<Need>,
}

/// The rows a selection looks at, with their characters split out on first use.
struct Grid<'a> {
    rows: &'a [Row<'a>],
    chars: Vec<OnceCell<Vec<char>>>,
    tables: &'a Tables,
    s: &'a Settings,
}

impl<'a> Grid<'a> {
    fn chars(&self, r: usize) -> &[char] {
        self.chars[r].get_or_init(|| self.rows[r].text.chars().collect())
    }

    fn letter(&self, r: usize, c: usize) -> Option<char> {
        self.chars(r).get(c).copied().filter(|c| c.is_alphabetic())
    }

    /// The entries for the letters at `g` and `h` (row, column) in `rel`.
    fn add(
        &self,
        out: &mut Vec<Cand<'a>>,
        missing: &mut BTreeSet<Need>,
        rel: Relation,
        g: (usize, usize),
        h: (usize, usize),
    ) {
        let (Some(a), Some(b)) = (self.letter(g.0, g.1), self.letter(h.0, h.1)) else {
            return;
        };
        let tables: &'a Tables = self.tables;
        let Some(list) = tables.get(rel, a, b) else {
            missing.insert(Need::Pair(rel, a, b));
            return;
        };
        let row = &self.rows[g.0];
        let cell = cell_hash(row.line, row.start_col + g.1, rel);
        out.extend(list.iter().enumerate().map(|(i, entry)| Cand {
            g: g.1,
            h: h.1,
            rel,
            entry,
            hash: unit_hash(cell, i, self.s.seed),
        }));
    }

    /// The in-row ligatures of row `r`.
    fn in_row(&self, r: usize) -> Chosen {
        let chars = self.chars(r);
        let mut missing = BTreeSet::new();
        let mut cands = Vec::new();
        for c in 0..chars.len() {
            if self.letter(r, c).is_none() {
                continue;
            }
            self.add(&mut cands, &mut missing, Relation::H, (r, c), (r, c + 1));
            for n in 1..=self.s.spacing {
                let last = c + usize::from(n);
                if chars.get(last) != Some(&' ') {
                    break;
                }
                self.add(
                    &mut cands,
                    &mut missing,
                    Relation::S(n),
                    (r, c),
                    (r, last + 1),
                );
            }
        }
        sort(&mut cands);
        let density = f64::from(self.s.density);
        let mut used = Claims::default();
        let mut chosen = Chosen::default();
        for c in &cands {
            if c.rel != Relation::H && c.hash >= density {
                continue;
            }
            let uses = c.uses(None);
            if uses.clone().any(|u| used.contains(&u))
                || !self.clear(c, r, r, &mut missing, &mut chosen.reach)
            {
                continue;
            }
            used.extend(uses);
            chosen.placed.push((c.g, c.rel, c.entry.clone()));
        }
        used.retain(|u| matches!(u, Res::Key(..)));
        chosen.claims = used;
        chosen.missing = missing.into_iter().collect();
        chosen
    }

    /// The ligatures between row `r` and the next, against the connector keys the two
    /// rows' in-row ligatures claimed (`claims[0]` above, `claims[1]` below) and
    /// their own.
    fn between(&self, r: usize, claims: [&Claims; 2]) -> Chosen {
        let mut missing = BTreeSet::new();
        let mut cands = Vec::new();
        for c in 0..self.chars(r).len() {
            if self.letter(r, c).is_none() {
                continue;
            }
            self.add(&mut cands, &mut missing, Relation::V, (r, c), (r + 1, c));
            self.add(
                &mut cands,
                &mut missing,
                Relation::Dr,
                (r, c),
                (r + 1, c + 1),
            );
            if c > 0 {
                self.add(
                    &mut cands,
                    &mut missing,
                    Relation::Dl,
                    (r, c),
                    (r + 1, c - 1),
                );
            }
        }
        sort(&mut cands);
        let density = f64::from(self.s.density);
        let start_col = self.rows[r].start_col;
        let mut used = Claims::default();
        let mut chosen = Chosen::default();
        for c in &cands {
            if c.hash >= density {
                continue;
            }
            let uses = c.uses(Some((start_col + c.g) / 2));
            let taken = |u: Res| match u {
                Res::Key(kind, (row, col), i, e) => {
                    claims[usize::from(row)].contains(&Res::Key(kind, (0, col), i, e))
                }
                _ => false,
            };
            if uses.clone().any(|u| used.contains(&u) || taken(u))
                || !self.clear(c, r, r + 1, &mut missing, &mut chosen.reach)
            {
                continue;
            }
            used.extend(uses);
            chosen.placed.push((c.g, c.rel, c.entry.clone()));
        }
        chosen.missing = missing.into_iter().collect();
        chosen
    }

    /// The prototype's `clearOfOthers` for a candidate whose upper cell is in row
    /// `upper` and lower cell in row `lower`: the path's probe points stay at least
    /// [`CLEARANCE`] from the strokes of every other occupied (non-blank) cell in the
    /// rows and columns around the pair. The row above the pair and the row below it
    /// are read only when the probe reaches them ([`Probe::reach`], recorded in
    /// `reach`); nothing there could block it otherwise. Rows outside `rows` count as
    /// empty. A neighbour whose strokes are not worked out yet blocks the path and is
    /// listed as missing.
    fn clear(
        &self,
        c: &Cand,
        upper: usize,
        lower: usize,
        missing: &mut BTreeSet<Need>,
        reach: &mut [bool; 2],
    ) -> bool {
        let probe = &c.entry.probe;
        if probe.points.is_empty() {
            return true;
        }
        reach[0] |= probe.reach[0];
        reach[1] |= probe.reach[1];
        let first = upper.saturating_sub(usize::from(probe.reach[0]));
        let last = (lower + usize::from(probe.reach[1])).min(self.rows.len() - 1);
        let (left, right) = (c.g.min(c.h).saturating_sub(1), c.g.max(c.h) + 1);
        let mut clear = true;
        for r in first..=last {
            let chars = self.chars(r);
            for col in left..=right {
                let Some(&ch) = chars.get(col) else {
                    break;
                };
                if (r == upper && col == c.g) || (r == lower && col == c.h) || ch.is_whitespace() {
                    continue;
                }
                let glyph = match self.tables.strokes(ch) {
                    Some(Some(glyph)) => glyph,
                    Some(None) => continue,
                    None => {
                        missing.insert(Need::Glyph(ch));
                        clear = false;
                        continue;
                    }
                };
                let at = self
                    .tables
                    .offset(r as isize - upper as isize, col as isize - c.g as isize);
                if blocks(glyph, at, probe) {
                    return false;
                }
            }
        }
        clear
    }
}

/// Mechanism order first, then cost, then the seeded hash.
fn sort(cands: &mut [Cand]) {
    cands.sort_by(|a, b| {
        a.entry
            .mech
            .order()
            .cmp(&b.entry.mech.order())
            .then(a.entry.cost.total_cmp(&b.entry.cost))
            .then(a.hash.total_cmp(&b.hash))
    });
}

/// Whether a probe point lies within [`CLEARANCE`] of `glyph` drawn at `at`.
fn blocks(glyph: &Glyph, at: V2, probe: &Probe) -> bool {
    // The box is sampled on arcs; the extra 0.1 covers what sampling misses.
    let pad = CLEARANCE + 0.1;
    let b = glyph.bbox;
    let (x0, y0, x1, y1) = (
        b[0] + at[0] - pad,
        b[1] + at[1] - pad,
        b[2] + at[0] + pad,
        b[3] + at[1] + pad,
    );
    let p = probe.bbox;
    if p[2] < x0 || p[0] > x1 || p[3] < y0 || p[1] > y1 {
        return false;
    }
    probe.points.iter().any(|p| {
        (x0..=x1).contains(&p[0])
            && (y0..=y1).contains(&p[1])
            && glyph
                .prims
                .iter()
                .any(|q| dist_prim(q, [p[0] - at[0], p[1] - at[1]]) < CLEARANCE)
    })
}

/// A row's identity: document line and first column.
type RowId = (usize, usize);

/// A neighbouring row as a choice saw it; `None` when there was no row.
type Seen = Option<(RowId, String)>;

fn seen(row: Option<&Row>) -> Seen {
    row.map(|r| ((r.line, r.start_col), r.text.to_owned()))
}

/// Whether `row` is still what a choice that depended on it saw (`Some`); a choice
/// that did not depend on it (`None`) holds whatever is there.
fn holds(dep: &Option<Seen>, row: Option<&Row>) -> bool {
    match (dep, row) {
        (None, _) => true,
        (Some(None), row) => row.is_none(),
        (Some(Some((id, text))), Some(r)) => *id == (r.line, r.start_col) && text == r.text,
        (Some(Some(_)), None) => false,
    }
}

/// A row's in-row choice and what it was made from.
struct RowMemo {
    text: String,
    /// Changes whenever the row is chosen again; bands remember the versions they
    /// were chosen against.
    version: u64,
    /// The rows above and below, when a choice here depended on their ink.
    near: [Option<Seen>; 2],
    chosen: Chosen,
}

/// A band's choice and what it was made from.
struct BandMemo {
    /// The two rows' versions.
    rows: [u64; 2],
    /// The rows above and below the band, when a choice depended on their ink.
    near: [Option<Seen>; 2],
    chosen: Chosen,
}

/// Selection that keeps each row's and each band's choice for the next frame. A row
/// is chosen again when its line, first column or text changes, or a row its choice
/// depended on (through the clearance check) changes; a band when either of its rows
/// was chosen again or a row beyond it that it depended on changes. Everything is
/// chosen again when the settings change or the tables are not a later state of the
/// same tables; when the tables only grew, just the rows and bands that missed
/// something are. The result is always what a fresh [`select`] gives.
#[derive(Default)]
pub struct Selector {
    made_with: Option<((u64, u64), Settings)>,
    rows: HashMap<RowId, RowMemo>,
    bands: HashMap<(RowId, RowId), BandMemo>,
    version: u64,
    work: Work,
}

impl Selector {
    /// Choose the ligatures for the visible rows.
    pub fn select(&mut self, rows: &[Row], tables: &Tables, s: &Settings) -> Selection {
        let stamp = tables.stamp();
        match self.made_with {
            Some((was, with)) if with == *s && was.0 == stamp.0 && was.1 <= stamp.1 => {
                if was.1 < stamp.1 {
                    self.rows.retain(|_, m| m.chosen.missing.is_empty());
                    self.bands.retain(|_, m| m.chosen.missing.is_empty());
                }
            }
            _ => {
                self.rows.clear();
                self.bands.clear();
            }
        }
        self.made_with = Some((stamp, *s));
        self.work = Work::default();
        let grid = Grid {
            rows,
            chars: rows.iter().map(|_| OnceCell::new()).collect(),
            tables,
            s,
        };
        let id = |r: &Row| (r.line, r.start_col);
        let near = |i: usize, by: isize| i.checked_add_signed(by).and_then(|j| rows.get(j));

        let mut row_memos: Vec<RowMemo> = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let (above, below) = (near(i, -1), near(i, 1));
            let kept = self.rows.remove(&id(row)).filter(|m| {
                m.text == row.text && holds(&m.near[0], above) && holds(&m.near[1], below)
            });
            let memo = kept.unwrap_or_else(|| {
                let chosen = grid.in_row(i);
                self.work.rows += 1;
                self.work.cells += grid.chars(i).len();
                self.version += 1;
                RowMemo {
                    text: row.text.to_owned(),
                    version: self.version,
                    near: [
                        chosen.reach[0].then(|| seen(above)),
                        chosen.reach[1].then(|| seen(below)),
                    ],
                    chosen,
                }
            });
            row_memos.push(memo);
        }

        let mut band_memos: Vec<BandMemo> = Vec::with_capacity(rows.len().saturating_sub(1));
        for i in 0..rows.len().saturating_sub(1) {
            let versions = [row_memos[i].version, row_memos[i + 1].version];
            let (above, below) = (near(i, -1), near(i, 2));
            let kept = self
                .bands
                .remove(&(id(&rows[i]), id(&rows[i + 1])))
                .filter(|m| {
                    m.rows == versions && holds(&m.near[0], above) && holds(&m.near[1], below)
                });
            let memo = kept.unwrap_or_else(|| {
                let claims = [&row_memos[i].chosen.claims, &row_memos[i + 1].chosen.claims];
                let chosen = grid.between(i, claims);
                self.work.bands += 1;
                BandMemo {
                    rows: versions,
                    near: [
                        chosen.reach[0].then(|| seen(above)),
                        chosen.reach[1].then(|| seen(below)),
                    ],
                    chosen,
                }
            });
            band_memos.push(memo);
        }

        let chosen = row_memos
            .iter()
            .map(|m| &m.chosen)
            .enumerate()
            .chain(band_memos.iter().map(|m| &m.chosen).enumerate());
        let mut placed = Vec::new();
        let mut missing = BTreeSet::new();
        for (row, c) in chosen {
            placed.extend(c.placed.iter().map(|(col, rel, entry)| Placed {
                row,
                col: *col,
                rel: *rel,
                entry: entry.clone(),
            }));
            missing.extend(c.missing.iter().copied());
        }

        self.rows = rows.iter().map(id).zip(row_memos).collect();
        self.bands = rows
            .windows(2)
            .map(|w| (id(&w[0]), id(&w[1])))
            .zip(band_memos)
            .collect();
        Selection {
            placed,
            missing: missing.into_iter().collect(),
        }
    }

    /// What the last selection worked out afresh.
    pub fn work(&self) -> Work {
        self.work
    }
}

/// Choose the ligatures for the visible rows, keeping nothing.
pub fn select(rows: &[Row], tables: &Tables, s: &Settings) -> Selection {
    Selector::default().select(rows, tables, s)
}

/// `select`, also returning the number of cells that were examined.
#[cfg(test)]
fn run(rows: &[Row], tables: &Tables, s: &Settings) -> (Selection, usize) {
    let mut selector = Selector::default();
    let selection = selector.select(rows, tables, s);
    (selection, selector.work().cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::FontFamily;
    use crate::bitmap_fonts::face;
    use crate::lace::vectorize::FaceMetrics;
    use std::collections::BTreeMap;

    /// A cell as (row in `rows`, screen column).
    type Cell = (usize, usize);

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
            for need in sel.missing {
                t.supply(need, &rows_of);
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
        settings_seed(density, spacing, 3)
    }

    fn settings_seed(density: f32, spacing: u8, seed: u32) -> Settings {
        Settings {
            density,
            spacing,
            seed,
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

    /// Release timings: `cargo test --release -p texted-app --lib lace::layer::tests::timing
    /// -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing() {
        for (nrows, cols) in [(40, 100), (60, 190), (120, 300)] {
            let mut text = fill_grid(&WORDS, cols, nrows + 1, 5);
            let mut t = tables();
            let s = settings(0.6, 1);
            settle(&mut t, &rows_from(&text, 0, nrows + 1, 0), &s);
            let n = 20;
            let time = |f: &mut dyn FnMut()| {
                let start = std::time::Instant::now();
                for _ in 0..n {
                    f();
                }
                start.elapsed() / n
            };
            let rows = rows_from(&text, 0, nrows, 0);
            let full = time(&mut || {
                std::hint::black_box(select(&rows, &t, &s));
            });
            // Nothing changed.
            let mut sel = Selector::default();
            sel.select(&rows, &t, &s);
            let same = time(&mut || {
                std::hint::black_box(sel.select(&rows, &t, &s));
            });
            // Scrolling by one row back and forth.
            let (a, b) = (
                rows_from(&text, 0, nrows, 0),
                rows_from(&text, 1, nrows + 1, 0),
            );
            sel.select(&a, &t, &s);
            let mut flip = false;
            let scroll = time(&mut || {
                flip = !flip;
                std::hint::black_box(sel.select(if flip { &b } else { &a }, &t, &s));
            });
            // A letter typed into the middle row and taken out again.
            let mid = nrows / 2;
            let plain = text[mid].clone();
            text[mid].insert(plain.find(' ').unwrap_or(0), 'f');
            let docs = [plain, text[mid].clone()];
            settle(&mut t, &rows_from(&text, 0, nrows, 0), &s);
            sel.select(&rows_from(&text, 0, nrows, 0), &t, &s);
            let mut k = 0;
            let edit = time(&mut || {
                k += 1;
                text[mid] = docs[k % 2].clone();
                std::hint::black_box(sel.select(&rows_from(&text, 0, nrows, 0), &t, &s));
            });
            eprintln!(
                "TIMING {nrows}x{cols}: full {full:?}, unchanged {same:?}, scroll one row \
                 {scroll:?}, type a letter {edit:?} ({:?})",
                sel.work()
            );
        }
    }

    type Shape = (Vec<(usize, usize, Relation, usize)>, Vec<Need>);

    /// A selection as comparable values: placements by entry identity, and needs.
    fn shape(sel: &Selection) -> Shape {
        let placed = sel
            .placed
            .iter()
            .map(|p| (p.row, p.col, p.rel, Arc::as_ptr(&p.entry) as usize))
            .collect();
        (placed, sel.missing.clone())
    }

    /// Selects with `sel` and afresh until nothing is missing, checking at every step
    /// (partly filled tables included) that both agree.
    fn agree(sel: &mut Selector, t: &mut Tables, rows: &[Row], s: &Settings, step: &str) {
        agree_in(&rows_of, sel, t, rows, s, step);
    }

    /// `agree` for tables of the face whose bitmaps `glyphs` gives.
    fn agree_in(
        glyphs: &impl Fn(char) -> Option<Vec<u8>>,
        sel: &mut Selector,
        t: &mut Tables,
        rows: &[Row],
        s: &Settings,
        step: &str,
    ) -> Selection {
        for _ in 0..10 {
            let kept = sel.select(rows, t, s);
            let fresh = select(rows, t, s);
            assert_eq!(shape(&kept), shape(&fresh), "{step}");
            if kept.missing.is_empty() {
                return kept;
            }
            for need in kept.missing {
                t.supply(need, glyphs);
            }
        }
        panic!("{step}: selection never settled");
    }

    #[test]
    fn a_row_is_chosen_again_when_the_ink_it_read_changes() {
        // In BIOS 8×8 a swash between these words passes close enough to the row above
        // for its letters to decide whether it may be drawn.
        let f = face(FontFamily::Bios);
        let glyphs = move |c: char| f.glyph(c).map(<[u8]>::to_vec);
        let m = FaceMetrics::measure(glyphs, f.width, f.height).unwrap();
        let mut t = Tables::new(m, false, [f.width, f.height]);
        let s = settings_seed(1.0, 1, 5);
        let (above, row) = ("ступінь jog ступінь", "quip quip quip");
        let blank = " ".repeat(above.chars().count());
        let rows = |top| {
            [
                Row {
                    line: 0,
                    start_col: 0,
                    text: top,
                },
                Row {
                    line: 1,
                    start_col: 0,
                    text: row,
                },
            ]
        };
        let own = |sel: &Selection| -> Vec<_> {
            let row = sel.placed.iter().filter(|p| p.row == 1);
            row.map(|p| (p.col, p.rel, Arc::as_ptr(&p.entry) as usize))
                .collect()
        };
        let mut sel = Selector::default();
        agree_in(&glyphs, &mut sel, &mut t, &rows(above), &s, "filling");
        let alone = agree_in(&glyphs, &mut sel, &mut t, &rows(&blank), &s, "alone");
        let under = agree_in(&glyphs, &mut sel, &mut t, &rows(above), &s, "under");
        // The row's text did not change, yet its choice did: it was made again with
        // the row above, which changed.
        assert_ne!(own(&alone), own(&under));
        assert_eq!((sel.work().rows, sel.work().bands), (2, 1));
    }

    /// Rows `first..first + 12` of `doc` seen from column `start_col`.
    fn view(doc: &[String], first: usize, start_col: usize) -> Vec<(usize, usize, String)> {
        (first..first + 12)
            .map(|i| (i, start_col, doc[i].chars().skip(start_col).collect()))
            .collect()
    }

    fn as_rows(view: &[(usize, usize, String)]) -> Vec<Row<'_>> {
        view.iter()
            .map(|(line, start_col, text)| Row {
                line: *line,
                start_col: *start_col,
                text,
            })
            .collect()
    }

    #[test]
    fn incremental_selection_matches_a_fresh_one() {
        let mut doc = fill_grid(&WORDS, 40, 30, 7);
        let mut t = tables();
        let mut s = settings(0.6, 2);
        let mut sel = Selector::default();
        let step = |sel: &mut Selector,
                    t: &mut Tables,
                    s: &Settings,
                    v: Vec<(usize, usize, String)>,
                    what: &str| agree(sel, t, &as_rows(&v), s, what);
        step(&mut sel, &mut t, &s, view(&doc, 0, 0), "first frame");
        step(&mut sel, &mut t, &s, view(&doc, 0, 0), "same frame");
        assert_eq!(sel.work(), Work::default());
        step(&mut sel, &mut t, &s, view(&doc, 1, 0), "scrolled one row");
        assert!(sel.work().rows <= 3, "{:?}", sel.work());
        step(
            &mut sel,
            &mut t,
            &s,
            view(&doc, 4, 0),
            "scrolled three more",
        );
        doc[8].insert(3, 'j');
        step(&mut sel, &mut t, &s, view(&doc, 4, 0), "a letter typed");
        doc[9] = format!("quiz {}", doc[9]);
        step(
            &mut sel,
            &mut t,
            &s,
            view(&doc, 4, 0),
            "a word with new pairs",
        );
        doc[10].clear();
        step(&mut sel, &mut t, &s, view(&doc, 4, 0), "a line emptied");
        doc.remove(6);
        step(&mut sel, &mut t, &s, view(&doc, 4, 0), "a line deleted");
        step(&mut sel, &mut t, &s, view(&doc, 2, 0), "scrolled back up");
        step(&mut sel, &mut t, &s, view(&doc, 2, 3), "scrolled sideways");
        s.density = 0.9;
        step(&mut sel, &mut t, &s, view(&doc, 2, 3), "density changed");
        s.seed = 11;
        step(&mut sel, &mut t, &s, view(&doc, 2, 3), "seed changed");
        let mut other = tables();
        step(&mut sel, &mut other, &s, view(&doc, 2, 3), "other tables");
        step(&mut sel, &mut t, &s, view(&doc, 2, 3), "first tables again");
    }

    /// On a 60 × 190 screen of `family`, types a letter into row 30 and returns what
    /// the selector worked out again, with what it had to: row 30 and its two bands,
    /// plus a neighbouring row (and the band beyond it) whose choice read row 30's
    /// ink, and a band beyond them that did. Checks the result against a fresh
    /// selection, also after the tables fill what the edit lacks.
    fn retyped(family: FontFamily) -> (Work, Work) {
        let f = face(family);
        let glyphs = move |c: char| f.glyph(c).map(<[u8]>::to_vec);
        let m = FaceMetrics::measure(glyphs, f.width, f.height).unwrap();
        let mut t = Tables::new(m, false, [f.width, f.height]);
        let mut text = fill_grid(&WORDS, 190, 60, 11);
        let s = settings(0.6, 1);
        let settled = |t: &mut Tables, rows: &[Row]| loop {
            let sel = select(rows, t, &s);
            if sel.missing.is_empty() {
                return;
            }
            for need in sel.missing {
                t.supply(need, &glyphs);
            }
        };
        settled(&mut t, &rows_from(&text, 0, 60, 0));
        let mut sel = Selector::default();
        sel.select(&rows_from(&text, 0, 60, 0), &t, &s);
        assert_eq!((sel.work().rows, sel.work().bands), (60, 59));
        sel.select(&rows_from(&text, 0, 60, 0), &t, &s);
        assert_eq!(sel.work(), Work::default(), "nothing changed");
        // What reads row 30 besides its own row and bands.
        let row = |r: usize, side: usize| sel.rows[&(r, 0)].near[side].is_some();
        let band = |r: usize, side: usize| sel.bands[&((r, 0), (r + 1, 0))].near[side].is_some();
        let (up, down) = (row(29, 1), row(31, 0));
        let must = Work {
            rows: 1 + usize::from(up) + usize::from(down),
            bands: 2 + usize::from(up || band(28, 1)) + usize::from(down || band(31, 0)),
            cells: 0,
        };
        // A letter typed into a word in the middle of row 30.
        let at = text[30].find(' ').unwrap();
        text[30].insert(at, 'f');
        let rows = rows_from(&text, 0, 60, 0);
        let got = sel.select(&rows, &t, &s);
        let typed = Work {
            cells: 0,
            ..sel.work()
        };
        assert_eq!(typed, must, "{}", family.name());
        assert_eq!(shape(&got), shape(&select(&rows, &t, &s)));
        // Filling what the edit lacks chooses again only what waited for it.
        for need in got.missing {
            t.supply(need, &glyphs);
        }
        let got = sel.select(&rows, &t, &s);
        let filled = sel.work();
        assert!(
            filled.rows <= must.rows && filled.bands <= must.bands,
            "{filled:?}"
        );
        assert_eq!(shape(&got), shape(&select(&rows, &t, &s)));
        (typed, filled)
    }

    #[test]
    fn one_changed_row_reselects_that_row_and_its_two_bands() {
        // EGA has no stroke that can reach another row: exactly the row and its bands.
        let (typed, _) = retyped(FontFamily::Ega);
        assert_eq!((typed.rows, typed.bands), (1, 2));
        // In VGA a swash may pass near the row above or below; a neighbour whose choice
        // read row 30's ink is chosen again too, and nothing else.
        let (typed, _) = retyped(FontFamily::Vga);
        assert!(typed.rows <= 3 && typed.bands <= 4, "{typed:?}");
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
        let mut t = tables();
        for seed in 1..=8 {
            let text = fill_grid(&WORDS, 48, 17, seed);
            let s = Settings {
                density: 0.6,
                spacing: 1,
                seed,
            };
            let top = rows_from(&text, 0, 14, 0);
            let low = rows_from(&text, 3, 17, 0);
            let (a, _) = settle(&mut t, &top, &s);
            let (b, _) = settle(&mut t, &low, &s);
            // In-row placements for document rows 3..13, band placements whose
            // upper row is 3..12.
            let inside = |p: &Placed, rows: &[Row]| {
                let line = rows[p.row].line;
                if matches!(p.rel, Relation::H | Relation::S(_)) {
                    (3..13).contains(&line)
                } else {
                    (3..12).contains(&line)
                }
            };
            let pick = |sel: &Selection, rows: &[Row]| {
                let mut v: Vec<_> = sel
                    .placed
                    .iter()
                    .filter(|p| inside(p, rows))
                    .map(|p| key_of(p, rows))
                    .collect();
                v.sort_by_key(|k| (k.0, k.1, format!("{:?}", k.2)));
                v
            };
            let (x, y) = (pick(&a, &top), pick(&b, &low));
            assert!(x.len() > 5, "seed {seed}");
            assert_eq!(x, y, "seed {seed}");
        }
    }

    #[test]
    fn bands_never_claim_the_same_connector() {
        let mut t = tables();
        for seed in 1..=8 {
            let text = fill_grid(&WORDS, 48, 14, seed);
            let rows = rows_from(&text, 0, 14, 0);
            let (sel, _) = settle(&mut t, &rows, &settings_seed(1.0, 1, seed));
            // Which group (in-row, or the band under row r) claimed each key.
            let mut owner: BTreeMap<String, Option<usize>> = BTreeMap::new();
            for p in &sel.placed {
                let group = match p.rel {
                    Relation::H | Relation::S(_) => None,
                    _ => Some(p.row),
                };
                let (g, h) = ((p.row, p.col), h_cell(p));
                for &k in &p.entry.keys {
                    // The key with its side replaced by the cell it names.
                    let id = match key_res(k, (0, 0), (1, 0)) {
                        Res::Key(kind, (side, _), i, e) => {
                            let cell = if side == 0 { g } else { h };
                            format!("{kind}{cell:?}{i}.{e}")
                        }
                        _ => unreachable!(),
                    };
                    let prev = owner.insert(id.clone(), group);
                    assert!(
                        prev.is_none_or(|o| o == group),
                        "seed {seed}: {id} claimed by {prev:?} and {group:?}"
                    );
                }
            }
        }
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
    fn a_tail_that_crosses_a_third_letter_is_rejected() {
        let mut t = tables();
        let s = settings(1.0, 1);
        let joined = |t: &mut Tables, top: &str| -> Vec<Placed> {
            let rows = [
                Row {
                    line: 0,
                    start_col: 0,
                    text: top,
                },
                Row {
                    line: 1,
                    start_col: 0,
                    text: " d",
                },
            ];
            settle(t, &rows, &s).0.placed
        };
        // Alone, g's descender reaches d below and to the right with a tail.
        let alone = joined(&mut t, "g ");
        let tail = alone
            .iter()
            .find(|p| (p.row, p.col, p.rel) == (0, 0, Relation::Dr))
            .expect("g joins d");
        assert_eq!(tail.entry.mech, crate::lace::ligatures::Mech::Tail);
        // With a p beside g nothing else claims g or d, yet the tail is rejected...
        assert!(joined(&mut t, "gp").is_empty());
        // ...because it passes through p's descender.
        let p = t.strokes('p').flatten().expect("p is vectorised");
        assert!(blocks(p, t.offset(0, 1), &tail.entry.probe));
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
