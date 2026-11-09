//! The lace worker: builds pair tables off the UI thread.
//!
//! One worker and one pending request, as in `DocumentStats`: the UI thread runs
//! `layer::select` on the latest snapshot and forwards the pairs it still lacks; the
//! worker fills them, publishes a copy of the tables and wakes the UI. A request
//! for another face (or cell, or serif setting) drops the work in progress, so a
//! frame never gets tables built for a different face. [`LaceService::place`] keeps
//! the last selection while its rows, tables and settings stay the same, so a
//! redraw that changes nothing (a caret blink, an effect frame) selects nothing.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread::{self, JoinHandle};

use super::layer::{select, Placed, Row, Settings};
use super::table::{Relation, Tables};
use super::vectorize::FaceMetrics;
use crate::bitmap_fonts::BitmapFace;

/// How many keys' tables the worker keeps, so switching back (serifs, a font) reuses
/// them; the least recently used is dropped first.
const KEPT: usize = 4;

/// A bitmap strike: its records' address, width and height.
pub type FaceId = (usize, u16, u16);

/// Which tables a frame needs: the face, the cell in font pixels and whether serifs
/// count as strokes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceKey {
    pub id: FaceId,
    pub cell: [u16; 2],
    pub serifs: bool,
}

type Pair = (Relation, char, char);

struct Request {
    key: FaceKey,
    glyphs: BitmapFace,
    pairs: Vec<Pair>,
    epoch: u64,
}

#[derive(Default)]
struct Slots {
    pending: Option<Request>,
    published: Option<(FaceKey, Arc<Tables>)>,
    stop: bool,
}

struct Shared {
    slots: Mutex<Slots>,
    wake: Condvar,
    /// Bumped whenever the requested key changes; work from an older epoch is dropped.
    epoch: AtomicU64,
}

pub struct LaceService {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    /// The last request sent, so a frame that needs the same pairs sends nothing.
    requested: Option<(FaceKey, Vec<Pair>)>,
    shown: Shown,
}

/// The last selection and what it was made from.
#[derive(Default)]
struct Shown {
    made_from: Option<(FaceKey, Option<Arc<Tables>>, Settings)>,
    rows: Vec<(usize, usize, String)>,
    placed: Vec<Placed>,
}

impl Shown {
    fn holds(
        &self,
        face: FaceKey,
        tables: &Option<Arc<Tables>>,
        s: Settings,
        rows: &[Row],
    ) -> bool {
        let same_tables = |t: &Option<Arc<Tables>>| match (t, tables) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        self.made_from
            .as_ref()
            .is_some_and(|(k, t, was)| *k == face && same_tables(t) && *was == s)
            && self.rows.len() == rows.len()
            && self
                .rows
                .iter()
                .zip(rows)
                .all(|(a, b)| a.0 == b.line && a.1 == b.start_col && a.2 == b.text)
    }
}

impl LaceService {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            slots: Mutex::new(Slots::default()),
            wake: Condvar::new(),
            epoch: AtomicU64::new(0),
        });
        let work = shared.clone();
        let worker = thread::spawn(move || {
            let mut kept: Vec<(FaceKey, Tables)> = Vec::new();
            loop {
                let request = {
                    let mut slots = work.slots.lock().unwrap();
                    while slots.pending.is_none() && !slots.stop {
                        slots = work.wake.wait(slots).unwrap();
                    }
                    if slots.stop {
                        return;
                    }
                    slots.pending.take().unwrap()
                };
                let current = || work.epoch.load(Ordering::Relaxed) == request.epoch;
                let Some(tables) = fill(&mut kept, &request, current) else {
                    continue;
                };
                let mut slots = work.slots.lock().unwrap();
                if !slots.stop && current() {
                    slots.published = Some((request.key, tables));
                    drop(slots);
                    wake();
                }
            }
        });
        Self {
            shared,
            worker: Some(worker),
            requested: None,
            shown: Shown::default(),
        }
    }

    /// The connections for `rows`, selected on the latest tables for `face` (empty
    /// tables for `metrics` until the worker publishes some) and requesting the pairs
    /// they lack. The last selection is reused while nothing it depends on changed.
    pub fn place(
        &mut self,
        face: FaceKey,
        glyphs: BitmapFace,
        metrics: FaceMetrics,
        rows: &[Row],
        settings: Settings,
    ) -> &[Placed] {
        let tables = self.tables(face);
        if !self.shown.holds(face, &tables, settings, rows) {
            let empty;
            let on = match &tables {
                Some(t) => t.as_ref(),
                None => {
                    empty = Tables::new(metrics, face.serifs, face.cell);
                    &empty
                }
            };
            let selection = select(rows, on, &settings);
            self.request(face, glyphs, &selection.missing);
            self.shown = Shown {
                made_from: Some((face, tables, settings)),
                rows: rows
                    .iter()
                    .map(|r| (r.line, r.start_col, r.text.to_owned()))
                    .collect(),
                placed: selection.placed,
            };
        }
        &self.shown.placed
    }

    /// Ask for `missing` in the tables of `face`, whose bitmaps are `glyphs`. A new
    /// key is always sent, even with nothing missing, so its tables get published.
    pub fn request(&mut self, face: FaceKey, glyphs: BitmapFace, missing: &[Pair]) {
        debug_assert_eq!(face.id, glyphs.id());
        let same_key = self.requested.as_ref().is_some_and(|(k, _)| *k == face);
        if same_key
            && (missing.is_empty() || self.requested.as_ref().is_some_and(|r| r.1 == missing))
        {
            return;
        }
        let epoch = if same_key {
            self.shared.epoch.load(Ordering::Relaxed)
        } else {
            self.shared.epoch.fetch_add(1, Ordering::Relaxed) + 1
        };
        let mut slots = self.shared.slots.lock().unwrap();
        slots.pending = Some(Request {
            key: face,
            glyphs,
            pairs: missing.to_vec(),
            epoch,
        });
        self.requested = Some((face, missing.to_vec()));
        self.shared.wake.notify_one();
    }

    /// The latest tables the worker published for `face`, if they are for it.
    pub fn tables(&self, face: FaceKey) -> Option<Arc<Tables>> {
        let slots = self.shared.slots.lock().unwrap();
        slots
            .published
            .as_ref()
            .filter(|(k, _)| *k == face)
            .map(|(_, t)| t.clone())
    }
}

/// Fill the request's pairs into the kept tables for its key and return a copy to
/// publish, or `None` when the face cannot be measured or the key went stale.
fn fill(
    kept: &mut Vec<(FaceKey, Tables)>,
    request: &Request,
    current: impl Fn() -> bool,
) -> Option<Arc<Tables>> {
    let glyphs = request.glyphs;
    let rows = |c: char| glyphs.glyph(c).map(<[u8]>::to_vec);
    let mut tables = match kept.iter().position(|(k, _)| *k == request.key) {
        Some(at) => kept.remove(at).1,
        None => {
            let metrics = FaceMetrics::measure(rows, glyphs.width, glyphs.height).ok()?;
            Tables::new(metrics, request.key.serifs, request.key.cell)
        }
    };
    let mut done = true;
    for &(rel, a, b) in &request.pairs {
        if !current() {
            done = false;
            break;
        }
        tables.fill(rel, a, b, &rows);
    }
    let snapshot = done.then(|| Arc::new(tables.clone()));
    // Filled entries stay valid for their key, even when the work was cut short.
    kept.push((request.key, tables));
    if kept.len() > KEPT {
        kept.remove(0);
    }
    snapshot
}

impl Drop for LaceService {
    fn drop(&mut self) {
        self.shared.epoch.fetch_add(1, Ordering::Relaxed);
        {
            let mut slots = self.shared.slots.lock().unwrap();
            slots.stop = true;
            slots.pending = None;
        }
        self.shared.wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::FontFamily;
    use crate::bitmap_fonts::face;
    use crate::lace::ligatures::Mech;
    use crate::lace::vectorize::FaceMetrics;
    use std::sync::mpsc::{channel, Receiver};
    use std::time::{Duration, Instant};

    fn key(f: BitmapFace) -> FaceKey {
        FaceKey {
            id: f.id(),
            cell: [f.width, f.height],
            serifs: false,
        }
    }

    fn service() -> (LaceService, Receiver<()>) {
        let (send, recv) = channel();
        let service = LaceService::new(move || {
            let _ = send.send(());
        });
        (service, recv)
    }

    /// Waits for wakes until `done` holds, failing after `limit`.
    fn wait_until(recv: &Receiver<()>, limit: Duration, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + limit;
        while !done() {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                recv.recv_timeout(left).is_ok(),
                "no wake within {limit:?} with the condition unmet"
            );
        }
    }

    #[test]
    fn worker_builds_tables_and_wakes() {
        let (mut s, recv) = service();
        let vga = face(FontFamily::Vga);
        s.request(key(vga), vga, &[(Relation::H, 'f', 'i')]);
        wait_until(&recv, Duration::from_secs(2), || {
            s.tables(key(vga)).is_some()
        });
        let t = s.tables(key(vga)).unwrap();
        let fi = t.get(Relation::H, 'f', 'i').expect("f,i filled");
        assert!(fi.iter().any(|e| e.mech == Mech::Hook), "{fi:?}");
    }

    #[test]
    fn stale_tables_are_ignored_after_a_face_change() {
        let (mut s, recv) = service();
        let (vga, bios) = (face(FontFamily::Vga), face(FontFamily::Bios));
        // Seconds of work, so the switch lands while the worker is still on VGA.
        let many: Vec<_> = [Relation::H, Relation::V, Relation::Dr, Relation::Dl]
            .into_iter()
            .flat_map(|rel| ('a'..='z').flat_map(move |a| ('a'..='z').map(move |b| (rel, a, b))))
            .collect();
        s.request(key(vga), vga, &many);
        // Let the worker start on it.
        std::thread::sleep(Duration::from_millis(50));
        s.request(key(bios), bios, &[(Relation::H, 'f', 'i')]);

        let rows = |c| bios.glyph(c).map(<[u8]>::to_vec);
        let metrics = FaceMetrics::measure(rows, bios.width, bios.height).unwrap();
        let mut want = Tables::new(metrics, false, [bios.width, bios.height]);
        want.fill(Relation::H, 'f', 'i', &rows);
        let want = want.get(Relation::H, 'f', 'i').unwrap().to_vec();

        // Every BIOS frame sees BIOS tables or none, never the VGA work.
        let mut check = || {
            let Some(t) = s.tables(key(bios)) else {
                return false;
            };
            assert_eq!(t.get(Relation::H, 'f', 'i'), Some(want.as_slice()));
            assert!(t.get(Relation::H, 'a', 'b').is_none(), "VGA pairs in BIOS");
            assert!(s.tables(key(vga)).is_none());
            true
        };
        wait_until(&recv, Duration::from_secs(5), &mut check);
        // Late VGA results must not replace the BIOS snapshot.
        let grace = Instant::now() + Duration::from_millis(300);
        while let Some(left) = grace.checked_duration_since(Instant::now()) {
            let _ = recv.recv_timeout(left);
            assert!(check(), "BIOS tables were replaced");
        }
        // The VGA batch was dropped, not finished: back on VGA, most of it is still unfilled.
        s.request(key(vga), vga, &[(Relation::H, 'a', 'a')]);
        wait_until(&recv, Duration::from_secs(5), || {
            s.tables(key(vga)).is_some()
        });
        let t = s.tables(key(vga)).unwrap();
        let filled = many
            .iter()
            .filter(|p| t.get(p.0, p.1, p.2).is_some())
            .count();
        assert!(filled < many.len() / 2, "{filled} of {} filled", many.len());
    }

    #[test]
    fn placing_requests_what_is_missing_and_keeps_an_unchanged_frame() {
        let (mut s, recv) = service();
        let vga = face(FontFamily::Vga);
        let rows = |c| vga.glyph(c).map(<[u8]>::to_vec);
        let metrics = FaceMetrics::measure(rows, vga.width, vga.height).unwrap();
        let settings = Settings {
            density: 1.0,
            spacing: 1,
            seed: 1,
        };
        let row = |text| Row {
            line: 0,
            start_col: 0,
            text,
        };
        // Nothing is filled yet: no connections, and the pairs go to the worker.
        assert!(s
            .place(key(vga), vga, metrics, &[row("fifth office")], settings)
            .is_empty());
        let mut first = std::ptr::null();
        wait_until(&recv, Duration::from_secs(5), || {
            let placed = s.place(key(vga), vga, metrics, &[row("fifth office")], settings);
            first = placed.as_ptr();
            !placed.is_empty()
        });
        // The same rows, tables and settings reuse the selection.
        let again = s.place(key(vga), vga, metrics, &[row("fifth office")], settings);
        assert_eq!(again.as_ptr(), first);
        let other = s.place(key(vga), vga, metrics, &[row("fifth offices")], settings);
        assert_ne!(other.as_ptr(), first);
    }
}
