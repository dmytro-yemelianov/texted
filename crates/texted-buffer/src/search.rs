//! Resumable, cancellable KMP scanning with constant match-storage cost.
use crate::BufferSnapshot;
use std::sync::atomic::{AtomicU64, Ordering};
pub const MAX_QUERY_BYTES: usize = 64 * 1024;
pub const SCAN_QUANTUM: usize = 4096;
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchError {
    Empty,
    TooLong,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    pub start: usize,
    pub end: usize,
    pub ordinal: usize,
}
#[derive(Debug, Clone, Copy)]
pub struct Checkpoint {
    session: u64,
    pub offset: usize,
    prefix: usize,
    pub count: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    Complete,
    Cancelled,
    Stopped,
    InvalidCheckpoint,
}
#[derive(Debug, Clone, Copy)]
pub struct ScanReport {
    pub state: ScanState,
    pub checkpoint: Checkpoint,
    pub bytes_scanned: usize,
}
pub struct SearchSession {
    pub snapshot: BufferSnapshot,
    query: Box<str>,
    failure: Vec<usize>,
    id: u64,
}
impl SearchSession {
    pub fn new(
        snapshot: BufferSnapshot,
        query: &str,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self, SearchError> {
        if query.is_empty() {
            return Err(SearchError::Empty);
        }
        if query.len() > MAX_QUERY_BYTES {
            return Err(SearchError::TooLong);
        }
        let bytes = query.as_bytes();
        let mut failure = vec![0; bytes.len()];
        let mut prefix = 0;
        for i in 0..bytes.len() {
            if i % SCAN_QUANTUM == 0 && cancelled() {
                return Err(SearchError::Cancelled);
            }
            if i == 0 {
                continue;
            }
            while prefix > 0 && bytes[i] != bytes[prefix] {
                prefix = failure[prefix - 1];
            }
            if bytes[i] == bytes[prefix] {
                prefix += 1;
            }
            failure[i] = prefix;
        }
        Ok(Self {
            snapshot,
            query: query.into(),
            failure,
            id: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
        })
    }
    pub fn auxiliary_bytes(&self) -> usize {
        self.query.len() + self.failure.capacity() * std::mem::size_of::<usize>()
    }
    pub fn start(&self) -> Checkpoint {
        Checkpoint {
            session: self.id,
            offset: 0,
            prefix: 0,
            count: 0,
        }
    }
    /// Checkpoints retain the global non-overlap phase, including partial matches across chunks.
    pub fn scan(
        &self,
        at: Checkpoint,
        cancelled: impl FnMut() -> bool,
        visit: impl FnMut(SearchMatch) -> bool,
        checkpoint: impl FnMut(Checkpoint),
    ) -> ScanReport {
        self.scan_until(at, self.snapshot.total_bytes, cancelled, visit, checkpoint)
    }
    pub fn scan_until(
        &self,
        mut at: Checkpoint,
        stop_at: usize,
        mut cancelled: impl FnMut() -> bool,
        mut visit: impl FnMut(SearchMatch) -> bool,
        mut checkpoint: impl FnMut(Checkpoint),
    ) -> ScanReport {
        let begin = at.offset;
        let report = |state, at: Checkpoint| ScanReport {
            state,
            bytes_scanned: at.offset.saturating_sub(begin),
            checkpoint: at,
        };
        if at.session != self.id
            || at.offset > self.snapshot.total_bytes
            || at.prefix >= self.query.len()
        {
            return report(ScanState::InvalidCheckpoint, at);
        }
        if cancelled() {
            return report(ScanState::Cancelled, at);
        }
        let pattern = self.query.as_bytes();
        for byte in self
            .snapshot
            .content
            .bytes_at(at.offset)
            .take(stop_at.saturating_sub(at.offset))
        {
            while at.prefix > 0 && byte != pattern[at.prefix] {
                at.prefix = self.failure[at.prefix - 1];
            }
            if byte == pattern[at.prefix] {
                at.prefix += 1;
            }
            at.offset += 1;
            if at.prefix == pattern.len() {
                let found = SearchMatch {
                    start: at.offset - pattern.len(),
                    end: at.offset,
                    ordinal: at.count,
                };
                at.count += 1;
                at.prefix = 0;
                if !visit(found) {
                    return report(ScanState::Stopped, at);
                }
            }
            if (at.offset - begin) % SCAN_QUANTUM == 0 {
                checkpoint(at);
                if cancelled() {
                    return report(ScanState::Cancelled, at);
                }
            }
        }
        checkpoint(at);
        report(
            if cancelled() {
                ScanState::Cancelled
            } else if at.offset == self.snapshot.total_bytes {
                ScanState::Complete
            } else {
                ScanState::Stopped
            },
            at,
        )
    }
    pub fn seek(
        &self,
        start: usize,
        forward: bool,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Option<SearchMatch>, SearchError> {
        let mut first = None;
        let mut last = None;
        let mut found = None;
        let report = self.scan(
            self.start(),
            &mut cancelled,
            |m| {
                first.get_or_insert(m);
                last = Some(m);
                if forward && m.start >= start {
                    found = Some(m);
                    return false;
                }
                if !forward && m.start < start {
                    found = Some(m);
                }
                if !forward && m.start >= start && found.is_some() {
                    return false;
                }
                true
            },
            |_| {},
        );
        if report.state == ScanState::Cancelled {
            return Err(SearchError::Cancelled);
        }
        Ok(found.or(if forward { first } else { last }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextBuffer;
    use std::cell::Cell;
    #[test]
    fn streaming_matches_agree_with_reference_and_checkpoints_preserve_non_overlap() {
        let mut seed = 7u64;
        for _ in 0..100 {
            let mut text = String::new();
            for _ in 0..300 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                text.push(['a', 'b', 'ї', '\n'][(seed >> 32) as usize % 4]);
            }
            let snap = TextBuffer::from_string(text.clone()).create_snapshot();
            for query in ["a", "aa", "ї", "ї\nb", "\n", "ba"] {
                let expected: Vec<_> = text
                    .match_indices(query)
                    .map(|(a, s)| (a, a + s.len()))
                    .collect();
                let session = SearchSession::new(snap.clone(), query, || false).unwrap();
                let mut at = session.start();
                let mut actual = Vec::new();
                loop {
                    let r = session.scan(
                        at,
                        || false,
                        |m| {
                            actual.push((m.start, m.end));
                            false
                        },
                        |_| {},
                    );
                    at = r.checkpoint;
                    if r.state == ScanState::Complete {
                        break;
                    }
                }
                assert_eq!(actual, expected);
            }
        }
    }
    #[test]
    fn cancellation_bounds_work_and_can_resume_inside_utf8_and_a_pattern() {
        let snap = TextBuffer::from_string("їa\n".repeat(20_000)).create_snapshot();
        let session = SearchSession::new(snap, "їa\nї", || false).unwrap();
        let cancel = Cell::new(false);
        let mut matches = Vec::new();
        let r = session.scan(
            session.start(),
            || cancel.get(),
            |m| {
                matches.push((m.start, m.end));
                cancel.set(true);
                true
            },
            |_| {},
        );
        assert_eq!(r.state, ScanState::Cancelled);
        assert!(r.bytes_scanned <= SCAN_QUANTUM);
        let r = session.scan(
            r.checkpoint,
            || false,
            |m| {
                matches.push((m.start, m.end));
                true
            },
            |_| {},
        );
        assert_eq!(r.state, ScanState::Complete);
        assert_eq!(matches, session.snapshot.find_matches("їa\nї"));
    }
    #[test]
    fn dense_count_is_bounded_and_foreign_checkpoints_are_rejected() {
        let snap = TextBuffer::from_string("a".repeat(2_000_000)).create_snapshot();
        let a = SearchSession::new(snap.clone(), "a", || false).unwrap();
        let b = SearchSession::new(snap, "aa", || false).unwrap();
        let r = a.scan(a.start(), || false, |_| true, |_| {});
        assert_eq!(r.checkpoint.count, 2_000_000);
        assert!(a.auxiliary_bytes() < 64);
        assert_eq!(
            b.scan(r.checkpoint, || false, |_| true, |_| {}).state,
            ScanState::InvalidCheckpoint
        );
        assert!(matches!(
            SearchSession::new(a.snapshot.clone(), &"x".repeat(MAX_QUERY_BYTES + 1), || {
                false
            }),
            Err(SearchError::TooLong)
        ));
        assert!(matches!(
            SearchSession::new(a.snapshot.clone(), "x", || true),
            Err(SearchError::Cancelled)
        ));
    }
    #[test]
    fn seeks_wrap_without_a_complete_match_vector() {
        let s = SearchSession::new(
            TextBuffer::from_string("aaaaїaa".into()).create_snapshot(),
            "aa",
            || false,
        )
        .unwrap();
        assert_eq!(s.seek(1, true, || false).unwrap().unwrap().start, 2);
        assert_eq!(s.seek(20, true, || false).unwrap().unwrap().start, 0);
        assert_eq!(s.seek(0, false, || false).unwrap().unwrap().start, 6);
        assert_eq!(s.seek(2, false, || false).unwrap().unwrap().start, 0);
    }
}
